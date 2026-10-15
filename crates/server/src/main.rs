//! Process entry point.
//!
//! `spot-price migrate` applies the database migrations and exits.
//! `spot-price serve` (the default) serves HTTP and runs the scheduled jobs.
//! `serve` refuses to start before `migrate` has run.
//! `spot-price export-openapi <path>` writes the `OpenAPI` document and exits;
//! it needs no environment and no database.

use std::process::ExitCode;
use std::time::Duration;

use spot_price_server::config::Config;
use spot_price_server::{lifecycle, observability};

/// The exit code of a usage or configuration error.
const EXIT_CONFIG: u8 = 2;

/// The time for blocking tasks after the main future ends.
const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(5);

enum Command {
    Serve,
    Migrate,
}

/// Writes the `OpenAPI` document to `path`.
fn export_openapi(path: &str) -> ExitCode {
    let Ok(text) = spot_price_server::http::openapi::document_json() else {
        report("spot-price: the OpenAPI document could not be built");
        return ExitCode::FAILURE;
    };
    if std::fs::write(path, text).is_err() {
        report("spot-price: the OpenAPI document could not be written");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let command = match args.next().as_deref() {
        None | Some("serve") => Command::Serve,
        Some("migrate") => Command::Migrate,
        Some("export-openapi") => {
            if let (Some(path), None) = (args.next(), args.next()) {
                return export_openapi(&path);
            }
            report("usage: spot-price export-openapi <path>");
            return ExitCode::from(EXIT_CONFIG);
        }
        Some(_) => {
            report("usage: spot-price [serve|migrate|export-openapi <path>]");
            return ExitCode::from(EXIT_CONFIG);
        }
    };
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            // The text names settings, never their values.
            report(&format!("spot-price: {error}"));
            return ExitCode::from(EXIT_CONFIG);
        }
    };
    if observability::init(config.log_format).is_err() {
        report("spot-price: logging could not be set up");
        return ExitCode::FAILURE;
    }
    observability::install_panic_hook();

    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    else {
        tracing::error!(stage = "runtime", "the async runtime could not start");
        return ExitCode::FAILURE;
    };
    let result = match command {
        Command::Serve => runtime.block_on(lifecycle::serve(config)),
        Command::Migrate => runtime.block_on(lifecycle::migrate(&config)),
    };
    // Dropping a runtime waits for its blocking tasks without a limit.
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(stage) => {
            tracing::error!(stage, "the service stopped with an error");
            ExitCode::FAILURE
        }
    }
}

/// Writes a message to standard error before logging is set up.
#[expect(clippy::print_stderr, reason = "logging is not set up yet")]
fn report(message: &str) {
    eprintln!("{message}");
}
