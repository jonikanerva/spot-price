//! Public Fingrid grid series (STACK.md §5).
//!
//! The actual datasets (75, 124) live in `fingrid_actuals`, upsert-latest.
//! The forecast datasets (245, 165) live only in `fingrid_forecasts`,
//! append-only per issuance. The two writes never share a transaction.

use jiff::{SignedDuration, Timestamp};
use jiff_sqlx::ToSqlx as _;
use sqlx::PgPool;

use crate::db::StoreError;

/// Wind power forecast, 15 min, about 72 h ahead.
pub const DATASET_WIND_FORECAST: i32 = 245;
/// Actual wind power, 15 min.
pub const DATASET_WIND_ACTUAL: i32 = 75;
/// Consumption forecast, 15 min, about 24 h ahead.
pub const DATASET_CONSUMPTION_FORECAST: i32 = 165;
/// Actual consumption.
pub const DATASET_CONSUMPTION_ACTUAL: i32 = 124;

/// All four datasets, in the order of the Fingrid request.
pub const FORECAST_DATASETS: [i32; 4] = [
    DATASET_WIND_FORECAST,
    DATASET_WIND_ACTUAL,
    DATASET_CONSUMPTION_FORECAST,
    DATASET_CONSUMPTION_ACTUAL,
];

/// How far before the issuance a target may lie and still be archived as a
/// vintage. A value below `REFERENCE_MAX_LEAD_H` of the revision-magnitude
/// study in `tools/` leaves that study without a reference vintage.
pub const VINTAGE_BACKFILL_HOURS: i64 = 6;

/// One Fingrid observation. `value` is in the dataset unit (MW).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FingridRecord {
    pub dataset_id: i32,
    pub start_time: Timestamp,
    pub end_time: Timestamp,
    pub value: f64,
}

/// One archived issuance of a forecast dataset value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForecastVintageRecord {
    pub dataset_id: i32,
    pub issued_at: Timestamp,
    pub start_time: Timestamp,
    pub end_time: Timestamp,
    pub value: f64,
}

/// Whether `dataset_id` is a forecast dataset (245 or 165).
#[must_use]
pub fn is_forecast_dataset(dataset_id: i32) -> bool {
    dataset_id == DATASET_WIND_FORECAST || dataset_id == DATASET_CONSUMPTION_FORECAST
}

/// Upserts actual records in one transaction. Returns the number of records
/// handed in, not the number of rows written.
///
/// The change guard rewrites a row only when its `(end_time, value)`
/// differs, so `fetched_at` means "last changed" (STACK.md §5).
///
/// # Errors
///
/// Returns [`StoreError::Query`] when a statement fails. Nothing is written
/// then.
pub async fn store_fingrid_records(
    pool: &PgPool,
    records: &[FingridRecord],
) -> Result<usize, StoreError> {
    if records.is_empty() {
        return Ok(0);
    }
    let mut transaction = pool.begin().await?;
    for record in records {
        sqlx::query!(
            r#"
            INSERT INTO fingrid_actuals (dataset_id, start_time, end_time, value)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (dataset_id, start_time)
            DO UPDATE SET end_time = EXCLUDED.end_time,
                          value = EXCLUDED.value,
                          fetched_at = NOW()
            WHERE (fingrid_actuals.end_time, fingrid_actuals.value)
                  IS DISTINCT FROM (EXCLUDED.end_time, EXCLUDED.value)
            "#,
            record.dataset_id,
            record.start_time.to_sqlx(),
            record.end_time.to_sqlx(),
            record.value,
        )
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(records.len())
}

/// The actual records of one dataset with `start_time` in `[start, end)`,
/// oldest first.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the query fails.
pub async fn get_fingrid_records_by_range(
    pool: &PgPool,
    dataset_id: i32,
    start: Timestamp,
    end: Timestamp,
) -> Result<Vec<FingridRecord>, StoreError> {
    let rows = sqlx::query!(
        r#"
        SELECT dataset_id, start_time, end_time, value
        FROM fingrid_actuals
        WHERE dataset_id = $1 AND start_time >= $2 AND start_time < $3
        ORDER BY start_time
        "#,
        dataset_id,
        start.to_sqlx(),
        end.to_sqlx(),
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| FingridRecord {
            dataset_id: row.dataset_id,
            start_time: row.start_time.to_jiff(),
            end_time: row.end_time.to_jiff(),
            value: row.value,
        })
        .collect())
}

/// Deletes actual rows with `start_time` before `before`. Returns the
/// number of deleted rows.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the statement fails.
pub async fn prune_fingrid_records_before(
    pool: &PgPool,
    before: Timestamp,
) -> Result<u64, StoreError> {
    let result = sqlx::query!(
        "DELETE FROM fingrid_actuals WHERE start_time < $1",
        before.to_sqlx()
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Archives forecast records under one issuance, append-only. Returns the
/// number of inserted rows; a repeated issuance inserts none.
///
/// Two guards run here, so no caller can widen them: only the forecast
/// datasets pass, and only targets with
/// `start_time >= issued_at - VINTAGE_BACKFILL_HOURS` pass. The bound is
/// inclusive.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when a statement fails. Nothing is written
/// then.
pub async fn store_fingrid_forecast_vintages(
    pool: &PgPool,
    issued_at: Timestamp,
    records: &[FingridRecord],
) -> Result<u64, StoreError> {
    let oldest_archived = issued_at
        .checked_sub(SignedDuration::from_hours(VINTAGE_BACKFILL_HOURS))
        .map_err(|_| StoreError::InvalidRow)?;
    let archived: Vec<&FingridRecord> = records
        .iter()
        .filter(|r| is_forecast_dataset(r.dataset_id) && r.start_time >= oldest_archived)
        .collect();
    if archived.is_empty() {
        return Ok(0);
    }
    let mut inserted = 0;
    let mut transaction = pool.begin().await?;
    for record in archived {
        let result = sqlx::query!(
            r#"
            INSERT INTO fingrid_forecasts (dataset_id, issued_at, start_time, end_time, value)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (dataset_id, issued_at, start_time) DO NOTHING
            "#,
            record.dataset_id,
            issued_at.to_sqlx(),
            record.start_time.to_sqlx(),
            record.end_time.to_sqlx(),
            record.value,
        )
        .execute(&mut *transaction)
        .await?;
        inserted += result.rows_affected();
    }
    transaction.commit().await?;
    Ok(inserted)
}

/// The live read of the forecast route: the newest issuance of each target
/// in `[start, end)`, exactly one row per `start_time`, oldest target first.
///
/// The query is a LATERAL skip-scan on purpose. Never change it to
/// `DISTINCT ON` (STACK.md §5).
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the query fails.
pub async fn get_fingrid_forecast_vintages_latest(
    pool: &PgPool,
    dataset_id: i32,
    start: Timestamp,
    end: Timestamp,
) -> Result<Vec<FingridRecord>, StoreError> {
    let rows = sqlx::query!(
        r#"
        SELECT v.dataset_id AS "dataset_id!", v.start_time AS "start_time!",
               v.end_time AS "end_time!", v.value AS "value!"
        FROM (
          SELECT DISTINCT start_time
          FROM fingrid_forecasts
          WHERE dataset_id = $1 AND start_time >= $2 AND start_time < $3
        ) AS targets
        CROSS JOIN LATERAL (
          SELECT dataset_id, start_time, end_time, value
          FROM fingrid_forecasts f
          WHERE f.dataset_id = $1 AND f.start_time = targets.start_time
          ORDER BY f.issued_at DESC
          LIMIT 1
        ) AS v
        ORDER BY v.start_time
        "#,
        dataset_id,
        start.to_sqlx(),
        end.to_sqlx(),
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| FingridRecord {
            dataset_id: row.dataset_id,
            start_time: row.start_time.to_jiff(),
            end_time: row.end_time.to_jiff(),
            value: row.value,
        })
        .collect())
}

/// The newest `issued_at` of one forecast dataset, if any. A backward scan
/// of the primary key `(dataset_id, issued_at, start_time)`.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the query fails.
pub async fn get_latest_forecast_issued_at(
    pool: &PgPool,
    dataset_id: i32,
) -> Result<Option<Timestamp>, StoreError> {
    let latest = sqlx::query_scalar!(
        "SELECT MAX(issued_at) FROM fingrid_forecasts WHERE dataset_id = $1",
        dataset_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(latest.map(jiff_sqlx::Timestamp::to_jiff))
}

/// The offline read: every archived issuance of one forecast dataset with
/// `start_time` in `[start, end)`, ordered by `(start_time, issued_at)`.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the query fails.
pub async fn get_fingrid_forecast_vintages_all(
    pool: &PgPool,
    dataset_id: i32,
    start: Timestamp,
    end: Timestamp,
) -> Result<Vec<ForecastVintageRecord>, StoreError> {
    let rows = sqlx::query!(
        r#"
        SELECT dataset_id, issued_at, start_time, end_time, value
        FROM fingrid_forecasts
        WHERE dataset_id = $1 AND start_time >= $2 AND start_time < $3
        ORDER BY start_time, issued_at
        "#,
        dataset_id,
        start.to_sqlx(),
        end.to_sqlx(),
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ForecastVintageRecord {
            dataset_id: row.dataset_id,
            issued_at: row.issued_at.to_jiff(),
            start_time: row.start_time.to_jiff(),
            end_time: row.end_time.to_jiff(),
            value: row.value,
        })
        .collect())
}

/// Deletes vintages whose issuance is before `before`. Returns the number of
/// deleted rows.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the statement fails.
pub async fn prune_fingrid_forecast_vintages_before(
    pool: &PgPool,
    before: Timestamp,
) -> Result<u64, StoreError> {
    let result = sqlx::query!(
        "DELETE FROM fingrid_forecasts WHERE issued_at < $1",
        before.to_sqlx()
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}
