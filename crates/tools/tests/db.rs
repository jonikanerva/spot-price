//! The `--db` reads against PostgreSQL. `DATABASE_URL` must point to an
//! isolated server where the test user can create databases;
//! `scripts/test-db.sh start` starts one. Each test gets a fresh database
//! with the workspace migrations.

#[cfg(test)]
mod db {
    use std::collections::BTreeMap;

    use jiff::Timestamp;
    use spot_price_domain::price::eur_mwh_to_cents_kwh;
    use spot_price_tools::data::{BacktestData, GridActual, PricePoint, Vintage};
    use spot_price_tools::db::{read_backtest_data, read_study_input};
    use spot_price_tools::error::ToolError;
    use spot_price_tools::revision::ActualRecord;
    use sqlx::{PgConnection, PgPool};

    const NOW: &str = "2026-03-10T12:00:00Z";
    const WINDOW_DAYS: i64 = 2;
    /// `NOW` minus `WINDOW_DAYS`: the inclusive start of the window.
    const START: &str = "2026-03-08T12:00:00Z";

    fn ts(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn plus_minutes(text: &str, minutes: i64) -> String {
        let at = ts(text)
            .checked_add(jiff::SignedDuration::from_mins(minutes))
            .unwrap();
        at.to_string()
    }

    async fn price(conn: &mut PgConnection, area: &str, start: &str, eur_mwh: f64) {
        sqlx::query(
            "INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area) \
             VALUES ($1::timestamptz, $1::timestamptz + interval '15 minutes', $2, $3)",
        )
        .bind(start)
        .bind(eur_mwh)
        .bind(area)
        .execute(conn)
        .await
        .unwrap();
    }

    async fn actual(conn: &mut PgConnection, dataset_id: i32, start: &str, value: f64) {
        sqlx::query(
            "INSERT INTO fingrid_actuals (dataset_id, start_time, end_time, value) \
             VALUES ($1, $2::timestamptz, $2::timestamptz + interval '15 minutes', $3)",
        )
        .bind(dataset_id)
        .bind(start)
        .bind(value)
        .execute(conn)
        .await
        .unwrap();
    }

    async fn vintage(
        conn: &mut PgConnection,
        dataset_id: i32,
        issued: &str,
        start: &str,
        value: f64,
    ) {
        sqlx::query(
            "INSERT INTO fingrid_forecasts (dataset_id, issued_at, start_time, end_time, value) \
             VALUES ($1, $2::timestamptz, $3::timestamptz, $3::timestamptz + interval '15 minutes', $4)",
        )
        .bind(dataset_id)
        .bind(issued)
        .bind(start)
        .bind(value)
        .execute(conn)
        .await
        .unwrap();
    }

    fn grid(dataset_id: i32, start: &str, value: f64) -> GridActual {
        GridActual {
            dataset_id,
            start: ts(start),
            end: ts(&plus_minutes(start, 15)),
            value,
        }
    }

    fn vin(dataset_id: i32, issued: &str, start: &str, value: f64) -> Vintage {
        Vintage {
            dataset_id,
            issued_at: ts(issued),
            start: ts(start),
            end: ts(&plus_minutes(start, 15)),
            value,
        }
    }

    /// Rows inside and just outside the window, inserted out of order.
    async fn seed(conn: &mut PgConnection) {
        let before = plus_minutes(START, -15);
        let last = plus_minutes(NOW, -15);
        price(conn, "SE1", START, 101.0).await;
        price(conn, "FI", &last, 312.5).await;
        price(conn, "FI", START, 85.46).await;
        price(conn, "FI", &before, 1.0).await;
        price(conn, "FI", NOW, 2.0).await;
        price(conn, "SE2", START, 3.0).await;
        price(conn, "EE", START, -12.34).await;
        // A sub-millisecond part is cut, like a JavaScript `Date`.
        price(conn, "FI", "2026-03-09T00:00:00.000999Z", 50.0).await;
        actual(conn, 75, &last, 2000.0).await;
        actual(conn, 75, START, 2100.0).await;
        actual(conn, 124, START, 8000.0).await;
        actual(conn, 124, &before, 7000.0).await;
        actual(conn, 999, START, 1.0).await;
        vintage(conn, 245, "2026-03-08T10:00:00Z", &last, 3000.0).await;
        vintage(conn, 245, "2026-03-08T11:00:00Z", START, 3100.0).await;
        vintage(conn, 245, "2026-03-08T09:00:00Z", START, 3200.0).await;
        vintage(conn, 165, "2026-03-07T00:00:00Z", START, 9000.0).await;
        vintage(conn, 165, "2026-03-07T00:00:00Z", NOW, 9100.0).await;
        vintage(conn, 75, "2026-03-07T00:00:00Z", START, 1.0).await;
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn reads_the_backtest_window_like_the_typescript_store(pool: PgPool) {
        let mut conn = pool.acquire().await.unwrap();
        seed(&mut conn).await;
        let got = read_backtest_data(&mut conn, WINDOW_DAYS, ts(NOW))
            .await
            .unwrap();

        let last = plus_minutes(NOW, -15);
        let p = |area: &str, start: &str, eur: f64| PricePoint {
            area: area.to_owned(),
            start: ts(start),
            spot_cents_kwh: eur_mwh_to_cents_kwh(eur),
        };
        let want = BacktestData {
            // Ordered by area, then delivery start.
            prices: vec![
                p("EE", START, -12.34),
                p("FI", START, 85.46),
                p("FI", "2026-03-09T00:00:00Z", 50.0),
                p("FI", &last, 312.5),
                p("SE1", START, 101.0),
            ],
            actuals: BTreeMap::from([
                (75, vec![grid(75, START, 2100.0), grid(75, &last, 2000.0)]),
                (124, vec![grid(124, START, 8000.0)]),
            ]),
            vintages: BTreeMap::from([
                (
                    245,
                    vec![
                        vin(245, "2026-03-08T09:00:00Z", START, 3200.0),
                        vin(245, "2026-03-08T11:00:00Z", START, 3100.0),
                        vin(245, "2026-03-08T10:00:00Z", &last, 3000.0),
                    ],
                ),
                (165, vec![vin(165, "2026-03-07T00:00:00Z", START, 9000.0)]),
            ]),
        };
        assert_eq!(got, want);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn reads_the_revision_input_with_the_paired_actuals(pool: PgPool) {
        let mut conn = pool.acquire().await.unwrap();
        seed(&mut conn).await;
        let got = read_study_input(&mut conn, WINDOW_DAYS, ts(NOW))
            .await
            .unwrap();
        assert_eq!(
            got.vintages_by_dataset.keys().copied().collect::<Vec<_>>(),
            [165, 245]
        );
        assert_eq!(got.vintages_by_dataset[&245].len(), 3);
        assert_eq!(
            got.actuals_by_dataset[&124],
            [ActualRecord {
                dataset_id: 124,
                start: ts(START),
                value: 8000.0
            }]
        );
        assert_eq!(got.actuals_by_dataset[&75].len(), 2);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn an_empty_database_gives_empty_datasets(pool: PgPool) {
        let mut conn = pool.acquire().await.unwrap();
        let got = read_backtest_data(&mut conn, WINDOW_DAYS, ts(NOW))
            .await
            .unwrap();
        assert_eq!(got.prices.len(), 0);
        assert_eq!(got.actuals.keys().copied().collect::<Vec<_>>(), [75, 124]);
        assert!(got.actuals.values().all(Vec::is_empty));
        assert!(got.vintages.values().all(Vec::is_empty));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_window_beyond_the_time_range_is_an_error(pool: PgPool) {
        let mut conn = pool.acquire().await.unwrap();
        let got = read_backtest_data(&mut conn, 1_000_000_000, ts(NOW)).await;
        assert!(matches!(got, Err(ToolError::OutOfRange(_))));
    }
}
