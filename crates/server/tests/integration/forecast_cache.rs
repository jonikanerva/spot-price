//! The precomputed forecast: the swap, the start-up computation, the
//! job triggers, a failed recompute, and the `generatedAt` rule. The data is
//! the golden seed of the TypeScript cases.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use spot_price_server::clock::{Clock, FixedClock};
use spot_price_server::config::Secret;
use spot_price_server::forecast_cache::ForecastCache;
use spot_price_server::http::{AppState, router};
use spot_price_server::jobs::{self, JobContext};
use spot_price_server::lifecycle::startup_forecast;
use sqlx::PgPool;
use tokio::sync::watch;
use tower::ServiceExt as _;
use tracing::field::{Field, Visit};
use tracing::instrument::WithSubscriber as _;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt as _};

use crate::common::{FakeServer, fingrid, hours, json, minutes, nordpool, raw, ts, weather};

const NOW: &str = "2026-10-24T09:07:30Z";
const ALICE: &str = "sp_a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
const BOB: &str = "sp_b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2";
/// The newest vintage of the seed.
const SEED_ISSUED: &str = "2026-10-24T08:00:00.000Z";
const SEED: &str = include_str!("../golden/http/seed.sql");

/// The level, the message, and the `code` field of one event.
type Recorded = (Level, String, Option<String>);

/// The events of this service in the futures that run with this layer.
#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<Recorded>>>);

impl Events {
    fn take(&self) -> Vec<Recorded> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

#[derive(Default)]
struct EventFields {
    message: String,
    code: Option<String>,
}

impl Visit for EventFields {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "code" {
            self.code = Some(value.to_owned());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        }
    }
}

impl<S: Subscriber> Layer<S> for Events {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        if !event.metadata().target().starts_with("spot_price_server") {
            return;
        }
        let mut fields = EventFields::default();
        event.record(&mut fields);
        self.0
            .lock()
            .unwrap()
            .push((*event.metadata().level(), fields.message, fields.code));
    }
}

/// Loads the seed with a fixed `fetched_at` for every price.
async fn seed(pool: &PgPool) {
    sqlx::raw_sql(SEED).execute(pool).await.unwrap();
    sqlx::query("UPDATE prices SET fetched_at = '2026-10-23T11:02:00Z'")
        .execute(pool)
        .await
        .unwrap();
}

fn app(pool: &PgPool, now: &str, cache: Option<&ForecastCache>) -> Router {
    let state = AppState::new(
        pool.clone(),
        Arc::new(FixedClock(ts(now))),
        &"http://localhost:3000".parse().unwrap(),
    );
    router(match cache {
        Some(cache) => state.with_forecast(cache.clone()),
        None => state,
    })
}

async fn forecast(app: &Router, key: &str) -> (StatusCode, String) {
    forecast_at(app, key, "").await
}

async fn forecast_at(app: &Router, key: &str, query: &str) -> (StatusCode, String) {
    let request = Request::get(format!("/api/v1/price/forecast{query}"))
        .header("authorization", format!("Bearer {key}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

fn generated_at(body: &str) -> String {
    let parsed: Value = serde_json::from_str(body).unwrap();
    parsed["generatedAt"].as_str().unwrap().to_owned()
}

async fn filled(pool: &PgPool, now: &str) -> ForecastCache {
    let cache = ForecastCache::default();
    cache.refresh(pool, ts(now), "test").await.unwrap();
    cache
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_cached_result_gives_the_same_body_as_a_computation_per_request(pool: PgPool) {
    seed(&pool).await;
    let cache = filled(&pool, NOW).await;
    let cached = app(&pool, NOW, Some(&cache));
    let empty = ForecastCache::default();
    let per_request = app(&pool, NOW, Some(&empty));

    for key in [ALICE, BOB] {
        let (status, from_cache) = forecast(&cached, key).await;
        assert_eq!(status, StatusCode::OK, "{from_cache}");
        let (status, computed) = forecast(&per_request, key).await;
        assert_eq!(status, StatusCode::OK, "{computed}");
        assert_eq!(from_cache, computed);
        assert!(from_cache.contains(r#""available":true"#), "{from_cache}");
    }
    // The terms and the time zone of each user apply per request.
    assert_ne!(
        forecast(&cached, ALICE).await.1,
        forecast(&cached, BOB).await.1
    );
    // A computation per request does not fill the cache.
    assert!(empty.current().is_none());
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn generated_at_is_the_newest_grid_issuance_or_fi_price_arrival(pool: PgPool) {
    seed(&pool).await;
    let cache = ForecastCache::default();
    let app = app(&pool, NOW, Some(&cache));

    // The grid issuance (08:00) is newer than the price arrival (23 Oct).
    cache.refresh(&pool, ts(NOW), "test").await.unwrap();
    assert_eq!(generated_at(&forecast(&app, ALICE).await.1), SEED_ISSUED);

    // A newer neighbour price does not count; only FI prices do.
    sqlx::query("UPDATE prices SET fetched_at = '2026-10-24T09:00:00Z' WHERE area <> 'FI'")
        .execute(&pool)
        .await
        .unwrap();
    cache.refresh(&pool, ts(NOW), "test").await.unwrap();
    assert_eq!(generated_at(&forecast(&app, ALICE).await.1), SEED_ISSUED);

    // A newer FI price arrival wins.
    sqlx::query("UPDATE prices SET fetched_at = '2026-10-24T08:30:00Z' WHERE area = 'FI'")
        .execute(&pool)
        .await
        .unwrap();
    cache.refresh(&pool, ts(NOW), "test").await.unwrap();
    assert_eq!(
        generated_at(&forecast(&app, ALICE).await.1),
        "2026-10-24T08:30:00.000Z"
    );

    // Without grid forecasts, the FI price arrival is the freshness.
    sqlx::query("DELETE FROM fingrid_forecasts")
        .execute(&pool)
        .await
        .unwrap();
    cache.refresh(&pool, ts(NOW), "test").await.unwrap();
    assert_eq!(
        generated_at(&forecast(&app, ALICE).await.1),
        "2026-10-24T08:30:00.000Z"
    );

    // The responses without a computation keep the request instant.
    let (_, other_area) = forecast_at(&app, ALICE, "?area=SE3").await;
    assert_eq!(generated_at(&other_area), "2026-10-24T09:07:30.000Z");
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_failed_recompute_keeps_the_previous_result(pool: PgPool) {
    seed(&pool).await;
    let cache = filled(&pool, NOW).await;
    let before = cache.current().unwrap();
    let app = app(&pool, NOW, Some(&cache));
    let (_, body_before) = forecast(&app, ALICE).await;

    sqlx::raw_sql("DROP TABLE fingrid_forecasts")
        .execute(&pool)
        .await
        .unwrap();
    let events = Events::default();
    let error = cache
        .refresh(&pool, ts(NOW), "test")
        .with_subscriber(tracing_subscriber::registry().with(events.clone()))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "query");
    assert!(Arc::ptr_eq(&before, &cache.current().unwrap()));
    assert_eq!(
        events.take(),
        [(
            Level::WARN,
            "forecast recompute failed; the previous result stays".to_owned(),
            Some("query".to_owned()),
        )]
    );

    let (status, body_after) = forecast(&app, ALICE).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body_after, body_before);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_recompute_swaps_in_a_new_result_and_an_old_reader_keeps_its_copy(pool: PgPool) {
    seed(&pool).await;
    let cache = filled(&pool, NOW).await;
    let old = cache.current().unwrap();
    sqlx::query(
        "UPDATE fingrid_forecasts SET issued_at = '2026-10-24T09:00:00Z' \
         WHERE issued_at = '2026-10-24T08:00:00Z'",
    )
    .execute(&pool)
    .await
    .unwrap();
    cache.refresh(&pool, ts(NOW), "test").await.unwrap();
    let new = cache.current().unwrap();
    assert!(!Arc::ptr_eq(&old, &new));
    assert_eq!(old.generated_at, ts("2026-10-24T08:00:00Z"));
    assert_eq!(new.generated_at, ts("2026-10-24T09:00:00Z"));
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_window_from_the_clock_is_computed_per_request(pool: PgPool) {
    seed(&pool).await;
    sqlx::query("DELETE FROM prices WHERE area = 'FI'")
        .execute(&pool)
        .await
        .unwrap();
    let cache = filled(&pool, NOW).await;
    assert!(cache.current().unwrap().window_from_clock);

    let later = "2026-10-24T10:07:30Z";
    let (_, cached) = forecast(&app(&pool, later, Some(&cache)), ALICE).await;
    let (_, computed) = forecast(&app(&pool, later, None), ALICE).await;
    assert_eq!(cached, computed);
}

// --- Triggers ----------------------------------------------------------------

fn context(pool: &PgPool, server: &FakeServer) -> JobContext {
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(ts(NOW)));
    JobContext {
        pool: pool.clone(),
        nordpool: nordpool(server),
        fingrid: fingrid(server),
        weather: weather(server),
        clock,
        forecast: ForecastCache::default(),
    }
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_start_up_task_fills_the_cache(pool: PgPool) {
    seed(&pool).await;
    let server = FakeServer::start(|_| raw(StatusCode::INTERNAL_SERVER_ERROR, "")).await;
    let context = context(&pool, &server);
    let (_stop, stopped) = watch::channel(false);
    startup_forecast(context.clone(), stopped).await;
    let snapshot = context.forecast.current().unwrap();
    assert_eq!(snapshot.generated_at, ts("2026-10-24T08:00:00Z"));
    assert_eq!(snapshot.built_at, ts(NOW));
}

/// A Nord Pool body with 96 FI quarters from `start`.
fn nordpool_body(start: &str) -> Value {
    let entries: Vec<Value> = (0..96)
        .map(|i| {
            let delivery_start = minutes(ts(start), 15 * i);
            serde_json::json!({
                "deliveryStart": delivery_start.to_string(),
                "deliveryEnd": minutes(delivery_start, 15).to_string(),
                "entryPerArea": {"FI": 30.0}
            })
        })
        .collect();
    serde_json::json!({"multiAreaEntries": entries})
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_price_run_that_stores_prices_recomputes_the_forecast(pool: PgPool) {
    seed(&pool).await;
    let body = nordpool_body("2026-10-25T00:00:00Z");
    let server = FakeServer::start(move |_| json(StatusCode::OK, &body)).await;
    let context = context(&pool, &server);
    // The seed gives every price one fixed `fetched_at`; a written row gets
    // the database clock.
    let written = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM prices \
             WHERE area = 'FI' AND fetched_at <> '2026-10-23T11:02:00Z'",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    assert_eq!(written().await, 0);
    jobs::price_fetch(&context, "test").await;
    assert!(written().await > 0, "the run stores prices");
    assert!(context.forecast.current().is_some());
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_price_run_that_stores_nothing_keeps_the_forecast(pool: PgPool) {
    seed(&pool).await;
    // Nord Pool has nothing new; the run succeeds and stores no row.
    let server = FakeServer::start(|_| raw(StatusCode::OK, "")).await;
    let context = context(&pool, &server);
    jobs::price_fetch(&context, "test").await;
    assert!(context.forecast.current().is_none());
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_failed_price_run_keeps_the_forecast(pool: PgPool) {
    seed(&pool).await;
    let server = FakeServer::start(|_| raw(StatusCode::INTERNAL_SERVER_ERROR, "")).await;
    let context = context(&pool, &server);
    jobs::price_fetch(&context, "test").await;
    assert!(context.forecast.current().is_none());
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_fingrid_run_that_stores_data_recomputes_the_forecast(pool: PgPool) {
    seed(&pool).await;
    let start = ts("2026-10-24T09:00:00Z");
    let body = serde_json::json!({"data": [{
        "datasetId": 75,
        "startTime": start.to_string(),
        "endTime": hours(start, 1).to_string(),
        "value": 1000.0
    }]});
    let server = FakeServer::start(move |_| json(StatusCode::OK, &body)).await;
    let context = context(&pool, &server);
    jobs::fingrid_fetch(&context, &Secret::new("test-key".to_owned()), "test").await;
    assert!(context.forecast.current().is_some());
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_degraded_fingrid_run_keeps_the_forecast(pool: PgPool) {
    seed(&pool).await;
    let server = FakeServer::start(|_| raw(StatusCode::UNAUTHORIZED, "{}")).await;
    let context = context(&pool, &server);
    jobs::fingrid_fetch(&context, &Secret::new("test-key".to_owned()), "test").await;
    assert!(context.forecast.current().is_none());
}
