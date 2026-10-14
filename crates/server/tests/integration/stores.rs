//! The stores against a migrated database.

use jiff::Timestamp;
use spot_price_domain::areas::Area;
use spot_price_server::store::fingrid::{
    DATASET_CONSUMPTION_ACTUAL, DATASET_CONSUMPTION_FORECAST, DATASET_WIND_ACTUAL,
    DATASET_WIND_FORECAST, FingridRecord, VINTAGE_BACKFILL_HOURS,
    get_fingrid_forecast_vintages_all, get_fingrid_forecast_vintages_latest,
    get_fingrid_records_by_range, prune_fingrid_forecast_vintages_before,
    prune_fingrid_records_before, store_fingrid_forecast_vintages, store_fingrid_records,
};
use spot_price_server::store::prices::{
    PriceRow, count_prices_by_range, get_latest_delivery_start, get_prices_by_areas,
    get_prices_by_range, store_prices,
};
use spot_price_server::store::user_settings::{
    UserSettingsRow, ensure_user_settings, get_user_settings, upsert_user_settings,
};
use spot_price_server::store::weather::{
    WeatherDailyRecord, WeatherRecord, get_weather_records_by_range,
    prune_weather_daily_records_before, prune_weather_records_before, store_weather_daily_records,
    store_weather_records,
};
use sqlx::PgPool;

use crate::common::{days, hours, minutes, ts};

const NOW: &str = "2026-04-15T12:00:00Z";

fn price(start: &str, end: &str, value: f64, area: Area) -> PriceRow {
    PriceRow {
        delivery_start: ts(start),
        delivery_end: ts(end),
        price_eur_mwh: value,
        area,
    }
}

fn sample_prices() -> Vec<PriceRow> {
    vec![
        price(
            "2026-02-24T00:00:00Z",
            "2026-02-24T01:00:00Z",
            30.5,
            Area::Fi,
        ),
        price(
            "2026-02-24T01:00:00Z",
            "2026-02-24T02:00:00Z",
            28.3,
            Area::Fi,
        ),
        price(
            "2026-02-24T02:00:00Z",
            "2026-02-24T03:00:00Z",
            25.1,
            Area::Fi,
        ),
    ]
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn stores_prices_and_reads_them_by_utc_range(pool: PgPool) {
    assert_eq!(store_prices(&pool, &sample_prices()).await.unwrap(), 3);
    let read = get_prices_by_range(
        &pool,
        ts("2026-02-24T00:00:00Z"),
        ts("2026-02-24T03:00:00Z"),
        Area::Fi,
    )
    .await
    .unwrap();
    assert_eq!(read, sample_prices());
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_repeated_price_overwrites_the_stored_one(pool: PgPool) {
    store_prices(&pool, &sample_prices()).await.unwrap();
    store_prices(
        &pool,
        &[price(
            "2026-02-24T00:00:00Z",
            "2026-02-24T01:00:00Z",
            99.9,
            Area::Fi,
        )],
    )
    .await
    .unwrap();
    let read = get_prices_by_range(
        &pool,
        ts("2026-02-24T00:00:00Z"),
        ts("2026-02-24T01:00:00Z"),
        Area::Fi,
    )
    .await
    .unwrap();
    assert_eq!(read.len(), 1);
    assert!((read[0].price_eur_mwh - 99.9).abs() < f64::EPSILON);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn counts_prices_by_utc_range(pool: PgPool) {
    store_prices(&pool, &sample_prices()).await.unwrap();
    let day = |start: &str, end: &str| count_prices_by_range(&pool, ts(start), ts(end), Area::Fi);
    assert_eq!(
        day("2026-02-24T00:00:00Z", "2026-02-25T00:00:00Z")
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        day("2026-02-25T00:00:00Z", "2026-02-26T00:00:00Z")
            .await
            .unwrap(),
        0
    );
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_range_includes_its_start_and_excludes_its_end(pool: PgPool) {
    store_prices(&pool, &sample_prices()).await.unwrap();
    let with_start = get_prices_by_range(
        &pool,
        ts("2026-02-24T00:00:00Z"),
        ts("2026-02-24T00:30:00Z"),
        Area::Fi,
    )
    .await
    .unwrap();
    assert_eq!(with_start.len(), 1);
    let without_end = get_prices_by_range(
        &pool,
        ts("2026-02-24T03:00:00Z"),
        ts("2026-02-24T04:00:00Z"),
        Area::Fi,
    )
    .await
    .unwrap();
    assert_eq!(without_end.len(), 0);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn reads_several_areas_grouped_and_the_latest_start(pool: PgPool) {
    let mut prices = sample_prices();
    prices.push(price(
        "2026-02-24T00:00:00Z",
        "2026-02-24T01:00:00Z",
        10.0,
        Area::Se3,
    ));
    prices.push(price(
        "2026-02-24T01:00:00Z",
        "2026-02-24T02:00:00Z",
        11.0,
        Area::Se3,
    ));
    prices.push(price(
        "2026-02-24T00:00:00Z",
        "2026-02-24T01:00:00Z",
        12.0,
        Area::Ee,
    ));
    store_prices(&pool, &prices).await.unwrap();

    let grouped = get_prices_by_areas(
        &pool,
        ts("2026-02-24T00:00:00Z"),
        ts("2026-02-25T00:00:00Z"),
        &[Area::Se3, Area::Se1, Area::Fi],
    )
    .await
    .unwrap();
    assert_eq!(
        grouped.keys().copied().collect::<Vec<_>>(),
        [Area::Fi, Area::Se3]
    );
    assert_eq!(grouped[&Area::Se3].len(), 2);
    assert!(grouped[&Area::Se3][0].delivery_start < grouped[&Area::Se3][1].delivery_start);
    assert!(
        get_prices_by_areas(
            &pool,
            ts("2026-02-24T00:00:00Z"),
            ts("2026-02-25T00:00:00Z"),
            &[]
        )
        .await
        .unwrap()
        .is_empty()
    );

    assert_eq!(
        get_latest_delivery_start(&pool, Area::Fi).await.unwrap(),
        Some(ts("2026-02-24T02:00:00Z"))
    );
    assert_eq!(
        get_latest_delivery_start(&pool, Area::No4).await.unwrap(),
        None
    );
}

// --- Fingrid -------------------------------------------------------------

fn record(dataset_id: i32, start: Timestamp, value: f64) -> FingridRecord {
    FingridRecord {
        dataset_id,
        start_time: start,
        end_time: minutes(start, 15),
        value,
    }
}

const ANCHOR_FETCHED_AT: &str = "2020-01-01T00:00:00Z";

/// Writes a fixed old `fetched_at`, so a rewrite of a row is visible.
async fn anchor_fetched_at(pool: &PgPool) {
    sqlx::query("UPDATE fingrid_actuals SET fetched_at = $1::timestamptz")
        .bind(ANCHOR_FETCHED_AT)
        .execute(pool)
        .await
        .unwrap();
}

/// `(fetched_at, end_time, value)` of one actual row.
async fn actual_row(pool: &PgPool, dataset_id: i32, start: Timestamp) -> (String, String, f64) {
    sqlx::query_as(
        r#"SELECT to_char(fetched_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
                  to_char(end_time AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'), value
           FROM fingrid_actuals WHERE dataset_id = $1 AND start_time = $2::timestamptz"#,
    )
    .bind(dataset_id)
    .bind(start.to_string())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn count_vintages(pool: &PgPool, dataset_id: i32) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM fingrid_forecasts WHERE dataset_id = $1")
        .bind(dataset_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn actuals_upsert_and_read_back_by_range(pool: PgPool) {
    let now = ts(NOW);
    store_fingrid_records(&pool, &[record(DATASET_WIND_ACTUAL, now, 100.0)])
        .await
        .unwrap();
    store_fingrid_records(&pool, &[record(DATASET_WIND_ACTUAL, now, 200.0)])
        .await
        .unwrap();
    let rows =
        get_fingrid_records_by_range(&pool, DATASET_WIND_ACTUAL, days(now, -1), days(now, 1))
            .await
            .unwrap();
    assert_eq!(rows, [record(DATASET_WIND_ACTUAL, now, 200.0)]);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_change_guard_keeps_an_unchanged_row(pool: PgPool) {
    let now = ts(NOW);
    store_fingrid_records(&pool, &[record(DATASET_WIND_ACTUAL, now, 100.0)])
        .await
        .unwrap();
    anchor_fetched_at(&pool).await;
    store_fingrid_records(&pool, &[record(DATASET_WIND_ACTUAL, now, 100.0)])
        .await
        .unwrap();
    let (fetched_at, _, value) = actual_row(&pool, DATASET_WIND_ACTUAL, now).await;
    assert_eq!(fetched_at, ANCHOR_FETCHED_AT);
    assert!((value - 100.0).abs() < f64::EPSILON);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_change_guard_rewrites_a_changed_value_or_end_time(pool: PgPool) {
    let now = ts(NOW);
    store_fingrid_records(&pool, &[record(DATASET_WIND_ACTUAL, now, 100.0)])
        .await
        .unwrap();
    anchor_fetched_at(&pool).await;
    store_fingrid_records(&pool, &[record(DATASET_WIND_ACTUAL, now, 200.0)])
        .await
        .unwrap();
    let (fetched_at, _, value) = actual_row(&pool, DATASET_WIND_ACTUAL, now).await;
    assert_ne!(fetched_at, ANCHOR_FETCHED_AT);
    assert!((value - 200.0).abs() < f64::EPSILON);

    anchor_fetched_at(&pool).await;
    let stretched = FingridRecord {
        end_time: hours(now, 1),
        ..record(DATASET_WIND_ACTUAL, now, 200.0)
    };
    store_fingrid_records(&pool, &[stretched]).await.unwrap();
    let (fetched_at, end_time, _) = actual_row(&pool, DATASET_WIND_ACTUAL, now).await;
    assert_ne!(fetched_at, ANCHOR_FETCHED_AT);
    assert_eq!(end_time, hours(now, 1).to_string());
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_change_guard_applies_per_row_in_one_batch(pool: PgPool) {
    let t: Vec<Timestamp> = (0..4).map(|i| minutes(ts(NOW), 15 * i)).collect();
    let batch = |values: [f64; 4]| -> Vec<FingridRecord> {
        t.iter()
            .zip(values)
            .map(|(start, value)| record(DATASET_WIND_ACTUAL, *start, value))
            .collect()
    };
    store_fingrid_records(&pool, &batch([100.0, 101.0, 102.0, 103.0]))
        .await
        .unwrap();
    anchor_fetched_at(&pool).await;
    store_fingrid_records(&pool, &batch([100.0, 999.0, 102.0, 888.0]))
        .await
        .unwrap();

    for (index, changed) in [(0, false), (1, true), (2, false), (3, true)] {
        let (fetched_at, _, _) = actual_row(&pool, DATASET_WIND_ACTUAL, t[index]).await;
        assert_eq!(fetched_at != ANCHOR_FETCHED_AT, changed, "row {index}");
    }
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_actual_count_is_the_records_handed_in(pool: PgPool) {
    let now = ts(NOW);
    let batch = [
        record(DATASET_WIND_ACTUAL, now, 100.0),
        record(DATASET_CONSUMPTION_ACTUAL, now, 8000.0),
    ];
    assert_eq!(store_fingrid_records(&pool, &batch).await.unwrap(), 2);
    anchor_fetched_at(&pool).await;
    assert_eq!(store_fingrid_records(&pool, &batch).await.unwrap(), 2);
    let (fetched_at, _, _) = actual_row(&pool, DATASET_WIND_ACTUAL, now).await;
    assert_eq!(fetched_at, ANCHOR_FETCHED_AT);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn prunes_actuals_by_start_time(pool: PgPool) {
    let now = ts(NOW);
    store_fingrid_records(
        &pool,
        &[
            record(DATASET_WIND_ACTUAL, days(now, -10), 1.0),
            record(DATASET_WIND_ACTUAL, now, 2.0),
        ],
    )
    .await
    .unwrap();
    assert_eq!(
        prune_fingrid_records_before(&pool, days(now, -1))
            .await
            .unwrap(),
        1
    );
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn vintages_are_append_only_per_issuance(pool: PgPool) {
    let now = ts(NOW);
    let first = store_fingrid_forecast_vintages(
        &pool,
        hours(now, -2),
        &[record(DATASET_WIND_FORECAST, now, 100.0)],
    )
    .await
    .unwrap();
    let second =
        store_fingrid_forecast_vintages(&pool, now, &[record(DATASET_WIND_FORECAST, now, 200.0)])
            .await
            .unwrap();
    assert_eq!((first, second), (1, 1));
    assert_eq!(count_vintages(&pool, DATASET_WIND_FORECAST).await, 2);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_live_read_returns_one_row_per_target_from_the_newest_issuance(pool: PgPool) {
    let now = ts(NOW);
    for (issuance, value) in [(-44, 10.0), (-1, 30.0), (-12, 20.0)] {
        store_fingrid_forecast_vintages(
            &pool,
            hours(now, issuance),
            &[record(DATASET_WIND_FORECAST, now, value)],
        )
        .await
        .unwrap();
    }
    assert_eq!(count_vintages(&pool, DATASET_WIND_FORECAST).await, 3);
    let latest = get_fingrid_forecast_vintages_latest(
        &pool,
        DATASET_WIND_FORECAST,
        days(now, -1),
        days(now, 1),
    )
    .await
    .unwrap();
    assert_eq!(latest, [record(DATASET_WIND_FORECAST, now, 30.0)]);

    let all = get_fingrid_forecast_vintages_all(
        &pool,
        DATASET_WIND_FORECAST,
        days(now, -1),
        days(now, 1),
    )
    .await
    .unwrap();
    let issuances: Vec<Timestamp> = all.iter().map(|v| v.issued_at).collect();
    assert_eq!(
        issuances,
        [hours(now, -44), hours(now, -12), hours(now, -1)]
    );
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_live_read_keeps_targets_in_order_and_inside_the_range(pool: PgPool) {
    let now = ts(NOW);
    let records: Vec<FingridRecord> = (0..4)
        .rev()
        .map(|i| {
            record(
                DATASET_WIND_FORECAST,
                minutes(now, 15 * i),
                f64::from(i32::try_from(i).unwrap()),
            )
        })
        .collect();
    store_fingrid_forecast_vintages(&pool, now, &records)
        .await
        .unwrap();
    store_fingrid_forecast_vintages(
        &pool,
        now,
        &[record(DATASET_CONSUMPTION_FORECAST, now, 9.0)],
    )
    .await
    .unwrap();
    let latest = get_fingrid_forecast_vintages_latest(
        &pool,
        DATASET_WIND_FORECAST,
        minutes(now, 15),
        minutes(now, 45),
    )
    .await
    .unwrap();
    let starts: Vec<Timestamp> = latest.iter().map(|r| r.start_time).collect();
    assert_eq!(starts, [minutes(now, 15), minutes(now, 30)]);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_repeated_issuance_inserts_nothing_and_keeps_the_first_value(pool: PgPool) {
    let now = ts(NOW);
    let first =
        store_fingrid_forecast_vintages(&pool, now, &[record(DATASET_WIND_FORECAST, now, 100.0)])
            .await
            .unwrap();
    let second =
        store_fingrid_forecast_vintages(&pool, now, &[record(DATASET_WIND_FORECAST, now, 999.0)])
            .await
            .unwrap();
    assert_eq!((first, second), (1, 0));
    let latest = get_fingrid_forecast_vintages_latest(
        &pool,
        DATASET_WIND_FORECAST,
        days(now, -1),
        days(now, 1),
    )
    .await
    .unwrap();
    assert!((latest[0].value - 100.0).abs() < f64::EPSILON);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_vintage_guards_drop_actuals_and_stale_targets(pool: PgPool) {
    let now = ts(NOW);
    let edge = hours(now, -VINTAGE_BACKFILL_HOURS);
    let inserted = store_fingrid_forecast_vintages(
        &pool,
        now,
        &[
            record(DATASET_WIND_FORECAST, days(now, -2), 100.0),
            record(DATASET_WIND_FORECAST, hours(now, 3), 200.0),
            record(DATASET_CONSUMPTION_FORECAST, hours(now, 3), 300.0),
            record(DATASET_WIND_ACTUAL, hours(now, 3), 700.0),
            record(DATASET_CONSUMPTION_ACTUAL, days(now, -2), 8000.0),
            // The bound is inclusive: the issuance itself and the window edge
            // stay; one millisecond before the edge drops.
            record(DATASET_WIND_FORECAST, now, 42.0),
            record(DATASET_WIND_FORECAST, edge, 10.0),
            record(
                DATASET_WIND_FORECAST,
                edge.checked_sub(jiff::SignedDuration::from_millis(1))
                    .unwrap(),
                20.0,
            ),
        ],
    )
    .await
    .unwrap();
    assert_eq!(inserted, 4);
    assert_eq!(count_vintages(&pool, DATASET_WIND_FORECAST).await, 3);
    assert_eq!(count_vintages(&pool, DATASET_CONSUMPTION_FORECAST).await, 1);
    assert_eq!(count_vintages(&pool, DATASET_WIND_ACTUAL).await, 0);
    assert_eq!(count_vintages(&pool, DATASET_CONSUMPTION_ACTUAL).await, 0);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_live_read_still_returns_a_vintage_older_than_the_write_guard(pool: PgPool) {
    let now = ts(NOW);
    let past = days(now, -5);
    sqlx::query(
        "INSERT INTO fingrid_forecasts (dataset_id, issued_at, start_time, end_time, value)
         VALUES ($1, $2::timestamptz, $3::timestamptz, $4::timestamptz, 123)",
    )
    .bind(DATASET_WIND_FORECAST)
    .bind(now.to_string())
    .bind(past.to_string())
    .bind(minutes(past, 15).to_string())
    .execute(&pool)
    .await
    .unwrap();
    let inserted =
        store_fingrid_forecast_vintages(&pool, now, &[record(DATASET_WIND_FORECAST, past, 456.0)])
            .await
            .unwrap();
    assert_eq!(inserted, 0);
    let latest = get_fingrid_forecast_vintages_latest(
        &pool,
        DATASET_WIND_FORECAST,
        days(past, -1),
        days(past, 1),
    )
    .await
    .unwrap();
    assert_eq!(latest.len(), 1);
    assert!((latest[0].value - 123.0).abs() < f64::EPSILON);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn prunes_vintages_by_issuance(pool: PgPool) {
    let now = ts(NOW);
    store_fingrid_forecast_vintages(
        &pool,
        days(now, -10),
        &[record(DATASET_WIND_FORECAST, days(now, -10), 1.0)],
    )
    .await
    .unwrap();
    store_fingrid_forecast_vintages(&pool, now, &[record(DATASET_WIND_FORECAST, now, 3.0)])
        .await
        .unwrap();
    assert_eq!(
        prune_fingrid_forecast_vintages_before(&pool, days(now, -1))
            .await
            .unwrap(),
        1
    );
    assert_eq!(count_vintages(&pool, DATASET_WIND_FORECAST).await, 1);
}

// --- Weather ---------------------------------------------------------------

const WEATHER_NOW: &str = "2026-06-13T13:00:00Z";

fn weather(point: &str, issued_at: Timestamp, target: Timestamp, temp: f64) -> WeatherRecord {
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

pub fn daily(
    point: &str,
    issued_at: Timestamp,
    date: jiff::civil::Date,
    temp_day: f64,
) -> WeatherDailyRecord {
    let at = |time: jiff::civil::Time| {
        date.to_datetime(time)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap()
            .timestamp()
    };
    WeatherDailyRecord {
        point_id: point.to_owned(),
        issued_at,
        target_date: date,
        target_dt: at(jiff::civil::time(9, 0, 0, 0)),
        temp_morn: 9.0,
        temp_day,
        temp_eve: 13.0,
        temp_night: 10.0,
        temp_min: 8.0,
        temp_max: 16.0,
        clouds: 42.0,
        uvi: 4.2,
        sunrise: Some(at(jiff::civil::time(1, 3, 0, 0))),
        sunset: Some(at(jiff::civil::time(19, 48, 0, 0))),
    }
}

/// `(issued_at, target_date, temp_day)` of the daily rows of one point.
pub async fn daily_rows(pool: &PgPool, point: &str) -> Vec<(String, String, f64)> {
    sqlx::query_as(
        "SELECT issued_at::text, target_date::text, temp_day FROM weather_daily_forecasts
         WHERE point_id = $1 ORDER BY target_date, issued_at",
    )
    .bind(point)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn hourly_weather_is_append_only_per_issuance(pool: PgPool) {
    let now = ts(WEATHER_NOW);
    let target = days(now, 1);
    let a = store_weather_records(&pool, &[weather("helsinki", hours(now, -2), target, 10.0)])
        .await
        .unwrap();
    let b = store_weather_records(&pool, &[weather("helsinki", now, target, 12.0)])
        .await
        .unwrap();
    let repeated = store_weather_records(&pool, &[weather("helsinki", now, target, 99.0)])
        .await
        .unwrap();
    assert_eq!((a, b, repeated), (1, 1, 0));
    let rows = get_weather_records_by_range(&pool, "helsinki", days(target, -1), days(target, 1))
        .await
        .unwrap();
    let temps: Vec<f64> = rows.iter().map(|r| r.temp).collect();
    assert_eq!(temps, [10.0, 12.0]);
    assert_eq!(
        prune_weather_records_before(&pool, hours(now, -1))
            .await
            .unwrap(),
        1
    );
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn daily_weather_is_append_only_per_issuance(pool: PgPool) {
    let now = ts(WEATHER_NOW);
    let date = jiff::civil::date(2026, 6, 15);
    let a = store_weather_daily_records(&pool, &[daily("helsinki", hours(now, -2), date, 15.0)])
        .await
        .unwrap();
    let b = store_weather_daily_records(&pool, &[daily("helsinki", now, date, 17.0)])
        .await
        .unwrap();
    let repeated = store_weather_daily_records(&pool, &[daily("helsinki", now, date, 99.0)])
        .await
        .unwrap();
    assert_eq!((a, b, repeated), (1, 1, 0));
    let rows = daily_rows(&pool, "helsinki").await;
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|(_, date, _)| date == "2026-06-15"));
    assert_eq!(rows.iter().map(|r| r.2).collect::<Vec<_>>(), [15.0, 17.0]);
    assert_eq!(
        prune_weather_daily_records_before(&pool, hours(now, -1))
            .await
            .unwrap(),
        1
    );
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn stores_a_daily_row_without_solar_bounds(pool: PgPool) {
    let polar = WeatherDailyRecord {
        sunrise: None,
        sunset: None,
        ..daily(
            "helsinki",
            ts(WEATHER_NOW),
            jiff::civil::date(2026, 6, 15),
            15.0,
        )
    };
    assert_eq!(
        store_weather_daily_records(&pool, &[polar]).await.unwrap(),
        1
    );
    let (sunrise, sunset): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT sunrise::text, sunset::text FROM weather_daily_forecasts")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((sunrise, sunset), (None, None));
}

// --- User settings ---------------------------------------------------------

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn ensure_creates_the_defaults_once(pool: PgPool) {
    let now = ts(NOW);
    assert_eq!(get_user_settings(&pool, "u1").await.unwrap(), None);
    let created = ensure_user_settings(&pool, "u1", now).await.unwrap();
    assert_eq!(created, UserSettingsRow::defaults("u1".to_owned()));

    let custom = UserSettingsRow {
        margin_cents_kwh: 1.5,
        area: "SE3".to_owned(),
        timezone: "Europe/Stockholm".to_owned(),
        night_start_hour: 23,
        ..created
    };
    upsert_user_settings(&pool, &custom, now).await.unwrap();
    assert_eq!(
        ensure_user_settings(&pool, "u1", now).await.unwrap(),
        custom
    );
    assert_eq!(get_user_settings(&pool, "u1").await.unwrap(), Some(custom));
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn upsert_sets_updated_at_from_the_given_clock(pool: PgPool) {
    let now = ts("2026-05-01T08:30:00Z");
    upsert_user_settings(&pool, &UserSettingsRow::defaults("u2".to_owned()), now)
        .await
        .unwrap();
    let updated_at: String = sqlx::query_scalar(
        r#"SELECT to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') FROM user_settings"#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(updated_at, "2026-05-01T08:30:00Z");
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn timestamps_round_trip_with_microsecond_precision(pool: PgPool) {
    let start = ts("2026-02-24T00:00:00.123456Z");
    store_prices(
        &pool,
        &[PriceRow {
            delivery_start: start,
            ..sample_prices()[0]
        }],
    )
    .await
    .unwrap();
    let read = get_prices_by_range(&pool, start, minutes(start, 1), Area::Fi)
        .await
        .unwrap();
    assert_eq!(read[0].delivery_start, start);
}
