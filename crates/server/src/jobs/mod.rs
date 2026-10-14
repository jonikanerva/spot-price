//! The background jobs: the Nord Pool price fetch, the Fingrid fetch, and the
//! weather collection, plus their schedules. These are the only background
//! tasks of the service (STACK.md §9).
//!
//! Each job logs its own outcome and never returns an error to the
//! scheduler, so one job's failure cannot affect another job.

pub mod fingrid;
pub mod prices;
pub mod scheduler;
pub mod weather;

use std::sync::Arc;

use futures_util::FutureExt as _;
use sqlx::PgPool;

use crate::clock::Clock;
use crate::config::Secret;
use crate::upstream::fingrid::FingridClient;
use crate::upstream::nordpool::NordPoolClient;
use crate::upstream::weather::WeatherClient;
use fingrid::ForecastFetchResult;
use scheduler::Jobs;
use weather::{DailySummary, PointFailure, WeatherJobResult};

/// The shared inputs of the jobs.
#[derive(Clone)]
pub struct JobContext {
    pub pool: PgPool,
    pub nordpool: NordPoolClient,
    pub fingrid: FingridClient,
    pub weather: WeatherClient,
    pub clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for JobContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("JobContext").finish_non_exhaustive()
    }
}

/// The production jobs. A job without its key is disabled, so no billable
/// or keyed call is made without configuration.
#[must_use]
pub fn production_jobs(
    context: &JobContext,
    fingrid_key: Option<Secret>,
    weather_key: Option<Secret>,
) -> Jobs {
    let price = {
        let context = context.clone();
        Arc::new(move |label: &'static str| {
            let context = context.clone();
            async move { price_fetch(&context, label).await }.boxed()
        }) as scheduler::PriceJob
    };
    let fingrid = fingrid_key.map(|key| {
        let context = context.clone();
        Arc::new(move |label: &'static str| {
            let context = context.clone();
            let key = key.clone();
            async move { fingrid_fetch(&context, &key, label).await }.boxed()
        }) as scheduler::HourlyJob
    });
    let weather = weather_key.map(|key| {
        let context = context.clone();
        Arc::new(move |label: &'static str| {
            let context = context.clone();
            let key = key.clone();
            async move { weather_fetch(&context, &key, label).await }.boxed()
        }) as scheduler::HourlyJob
    });
    Jobs {
        price,
        fingrid,
        weather,
    }
}

/// Runs the price fetch and logs the outcome. Returns whether tomorrow's
/// prices are stored; a failure returns false.
pub async fn price_fetch(context: &JobContext, label: &'static str) -> bool {
    match prices::run_fetch_job(&context.pool, &context.nordpool, context.clock.now()).await {
        Ok(result) => {
            let stored: usize = result.results.iter().map(|r| r.stored).sum();
            tracing::info!(
                job = "prices",
                stage = label,
                stored,
                "price fetch finished"
            );
            if !result.tomorrow_available {
                tracing::info!(
                    job = "prices",
                    tomorrow_available = false,
                    "tomorrow's prices are not available yet; the next cycle retries"
                );
            }
            result.tomorrow_available
        }
        Err(error) => {
            tracing::error!(
                job = "prices",
                stage = label,
                code = error.code(),
                "price fetch failed"
            );
            false
        }
    }
}

/// Runs the Fingrid fetch and logs the outcome.
pub async fn fingrid_fetch(context: &JobContext, key: &Secret, label: &'static str) {
    let now = context.clock.now();
    match fingrid::run_forecast_fetch_job(&context.pool, &context.fingrid, key, now).await {
        Ok(ForecastFetchResult::Stored {
            stored,
            pruned,
            vintage_stored,
            vintage_pruned,
            vintage_degraded_reason,
        }) => match vintage_degraded_reason {
            None => tracing::info!(
                job = "fingrid",
                stage = label,
                stored,
                pruned,
                vintage_stored,
                vintage_pruned,
                "Fingrid fetch finished"
            ),
            Some(reason) => tracing::warn!(
                job = "fingrid",
                stage = label,
                stored,
                pruned,
                reason,
                "Fingrid fetch finished; vintage archival degraded"
            ),
        },
        Ok(ForecastFetchResult::Degraded { reason }) => {
            tracing::warn!(job = "fingrid", stage = label, reason, "Fingrid degraded");
        }
        Err(error) => {
            tracing::error!(
                job = "fingrid",
                stage = label,
                code = error.code(),
                "Fingrid fetch failed"
            );
        }
    }
}

/// Runs the weather collection and logs the outcome. The daily summary is
/// always logged, so a daily collection that stops is visible.
pub async fn weather_fetch(context: &JobContext, key: &Secret, label: &'static str) {
    let now = context.clock.now();
    match weather::run_weather_fetch_job(&context.pool, &context.weather, key, now).await {
        Ok(WeatherJobResult::Ok {
            stored,
            pruned,
            daily,
        }) => tracing::info!(
            job = "weather",
            stage = label,
            stored,
            pruned,
            daily_stored = daily.stored,
            daily_failed = point_ids(&daily.failures),
            "weather fetch finished"
        ),
        Ok(WeatherJobResult::Partial {
            stored,
            pruned,
            failures,
            daily,
        }) => tracing::warn!(
            job = "weather",
            stage = label,
            stored,
            pruned,
            failed_points = point_ids(&failures),
            daily_stored = daily.stored,
            daily_failed = point_ids(&daily.failures),
            "weather fetch finished with degraded points"
        ),
        Ok(WeatherJobResult::Failed { failures, daily }) => {
            log_failed_weather(label, &failures, &daily);
        }
        Err(error) => {
            tracing::error!(
                job = "weather",
                stage = label,
                code = error.code(),
                "weather fetch failed"
            );
        }
    }
}

fn log_failed_weather(label: &'static str, failures: &[PointFailure], daily: &DailySummary) {
    for failure in failures {
        tracing::warn!(
            job = "weather",
            point = failure.point_id,
            reason = failure.reason.as_str(),
            "weather point degraded"
        );
    }
    tracing::warn!(
        job = "weather",
        stage = label,
        failed_points = point_ids(failures),
        daily_stored = daily.stored,
        daily_failed = point_ids(&daily.failures),
        "weather degraded: every point failed"
    );
}

fn point_ids(failures: &[PointFailure]) -> String {
    failures
        .iter()
        .map(|failure| failure.point_id)
        .collect::<Vec<_>>()
        .join(", ")
}
