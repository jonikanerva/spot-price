//! Behaviour of the Rust service that the TypeScript fixtures do not cover:
//! session storage, expiry clean-up, the Origin check, and invalid stored
//! settings.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sha2::{Digest as _, Sha256};
use spot_price_server::clock::FixedClock;
use spot_price_server::http::{AppState, router};
use sqlx::PgPool;
use tower::ServiceExt as _;

use crate::common::ts;

const NOW: &str = "2026-10-24T09:07:30Z";

fn app(pool: PgPool, now: &str) -> Router {
    router(AppState::new(
        pool,
        Arc::new(FixedClock(ts(now))),
        &"http://localhost:3000".parse().unwrap(),
    ))
}

struct Answer {
    status: StatusCode,
    cookies: Vec<String>,
    body: String,
}

async fn send(app: &Router, request: Request<Body>) -> Answer {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let cookies = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| value.to_str().unwrap().to_owned())
        .collect();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    Answer {
        status,
        cookies,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
    }
}

fn login(username: &str, password: &str) -> Request<Body> {
    Request::post("/api/session/login-or-signup")
        .header("content-type", "application/json")
        .body(Body::from(format!(
            r#"{{"username":"{username}","password":"{password}"}}"#
        )))
        .unwrap()
}

/// `spot_session=<hex>` of a response.
fn session_pair(answer: &Answer) -> String {
    answer.cookies[0].split(';').next().unwrap().to_owned()
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn signup_writes_better_auth_rows_and_a_hashed_session(pool: PgPool) {
    let app = app(pool.clone(), NOW);
    let answer = send(&app, login("  NewUser ", "correct-horse-1")).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let pair = session_pair(&answer);
    let token = pair.strip_prefix("spot_session=").unwrap();
    assert_eq!(token.len(), 64);

    let (user_id, email, name): (String, String, String) =
        sqlx::query_as(r#"SELECT id, email, name FROM "user""#)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (email.as_str(), name.as_str()),
        ("newuser@local.spot", "newuser")
    );
    let (account_id, provider, password): (String, String, String) = sqlx::query_as(
        r#"SELECT "accountId", "providerId", password FROM account WHERE "userId" = $1"#,
    )
    .bind(&user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (account_id.as_str(), provider.as_str()),
        (user_id.as_str(), "credential")
    );
    assert!(
        spot_price_server::auth::password::verify_blocking(&password, "correct-horse-1").unwrap()
    );
    let username: String = sqlx::query_scalar("SELECT username FROM usernames WHERE user_id = $1")
        .bind(&user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(username, "newuser");
    let settings: i64 = sqlx::query_scalar("SELECT count(*) FROM user_settings WHERE user_id = $1")
        .bind(&user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(settings, 1);

    // The table holds the SHA-256 of the token bytes, never the token.
    let raw = spot_price_server::auth::random::from_hex::<32>(token).unwrap();
    let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM app_sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, Sha256::digest(raw).to_vec());
    assert!(!stored.windows(raw.len()).any(|part| part == raw));

    // The same user signs in again with the password; a wrong one fails.
    let again = send(&app, login("newuser", "correct-horse-1")).await;
    assert_eq!(again.status, StatusCode::OK);
    assert!(
        again
            .body
            .starts_with(r#"{"redirect":false,"token":null,"user":{"name":"newuser""#)
    );
    let wrong = send(&app, login("newuser", "wrong-horse-1")).await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        wrong.body,
        r#"{"message":"Invalid email or password","code":"INVALID_EMAIL_OR_PASSWORD"}"#
    );
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn sessions_expire_renew_and_end(pool: PgPool) {
    let first = app(pool.clone(), NOW);
    let pair = session_pair(&send(&first, login("renewer", "renewer-pass-1")).await);
    let get_session = |pair: &str| {
        Request::get("/api/session")
            .header("cookie", pair)
            .body(Body::empty())
            .unwrap()
    };

    // Before one day: no renewal.
    let early = app(pool.clone(), "2026-10-25T09:07:29Z");
    let answer = send(&early, get_session(&pair)).await;
    assert_eq!(answer.cookies, Vec::<String>::new());
    assert!(
        answer
            .body
            .contains(r#""expiresAt":"2026-10-31T09:07:30.000Z""#)
    );

    // After one day: the expiry moves and the cookie comes again.
    let later = app(pool.clone(), "2026-10-26T00:00:00Z");
    let answer = send(&later, get_session(&pair)).await;
    assert_eq!(answer.cookies.len(), 1);
    assert!(
        answer
            .body
            .contains(r#""expiresAt":"2026-11-02T00:00:00.000Z""#)
    );

    // After the expiry: no session, and the next sign-in deletes the row.
    let expired = app(pool.clone(), "2026-11-03T00:00:00Z");
    let answer = send(&expired, get_session(&pair)).await;
    assert_eq!(answer.body, r#"{"session":null,"username":null}"#);
    send(&expired, login("renewer", "renewer-pass-1")).await;
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM app_sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 1);

    // Sign-out deletes the session and clears the cookie.
    let current = session_pair(&send(&expired, login("renewer", "renewer-pass-1")).await);
    let out = send(
        &expired,
        Request::post("/api/session/sign-out")
            .header("cookie", &current)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(out.body, r#"{"success":true}"#);
    assert_eq!(
        out.cookies,
        ["spot_session=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax"]
    );
    let answer = send(&expired, get_session(&current)).await;
    assert_eq!(answer.body, r#"{"session":null,"username":null}"#);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn state_changes_from_another_origin_are_forbidden(pool: PgPool) {
    let app = app(pool, NOW);
    let pair = session_pair(&send(&app, login("origin", "origin-pass-1")).await);
    for (method, path, body) in [
        ("POST", "/api/keys/regenerate", ""),
        ("PUT", "/api/v1/me/settings", r#"{"vatPercent":10}"#),
        ("POST", "/api/session/sign-out", ""),
        (
            "POST",
            "/api/session/login-or-signup",
            r#"{"username":"origin","password":"origin-pass-1"}"#,
        ),
    ] {
        let request = |origin: &str| {
            Request::builder()
                .method(method)
                .uri(path)
                .header("cookie", &pair)
                .header("content-type", "application/json")
                .header("origin", origin)
                .body(Body::from(body))
                .unwrap()
        };
        let cross = send(&app, request("https://evil.example")).await;
        assert_eq!(cross.status, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(cross.body, r#"{"error":"Forbidden"}"#);
        if path != "/api/session/sign-out" {
            let same = send(&app, request("http://localhost:3000")).await;
            assert_eq!(same.status, StatusCode::OK, "{path}: {}", same.body);
        }
    }
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn invalid_stored_settings_answer_the_generic_500(pool: PgPool) {
    sqlx::raw_sql(
        "INSERT INTO api_keys (id, user_id, key_plaintext, created_at) VALUES ('k', 'u', 'sp_k', now());
         INSERT INTO user_settings (user_id, timezone) VALUES ('u', 'America/New_York');",
    )
    .execute(&pool)
    .await
    .unwrap();
    let app = app(pool, NOW);
    for path in [
        "/api/v1/price/now",
        "/api/v1/price/today",
        "/api/v1/price/forecast",
    ] {
        let answer = send(
            &app,
            Request::get(path)
                .header("authorization", "Bearer sp_k")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR, "{path}");
        assert_eq!(answer.body, "Internal Server Error");
    }
}
