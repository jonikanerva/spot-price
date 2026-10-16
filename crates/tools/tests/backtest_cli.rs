//! Behaviour of the backtest CLI layer: database assembly, the fixture
//! round trip, the old fixture shape, `--window`, and the thin-data guard.

#[cfg(test)]
mod backtest_cli {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use jiff::Timestamp;
    use spot_price_domain::forecast::FLOOR_HISTORY_DAYS;
    use spot_price_domain::price::eur_mwh_to_cents_kwh;
    use spot_price_tools::backtest::{
        BacktestOptions, ForecastVariant, HORIZON_LABELS, HorizonPairedDelta, PairedDelta,
        compare_optimism, run_backtest,
    };
    use spot_price_tools::backtest_cli::{
        DEFAULT_WINDOW_DAYS, MAX_EXTENSION_WEEKS, MIN_SCOREABLE_ORIGINS, StoredPrice,
        assemble_backtest_data, is_thin_data, parse_variant, parse_window_days, print_paired_table,
    };
    use spot_price_tools::cli::{Context, Output, Stream, run};
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
    fn build_db_shaped() -> (Prices, Actuals, Vintages) {
        build_db_shaped_days(14)
    }

    /// Database-shaped rows for `days` days with a 2-deep vintage ladder.
    #[expect(clippy::cast_precision_loss, reason = "q is a small index")]
    fn build_db_shaped_days(days: i64) -> (Prices, Actuals, Vintages) {
        let mut by_area: Vec<(String, Vec<StoredPrice>)> = ["FI", "SE1", "SE3", "EE"]
            .iter()
            .map(|a| ((*a).to_owned(), Vec::new()))
            .collect();
        let (mut wind75, mut cons124, mut wind245, mut cons165) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for q in 0..days * 96 {
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

    // --ab

    fn run_cli(args: &[&str]) -> Output {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let env = |_: &str| None;
        let ctx = Context {
            env: &env,
            now: "2026-10-14T00:00:00Z".parse().unwrap(),
            workspace_root: &root,
        };
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        run(&args, &ctx)
    }

    /// A fixture with enough origins for a verdict.
    fn verdict_fixture(name: &str) -> PathBuf {
        let (prices, actuals, vintages) = build_db_shaped_days(24);
        let data = assemble_backtest_data(&prices, actuals, vintages);
        let file = temp_file(name);
        std::fs::write(&file, to_fixture_json(&data)).unwrap();
        file
    }

    #[test]
    fn ab_parses_overrides_onto_the_production_options() {
        let production = ForecastVariant::default();
        assert_eq!(
            parse_variant("ridge_lambda=2.5").unwrap(),
            ForecastVariant {
                ridge_lambda: 2.5,
                ..production
            }
        );
        assert_eq!(
            parse_variant("wind_extension_weeks=2, consumption_extension_weeks=0").unwrap(),
            ForecastVariant {
                wind_extension_weeks: 2,
                consumption_extension_weeks: 0,
                ..production
            }
        );
        assert_eq!(parse_variant("ridge_lambda=0").unwrap().ridge_lambda, 0.0);
        assert_eq!(
            parse_variant(&format!("wind_extension_weeks={MAX_EXTENSION_WEEKS}"))
                .unwrap()
                .wind_extension_weeks,
            MAX_EXTENSION_WEEKS
        );
    }

    #[test]
    fn ab_extension_weeks_stay_inside_the_served_history_window() {
        assert_eq!(MAX_EXTENSION_WEEKS, 4);
        assert!(i64::from(MAX_EXTENSION_WEEKS) * 7 <= FLOOR_HISTORY_DAYS);
        assert!(i64::from(MAX_EXTENSION_WEEKS + 1) * 7 > FLOOR_HISTORY_DAYS);
        let production = ForecastVariant::default();
        assert!(production.wind_extension_weeks <= MAX_EXTENSION_WEEKS);
        assert!(production.consumption_extension_weeks <= MAX_EXTENSION_WEEKS);
        let err = parse_variant("wind_extension_weeks=8").unwrap_err();
        assert!(err.to_string().contains("from 0 to 4"), "{err}");
    }

    #[test]
    fn ab_rejects_bad_keys_and_values() {
        for raw in [
            "",
            "ridge_lambda",
            "foo=1",
            "ridge_lambda=-1",
            "ridge_lambda=abc",
            "ridge_lambda=",
            "ridge_lambda=Infinity",
            "wind_extension_weeks=1.5",
            "wind_extension_weeks=-1",
            "consumption_extension_weeks=53",
            "wind_extension_weeks=5",
            "wind_extension_weeks=8",
            "ridge_lambda=1,ridge_lambda=2",
        ] {
            assert!(parse_variant(raw).is_err(), "{raw:?}");
        }
    }

    #[test]
    fn ab_against_the_production_options_prints_zero_deltas() {
        let file = verdict_fixture("ab-self.json");
        let got = run_cli(&[
            "backtest",
            "--data",
            file.to_str().unwrap(),
            "--ab",
            "ridge_lambda=1",
        ]);
        let stdout = got.text(Stream::Stdout);
        assert_eq!(got.code, 0, "{stdout}{}", got.text(Stream::Stderr));
        assert_eq!(got.text(Stream::Stderr), "");
        assert!(stdout.contains("Paired A/B backtest"));
        assert!(stdout.contains("origins paired:      true"));
        assert!(stdout.contains("delta = B − A"));
        for name in [
            "MAE (c/kWh)",
            "sMAPE",
            "rMAE last_week",
            "rMAE last_pub_day",
        ] {
            let row = stdout
                .lines()
                .find(|l| l.starts_with(&format!("  {name:<20}")))
                .unwrap_or_else(|| panic!("{name}"));
            assert!(row.trim_end().ends_with("0.000"), "{row}");
        }
        for label in HORIZON_LABELS {
            assert!(stdout.contains(&format!("    {label:<7}  ")), "{label}");
        }
        let (unpaired, paired) = stdout.split_once("  PAIRED MAE per horizon").unwrap();
        let delta_rows: Vec<&str> = unpaired
            .lines()
            .filter(|l| {
                ["Spearman", "P@N-cheap", "P@N-peak", "MAE (c/kWh, ref)"]
                    .iter()
                    .any(|m| l.contains(m))
                    && !l.contains("horizon")
            })
            .collect();
        assert_eq!(delta_rows.len(), 4 * HORIZON_LABELS.len());
        for row in delta_rows {
            assert!(row.trim_end().ends_with("0.000"), "{row}");
        }
        assert!(stdout.contains("REFERENCE ONLY"));
        // Paired rows: paired, meanDelta, W, L, T, excluded.
        let paired_rows: Vec<Vec<&str>> = paired
            .lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>())
            .filter(|cells| {
                cells
                    .iter()
                    .any(|c| ["MAE", "Spearman", "P@N-cheap", "P@N-peak"].contains(c))
                    && cells.last().is_some_and(|c| c.parse::<usize>().is_ok())
            })
            .map(|cells| cells.into_iter().rev().take(6).collect())
            .collect();
        assert_eq!(paired_rows.len(), 4 * HORIZON_LABELS.len());
        for row in paired_rows {
            let [excluded, ties, losses, wins, mean, n] = row[..] else {
                panic!("{row:?}");
            };
            assert_eq!((mean, wins, losses, excluded), ("0.000", "0", "0", "0"));
            assert_eq!(ties, n);
            assert!(n.parse::<usize>().unwrap() >= MIN_SCOREABLE_ORIGINS);
        }
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn the_paired_table_prints_each_metric_on_its_own_row() {
        // Metric m (0 MAE, 1 Spearman, 2 cheap, 3 peak) of horizon h is
        // value k = 4h + m + 1: k wins, k + 10 excluded, mean k / 1000.
        let delta = |k: usize| PairedDelta {
            paired: 50,
            mean_delta: Some(f64::from(u32::try_from(k).unwrap()) / 1000.0),
            wins: k,
            losses: 0,
            ties: 50 - k,
            excluded: k + 10,
        };
        let paired: [HorizonPairedDelta; 3] = std::array::from_fn(|h| HorizonPairedDelta {
            mae: delta(4 * h + 1),
            spearman: delta(4 * h + 2),
            precision_at_n_cheap: delta(4 * h + 3),
            precision_at_n_peak: delta(4 * h + 4),
        });
        let mut out = Output::default();
        print_paired_table(&paired, &mut out);
        let text = out.text(Stream::Stdout);
        let verdict = text
            .find("PAIRED MAE per horizon (the A/B verdict lines")
            .unwrap();
        let info = text
            .find("PAIRED rank metrics per horizon (information, not the verdict")
            .unwrap();
        assert!(verdict < info);
        assert!(text.contains("W = B more accurate (delta < 0)"));
        assert!(text.contains("W = B ranks better (delta > 0)"));
        assert!(text.contains("do not react to model options"));
        assert_eq!(text.matches("or the delta is not finite").count(), 2);
        let rows: Vec<Vec<&str>> = text
            .lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>())
            .filter(|cells| {
                cells
                    .iter()
                    .any(|c| ["MAE", "Spearman", "P@N-cheap", "P@N-peak"].contains(c))
                    && cells.last().is_some_and(|c| c.parse::<usize>().is_ok())
            })
            .collect();
        // The MAE table first (one row per horizon), then the rank table.
        let order: Vec<(usize, usize)> = (0..3)
            .map(|h| (h, 0))
            .chain((0..3).flat_map(|h| (1..4).map(move |m| (h, m))))
            .collect();
        assert_eq!(rows.len(), order.len());
        for (cells, &(h, m)) in rows.iter().zip(&order) {
            let k = 4 * h + m + 1;
            let first_of_horizon = m <= 1;
            let cells = if first_of_horizon {
                assert_eq!(cells[0], HORIZON_LABELS[h], "{cells:?}");
                &cells[1..]
            } else {
                &cells[..]
            };
            let want = [
                ["MAE", "Spearman", "P@N-cheap", "P@N-peak"][m].to_owned(),
                "50".to_owned(),
                format!("0.{k:03}"),
                k.to_string(),
                "0".to_owned(),
                (50 - k).to_string(),
                (k + 10).to_string(),
            ];
            assert_eq!(cells, want, "{cells:?}");
        }
    }

    #[test]
    fn ab_with_a_variant_prints_the_variant_and_a_verdict_exit_code() {
        let file = verdict_fixture("ab-ridge.json");
        let got = run_cli(&[
            "backtest",
            "--ab",
            "ridge_lambda=1000000",
            "--data",
            file.to_str().unwrap(),
        ]);
        let stdout = got.text(Stream::Stdout);
        assert_eq!(got.code, 0, "{stdout}{}", got.text(Stream::Stderr));
        assert!(stdout.contains(
            "arm B:               ridge_lambda=1000000 wind_extension_weeks=4 consumption_extension_weeks=1"
        ));
        assert!(stdout.contains("origins paired:      true"));
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn ab_marks_thin_data_as_not_a_verdict() {
        let data = assembled();
        let file = temp_file("ab-thin.json");
        std::fs::write(&file, to_fixture_json(&data)).unwrap();
        let got = run_cli(&[
            "backtest",
            "--data",
            file.to_str().unwrap(),
            "--ab",
            "ridge_lambda=2",
        ]);
        assert_eq!(got.code, 1);
        assert!(got.text(Stream::Stderr).contains("shared origins"));
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn ab_errors_are_reported_before_any_data_is_read() {
        let bad = run_cli(&["backtest", "--data", "missing.json", "--ab", "foo=1"]);
        assert_eq!(bad.code, 1);
        assert!(bad.text(Stream::Stderr).contains("--ab key must be"));

        let no_value = run_cli(&["backtest", "--data", "missing.json", "--ab"]);
        assert_eq!(no_value.code, 1);
        assert!(no_value.text(Stream::Stderr).contains("--ab needs"));

        let both = run_cli(&["backtest", "--db", "--compare", "--ab", "ridge_lambda=2"]);
        assert_eq!(both.code, 1);
        assert!(
            both.text(Stream::Stderr)
                .contains("cannot be used together")
        );
    }

    #[test]
    fn the_usage_documents_ab() {
        let usage = run_cli(&["backtest"]);
        assert_eq!(usage.code, 2);
        let text = usage.text(Stream::Stderr);
        assert!(text.contains("[--ab <key=value[,key=value]>]"));
        assert!(text.contains("ridge_lambda, wind_extension_weeks, consumption_extension_weeks"));
    }
}
