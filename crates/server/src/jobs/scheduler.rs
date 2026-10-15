//! The schedules of the background jobs (STACK.md §9).
//!
//! - Price fetch: at start-up, then `0 */2 * * *` in UTC.
//! - Price burst: `*/10 12-13 * * *` in Europe/Oslo, around the Nord Pool
//!   publication. A tick is skipped when tomorrow (UTC date) is captured.
//! - Fingrid and weather: at start-up, then hourly, only with their key.
//!
//! Each schedule is one owned task. A run that passes a schedule time skips
//! that time, so a job never overlaps itself. The two price schedules share
//! one lock: a price tick is skipped while another price run is active. Each
//! run has a deadline, and a panic ends only that run.

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use croner::Cron;
use futures_util::FutureExt as _;
use futures_util::future::BoxFuture;
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::clock::Clock;
use crate::jobs::prices::utc_date;

/// The baseline price schedule, in UTC.
pub const PRICE_SCHEDULE: &str = "0 */2 * * *";
/// The publication-window schedule, in [`BURST_TIME_ZONE`].
pub const BURST_SCHEDULE: &str = "*/10 12-13 * * *";
/// The time zone of the burst schedule; it follows the CET/CEST change.
pub const BURST_TIME_ZONE: &str = "Europe/Oslo";
/// The Fingrid and weather schedule, in UTC.
pub const HOURLY_SCHEDULE: &str = "0 * * * *";

/// The deadline of one price run: two dates, each with three attempts of
/// 30 s and two retry delays of 7 s, and the stores.
pub const PRICE_DEADLINE: Duration = Duration::from_secs(300);
/// The deadline of one Fingrid or weather run.
pub const HOURLY_DEADLINE: Duration = Duration::from_secs(120);

/// A price run. The argument is the label of the log line. The output is
/// true when tomorrow's prices are stored.
pub type PriceJob = Arc<dyn Fn(&'static str) -> BoxFuture<'static, bool> + Send + Sync>;
/// A Fingrid or weather run. The argument is the label of the log line.
pub type HourlyJob = Arc<dyn Fn(&'static str) -> BoxFuture<'static, ()> + Send + Sync>;

/// The jobs to schedule. `None` disables a job.
#[derive(Clone)]
pub struct Jobs {
    pub price: PriceJob,
    pub fingrid: Option<HourlyJob>,
    pub weather: Option<HourlyJob>,
}

impl std::fmt::Debug for Jobs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Jobs")
            .field("fingrid", &self.fingrid.is_some())
            .field("weather", &self.weather.is_some())
            .finish_non_exhaustive()
    }
}

/// A schedule string or time zone is invalid.
#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
#[error("a job schedule is invalid")]
pub struct ScheduleError;

/// One cron schedule in one time zone.
#[derive(Debug, Clone)]
pub struct Schedule {
    cron: Cron,
    zone: TimeZone,
}

impl Schedule {
    /// Parses `pattern` for the time zone `zone`.
    ///
    /// # Errors
    ///
    /// Returns [`ScheduleError`] when the pattern or the zone is invalid.
    pub fn new(pattern: &str, zone: TimeZone) -> Result<Self, ScheduleError> {
        let cron = pattern.parse::<Cron>().map_err(|_| ScheduleError)?;
        Ok(Self { cron, zone })
    }

    /// The first schedule time after `after`.
    #[must_use]
    pub fn next_after(&self, after: Timestamp) -> Option<Timestamp> {
        let zoned = after.to_zoned(self.zone.clone());
        self.cron
            .find_next_occurrence(&zoned, false)
            .ok()
            .map(|next| next.timestamp())
    }
}

/// Whether a burst tick at `now` can skip the fetch: tomorrow (the UTC date
/// after `now`) is already captured.
#[must_use]
pub fn burst_can_skip(now: Timestamp, last_captured: Option<Date>) -> bool {
    last_captured.is_some_and(|captured| Some(captured) == utc_date(now).tomorrow().ok())
}

/// Starts every enabled schedule. Each task stops when `shutdown` becomes
/// true or its sender is dropped.
///
/// # Errors
///
/// Returns [`ScheduleError`] when a constant schedule does not parse.
pub fn start(
    jobs: Jobs,
    clock: &Arc<dyn Clock>,
    shutdown: &watch::Receiver<bool>,
) -> Result<JoinSet<()>, ScheduleError> {
    let utc = TimeZone::UTC;
    let oslo = TimeZone::get(BURST_TIME_ZONE).map_err(|_| ScheduleError)?;
    let price_schedule = Schedule::new(PRICE_SCHEDULE, utc.clone())?;
    let burst_schedule = Schedule::new(BURST_SCHEDULE, oslo)?;
    let hourly_schedule = Schedule::new(HOURLY_SCHEDULE, utc)?;
    let price_lock = Arc::new(tokio::sync::Mutex::new(()));
    let mut tasks = JoinSet::new();

    tracing::info!(
        stage = "scheduler",
        "price fetch scheduled: every 2 h (UTC) and every 10 min 12:00-13:59 Europe/Oslo"
    );
    {
        let price = jobs.price.clone();
        let lock = price_lock.clone();
        let mut label = "Startup fetch";
        tasks.spawn(run_schedule(
            price_schedule,
            true,
            clock.clone(),
            shutdown.clone(),
            move || {
                let price = price.clone();
                let lock = lock.clone();
                let current = std::mem::replace(&mut label, "Scheduled fetch");
                async move {
                    let Ok(_guard) = lock.try_lock() else {
                        tracing::info!(job = "prices", "a price run is active; tick skipped");
                        return;
                    };
                    guarded("prices", PRICE_DEADLINE, price(current)).await;
                }
                .boxed()
            },
        ));
    }
    {
        let price = jobs.price;
        let lock = price_lock;
        let burst_clock = clock.clone();
        let captured: Arc<Mutex<Option<Date>>> = Arc::new(Mutex::new(None));
        tasks.spawn(run_schedule(
            burst_schedule,
            false,
            clock.clone(),
            shutdown.clone(),
            move || {
                let price = price.clone();
                let lock = lock.clone();
                let captured = captured.clone();
                let now = burst_clock.now();
                async move {
                    let last = captured.lock().map_or(None, |guard| *guard);
                    if burst_can_skip(now, last) {
                        return;
                    }
                    let Ok(_guard) = lock.try_lock() else {
                        tracing::info!(job = "prices", "a price run is active; tick skipped");
                        return;
                    };
                    let available =
                        guarded("prices", PRICE_DEADLINE, price("Publication-window fetch"))
                            .await
                            .unwrap_or(false);
                    if available {
                        if let (Ok(mut guard), Ok(tomorrow)) =
                            (captured.lock(), utc_date(now).tomorrow())
                        {
                            *guard = Some(tomorrow);
                        }
                        tracing::info!(
                            job = "prices",
                            "tomorrow's prices are captured; burst polling paused for today"
                        );
                    }
                }
                .boxed()
            },
        ));
    }
    for (name, job) in [("fingrid", jobs.fingrid), ("weather", jobs.weather)] {
        let Some(job) = job else {
            tracing::info!(job = name, "fetch disabled: no API key");
            continue;
        };
        tracing::info!(job = name, "fetch scheduled: hourly");
        let mut label = "Startup fetch";
        tasks.spawn(run_schedule(
            hourly_schedule.clone(),
            true,
            clock.clone(),
            shutdown.clone(),
            move || {
                let current = std::mem::replace(&mut label, "Scheduled fetch");
                guarded(name, HOURLY_DEADLINE, job(current))
                    .map(|_| ())
                    .boxed()
            },
        ));
    }
    Ok(tasks)
}

/// Runs `tick` now when `run_now` is set, then at each schedule time, until
/// `shutdown` becomes true. A stop signal drops a running tick at its next
/// await point.
async fn run_schedule<F>(
    schedule: Schedule,
    run_now: bool,
    clock: Arc<dyn Clock>,
    mut shutdown: watch::Receiver<bool>,
    mut tick: F,
) where
    F: FnMut() -> BoxFuture<'static, ()> + Send,
{
    if run_now {
        tokio::select! {
            () = tick() => {}
            _ = shutdown.wait_for(|stop| *stop) => return,
        }
    }
    let mut last_fire: Option<Timestamp> = None;
    loop {
        let now = clock.now();
        // A wake-up a little before the schedule time must not fire it twice.
        let after = last_fire.map_or(now, |fire| fire.max(now));
        let Some(next) = schedule.next_after(after) else {
            tracing::error!(code = "schedule", "a schedule has no next time; it stopped");
            return;
        };
        let delay = Duration::try_from(now.duration_until(next)).unwrap_or(Duration::ZERO);
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            _ = shutdown.wait_for(|stop| *stop) => return,
        }
        last_fire = Some(next);
        tokio::select! {
            () = tick() => {}
            _ = shutdown.wait_for(|stop| *stop) => return,
        }
    }
}

/// Runs one job with a deadline and panic containment. `None` when the job
/// did not finish.
async fn guarded<T>(
    job: &'static str,
    deadline: Duration,
    run: BoxFuture<'static, T>,
) -> Option<T> {
    let started = tokio::time::Instant::now();
    let outcome = AssertUnwindSafe(tokio::time::timeout(deadline, run))
        .catch_unwind()
        .await;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match outcome {
        Ok(Ok(value)) => Some(value),
        Ok(Err(_)) => {
            tracing::warn!(
                job,
                code = "deadline",
                elapsed_ms,
                "a job run passed its deadline"
            );
            None
        }
        Err(_) => {
            tracing::error!(job, code = "panic", elapsed_ms, "a job run panicked");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(iso: &str) -> Timestamp {
        iso.parse().unwrap()
    }

    fn next(pattern: &str, zone: &str, after: &str) -> String {
        Schedule::new(pattern, TimeZone::get(zone).unwrap())
            .unwrap()
            .next_after(ts(after))
            .unwrap()
            .to_string()
    }

    #[test]
    fn the_price_schedule_is_every_two_hours_in_utc() {
        assert_eq!(
            next(PRICE_SCHEDULE, "UTC", "2026-10-14T10:30:00Z"),
            "2026-10-14T12:00:00Z"
        );
        assert_eq!(
            next(PRICE_SCHEDULE, "UTC", "2026-10-14T12:00:00Z"),
            "2026-10-14T14:00:00Z"
        );
    }

    #[test]
    fn the_burst_follows_oslo_time_across_the_dst_change() {
        // CEST: 12:00 in Oslo is 10:00 UTC.
        assert_eq!(
            next(BURST_SCHEDULE, BURST_TIME_ZONE, "2026-06-15T09:55:00Z"),
            "2026-06-15T10:00:00Z"
        );
        // CET: 12:00 in Oslo is 11:00 UTC.
        assert_eq!(
            next(BURST_SCHEDULE, BURST_TIME_ZONE, "2026-01-15T10:55:00Z"),
            "2026-01-15T11:00:00Z"
        );
        // The last tick of the window is 13:50 local time.
        assert_eq!(
            next(BURST_SCHEDULE, BURST_TIME_ZONE, "2026-01-15T12:45:00Z"),
            "2026-01-15T12:50:00Z"
        );
        assert_eq!(
            next(BURST_SCHEDULE, BURST_TIME_ZONE, "2026-01-15T12:50:00Z"),
            "2026-01-16T11:00:00Z"
        );
        // The night of 2026-03-29 moves Oslo from CET to CEST.
        assert_eq!(
            next(BURST_SCHEDULE, BURST_TIME_ZONE, "2026-03-28T13:55:00Z"),
            "2026-03-29T10:00:00Z"
        );
    }

    #[test]
    fn the_hourly_schedule_is_on_the_utc_hour() {
        assert_eq!(
            next(HOURLY_SCHEDULE, "UTC", "2026-10-14T10:00:00Z"),
            "2026-10-14T11:00:00Z"
        );
    }

    #[test]
    fn a_burst_tick_skips_only_when_tomorrow_is_captured() {
        let now = ts("2026-04-15T11:10:00Z");
        assert!(!burst_can_skip(now, None));
        assert!(burst_can_skip(now, Some(jiff::civil::date(2026, 4, 16))));
        assert!(!burst_can_skip(now, Some(jiff::civil::date(2026, 4, 15))));
        // The next day has a new tomorrow.
        assert!(!burst_can_skip(
            ts("2026-04-16T11:00:00Z"),
            Some(jiff::civil::date(2026, 4, 16))
        ));
    }
}
