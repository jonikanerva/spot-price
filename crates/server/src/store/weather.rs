//! Public `OpenWeatherMap` forecasts (STACK.md §5).
//!
//! Both tables are append-only per issuance: the inserts use
//! `ON CONFLICT DO NOTHING`. Never change them to `DO UPDATE`. The hourly
//! and the daily writes never share a transaction.

use jiff::Timestamp;
use jiff::civil::Date;
use jiff_sqlx::ToSqlx;
use sqlx::PgPool;

use crate::db::StoreError;

/// One issued hourly forecast of one point.
#[derive(Debug, Clone, PartialEq)]
pub struct WeatherRecord {
    pub point_id: String,
    /// The issuance hour: the fetch time truncated to the UTC hour.
    pub issued_at: Timestamp,
    pub target_time: Timestamp,
    pub temp: f64,
    pub clouds: f64,
    pub uvi: f64,
    pub wind_speed: f64,
    pub wind_deg: f64,
}

/// One issued daily forecast of one point.
#[derive(Debug, Clone, PartialEq)]
pub struct WeatherDailyRecord {
    pub point_id: String,
    /// The issuance hour: the fetch time truncated to the UTC hour.
    pub issued_at: Timestamp,
    /// The UTC calendar date of `target_dt`.
    pub target_date: Date,
    pub target_dt: Timestamp,
    pub temp_morn: f64,
    pub temp_day: f64,
    pub temp_eve: f64,
    pub temp_night: f64,
    pub temp_min: f64,
    pub temp_max: f64,
    pub clouds: f64,
    pub uvi: f64,
    /// `None` at polar latitudes during midnight sun and polar night.
    pub sunrise: Option<Timestamp>,
    pub sunset: Option<Timestamp>,
}

/// Inserts hourly records in one transaction. Returns the number of
/// inserted rows; a repeated issuance inserts none.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when a statement fails. Nothing is written
/// then.
pub async fn store_weather_records(
    pool: &PgPool,
    records: &[WeatherRecord],
) -> Result<u64, StoreError> {
    if records.is_empty() {
        return Ok(0);
    }
    let mut inserted = 0;
    let mut transaction = pool.begin().await?;
    for record in records {
        let result = sqlx::query!(
            r#"
            INSERT INTO weather_forecasts
              (point_id, issued_at, target_time, temp, clouds, uvi, wind_speed, wind_deg)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (point_id, issued_at, target_time) DO NOTHING
            "#,
            record.point_id,
            record.issued_at.to_sqlx(),
            record.target_time.to_sqlx(),
            record.temp,
            record.clouds,
            record.uvi,
            record.wind_speed,
            record.wind_deg,
        )
        .execute(&mut *transaction)
        .await?;
        inserted += result.rows_affected();
    }
    transaction.commit().await?;
    Ok(inserted)
}

/// The hourly records of one point with `target_time` in `[start, end)`,
/// ordered by `(target_time, issued_at)`.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the query fails.
pub async fn get_weather_records_by_range(
    pool: &PgPool,
    point_id: &str,
    start: Timestamp,
    end: Timestamp,
) -> Result<Vec<WeatherRecord>, StoreError> {
    let rows = sqlx::query!(
        r#"
        SELECT point_id, issued_at, target_time, temp, clouds, uvi, wind_speed, wind_deg
        FROM weather_forecasts
        WHERE point_id = $1 AND target_time >= $2 AND target_time < $3
        ORDER BY target_time, issued_at
        "#,
        point_id,
        start.to_sqlx(),
        end.to_sqlx(),
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| WeatherRecord {
            point_id: row.point_id,
            issued_at: row.issued_at.to_jiff(),
            target_time: row.target_time.to_jiff(),
            temp: row.temp,
            clouds: row.clouds,
            uvi: row.uvi,
            wind_speed: row.wind_speed,
            wind_deg: row.wind_deg,
        })
        .collect())
}

/// Deletes hourly rows whose issuance is before `before`. Returns the
/// number of deleted rows.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the statement fails.
pub async fn prune_weather_records_before(
    pool: &PgPool,
    before: Timestamp,
) -> Result<u64, StoreError> {
    let result = sqlx::query!(
        "DELETE FROM weather_forecasts WHERE issued_at < $1",
        before.to_sqlx()
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Inserts daily records in one transaction. Returns the number of inserted
/// rows; a repeated issuance inserts none.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when a statement fails. Nothing is written
/// then.
pub async fn store_weather_daily_records(
    pool: &PgPool,
    records: &[WeatherDailyRecord],
) -> Result<u64, StoreError> {
    if records.is_empty() {
        return Ok(0);
    }
    let mut inserted = 0;
    let mut transaction = pool.begin().await?;
    for record in records {
        let result = sqlx::query!(
            r#"
            INSERT INTO weather_daily_forecasts
              (point_id, issued_at, target_date, target_dt,
               temp_morn, temp_day, temp_eve, temp_night, temp_min, temp_max,
               clouds, uvi, sunrise, sunset)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            ON CONFLICT (point_id, issued_at, target_date) DO NOTHING
            "#,
            record.point_id,
            record.issued_at.to_sqlx(),
            // crates/server/sqlx.toml maps only TIMESTAMPTZ, so the DATE
            // parameter needs an explicit type.
            record.target_date.to_sqlx() as jiff_sqlx::Date,
            record.target_dt.to_sqlx(),
            record.temp_morn,
            record.temp_day,
            record.temp_eve,
            record.temp_night,
            record.temp_min,
            record.temp_max,
            record.clouds,
            record.uvi,
            record.sunrise.map(ToSqlx::to_sqlx),
            record.sunset.map(ToSqlx::to_sqlx),
        )
        .execute(&mut *transaction)
        .await?;
        inserted += result.rows_affected();
    }
    transaction.commit().await?;
    Ok(inserted)
}

/// Deletes daily rows whose issuance is before `before`. Returns the number
/// of deleted rows.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the statement fails.
pub async fn prune_weather_daily_records_before(
    pool: &PgPool,
    before: Timestamp,
) -> Result<u64, StoreError> {
    let result = sqlx::query!(
        "DELETE FROM weather_daily_forecasts WHERE issued_at < $1",
        before.to_sqlx()
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}
