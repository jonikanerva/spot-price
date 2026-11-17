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

/// Every table of the TypeScript schema, for the row comparison.
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

/// Every row of every table of the TypeScript schema.
async fn rows(pool: &PgPool) -> Vec<String> {
    rows_of(pool, &TABLES).await
}

/// Every row of `tables`, as JSON text in a fixed order.
async fn rows_of(pool: &PgPool, tables: &[&str]) -> Vec<String> {
    let mut all = Vec::new();
    for table in tables {
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
        variant: None,
        version: None,
    }
}

/// The failure of a ledger error that names `variant` and `version`.
fn ledger_failure(
    kind: MigrationFailureKind,
    variant: &'static str,
    version: i64,
) -> MigrationFailure {
    MigrationFailure {
        variant: Some(variant),
        version: Some(version),
        ..failure(kind)
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
            variant: None,
            version: None,
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
            variant: None,
            version: None,
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
        Err(ledger_failure(
            MigrationFailureKind::Checksum,
            "version_mismatch",
            1
        ))
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
        Err(ledger_failure(
            MigrationFailureKind::Version,
            "version_missing",
            9999
        ))
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
        Err(ledger_failure(MigrationFailureKind::Dirty, "dirty", 2))
    );
}

#[sqlx::test(migrations = false)]
async fn a_server_without_tls_fails_a_required_tls_connection(pool: PgPool) {
    // The local test server runs with `ssl = off`.
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
            variant: None,
            version: None,
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
            "contract_terms_invalid_night_hours|not migrated".to_owned(),
            "user_settings_without_contract_terms|not migrated".to_owned(),
        ]
    );
    let rendered = lines.join("\n");
    assert!(
        !rendered.contains("bad-"),
        "the output must hold no user id"
    );
}

#[sqlx::test(migrations = false)]
async fn the_release_precheck_counts_the_invalid_terms_after_migration_4(pool: PgPool) {
    apply_ts_migrations(&pool, TS_MIGRATIONS.len()).await;
    insert_representative_rows(&pool).await;
    MIGRATOR.run_to(4, &pool).await.unwrap();
    let last_two = |lines: Vec<String>| lines[lines.len() - 2..].to_vec();
    assert_eq!(
        last_two(precheck(&pool).await),
        [
            "contract_terms_invalid_night_hours|0",
            "user_settings_without_contract_terms|0"
        ]
    );

    // The CHECK constraints keep such rows out; without them, the count
    // finds each invalid row once.
    sqlx::raw_sql(
        "ALTER TABLE contract_terms
           DROP CONSTRAINT contract_terms_night_start_hour_check,
           DROP CONSTRAINT contract_terms_night_end_hour_check;
         INSERT INTO contract_terms
           (user_id, valid_from, margin_cents_kwh, transfer_day_cents_kwh,
            transfer_night_cents_kwh, tax_cents_kwh, vat_percent, night_start_hour, night_end_hour)
         VALUES ('u1', '2026-01-01', 0, 0, 0, 0, 0, 24, 7),
                ('u1', '2026-02-01', 0, 0, 0, 0, 0, 22, -1),
                ('u1', '2026-03-01', 0, 0, 0, 0, 0, 23, 0);
         -- Two users without a version, like a sign-up through the release
         -- before migration 4 during the deploy.
         INSERT INTO user_settings (user_id) VALUES ('late-1'), ('late-2')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let lines = precheck(&pool).await;
    assert_eq!(lines.len(), 7);
    assert!(!lines.join("\n").contains("late-"), "no user id");
    assert_eq!(
        last_two(lines),
        [
            "contract_terms_invalid_night_hours|2",
            "user_settings_without_contract_terms|2"
        ]
    );
}

#[sqlx::test(migrations = false)]
async fn the_release_precheck_runs_after_migration_5(pool: PgPool) {
    apply_ts_migrations(&pool, TS_MIGRATIONS.len()).await;
    insert_representative_rows(&pool).await;
    db::migrate(&pool).await.unwrap();
    sqlx::raw_sql("INSERT INTO user_settings (user_id, timezone) VALUES ('late-1', 'UTC')")
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
            "user_settings_invalid_area|0".to_owned(),
            "user_settings_invalid_night_hours|dropped".to_owned(),
            "contract_terms_invalid_night_hours|0".to_owned(),
            "user_settings_without_contract_terms|1".to_owned(),
        ]
    );
}

/// `(user_id, valid_from, margin, day, night, tax, vat, start, end)` of each
/// `contract_terms` row.
type TermsRow = (String, String, f64, f64, f64, f64, f64, i32, i32);

async fn contract_terms(pool: &PgPool) -> Vec<TermsRow> {
    sqlx::query_as(
        "SELECT user_id, valid_from::text, margin_cents_kwh, transfer_day_cents_kwh,
                transfer_night_cents_kwh, tax_cents_kwh, vat_percent, night_start_hour,
                night_end_hour
         FROM contract_terms ORDER BY user_id, valid_from",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = false)]
async fn migration_4_copies_each_settings_row_as_the_first_version(pool: PgPool) {
    apply_ts_migrations(&pool, TS_MIGRATIONS.len()).await;
    insert_representative_rows(&pool).await;
    sqlx::raw_sql(
        "INSERT INTO user_settings
           (user_id, margin_cents_kwh, transfer_day_cents_kwh, transfer_night_cents_kwh,
            tax_cents_kwh, vat_percent, night_start_hour, night_end_hour, timezone, area)
         VALUES ('u2', 0.59, 3.5, 1.9, 2.79372, 24, 0, 23, 'Europe/Stockholm', 'SE3')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let rows_before = rows(&pool).await;

    MIGRATOR.run_to(4, &pool).await.unwrap();

    assert_eq!(
        contract_terms(&pool).await,
        [
            (
                "u1".to_owned(),
                "1970-01-01".to_owned(),
                0.49,
                2.92,
                1.37,
                2.82752,
                25.5,
                22,
                7
            ),
            (
                "u2".to_owned(),
                "1970-01-01".to_owned(),
                0.59,
                3.5,
                1.9,
                2.79372,
                24.0,
                0,
                23
            ),
        ]
    );
    // The migration is additive: every other table keeps its rows, and
    // `user_settings` keeps its term columns.
    assert_eq!(rows(&pool).await, rows_before);
    // A version goes with its settings row.
    sqlx::query("DELETE FROM user_settings WHERE user_id = 'u2'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(contract_terms(&pool).await.len(), 1);
}

#[sqlx::test(migrations = false)]
async fn migration_4_on_an_empty_database_creates_an_empty_table(pool: PgPool) {
    db::migrate(&pool).await.unwrap();
    assert_eq!(contract_terms(&pool).await, []);
    // A version needs a settings row and night hours in 0..=23.
    for (user, start, end, sqlstate) in [
        ("nobody", 22, 7, "23503"),
        ("u1", 24, 7, "23514"),
        ("u1", 22, -1, "23514"),
    ] {
        sqlx::query("INSERT INTO user_settings (user_id) VALUES ('u1') ON CONFLICT DO NOTHING")
            .execute(&pool)
            .await
            .unwrap();
        let error = sqlx::query(
            "INSERT INTO contract_terms
               (user_id, valid_from, margin_cents_kwh, transfer_day_cents_kwh,
                transfer_night_cents_kwh, tax_cents_kwh, vat_percent, night_start_hour,
                night_end_hour)
             VALUES ($1, '2026-01-01', 0, 0, 0, 0, 0, $2, $3)",
        )
        .bind(user)
        .bind(start)
        .bind(end)
        .execute(&pool)
        .await
        .unwrap_err();
        let code = error
            .as_database_error()
            .unwrap()
            .code()
            .unwrap()
            .into_owned();
        assert_eq!(code, sqlstate, "{user} {start} {end}");
    }
}

#[sqlx::test(migrations = false)]
async fn an_invalid_night_hour_fails_migration_4_and_changes_nothing(pool: PgPool) {
    apply_ts_migrations(&pool, TS_MIGRATIONS.len()).await;
    sqlx::raw_sql(
        "INSERT INTO user_settings (user_id, night_start_hour) VALUES ('good', 22), ('bad', 24)",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        db::migrate(&pool).await,
        Err(MigrationFailure {
            kind: MigrationFailureKind::Execute,
            sqlstate: Some("23514".to_owned()),
            reason: None,
            variant: None,
            version: None,
        })
    );
    let applied: Vec<i64> = sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY 1")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(applied, [1, 2, 3]);
    let table: Option<String> = sqlx::query_scalar("SELECT to_regclass('contract_terms')::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(table, None);

    // The owner corrects the row; the next run applies migration 4.
    sqlx::query("UPDATE user_settings SET night_start_hour = 23 WHERE user_id = 'bad'")
        .execute(&pool)
        .await
        .unwrap();
    db::migrate(&pool).await.unwrap();
    assert_eq!(contract_terms(&pool).await.len(), 2);
}

/// The indexes that migration 3 drops. Each duplicates another index.
const DROPPED_INDEXES: [&str; 3] = [
    "idx_fingrid_actuals_dataset_start",
    "idx_fingrid_forecasts_issued",
    "idx_api_keys_user_id",
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

    MIGRATOR.run_to(3, &pool).await.unwrap();

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

#[sqlx::test(migrations = false)]
async fn a_held_table_lock_fails_migration_3_and_keeps_the_indexes(pool: PgPool) {
    apply_ts_migrations(&pool, TS_MIGRATIONS.len()).await;
    MIGRATOR.run_to(2, &pool).await.unwrap();

    // An open write transaction of the running release, like the Fingrid job.
    let mut writer = pool.begin().await.unwrap();
    sqlx::raw_sql("LOCK TABLE fingrid_forecasts IN ROW EXCLUSIVE MODE")
        .execute(&mut *writer)
        .await
        .unwrap();

    let started = std::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(20), db::migrate(&pool))
        .await
        .expect("lock_timeout ends the wait");
    assert_eq!(
        result,
        Err(MigrationFailure {
            kind: MigrationFailureKind::Execute,
            sqlstate: Some("55P03".to_owned()),
            reason: None,
            variant: None,
            version: None,
        })
    );
    assert!(started.elapsed() >= Duration::from_secs(4));
    for name in DROPPED_INDEXES {
        assert!(index_exists(&pool, name).await, "{name} after the failure");
    }
    let applied: Vec<i64> = sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY 1")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(applied, [1, 2]);

    writer.rollback().await.unwrap();
    db::migrate(&pool).await.unwrap();
    for name in DROPPED_INDEXES {
        assert!(!index_exists(&pool, name).await, "{name} after the retry");
    }
}

/// The term columns of `user_settings` that migration 5 drops.
const DROPPED_TERM_COLUMNS: [&str; 7] = [
    "margin_cents_kwh",
    "transfer_day_cents_kwh",
    "transfer_night_cents_kwh",
    "tax_cents_kwh",
    "vat_percent",
    "night_start_hour",
    "night_end_hour",
];

/// The tables whose rows migration 5 must not change.
const TABLES_KEPT_BY_MIGRATION_5: [&str; 12] = [
    "\"user\"",
    "\"account\"",
    "usernames",
    "api_keys",
    "contract_terms",
    "app_sessions",
    "prices",
    "fingrid_actuals",
    "fingrid_forecasts",
    "weather_forecasts",
    "weather_daily_forecasts",
    "_migrations",
];

/// True for a snapshot line of an object that migration 5 drops.
fn dropped_by_migration_5(line: &str) -> bool {
    let fields: Vec<&str> = line.split('|').collect();
    let dropped_table = |name: &str| name == "session" || name == "verification";
    match fields[0] {
        "column" => {
            dropped_table(fields[1])
                || (fields[1] == "user_settings" && DROPPED_TERM_COLUMNS.contains(&fields[3]))
        }
        "constraint" | "index" => dropped_table(fields[1]),
        "relation" => {
            dropped_table(fields[3])
                || fields[3].starts_with("session_")
                || fields[3].starts_with("verification_")
        }
        _ => false,
    }
}

async fn user_settings_columns(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT column_name::text FROM information_schema.columns
         WHERE table_schema = current_schema() AND table_name = 'user_settings'
         ORDER BY ordinal_position",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn table_exists(pool: &PgPool, name: &str) -> bool {
    sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = false)]
async fn migration_5_drops_only_the_unused_objects_on_a_typescript_database(pool: PgPool) {
    apply_ts_migrations(&pool, TS_MIGRATIONS.len()).await;
    insert_representative_rows(&pool).await;
    MIGRATOR.run_to(4, &pool).await.unwrap();
    // A second version, which the term columns of `user_settings` never had.
    sqlx::raw_sql(
        "INSERT INTO contract_terms
           (user_id, valid_from, margin_cents_kwh, transfer_day_cents_kwh,
            transfer_night_cents_kwh, tax_cents_kwh, vat_percent, night_start_hour, night_end_hour)
         VALUES ('u1', '2026-12-01', 0.59, 2.92, 1.37, 2.82752, 25.5, 22, 7);
         INSERT INTO app_sessions (token_hash, user_id, created_at, expires_at)
         VALUES (sha256('token'::bytea), 'u1', '2026-01-01T00:00:00Z', '2026-01-08T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let schema_before = snapshot(&pool).await;
    let rows_before = rows_of(&pool, &TABLES_KEPT_BY_MIGRATION_5).await;
    let settings_before: Vec<(String, String, String, String)> = settings_rows(&pool).await;

    db::migrate(&pool).await.unwrap();
    db::ensure_migrated(&pool).await.unwrap();

    // The snapshot loses the lines of the dropped objects. The snapshot
    // numbers the columns of a table from 1, so the three last columns of
    // `user_settings` get new numbers. Nothing else changes.
    let schema_after = snapshot(&pool).await;
    let removed: Vec<&str> = schema_before
        .lines()
        .filter(|line| !schema_after.lines().any(|after| after == *line))
        .collect();
    let added: Vec<&str> = schema_after
        .lines()
        .filter(|line| !schema_before.lines().any(|before| before == *line))
        .collect();
    let renumbered_before = [
        "column|user_settings|009|timezone|text|not null|'Europe/Helsinki'::text|||",
        "column|user_settings|010|area|text|not null|'FI'::text|||",
        "column|user_settings|011|updated_at|timestamp with time zone|not null|now()|||",
    ];
    assert_eq!(
        added,
        [
            "column|user_settings|002|timezone|text|not null|'Europe/Helsinki'::text|||",
            "column|user_settings|003|area|text|not null|'FI'::text|||",
            "column|user_settings|004|updated_at|timestamp with time zone|not null|now()|||",
        ]
    );
    let dropped: Vec<&str> = removed
        .iter()
        .copied()
        .filter(|line| !renumbered_before.contains(line))
        .collect();
    assert_eq!(removed.len(), dropped.len() + renumbered_before.len());
    for line in &dropped {
        assert!(dropped_by_migration_5(line), "{line}");
    }
    // 8 + 6 + 7 columns, 3 + 1 constraints, 3 + 2 indexes, 4 + 3 relations.
    assert_eq!(dropped.len(), 37);
    assert!(!table_exists(&pool, "\"session\"").await);
    assert!(!table_exists(&pool, "verification").await);
    assert_eq!(
        user_settings_columns(&pool).await,
        ["user_id", "timezone", "area", "updated_at"]
    );
    assert_eq!(
        rows_of(&pool, &TABLES_KEPT_BY_MIGRATION_5).await,
        rows_before
    );
    assert_eq!(settings_rows(&pool).await, settings_before);
    assert_eq!(settings_before.len(), 1);
}

/// `(user_id, timezone, area, updated_at)` of each `user_settings` row.
async fn settings_rows(pool: &PgPool) -> Vec<(String, String, String, String)> {
    sqlx::query_as(
        "SELECT user_id, timezone, area, updated_at::text FROM user_settings ORDER BY user_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = false)]
async fn migration_5_applies_on_an_empty_database(pool: PgPool) {
    db::migrate(&pool).await.unwrap();
    assert!(!table_exists(&pool, "\"session\"").await);
    assert!(!table_exists(&pool, "verification").await);
    assert_eq!(
        user_settings_columns(&pool).await,
        ["user_id", "timezone", "area", "updated_at"]
    );
    // The kept columns keep their defaults.
    sqlx::query("INSERT INTO user_settings (user_id) VALUES ('u1')")
        .execute(&pool)
        .await
        .unwrap();
    let rows = settings_rows(&pool).await;
    assert_eq!(
        (rows[0].1.as_str(), rows[0].2.as_str()),
        ("Europe/Helsinki", "FI")
    );
}

#[sqlx::test(migrations = false)]
async fn a_held_table_lock_fails_migration_5_and_drops_nothing(pool: PgPool) {
    apply_ts_migrations(&pool, TS_MIGRATIONS.len()).await;
    insert_representative_rows(&pool).await;
    MIGRATOR.run_to(4, &pool).await.unwrap();
    let schema_before = snapshot(&pool).await;
    let rows_before = rows(&pool).await;

    // An open read transaction of the running release, like a price request.
    let mut reader = pool.begin().await.unwrap();
    sqlx::raw_sql("LOCK TABLE user_settings IN ACCESS SHARE MODE")
        .execute(&mut *reader)
        .await
        .unwrap();

    let started = std::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(20), db::migrate(&pool))
        .await
        .expect("lock_timeout ends the wait");
    assert_eq!(
        result,
        Err(MigrationFailure {
            kind: MigrationFailureKind::Execute,
            sqlstate: Some("55P03".to_owned()),
            reason: None,
            variant: None,
            version: None,
        })
    );
    assert!(started.elapsed() >= Duration::from_secs(4));
    assert_eq!(snapshot(&pool).await, schema_before);
    assert_eq!(rows(&pool).await, rows_before);
    let applied: Vec<i64> = sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY 1")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(applied, [1, 2, 3, 4]);

    reader.rollback().await.unwrap();
    db::migrate(&pool).await.unwrap();
    assert!(!table_exists(&pool, "\"session\"").await);
}
