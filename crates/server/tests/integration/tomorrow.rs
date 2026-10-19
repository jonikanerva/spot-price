//! `tomorrowAvailable` of the public spot prices and the setup chart, and
//! `available` of `/api/v1/price/tomorrow`: true only when the store has
//! every quarter of the next local day.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use spot_price_server::clock::FixedClock;
use spot_price_server::http::{AppState, router};
use sqlx::PgPool;
use tower::ServiceExt as _;

use crate::common::ts;

/// Before the auction: Helsinki day 2026-10-18, published CET day
/// 2026-10-18, which ends at 01:00 Helsinki on 2026-10-19.
const NOW: &str = "2026-10-18T07:05:00Z";
/// The end of the published CET delivery day 2026-10-18.
const PUBLISHED_TO: &str = "2026-10-18T22:00:00Z";
/// The end of the next CET delivery day, after the auction.
const AUCTION_TO: &str = "2026-10-19T22:00:00Z";

fn app(pool: PgPool) -> Router {
    app_at(pool, NOW)
}

fn app_at(pool: PgPool, now: &str) -> Router {
    router(AppState::new(
        pool,
        Arc::new(FixedClock(ts(now))),
        &"http://localhost:3000".parse().unwrap(),
    ))
}

/// Stores one FI quarter per 15 minutes in `[from, to)`.
async fn store_fi(pool: &PgPool, from: &str, to: &str) {
    sqlx::query(
        "INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area)
         SELECT t, t + interval '15 minutes', 50, 'FI'
         FROM generate_series($1::timestamptz, $2::timestamptz - interval '15 minutes',
                              interval '15 minutes') AS t",
    )
    .bind(from)
    .bind(to)
    .execute(pool)
    .await
    .unwrap();
}

async fn get(app: &Router, path: &str, cookie: Option<&str>) -> Value {
    let mut request = Request::get(path);
    if let Some(cookie) = cookie {
        request = if cookie.starts_with("sp_") {
            request.header("authorization", format!("Bearer {cookie}"))
        } else {
            request.header("cookie", cookie)
        };
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// `(tomorrowAvailable, today points, tomorrow points)` of a body.
fn days(body: &Value) -> (bool, usize, usize) {
    (
        body["tomorrowAvailable"].as_bool().unwrap(),
        body["today"].as_array().unwrap().len(),
        body["tomorrow"].as_array().unwrap().len(),
    )
}

/// `spot_session=<hex>` of a new account with the default settings.
async fn sign_up(app: &Router) -> String {
    let request = Request::post("/api/session/login-or-signup")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"username":"tomorrow","password":"hunter2hunter2"}"#,
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers().get("set-cookie").unwrap();
    cookie
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_first_hour_of_tomorrow_is_not_an_available_tomorrow(pool: PgPool) {
    store_fi(&pool, "2026-10-17T21:00:00Z", PUBLISHED_TO).await;
    let app = app(pool.clone());
    let cookie = sign_up(&app).await;

    let spot = get(&app, "/api/public/spot?area=FI", None).await;
    assert_eq!(days(&spot), (false, 96, 4));
    let chart = get(&app, "/api/v1/me/chart", Some(&cookie)).await;
    assert_eq!(days(&chart), (false, 96, 4));

    store_fi(&pool, PUBLISHED_TO, AUCTION_TO).await;
    let spot = get(&app, "/api/public/spot?area=FI", None).await;
    assert_eq!(days(&spot), (true, 96, 96));
    let chart = get(&app, "/api/v1/me/chart", Some(&cookie)).await;
    assert_eq!(days(&chart), (true, 96, 96));
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_missing_quarter_of_tomorrow_is_not_an_available_tomorrow(pool: PgPool) {
    store_fi(&pool, "2026-10-17T21:00:00Z", "2026-10-19T12:00:00Z").await;
    store_fi(&pool, "2026-10-19T12:15:00Z", AUCTION_TO).await;
    let app = app(pool);
    let spot = get(&app, "/api/public/spot?area=FI", None).await;
    assert_eq!(days(&spot), (false, 96, 95));
}

/// A new API key of the account behind `cookie`.
async fn api_key(app: &Router, cookie: &str) -> String {
    let request = Request::post("/api/keys/regenerate")
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    body["apiKey"].as_str().unwrap().to_owned()
}

/// `(available, prices)` of `/api/v1/price/tomorrow`.
async fn tomorrow_route(app: &Router, key: &str) -> (bool, usize) {
    let body = get(app, "/api/v1/price/tomorrow", Some(key)).await;
    (
        body["available"].as_bool().unwrap(),
        body["prices"].as_array().unwrap().len(),
    )
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_tomorrow_route_is_available_only_for_a_complete_day(pool: PgPool) {
    // (now, start of today in Helsinki, end of the published CET day, end of
    // the next CET day, quarters of tomorrow in Helsinki)
    let days = [
        (NOW, "2026-10-17T21:00:00Z", PUBLISHED_TO, AUCTION_TO, 96),
        // Tomorrow is 2026-03-29, a 23-hour day in Helsinki.
        (
            "2026-03-28T07:05:00Z",
            "2026-03-27T22:00:00Z",
            "2026-03-28T23:00:00Z",
            "2026-03-29T22:00:00Z",
            92,
        ),
        // Tomorrow is 2026-10-25, a 25-hour day in Helsinki.
        (
            "2026-10-24T07:05:00Z",
            "2026-10-23T21:00:00Z",
            "2026-10-24T22:00:00Z",
            "2026-10-25T23:00:00Z",
            100,
        ),
    ];
    for (now, today_from, published_to, auction_to, quarters) in days {
        sqlx::query("TRUNCATE prices").execute(&pool).await.unwrap();
        store_fi(&pool, today_from, published_to).await;
        let app = app_at(pool.clone(), now);
        let cookie = sign_up(&app).await;
        let key = api_key(&app, &cookie).await;
        assert_eq!(tomorrow_route(&app, &key).await, (false, 0), "{now}");

        store_fi(&pool, published_to, auction_to).await;
        assert_eq!(tomorrow_route(&app, &key).await, (true, quarters), "{now}");
        sqlx::query(
            r#"TRUNCATE "user", account, usernames, user_settings, api_keys, app_sessions CASCADE"#,
        )
        .execute(&pool)
        .await
        .unwrap();
    }
}
