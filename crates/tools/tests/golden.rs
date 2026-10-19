//! The tools against `tests/golden/`. Most fixtures come from the TypeScript
//! tools and CLIs; the synthetic backtest sections come from the Rust tools
//! (`tests/golden/README.md`). Printed values and counts must be equal. A raw
//! float may differ within 1e-9 relative, because V8 and Rust use different
//! `asinh`, `sinh`, `sin`, and `cos`.

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "a golden value must be equal bit for bit unless a tolerance is counted"
)]
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test counts are small, and a rounded non-negative count fits usize"
)]
mod golden {
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    use jiff::Timestamp;
    use serde_json::Value;
    use spot_price_domain::forecast::{
        BandOffsets, DeriveOptions, HorizonResidual, QuarterStart, UtcHour, derive_band_offsets,
        iso_millis,
    };
    use spot_price_domain::js::{math_round, number_to_string};
    use spot_price_tools::backtest::{
        AsOfMode, BacktestOptions, BacktestSummary, DerivedBands, HORIZON_LABELS,
        OptimismComparison, PerBaseline, compare_optimism, derive_bands_from_backtest,
        forecast_at_issue_time, reconstruct_issue_time, run_backtest,
    };
    use spot_price_tools::backtest_cli::{StoredPrice, assemble_backtest_data, print_compare};
    use spot_price_tools::cli::{Context, Output, Stream, run};
    use spot_price_tools::data::{
        BacktestData, GridActual, PricePoint, Vintage, load_fixture, to_fixture_json,
    };
    use spot_price_tools::metrics::{
        RankMode, bias, mae, median, percentile, precision_at_n, rms, rmse, spearman,
        standard_deviation,
    };
    use spot_price_tools::revision::{
        ActualRecord, DatasetRevisionSummary, RevisionStudyInput, recommendation,
        run_revision_study,
    };
    use spot_price_tools::revision_cli;

    const QUARTER_MS: i64 = 15 * 60 * 1000;
    const HOUR_MS: i64 = 60 * 60 * 1000;

    // -----------------------------------------------------------------------
    // Loading
    // -----------------------------------------------------------------------

    fn golden_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
    }

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn read(name: &str) -> Value {
        let text = std::fs::read_to_string(golden_dir().join(name)).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn expected() -> &'static Value {
        static EXPECTED: OnceLock<Value> = OnceLock::new();
        EXPECTED.get_or_init(|| read("backtest_expected.json"))
    }

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_millisecond(ms).unwrap()
    }

    fn f(v: &Value) -> f64 {
        v.as_f64().unwrap()
    }

    fn opt(v: &Value) -> Option<f64> {
        v.as_f64()
    }

    fn int(v: &Value) -> i64 {
        v.as_i64().unwrap()
    }

    fn count(v: &Value) -> usize {
        usize::try_from(v.as_u64().unwrap()).unwrap()
    }

    /// The synthetic backtest data, built like `backtestDataOf` in the script.
    fn synthetic() -> &'static BacktestData {
        static DATA: OnceLock<BacktestData> = OnceLock::new();
        DATA.get_or_init(|| {
            let c = read("backtest_input.json");
            let start_ms = int(&c["startMs"]);
            let quarters = count(&c["quarters"]);
            let quarter_ms = |q: usize| start_ms + i64::try_from(q).unwrap() * QUARTER_MS;
            let mut data = BacktestData::default();
            for area in ["FI", "SE1", "SE3", "EE"] {
                for (q, value) in c["prices"][area].as_array().unwrap().iter().enumerate() {
                    if let Some(value) = value.as_f64() {
                        data.prices.push(PricePoint {
                            area: area.to_owned(),
                            start: ts(quarter_ms(q)),
                            spot_cents_kwh: value,
                        });
                    }
                }
            }
            for (id, values) in c["actuals"].as_object().unwrap() {
                let id: i32 = id.parse().unwrap();
                let rows = values
                    .as_array()
                    .unwrap()
                    .iter()
                    .enumerate()
                    .map(|(q, v)| GridActual {
                        dataset_id: id,
                        start: ts(quarter_ms(q)),
                        end: ts(quarter_ms(q) + QUARTER_MS),
                        value: f(v),
                    })
                    .collect();
                data.actuals.insert(id, rows);
            }
            for (id, ladder) in c["vintages"].as_object().unwrap() {
                let id: i32 = id.parse().unwrap();
                let ladder = ladder.as_array().unwrap();
                let mut rows = Vec::new();
                for q in 0..quarters {
                    for rung in ladder {
                        let lead_h = int(&rung["leadH"]);
                        rows.push(Vintage {
                            dataset_id: id,
                            issued_at: ts(quarter_ms(q) - lead_h * HOUR_MS),
                            start: ts(quarter_ms(q)),
                            end: ts(quarter_ms(q) + QUARTER_MS),
                            value: f(&rung["values"][q]),
                        });
                    }
                }
                data.vintages.insert(id, rows);
            }
            data
        })
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "spot-price-tools-golden-{}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn run_cli(args: &[&str]) -> Output {
        let root = workspace_root();
        let env = |_: &str| None;
        let ctx = Context {
            env: &env,
            now: "2026-10-14T00:00:00Z".parse().unwrap(),
            workspace_root: &root,
        };
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        run(&args, &ctx)
    }

    fn assert_same_cli(got: &Output, want: &Value, what: &str) {
        assert_eq!(
            got.text(Stream::Stdout),
            want["stdout"].as_str().unwrap(),
            "{what} stdout"
        );
        assert_eq!(
            got.text(Stream::Stderr),
            want["stderr"].as_str().unwrap(),
            "{what} stderr"
        );
        assert_eq!(i64::from(got.code), int(&want["status"]), "{what} status");
    }

    // -----------------------------------------------------------------------
    // Comparison with counted tolerance
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct Stats {
        compared: Cell<usize>,
        tolerant: Cell<usize>,
        coverage_changed: Cell<usize>,
    }

    impl Stats {
        fn close(&self, got: f64, want: f64, what: &str) {
            self.compared.set(self.compared.get() + 1);
            if got == want {
                return;
            }
            let scale = got.abs().max(want.abs()).max(1.0);
            assert!(
                (got - want).abs() <= 1e-9 * scale,
                "{what}: got {got}, want {want}"
            );
            self.tolerant.set(self.tolerant.get() + 1);
        }

        fn close_opt(&self, got: Option<f64>, want: &Value, what: &str) {
            match (got, opt(want)) {
                (Some(got), Some(want)) => self.close(got, want, what),
                (None, None) => self.compared.set(self.compared.get() + 1),
                (got, _) => panic!("{what}: got {got:?}, want {want}"),
            }
        }

        /// A band coverage over `n` residuals. It may differ only by
        /// residuals on a band edge, at most `edges` of them.
        fn coverage(&self, got: Option<f64>, want: &Value, n: usize, edges: usize, what: &str) {
            self.compared.set(self.compared.get() + 1);
            match (got, opt(want)) {
                (None, None) => {}
                (Some(got), Some(want)) => {
                    let changed = ((got - want).abs() * n as f64).round() as usize;
                    assert!(
                        changed <= edges,
                        "{what}: {changed} changed, {edges} on an edge"
                    );
                    self.coverage_changed
                        .set(self.coverage_changed.get() + changed);
                }
                (got, _) => panic!("{what}: got {got:?}, want {want}"),
            }
        }

        fn report(&self, what: &str) {
            println!(
                "{what}: {} values compared, {} within tolerance and not bit-equal, \
                 {} covered residuals differ on band edges",
                self.compared.get(),
                self.tolerant.get(),
                self.coverage_changed.get()
            );
        }
    }

    // -----------------------------------------------------------------------
    // Backtest
    // -----------------------------------------------------------------------

    /// Residuals on a band edge: of all residuals, and per horizon.
    #[derive(Debug, Default, Clone, Copy)]
    struct Edges {
        overall: usize,
        by_horizon: [usize; 3],
    }

    /// The residuals of the scored origins split by horizon, rebuilt with
    /// the public engine functions, and their edge counts.
    fn edges_of(data: &BacktestData, summary: &BacktestSummary, mode: AsOfMode) -> Edges {
        let realized: BTreeMap<QuarterStart, f64> = data
            .prices
            .iter()
            .filter(|p| p.area == "FI")
            .map(|p| (QuarterStart::floor(p.start), p.spot_cents_kwh))
            .collect();
        let mut by_horizon: [Vec<HorizonResidual>; 3] = Default::default();
        for &t_ms in &summary.scored_origins_ms {
            let inputs = reconstruct_issue_time(data, t_ms, mode).unwrap();
            let fc = forecast_at_issue_time(&inputs).unwrap();
            for &(key, pred) in &fc.predicted {
                let Some(&actual) = realized.get(&key) else {
                    continue;
                };
                let h = (key.as_millisecond() - fc.series_start_ms).div_euclid(24 * HOUR_MS);
                if let Some(bucket) = usize::try_from(h).ok().and_then(|h| by_horizon.get_mut(h)) {
                    bucket.push(HorizonResidual {
                        utc_hour: UtcHour::of(key),
                        predicted_raw: pred,
                        actual_raw: actual,
                    });
                }
            }
        }
        for (residuals, metrics) in by_horizon.iter().zip(&summary.by_horizon) {
            assert_eq!(residuals.len(), metrics.pairs);
        }
        Edges {
            overall: edge_residuals(&summary.residuals),
            by_horizon: by_horizon.map(|r| edge_residuals(&r)),
        }
    }

    fn synthetic_summary() -> &'static (BacktestSummary, Edges) {
        static SUMMARY: OnceLock<(BacktestSummary, Edges)> = OnceLock::new();
        SUMMARY.get_or_init(|| {
            let summary = run_backtest(synthetic(), &BacktestOptions::default()).unwrap();
            let edges = edges_of(synthetic(), &summary, AsOfMode::IssueTime);
            (summary, edges)
        })
    }

    fn check_summary(got: &BacktestSummary, want: &Value, edges: Edges, stats: &Stats, what: &str) {
        assert_eq!(got.origins, count(&want["origins"]), "{what} origins");
        assert_eq!(
            got.fallback_origins,
            count(&want["fallbackOrigins"]),
            "{what}"
        );
        assert_eq!(
            got.pre_vintage_origins,
            count(&want["preVintageOrigins"]),
            "{what}"
        );
        // The TypeScript sections have no such key; it reads as 0.
        let no_grid_data = want.get("noGridDataOrigins").map_or(0, count);
        assert_eq!(got.no_grid_data_origins, no_grid_data, "{what}");
        let iso = |ms: Option<i64>| ms.map(|ms| iso_millis(ts(ms)));
        assert_eq!(
            iso(got.scored_window_start_ms).as_deref(),
            want["scoredWindowStart"].as_str()
        );
        assert_eq!(
            iso(got.scored_window_end_ms).as_deref(),
            want["scoredWindowEnd"].as_str()
        );
        assert_eq!(
            got.scored_window_span_days,
            f(&want["scoredWindowSpanDays"])
        );
        let origins: Vec<i64> = want["scoredOriginsMs"]
            .as_array()
            .unwrap()
            .iter()
            .map(int)
            .collect();
        assert_eq!(
            got.scored_origins_ms.iter().copied().collect::<Vec<_>>(),
            origins
        );
        assert_eq!(
            got.leak_free,
            want["leakFree"].as_bool().unwrap(),
            "{what} leak_free"
        );
        stats.close_opt(got.model_mae, &want["modelMae"], "model mae");
        stats.close_opt(got.model_smape, &want["modelSmape"], "model smape");
        stats.close_opt(
            got.baseline_mae.last_week,
            &want["baselineMae"]["last_week"],
            "b lw",
        );
        stats.close_opt(
            got.baseline_mae.last_published_day,
            &want["baselineMae"]["last_published_day"],
            "b lpd",
        );
        stats.close_opt(got.r_mae.last_week, &want["rMae"]["last_week"], "rmae lw");
        stats.close_opt(
            got.r_mae.last_published_day,
            &want["rMae"]["last_published_day"],
            "rmae lpd",
        );
        for (h, (label, m)) in HORIZON_LABELS.iter().zip(&got.by_horizon).enumerate() {
            let w = &want["byHorizon"][label];
            assert_eq!(m.pairs, count(&w["pairs"]), "{what} {label} pairs");
            stats.close_opt(m.mae, &w["mae"], "h mae");
            stats.close_opt(m.smape, &w["smape"], "h smape");
            stats.close_opt(m.spearman, &w["spearman"], "h spearman");
            stats.close_opt(m.precision_at_n_cheap, &w["precisionAtNCheap"], "h cheap");
            stats.close_opt(m.precision_at_n_peak, &w["precisionAtNPeak"], "h peak");
            let edges = edges.by_horizon[h];
            stats.coverage(
                m.band_coverage,
                &w["bandCoverage"],
                m.pairs,
                edges,
                "h coverage",
            );
        }
        assert_eq!(
            got.residuals.len(),
            count(&want["residualCount"]),
            "{what} residuals"
        );
        if let Some(hours) = want["residualHours"].as_array() {
            let got_hours: Vec<u64> = got
                .residuals
                .iter()
                .map(|r| u64::from(r.utc_hour.get()))
                .collect();
            let want_hours: Vec<u64> = hours.iter().map(|h| h.as_u64().unwrap()).collect();
            assert_eq!(got_hours, want_hours, "{what} residual hours");
            let want_preds = want["residualPredMilli"].as_array().unwrap();
            for (r, want) in got.residuals.iter().zip(want_preds) {
                stats.close(
                    math_round(r.predicted_raw * 1000.0),
                    f(want),
                    "residual prediction",
                );
            }
        }
    }

    #[test]
    fn synthetic_backtest_matches_the_golden_file() {
        let stats = Stats::default();
        let (summary, edges) = synthetic_summary();
        check_summary(
            summary,
            &expected()["synthetic"]["summary"],
            *edges,
            &stats,
            "synthetic",
        );
        println!("edge residuals: {edges:?}");
        assert_eq!(summary.origins, 93);
        stats.report("synthetic backtest");
        // Predictions are rounded to 0.001, so they must be bit-equal.
        assert_eq!(
            stats.tolerant.get(),
            0,
            "a rounded prediction or a metric differs"
        );
    }

    #[test]
    fn committed_fixture_backtest_matches_typescript() {
        let stats = Stats::default();
        let data = load_fixture(&workspace_root().join("crates/tools/data/fixture.json")).unwrap();
        let summary = run_backtest(&data, &BacktestOptions::default()).unwrap();
        check_summary(
            &summary,
            &expected()["committedFixture"]["summary"],
            Edges::default(),
            &stats,
            "fixture",
        );
        assert_eq!(summary.origins, 0);
    }

    #[test]
    fn optimism_comparison_matches_the_golden_file() {
        let stats = Stats::default();
        let want = &expected()["synthetic"]["compare"];
        let cmp = compare_optimism(synthetic()).unwrap();
        assert_eq!(cmp.scored_origins, count(&want["scoredOrigins"]));
        assert_eq!(
            cmp.honest.leak_free,
            want["honestLeakFree"].as_bool().unwrap()
        );
        let leaked_edges = edges_of(synthetic(), &cmp.leaked, AsOfMode::Latest);
        let honest_edges = synthetic_summary().1;
        check_summary(&cmp.leaked, &want["leaked"], leaked_edges, &stats, "leaked");
        stats.close_opt(cmp.delta_mae, &want["deltaMae"], "delta mae");
        stats.close_opt(cmp.delta_smape, &want["deltaSmape"], "delta smape");
        // A coverage delta can move by the edge residuals of both runs.
        let delta_bound = |l: usize, h: usize, n: usize| (l + h) as f64 / n as f64 + 1e-12;
        let n = cmp.honest.residuals.len();
        let got = cmp.delta_band_coverage.unwrap();
        let bound = delta_bound(leaked_edges.overall, honest_edges.overall, n);
        assert!(
            (got - f(&want["deltaBandCoverage"])).abs() <= bound,
            "delta coverage"
        );
        for (h, (label, d)) in HORIZON_LABELS.iter().zip(&cmp.delta_by_horizon).enumerate() {
            let w = &want["deltaByHorizon"][label];
            stats.close_opt(d.delta_mae, &w["deltaMae"], "h delta mae");
            stats.close_opt(d.delta_smape, &w["deltaSmape"], "h delta smape");
            let pairs = cmp.honest.by_horizon[h].pairs;
            let bound = delta_bound(
                leaked_edges.by_horizon[h],
                honest_edges.by_horizon[h],
                pairs,
            );
            let got = d.delta_band_coverage.unwrap();
            assert!(
                (got - f(&w["deltaBandCoverage"])).abs() <= bound,
                "{label} delta coverage"
            );
        }
        let ladder = want["ladderDiagnostic"].as_array().unwrap();
        assert_eq!(cmp.ladder_diagnostic.len(), ladder.len());
        for ((id, ld), w) in cmp.ladder_diagnostic.iter().zip(ladder) {
            assert_eq!(id.to_string(), w[0].as_str().unwrap(), "ladder order");
            stats.close_opt(ld.median_depth, &w[1]["medianDepth"], "median depth");
            stats.close_opt(ld.expected_depth, &w[1]["expectedDepth"], "expected depth");
            stats.close_opt(
                ld.sparse_target_fraction,
                &w[1]["sparseTargetFraction"],
                "sparse",
            );
        }
        stats.report("optimism comparison");
    }

    #[test]
    fn backtest_cli_output_matches_the_golden_file() {
        let dir = temp_dir("backtest-cli");
        let file = dir.join("synthetic.json");
        std::fs::write(&file, to_fixture_json(synthetic())).unwrap();
        let got = run_cli(&["backtest", "--data", file.to_str().unwrap()]);
        let want = &expected()["synthetic"]["cli"];
        let (summary, edges) = synthetic_summary();
        let changed = assert_same_report(
            &got.text(Stream::Stdout),
            want["stdout"].as_str().unwrap(),
            summary,
            *edges,
        );
        println!("backtest report: {changed} coverage lines differ within the edge bound");
        assert_eq!(got.text(Stream::Stderr), want["stderr"].as_str().unwrap());
        assert_eq!(i64::from(got.code), int(&want["status"]));

        let fixture = workspace_root().join("crates/tools/data/fixture.json");
        let got = run_cli(&["backtest", "--data", fixture.to_str().unwrap()]);
        let typescript = &expected()["committedFixture"]["cli"];
        // The TypeScript report has no clamp count line and no no-grid-data
        // line; the Rust report adds exactly these lines after the fallback
        // line.
        let fallback = "\n  fallback origins:    0\n";
        let ts_stdout = typescript["stdout"].as_str().unwrap();
        assert_eq!(ts_stdout.matches(fallback).count(), 1);
        let want_stdout = ts_stdout.replacen(
            fallback,
            &format!("{fallback}  sanity-clamped quarters: 0\n  no-grid-data origins: 0\n"),
            1,
        );
        assert_eq!(
            got.text(Stream::Stdout),
            want_stdout,
            "committed fixture stdout"
        );
        assert_eq!(
            got.text(Stream::Stderr),
            typescript["stderr"].as_str().unwrap(),
            "committed fixture stderr"
        );
        assert_eq!(
            i64::from(got.code),
            int(&typescript["status"]),
            "committed fixture status"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn backtest_cli_exit_codes_match_typescript() {
        let usage = run_cli(&["backtest"]);
        assert_eq!(i64::from(usage.code), int(&expected()["usage"]["status"]));
        assert!(usage.text(Stream::Stderr).starts_with("usage: "));

        let bad = run_cli(&["backtest", "--db", "--window", "abc"]);
        assert_eq!(i64::from(bad.code), int(&expected()["badWindow"]["status"]));
        let message = r#"--window must be a positive integer number of days, got "abc""#;
        assert!(
            expected()["badWindow"]["stderr"]
                .as_str()
                .unwrap()
                .contains(message)
        );
        assert!(bad.text(Stream::Stderr).contains(message));

        let no_url = run_cli(&["backtest", "--db"]);
        assert_eq!(no_url.code, 1);
        assert!(no_url.text(Stream::Stderr).contains("DATABASE_PUBLIC_URL"));
    }

    #[test]
    fn fixture_export_text_matches_json_stringify() {
        let want = &expected()["fixtureJson"];
        let input = &want["input"];
        let mut data = BacktestData::default();
        for p in input["prices"].as_array().unwrap() {
            data.prices.push(PricePoint {
                area: p["area"].as_str().unwrap().to_owned(),
                start: p["start"].as_str().unwrap().parse().unwrap(),
                spot_cents_kwh: f(&p["spotCentsKwh"]),
            });
        }
        for (id, rows) in input["fingridActualsByDataset"].as_object().unwrap() {
            let rows = rows
                .as_array()
                .unwrap()
                .iter()
                .map(|r| GridActual {
                    dataset_id: id.parse().unwrap(),
                    start: r["startTime"].as_str().unwrap().parse().unwrap(),
                    end: r["endTime"].as_str().unwrap().parse().unwrap(),
                    value: f(&r["value"]),
                })
                .collect();
            data.actuals.insert(id.parse().unwrap(), rows);
        }
        for (id, rows) in input["fingridForecastVintagesByDataset"]
            .as_object()
            .unwrap()
        {
            let rows = rows
                .as_array()
                .unwrap()
                .iter()
                .map(|r| Vintage {
                    dataset_id: id.parse().unwrap(),
                    issued_at: r["issuedAt"].as_str().unwrap().parse().unwrap(),
                    start: r["startTime"].as_str().unwrap().parse().unwrap(),
                    end: r["endTime"].as_str().unwrap().parse().unwrap(),
                    value: f(&r["value"]),
                })
                .collect();
            data.vintages.insert(id.parse().unwrap(), rows);
        }
        assert_eq!(to_fixture_json(&data), want["text"].as_str().unwrap());
    }

    /// Compares two texts line by line. A line may differ only in its last
    /// field, a band coverage; `bound` gives the allowed difference for a
    /// line of `want`, or `None` when the line must be equal. Returns the
    /// differing lines.
    fn assert_same_lines(
        got: &str,
        want: &str,
        bound: impl Fn(&str, &str) -> Option<f64>,
    ) -> usize {
        assert_eq!(got.lines().count(), want.lines().count(), "line count");
        let mut changed = 0;
        for (g, w) in got.lines().zip(want.lines()) {
            if g == w {
                continue;
            }
            let (g_tokens, w_tokens): (Vec<&str>, Vec<&str>) = (
                g.split_whitespace().collect(),
                w.split_whitespace().collect(),
            );
            assert_eq!(g_tokens.len(), w_tokens.len(), "{g} / {w}");
            let last = g_tokens.len() - 1;
            assert_eq!(g_tokens[..last], w_tokens[..last], "{g} / {w}");
            let bound = bound(w, w_tokens[0]).unwrap_or_else(|| panic!("differs: {g} / {w}"));
            let (g_value, w_value): (f64, f64) = (
                g_tokens[last].parse().unwrap(),
                w_tokens[last].parse().unwrap(),
            );
            assert!((g_value - w_value).abs() <= bound, "{g} / {w}");
            changed += 1;
        }
        changed
    }

    /// The allowed difference of a printed coverage over `n` residuals with
    /// `edges` edge residuals, including the 0.001 print rounding.
    fn print_bound(edges: usize, n: usize) -> f64 {
        edges as f64 / n as f64 + 0.001 + 1e-12
    }

    /// [`assert_same_lines`] for the backtest report: the overall and the
    /// per-horizon band coverages may differ.
    fn assert_same_report(got: &str, want: &str, summary: &BacktestSummary, edges: Edges) -> usize {
        assert_same_lines(got, want, |line, first| {
            if line.starts_with("  band residuals:") {
                return Some(print_bound(edges.overall, summary.residuals.len()));
            }
            let h = HORIZON_LABELS.iter().position(|l| *l == first)?;
            Some(print_bound(
                edges.by_horizon[h],
                summary.by_horizon[h].pairs,
            ))
        })
    }

    /// The rows that the fake database of the golden script serves, read
    /// back like the CLI does: prices as EUR/MWh `spot × 10`, in area and
    /// start order, and vintages in target and issue order.
    fn db_shaped(data: &BacktestData) -> BacktestData {
        let mut prices: Vec<&PricePoint> = data.prices.iter().collect();
        prices.sort_by(|a, b| a.area.cmp(&b.area).then(a.start.cmp(&b.start)));
        let mut grouped: Vec<(String, Vec<StoredPrice>)> = Vec::new();
        for p in prices {
            let row = StoredPrice {
                area: p.area.clone(),
                delivery_start: p.start,
                price_eur_mwh: p.spot_cents_kwh * 10.0,
            };
            match grouped.last_mut() {
                Some((area, rows)) if *area == p.area => rows.push(row),
                _ => grouped.push((p.area.clone(), vec![row])),
            }
        }
        let mut vintages = data.vintages.clone();
        for rows in vintages.values_mut() {
            rows.sort_by(|a, b| a.start.cmp(&b.start).then(a.issued_at.cmp(&b.issued_at)));
        }
        assemble_backtest_data(&grouped, data.actuals.clone(), vintages)
    }

    #[test]
    fn compare_report_matches_the_golden_file() {
        let data = db_shaped(synthetic());
        // The EUR/MWh round trip gives back every spot price exactly.
        let spot = |d: &BacktestData| {
            let mut v: Vec<(String, i64, u64)> = d
                .prices
                .iter()
                .map(|p| {
                    (
                        p.area.clone(),
                        p.start.as_millisecond(),
                        p.spot_cents_kwh.to_bits(),
                    )
                })
                .collect();
            v.sort();
            v
        };
        assert_eq!(spot(&data), spot(synthetic()));

        let mut out = Output::default();
        let ok = print_compare(&data, &mut out).unwrap();
        let want = &expected()["dbCompare"];
        assert_eq!(i64::from(i32::from(!ok)), int(&want["status"]));
        assert_eq!(out.text(Stream::Stderr), want["stderr"].as_str().unwrap());

        let cmp = compare_optimism(&data).unwrap();
        let honest = edges_of(&data, &cmp.honest, AsOfMode::IssueTime);
        let leaked = edges_of(&data, &cmp.leaked, AsOfMode::Latest);
        let changed = assert_same_lines(
            &out.text(Stream::Stdout),
            want["stdout"].as_str().unwrap(),
            |line, first| {
                if line.starts_with("  band coverage") {
                    let n = cmp.honest.residuals.len();
                    return Some(print_bound(leaked.overall + honest.overall, n));
                }
                let h = HORIZON_LABELS.iter().position(|l| *l == first)?;
                let n = cmp.honest.by_horizon[h].pairs;
                Some(print_bound(leaked.by_horizon[h] + honest.by_horizon[h], n))
            },
        );
        println!("compare report: {changed} coverage lines differ within the edge bound");
    }

    // -----------------------------------------------------------------------
    // Band artifact
    // -----------------------------------------------------------------------

    /// Residuals whose actual price lies within 1e-9 of a band edge. Only
    /// these can change sides because of a different `asinh` or `sinh`.
    fn edge_residuals(residuals: &[HorizonResidual]) -> usize {
        let derived = derive_band_offsets(residuals, &DeriveOptions::default());
        residuals
            .iter()
            .filter(|r| {
                let offsets = derived.offsets_by_hour[usize::from(r.utc_hour.get())]
                    .or(derived.global_offsets);
                offsets.is_some_and(|o| {
                    let point = r.predicted_raw.asinh();
                    let near = |edge: f64| {
                        (r.actual_raw - edge).abs() <= 1e-9 * r.actual_raw.abs().max(1.0)
                    };
                    near((point + o.low_offset).sinh()) || near((point + o.high_offset).sinh())
                })
            })
            .count()
    }

    /// The numbers of a rendered Rust artifact, in source order, from the
    /// `CALIBRATED_BANDS` item on.
    fn artifact_numbers(text: &str) -> Vec<f64> {
        let body = &text[text.find("pub const CALIBRATED_BANDS").unwrap()..];
        let body = &body[..body.find("generated_at").unwrap()];
        body.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e'))
            .filter_map(|token| token.parse::<f64>().ok())
            .collect()
    }

    fn want_artifact_numbers(want: &Value) -> Vec<f64> {
        let mut numbers = vec![f(&want["nominalCoverage"])];
        numbers.extend(opt(&want["observedCoverage"]));
        let mut hours: BTreeMap<u64, (f64, f64)> = BTreeMap::new();
        for entry in want["offsetsByHour"].as_array().unwrap() {
            let o = &entry[1];
            hours.insert(
                entry[0].as_u64().unwrap(),
                (f(&o["lowOffset"]), f(&o["highOffset"])),
            );
        }
        for (low, high) in hours.values() {
            numbers.extend([*low, *high]);
        }
        if want["globalOffsets"].is_object() {
            numbers.extend([
                f(&want["globalOffsets"]["lowOffset"]),
                f(&want["globalOffsets"]["highOffset"]),
            ]);
        }
        numbers
    }

    #[test]
    fn regenerate_bands_matches_the_golden_file() {
        let want = &expected()["synthetic"]["bands"];
        let generated_at = expected()["generatedAt"].as_str().unwrap();
        let derived = derive_bands_from_backtest(synthetic(), generated_at.to_owned()).unwrap();
        assert!(derived.bands.calibrated);
        assert_eq!(
            derived.bands.calibrated,
            want["calibrated"].as_bool().unwrap()
        );
        assert_eq!(derived.bands.generated_at, generated_at);
        let residuals = &derived.summary.residuals;
        assert_eq!(residuals.len(), count(&want["residualCount"]));

        // The coverage can differ only by residuals on a band edge.
        let edges = edge_residuals(residuals);
        let n = residuals.len() as f64;
        let got_covered = derived.observed_coverage.unwrap() * n;
        let want_covered = f(&want["measuredCoverage"]) * n;
        let changed = (got_covered - want_covered).abs().round();
        println!(
            "band coverage: {edges} residuals on a band edge, {changed} covered counts differ"
        );
        assert!(
            changed <= edges as f64,
            "coverage differs beyond the edge residuals"
        );

        // The end-to-end run writes the artifact file and formats it.
        let dir = temp_dir("bands");
        let fixture = dir.join("synthetic.json");
        std::fs::write(&fixture, to_fixture_json(synthetic())).unwrap();
        let out = dir.join("conformal_artifact.rs");
        let got = run_cli(&[
            "regenerate-bands",
            "--data",
            fixture.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--generated-at",
            generated_at,
        ]);
        assert_eq!(got.code, 0, "{}", got.text(Stream::Stderr));
        let text = std::fs::read_to_string(&out).unwrap();
        let stats = Stats::default();
        let got_numbers = artifact_numbers(&text);
        let want_numbers = want_artifact_numbers(want);
        assert_eq!(got_numbers.len(), want_numbers.len(), "artifact numbers");
        // Index 1 is the observed coverage; the rest are offsets.
        stats.coverage(
            got_numbers.get(1).copied(),
            &want["observedCoverage"],
            residuals.len(),
            edges,
            "artifact coverage",
        );
        for (i, (got, want)) in got_numbers.iter().zip(&want_numbers).enumerate() {
            if i != 1 {
                stats.close(*got, *want, "artifact number");
            }
        }
        stats.report("band artifact");
        assert!(text.contains(&format!("Cow::Borrowed({generated_at:?})")));
        let p = &want["provenance"];
        assert!(text.contains(&format!(
            "//!   scored window: {} … {}",
            p["scoredWindowStart"].as_str().unwrap(),
            p["scoredWindowEnd"].as_str().unwrap()
        )));
        assert!(text.contains(&format!(
            "//!   scored origins: {}   span: {:.1} days\n",
            count(&p["scoredOrigins"]),
            f(&p["scoredWindowSpanDays"])
        )));
        assert!(!text.contains("SUMMER-ONLY"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn regenerate_bands_on_the_committed_fixture_ships_dark() {
        let want = &expected()["committedFixture"]["bands"];
        assert!(!want["calibrated"].as_bool().unwrap());
        let dir = temp_dir("bands-fixture");
        let out = dir.join("conformal_artifact.rs");
        let got = run_cli(&[
            "regenerate-bands",
            "--out",
            out.to_str().unwrap(),
            "--generated-at",
            "2026-10-14T00:00:00Z",
        ]);
        assert_eq!(got.code, 0, "{}", got.text(Stream::Stderr));
        let text = std::fs::read_to_string(&out).unwrap();
        assert_eq!(artifact_numbers(&text), want_artifact_numbers(want));
        assert!(text.contains("//!   scored window: n/a … n/a\n"));
        assert!(text.contains("//!   scored origins: 0   span: 0.0 days\n"));
        assert!(text.contains("//!   measured out-of-sample coverage: n/a"));
        assert!(text.contains("generated_at: Cow::Borrowed(\"\"),"));
        let stdout = got.text(Stream::Stdout);
        assert!(
            stdout.contains("  calibrated:          false (measured coverage n/a vs nominal 0.8)")
        );
        assert!(stdout.contains("  → shipping DARK"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    // -----------------------------------------------------------------------
    // Regeneration of the Rust-produced backtest sections
    // -----------------------------------------------------------------------

    /// A JSON value with members in output order and JavaScript number text.
    enum Json {
        Null,
        Bool(bool),
        Number(f64),
        Text(String),
        Array(Vec<Json>),
        Object(Vec<(&'static str, Json)>),
    }

    impl Json {
        fn write(&self, out: &mut String) {
            match self {
                Self::Null => out.push_str("null"),
                Self::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
                Self::Number(x) => out.push_str(&number_to_string(*x)),
                Self::Text(t) => out.push_str(&serde_json::to_string(t).unwrap()),
                Self::Array(items) => {
                    out.push('[');
                    for (i, item) in items.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        item.write(out);
                    }
                    out.push(']');
                }
                Self::Object(members) => {
                    out.push('{');
                    for (i, (key, value)) in members.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        out.push_str(&serde_json::to_string(key).unwrap());
                        out.push(':');
                        value.write(out);
                    }
                    out.push('}');
                }
            }
        }
    }

    fn num(x: Option<f64>) -> Json {
        x.map_or(Json::Null, Json::Number)
    }

    fn int_json(n: usize) -> Json {
        Json::Number(n as f64)
    }

    fn iso_json(ms: Option<i64>) -> Json {
        ms.map_or(Json::Null, |ms| Json::Text(iso_millis(ts(ms))))
    }

    fn summary_json(s: &BacktestSummary, with_residuals: bool) -> Json {
        let per_baseline = |b: &PerBaseline<Option<f64>>| {
            Json::Object(vec![
                ("last_week", num(b.last_week)),
                ("last_published_day", num(b.last_published_day)),
            ])
        };
        let by_horizon = HORIZON_LABELS
            .iter()
            .zip(&s.by_horizon)
            .map(|(label, m)| {
                let metrics = Json::Object(vec![
                    ("mae", num(m.mae)),
                    ("smape", num(m.smape)),
                    ("spearman", num(m.spearman)),
                    ("precisionAtNCheap", num(m.precision_at_n_cheap)),
                    ("precisionAtNPeak", num(m.precision_at_n_peak)),
                    ("bandCoverage", num(m.band_coverage)),
                    ("pairs", int_json(m.pairs)),
                ]);
                (*label, metrics)
            })
            .collect();
        let residuals = |part: fn(&HorizonResidual) -> f64| {
            if with_residuals {
                Json::Array(s.residuals.iter().map(|r| Json::Number(part(r))).collect())
            } else {
                Json::Null
            }
        };
        Json::Object(vec![
            ("origins", int_json(s.origins)),
            ("fallbackOrigins", int_json(s.fallback_origins)),
            ("preVintageOrigins", int_json(s.pre_vintage_origins)),
            ("noGridDataOrigins", int_json(s.no_grid_data_origins)),
            ("scoredWindowStart", iso_json(s.scored_window_start_ms)),
            ("scoredWindowEnd", iso_json(s.scored_window_end_ms)),
            (
                "scoredWindowSpanDays",
                Json::Number(s.scored_window_span_days),
            ),
            (
                "scoredOriginsMs",
                Json::Array(
                    s.scored_origins_ms
                        .iter()
                        .map(|ms| Json::Number(*ms as f64))
                        .collect(),
                ),
            ),
            ("modelMae", num(s.model_mae)),
            ("modelSmape", num(s.model_smape)),
            ("baselineMae", per_baseline(&s.baseline_mae)),
            ("rMae", per_baseline(&s.r_mae)),
            ("leakFree", Json::Bool(s.leak_free)),
            ("byHorizon", Json::Object(by_horizon)),
            ("residualCount", int_json(s.residuals.len())),
            ("residualHours", residuals(|r| f64::from(r.utc_hour.get()))),
            (
                "residualPredMilli",
                residuals(|r| math_round(r.predicted_raw * 1000.0)),
            ),
        ])
    }

    fn compare_json(cmp: &OptimismComparison) -> Json {
        let by_horizon = HORIZON_LABELS
            .iter()
            .zip(&cmp.delta_by_horizon)
            .map(|(label, d)| {
                let deltas = Json::Object(vec![
                    ("deltaMae", num(d.delta_mae)),
                    ("deltaSmape", num(d.delta_smape)),
                    ("deltaBandCoverage", num(d.delta_band_coverage)),
                ]);
                (*label, deltas)
            })
            .collect();
        let ladder = cmp
            .ladder_diagnostic
            .iter()
            .map(|(id, ld)| {
                Json::Array(vec![
                    Json::Text(id.to_string()),
                    Json::Object(vec![
                        ("medianDepth", num(ld.median_depth)),
                        ("expectedDepth", num(ld.expected_depth)),
                        ("sparseTargetFraction", num(ld.sparse_target_fraction)),
                    ]),
                ])
            })
            .collect();
        Json::Object(vec![
            ("scoredOrigins", int_json(cmp.scored_origins)),
            ("deltaMae", num(cmp.delta_mae)),
            ("deltaSmape", num(cmp.delta_smape)),
            ("deltaBandCoverage", num(cmp.delta_band_coverage)),
            ("deltaByHorizon", Json::Object(by_horizon)),
            ("ladderDiagnostic", Json::Array(ladder)),
            ("leaked", summary_json(&cmp.leaked, false)),
            ("honestLeakFree", Json::Bool(cmp.honest.leak_free)),
        ])
    }

    fn bands_json(derived: &DerivedBands) -> Json {
        let offsets = |o: &BandOffsets| {
            Json::Object(vec![
                ("lowOffset", Json::Number(o.low_offset)),
                ("highOffset", Json::Number(o.high_offset)),
            ])
        };
        let bands = &derived.bands;
        let summary = &derived.summary;
        let by_hour = bands
            .offsets_by_hour
            .iter()
            .enumerate()
            .filter_map(|(hour, o)| {
                o.as_ref()
                    .map(|o| Json::Array(vec![int_json(hour), offsets(o)]))
            })
            .collect();
        Json::Object(vec![
            ("calibrated", Json::Bool(bands.calibrated)),
            ("nominalCoverage", Json::Number(bands.nominal_coverage)),
            ("observedCoverage", num(bands.observed_coverage)),
            ("measuredCoverage", num(derived.observed_coverage)),
            ("offsetsByHour", Json::Array(by_hour)),
            (
                "globalOffsets",
                bands.global_offsets.as_ref().map_or(Json::Null, offsets),
            ),
            ("generatedAt", Json::Text(bands.generated_at.to_string())),
            (
                "provenance",
                Json::Object(vec![
                    (
                        "scoredWindowStart",
                        iso_json(summary.scored_window_start_ms),
                    ),
                    ("scoredWindowEnd", iso_json(summary.scored_window_end_ms)),
                    ("scoredOrigins", int_json(summary.origins)),
                    (
                        "scoredWindowSpanDays",
                        Json::Number(summary.scored_window_span_days),
                    ),
                    ("observedCoverage", num(bands.observed_coverage)),
                ]),
            ),
            ("residualCount", int_json(summary.residuals.len())),
        ])
    }

    fn cli_json(args: &[&str], got: &Output) -> Json {
        Json::Object(vec![
            (
                "args",
                Json::Array(args.iter().map(|a| Json::Text((*a).to_owned())).collect()),
            ),
            ("stdout", Json::Text(got.text(Stream::Stdout))),
            ("stderr", Json::Text(got.text(Stream::Stderr))),
            ("status", Json::Number(f64::from(got.code))),
        ])
    }

    /// The text of the value of the top-level member `key` of a
    /// two-space-indented JSON object.
    fn top_level_value<'a>(text: &'a str, key: &str) -> &'a str {
        let marker = format!("\n  \"{key}\": ");
        let start = text.find(&marker).unwrap() + marker.len();
        let rest = &text[start..];
        let end = rest.find("\n  \"").map_or_else(
            || rest.rfind("\n}").unwrap(),
            |next| next - 1, // the comma before the next member
        );
        &rest[..end]
    }

    /// Rewrites the `synthetic` and `dbCompare` sections of
    /// `backtest_expected.json` from the Rust tools and keeps the other
    /// sections byte for byte. Run it, then format the file with Prettier
    /// (`tests/golden/README.md`).
    #[test]
    #[ignore = "rewrites tests/golden/backtest_expected.json"]
    fn regenerate_backtest_expected() {
        let path = golden_dir().join("backtest_expected.json");
        let original = std::fs::read_to_string(&path).unwrap();
        let generated_at = expected()["generatedAt"].as_str().unwrap();

        let summary = run_backtest(synthetic(), &BacktestOptions::default()).unwrap();
        let cmp = compare_optimism(synthetic()).unwrap();
        let derived = derive_bands_from_backtest(synthetic(), generated_at.to_owned()).unwrap();
        let dir = temp_dir("regenerate");
        let file = dir.join("synthetic.json");
        std::fs::write(&file, to_fixture_json(synthetic())).unwrap();
        let cli = run_cli(&["backtest", "--data", file.to_str().unwrap()]);
        std::fs::remove_dir_all(dir).unwrap();
        let mut db_out = Output::default();
        let ok = print_compare(&db_shaped(synthetic()), &mut db_out).unwrap();
        db_out.code = i32::from(!ok);

        let synthetic_json = Json::Object(vec![
            ("summary", summary_json(&summary, true)),
            ("compare", compare_json(&cmp)),
            ("bands", bands_json(&derived)),
            ("cli", cli_json(&["--data", "<tmp>/synthetic.json"], &cli)),
        ]);
        let mut text = String::from("{\n");
        for key in [
            "generatedAt",
            "synthetic",
            "committedFixture",
            "dbCompare",
            "usage",
            "badWindow",
            "fixtureJson",
        ] {
            if key != "generatedAt" {
                text.push_str(",\n");
            }
            text.push_str("  \"");
            text.push_str(key);
            text.push_str("\": ");
            match key {
                "synthetic" => synthetic_json.write(&mut text),
                "dbCompare" => cli_json(&["--db", "--compare"], &db_out).write(&mut text),
                _ => text.push_str(top_level_value(&original, key)),
            }
        }
        text.push_str("\n}\n");
        serde_json::from_str::<Value>(&text).unwrap();
        std::fs::write(&path, text).unwrap();
    }

    // -----------------------------------------------------------------------
    // Revision study
    // -----------------------------------------------------------------------

    fn study_input(case: &Value) -> RevisionStudyInput {
        let mut input = RevisionStudyInput::default();
        for (id, col) in case["vintages"].as_object().unwrap() {
            let id: i32 = id.parse().unwrap();
            let issued = col["issuedMs"].as_array().unwrap();
            let values = col["value"].as_array().unwrap();
            let rows = col["startMs"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, start)| Vintage {
                    dataset_id: id,
                    issued_at: ts(int(&issued[i])),
                    start: ts(int(start)),
                    end: ts(int(start) + QUARTER_MS),
                    value: f(&values[i]),
                })
                .collect();
            input.vintages_by_dataset.insert(id, rows);
        }
        for (id, col) in case["actuals"].as_object().unwrap() {
            let id: i32 = id.parse().unwrap();
            let values = col["value"].as_array().unwrap();
            let rows = col["startMs"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, start)| ActualRecord {
                    dataset_id: id,
                    start: ts(int(start)),
                    value: f(&values[i]),
                })
                .collect();
            input.actuals_by_dataset.insert(id, rows);
        }
        input
    }

    fn check_dataset(got: &DatasetRevisionSummary, want: &Value, stats: &Stats) {
        assert_eq!(i64::from(got.dataset_id), int(&want["datasetId"]));
        for (got, key) in [
            (got.admissible_targets, "admissibleTargets"),
            (got.excluded_targets, "excludedTargets"),
            (got.future_targets, "futureTargets"),
            (got.references_post_delivery, "referencesPostDelivery"),
            (
                got.references_pre_delivery_within_tol,
                "referencesPreDeliveryWithinTol",
            ),
            (got.production_band_samples, "productionBandSamples"),
        ] {
            assert_eq!(got, count(&want[key]), "{key}");
        }
        for (got, key) in [
            (got.exclusion_rate, "exclusionRate"),
            (got.empirical_max_lead_h, "empiricalMaxLeadH"),
            (got.empirical_p90_lead_h, "empiricalP90LeadH"),
            (got.sd_reference, "sdReference"),
            (got.mean_abs_reference, "meanAbsReference"),
            (got.production_band_rms, "productionBandRms"),
            (got.production_band_nsr, "productionBandNsr"),
            (got.attenuation_illustration, "attenuationIllustration"),
        ] {
            stats.close_opt(got, &want[key], key);
        }
        match (&got.actual_check, want["actualCheck"].is_null()) {
            (None, true) => {}
            (Some(check), false) => {
                let w = &want["actualCheck"];
                assert_eq!(check.targets_compared, count(&w["targetsCompared"]));
                stats.close_opt(
                    check.median_abs_ref_minus_actual,
                    &w["medianAbsRefMinusActual"],
                    "m",
                );
                stats.close_opt(
                    check.mean_abs_ref_minus_actual,
                    &w["meanAbsRefMinusActual"],
                    "a",
                );
            }
            (got, _) => panic!("actual check: got {got:?}, want {}", want["actualCheck"]),
        }
        let buckets = want["buckets"].as_array().unwrap();
        assert_eq!(got.buckets.len(), buckets.len());
        for (b, w) in got.buckets.iter().zip(buckets) {
            assert_eq!(b.label, w["label"].as_str().unwrap());
            assert_eq!(b.samples, count(&w["samples"]), "{} samples", b.label);
            assert_eq!(b.targets, count(&w["targets"]), "{} targets", b.label);
            assert_eq!(b.sufficient, w["sufficient"].as_bool().unwrap());
            for (got, key) in [
                (b.mean_abs_revision, "meanAbsRevision"),
                (b.median_abs_revision, "medianAbsRevision"),
                (b.p90_abs_revision, "p90AbsRevision"),
                (b.rms_revision, "rmsRevision"),
                (b.signed_bias_revision, "signedBiasRevision"),
                (b.noise_to_signal, "noiseToSignal"),
                (b.attenuation, "attenuation"),
                (b.rel_mean_abs, "relMeanAbs"),
            ] {
                stats.close_opt(got, &w[key], key);
            }
        }
    }

    #[test]
    fn revision_study_matches_typescript() {
        let inputs = read("revision_input.json");
        let expected = read("revision_expected.json");
        let stats = Stats::default();
        let dir = temp_dir("revision");
        for (case, want) in inputs
            .as_array()
            .unwrap()
            .iter()
            .zip(expected.as_array().unwrap())
        {
            let name = case["name"].as_str().unwrap();
            let input = study_input(case);
            let result = run_revision_study(&input);
            let datasets = want["result"]["datasets"].as_array().unwrap();
            assert_eq!(result.datasets.len(), datasets.len(), "{name}");
            for (got, want) in result.datasets.iter().zip(datasets) {
                check_dataset(got, want, &stats);
            }
            let window = &want["result"]["window"];
            match &result.window {
                None => assert!(window.is_null(), "{name} window"),
                Some(w) => {
                    for (got, key) in [
                        (w.earliest_issued_at, "earliestIssuedAt"),
                        (w.latest_issued_at, "latestIssuedAt"),
                        (w.earliest_target, "earliestTarget"),
                        (w.latest_target, "latestTarget"),
                    ] {
                        assert_eq!(
                            iso_millis(got),
                            window[key].as_str().unwrap(),
                            "{name} {key}"
                        );
                    }
                }
            }
            let rec = recommendation(&result);
            assert_eq!(
                rec.verdict.as_str(),
                want["recommendation"]["verdict"].as_str().unwrap()
            );
            assert_eq!(
                rec.reason,
                want["recommendation"]["reason"].as_str().unwrap(),
                "{name}"
            );

            let file = dir.join(format!("{name}.json"));
            let fixture = revision_cli::to_fixture_json(&input);
            if let Some(text) = want["fixtureJson"].as_str() {
                assert_eq!(fixture, text, "{name} fixture text");
            }
            std::fs::write(&file, fixture).unwrap();
            let got = run_cli(&["revision-magnitude", "--data", file.to_str().unwrap()]);
            assert_same_cli(&got, &want["cli"], name);
        }
        stats.report("revision study");
        assert_eq!(
            stats.tolerant.get(),
            0,
            "the study has no transcendental functions"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    // -----------------------------------------------------------------------
    // Metrics
    // -----------------------------------------------------------------------

    fn floats(v: &Value) -> Vec<f64> {
        v.as_array().unwrap().iter().map(f).collect()
    }

    #[test]
    fn metrics_match_typescript() {
        let stats = Stats::default();
        for case in read("metrics.json").as_array().unwrap() {
            let (pred, act) = (floats(&case["pred"]), floats(&case["act"]));
            stats.close(mae(&pred, &act).unwrap(), f(&case["mae"]), "mae");
            stats.close(rmse(&pred, &act).unwrap(), f(&case["rmse"]), "rmse");
            stats.close(bias(&pred, &act).unwrap(), f(&case["bias"]), "bias");
            stats.close_opt(
                spearman(&pred, &act).unwrap(),
                &case["spearman"],
                "spearman",
            );
            for (i, n) in [1, 2, 16, 200].into_iter().enumerate() {
                let cheap = precision_at_n(&pred, &act, n, RankMode::Cheap).unwrap();
                let peak = precision_at_n(&pred, &act, n, RankMode::Peak).unwrap();
                stats.close_opt(cheap, &case["precisionCheap"][i], "cheap");
                stats.close_opt(peak, &case["precisionPeak"][i], "peak");
            }
            stats.close_opt(median(&pred), &case["median"], "median");
            for (i, p) in [0.0, 10.0, 50.0, 90.0, 99.5, 100.0].into_iter().enumerate() {
                stats.close_opt(percentile(&act, p), &case["percentiles"][i], "percentile");
            }
            stats.close_opt(standard_deviation(&pred), &case["standardDeviation"], "sd");
            stats.close_opt(rms(&act), &case["rms"], "rms");
        }
        assert_eq!(
            stats.tolerant.get(),
            0,
            "the metrics use only exact operations"
        );
    }
}
