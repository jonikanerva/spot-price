//! Behaviour of the backtest engine: vintage selection, issue-time
//! reconstruction, the leakage guard, metrics, and the optimism comparison.

#[cfg(test)]
mod backtest {
    use std::collections::BTreeMap;

    use jiff::Timestamp;
    use spot_price_domain::forecast::QuarterStart;
    use spot_price_tools::backtest::{
        AsOfMode, BacktestOptions, HORIZON_LABELS, MIN_CALIBRATION_WINDOW_DAYS,
        collect_input_timestamps, compare_optimism, derive_bands_from_backtest,
        find_leaking_inputs, forecast_at_issue_time, mae, reconstruct_issue_time, rmae,
        run_backtest, select_vintages_as_of, smape,
    };
    use spot_price_tools::data::{BacktestData, GridActual, PricePoint, Vintage};

    const QUARTER_MS: i64 = 15 * 60 * 1000;
    const HOUR_MS: i64 = 60 * 60 * 1000;
    const DAY_MS: i64 = 24 * HOUR_MS;
    /// Long enough that the stale issuance is knowable for every target of
    /// the 72 h window, which starts about 35 h after the issue time.
    const STALE_LEAD_H: i64 = 130;

    fn ms(text: &str) -> i64 {
        text.parse::<Timestamp>().unwrap().as_millisecond()
    }

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_millisecond(ms).unwrap()
    }

    fn actual(start_ms: i64, value: f64, dataset_id: i32) -> GridActual {
        GridActual {
            dataset_id,
            start: ts(start_ms),
            end: ts(start_ms + QUARTER_MS),
            value,
        }
    }

    fn vintage(start_ms: i64, value: f64, dataset_id: i32, issued_ms: i64) -> Vintage {
        Vintage {
            dataset_id,
            issued_at: ts(issued_ms),
            start: ts(start_ms),
            end: ts(start_ms + QUARTER_MS),
            value,
        }
    }

    fn price(area: &str, start_ms: i64, value: f64) -> PricePoint {
        PricePoint {
            area: area.to_owned(),
            start: ts(start_ms),
            spot_cents_kwh: value,
        }
    }

    /// Mean-zero pseudo-noise in [-1, 1] per quarter index.
    fn noise_at(q: i64) -> f64 {
        #[expect(clippy::cast_precision_loss, reason = "q is a small index")]
        let s = (q as f64 * 12.9898).sin() * 43758.5453;
        (s - s.floor() - 0.5) * 2.0
    }

    /// A 20-day window. Actuals have one value per quarter; forecast
    /// vintages have a fresh (−2 h) issuance and a stale (−130 h) one. The
    /// stale wind issuance has a mean-zero revision scaled by `stale_revision`.
    #[expect(clippy::cast_precision_loss, reason = "q is a small index")]
    fn build_data(stale_revision: f64) -> BacktestData {
        let start = ms("2026-02-01T00:00:00Z");
        let mut data = BacktestData::default();
        let (mut wind75, mut cons124, mut wind245, mut cons165) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for q in 0..20 * 96 {
            let at = start + q * QUARTER_MS;
            let consumption = 8000.0 + (q % 96) as f64 * 15.0;
            let wind = 2000.0 + ((q * 131) % 1800) as f64;
            let fi = 0.0012 * (consumption - wind) + 2.0;
            data.prices.push(price("FI", at, fi));
            data.prices.push(price("SE1", at, fi - 0.5));
            data.prices.push(price("SE3", at, fi + 0.3));
            data.prices.push(price("EE", at, fi + 0.1));
            cons124.push(actual(at, consumption, 124));
            wind75.push(actual(at, wind, 75));
            let stale_wind = wind + stale_revision * noise_at(q);
            cons165.push(vintage(at, consumption, 165, at - 2 * HOUR_MS));
            cons165.push(vintage(at, consumption, 165, at - STALE_LEAD_H * HOUR_MS));
            wind245.push(vintage(at, wind, 245, at - 2 * HOUR_MS));
            wind245.push(vintage(at, stale_wind, 245, at - STALE_LEAD_H * HOUR_MS));
        }
        data.actuals = BTreeMap::from([(75, wind75), (124, cons124)]);
        data.vintages = BTreeMap::from([(245, wind245), (165, cons165)]);
        data
    }

    // selectVintagesAsOf

    #[test]
    fn keeps_the_freshest_issuance_strictly_before_the_bound() {
        let t = ms("2026-02-10T13:00:00Z");
        let vs = [
            vintage(t + DAY_MS, 10.0, 245, t - HOUR_MS),
            vintage(t + DAY_MS, 20.0, 245, t - 3 * HOUR_MS),
            vintage(t + DAY_MS, 30.0, 245, t),
        ];
        let picked = select_vintages_as_of(&vs, Some(t));
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].value, 10.0);
    }

    #[test]
    fn includes_an_issuance_once_the_bound_is_past_it() {
        let t = ms("2026-02-10T13:00:00Z");
        let vs = [vintage(t + DAY_MS, 30.0, 245, t)];
        assert_eq!(select_vintages_as_of(&vs, Some(t)).len(), 0);
        assert_eq!(select_vintages_as_of(&vs, Some(t + 1)).len(), 1);
    }

    #[test]
    fn no_bound_takes_the_freshest_issuance() {
        let t = ms("2026-02-10T13:00:00Z");
        let vs = [
            vintage(t, 10.0, 245, t - HOUR_MS),
            vintage(t, 99.0, 245, t + HOUR_MS),
        ];
        let picked = select_vintages_as_of(&vs, None);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].value, 99.0);
    }

    // reconstructIssueTime

    #[test]
    fn exposes_prices_only_through_the_publication_horizon() {
        let data = build_data(0.0);
        let inputs = reconstruct_issue_time(&data, ms("2026-02-10T13:00:00Z"), AsOfMode::IssueTime);
        assert!(inputs.last_published_ms.is_some());
        let horizon_end = ms("2026-02-12T00:00:00Z");
        assert!(
            inputs
                .fi_prices
                .keys()
                .all(|k| k.as_millisecond() < horizon_end)
        );
    }

    #[test]
    fn keeps_actuals_before_issue_time_and_forecasts_forward_looking() {
        let data = build_data(0.0);
        let t = ms("2026-02-10T13:00:00Z");
        let inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime);
        assert!(
            inputs.actual[&124]
                .iter()
                .all(|r| r.start.as_millisecond() < t)
        );
        let forecast = &inputs.forecast[&245];
        assert!(forecast.iter().any(|r| r.start.as_millisecond() >= t));
        assert!(forecast.iter().all(|r| r.issued_at.as_millisecond() < t));
    }

    // Leakage guard

    #[test]
    fn issue_time_mode_is_leak_free_at_every_origin() {
        let data = build_data(0.0);
        let base = ms("2026-02-05T00:00:00Z");
        for d in 0..10 {
            for hour in [6, 13] {
                let t = base + d * DAY_MS + hour * HOUR_MS;
                let inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime);
                assert_eq!(find_leaking_inputs(&inputs).len(), 0);
                for stamp in collect_input_timestamps(&inputs) {
                    if !stamp.forward_looking_allowed {
                        assert!(stamp.ms < stamp.boundary_ms.unwrap());
                    }
                }
            }
        }
        assert!(
            run_backtest(&data, &BacktestOptions::default())
                .unwrap()
                .leak_free
        );
    }

    #[test]
    fn latest_mode_leaks_by_design() {
        let options = BacktestOptions {
            as_of_mode: Some(AsOfMode::Latest),
            restrict_origins_ms: None,
        };
        assert!(!run_backtest(&build_data(0.0), &options).unwrap().leak_free);
    }

    #[test]
    fn flags_an_injected_future_issuance() {
        let data = build_data(0.0);
        let t = ms("2026-02-10T13:00:00Z");
        let mut inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime);
        inputs
            .forecast
            .entry(245)
            .or_default()
            .push(vintage(t + DAY_MS, 1234.0, 245, t + HOUR_MS));
        let leaks = find_leaking_inputs(&inputs);
        assert_ne!(leaks.len(), 0);
        assert!(
            leaks
                .iter()
                .any(|l| l.source == "fingrid_forecast_vintage_245")
        );
    }

    #[test]
    fn detects_an_injected_future_fi_price() {
        let data = build_data(0.0);
        let mut inputs =
            reconstruct_issue_time(&data, ms("2026-02-10T13:00:00Z"), AsOfMode::IssueTime);
        let key = QuarterStart::from_millisecond(inputs.origin_ms + DAY_MS);
        inputs.fi_prices.insert(key, 99.0);
        assert_ne!(find_leaking_inputs(&inputs).len(), 0);
    }

    // forecastAtIssueTime

    #[test]
    fn anchors_the_series_one_quarter_after_the_last_published_delivery() {
        let data = build_data(0.0);
        let inputs = reconstruct_issue_time(&data, ms("2026-02-10T13:00:00Z"), AsOfMode::IssueTime);
        let fc = forecast_at_issue_time(&inputs).unwrap();
        assert_eq!(
            fc.series_start_ms,
            inputs.last_published_ms.unwrap() + QUARTER_MS
        );
        assert_ne!(fc.predicted.len(), 0);
    }

    // Metrics

    #[test]
    fn mae_and_smape_compute_the_expected_values() {
        assert!((mae(&[(1.0, 2.0), (3.0, 3.0)]).unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(mae(&[]), None);
        assert!((smape(&[(1.0, 3.0), (3.0, 3.0)]).unwrap() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn rmae_is_the_model_to_baseline_ratio() {
        assert!((rmae(Some(0.5), Some(1.0)).unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(rmae(Some(0.5), Some(0.0)), None);
        assert_eq!(rmae(None, Some(1.0)), None);
    }

    // runBacktest

    #[test]
    fn scores_many_origins_and_beats_the_naive_baseline() {
        let summary = run_backtest(&build_data(0.0), &BacktestOptions::default()).unwrap();
        assert!(summary.leak_free);
        assert!(summary.origins > 5);
        assert_eq!(summary.pre_vintage_origins, 0);
        assert_eq!(summary.scored_origins_ms.len(), summary.origins);
        assert!(summary.model_mae.is_some());
        assert!(summary.r_mae.last_week.unwrap() < 1.0);
    }

    #[test]
    #[expect(clippy::cast_precision_loss, reason = "q is a small index")]
    fn skips_and_counts_pre_vintage_origins() {
        let start = ms("2026-02-01T00:00:00Z");
        let issued_all = ms("2026-02-20T00:00:00Z");
        let mut data = BacktestData::default();
        let (mut wind75, mut cons124, mut wind245, mut cons165) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for q in 0..24 * 96 {
            let at = start + q * QUARTER_MS;
            let consumption = 8000.0 + (q % 96) as f64 * 15.0;
            let wind = 2000.0 + ((q * 131) % 1800) as f64;
            data.prices
                .push(price("FI", at, 0.0012 * (consumption - wind) + 2.0));
            cons124.push(actual(at, consumption, 124));
            wind75.push(actual(at, wind, 75));
            cons165.push(vintage(at, consumption, 165, issued_all));
            wind245.push(vintage(at, wind, 245, issued_all));
        }
        data.actuals = BTreeMap::from([(75, wind75), (124, cons124)]);
        data.vintages = BTreeMap::from([(245, wind245), (165, cons165)]);
        let summary = run_backtest(&data, &BacktestOptions::default()).unwrap();
        assert!(summary.pre_vintage_origins > 0);
        assert!(summary.origins > 0);
    }

    #[test]
    fn reports_the_scored_origin_window() {
        let summary = run_backtest(&build_data(0.0), &BacktestOptions::default()).unwrap();
        assert!(summary.scored_window_start_ms.is_some());
        assert!(summary.scored_window_end_ms.is_some());
        assert!(summary.scored_window_span_days > 0.0);
    }

    #[test]
    fn harvests_out_of_sample_residuals() {
        let summary = run_backtest(&build_data(0.0), &BacktestOptions::default()).unwrap();
        assert_ne!(summary.residuals.len(), 0);
        for r in &summary.residuals {
            assert!(r.utc_hour.get() <= 23);
            assert!(r.predicted_raw.is_finite() && r.actual_raw.is_finite());
        }
    }

    // deriveBandsFromBacktest

    #[test]
    fn ships_dark_under_the_minimum_calibration_window() {
        let derived =
            derive_bands_from_backtest(&build_data(0.0), "2026-06-01T00:00:00.000Z".to_owned())
                .unwrap();
        assert!(derived.summary.scored_window_span_days < MIN_CALIBRATION_WINDOW_DAYS);
        assert!(!derived.bands.calibrated);
        assert_eq!(derived.bands.hour_count(), 0);
        assert_eq!(derived.bands.observed_coverage, None);
        assert!(derived.observed_coverage.unwrap() >= 0.0);
    }

    // compareOptimism

    #[test]
    fn a_non_revising_ladder_gives_about_zero_optimism() {
        let cmp = compare_optimism(&build_data(0.0)).unwrap();
        assert!(cmp.scored_origins > 0);
        assert_eq!(
            cmp.leaked.scored_origins_ms.len(),
            cmp.honest.scored_origins_ms.len()
        );
        assert!(cmp.honest.leak_free);
        assert!(!cmp.leaked.leak_free);
        assert!(cmp.delta_mae.unwrap_or(0.0).abs() < 1e-6);
    }

    #[test]
    fn a_revising_ladder_makes_the_leaked_scoreboard_optimistic() {
        let cmp = compare_optimism(&build_data(1500.0)).unwrap();
        assert!(cmp.scored_origins > 0);
        let delta = cmp.delta_mae.unwrap();
        assert!(delta < 0.0);
        assert!(delta.abs() > 1e-3);
    }

    #[test]
    fn reports_the_ladder_depth_per_dataset() {
        let cmp = compare_optimism(&build_data(0.0)).unwrap();
        for id in [245, 165] {
            assert_eq!(cmp.ladder_diagnostic[&id].median_depth, Some(2.0));
        }
        // The report order is the JavaScript integer-key order: 165, then 245.
        assert_eq!(
            cmp.ladder_diagnostic.keys().copied().collect::<Vec<_>>(),
            [165, 245]
        );
    }

    // Per-horizon rank metrics

    #[test]
    fn fills_every_horizon_with_pairs_and_rank_metrics() {
        let summary = run_backtest(&build_data(0.0), &BacktestOptions::default()).unwrap();
        let mut total = 0;
        for (label, m) in HORIZON_LABELS.iter().zip(&summary.by_horizon) {
            assert!(m.pairs > 0, "{label}");
            total += m.pairs;
            assert!(m.spearman.is_some() && m.precision_at_n_cheap.is_some());
            assert!(m.precision_at_n_peak.is_some());
        }
        assert_eq!(total, summary.residuals.len());
    }
}
