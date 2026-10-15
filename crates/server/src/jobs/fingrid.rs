//! The Fingrid grid-data fetch of the FI forecast (STACK.md §9).
//!
//! The job fetches the four datasets for `[now - HISTORY_DAYS,
//! now + FORECAST_DAYS)`. Actuals go to `fingrid_actuals` first, in their own
//! transaction. Forecasts are then archived as vintages; a vintage failure
//! never undoes the actuals.

use jiff::{SignedDuration, Timestamp};
use spot_price_domain::forecast::FLOOR_HISTORY_DAYS;
use sqlx::PgPool;

use crate::clock::truncate_to_utc_hour;
use crate::config::Secret;
use crate::db::StoreError;
use crate::store::fingrid::{
    is_forecast_dataset, prune_fingrid_forecast_vintages_before, prune_fingrid_records_before,
    store_fingrid_forecast_vintages, store_fingrid_records,
};
use crate::upstream::fingrid::FingridClient;

pub use spot_price_domain::forecast::FORECAST_DAYS;

/// The four-week wind extension of the forecast, in days.
const WIND_EXTENSION_DAYS: i64 = 4 * 7;

/// History fetched before now: the longer of the forecast floor history and
/// the wind extension, plus one day. A wider window cannot backfill: Fingrid
/// serves data only forward from now.
pub const HISTORY_DAYS: i64 = if FLOOR_HISTORY_DAYS > WIND_EXTENSION_DAYS {
    FLOOR_HISTORY_DAYS
} else {
    WIND_EXTENSION_DAYS
} + 1;

/// Retention of the actual rows, by `start_time`.
pub const RETENTION_DAYS: i64 = 730;

/// Retention of the forecast vintages, by `issued_at`.
pub const VINTAGE_RETENTION_DAYS: i64 = 180;

/// The outcome of one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForecastFetchResult {
    Stored {
        /// Actual records handed to the store.
        stored: usize,
        pruned: u64,
        vintage_stored: u64,
        vintage_pruned: u64,
        /// Set when the vintage write or prune failed after the actuals
        /// committed.
        vintage_degraded_reason: Option<String>,
    },
    /// Fingrid did not deliver data. Nothing was written.
    Degraded { reason: String },
}

/// Runs one fetch at `now`.
///
/// # Errors
///
/// Returns [`StoreError`] when the actuals write or prune fails.
pub async fn run_forecast_fetch_job(
    pool: &PgPool,
    client: &FingridClient,
    api_key: &Secret,
    now: Timestamp,
) -> Result<ForecastFetchResult, StoreError> {
    let start = days_before(now, HISTORY_DAYS)?;
    let end = days_before(now, -FORECAST_DAYS)?;
    let records = match client.fetch_series(api_key, start, end).await {
        Ok(records) => records,
        Err(degraded) => {
            return Ok(ForecastFetchResult::Degraded { reason: degraded.0 });
        }
    };

    let actuals: Vec<_> = records
        .iter()
        .copied()
        .filter(|record| !is_forecast_dataset(record.dataset_id))
        .collect();
    let stored = store_fingrid_records(pool, &actuals).await?;
    let pruned = prune_fingrid_records_before(pool, days_before(now, RETENTION_DAYS)?).await?;

    let issued_at = truncate_to_utc_hour(now);
    let mut vintage_stored = 0;
    let mut vintage_pruned = 0;
    let vintages = async {
        vintage_stored = store_fingrid_forecast_vintages(pool, issued_at, &records).await?;
        let cutoff = days_before(now, VINTAGE_RETENTION_DAYS)?;
        vintage_pruned = prune_fingrid_forecast_vintages_before(pool, cutoff).await?;
        Ok::<_, StoreError>(())
    }
    .await;
    let vintage_degraded_reason = vintages.err().map(|error| {
        tracing::warn!(
            job = "fingrid",
            code = error.code(),
            "the forecast-vintage archival degraded"
        );
        error.to_string()
    });
    Ok(ForecastFetchResult::Stored {
        stored,
        pruned,
        vintage_stored,
        vintage_pruned,
        vintage_degraded_reason,
    })
}

/// `now` minus `days` fixed 24-hour days.
fn days_before(now: Timestamp, days: i64) -> Result<Timestamp, StoreError> {
    now.checked_sub(SignedDuration::from_hours(days * 24))
        .map_err(|_| StoreError::InvalidRow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windows_equal_the_typescript_values() {
        assert_eq!(HISTORY_DAYS, 31);
        assert_eq!(FORECAST_DAYS, 3);
        assert_eq!(RETENTION_DAYS, 730);
        assert_eq!(VINTAGE_RETENTION_DAYS, 180);
    }
}
