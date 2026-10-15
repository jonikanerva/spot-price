//! Cookie sessions of the setup UI.
//!
//! The cookie holds 32 random bytes as hexadecimal text. The database holds
//! only the SHA-256 hash of those bytes. A session lasts
//! [`SESSION_LIFETIME`]; a request on a session older than
//! [`SESSION_UPDATE_AGE`] since its last renewal renews it and sends the
//! cookie again.

use axum::extract::{Request, State};
use axum::http::header::{COOKIE, ORIGIN, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use jiff::{SignedDuration, Timestamp};
use sha2::{Digest as _, Sha256};

use super::{AppState, wire};
use crate::auth::random;
use crate::store::accounts::UserRow;
use crate::store::sessions::{self, SessionRow};

/// The lifetime of a session and of its cookie.
pub const SESSION_LIFETIME: SignedDuration = SignedDuration::from_hours(7 * 24);
/// The age after which a request renews a session.
pub const SESSION_UPDATE_AGE: SignedDuration = SignedDuration::from_hours(24);

/// The cookie name over plain HTTP (local development).
pub const COOKIE_NAME: &str = "spot_session";
/// The cookie name over HTTPS. The `__Host-` prefix makes the browser
/// require `Secure`, `Path=/`, and no `Domain`.
pub const SECURE_COOKIE_NAME: &str = "__Host-spot_session";

/// The cookie and origin rules, from `BETTER_AUTH_URL`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    /// `scheme://host[:port]` of the service.
    pub origin: String,
    /// True when the service URL is HTTPS.
    pub secure: bool,
}

impl SessionConfig {
    /// The rules for the public base URL of the service.
    #[must_use]
    pub fn new(base_url: &reqwest::Url) -> Self {
        Self {
            origin: base_url.origin().ascii_serialization(),
            secure: base_url.scheme() == "https",
        }
    }

    #[must_use]
    pub const fn cookie_name(&self) -> &'static str {
        if self.secure {
            SECURE_COOKIE_NAME
        } else {
            COOKIE_NAME
        }
    }

    /// The `Set-Cookie` value of a session token.
    #[must_use]
    pub fn cookie(&self, token: &[u8; 32]) -> Option<HeaderValue> {
        let secure = if self.secure { " Secure;" } else { "" };
        HeaderValue::from_str(&format!(
            "{}={}; Max-Age={}; Path=/; HttpOnly;{secure} SameSite=Lax",
            self.cookie_name(),
            random::hex(token),
            SESSION_LIFETIME.as_secs(),
        ))
        .ok()
    }

    /// The `Set-Cookie` value that removes the session cookie.
    #[must_use]
    pub fn clear_cookie(&self) -> Option<HeaderValue> {
        let secure = if self.secure { " Secure;" } else { "" };
        HeaderValue::from_str(&format!(
            "{}=; Max-Age=0; Path=/; HttpOnly;{secure} SameSite=Lax",
            self.cookie_name()
        ))
        .ok()
    }
}

/// The SHA-256 hash of a session token.
#[must_use]
pub fn token_hash(token: &[u8; 32]) -> [u8; 32] {
    Sha256::digest(token).into()
}

/// The session token of the request cookie, when it is well formed.
#[must_use]
pub fn read_token(headers: &HeaderMap, cookie_name: &str) -> Option<[u8; 32]> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| name.trim() == cookie_name)
        .and_then(|(_, value)| random::from_hex::<32>(value.trim()))
}

/// A session operation failed. The cause is logged where it happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the session operation failed")]
pub struct SessionError;

/// The signed-in user of a request, set by [`require_session`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedIn {
    pub user: UserRow,
}

/// A session found for a request, after a renewal when one was due.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSession {
    pub row: SessionRow,
    /// The `Set-Cookie` value of a renewed session.
    pub renewed_cookie: Option<HeaderValue>,
}

/// Starts a new session for `user_id` and gives the `Set-Cookie` value.
///
/// # Errors
///
/// Returns [`SessionError`] when no random bytes are available or the store fails;
/// the cause is logged.
pub async fn start(state: &AppState, user_id: &str) -> Result<HeaderValue, SessionError> {
    let Ok(token) = random::bytes::<32>() else {
        tracing::error!(code = "random", "no random bytes for a session token");
        return Err(SessionError);
    };
    let now = state.now();
    let expires_at = now.checked_add(SESSION_LIFETIME).map_err(|_| {
        tracing::error!(code = "time", "the session expiry is out of range");
        SessionError
    })?;
    sessions::create_session(&state.pool, &token_hash(&token), user_id, now, expires_at)
        .await
        .map_err(|error| {
            tracing::error!(code = error.code(), "a session could not be stored");
            SessionError
        })?;
    state.session.cookie(&token).ok_or(SessionError)
}

/// The session of the request cookie. A session older than
/// [`SESSION_UPDATE_AGE`] since its last renewal gets a new expiry.
///
/// # Errors
///
/// Returns [`SessionError`] when the store fails; the cause is logged.
pub async fn current(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<ActiveSession>, SessionError> {
    let Some(token) = read_token(headers, state.session.cookie_name()) else {
        return Ok(None);
    };
    let hash = token_hash(&token);
    let now = state.now();
    let found = sessions::find_session(&state.pool, &hash, now)
        .await
        .map_err(|error| {
            tracing::error!(code = error.code(), "a session could not be read");
            SessionError
        })?;
    let Some(mut row) = found else {
        return Ok(None);
    };
    let renew_at = row
        .expires_at
        .checked_sub(SESSION_LIFETIME)
        .and_then(|issued| issued.checked_add(SESSION_UPDATE_AGE))
        .unwrap_or(Timestamp::MIN);
    let mut renewed_cookie = None;
    if renew_at <= now
        && let Ok(expires_at) = now.checked_add(SESSION_LIFETIME)
    {
        sessions::extend_session(&state.pool, &hash, expires_at)
            .await
            .map_err(|error| {
                tracing::error!(code = error.code(), "a session could not be renewed");
                SessionError
            })?;
        row.expires_at = expires_at;
        renewed_cookie = state.session.cookie(&token);
    }
    Ok(Some(ActiveSession {
        row,
        renewed_cookie,
    }))
}

/// Ends the session of the request cookie, if any.
///
/// # Errors
///
/// Returns [`SessionError`] when the store fails; the cause is logged.
pub async fn end(state: &AppState, headers: &HeaderMap) -> Result<(), SessionError> {
    if let Some(token) = read_token(headers, state.session.cookie_name()) {
        sessions::delete_session(&state.pool, &token_hash(&token))
            .await
            .map_err(|error| {
                tracing::error!(code = error.code(), "a session could not be deleted");
                SessionError
            })?;
    }
    Ok(())
}

/// Middleware: answers 401 `{"error":"Unauthorized"}` without a valid
/// session; otherwise adds [`SignedIn`] to the request.
pub async fn require_session(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let active = match current(&state, request.headers()).await {
        Ok(Some(active)) => active,
        Ok(None) => return wire::error(StatusCode::UNAUTHORIZED, "Unauthorized"),
        Err(SessionError) => return wire::internal_error(),
    };
    request.extensions_mut().insert(SignedIn {
        user: active.row.user,
    });
    let mut response = next.run(request).await;
    if let Some(cookie) = active.renewed_cookie {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    response
}

/// Whether a state-changing request may run: a request with an `Origin`
/// header must come from the origin of the service.
#[must_use]
pub fn same_origin(state: &AppState, headers: &HeaderMap) -> bool {
    headers
        .get(ORIGIN)
        .is_none_or(|origin| origin.as_bytes() == state.session.origin.as_bytes())
}

/// The 403 answer of a cross-origin state change.
#[must_use]
pub fn forbidden() -> Response {
    wire::error(StatusCode::FORBIDDEN, "Forbidden")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cookie_follows_the_scheme() {
        let local = SessionConfig::new(&"http://localhost:3000".parse().unwrap());
        assert_eq!(local.origin, "http://localhost:3000");
        let cookie = local.cookie(&[0xab; 32]).unwrap();
        assert_eq!(
            cookie.to_str().unwrap(),
            format!(
                "spot_session={}; Max-Age=604800; Path=/; HttpOnly; SameSite=Lax",
                "ab".repeat(32)
            )
        );
        let public = SessionConfig::new(&"https://spot.example/app".parse().unwrap());
        assert_eq!(public.origin, "https://spot.example");
        assert_eq!(
            public.clear_cookie().unwrap().to_str().unwrap(),
            "__Host-spot_session=; Max-Age=0; Path=/; HttpOnly; Secure; SameSite=Lax"
        );
    }

    #[test]
    fn reads_the_token_of_the_named_cookie() {
        let mut headers = HeaderMap::new();
        let token = "0f".repeat(32);
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&format!("other=1; spot_session={token}; x=y")).unwrap(),
        );
        assert_eq!(read_token(&headers, "spot_session"), Some([0x0f; 32]));
        assert_eq!(read_token(&headers, "__Host-spot_session"), None);
        headers.insert(COOKIE, HeaderValue::from_static("spot_session=short"));
        assert_eq!(read_token(&headers, "spot_session"), None);
    }
}
