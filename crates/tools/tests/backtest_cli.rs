//! Behaviour of the backtest CLI layer: database assembly, the fixture
//! round trip, the old fixture shape, `--window`, and the thin-data guard.

#[cfg(test)]
mod backtest_cli {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use jiff::Timestamp;
    use spot_price_domain::price::eur_mwh_to_cents_kwh;
    use spot_price_tools::backtest::{BacktestOptions, compare_optimism, run_backtest};
    use spot_price_tools::backtest_cli::{
        DEFAULT_WINDOW_DAYS, MIN_SCOREABLE_ORIGINS, StoredPrice, assemble_backtest_data,
        is_thin_data, parse_window_days,
    };
    use spot_price_tools::data::{
        BacktestData, DatasetId, GridActual, Vintage, load_fixture, to_fixture_json,
    };

    const QUARTER_MS: i64 = 15 * 60 * 1000;
    const HOUR_MS: i64 = 60 * 60 * 1000;

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_millisecond(ms).unwrap()
    }

    fn start_ms() -> i64 {
        "2026-02-01T00:00:00Z"
            .parse::<Timestamp>()
            .unwrap()
            .as_millisecond()
    }

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spot-price-tools-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn price(area: &str, at: i64, eur_mwh: f64) -> StoredPrice {
        StoredPrice {
            area: area.to_owned(),
            delivery_start: ts(at),
            price_eur_mwh: eur_mwh,
        }
    }

    fn actual(at: i64, value: f64, dataset_id: DatasetId) -> GridActual {
        GridActual {
            dataset_id,
            start: ts(at),
            end: ts(at + QUARTER_MS),
            value,
        }
    }

    fn vintage(at: i64, value: f64, dataset_id: DatasetId, issued_ms: i64) -> Vintage {
        Vintage {
            dataset_id,
            issued_at: ts(issued_ms),
            start: ts(at),
            end: ts(at + QUARTER_MS),
            value,
        }
    }

    type Prices = Vec<(String, Vec<StoredPrice>)>;
    type Actuals = BTreeMap<DatasetId, Vec<GridActual>>;
    type Vintages = BTreeMap<DatasetId, Vec<Vintage>>;

    /// Database-shaped rows for 14 days with a 2-deep vintage ladder.
    #[expect(clippy::cast_precision_loss, reason = "q is a small index")]
    fn build_db_shaped() -> (Prices, Actuals, Vintages) {
        let mut by_area: Vec<(String, Vec<StoredPrice>)> = ["FI", "SE1", "SE3", "EE"]
            .iter()
            .map(|a| ((*a).to_owned(), Vec::new()))
            .collect();
        let (mut wind75, mut cons124, mut wind245, mut cons165) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for q in 0..14 * 96 {
            let at = start_ms() + q * QUARTER_MS;
            let consumption = 8000.0 + (q % 96) as f64 * 15.0;
            let wind = 2000.0 + ((q * 131) % 1800) as f64;
            let eur = 0.12 * (consumption - wind) + 200.0;
            for (rows, offset) in by_area.iter_mut().zip([0.0, -5.0, 3.0, 1.0]) {
                rows.1.push(price(&rows.0.clone(), at, eur + offset));
            }
            cons124.push(actual(at, consumption, 124));
            wind75.push(actual(at, wind, 75));
            cons165.push(vintage(at, consumption, 165, at - 2 * HOUR_MS));
            cons165.push(vintage(at, consumption + 300.0, 165, at - 80 * HOUR_MS));
            wind245.push(vintage(at, wind, 245, at - 2 * HOUR_MS));
            wind245.push(vintage(at, wind + 300.0, 245, at - 80 * HOUR_MS));
        }
        (
            by_area,
            BTreeMap::from([(75, wind75), (124, cons124)]),
            BTreeMap::from([(245, wind245), (165, cons165)]),
        )
    }

    fn assembled() -> BacktestData {
        let (prices, actuals, vintages) = build_db_shaped();
        assemble_backtest_data(&prices, actuals, vintages)
    }

    // Assembly parity with load_fixture

    #[test]
    fn assembled_data_equals_the_loaded_export() {
        let data = assembled();
        let file = temp_file("parity.json");
        std::fs::write(&file, to_fixture_json(&data)).unwrap();
        assert_eq!(load_fixture(&file).unwrap(), data);
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn converts_eur_mwh_to_cents_kwh_once() {
        let prices = vec![("FI".to_owned(), vec![price("FI", start_ms(), 312.5)])];
        let data = assemble_backtest_data(&prices, BTreeMap::new(), BTreeMap::new());
        assert_eq!(
            data.prices[0].spot_cents_kwh.to_bits(),
            eur_mwh_to_cents_kwh(312.5).to_bits()
        );
    }

    #[test]
    fn keeps_actuals_and_vintages_in_their_own_maps() {
        let (_, actuals, vintages) = build_db_shaped();
        let data = assemble_backtest_data(&[], actuals, vintages);
        assert_eq!(data.actuals.keys().copied().collect::<Vec<_>>(), [75, 124]);
        assert_eq!(
            data.vintages.keys().copied().collect::<Vec<_>>(),
            [165, 245]
        );
        assert!(data.vintages[&245][0].issued_at < data.vintages[&245][0].start);
    }

    // Export round trip and comparison alignment

    #[test]
    fn the_backtest_of_the_export_equals_the_backtest_of_the_data() {
        let data = assembled();
        let a = run_backtest(&data, &BacktestOptions::default()).unwrap();
        let file = temp_file("roundtrip.json");
        std::fs::write(&file, to_fixture_json(&data)).unwrap();
        let b = run_backtest(&load_fixture(&file).unwrap(), &BacktestOptions::default()).unwrap();
        assert_eq!(b.origins, a.origins);
        assert_eq!(b.pre_vintage_origins, a.pre_vintage_origins);
        assert_eq!(b.leak_free, a.leak_free);
        assert_eq!(b.model_mae.map(f64::to_bits), a.model_mae.map(f64::to_bits));
        assert_eq!(b.residuals, a.residuals);
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn compare_scores_the_same_origins_in_both_runs() {
        let cmp = compare_optimism(&assembled()).unwrap();
        assert!(cmp.scored_origins > 0);
        assert_eq!(cmp.leaked.scored_origins_ms.len(), cmp.scored_origins);
        assert_eq!(cmp.honest.scored_origins_ms, cmp.leaked.scored_origins_ms);
    }

    // Old fixture shape

    #[test]
    fn an_old_shape_fixture_loads_actuals_only() {
        let mut prices = Vec::new();
        let mut records = Vec::new();
        for q in 0..10 * 96 {
            let at = start_ms() + q * QUARTER_MS;
            let start = spot_price_domain::forecast::iso_millis(ts(at));
            let end = spot_price_domain::forecast::iso_millis(ts(at + QUARTER_MS));
            prices.push(format!(r#"{{"start":"{start}","spotCentsKwh":2}}"#));
            records.push(format!(
                r#"{{"startTime":"{start}","endTime":"{end}","value":3000}}"#
            ));
        }
        let records = records.join(",");
        let text = format!(
            r#"{{"prices":[{}],"fingrid":{{"245":[{records}],"75":[{records}],"165":[{records}],"124":[{records}]}}}}"#,
            prices.join(",")
        );
        let file = temp_file("old.json");
        std::fs::write(&file, text).unwrap();
        let data = load_fixture(&file).unwrap();
        assert_eq!(data.actuals.keys().copied().collect::<Vec<_>>(), [75, 124]);
        assert_eq!(data.vintages.len(), 0);
        assert!(data.prices.iter().all(|p| p.area == "FI"));
        let summary = run_backtest(&data, &BacktestOptions::default()).unwrap();
        assert_eq!(summary.origins, 0);
        assert!(summary.pre_vintage_origins > 0);
        std::fs::remove_file(file).unwrap();
    }

    // --window

    #[test]
    fn window_defaults_when_absent() {
        assert_eq!(parse_window_days(None).unwrap(), DEFAULT_WINDOW_DAYS);
    }

    #[test]
    fn window_accepts_a_positive_integer() {
        assert_eq!(parse_window_days(Some("120")).unwrap(), 120);
        // `Number(...)` reads these forms too.
        assert_eq!(parse_window_days(Some(" 0x10 ")).unwrap(), 16);
        assert_eq!(parse_window_days(Some("1e2")).unwrap(), 100);
    }

    #[test]
    fn window_rejects_other_values() {
        for raw in ["0", "-5", "30.5", "abc", "30days", "", "Infinity"] {
            assert!(parse_window_days(Some(raw)).is_err(), "{raw:?}");
        }
    }

    // Thin-data guard

    #[test]
    fn fewer_origins_than_the_minimum_are_thin() {
        assert!(is_thin_data(MIN_SCOREABLE_ORIGINS - 1));
        assert!(is_thin_data(0));
    }

    #[test]
    fn the_minimum_and_more_origins_are_a_verdict() {
        assert!(!is_thin_data(MIN_SCOREABLE_ORIGINS));
        assert!(!is_thin_data(MIN_SCOREABLE_ORIGINS + 50));
    }
}
