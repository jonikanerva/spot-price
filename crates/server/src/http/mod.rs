//! The HTTP router and its shared state.

pub mod health;

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use axum::response::IntoResponse as _;
use sqlx::PgPool;
use tower_http::catch_panic::CatchPanicLayer;

use crate::clock::Clock;

/// The shared state of the handlers.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AppState").finish_non_exhaustive()
    }
}

/// Builds the router with all routes and middleware.
pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(health::routes())
        .with_state(state)
        // A panic in a handler becomes a bare 500; the panic hook logs only
        // its source location.
        .layer(CatchPanicLayer::custom(|_| {
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }))
}
