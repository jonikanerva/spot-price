//! Sign-in, sign-up, sign-out, the session info, and the API key routes of
//! the setup UI.
//!
//! The sign-in and sign-up answers keep the Better Auth bodies of the
//! TypeScript service: `{"redirect", "token", "user"}`, `{"token", "user"}`,
//! and errors as `{"message", "code"}`. The UI reads `message` or `error`.

use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use serde_json::Value;

use super::dto::{
    ApiKeyDto, AuthErrorDto, SessionDto, SessionInfoDto, SessionResponseDto, SignInDto, SignUpDto,
    SuccessDto, UserDto,
};
use super::input;
use super::session::{self, SignedIn};
use super::{AppState, PassThrough, rate_limit, wire};
use crate::auth::password::PasswordError;
use crate::auth::username::{Username, normalize};
use crate::auth::{
    MAX_PASSWORD_LENGTH, MAX_USERS, MIN_PASSWORD_LENGTH, internal_email, js_length, random,
};
use crate::store::accounts::{self, CreateAccountError, NewAccount, UserRow};
use crate::store::{api_keys, user_settings};

const INVALID_JSON: &str = "Invalid JSON payload";
const INVALID_CREDENTIALS_INPUT: &str =
    "username must match [a-z0-9_-] and be 3-32 chars; password is required";
const REGISTRATION_CLOSED: &str = "Registration is currently closed.";

const INVALID_EMAIL_OR_PASSWORD: AuthErrorDto = AuthErrorDto {
    message: "Invalid email or password",
    code: "INVALID_EMAIL_OR_PASSWORD",
};
const PASSWORD_TOO_SHORT: AuthErrorDto = AuthErrorDto {
    message: "Password too short",
    code: "PASSWORD_TOO_SHORT",
};
const PASSWORD_TOO_LONG: AuthErrorDto = AuthErrorDto {
    message: "Password too long",
    code: "PASSWORD_TOO_LONG",
};
const USER_ALREADY_EXISTS: AuthErrorDto = AuthErrorDto {
    message: "User already exists. Use another email.",
    code: "USER_ALREADY_EXISTS_USE_ANOTHER_EMAIL",
};
const FAILED_TO_CREATE_USER: AuthErrorDto = AuthErrorDto {
    message: "Failed to create user",
    code: "FAILED_TO_CREATE_USER",
};

/// A response that Hono passed through from Better Auth: no rate-limit
/// headers are added to it.
fn pass_through(mut response: Response) -> Response {
    response.extensions_mut().insert(PassThrough);
    response
}

fn auth_error(status: StatusCode, error: &AuthErrorDto) -> Response {
    pass_through(wire::json(status, error))
}

/// The login limit: 10 requests per 15 minutes per client.
pub async fn login_limit(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let key = format!("login:{}", rate_limit::client_key(request.headers()));
    let limiter = &state.limits.login;
    let decision = limiter.check(&key, state.now().as_millisecond());
    if !decision.allowed {
        return limiter.limited(&decision);
    }
    let mut response = next.run(request).await;
    if response.extensions().get::<PassThrough>().is_none() {
        rate_limit::set_headers(response.headers_mut(), &decision);
    }
    response
}

/// `POST /api/session/login-or-signup`: signs in an existing username, or
/// creates the account and signs it in.
pub async fn login_or_signup(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !session::same_origin(&state, &headers) {
        return session::forbidden();
    }
    // `c.req.json()` decodes UTF-8 with replacements and drops a byte order
    // mark, whatever the content type.
    let bytes = body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&body);
    let Some(payload) = input::parse_json(&String::from_utf8_lossy(bytes)) else {
        return wire::error(StatusCode::BAD_REQUEST, INVALID_JSON);
    };
    if !matches!(payload, Value::Object(_) | Value::Array(_)) {
        return wire::error(StatusCode::BAD_REQUEST, INVALID_JSON);
    }
    let username_text = match payload.get("username") {
        None | Some(Value::Null) => "",
        // A username that is not a string or null answers the generic 500,
        // like the v1 contract.
        Some(value) if input::is_infinite_number(value) => return wire::internal_error(),
        Some(Value::String(text)) => text.as_str(),
        Some(_) => return wire::internal_error(),
    };
    let username = Username::parse(&normalize(username_text));
    let password = payload.get("password").filter(|value| truthy(value));
    let (Some(username), Some(password)) = (username, password) else {
        return wire::error(StatusCode::BAD_REQUEST, INVALID_CREDENTIALS_INPUT);
    };
    let password = match password {
        Value::String(text) if !input::is_infinite_number(password) => text.clone(),
        other => {
            return auth_error(
                StatusCode::BAD_REQUEST,
                &AuthErrorDto {
                    message: password_type_message(other),
                    code: "VALIDATION_ERROR",
                },
            );
        }
    };
    let answer = match accounts::user_id_by_username(&state.pool, username.as_str()).await {
        Ok(Some(_)) => sign_in(&state, &username, password).await,
        Ok(None) => sign_up(&state, &username, password).await,
        Err(error) => {
            tracing::error!(code = error.code(), "a username could not be read");
            Err(())
        }
    };
    answer.unwrap_or_else(|()| wire::internal_error())
}

/// JavaScript truthiness of a JSON value.
fn truthy(value: &Value) -> bool {
    if input::is_infinite_number(value) {
        return true;
    }
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// The Better Auth validation text of a password that is not a string.
fn password_type_message(value: &Value) -> &'static str {
    if input::is_infinite_number(value) {
        return "[body.password] Invalid input: expected string, received number";
    }
    match value {
        Value::Bool(_) => "[body.password] Invalid input: expected string, received boolean",
        Value::Number(_) => "[body.password] Invalid input: expected string, received number",
        Value::Array(_) => "[body.password] Invalid input: expected string, received array",
        Value::Null | Value::String(_) | Value::Object(_) => {
            "[body.password] Invalid input: expected string, received object"
        }
    }
}

fn password_failed(error: PasswordError) {
    let code = match error {
        PasswordError::Random => "random",
        PasswordError::Scrypt => "scrypt",
        PasswordError::MalformedHash => "malformed_hash",
        PasswordError::Task => "hash_task",
    };
    tracing::error!(code, "a password hash failed");
}

/// The success answer of a sign-in or a sign-up: the body and the cookie of
/// a new session.
async fn signed_in<T: serde::Serialize>(
    state: &AppState,
    user: &UserRow,
    body: &T,
) -> Result<Response, ()> {
    let cookie = session::start(state, &user.id).await.map_err(|_| ())?;
    let mut response = pass_through(wire::json(StatusCode::OK, body));
    response.headers_mut().append(SET_COOKIE, cookie);
    Ok(response)
}

async fn sign_in(state: &AppState, username: &Username, password: String) -> Result<Response, ()> {
    let email = internal_email(username);
    let found = accounts::credential_user_by_email(&state.pool, &email)
        .await
        .map_err(|error| tracing::error!(code = error.code(), "an account could not be read"))?;
    let Some((user, Some(stored))) = found.map(|found| (found.user, found.password_hash)) else {
        // Hash anyway, so an unknown account takes the same time.
        state
            .passwords
            .hash(password)
            .await
            .map_err(password_failed)?;
        return Ok(auth_error(
            StatusCode::UNAUTHORIZED,
            &INVALID_EMAIL_OR_PASSWORD,
        ));
    };
    if !state
        .passwords
        .verify(stored, password)
        .await
        .map_err(password_failed)?
    {
        return Ok(auth_error(
            StatusCode::UNAUTHORIZED,
            &INVALID_EMAIL_OR_PASSWORD,
        ));
    }
    let body = SignInDto {
        redirect: false,
        token: None,
        user: UserDto::from(&user),
    };
    signed_in(state, &user, &body).await
}

async fn sign_up(state: &AppState, username: &Username, password: String) -> Result<Response, ()> {
    let users = accounts::count_users(&state.pool)
        .await
        .map_err(|error| tracing::error!(code = error.code(), "users could not be counted"))?;
    if users >= MAX_USERS {
        return Ok(wire::error(StatusCode::FORBIDDEN, REGISTRATION_CLOSED));
    }
    let length = js_length(&password);
    if length < MIN_PASSWORD_LENGTH {
        return Ok(auth_error(StatusCode::BAD_REQUEST, &PASSWORD_TOO_SHORT));
    }
    if length > MAX_PASSWORD_LENGTH {
        return Ok(auth_error(StatusCode::BAD_REQUEST, &PASSWORD_TOO_LONG));
    }
    let email = internal_email(username);
    let exists = accounts::email_exists(&state.pool, &email)
        .await
        .map_err(|error| tracing::error!(code = error.code(), "an account could not be read"))?;
    if exists {
        return Ok(auth_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            &USER_ALREADY_EXISTS,
        ));
    }
    let hash = state
        .passwords
        .hash(password)
        .await
        .map_err(password_failed)?;
    let (Ok(user_id), Ok(account_id)) = (random::better_auth_id(), random::better_auth_id()) else {
        tracing::error!(code = "random", "no random bytes for an account id");
        return Err(());
    };
    let created = accounts::create_account(
        &state.pool,
        NewAccount {
            user_id: &user_id,
            account_id: &account_id,
            username: username.as_str(),
            email: &email,
            password_hash: &hash,
            now: state.now(),
        },
    )
    .await;
    let user = match created {
        Ok(user) => user,
        Err(CreateAccountError::Conflict) => {
            return Ok(auth_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                &FAILED_TO_CREATE_USER,
            ));
        }
        Err(CreateAccountError::Store(error)) => {
            tracing::error!(code = error.code(), "an account could not be created");
            return Err(());
        }
    };
    let body = SignUpDto {
        token: None,
        user: UserDto::from(&user),
    };
    signed_in(state, &user, &body).await
}

/// `POST /api/session/sign-out`: ends the session and removes the cookie.
/// The answer is `{"success": true}` also without a session.
pub async fn sign_out(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !session::same_origin(&state, &headers) {
        return session::forbidden();
    }
    if session::end(&state, &headers).await.is_err() {
        return wire::internal_error();
    }
    let mut response = pass_through(wire::json(StatusCode::OK, &SuccessDto { success: true }));
    if let Some(cookie) = state.session.clear_cookie() {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    response
}

/// `GET /api/session`: the session and the username, or both `null`.
pub async fn current_session(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Ok(active) = session::current(&state, &headers).await else {
        return wire::internal_error();
    };
    let Some(active) = active else {
        return wire::json(
            StatusCode::OK,
            &SessionResponseDto {
                session: None,
                username: None,
            },
        );
    };
    let user = active.row.user;
    let username = match accounts::username_by_user_id(&state.pool, &user.id).await {
        Ok(username) => username,
        Err(error) => {
            tracing::error!(code = error.code(), "a username could not be read");
            return wire::internal_error();
        }
    };
    let body = SessionResponseDto {
        session: Some(SessionDto {
            session: SessionInfoDto {
                user_id: user.id.clone(),
                created_at: wire::iso(active.row.created_at),
                expires_at: wire::iso(active.row.expires_at),
            },
            user: UserDto::from(&user),
        }),
        username,
    };
    let mut response = wire::json(StatusCode::OK, &body);
    if let Some(cookie) = active.renewed_cookie {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    response
}

/// Replaces the key of a user with a new one.
async fn new_key(state: &AppState, user_id: &str) -> Result<ApiKeyDto, ()> {
    let (Ok(key), Ok(id)) = (random::api_key(), random::api_key_id()) else {
        tracing::error!(code = "random", "no random bytes for an API key");
        return Err(());
    };
    let now = state.now();
    api_keys::replace_api_key(&state.pool, user_id, &id, &key, now)
        .await
        .map_err(|error| tracing::error!(code = error.code(), "an API key could not be stored"))?;
    Ok(ApiKeyDto {
        api_key: key,
        created_at: wire::iso(now),
    })
}

async fn ensure_settings(state: &AppState, user_id: &str) -> Result<(), ()> {
    user_settings::ensure_user_settings(&state.pool, user_id, state.now())
        .await
        .map(|_| ())
        .map_err(|error| tracing::error!(code = error.code(), "user settings could not be read"))
}

/// `GET /api/keys`: the current key, or a new first key with 201.
pub async fn get_key(
    State(state): State<AppState>,
    Extension(user): Extension<SignedIn>,
) -> Response {
    let user_id = user.user.id;
    if ensure_settings(&state, &user_id).await.is_err() {
        return wire::internal_error();
    }
    match api_keys::current_api_key(&state.pool, &user_id).await {
        Ok(Some(current)) => wire::json(
            StatusCode::OK,
            &ApiKeyDto {
                api_key: current.key,
                created_at: wire::iso(current.created_at),
            },
        ),
        Ok(None) => match new_key(&state, &user_id).await {
            Ok(created) => wire::json(StatusCode::CREATED, &created),
            Err(()) => wire::internal_error(),
        },
        Err(error) => {
            tracing::error!(code = error.code(), "an API key could not be read");
            wire::internal_error()
        }
    }
}

/// `POST /api/keys/regenerate`: deletes the key and creates a new one.
pub async fn regenerate_key(
    State(state): State<AppState>,
    Extension(user): Extension<SignedIn>,
    headers: HeaderMap,
) -> Response {
    if !session::same_origin(&state, &headers) {
        return session::forbidden();
    }
    let user_id = user.user.id;
    if ensure_settings(&state, &user_id).await.is_err() {
        return wire::internal_error();
    }
    match new_key(&state, &user_id).await {
        Ok(created) => wire::json(StatusCode::OK, &created),
        Err(()) => wire::internal_error(),
    }
}
