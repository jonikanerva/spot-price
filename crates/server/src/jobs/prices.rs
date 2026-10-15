//! The day-ahead price fetch: today and tomorrow (UTC dates) for every area.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, ToSpan as _};
use spot_price_domain::areas::ALL_AREAS;
use sqlx::PgPool;

use crate::db::StoreError;
use crate::store::prices::{count_prices_by_range, store_prices};
use crate::upstream::nordpool::{NordPoolClient, NordPoolError};

/// The stored entries that make an area complete for a date. The value is
/// 23, as in the TypeScript service, although a quarter-hour DST day has 92
/// entries; the count range also covers only the UTC date.
pub const MIN_ENTRIES_PER_AREA: i64 = 23;

/// The outcome of one date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchResult {
    pub date: Date,
    /// The number of prices handed to the store.
    pub stored: usize,
    /// Every area was already complete, so Nord Pool was not called.
    pub skipped: bool,
}

/// The outcome of one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchJobResult {
    pub results: Vec<FetchResult>,
    pub tomorrow_available: bool,
}

/// A failed run.
#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
pub enum FetchJobError {
    #[error(transparent)]
    NordPool(#[from] NordPoolError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl FetchJobError {
    /// A fixed code for diagnostics.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::NordPool(error) => error.code(),
            Self::Store(error) => error.code(),
        }
    }
}

/// The UTC date of `instant`.
#[must_use]
pub fn utc_date(instant: Timestamp) -> Date {
    instant.to_zoned(TimeZone::UTC).date()
}

/// Whether every area has at least [`MIN_ENTRIES_PER_AREA`] prices that
/// start in the UTC `date`.
///
/// # Errors
///
/// Returns [`StoreError`] when a count fails.
pub async fn all_areas_present(pool: &PgPool, date: Date) -> Result<bool, StoreError> {
    let (start, end) = utc_date_range(date)?;
    for area in ALL_AREAS {
        if count_prices_by_range(pool, start, end, area).await? < MIN_ENTRIES_PER_AREA {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Fetches today and tomorrow. A failure on today stops the run; a failure
/// on tomorrow is logged and makes `tomorrow_available` false.
///
/// # Errors
///
/// Returns [`FetchJobError`] when the fetch or the store of today fails.
pub async fn run_fetch_job(
    pool: &PgPool,
    client: &NordPoolClient,
    now: Timestamp,
) -> Result<FetchJobResult, FetchJobError> {
    let today = utc_date(now);
    let tomorrow = today
        .checked_add(1.day())
        .map_err(|_| StoreError::InvalidRow)?;
    tracing::info!(
        job = "prices",
        date = today.to_string(),
        areas = ALL_AREAS.len(),
        "fetching prices for today and tomorrow"
    );

    let mut results = Vec::with_capacity(2);
    let today_result = fetch_for_date(pool, client, today).await?;
    log_result(today_result);
    results.push(today_result);

    let tomorrow_available = match fetch_for_date(pool, client, tomorrow).await {
        Ok(result) => {
            log_result(result);
            results.push(result);
            result.skipped || result.stored > 0
        }
        Err(error) => {
            tracing::warn!(
                job = "prices",
                date = tomorrow.to_string(),
                code = error.code(),
                "the fetch of tomorrow failed"
            );
            false
        }
    };
    Ok(FetchJobResult {
        results,
        tomorrow_available,
    })
}

async fn fetch_for_date(
    pool: &PgPool,
    client: &NordPoolClient,
    date: Date,
) -> Result<FetchResult, FetchJobError> {
    if all_areas_present(pool, date).await? {
        return Ok(FetchResult {
            date,
            stored: 0,
            skipped: true,
        });
    }
    let prices = client.fetch_day_ahead_prices(date, &ALL_AREAS).await?;
    let stored = if prices.is_empty() {
        0
    } else {
        store_prices(pool, &prices).await?
    };
    Ok(FetchResult {
        date,
        stored,
        skipped: false,
    })
}

fn log_result(result: FetchResult) {
    let date = result.date.to_string();
    if result.skipped {
        tracing::info!(
            job = "prices",
            date,
            skipped = true,
            "every area is already stored"
        );
    } else if result.stored == 0 {
        tracing::info!(
            job = "prices",
            date,
            stored = 0_u64,
            "prices are not published yet"
        );
    } else {
        tracing::info!(
            job = "prices",
            date,
            stored = result.stored,
            "stored prices"
        );
    }
}

/// `[00:00, 23:59:59.999)` of the UTC `date`, as in the TypeScript service.
fn utc_date_range(date: Date) -> Result<(Timestamp, Timestamp), StoreError> {
    let start = date
        .to_zoned(TimeZone::UTC)
        .map_err(|_| StoreError::InvalidRow)?
        .timestamp();
    let end = start
        .checked_add(SignedDuration::from_hours(24) - SignedDuration::from_millis(1))
        .map_err(|_| StoreError::InvalidRow)?;
    Ok((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_count_range_is_the_utc_date_without_its_last_millisecond() {
        let (start, end) = utc_date_range(jiff::civil::date(2026, 3, 29)).unwrap();
        assert_eq!(start.to_string(), "2026-03-29T00:00:00Z");
        assert_eq!(end.to_string(), "2026-03-29T23:59:59.999Z");
    }

    #[test]
    fn today_is_the_utc_date() {
        let now: Timestamp = "2026-03-28T23:30:00Z".parse().unwrap();
        assert_eq!(utc_date(now), jiff::civil::date(2026, 3, 28));
    }
}
