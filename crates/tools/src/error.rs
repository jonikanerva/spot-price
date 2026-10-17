//! Errors that stop a tool run. The binary prints the message to stderr and
//! exits with code 1. A message never holds the database URL.

/// A failed tool run.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("cannot write {path}: {source}")]
    Write {
        path: String,
        source: std::io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: String,
        source: serde_json::Error,
    },
    #[error("invalid fixture {path}: {reason}")]
    Fixture { path: String, reason: String },
    /// A flag value that is not valid.
    #[error("{0}")]
    Argument(String),
    /// No database URL in the flags or the process environment.
    #[error("DATABASE_PUBLIC_URL must be set, or --db-url given, for `{0}`")]
    MissingDatabaseUrl(&'static str),
    #[error(transparent)]
    OutOfRange(#[from] crate::backtest::OutOfRange),
    #[error(transparent)]
    Backtest(#[from] crate::backtest::BacktestError),
    #[error("database read failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error("cannot start the async runtime: {0}")]
    Runtime(std::io::Error),
    #[error("rustfmt failed on {path}: {reason}")]
    Rustfmt { path: String, reason: String },
    /// A value that the tool cannot write as a Rust literal.
    #[error("cannot render the artifact: {0}")]
    Render(String),
}
