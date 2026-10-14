//! The `OpenWeatherMap` collection of the FI forecast (STACK.md §9).
//!
//! Each point is fetched and stored on its own: one point's failure never
//! drops the other point's data, because an issuance cannot be fetched again
//! after its hour. For each point, the hourly rows commit first; the daily
//! rows follow in their own transaction and their own error handling.
//! `status`, `stored`, and `pruned` describe the hourly collection only.

use jiff::{SignedDuration, Timestamp};
use sqlx::PgPool;

use crate::config::Secret;
use crate::db::StoreError;
use crate::store::weather::{
    prune_weather_daily_records_before, prune_weather_records_before, store_weather_daily_records,
    store_weather_records,
};
use crate::upstream::weather::{WEATHER_POINTS, WeatherClient};

/// Retention of both weather tables, by `issued_at`. Keep it above one year
/// so that a backtest can span a full seasonal cycle.
pub const WEATHER_RETENTION_DAYS: i64 = 400;

/// One point that degraded, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointFailure {
    pub point_id: &'static str,
    pub reason: String,
}

/// The daily-block summary of one run. A daily failure never changes the
/// hourly status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DailySummary {
    pub stored: u64,
    pub failures: Vec<PointFailure>,
}

/// The outcome of one run, by hourly completeness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeatherJobResult {
    Ok {
        stored: u64,
        pruned: u64,
        daily: DailySummary,
    },
    Partial {
        stored: u64,
        pruned: u64,
        failures: Vec<PointFailure>,
        daily: DailySummary,
    },
    /// Every point failed on the hourly side. Nothing was pruned.
    Failed {
        failures: Vec<PointFailure>,
        daily: DailySummary,
    },
}

/// Runs one collection at `now`.
///
/// # Errors
///
/// Returns [`StoreError`] when an hourly write or the hourly prune fails.
pub async fn run_weather_fetch_job(
    pool: &PgPool,
    client: &WeatherClient,
    api_key: &Secret,
    now: Timestamp,
) -> Result<WeatherJobResult, StoreError> {
    let mut stored = 0;
    let mut successes = 0;
    let mut failures = Vec::new();
    let mut daily = DailySummary::default();

    for point in WEATHER_POINTS {
        let fetch = client.fetch_weather(api_key, point, now).await;
        match fetch.hourly {
            Ok(records) => {
                stored += store_weather_records(pool, &records).await?;
                successes += 1;
            }
            Err(reason) => failures.push(PointFailure {
                point_id: point.id,
                reason,
            }),
        }
        // This runs after an hourly failure too: a valid daily block must
        // not be lost because the hourly block drifted.
        match fetch.daily {
            Ok(records) => match store_weather_daily_records(pool, &records).await {
                Ok(inserted) => daily.stored += inserted,
                Err(error) => {
                    tracing::warn!(
                        job = "weather",
                        point = point.id,
                        code = error.code(),
                        "the daily store degraded"
                    );
                    daily.failures.push(PointFailure {
                        point_id: point.id,
                        reason: error.to_string(),
                    });
                }
            },
            Err(reason) => daily.failures.push(PointFailure {
                point_id: point.id,
                reason,
            }),
        }
    }

    // No fresh hourly data: do not prune during an upstream outage.
    if successes == 0 {
        return Ok(WeatherJobResult::Failed { failures, daily });
    }

    let cutoff = now
        .checked_sub(SignedDuration::from_hours(WEATHER_RETENTION_DAYS * 24))
        .map_err(|_| StoreError::InvalidRow)?;
    let pruned = prune_weather_records_before(pool, cutoff).await?;
    if let Err(error) = prune_weather_daily_records_before(pool, cutoff).await {
        tracing::warn!(
            job = "weather",
            code = error.code(),
            "the daily retention prune degraded"
        );
    }

    if failures.is_empty() {
        Ok(WeatherJobResult::Ok {
            stored,
            pruned,
            daily,
        })
    } else {
        Ok(WeatherJobResult::Partial {
            stored,
            pruned,
            failures,
            daily,
        })
    }
}
