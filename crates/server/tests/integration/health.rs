//! `GET /health` through the router.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use spot_price_server::clock::FixedClock;
use spot_price_server::http::{AppState, router};
use sqlx::PgPool;
use tower::ServiceExt as _;

use crate::common::ts;

async fn get_health(pool: PgPool) -> (StatusCode, Value) {
    let app = router(AppState {
        pool,
        clock: Arc::new(FixedClock(ts("2026-01-01T00:00:00Z"))),
    });
    let response = app
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn health_is_ok_when_the_database_answers(pool: PgPool) {
    let (status, body) = get_health(pool).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({"status": "ok", "db": "connected"}));
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn health_is_unavailable_when_the_pool_is_closed(pool: PgPool) {
    pool.close().await;
    let (status, body) = get_health(pool).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body,
        serde_json::json!({"status": "error", "db": "unavailable"})
    );
}
