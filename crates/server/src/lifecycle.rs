//! Process lifecycle: the `serve` and `migrate` commands and shutdown.

use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::clock::{Clock, SystemClock};
use crate::config::{Config, RunMode};
use crate::db::{self, PoolSettings};
use crate::http::{AppState, router};
use crate::jobs::{self, JobContext, scheduler};
use crate::upstream::fingrid::FingridClient;
use crate::upstream::nordpool::{NordPoolClient, RetrySettings};
use crate::upstream::weather::WeatherClient;
use crate::upstream::{HttpClient, HttpSettings, UpstreamUrls};

/// The time for active requests after a stop signal. Railway waits
/// `RAILWAY_DEPLOYMENT_DRAINING_SECONDS` (30 s by default) before SIGKILL.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// The time for the job tasks and the pool to stop after the server.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// The deadline of the `migrate` command.
const MIGRATE_DEADLINE: Duration = Duration::from_secs(120);

/// The deadline of the start-up schema check.
const SCHEMA_CHECK_DEADLINE: Duration = Duration::from_secs(30);

/// Applies the pending migrations and exits. The release runs this before
/// `serve`.
///
/// # Errors
///
/// Returns a fixed stage name when a step fails.
pub async fn migrate(config: &Config) -> Result<(), &'static str> {
    let pool = db::connect_lazy(&config.database_url, PoolSettings::default())
        .map_err(|_| "database_url")?;
    let result = tokio::time::timeout(MIGRATE_DEADLINE, db::migrate(&pool)).await;
    pool.close().await;
    match result {
        Ok(Ok(())) => {
            tracing::info!(stage = "migrate", "migrations are up to date");
            Ok(())
        }
        Ok(Err(failure)) => {
            let stage = failure.kind.stage();
            tracing::error!(
                stage,
                code = failure.sqlstate,
                reason = failure.reason,
                "the migration failed"
            );
            Err(stage)
        }
        Err(_) => {
            tracing::error!(
                stage = "migrate_timeout",
                "the migration did not end before its deadline"
            );
            Err("migrate_timeout")
        }
    }
}

/// Serves HTTP and runs the scheduled jobs until SIGTERM or SIGINT. Refuses
/// to start when a migration of this build is not applied.
///
/// # Errors
///
/// Returns a fixed stage name when start-up or serving fails.
pub async fn serve(config: Config) -> Result<(), &'static str> {
    if config.mode == RunMode::Production {
        if config.fingrid_api_key.is_none() {
            tracing::warn!(
                stage = "config",
                "forecast disabled: FINGRID_API_KEY is not set; the forecast endpoint answers available:false"
            );
        }
        if config.openweathermap_api_key.is_none() {
            tracing::warn!(
                stage = "config",
                "weather collection disabled: OPENWEATHERMAP_API_KEY is not set"
            );
        }
    }

    let pool = db::connect_lazy(&config.database_url, PoolSettings::default())
        .map_err(|_| "database_url")?;
    let schema = tokio::time::timeout(SCHEMA_CHECK_DEADLINE, db::ensure_migrated(&pool)).await;
    if !matches!(schema, Ok(Ok(()))) {
        pool.close().await;
        return Err("schema");
    }

    let http = HttpClient::new(HttpSettings::default()).map_err(|_| "http_client")?;
    let urls = UpstreamUrls::production().map_err(|_| "http_client")?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let context = JobContext {
        pool: pool.clone(),
        nordpool: NordPoolClient::new(http.clone(), urls.nordpool, RetrySettings::default()),
        fingrid: FingridClient::new(http.clone(), urls.fingrid),
        weather: WeatherClient::new(http, urls.openweathermap),
        clock: clock.clone(),
    };
    let app = router(AppState::new(
        pool.clone(),
        clock.clone(),
        &config.auth_base_url,
    ));

    let listener = TcpListener::bind(config.bind).await.map_err(|_| "bind")?;
    let bound = listener.local_addr().map_err(|_| "bind")?;
    tracing::info!(bind = bound.to_string(), "service started");

    let (stop, stopped) = watch::channel(false);
    let job_set = jobs::production_jobs(
        &context,
        config.fingrid_api_key.clone(),
        config.openweathermap_api_key.clone(),
    );
    let tasks = scheduler::start(job_set, &clock, &stopped).map_err(|_| "scheduler")?;

    let shutdown = async move {
        shutdown_signal().await;
        tracing::info!(stage = "shutdown", "shutdown started");
        stop.send_replace(true);
    };
    let server = axum::serve(listener, app).with_graceful_shutdown(shutdown);
    let mut grace = stopped.clone();
    let outcome = tokio::select! {
        result = server.into_future() => result.map_err(|_| "serve"),
        () = async {
            if grace.wait_for(|stop| *stop).await.is_ok() {
                tokio::time::sleep(SHUTDOWN_GRACE).await;
            } else {
                std::future::pending::<()>().await;
            }
        } => {
            tracing::warn!(stage = "shutdown", "the shutdown grace period ended with active requests");
            Ok(())
        }
    };

    stop_tasks(tasks).await;
    if tokio::time::timeout(STOP_TIMEOUT, pool.close())
        .await
        .is_err()
    {
        tracing::warn!(
            stage = "shutdown",
            "the database pool did not close in time"
        );
    }
    tracing::info!(stage = "shutdown", "service stopped");
    outcome
}

/// Waits for the job tasks to stop, then aborts the rest. A job stopped at
/// an await point writes nothing more: an open transaction rolls back.
pub async fn stop_tasks(mut tasks: JoinSet<()>) {
    let drained = tokio::time::timeout(STOP_TIMEOUT, async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        tasks.abort_all();
        tracing::warn!(stage = "shutdown", "the jobs did not stop in time");
    }
}

/// Waits for SIGTERM or SIGINT. If a handler cannot be installed, that
/// signal is logged and ignored, and the other signal still works.
async fn shutdown_signal() {
    let interrupt = async {
        if tokio::signal::ctrl_c().await.is_err() {
            tracing::error!(
                stage = "signal",
                code = "sigint",
                "the SIGINT handler could not be installed"
            );
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        } else {
            tracing::error!(
                stage = "signal",
                code = "sigterm",
                "the SIGTERM handler could not be installed"
            );
            std::future::pending::<()>().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
}
