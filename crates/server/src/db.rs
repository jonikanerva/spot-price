//! The PostgreSQL pool, the migrations, and the store error type.

use std::str::FromStr;
use std::time::Duration;

use sqlx::PgPool;
use sqlx::migrate::{MigrateError, Migrator};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

use crate::config::Secret;

/// The SQL migrations in `migrations/` at the repository root.
pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

/// Limits of the database work.
#[derive(Debug, Clone, Copy)]
pub struct PoolSettings {
    pub max_connections: u32,
    /// The idle connections that the pool keeps open. A new connection
    /// costs about 100 ms, so without them the first request after 10 idle
    /// minutes opens one or two connections on the request path (#150).
    /// Three cover the longest serial query chain of one request while the
    /// pool pings the released connection in the background.
    pub min_connections: u32,
    pub acquire_timeout: Duration,
    /// PostgreSQL `statement_timeout` of each connection.
    pub statement_timeout: Duration,
}

impl Default for PoolSettings {
    fn default() -> Self {
        Self {
            max_connections: 10,
            min_connections: 3,
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

/// Creates a lazy pool. No connection opens before the first query. The
/// pool keeps `min_connections` open from its first close of an idle
/// connection on.
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
        .min_connections(settings.min_connections)
        .acquire_timeout(settings.acquire_timeout)
        .connect_lazy_with(options))
}

/// The kind of a failed `migrate` run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationFailureKind {
    /// No connection: refused, closed, timed out, or rejected at sign-in.
    Connect,
    /// The TLS negotiation or handshake failed.
    Tls,
    /// An applied migration differs from the file of this build.
    Checksum,
    /// The ledger and the migrations of this build do not match.
    Version,
    /// A migration is marked as not successful in the ledger.
    Dirty,
    /// A migration or a ledger statement failed.
    Execute,
}

impl MigrationFailureKind {
    /// The fixed stage name of the log.
    #[must_use]
    pub const fn stage(self) -> &'static str {
        match self {
            Self::Connect => "migrate_connect",
            Self::Tls => "migrate_tls",
            Self::Checksum => "migrate_checksum",
            Self::Version => "migrate_version",
            Self::Dirty => "migrate_dirty",
            Self::Execute => "migrate_execute",
        }
    }
}

/// A failed `migrate` run, safe to log. It holds no URL and no server text,
/// except `reason`: only a `RAISE` (SQLSTATE `P0001`) gives a reason, and
/// only the files in `migrations/` raise during a migration run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationFailure {
    pub kind: MigrationFailureKind,
    /// The SQLSTATE of a database error.
    pub sqlstate: Option<String>,
    pub reason: Option<String>,
    /// The fixed name of a ledger error, such as `version_missing`.
    pub variant: Option<&'static str>,
    /// The migration version that a ledger error names.
    pub version: Option<i64>,
}

impl MigrationFailure {
    /// Classifies a migrator error.
    #[must_use]
    pub fn classify(error: &MigrateError) -> Self {
        match error {
            MigrateError::Execute(inner) | MigrateError::ExecuteMigration(inner, _) => {
                Self::from_sqlx(inner)
            }
            MigrateError::VersionMismatch(version) => {
                Self::ledger(MigrationFailureKind::Checksum, "version_mismatch", *version)
            }
            MigrateError::VersionMissing(version) => {
                Self::ledger(MigrationFailureKind::Version, "version_missing", *version)
            }
            MigrateError::VersionNotPresent(version) => Self::ledger(
                MigrationFailureKind::Version,
                "version_not_present",
                *version,
            ),
            MigrateError::VersionTooOld(version, _) => {
                Self::ledger(MigrationFailureKind::Version, "version_too_old", *version)
            }
            MigrateError::VersionTooNew(version, _) => {
                Self::ledger(MigrationFailureKind::Version, "version_too_new", *version)
            }
            MigrateError::Dirty(version) => {
                Self::ledger(MigrationFailureKind::Dirty, "dirty", *version)
            }
            _ => Self::kind(MigrationFailureKind::Execute),
        }
    }

    const fn kind(kind: MigrationFailureKind) -> Self {
        Self {
            kind,
            sqlstate: None,
            reason: None,
            variant: None,
            version: None,
        }
    }

    const fn ledger(kind: MigrationFailureKind, variant: &'static str, version: i64) -> Self {
        Self {
            kind,
            sqlstate: None,
            reason: None,
            variant: Some(variant),
            version: Some(version),
        }
    }

    fn from_sqlx(error: &sqlx::Error) -> Self {
        match error {
            sqlx::Error::Tls(_) => Self::kind(MigrationFailureKind::Tls),
            // rustls reports a failed handshake, such as a rejected
            // certificate, as an I/O error of kind `InvalidData`.
            sqlx::Error::Io(io) if io.kind() == std::io::ErrorKind::InvalidData => {
                Self::kind(MigrationFailureKind::Tls)
            }
            sqlx::Error::Io(_)
            | sqlx::Error::PoolTimedOut
            | sqlx::Error::PoolClosed
            | sqlx::Error::Protocol(_)
            | sqlx::Error::Configuration(_) => Self::kind(MigrationFailureKind::Connect),
            sqlx::Error::Database(database) => {
                let sqlstate = database.code().map(std::borrow::Cow::into_owned);
                let code = sqlstate.as_deref().unwrap_or_default();
                // Classes 08 and 28 and SQLSTATE 3D000 occur at sign-in.
                let kind = if code.starts_with("08") || code.starts_with("28") || code == "3D000" {
                    MigrationFailureKind::Connect
                } else {
                    MigrationFailureKind::Execute
                };
                let reason = (code == "P0001").then(|| database.message().to_owned());
                Self {
                    kind,
                    sqlstate,
                    reason,
                    variant: None,
                    version: None,
                }
            }
            _ => Self::kind(MigrationFailureKind::Execute),
        }
    }
}

/// Applies the pending migrations.
///
/// # Errors
///
/// Returns the classified [`MigrationFailure`] when the run fails.
pub async fn migrate(pool: &PgPool) -> Result<(), MigrationFailure> {
    MIGRATOR
        .run(pool)
        .await
        .map_err(|error| MigrationFailure::classify(&error))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn kind_of(error: &MigrateError) -> MigrationFailureKind {
        let failure = MigrationFailure::classify(error);
        assert_eq!(failure.sqlstate, None);
        assert_eq!(failure.reason, None);
        failure.kind
    }

    #[test]
    fn the_default_pool_keeps_three_idle_connections() {
        let settings = PoolSettings::default();
        assert_eq!(settings.min_connections, 3);
        assert_eq!(settings.max_connections, 10);
        assert!(settings.min_connections <= settings.max_connections);
    }

    #[tokio::test]
    async fn the_lazy_pool_uses_the_pool_settings() {
        let url = Secret::new("postgres://user@localhost:5432/spot".to_owned());
        let settings = PoolSettings {
            max_connections: 7,
            min_connections: 2,
            acquire_timeout: Duration::from_secs(4),
            statement_timeout: Duration::from_secs(30),
        };
        let pool = connect_lazy(&url, settings).unwrap();
        let options = pool.options();
        assert_eq!(options.get_min_connections(), 2);
        assert_eq!(options.get_max_connections(), 7);
        assert_eq!(options.get_acquire_timeout(), Duration::from_secs(4));
        assert_eq!(pool.size(), 0, "a lazy pool opens no connection at start");
        pool.close().await;
    }

    #[test]
    fn classifies_errors_without_a_database_error() {
        let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        let cases = [
            (
                MigrateError::Execute(sqlx::Error::PoolTimedOut),
                MigrationFailureKind::Connect,
            ),
            (
                MigrateError::Execute(sqlx::Error::Io(refused)),
                MigrationFailureKind::Connect,
            ),
            (
                MigrateError::Execute(sqlx::Error::Tls("postgres://user:secret@host".into())),
                MigrationFailureKind::Tls,
            ),
            (
                MigrateError::Execute(sqlx::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid peer certificate: UnknownIssuer",
                ))),
                MigrationFailureKind::Tls,
            ),
            (
                MigrateError::ExecuteMigration(sqlx::Error::PoolClosed, 2),
                MigrationFailureKind::Connect,
            ),
            (
                MigrateError::ExecuteMigration(sqlx::Error::RowNotFound, 2),
                MigrationFailureKind::Execute,
            ),
            (
                MigrateError::VersionMismatch(1),
                MigrationFailureKind::Checksum,
            ),
            (
                MigrateError::VersionMissing(9),
                MigrationFailureKind::Version,
            ),
            (
                MigrateError::VersionNotPresent(9),
                MigrationFailureKind::Version,
            ),
            (
                MigrateError::VersionTooOld(1, 2),
                MigrationFailureKind::Version,
            ),
            (
                MigrateError::VersionTooNew(3, 2),
                MigrationFailureKind::Version,
            ),
            (MigrateError::Dirty(2), MigrationFailureKind::Dirty),
            (
                MigrateError::Source("migrations/secret".into()),
                MigrationFailureKind::Execute,
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(kind_of(&error), expected, "{error}");
        }
    }

    #[test]
    fn a_ledger_error_names_its_variant_and_version() {
        let cases = [
            (MigrateError::VersionMissing(4), "version_missing", 4),
            (MigrateError::VersionNotPresent(5), "version_not_present", 5),
            (MigrateError::VersionTooOld(1, 3), "version_too_old", 1),
            (MigrateError::VersionTooNew(6, 3), "version_too_new", 6),
            (MigrateError::VersionMismatch(2), "version_mismatch", 2),
            (MigrateError::Dirty(3), "dirty", 3),
        ];
        for (error, variant, version) in cases {
            let failure = MigrationFailure::classify(&error);
            assert_eq!(failure.variant, Some(variant), "{error}");
            assert_eq!(failure.version, Some(version), "{error}");
        }
        let other = MigrationFailure::classify(&MigrateError::Execute(sqlx::Error::PoolClosed));
        assert_eq!((other.variant, other.version), (None, None));
    }

    #[test]
    fn every_kind_has_a_distinct_stage() {
        let stages = [
            MigrationFailureKind::Connect,
            MigrationFailureKind::Tls,
            MigrationFailureKind::Checksum,
            MigrationFailureKind::Version,
            MigrationFailureKind::Dirty,
            MigrationFailureKind::Execute,
        ]
        .map(MigrationFailureKind::stage);
        let mut unique = stages.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), stages.len());
        assert!(!stages.contains(&"migrate_timeout"));
        assert!(stages.iter().all(|stage| stage.starts_with("migrate_")));
    }
}
