//! `GET /health`: the database liveness check of the Railway deploy gate.

use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use super::{AppState, only};
use crate::db;

/// The deadline of the `SELECT 1` check, below the 30 s Railway timeout.
const HEALTH_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Health {
    pub status: &'static str,
    pub db: &'static str,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/health", only(get(health)))
}

/// 200 `{"status":"ok","db":"connected"}` when `SELECT 1` returns 1.
/// 503 with `"db":"query failed"` when it returns another value, and with
/// `"db":"unavailable"` when the query fails or passes its deadline.
async fn health(State(state): State<AppState>) -> Response {
    match tokio::time::timeout(HEALTH_DEADLINE, db::ping(&state.pool)).await {
        Ok(Ok(true)) => Json(Health {
            status: "ok",
            db: "connected",
        })
        .into_response(),
        Ok(Ok(false)) => unhealthy("query failed"),
        Ok(Err(_)) | Err(_) => unhealthy("unavailable"),
    }
}

fn unhealthy(db: &'static str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(Health {
            status: "error",
            db,
        }),
    )
        .into_response()
}
