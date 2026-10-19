//! The HTTP router and its shared state.
//!
//! The routes, status codes, and bodies are the ones of the TypeScript
//! (Hono) service. Its routing rules apply here too:
//! - An unknown path, and a known path with another method, answer 404
//!   `text/plain` `404 Not Found`.
//! - The API key check covers every path under `/api/v1/price`, the session
//!   check every path under `/api/v1/me` and `/api/keys`, also an unknown
//!   one, so such a request without credentials answers 401, not 404.
//! - The global rate limit covers every path except `/health`.

pub mod account;
pub mod api_key;
pub mod dto;
pub mod forecast;
pub mod health;
pub mod input;
pub mod me;
pub mod openapi;
pub mod price;
pub mod rate_limit;
pub mod session;
pub mod ui;
pub mod wire;

use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::extract::{MatchedPath, Request, State};
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::Response;
use axum::routing::{MethodRouter, any, get, post};
use jiff::Timestamp;
use reqwest::Url;
use sqlx::PgPool;
use tower_http::catch_panic::CatchPanicLayer;

use crate::auth::password::PasswordHasher;
use crate::clock::Clock;
use crate::forecast_cache::ForecastCache;
use rate_limit::RateLimits;
use session::SessionConfig;

/// The shared state of the handlers.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub clock: Arc<dyn Clock>,
    pub session: Arc<SessionConfig>,
    pub limits: Arc<RateLimits>,
    pub passwords: PasswordHasher,
    pub pages: Arc<ui::Pages>,
    /// The precomputed forecast. Empty until a job or the start-up task
    /// fills it; the forecast handler then computes per request.
    pub forecast: ForecastCache,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AppState").finish_non_exhaustive()
    }
}

impl AppState {
    /// The state for the public base URL of the service (`BETTER_AUTH_URL`).
    #[must_use]
    pub fn new(pool: PgPool, clock: Arc<dyn Clock>, base_url: &Url) -> Self {
        Self {
            pool,
            clock,
            session: Arc::new(SessionConfig::new(base_url)),
            limits: Arc::new(RateLimits::default()),
            passwords: PasswordHasher::default(),
            pages: Arc::new(ui::Pages::render()),
            forecast: ForecastCache::default(),
        }
    }

    /// The state with a shared forecast cache, so the jobs and the handlers
    /// use the same result.
    #[must_use]
    pub fn with_forecast(mut self, forecast: ForecastCache) -> Self {
        self.forecast = forecast;
        self
    }

    /// The current instant at millisecond precision, like JavaScript `Date`.
    #[must_use]
    pub fn now(&self) -> Timestamp {
        spot_price_domain::time::truncate_to_millisecond(self.clock.now())
    }
}

/// A response that the TypeScript service passed through from Better Auth.
/// Hono added no `RateLimit-*` headers to such a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassThrough;

/// The 404 handler of unknown paths and methods.
async fn not_found() -> Response {
    wire::not_found()
}

/// A method router whose other methods answer 404.
fn only<S: Clone + Send + Sync + 'static>(router: MethodRouter<S>) -> MethodRouter<S> {
    router.fallback(not_found)
}

/// The catch-all routes of a path prefix, so its middleware runs for an
/// unknown path below it, like `app.use("/prefix/*")` in Hono.
fn catch_all(router: Router<AppState>, prefix: &str) -> Router<AppState> {
    router
        .route(prefix, any(not_found))
        .route(&format!("{prefix}/"), any(not_found))
        .route(&format!("{prefix}/{{*rest}}"), any(not_found))
}

/// Builds the router with all routes and middleware.
pub fn router(state: AppState) -> Router {
    let (public, _) = openapi::public_routes().split_for_parts();
    let (price, _) = openapi::price_routes().split_for_parts();
    let (me, _) = openapi::me_routes().split_for_parts();

    let price = catch_all(price, "/api/v1/price")
        .layer(from_fn_with_state(state.clone(), api_key::limit))
        .layer(from_fn_with_state(state.clone(), api_key::authenticate));
    let me = catch_all(me, "/api/v1/me")
        .layer(from_fn_with_state(state.clone(), session::require_session));
    let keys = Router::new()
        .route("/api/keys", only(get(account::get_key)))
        .route("/api/keys/regenerate", only(post(account::regenerate_key)));
    let keys = catch_all_below(keys, "/api/keys")
        .layer(from_fn_with_state(state.clone(), session::require_session));
    let sessions = Router::new()
        .route(
            "/api/session/login-or-signup",
            only(
                post(account::login_or_signup)
                    .route_layer(from_fn_with_state(state.clone(), account::login_limit)),
            ),
        )
        .route("/api/session/sign-out", only(post(account::sign_out)))
        .route("/api/session", only(get(account::current_session)));

    let routes = Router::new()
        .merge(health::routes())
        .route("/", only(get(ui::home)))
        .route("/api/docs", only(get(ui::docs)))
        .route("/api/v1/openapi.json", only(get(openapi::serve)))
        .merge(public)
        .merge(price)
        .merge(me)
        .merge(keys)
        .merge(sessions)
        .fallback(not_found)
        .with_state(state.clone())
        .layer(from_fn_with_state(state, global_limit))
        .layer(from_fn(log_request))
        // A panic in a handler becomes the generic 500; the panic hook logs
        // only its source location.
        .layer(CatchPanicLayer::custom(|_| wire::internal_error()));
    // The `Allow` header is set after the route middleware, so only a layer
    // outside the routed router can remove it.
    Router::new()
        .fallback_service(routes)
        .layer(from_fn(drop_allow))
}

/// The catch-all routes below a prefix whose own path is a route already.
fn catch_all_below(router: Router<AppState>, prefix: &str) -> Router<AppState> {
    router
        .route(&format!("{prefix}/"), any(not_found))
        .route(&format!("{prefix}/{{*rest}}"), any(not_found))
}

/// The global limit: 120 requests per minute per client, except `/health`.
async fn global_limit(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }
    let key = rate_limit::client_key(request.headers());
    let limiter = &state.limits.global;
    let decision = limiter.check(&key, state.now().as_millisecond());
    if !decision.allowed {
        return limiter.limited(&decision);
    }
    let mut response = next.run(request).await;
    let pass_through = response.extensions().get::<PassThrough>().is_some();
    if !pass_through && !response.headers().contains_key("ratelimit-policy") {
        rate_limit::set_headers(response.headers_mut(), &decision);
    }
    response
}

/// Removes the `Allow` header that axum adds to a wrong-method answer: the
/// TypeScript service answered a plain 404 without it.
async fn drop_allow(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().remove(axum::http::header::ALLOW);
    response
}

/// Logs each request: the method, the matched route pattern (never the raw
/// path, the query, a header, or a body), the status, and the time.
async fn log_request(request: Request, next: Next) -> Response {
    let method = request.method().as_str().to_owned();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "unmatched".to_owned(), |path| path.as_str().to_owned());
    let started = Instant::now();
    let response = next.run(request).await;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    tracing::info!(
        method,
        route,
        status = response.status().as_u16(),
        elapsed_ms,
        "request"
    );
    response
}
