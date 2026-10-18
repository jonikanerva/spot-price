//! Migration paths: a database of the TypeScript migrations 001 to 007, an
//! empty database, a repeated run, and a partial TypeScript ledger. The
//! classified failures of `db::migrate`. The release precheck.
//!
//! The expected catalog is `fixtures/schema-snapshot.txt`, generated from the
//! unchanged TypeScript migrations in `fixtures/ts-migrations/` with
//! `scripts/schema-snapshot.sql`.

use std::time::Duration;

use spot_price_domain::areas::ALL_AREAS;
use spot_price_server::db::{self, MIGRATOR, MigrationFailure, MigrationFailureKind, StoreError};
use sqlx::postgres::{PgPoolOptions, PgSslMode};
use sqlx::{AssertSqlSafe, PgPool};

const TS_MIGRATIONS: [(i32, &str, &str); 7] = [
    (
        1,
        "baseline",
        include_str!("../fixtures/ts-migrations/001_baseline.sql"),
    ),
    (
        2,
        "fingrid_series",
        include_str!("../fixtures/ts-migrations/002_fingrid_series.sql"),
    ),
    (
        3,
        "scrub_session_ip_ua",
        include_str!("../fixtures/ts-migrations/003_scrub_session_ip_ua.sql"),
    ),
    (
        4,
        "weather_series",
        include_str!("../fixtures/ts-migrations/004_weather_series.sql"),
    ),
    (
        5,
        "fingrid_forecasts",
        include_str!("../fixtures/ts-migrations/005_fingrid_forecasts.sql"),
    ),
    (
        6,
        "normalize_table_names",
        include_str!("../fixtures/ts-migrations/006_normalize_table_names.sql"),
    ),
    (
        7,
        "weather_daily_forecasts",
        include_str!("../fixtures/ts-migrations/007_weather_daily_forecasts.sql"),
    ),
];

const SNAPSHOT_SQL: &str = include_str!("../../../../scripts/schema-snapshot.sql");
const PRECHECK_SQL: &str = include_str!("../../../../scripts/release-precheck.sql");
const EXPECTED_SNAPSHOT: &str = include_str!("../fixtures/schema-snapshot.txt");

/// The version of the baseline migration.
const BASELINE: i64 = 1;

/// Every application table, for the row comparison.
const TABLES: [&str; 13] = [
    "\"user\"",
    "\"session\"",
    "\"account\"",
    "\"verification\"",
    "usernames",
    "api_keys",
    "user_settings",
    "prices",
    "fingrid_actuals",
    "fingrid_forecasts",
    "weather_forecasts",
    "weather_daily_forecasts",
    "_migrations",
];

/// The normalised catalog, one line per object, like `psql -A -t`.
async fn snapshot(pool: &PgPool) -> String {
    let lines: Vec<String> = sqlx::query_scalar(SNAPSHOT_SQL)
        .fetch_all(pool)
        .await
        .unwrap();
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// Every row of every table, as JSON text in a fixed order.
async fn rows(pool: &PgPool) -> Vec<String> {
    let mut all = Vec::new();
    for table in TABLES {
        let sql = format!(
            "SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text), '[]')::text FROM {table} t"
        );
        // The table names are the constants above, not input.
        let json: String = sqlx::query_scalar(AssertSqlSafe(sql))
            .fetch_one(pool)
            .await
            .unwrap();
        all.push(format!("{table}: {json}"));
    }
    all
}

/// Applies the TypeScript migrations like the TypeScript migrator did: each
/// file and its ledger row in one transaction.
async fn apply_ts_migrations(pool: &PgPool, count: usize) {
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS _migrations (
           version INTEGER PRIMARY KEY,
           name TEXT NOT NULL,
           applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
         )",
    )
    .execute(pool)
    .await
    .unwrap();
    for (version, name, sql) in TS_MIGRATIONS.iter().take(count) {
        let mut transaction = pool.begin().await.unwrap();
        sqlx::raw_sql(*sql)
            .execute(&mut *transaction)
            .await
            .unwrap();
        sqlx::query("INSERT INTO _migrations (version, name) VALUES ($1, $2)")
            .bind(version)
            .bind(name)
            .execute(&mut *transaction)
            .await
            .unwrap();
        transaction.commit().await.unwrap();
    }
}

/// One row in every table, including values that the data changes of the
/// TypeScript migrations 003 and 006 would remove.
async fn insert_representative_rows(pool: &PgPool) {
    sqlx::raw_sql(
        r#"
        INSERT INTO "user" (id, name, email, "emailVerified", image, "createdAt", "updatedAt")
        VALUES ('u1', 'Name', 'user@example.test', false, NULL, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
        INSERT INTO "session" (id, "expiresAt", token, "createdAt", "updatedAt", "ipAddress", "userAgent", "userId")
        VALUES ('s1', '2026-02-01T00:00:00Z', 'token-1', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                '192.0.2.1', 'agent', 'u1');
        INSERT INTO "account" (id, "accountId", "providerId", "userId", password, "createdAt", "updatedAt")
        VALUES ('a1', 'u1', 'credential', 'u1', 'hash', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
        INSERT INTO "verification" (id, identifier, value, "expiresAt", "createdAt", "updatedAt")
        VALUES ('v1', 'id', 'value', '2026-02-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
        INSERT INTO usernames (user_id, username, created_at) VALUES ('u1', 'name', '2026-01-01T00:00:00Z');
        INSERT INTO api_keys (id, user_id, key_plaintext, created_at, last_used_at)
        VALUES ('k1', 'u1', 'sp_key', '2026-01-01T00:00:00Z', NULL);
        INSERT INTO user_settings (user_id, area, updated_at) VALUES ('u1', 'SE3', '2026-01-01T00:00:00Z');
        INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area, fetched_at)
        VALUES ('2026-01-01T00:00:00Z', '2026-01-01T00:15:00Z', 12.5, 'FI', '2026-01-01T00:00:00Z');
        INSERT INTO fingrid_actuals (dataset_id, start_time, end_time, value, fetched_at)
        VALUES (75, '2026-01-01T00:00:00Z', '2026-01-01T00:15:00Z', 100, '2026-01-01T00:00:00Z'),
               (245, '2026-01-01T00:00:00Z', '2026-01-01T00:15:00Z', 200, '2026-01-01T00:00:00Z');
        INSERT INTO fingrid_forecasts (dataset_id, issued_at, start_time, end_time, value, fetched_at)
        VALUES (245, '2026-01-01T00:00:00Z', '2026-01-01T01:00:00Z', '2026-01-01T01:15:00Z', 300,
                '2026-01-01T00:00:00Z');
        INSERT INTO weather_forecasts (point_id, issued_at, target_time, temp, clouds, uvi, wind_speed, wind_deg, fetched_at)
        VALUES ('helsinki', '2026-01-01T00:00:00Z', '2026-01-01T01:00:00Z', -5, 80, 0, 4, 180,
                '2026-01-01T00:00:00Z');
        INSERT INTO weather_daily_forecasts (point_id, issued_at, target_date, target_dt, temp_morn, temp_day,
                                             temp_eve, temp_night, temp_min, temp_max, clouds, uvi, sunrise,
                                             sunset, fetched_at)
        VALUES ('vaasa', '2026-01-01T00:00:00Z', '2026-01-01', '2026-01-01T10:00:00Z', -6, -3, -4, -7, -8, -2,
                90, 0.1, NULL, '2026-01-01T13:00:00Z', '2026-01-01T00:00:00Z');
        "#,
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn applied_migrations(pool: &PgPool) -> Vec<(i64, String)> {
    sqlx::query_as("SELECT version, installed_on::text FROM _sqlx_migrations ORDER BY version")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = false)]
async fn the_typescript_migrations_produce_the_expected_snapshot(pool: PgPool) {
    apply_ts_migrations(&pool, TS_MIGRATIONS.len()).await;
    assert_eq!(snapshot(&pool).await, EXPECTED_SNAPSHOT);
}

#[sqlx::test(migrations = false)]
async fn the_baseline_on_a_typescript_database_changes_nothing(pool: PgPool) {
    apply_ts_migrations(&pool, TS_MIGRATIONS.len()).await;
    insert_representative_rows(&pool).await;
    let schema_before = snapshot(&pool).await;
    let rows_before = rows(&pool).await;

    MIGRATOR.run_to(BASELINE, &pool).await.unwrap();

    assert_eq!(snapshot(&pool).await, schema_before);
    assert_eq!(rows(&pool).await, rows_before);
    let ip: Option<String> = sqlx::query_scalar(r#"SELECT "ipAddress" FROM "session""#)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(ip.as_deref(), Some("192.0.2.1"));
    let applied = applied_migrations(&pool).await;
    assert_eq!(
        applied.iter().map(|(v, _)| *v).collect::<Vec<_>>(),
        [BASELINE]
    );
}

#[sqlx::test(migrations = false)]
async fn every_migration_applies_on_a_typescript_database(pool: PgPool) {
    apply_ts_migrations(&pool, TS_MIGRATIONS.len()).await;
    insert_representative_rows(&pool).await;
    db::migrate(&pool).await.unwrap();
    db::ensure_migrated(&pool).await.unwrap();
}

#[sqlx::test(migrations = false)]
async fn the_baseline_on_an_empty_database_creates_the_typescript_schema(pool: PgPool) {
    MIGRATOR.run_to(BASELINE, &pool).await.unwrap();
    assert_eq!(snapshot(&pool).await, EXPECTED_SNAPSHOT);
    let ledger: Option<String> = sqlx::query_scalar("SELECT to_regclass('_migrations')::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        ledger, None,
        "the baseline must not create the TypeScript ledger"
    );
}

#[sqlx::test(migrations = false)]
async fn a_repeated_run_applies_nothing(pool: PgPool) {
    db::migrate(&pool).await.unwrap();
    let first = applied_migrations(&pool).await;
    let schema = snapshot(&pool).await;
    db::migrate(&pool).await.unwrap();
    assert_eq!(applied_migrations(&pool).await, first);
    assert_eq!(first.len(), MIGRATOR.iter().count());
    assert_eq!(snapshot(&pool).await, schema);
}

#[sqlx::test(migrations = false)]
async fn the_baseline_refuses_a_partial_typescript_ledger(pool: PgPool) {
    apply_ts_migrations(&pool, 5).await;
    let schema_before = snapshot(&pool).await;

    assert!(MIGRATOR.run_to(BASELINE, &pool).await.is_err());

    assert_eq!(snapshot(&pool).await, schema_before);
    let created: Option<String> = sqlx::query_scalar("SELECT to_regclass('fingrid_actuals')::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(created, None);
}

#[sqlx::test(migrations = false)]
async fn serve_refuses_an_unmigrated_database(pool: PgPool) {
    assert_eq!(
        db::ensure_migrated(&pool).await,
        Err(StoreError::NotMigrated)
    );
    MIGRATOR.run_to(BASELINE, &pool).await.unwrap();
    if MIGRATOR.iter().count() > 1 {
        assert_eq!(
            db::ensure_migrated(&pool).await,
            Err(StoreError::NotMigrated)
        );
    }
    db::migrate(&pool).await.unwrap();
    assert_eq!(db::ensure_migrated(&pool).await, Ok(()));
}

fn failure(kind: MigrationFailureKind) -> MigrationFailure {
    MigrationFailure {
        kind,
        sqlstate: None,
        reason: None,
    }
}

#[sqlx::test(migrations = false)]
async fn a_partial_typescript_ledger_fails_with_the_text_of_the_baseline_guard(pool: PgPool) {
    apply_ts_migrations(&pool, 5).await;
    assert_eq!(
        db::migrate(&pool).await,
        Err(MigrationFailure {
            kind: MigrationFailureKind::Execute,
            sqlstate: Some("P0001".to_owned()),
            reason: Some("the TypeScript migrations 001 to 007 are not all applied".to_owned()),
        })
    );
}

#[sqlx::test(migrations = false)]
async fn a_failed_statement_logs_its_sqlstate_but_no_text(pool: PgPool) {
    sqlx::raw_sql("CREATE TABLE prices (id TEXT)")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        db::migrate(&pool).await,
        Err(MigrationFailure {
            kind: MigrationFailureKind::Execute,
            sqlstate: Some("42703".to_owned()),
            reason: None,
        })
    );
}

#[sqlx::test(migrations = false)]
async fn a_modified_migration_fails_as_a_checksum_mismatch(pool: PgPool) {
    db::migrate(&pool).await.unwrap();
    sqlx::raw_sql("UPDATE _sqlx_migrations SET checksum = '\\x00' WHERE version = 1")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        db::migrate(&pool).await,
        Err(failure(MigrationFailureKind::Checksum))
    );
}

#[sqlx::test(migrations = false)]
async fn an_unknown_applied_migration_fails_as_a_version_mismatch(pool: PgPool) {
    db::migrate(&pool).await.unwrap();
    sqlx::raw_sql(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
         VALUES (9999, 'newer build', true, '\\x00', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        db::migrate(&pool).await,
        Err(failure(MigrationFailureKind::Version))
    );
}

#[sqlx::test(migrations = false)]
async fn an_unsuccessful_ledger_row_fails_as_dirty(pool: PgPool) {
    db::migrate(&pool).await.unwrap();
    sqlx::raw_sql("UPDATE _sqlx_migrations SET success = false WHERE version = 2")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        db::migrate(&pool).await,
        Err(failure(MigrationFailureKind::Dirty))
    );
}

#[sqlx::test(migrations = false)]
async fn a_server_without_tls_fails_a_required_tls_connection(pool: PgPool) {
    // The isolated test server runs with `ssl = off`.
    let options = (*pool.connect_options())
        .clone()
        .ssl_mode(PgSslMode::Require);
    let tls_pool = PgPoolOptions::new()
        .acquire_timeout(Duration::from_secs(5))
        .connect_lazy_with(options);
    assert_eq!(
        db::migrate(&tls_pool).await,
        Err(failure(MigrationFailureKind::Tls))
    );
}

#[sqlx::test(migrations = false)]
async fn a_refused_connection_fails_as_a_connection_failure(pool: PgPool) {
    let options = (*pool.connect_options()).clone().port(1);
    let refused = PgPoolOptions::new()
        .acquire_timeout(Duration::from_secs(1))
        .connect_lazy_with(options);
    assert_eq!(
        db::migrate(&refused).await,
        Err(failure(MigrationFailureKind::Connect))
    );
}

#[sqlx::test(migrations = false)]
async fn a_missing_database_fails_as_a_connection_failure(pool: PgPool) {
    let options = (*pool.connect_options())
        .clone()
        .database("spot_price_missing_database");
    let missing = PgPoolOptions::new()
        .acquire_timeout(Duration::from_secs(5))
        .connect_lazy_with(options);
    assert_eq!(
        db::migrate(&missing).await,
        Err(MigrationFailure {
            kind: MigrationFailureKind::Connect,
            sqlstate: Some("3D000".to_owned()),
            reason: None,
        })
    );
}

/// Runs `scripts/release-precheck.sql` in a read-only transaction, like
/// `psql -A -t` with `default_transaction_read_only=on`.
async fn precheck(pool: &PgPool) -> Vec<String> {
    let mut transaction = pool.begin().await.unwrap();
    sqlx::raw_sql("SET TRANSACTION READ ONLY")
        .execute(&mut *transaction)
        .await
        .unwrap();
    let lines: Vec<String> = sqlx::query_scalar(PRECHECK_SQL)
        .fetch_all(&mut *transaction)
        .await
        .unwrap();
    transaction.rollback().await.unwrap();
    lines
}

#[sqlx::test(migrations = false)]
async fn the_release_precheck_counts_the_settings_that_the_service_rejects(pool: PgPool) {
    apply_ts_migrations(&pool, TS_MIGRATIONS.len()).await;
    // One valid row for each area with its default zone covers every
    // supported area and zone of the domain.
    for area in ALL_AREAS {
        sqlx::query("INSERT INTO user_settings (user_id, area, timezone) VALUES ($1, $2, $3)")
            .bind(format!("good-{}", area.code()))
            .bind(area.code())
            .bind(area.timezone())
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::raw_sql(
        "INSERT INTO user_settings (user_id, area, timezone, night_start_hour, night_end_hour) VALUES
           ('bad-zone', 'FI', 'europe/helsinki', 22, 7),
           ('bad-area', 'DE', 'Europe/Berlin', 22, 7),
           ('bad-start', 'FI', 'Europe/Helsinki', 24, 7),
           ('bad-end', 'FI', 'Europe/Helsinki', 22, -1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let version: String = sqlx::query_scalar("SELECT current_setting('server_version')")
        .fetch_one(&pool)
        .await
        .unwrap();

    let lines = precheck(&pool).await;

    assert_eq!(
        lines,
        [
            "typescript_migrations|1,2,3,4,5,6,7".to_owned(),
            format!("server_version|{version}"),
            "user_settings_invalid_timezone|1".to_owned(),
            "user_settings_invalid_area|1".to_owned(),
            "user_settings_invalid_night_hours|2".to_owned(),
        ]
    );
    let rendered = lines.join("\n");
    assert!(
        !rendered.contains("bad-"),
        "the output must hold no user id"
    );
}

/// The indexes that migration 3 drops (#128). Each duplicates another index.
const DROPPED_INDEXES: [&str; 3] = [
    "idx_fingrid_actuals_dataset_start",
    "idx_api_keys_user_id",
    "idx_fingrid_forecasts_issued",
];

async fn index_exists(pool: &PgPool, name: &str) -> bool {
    sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = false)]
async fn the_redundant_indexes_are_dropped_on_a_typescript_database(pool: PgPool) {
    apply_ts_migrations(&pool, TS_MIGRATIONS.len()).await;
    insert_representative_rows(&pool).await;
    for name in DROPPED_INDEXES {
        assert!(index_exists(&pool, name).await, "{name} before");
    }
    let rows_before = rows(&pool).await;

    db::migrate(&pool).await.unwrap();

    for name in DROPPED_INDEXES {
        assert!(!index_exists(&pool, name).await, "{name} after");
    }
    // The indexes that replace them stay.
    for name in [
        "fingrid_actuals_pkey",
        "idx_api_keys_single_per_user",
        "fingrid_forecasts_pkey",
        "idx_fingrid_forecasts_target_issued",
    ] {
        assert!(index_exists(&pool, name).await, "{name} stays");
    }
    assert_eq!(rows(&pool).await, rows_before);
}

#[sqlx::test(migrations = false)]
async fn the_redundant_indexes_are_dropped_on_an_empty_database(pool: PgPool) {
    db::migrate(&pool).await.unwrap();
    for name in DROPPED_INDEXES {
        assert!(!index_exists(&pool, name).await, "{name}");
    }
}
