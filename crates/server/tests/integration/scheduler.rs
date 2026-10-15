//! The schedule loops with paused Tokio time and a clock that follows it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::FutureExt as _;
use jiff::Timestamp;
use spot_price_server::clock::Clock;
use spot_price_server::jobs::scheduler::{HourlyJob, Jobs, PriceJob, start};
use spot_price_server::lifecycle::stop_tasks;
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::common::ts;

/// A wall clock that starts at `base` and advances with Tokio time.
struct TokioClock {
    base: Timestamp,
    start: tokio::time::Instant,
}

impl Clock for TokioClock {
    fn now(&self) -> Timestamp {
        let elapsed = jiff::SignedDuration::try_from(self.start.elapsed()).unwrap();
        self.base.checked_add(elapsed).unwrap()
    }
}

fn clock(base: &str) -> Arc<dyn Clock> {
    Arc::new(TokioClock {
        base: ts(base),
        start: tokio::time::Instant::now(),
    })
}

/// The labels of the price runs, in order.
type Calls = Arc<Mutex<Vec<&'static str>>>;

/// A price job that records its label, waits `takes`, and answers
/// `available`.
fn price_job(calls: &Calls, available: bool, takes: Duration) -> PriceJob {
    let calls = calls.clone();
    Arc::new(move |label| {
        calls.lock().unwrap().push(label);
        async move {
            // A zero wait must not yield, so that a run ends in one poll.
            if !takes.is_zero() {
                tokio::time::sleep(takes).await;
            }
            available
        }
        .boxed()
    })
}

fn counting_job(count: &Arc<AtomicUsize>) -> HourlyJob {
    let count = count.clone();
    Arc::new(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        async {}.boxed()
    })
}

/// Runs the scheduler from `base` until `until`, then stops it.
async fn run_until(jobs: Jobs, base: &str, until: &str) -> JoinSet<()> {
    let clock = clock(base);
    let (stop, stopped) = watch::channel(false);
    let tasks = start(jobs, &clock, &stopped).unwrap();
    let wait = ts(base).duration_until(ts(until));
    tokio::time::sleep(Duration::try_from(wait).unwrap()).await;
    stop.send_replace(true);
    tasks
}

#[tokio::test(start_paused = true)]
async fn runs_every_job_at_start_up_and_on_its_schedule() {
    let calls: Calls = Arc::default();
    let fingrid = Arc::new(AtomicUsize::new(0));
    let weather = Arc::new(AtomicUsize::new(0));
    let jobs = Jobs {
        price: price_job(&calls, false, Duration::ZERO),
        fingrid: Some(counting_job(&fingrid)),
        weather: Some(counting_job(&weather)),
    };
    // Winter: the burst window 12:00-13:59 in Oslo is 11:00-12:59 UTC.
    let tasks = run_until(jobs, "2026-01-15T10:30:00Z", "2026-01-15T12:35:00Z").await;
    stop_tasks(tasks).await;

    // Start-up, 11:00, and 12:00.
    assert_eq!(fingrid.load(Ordering::SeqCst), 3);
    assert_eq!(weather.load(Ordering::SeqCst), 3);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.first(), Some(&"Startup fetch"));
    assert_eq!(calls.iter().filter(|c| **c == "Scheduled fetch").count(), 1);
    // 11:00, 11:10, ..., 12:30.
    assert_eq!(
        calls
            .iter()
            .filter(|c| **c == "Publication-window fetch")
            .count(),
        10
    );
}

#[tokio::test(start_paused = true)]
async fn disabled_jobs_never_run() {
    let calls: Calls = Arc::default();
    let jobs = Jobs {
        price: price_job(&calls, true, Duration::ZERO),
        fingrid: None,
        weather: None,
    };
    let tasks = run_until(jobs, "2026-01-15T10:30:00Z", "2026-01-15T10:35:00Z").await;
    assert_eq!(tasks.len(), 2, "only the two price schedules run");
    stop_tasks(tasks).await;
    assert_eq!(*calls.lock().unwrap(), ["Startup fetch"]);
}

#[tokio::test(start_paused = true)]
async fn the_burst_pauses_after_tomorrow_is_captured() {
    let calls: Calls = Arc::default();
    let jobs = Jobs {
        price: price_job(&calls, true, Duration::ZERO),
        fingrid: None,
        weather: None,
    };
    let tasks = run_until(jobs, "2026-01-15T10:30:00Z", "2026-01-16T11:05:00Z").await;
    stop_tasks(tasks).await;
    let calls = calls.lock().unwrap();
    // One burst run on each day; the first one captures tomorrow for that day.
    assert_eq!(
        calls
            .iter()
            .filter(|c| **c == "Publication-window fetch")
            .count(),
        2
    );
}

#[tokio::test(start_paused = true)]
async fn the_burst_keeps_polling_while_tomorrow_is_missing() {
    let calls: Calls = Arc::default();
    let jobs = Jobs {
        price: price_job(&calls, false, Duration::ZERO),
        fingrid: None,
        weather: None,
    };
    let tasks = run_until(jobs, "2026-01-15T10:58:00Z", "2026-01-15T11:25:00Z").await;
    stop_tasks(tasks).await;
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|c| **c == "Publication-window fetch")
            .count(),
        3
    );
}

#[tokio::test(start_paused = true)]
async fn a_burst_tick_is_skipped_while_a_price_run_is_active() {
    let calls: Calls = Arc::default();
    let jobs = Jobs {
        // The start-up run lasts until 11:01:20, past the 11:00 burst tick.
        price: price_job(&calls, false, Duration::from_secs(200)),
        fingrid: None,
        weather: None,
    };
    let tasks = run_until(jobs, "2026-01-15T10:58:00Z", "2026-01-15T11:15:00Z").await;
    stop_tasks(tasks).await;
    assert_eq!(
        *calls.lock().unwrap(),
        ["Startup fetch", "Publication-window fetch"]
    );
}

#[tokio::test(start_paused = true)]
async fn a_run_past_its_deadline_is_cancelled_and_the_schedule_continues() {
    let started = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicUsize::new(0));
    let job: HourlyJob = {
        let started = started.clone();
        let finished = finished.clone();
        Arc::new(move |_| {
            started.fetch_add(1, Ordering::SeqCst);
            let finished = finished.clone();
            async move {
                tokio::time::sleep(Duration::from_hours(24)).await;
                finished.fetch_add(1, Ordering::SeqCst);
            }
            .boxed()
        })
    };
    let calls: Calls = Arc::default();
    let jobs = Jobs {
        price: price_job(&calls, false, Duration::ZERO),
        fingrid: Some(job),
        weather: None,
    };
    let tasks = run_until(jobs, "2026-01-15T10:30:00Z", "2026-01-15T12:05:00Z").await;
    stop_tasks(tasks).await;
    assert_eq!(started.load(Ordering::SeqCst), 3);
    assert_eq!(finished.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn a_panicking_run_does_not_stop_the_schedule() {
    let runs = Arc::new(AtomicUsize::new(0));
    let job: HourlyJob = {
        let runs = runs.clone();
        Arc::new(move |_| {
            let run = runs.fetch_add(1, Ordering::SeqCst);
            async move {
                assert!(run != 0, "the first run panics");
            }
            .boxed()
        })
    };
    let calls: Calls = Arc::default();
    let jobs = Jobs {
        price: price_job(&calls, false, Duration::ZERO),
        fingrid: None,
        weather: Some(job),
    };
    let tasks = run_until(jobs, "2026-01-15T10:30:00Z", "2026-01-15T12:05:00Z").await;
    stop_tasks(tasks).await;
    assert_eq!(runs.load(Ordering::SeqCst), 3);
}

#[tokio::test(start_paused = true)]
async fn shutdown_stops_a_running_job_at_its_next_await() {
    let finished = Arc::new(AtomicUsize::new(0));
    let job: HourlyJob = {
        let finished = finished.clone();
        Arc::new(move |_| {
            let finished = finished.clone();
            async move {
                tokio::time::sleep(Duration::from_secs(60)).await;
                finished.fetch_add(1, Ordering::SeqCst);
            }
            .boxed()
        })
    };
    let calls: Calls = Arc::default();
    let jobs = Jobs {
        price: price_job(&calls, false, Duration::from_secs(60)),
        fingrid: Some(job),
        weather: None,
    };
    let tasks = run_until(jobs, "2026-01-15T10:30:00Z", "2026-01-15T10:30:10Z").await;
    let stopped_at = tokio::time::Instant::now();
    stop_tasks(tasks).await;
    assert!(stopped_at.elapsed() < Duration::from_secs(1));
    assert_eq!(finished.load(Ordering::SeqCst), 0);
}
