//! The PostgreSQL pool, the migrations, and the store error type.

use std::str::FromStr;
use std::time::Duration;

use sqlx::PgPool;
use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

use crate::config::Secret;

/// The SQL migrations in `migrations/` at the repository root.
pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

/// Limits of the database work.
#[derive(Debug, Clone, Copy)]
pub struct PoolSettings {
    pub max_connections: u32,
    pub acquire_timeout: Duration,
    /// PostgreSQL `statement_timeout` of each connection.
    pub statement_timeout: Duration,
}

impl Default for PoolSettings {
    fn default() -> Self {
        Self {
            max_connections: 10,
            acquire_timeout: Duration::from_secs(10),
            statement_timeout: Duration::from_secs(30),
        }
    }
}

/// A failed database operation. The variants carry no SQL, value, or
/// connection detail, so they are safe to log.
#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
pub enum StoreError {
    #[error("the database URL is invalid")]
    InvalidUrl,
    #[error("the database migration failed")]
    Migration,
    #[error("the database schema is not migrated")]
    NotMigrated,
    #[error("the database query failed")]
    Query,
    #[error("a stored row is invalid")]
    InvalidRow,
}

impl StoreError {
    /// A fixed code for diagnostics.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidUrl => "invalid_database_url",
            Self::Migration => "migration",
            Self::NotMigrated => "not_migrated",
            Self::Query => "query",
            Self::InvalidRow => "invalid_row",
        }
    }
}

impl From<sqlx::Error> for StoreError {
    fn from(_: sqlx::Error) -> Self {
        Self::Query
    }
}

/// Creates a lazy pool. No connection opens before the first query.
///
/// # Errors
///
/// Returns [`StoreError::InvalidUrl`] when the URL cannot be parsed.
pub fn connect_lazy(database_url: &Secret, settings: PoolSettings) -> Result<PgPool, StoreError> {
    let statement_timeout = format!("{}ms", settings.statement_timeout.as_millis());
    let options = PgConnectOptions::from_str(database_url.expose())
        .map_err(|_| StoreError::InvalidUrl)?
        .options([("statement_timeout", statement_timeout.as_str())]);
    Ok(PgPoolOptions::new()
        .max_connections(settings.max_connections)
        .acquire_timeout(settings.acquire_timeout)
        .connect_lazy_with(options))
}

/// Applies the pending migrations.
///
/// # Errors
///
/// Returns [`StoreError::Migration`] when a migration fails.
pub async fn migrate(pool: &PgPool) -> Result<(), StoreError> {
    MIGRATOR.run(pool).await.map_err(|_| StoreError::Migration)
}

/// Checks that every migration of this build is applied. `serve` does not
/// migrate; the `migrate` command does that as a separate release step.
///
/// # Errors
///
/// Returns [`StoreError::NotMigrated`] when a migration is missing or the
/// ledger cannot be read.
pub async fn ensure_migrated(pool: &PgPool) -> Result<(), StoreError> {
    let applied: Vec<i64> = sqlx::query_scalar!(
        r#"SELECT version AS "version!" FROM _sqlx_migrations WHERE success ORDER BY version"#
    )
    .fetch_all(pool)
    .await
    .map_err(|_| StoreError::NotMigrated)?;
    if MIGRATOR
        .iter()
        .any(|migration| !applied.contains(&migration.version))
    {
        return Err(StoreError::NotMigrated);
    }
    Ok(())
}

/// The database liveness check of `GET /health`: `Ok(true)` when
/// `SELECT 1` returns 1.
///
/// # Errors
///
/// Returns [`StoreError::Query`] when the database does not answer.
pub async fn ping(pool: &PgPool) -> Result<bool, StoreError> {
    let ok = sqlx::query_scalar!(r#"SELECT 1 AS "ok!""#)
        .fetch_one(pool)
        .await?;
    Ok(ok == 1)
}
