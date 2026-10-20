//! The price, Fingrid, and weather jobs against a migrated database and a
//! fake upstream.

use axum::http::StatusCode;
use jiff::Timestamp;
use serde_json::{Value, json};
use spot_price_domain::areas::{ALL_AREAS, Area};
use spot_price_server::config::Secret;
use spot_price_server::jobs::fingrid::{
    ARCHIVE_HORIZON_HOURS, ArchiveFetchResult, FORECAST_DAYS, ForecastFetchResult, HISTORY_DAYS,
    RETENTION_DAYS, VINTAGE_RETENTION_DAYS, run_forecast_fetch_job, run_vintage_archive_job,
};
use spot_price_server::jobs::prices::{
    FetchJobError, FetchResult, all_areas_present, run_fetch_job,
};
use spot_price_server::jobs::weather::{
    PointFailure, WEATHER_RETENTION_DAYS, WeatherJobResult, run_weather_fetch_job,
};
use spot_price_server::store::fingrid::{
    DATASET_CONSUMPTION_FORECAST, DATASET_CONSUMPTION_FORECAST_72H, DATASET_SOLAR_FORECAST,
    DATASET_WIND_ACTUAL, DATASET_WIND_FORECAST, FingridRecord, VINTAGE_BACKFILL_HOURS,
    get_fingrid_forecast_vintages_all, get_fingrid_forecast_vintages_latest,
    get_fingrid_records_by_range, store_fingrid_forecast_vintages, store_fingrid_records,
};
use spot_price_server::store::prices::{PriceRow, store_prices};
use spot_price_server::store::weather::{
    WeatherRecord, get_weather_records_by_range, store_weather_daily_records, store_weather_records,
};
use spot_price_server::upstream::nordpool::NordPoolError;
use sqlx::PgPool;

use crate::common::{FakeServer, days, fingrid, hours, json, minutes, nordpool, raw, ts, weather};
use crate::stores::{daily, daily_rows};

// --- Prices ----------------------------------------------------------------

/// `count` quarter-hour prices from `start` for `area`.
fn day_prices(start: &str, area: Area, count: i64) -> Vec<PriceRow> {
    (0..count)
        .map(|i| {
            let delivery_start = minutes(ts(start), 15 * i);
            PriceRow {
                delivery_start,
                delivery_end: minutes(delivery_start, 15),
                price_eur_mwh: 30.0,
                area,
            }
        })
        .collect()
}

async fn fill_all_areas(pool: &PgPool, start: &str, count: i64) {
    for area in ALL_AREAS {
        store_prices(pool, &day_prices(start, area, count))
            .await
            .unwrap();
    }
}

/// A Nord Pool body with 96 FI quarters from `start`.
fn nordpool_body(start: &str) -> Value {
    let entries: Vec<Value> = day_prices(start, Area::Fi, 96)
        .iter()
        .map(|p| {
            json!({
                "deliveryStart": p.delivery_start.to_string(),
                "deliveryEnd": p.delivery_end.to_string(),
                "entryPerArea": {"FI": p.price_eur_mwh}
            })
        })
        .collect();
    json!({"multiAreaEntries": entries})
}

const PRICE_NOW: &str = "2026-03-29T10:00:00Z";

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn all_areas_present_needs_every_area(pool: PgPool) {
    let date = jiff::civil::date(2026, 3, 29);
    assert!(!all_areas_present(&pool, date).await.unwrap());
    store_prices(&pool, &day_prices("2026-03-29T00:00:00Z", Area::Fi, 96))
        .await
        .unwrap();
    assert!(!all_areas_present(&pool, date).await.unwrap());
    fill_all_areas(&pool, "2026-03-29T00:00:00Z", 96).await;
    assert!(all_areas_present(&pool, date).await.unwrap());
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn all_areas_present_accepts_a_short_dst_day(pool: PgPool) {
    fill_all_areas(&pool, "2026-03-29T00:00:00Z", 92).await;
    assert!(
        all_areas_present(&pool, jiff::civil::date(2026, 3, 29))
            .await
            .unwrap()
    );
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn all_areas_present_uses_the_threshold_of_23_entries(pool: PgPool) {
    let date = jiff::civil::date(2026, 3, 29);
    fill_all_areas(&pool, "2026-03-29T00:00:00Z", 22).await;
    assert!(!all_areas_present(&pool, date).await.unwrap());
    fill_all_areas(&pool, "2026-03-29T00:00:00Z", 23).await;
    assert!(all_areas_present(&pool, date).await.unwrap());
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn nothing_published_means_tomorrow_is_not_available(pool: PgPool) {
    let server = FakeServer::start(|_| raw(StatusCode::OK, "")).await;
    let result = run_fetch_job(&pool, &nordpool(&server), ts(PRICE_NOW))
        .await
        .unwrap();
    assert!(!result.tomorrow_available);
    let dates: Vec<_> = server
        .recorded()
        .iter()
        .map(|r| r.param("date").unwrap())
        .collect();
    assert_eq!(dates, ["2026-03-29", "2026-03-30"]);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn stored_tomorrow_prices_mean_tomorrow_is_available(pool: PgPool) {
    fill_all_areas(&pool, "2026-03-29T00:00:00Z", 96).await;
    let server =
        FakeServer::start(|_| json(StatusCode::OK, &nordpool_body("2026-03-30T00:00:00Z"))).await;
    let result = run_fetch_job(&pool, &nordpool(&server), ts(PRICE_NOW))
        .await
        .unwrap();
    assert!(result.tomorrow_available);
    assert_eq!(
        result.results,
        [
            FetchResult {
                date: jiff::civil::date(2026, 3, 29),
                stored: 0,
                skipped: true
            },
            FetchResult {
                date: jiff::civil::date(2026, 3, 30),
                stored: 96,
                skipped: false
            },
        ]
    );
    let dates: Vec<_> = server
        .recorded()
        .iter()
        .map(|r| r.param("date").unwrap())
        .collect();
    assert_eq!(dates, ["2026-03-30"]);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_failed_tomorrow_fetch_is_contained(pool: PgPool) {
    let server = FakeServer::start(|request| {
        if request.param("date").as_deref() == Some("2026-03-30") {
            raw(StatusCode::INTERNAL_SERVER_ERROR, "")
        } else {
            raw(StatusCode::OK, "")
        }
    })
    .await;
    let result = run_fetch_job(&pool, &nordpool(&server), ts(PRICE_NOW))
        .await
        .unwrap();
    assert!(!result.tomorrow_available);
    assert_eq!(result.results.len(), 1);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn complete_days_skip_the_upstream(pool: PgPool) {
    fill_all_areas(&pool, "2026-03-29T00:00:00Z", 96).await;
    fill_all_areas(&pool, "2026-03-30T00:00:00Z", 96).await;
    let server = FakeServer::start(|_| raw(StatusCode::INTERNAL_SERVER_ERROR, "")).await;
    let result = run_fetch_job(&pool, &nordpool(&server), ts(PRICE_NOW))
        .await
        .unwrap();
    assert!(result.tomorrow_available);
    assert!(result.results.iter().all(|r| r.skipped));
    assert_eq!(server.recorded().len(), 0);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_failed_today_fetch_stops_the_run(pool: PgPool) {
    let server = FakeServer::start(|_| raw(StatusCode::INTERNAL_SERVER_ERROR, "")).await;
    let result = run_fetch_job(&pool, &nordpool(&server), ts(PRICE_NOW)).await;
    assert_eq!(
        result,
        Err(FetchJobError::NordPool(NordPoolError::Status(500)))
    );
    let dates: Vec<_> = server
        .recorded()
        .iter()
        .map(|r| r.param("date").unwrap())
        .collect();
    assert_eq!(dates, ["2026-03-29", "2026-03-29", "2026-03-29"]);
}

// --- Fingrid -----------------------------------------------------------------

const FINGRID_NOW: &str = "2026-04-15T12:00:00Z";

fn record(dataset_id: i32, start: Timestamp, value: f64) -> FingridRecord {
    FingridRecord {
        dataset_id,
        start_time: start,
        end_time: minutes(start, 15),
        value,
    }
}

fn fingrid_body(records: &[FingridRecord]) -> Value {
    let data: Vec<Value> = records
        .iter()
        .map(|r| {
            json!({
                "datasetId": r.dataset_id,
                "startTime": r.start_time.to_string(),
                "endTime": r.end_time.to_string(),
                "value": r.value
            })
        })
        .collect();
    json!({"data": data})
}

async fn fingrid_server(records: Vec<FingridRecord>) -> FakeServer {
    let body = fingrid_body(&records);
    FakeServer::start(move |_| json(StatusCode::OK, &body)).await
}

fn key() -> Secret {
    Secret::new("test-key".to_owned())
}

async fn count_vintages(pool: &PgPool, dataset_id: i32) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM fingrid_forecasts WHERE dataset_id = $1")
        .bind(dataset_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_fingrid_job_prunes_only_actuals_beyond_retention(pool: PgPool) {
    let now = ts(FINGRID_NOW);
    let expired = days(now, -(RETENTION_DAYS + 10));
    let accumulated = days(now, -(HISTORY_DAYS + 100));
    store_fingrid_records(
        &pool,
        &[
            record(DATASET_WIND_ACTUAL, expired, 999.0),
            record(DATASET_WIND_ACTUAL, accumulated, 555.0),
        ],
    )
    .await
    .unwrap();
    let server = fingrid_server(vec![record(DATASET_WIND_ACTUAL, now, 111.0)]).await;

    let result = run_forecast_fetch_job(&pool, &fingrid(&server), &key(), now)
        .await
        .unwrap();
    let ForecastFetchResult::Stored { stored, pruned, .. } = result else {
        panic!("expected a stored result: {result:?}");
    };
    assert_eq!((stored, pruned), (1, 1));
    let remaining =
        get_fingrid_records_by_range(&pool, DATASET_WIND_ACTUAL, days(expired, -1), days(now, 1))
            .await
            .unwrap();
    let values: Vec<f64> = remaining.iter().map(|r| r.value).collect();
    assert_eq!(values, [555.0, 111.0]);

    // The request window is [now - 31 d, now + 3 d).
    let request = &server.recorded()[0];
    assert_eq!(request.api_key.as_deref(), Some("test-key"));
    assert_eq!(
        request.param("startTime").as_deref(),
        Some("2026-03-15T12:00:00.000Z")
    );
    assert_eq!(
        request.param("endTime").as_deref(),
        Some("2026-04-18T12:00:00.000Z")
    );
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_fingrid_job_gives_each_dataset_class_one_home(pool: PgPool) {
    let now = ts("2026-04-15T12:34:56Z");
    let target = ts(FINGRID_NOW);
    store_fingrid_forecast_vintages(
        &pool,
        days(now, -3),
        &[record(DATASET_WIND_FORECAST, target, 100.0)],
    )
    .await
    .unwrap();
    // An issuance beyond retention; inserted directly, past the write guard.
    sqlx::query(
        "INSERT INTO fingrid_forecasts (dataset_id, issued_at, start_time, end_time, value)
         VALUES (245, $1::timestamptz, $2::timestamptz, $3::timestamptz, 50)",
    )
    .bind(days(now, -(VINTAGE_RETENTION_DAYS + 10)).to_string())
    .bind(target.to_string())
    .bind(minutes(target, 15).to_string())
    .execute(&pool)
    .await
    .unwrap();
    let server = fingrid_server(vec![
        record(DATASET_WIND_FORECAST, target, 200.0),
        record(DATASET_WIND_ACTUAL, target, 700.0),
    ])
    .await;

    let result = run_forecast_fetch_job(&pool, &fingrid(&server), &key(), now)
        .await
        .unwrap();
    assert_eq!(
        result,
        ForecastFetchResult::Stored {
            stored: 1,
            pruned: 0,
            vintage_stored: 1,
            vintage_pruned: 1,
            vintage_degraded_reason: None,
        }
    );
    let actual_forecasts = get_fingrid_records_by_range(
        &pool,
        DATASET_WIND_FORECAST,
        days(target, -1),
        days(target, 1),
    )
    .await
    .unwrap();
    assert_eq!(actual_forecasts.len(), 0);
    assert_eq!(count_vintages(&pool, DATASET_WIND_FORECAST).await, 2);
    assert_eq!(count_vintages(&pool, DATASET_WIND_ACTUAL).await, 0);
    let latest = get_fingrid_forecast_vintages_latest(
        &pool,
        DATASET_WIND_FORECAST,
        days(target, -1),
        days(target, 1),
    )
    .await
    .unwrap();
    assert!((latest[0].value - 200.0).abs() < f64::EPSILON);
    let issued: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM fingrid_forecasts WHERE issued_at = '2026-04-15T12:00:00Z'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        issued, 1,
        "the issuance is the run time truncated to the UTC hour"
    );
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_fingrid_job_archives_only_the_in_window_share(pool: PgPool) {
    let now = ts(FINGRID_NOW);
    let mut records = Vec::new();
    let mut t = days(now, -HISTORY_DAYS);
    while t < days(now, FORECAST_DAYS) {
        records.push(record(DATASET_WIND_FORECAST, t, 1.0));
        records.push(record(DATASET_CONSUMPTION_FORECAST, t, 2.0));
        t = hours(t, 6);
    }
    let cutoff = hours(now, -VINTAGE_BACKFILL_HOURS);
    let in_window = records.iter().filter(|r| r.start_time >= cutoff).count();
    let forecast_count = records.len();
    records.push(record(DATASET_WIND_ACTUAL, now, 700.0));
    let server = fingrid_server(records).await;

    let result = run_forecast_fetch_job(&pool, &fingrid(&server), &key(), now)
        .await
        .unwrap();
    let ForecastFetchResult::Stored { vintage_stored, .. } = result else {
        panic!("expected a stored result");
    };
    assert_eq!(vintage_stored, u64::try_from(in_window).unwrap());
    assert!(forecast_count > 9 * in_window);
    let oldest: String = sqlx::query_scalar(
        r#"SELECT to_char(MIN(start_time) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') FROM fingrid_forecasts"#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(ts(&oldest) >= cutoff);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_vintage_failure_keeps_the_committed_actuals(pool: PgPool) {
    let now = ts(FINGRID_NOW);
    sqlx::raw_sql("DROP TABLE fingrid_forecasts")
        .execute(&pool)
        .await
        .unwrap();
    let server = fingrid_server(vec![
        record(DATASET_WIND_FORECAST, now, 200.0),
        record(DATASET_WIND_ACTUAL, now, 700.0),
    ])
    .await;

    let result = run_forecast_fetch_job(&pool, &fingrid(&server), &key(), now)
        .await
        .unwrap();
    assert_eq!(
        result,
        ForecastFetchResult::Stored {
            stored: 1,
            pruned: 0,
            vintage_stored: 0,
            vintage_pruned: 0,
            vintage_degraded_reason: Some("the database query failed".to_owned()),
        }
    );
    let actuals =
        get_fingrid_records_by_range(&pool, DATASET_WIND_ACTUAL, days(now, -1), days(now, 1))
            .await
            .unwrap();
    assert_eq!(actuals, [record(DATASET_WIND_ACTUAL, now, 700.0)]);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_archive_job_stores_only_the_archived_datasets(pool: PgPool) {
    let now = ts("2026-04-15T12:34:56Z");
    let issued_at = ts("2026-04-15T12:00:00Z");
    let too_old = minutes(hours(issued_at, -VINTAGE_BACKFILL_HOURS), -15);
    let server = fingrid_server(vec![
        record(DATASET_CONSUMPTION_FORECAST_72H, hours(now, 71), 9000.0),
        record(DATASET_SOLAR_FORECAST, hours(now, 2), 300.0),
        record(DATASET_SOLAR_FORECAST, too_old, 100.0),
        // A body that also holds other datasets must not write them.
        record(DATASET_WIND_FORECAST, hours(now, 2), 200.0),
        record(DATASET_WIND_ACTUAL, now, 700.0),
    ])
    .await;

    let result = run_vintage_archive_job(&pool, &fingrid(&server), &key(), now)
        .await
        .unwrap();
    assert_eq!(result, ArchiveFetchResult::Stored { vintage_stored: 2 });
    assert_eq!(
        count_vintages(&pool, DATASET_CONSUMPTION_FORECAST_72H).await,
        1
    );
    assert_eq!(count_vintages(&pool, DATASET_SOLAR_FORECAST).await, 1);
    assert_eq!(count_vintages(&pool, DATASET_WIND_FORECAST).await, 0);
    assert_eq!(count_vintages(&pool, DATASET_WIND_ACTUAL).await, 0);
    let actuals =
        get_fingrid_records_by_range(&pool, DATASET_WIND_ACTUAL, days(now, -1), days(now, 1))
            .await
            .unwrap();
    assert_eq!(actuals.len(), 0);
    let issued: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM fingrid_forecasts WHERE issued_at = '2026-04-15T12:00:00Z'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        issued, 2,
        "the issuance is the fetch time cut to the UTC hour"
    );

    let requests = server.recorded();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.param("datasets").as_deref(), Some("166,248"));
    assert_eq!(
        request.param("startTime").as_deref(),
        Some("2026-04-15T06:00:00.000Z")
    );
    assert_eq!(
        request.param("endTime").as_deref(),
        Some("2026-04-18T13:34:56.000Z")
    );
    assert_eq!(ARCHIVE_HORIZON_HOURS, 73);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn repeated_archive_runs_keep_one_version_per_issuance(pool: PgPool) {
    let target = ts("2026-04-16T10:00:00Z");
    for (now, value, inserted) in [
        ("2026-04-15T12:10:00Z", 1.0, 1),
        ("2026-04-15T13:10:00Z", 2.0, 1),
        // The same fetch hour again: the first version of the hour stays.
        ("2026-04-15T13:40:00Z", 3.0, 0),
    ] {
        let server = fingrid_server(vec![
            record(DATASET_SOLAR_FORECAST, target, value),
            record(DATASET_CONSUMPTION_FORECAST_72H, target, value * 1000.0),
        ])
        .await;
        let result = run_vintage_archive_job(&pool, &fingrid(&server), &key(), ts(now))
            .await
            .unwrap();
        assert_eq!(
            result,
            ArchiveFetchResult::Stored {
                vintage_stored: 2 * inserted
            },
            "{now}"
        );
    }
    let all = get_fingrid_forecast_vintages_all(
        &pool,
        DATASET_SOLAR_FORECAST,
        days(target, -1),
        days(target, 1),
    )
    .await
    .unwrap();
    let versions: Vec<_> = all.iter().map(|v| (v.issued_at, v.value)).collect();
    assert_eq!(
        versions,
        [
            (ts("2026-04-15T12:00:00Z"), 1.0),
            (ts("2026-04-15T13:00:00Z"), 2.0),
        ]
    );
    assert_eq!(
        count_vintages(&pool, DATASET_CONSUMPTION_FORECAST_72H).await,
        2
    );
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_degraded_archive_fetch_writes_nothing(pool: PgPool) {
    let server = FakeServer::start(|_| raw(StatusCode::SERVICE_UNAVAILABLE, "{}")).await;
    let result = run_vintage_archive_job(&pool, &fingrid(&server), &key(), ts(FINGRID_NOW))
        .await
        .unwrap();
    assert_eq!(
        result,
        ArchiveFetchResult::Degraded {
            reason: "Fingrid returned HTTP 503".to_owned()
        }
    );
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fingrid_forecasts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total, 0);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_fingrid_job_prunes_archived_datasets_by_issuance(pool: PgPool) {
    let now = ts(FINGRID_NOW);
    let expired = days(now, -(VINTAGE_RETENTION_DAYS + 1));
    for dataset_id in [DATASET_CONSUMPTION_FORECAST_72H, DATASET_SOLAR_FORECAST] {
        store_fingrid_forecast_vintages(&pool, expired, &[record(dataset_id, expired, 1.0)])
            .await
            .unwrap();
        store_fingrid_forecast_vintages(&pool, now, &[record(dataset_id, now, 2.0)])
            .await
            .unwrap();
    }
    let server = fingrid_server(vec![record(DATASET_WIND_ACTUAL, now, 700.0)]).await;

    let result = run_forecast_fetch_job(&pool, &fingrid(&server), &key(), now)
        .await
        .unwrap();
    let ForecastFetchResult::Stored { vintage_pruned, .. } = result else {
        panic!("expected a stored result: {result:?}");
    };
    assert_eq!(vintage_pruned, 2);
    assert_eq!(
        count_vintages(&pool, DATASET_CONSUMPTION_FORECAST_72H).await,
        1
    );
    assert_eq!(count_vintages(&pool, DATASET_SOLAR_FORECAST).await, 1);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_degraded_fingrid_fetch_writes_nothing(pool: PgPool) {
    let server = FakeServer::start(|_| raw(StatusCode::UNAUTHORIZED, "{}")).await;
    let result = run_forecast_fetch_job(&pool, &fingrid(&server), &key(), ts(FINGRID_NOW))
        .await
        .unwrap();
    assert_eq!(
        result,
        ForecastFetchResult::Degraded {
            reason: "Fingrid auth failed (HTTP 401)".to_owned()
        }
    );
}

// --- Weather -----------------------------------------------------------------

const WEATHER_NOW: &str = "2026-06-13T13:00:00Z";
const HELSINKI_LAT: &str = "60.17";

fn hourly_entry(target: Timestamp, temp: f64) -> Value {
    json!({"dt": target.as_second(), "temp": temp, "clouds": 50, "uvi": 1, "wind_speed": 4, "wind_deg": 180})
}

fn daily_entry(temp_day: f64) -> Value {
    json!({
        "dt": ts("2026-06-15T09:00:00Z").as_second(),
        "sunrise": ts("2026-06-15T01:03:00Z").as_second(),
        "sunset": ts("2026-06-15T19:48:00Z").as_second(),
        "temp": {"morn": 9, "day": temp_day, "eve": 13, "night": 10, "min": 8, "max": 16},
        "clouds": 42,
        "uvi": 4.2
    })
}

/// A fake One Call server: `helsinki` and `vaasa` give the body of each
/// point, chosen by the `lat` query parameter.
async fn weather_server(
    helsinki: impl Fn() -> axum::response::Response + Send + Sync + 'static,
    vaasa: impl Fn() -> axum::response::Response + Send + Sync + 'static,
) -> FakeServer {
    FakeServer::start(move |request| {
        if request.param("lat").as_deref() == Some(HELSINKI_LAT) {
            helsinki()
        } else {
            vaasa()
        }
    })
    .await
}

fn weather_record(
    point: &str,
    issued_at: Timestamp,
    target: Timestamp,
    temp: f64,
) -> WeatherRecord {
    WeatherRecord {
        point_id: point.to_owned(),
        issued_at,
        target_time: target,
        temp,
        clouds: 50.0,
        uvi: 1.0,
        wind_speed: 4.0,
        wind_deg: 180.0,
    }
}

async fn hourly_temps(pool: &PgPool, point: &str, target: Timestamp) -> Vec<f64> {
    get_weather_records_by_range(pool, point, days(target, -1), days(target, 1))
        .await
        .unwrap()
        .iter()
        .map(|r| r.temp)
        .collect()
}

fn good(
    target: Timestamp,
    temp: f64,
    daily_temp: Option<f64>,
) -> impl Fn() -> axum::response::Response {
    move || {
        let daily: Vec<Value> = daily_temp.map(daily_entry).into_iter().collect();
        json(
            StatusCode::OK,
            &json!({"hourly": [hourly_entry(target, temp)], "daily": daily}),
        )
    }
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_weather_job_prunes_only_beyond_retention(pool: PgPool) {
    let now = ts(WEATHER_NOW);
    let target = days(now, 1);
    store_weather_records(
        &pool,
        &[
            weather_record(
                "helsinki",
                days(now, -(WEATHER_RETENTION_DAYS + 10)),
                target,
                1.0,
            ),
            weather_record("helsinki", days(now, -30), target, 2.0),
        ],
    )
    .await
    .unwrap();
    let server = weather_server(good(target, 3.0, None), good(target, 3.0, None)).await;

    let result = run_weather_fetch_job(&pool, &weather(&server), &key(), now)
        .await
        .unwrap();
    let WeatherJobResult::Ok { stored, pruned, .. } = result else {
        panic!("expected ok: {result:?}");
    };
    assert_eq!((stored, pruned), (2, 1));
    assert_eq!(hourly_temps(&pool, "helsinki", target).await, [2.0, 3.0]);
    assert_eq!(server.recorded().len(), 2, "one call per point");
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_weather_job_degrades_per_point(pool: PgPool) {
    let now = ts(WEATHER_NOW);
    let target = days(now, 1);
    let server = weather_server(
        || raw(StatusCode::UNAUTHORIZED, "{}"),
        good(target, 7.0, None),
    )
    .await;

    let result = run_weather_fetch_job(&pool, &weather(&server), &key(), now)
        .await
        .unwrap();
    let WeatherJobResult::Partial {
        stored, failures, ..
    } = result
    else {
        panic!("expected partial: {result:?}");
    };
    assert_eq!(stored, 1);
    assert_eq!(
        failures,
        [PointFailure {
            point_id: "helsinki",
            reason: "OpenWeatherMap auth failed (HTTP 401)".to_owned()
        }]
    );
    assert_eq!(hourly_temps(&pool, "vaasa", target).await, [7.0]);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_weather_job_stores_and_prunes_the_daily_block(pool: PgPool) {
    let now = ts(WEATHER_NOW);
    let target = days(now, 1);
    let date = jiff::civil::date(2026, 6, 15);
    store_weather_daily_records(
        &pool,
        &[
            daily(
                "helsinki",
                days(now, -(WEATHER_RETENTION_DAYS + 10)),
                date,
                1.0,
            ),
            daily("helsinki", days(now, -30), date, 2.0),
        ],
    )
    .await
    .unwrap();
    let server = weather_server(good(target, 3.0, Some(3.0)), good(target, 3.0, Some(3.0))).await;

    let result = run_weather_fetch_job(&pool, &weather(&server), &key(), now)
        .await
        .unwrap();
    let WeatherJobResult::Ok { daily, .. } = result else {
        panic!("expected ok");
    };
    assert_eq!(daily.stored, 2);
    assert_eq!(daily.failures.len(), 0);
    let temps: Vec<f64> = daily_rows(&pool, "helsinki")
        .await
        .iter()
        .map(|r| r.2)
        .collect();
    assert_eq!(temps, [2.0, 3.0]);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_broken_daily_block_keeps_every_hourly_row(pool: PgPool) {
    let now = ts(WEATHER_NOW);
    let target = days(now, 1);
    let server = weather_server(
        move || {
            json(
                StatusCode::OK,
                &json!({"hourly": [hourly_entry(target, 5.0)], "daily": [{"dt": "x"}]}),
            )
        },
        good(target, 7.0, Some(17.0)),
    )
    .await;

    let result = run_weather_fetch_job(&pool, &weather(&server), &key(), now)
        .await
        .unwrap();
    let WeatherJobResult::Ok { stored, daily, .. } = result else {
        panic!("a daily failure must not change the hourly status: {result:?}");
    };
    assert_eq!(stored, 2);
    assert_eq!(daily.stored, 1);
    assert_eq!(daily.failures.len(), 1);
    assert_eq!(daily.failures[0].point_id, "helsinki");
    assert!(daily.failures[0].reason.contains("daily"));
    assert_eq!(hourly_temps(&pool, "helsinki", target).await, [5.0]);
    assert_eq!(daily_rows(&pool, "helsinki").await.len(), 0);
    assert_eq!(daily_rows(&pool, "vaasa").await.len(), 1);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_broken_hourly_block_keeps_the_daily_rows(pool: PgPool) {
    let now = ts(WEATHER_NOW);
    let target = days(now, 1);
    let server = weather_server(
        || {
            json(
                StatusCode::OK,
                &json!({"hourly": [{"dt": "x"}], "daily": [daily_entry(21.0)]}),
            )
        },
        good(target, 7.0, Some(17.0)),
    )
    .await;

    let result = run_weather_fetch_job(&pool, &weather(&server), &key(), now)
        .await
        .unwrap();
    let WeatherJobResult::Partial {
        stored,
        failures,
        daily,
        ..
    } = result
    else {
        panic!("expected partial: {result:?}");
    };
    assert_eq!(stored, 1);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].point_id, "helsinki");
    assert_eq!(daily.stored, 2);
    assert_eq!(daily.failures.len(), 0);
    let helsinki = daily_rows(&pool, "helsinki").await;
    assert_eq!(helsinki.len(), 1);
    assert!((helsinki[0].2 - 21.0).abs() < f64::EPSILON);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn every_hourly_failure_stores_daily_rows_but_prunes_nothing(pool: PgPool) {
    let now = ts(WEATHER_NOW);
    store_weather_daily_records(
        &pool,
        &[daily(
            "helsinki",
            days(now, -(WEATHER_RETENTION_DAYS + 10)),
            jiff::civil::date(2026, 6, 15),
            1.0,
        )],
    )
    .await
    .unwrap();
    let broken_hourly = || {
        json(
            StatusCode::OK,
            &json!({"hourly": [{"dt": "x"}], "daily": [daily_entry(21.0)]}),
        )
    };
    let server = weather_server(broken_hourly, broken_hourly).await;

    let result = run_weather_fetch_job(&pool, &weather(&server), &key(), now)
        .await
        .unwrap();
    let WeatherJobResult::Failed { failures, daily } = result else {
        panic!("expected failed: {result:?}");
    };
    assert_eq!(failures.len(), 2);
    assert_eq!(daily.stored, 2);
    assert_eq!(daily_rows(&pool, "helsinki").await.len(), 2, "no prune");
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_daily_store_failure_is_reported_and_keeps_the_hourly_rows(pool: PgPool) {
    let now = ts(WEATHER_NOW);
    let target = days(now, 1);
    sqlx::raw_sql("DROP TABLE weather_daily_forecasts")
        .execute(&pool)
        .await
        .unwrap();
    let server = weather_server(good(target, 3.0, Some(3.0)), good(target, 4.0, Some(4.0))).await;

    let result = run_weather_fetch_job(&pool, &weather(&server), &key(), now)
        .await
        .unwrap();
    let WeatherJobResult::Ok { stored, daily, .. } = result else {
        panic!("expected ok: {result:?}");
    };
    assert_eq!(stored, 2);
    assert_eq!(daily.stored, 0);
    assert_eq!(daily.failures.len(), 2);
    assert_eq!(daily.failures[0].reason, "the database query failed");
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_total_weather_outage_reports_failure(pool: PgPool) {
    let server = FakeServer::start(|_| raw(StatusCode::INTERNAL_SERVER_ERROR, "{}")).await;
    let result = run_weather_fetch_job(&pool, &weather(&server), &key(), ts(WEATHER_NOW))
        .await
        .unwrap();
    let WeatherJobResult::Failed { failures, .. } = result else {
        panic!("expected failed");
    };
    assert_eq!(failures.len(), 2);
}
