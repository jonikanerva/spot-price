//! Behaviour of the backtest engine: vintage selection, issue-time
//! reconstruction, the leakage guard, metrics, and the optimism comparison.

#[cfg(test)]
mod backtest {
    use std::collections::{BTreeMap, BTreeSet};

    use jiff::Timestamp;
    use jiff::tz::TimeZone;
    use spot_price_domain::forecast::{
        ForecastOptions, ForecastWindow, LatestPublishedRule, PrevDayBasis, QuarterStart,
    };
    use spot_price_tools::backtest::{
        AsOfMode, BacktestOptions, Better, DayBasis, ForecastVariant, HORIZON_LABELS,
        HorizonMetrics, HorizonPairedDelta, IssueTimeInputs, MIN_CALIBRATION_WINDOW_DAYS,
        OriginMetrics, PairedDelta, collect_input_timestamps, compare_optimism, compare_variants,
        derive_bands_from_backtest, find_leaking_inputs, forecast_at_issue_time,
        forecast_at_issue_time_with, mae, paired_delta, paired_horizon, reconstruct_issue_time,
        rmae, run_backtest, select_vintages_as_of, smape,
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
        let inputs =
            reconstruct_issue_time(&data, ms("2026-02-10T13:00:00Z"), AsOfMode::IssueTime).unwrap();
        assert!(inputs.last_published_ms.is_some());
        let horizon_end = ms("2026-02-11T23:00:00Z");
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
        let inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime).unwrap();
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
                let inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime).unwrap();
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
            ..BacktestOptions::default()
        };
        assert!(!run_backtest(&build_data(0.0), &options).unwrap().leak_free);
    }

    #[test]
    fn flags_an_injected_future_issuance() {
        let data = build_data(0.0);
        let t = ms("2026-02-10T13:00:00Z");
        let mut inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime).unwrap();
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
            reconstruct_issue_time(&data, ms("2026-02-10T13:00:00Z"), AsOfMode::IssueTime).unwrap();
        let key = QuarterStart::from_millisecond(inputs.origin_ms + DAY_MS);
        inputs.fi_prices.insert(key, 99.0);
        assert_ne!(find_leaking_inputs(&inputs).len(), 0);
    }

    // forecastAtIssueTime

    #[test]
    fn anchors_the_series_one_quarter_after_the_last_published_delivery() {
        let data = build_data(0.0);
        let inputs =
            reconstruct_issue_time(&data, ms("2026-02-10T13:00:00Z"), AsOfMode::IssueTime).unwrap();
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
    fn scores_many_origins_against_both_baselines() {
        let summary = run_backtest(&build_data(0.0), &BacktestOptions::default()).unwrap();
        assert!(summary.leak_free);
        assert!(summary.origins > 5);
        assert_eq!(summary.pre_vintage_origins, 0);
        assert_eq!(summary.scored_origins_ms.len(), summary.origins);
        assert!(summary.model_mae.is_some());
        for r_mae in [summary.r_mae.last_week, summary.r_mae.last_published_day] {
            assert!(r_mae.is_some_and(|r| r.is_finite() && r > 0.0), "{r_mae:?}");
        }
    }

    /// 24 days whose vintages are all issued on day 19, so the earlier
    /// origins have no knowable wind forecast.
    #[expect(clippy::cast_precision_loss, reason = "q is a small index")]
    fn build_late_vintage_data() -> BacktestData {
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
        data
    }

    #[test]
    fn skips_and_counts_pre_vintage_origins() {
        let data = build_late_vintage_data();
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

    // Paired A/B comparison

    /// A ridge penalty far above the production value, so the fit shrinks
    /// and the predictions move.
    fn strong_ridge() -> ForecastVariant {
        ForecastVariant {
            ridge_lambda: 1e6,
            ..ForecastVariant::default()
        }
    }

    #[test]
    fn the_default_variant_is_the_production_model() {
        let variant = ForecastVariant::default();
        let production = ForecastOptions::default();
        let oslo = oslo();
        let options = variant.forecast_options(&oslo);
        assert_eq!(
            options.ridge_lambda.to_bits(),
            production.ridge_lambda.to_bits()
        );
        assert_eq!(
            options.wind_extension_weeks,
            production.wind_extension_weeks
        );
        assert_eq!(
            options.consumption_extension_weeks,
            production.consumption_extension_weeks
        );
        assert!(options.sanity_bound.is_none());
        assert_eq!(options.latest_published, production.latest_published);
        assert_eq!(variant.latest_published, LatestPublishedRule::AtOrBefore);
        assert_eq!(options.prev_day_basis, production.prev_day_basis);
        assert_eq!(variant.prev_day_basis, DayBasis::Utc);
        assert_eq!(BacktestOptions::default().variant, variant);

        let data = build_data(0.0);
        let inputs =
            reconstruct_issue_time(&data, ms("2026-02-10T13:00:00Z"), AsOfMode::IssueTime).unwrap();
        assert_eq!(
            forecast_at_issue_time(&inputs).unwrap(),
            forecast_at_issue_time_with(&inputs, variant).unwrap()
        );
    }

    #[test]
    fn the_variant_reaches_the_forecast() {
        let data = build_data(0.0);
        let inputs =
            reconstruct_issue_time(&data, ms("2026-02-10T13:00:00Z"), AsOfMode::IssueTime).unwrap();
        let a = forecast_at_issue_time(&inputs).unwrap();
        let b = forecast_at_issue_time_with(&inputs, strong_ridge()).unwrap();
        assert_eq!(a.series_start_ms, b.series_start_ms);
        assert_eq!(a.predicted.len(), b.predicted.len());
        assert_ne!(a.predicted, b.predicted);
    }

    #[test]
    fn a_against_itself_gives_zero_deltas_on_paired_origins() {
        let data = build_data(0.0);
        let cmp = compare_variants(
            &data,
            ForecastVariant::default(),
            ForecastVariant::default(),
        )
        .unwrap();
        assert!(cmp.origins_match);
        assert_eq!(cmp.a, cmp.b);
        assert_eq!(
            cmp.shared_origins,
            run_backtest(&data, &BacktestOptions::default())
                .unwrap()
                .origins
        );
        for (label, d) in HORIZON_LABELS.iter().zip(&cmp.delta_by_horizon) {
            assert_eq!(d.delta_spearman, Some(0.0), "{label}");
            assert_eq!(d.delta_precision_at_n_cheap, Some(0.0), "{label}");
            assert_eq!(d.delta_precision_at_n_peak, Some(0.0), "{label}");
            assert_eq!(d.delta_mae, Some(0.0), "{label}");
        }
    }

    #[test]
    fn the_latest_published_variant_reaches_the_forecast() {
        let data = build_data(0.0);
        let inputs =
            reconstruct_issue_time(&data, ms("2026-02-10T13:00:00Z"), AsOfMode::IssueTime).unwrap();
        let before = ForecastVariant {
            latest_published: LatestPublishedRule::Before,
            ..ForecastVariant::default()
        };
        assert_eq!(
            before.forecast_options(&oslo()).latest_published,
            LatestPublishedRule::Before
        );
        let a = forecast_at_issue_time(&inputs).unwrap();
        let b = forecast_at_issue_time_with(&inputs, before).unwrap();
        assert_eq!(a.series_start_ms, b.series_start_ms);
        assert_eq!(a.predicted.len(), b.predicted.len());
        assert_ne!(a.predicted, b.predicted);
    }

    fn oslo() -> TimeZone {
        TimeZone::get("Europe/Oslo").unwrap()
    }

    #[test]
    fn the_prev_day_basis_variant_reaches_the_forecast() {
        let data = build_data(0.0);
        let inputs =
            reconstruct_issue_time(&data, ms("2026-02-10T13:00:00Z"), AsOfMode::IssueTime).unwrap();
        let delivery = ForecastVariant {
            prev_day_basis: DayBasis::Delivery,
            ..ForecastVariant::default()
        };
        let zone = oslo();
        assert_eq!(
            delivery.forecast_options(&zone).prev_day_basis,
            PrevDayBasis::DeliveryDay(&zone)
        );
        let a = forecast_at_issue_time(&inputs).unwrap();
        let b = forecast_at_issue_time_with(&inputs, delivery).unwrap();
        assert_eq!(a.series_start_ms, ms("2026-02-11T23:00:00Z"));
        assert_eq!(a.series_start_ms, b.series_start_ms);
        assert_eq!(a.predicted.len(), b.predicted.len());
        assert_ne!(a.predicted, b.predicted);
    }

    #[test]
    fn a_different_variant_is_scored_on_the_same_origins_and_targets() {
        let data = build_data(0.0);
        let cmp = compare_variants(&data, ForecastVariant::default(), strong_ridge()).unwrap();
        assert!(cmp.origins_match);
        assert!(cmp.shared_origins > 5);
        assert_eq!(cmp.a.scored_origins_ms, cmp.b.scored_origins_ms);
        assert_eq!(cmp.variant_b, strong_ridge());
        assert!(cmp.a.leak_free && cmp.b.leak_free);
        for (h, label) in HORIZON_LABELS.iter().enumerate() {
            let (a, b, d) = (
                &cmp.a.by_horizon[h],
                &cmp.b.by_horizon[h],
                &cmp.delta_by_horizon[h],
            );
            assert_eq!(a.pairs, b.pairs, "{label}");
            assert!(a.pairs > 0, "{label}");
            let delta_mae = d.delta_mae.unwrap();
            assert!(
                (delta_mae - (b.mae.unwrap() - a.mae.unwrap())).abs() < 1e-12,
                "{label}"
            );
            let delta_spearman = d.delta_spearman.unwrap();
            assert!(
                (delta_spearman - (b.spearman.unwrap() - a.spearman.unwrap())).abs() < 1e-12,
                "{label}"
            );
            assert!(d.delta_precision_at_n_cheap.is_some(), "{label}");
            assert!(d.delta_precision_at_n_peak.is_some(), "{label}");
        }
        assert!(
            cmp.delta_by_horizon
                .iter()
                .any(|d| d.delta_mae.is_some_and(|m| m.abs() > 1e-9))
        );
    }

    // Paired (origin-by-origin) deltas

    fn values(pairs: &[(i64, Option<f64>)]) -> BTreeMap<i64, Option<f64>> {
        pairs.iter().copied().collect()
    }

    fn assert_counts_add_up(d: &PairedDelta) {
        assert_eq!(d.wins + d.losses + d.ties, d.paired, "{d:?}");
    }

    type Metric = fn(&OriginMetrics) -> Option<f64>;

    const METRICS: [(&str, Metric); 4] = [
        ("mae", |r| r.mae),
        ("spearman", |r| r.spearman),
        ("cheap", |r| r.precision_at_n_cheap),
        ("peak", |r| r.precision_at_n_peak),
    ];

    fn by_metric(p: &HorizonPairedDelta) -> [PairedDelta; 4] {
        [
            p.mae,
            p.spearman,
            p.precision_at_n_cheap,
            p.precision_at_n_peak,
        ]
    }

    #[test]
    fn a_against_itself_pairs_every_origin_as_a_tie_for_mae_and_ranks() {
        let data = build_data(0.0);
        let cmp = compare_variants(
            &data,
            ForecastVariant::default(),
            ForecastVariant::default(),
        )
        .unwrap();
        assert!(cmp.shared_origins > 5);
        // Every scored origin has a d+1 pair; the last origins have no
        // realised price for the later horizons.
        assert_eq!(
            cmp.a.by_horizon[0]
                .by_origin
                .keys()
                .copied()
                .collect::<BTreeSet<_>>(),
            cmp.a.scored_origins_ms
        );
        for (h, label) in HORIZON_LABELS.iter().enumerate() {
            let by_origin = &cmp.a.by_horizon[h].by_origin;
            let paired = by_metric(&cmp.paired_by_horizon[h]);
            // Every origin with a pair in this horizon has an MAE.
            assert!(by_origin.values().all(|r| r.mae.is_some()), "{label}");
            for ((name, metric), d) in METRICS.iter().zip(paired) {
                let with_value = by_origin.values().filter(|r| metric(r).is_some()).count();
                assert!(with_value > 0, "{label} {name}");
                assert_counts_add_up(&d);
                assert_eq!(d.paired, with_value, "{label} {name}");
                assert_eq!(d.ties, d.paired, "{label} {name}");
                assert_eq!((d.wins, d.losses, d.excluded), (0, 0, 0), "{label} {name}");
                assert_eq!(d.mean_delta, Some(0.0), "{label} {name}");
            }
        }
    }

    #[test]
    fn an_origin_with_a_value_in_one_arm_only_is_excluded_and_counted() {
        // Origin 2 has no value in arm B, origin 3 has none in either arm.
        let a = values(&[(1, Some(0.5)), (2, Some(0.9)), (3, None)]);
        let b = values(&[(1, Some(0.6)), (2, None), (3, None)]);
        let d = paired_delta(&a, &b, Better::Higher);
        assert_counts_add_up(&d);
        assert_eq!(d.paired, 1);
        assert_eq!(d.excluded, 1);
        assert_eq!((d.wins, d.losses, d.ties), (1, 0, 0));
        let paired_mean = d.mean_delta.unwrap();
        assert!((paired_mean - 0.1).abs() < 1e-12, "{paired_mean}");

        // The delta of the arm means mixes origins: B covers origin 1 only,
        // A covers origins 1 and 2, so it reports a loss of 0.1 for B.
        let delta_of_means = 0.6 - f64::midpoint(0.5, 0.9);
        assert!((delta_of_means + 0.1).abs() < 1e-12, "{delta_of_means}");
        assert!(paired_mean > 0.0 && delta_of_means < 0.0);

        // An origin that arm B never scored counts like a `None` value.
        let missing_origin = paired_delta(&a, &values(&[(1, Some(0.6))]), Better::Higher);
        assert_eq!(missing_origin, d);
    }

    #[test]
    fn counts_wins_losses_and_exact_ties_on_known_values() {
        let a = values(&[
            (1, Some(0.2)),
            (2, Some(0.5)),
            (3, Some(0.7)),
            (4, Some(0.4)),
            (5, Some(0.3)),
        ]);
        let b = values(&[
            (1, Some(0.3)),
            (2, Some(0.5)),
            (3, Some(0.6)),
            (4, Some(0.9)),
            (5, Some(0.3)),
        ]);
        let d = paired_delta(&a, &b, Better::Higher);
        assert_counts_add_up(&d);
        assert_eq!(d.paired, 5);
        assert_eq!((d.wins, d.losses, d.ties, d.excluded), (2, 1, 2, 0));
        let mean = d.mean_delta.unwrap();
        assert!((mean - 0.1).abs() < 1e-12, "{mean}");

        let empty = paired_delta(&BTreeMap::new(), &BTreeMap::new(), Better::Higher);
        assert_eq!(empty.paired, 0);
        assert_eq!(empty.mean_delta, None);
    }

    #[test]
    fn a_lower_value_wins_when_lower_is_better() {
        // MAE: B is more accurate on origins 1 and 3, less on 2, equal on 4.
        let a = values(&[
            (1, Some(1.0)),
            (2, Some(1.0)),
            (3, Some(2.0)),
            (4, Some(0.5)),
        ]);
        let b = values(&[
            (1, Some(0.5)),
            (2, Some(1.5)),
            (3, Some(1.0)),
            (4, Some(0.5)),
        ]);
        let d = paired_delta(&a, &b, Better::Lower);
        assert_counts_add_up(&d);
        assert_eq!(
            (d.paired, d.wins, d.losses, d.ties, d.excluded),
            (4, 2, 1, 1, 0)
        );
        // The mean delta keeps the B − A sign: negative = B more accurate.
        let mean = d.mean_delta.unwrap();
        assert!((mean + 0.25).abs() < 1e-12, "{mean}");
        let higher = paired_delta(&a, &b, Better::Higher);
        assert_eq!((higher.wins, higher.losses), (1, 2));
        assert_eq!(higher.mean_delta, d.mean_delta);
    }

    fn horizon_with(by_origin: &[(i64, OriginMetrics)]) -> HorizonMetrics {
        HorizonMetrics {
            mae: None,
            smape: None,
            spearman: None,
            precision_at_n_cheap: None,
            precision_at_n_peak: None,
            band_coverage: None,
            pairs: 0,
            by_origin: by_origin.iter().copied().collect(),
        }
    }

    fn metrics(mae: f64, spearman: f64, cheap: f64, peak: Option<f64>) -> OriginMetrics {
        OriginMetrics {
            mae: Some(mae),
            spearman: Some(spearman),
            precision_at_n_cheap: Some(cheap),
            precision_at_n_peak: peak,
        }
    }

    /// `(paired, wins, losses, ties, excluded)` and the mean delta.
    fn assert_paired(d: &PairedDelta, counts: (usize, usize, usize, usize, usize), mean: f64) {
        assert_counts_add_up(d);
        assert_eq!(
            (d.paired, d.wins, d.losses, d.ties, d.excluded),
            counts,
            "{d:?}"
        );
        assert!((d.mean_delta.unwrap() - mean).abs() < 1e-12, "{d:?}");
    }

    #[test]
    fn paired_horizon_reads_each_metric_as_b_minus_a_in_its_direction() {
        // Each metric has its own outcome, so a swapped arm or metric fails.
        let a = horizon_with(&[
            (1, metrics(1.0, 0.1, 0.5, Some(0.9))),
            (2, metrics(1.0, 0.1, 0.5, Some(0.9))),
            (3, metrics(1.0, 0.1, 0.5, Some(0.9))),
        ]);
        let b = horizon_with(&[
            (1, metrics(0.5, 0.3, 0.25, Some(1.0))),
            (2, metrics(1.5, 0.3, 0.25, Some(0.9))),
            (3, metrics(0.5, 0.3, 0.25, None)),
        ]);
        let d = paired_horizon(&a, &b);
        // MAE: a lower value wins, and the mean keeps the B − A sign.
        assert_paired(&d.mae, (3, 2, 1, 0, 0), -1.0 / 6.0);
        assert_paired(&d.spearman, (3, 3, 0, 0, 0), 0.2);
        assert_paired(&d.precision_at_n_cheap, (3, 0, 3, 0, 0), -0.25);
        assert_paired(&d.precision_at_n_peak, (2, 1, 0, 1, 1), 0.05);
    }

    #[test]
    fn a_delta_that_is_not_finite_is_excluded() {
        let a = values(&[(1, Some(0.5)), (2, Some(0.5)), (3, Some(0.5))]);
        let b = values(&[
            (1, Some(f64::NAN)),
            (2, Some(0.4)),
            (3, Some(f64::INFINITY)),
        ]);
        for better in [Better::Higher, Better::Lower] {
            let d = paired_delta(&a, &b, better);
            assert_counts_add_up(&d);
            assert_eq!((d.paired, d.excluded), (1, 2), "{better:?}");
            assert_eq!(d.wins + d.losses, 1, "{better:?}");
            assert!(d.mean_delta.unwrap().is_finite(), "{better:?}");
        }
    }

    #[test]
    fn a_different_variant_gets_consistent_paired_counts() {
        let data = build_data(0.0);
        let cmp = compare_variants(&data, ForecastVariant::default(), strong_ridge()).unwrap();
        for (h, label) in HORIZON_LABELS.iter().enumerate() {
            let (a, b) = (
                &cmp.a.by_horizon[h].by_origin,
                &cmp.b.by_horizon[h].by_origin,
            );
            let paired = by_metric(&cmp.paired_by_horizon[h]);
            for ((name, metric), d) in METRICS.iter().zip(paired) {
                assert_counts_add_up(&d);
                let either_has_value = a
                    .keys()
                    .chain(b.keys())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .filter(|t| {
                        a.get(t).and_then(metric).is_some() || b.get(t).and_then(metric).is_some()
                    })
                    .count();
                assert_eq!(d.paired + d.excluded, either_has_value, "{label} {name}");
            }
        }
        // Both arms score the same quarters of each origin, so the per-origin
        // MAE deltas weighted by their pair counts give the pooled MAE delta.
        let realized: BTreeSet<i64> = data
            .prices
            .iter()
            .filter(|p| p.area == "FI")
            .map(|p| p.start.as_millisecond())
            .collect();
        for (h, label) in HORIZON_LABELS.iter().enumerate() {
            let mae = cmp.paired_by_horizon[h].mae;
            assert!(mae.paired > 0, "{label}");
            assert_eq!(mae.excluded, 0, "{label}");
            assert!(mae.ties < mae.paired, "{label} {mae:?}");
            let (mut weighted, mut pairs) = (0.0, 0_usize);
            for (t, origin_a) in &cmp.a.by_horizon[h].by_origin {
                let inputs = reconstruct_issue_time(&data, *t, AsOfMode::IssueTime).unwrap();
                let day_start = inputs.origin_ms + i64::try_from(h).unwrap() * DAY_MS;
                let n = forecast_at_issue_time(&inputs)
                    .unwrap()
                    .predicted
                    .iter()
                    .map(|(key, _)| key.as_millisecond())
                    .filter(|ms| (day_start..day_start + DAY_MS).contains(ms))
                    .filter(|ms| realized.contains(ms))
                    .count();
                let origin_b = cmp.b.by_horizon[h].by_origin[t];
                #[expect(clippy::cast_precision_loss, reason = "at most 96 pairs")]
                let weight = n as f64;
                weighted += weight * (origin_b.mae.unwrap() - origin_a.mae.unwrap());
                pairs += n;
            }
            assert_eq!(pairs, cmp.a.by_horizon[h].pairs, "{label}");
            #[expect(clippy::cast_precision_loss, reason = "a small pair count")]
            let weighted = weighted / pairs as f64;
            let pooled = cmp.delta_by_horizon[h].delta_mae.unwrap();
            assert!(
                (weighted - pooled).abs() < 1e-9,
                "{label} {weighted} {pooled}"
            );
        }
        // Day by day, B is more accurate on some d+1 days and less on others.
        let d1 = cmp.paired_by_horizon[0].mae;
        assert!(d1.wins > 0 && d1.losses > 0, "{d1:?}");
    }

    #[test]
    fn each_origin_mae_comes_from_the_pairs_of_that_origin_and_day() {
        let data = build_data(0.0);
        let realized: BTreeMap<i64, f64> = data
            .prices
            .iter()
            .filter(|p| p.area == "FI")
            .map(|p| (p.start.as_millisecond(), p.spot_cents_kwh))
            .collect();
        let cmp = compare_variants(&data, ForecastVariant::default(), strong_ridge()).unwrap();
        for (variant, summary) in [(cmp.variant_a, &cmp.a), (cmp.variant_b, &cmp.b)] {
            let mut checked = 0;
            for &t in &summary.scored_origins_ms {
                let inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime).unwrap();
                let fc = forecast_at_issue_time_with(&inputs, variant).unwrap();
                for (h, label) in HORIZON_LABELS.iter().enumerate() {
                    let day_start = fc.series_start_ms + i64::try_from(h).unwrap() * DAY_MS;
                    let errors: Vec<f64> = fc
                        .predicted
                        .iter()
                        .map(|(key, p)| (key.as_millisecond(), p))
                        .filter(|(ms, _)| (day_start..day_start + DAY_MS).contains(ms))
                        .filter_map(|(ms, p)| realized.get(&ms).map(|a| (p - a).abs()))
                        .collect();
                    let got = summary.by_horizon[h].by_origin.get(&t);
                    if errors.is_empty() {
                        assert_eq!(got, None, "{label} {t}");
                        continue;
                    }
                    #[expect(clippy::cast_precision_loss, reason = "at most 96 errors")]
                    let want = errors.iter().sum::<f64>() / errors.len() as f64;
                    let got = got.and_then(|m| m.mae).unwrap();
                    assert!((got - want).abs() < 1e-12, "{label} {t} {got} {want}");
                    checked += 1;
                }
            }
            assert!(checked > 2 * summary.scored_origins_ms.len());
        }
    }

    // Served Fingrid window

    /// The issue time of the wind-forecast-gap fixture.
    const GAP_ISSUE: &str = "2026-02-10T13:00:00Z";

    /// 46 days. The wind forecast stops on 2026-02-07, 3.5 days before the
    /// issue time, so the 4-week tail extension looks back past the 30-day
    /// served history. Wind actuals before 2026-01-13 sit on a much higher
    /// level, so any read of them moves the forecast.
    #[expect(clippy::cast_precision_loss, reason = "q is a small index")]
    fn build_wind_gap_data() -> BacktestData {
        let start = ms("2026-01-01T00:00:00Z");
        let old_until = ms("2026-01-13T00:00:00Z");
        let wind_forecast_until = ms("2026-02-07T00:00:00Z");
        let mut data = BacktestData::default();
        let (mut wind75, mut cons124, mut wind245, mut cons165) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for q in 0..46 * 96 {
            let at = start + q * QUARTER_MS;
            let consumption = 8000.0 + (q % 96) as f64 * 15.0;
            let wind = 2000.0 + ((q * 131) % 1800) as f64;
            let fi = 0.0012 * (consumption - wind) + 2.0;
            data.prices.push(price("FI", at, fi));
            data.prices.push(price("SE1", at, fi - 0.5));
            data.prices.push(price("SE3", at, fi + 0.3));
            data.prices.push(price("EE", at, fi + 0.1));
            cons124.push(actual(at, consumption, 124));
            let recorded_wind = if at < old_until { wind + 4000.0 } else { wind };
            wind75.push(actual(at, recorded_wind, 75));
            cons165.push(vintage(at, consumption, 165, at - STALE_LEAD_H * HOUR_MS));
            if at < wind_forecast_until {
                wind245.push(vintage(at, wind, 245, at - 2 * HOUR_MS));
            }
        }
        data.actuals = BTreeMap::from([(75, wind75), (124, cons124)]);
        data.vintages = BTreeMap::from([(245, wind245), (165, cons165)]);
        data
    }

    /// The window of the served forecast, from the domain rule that the
    /// server reads with: `(history_start, series_end)` in UTC ms.
    fn served_window(inputs: &IssueTimeInputs) -> (i64, i64) {
        let window =
            ForecastWindow::new(inputs.last_published_ms.map(ts), ts(inputs.t_ms)).unwrap();
        assert_eq!(window.series_start.as_millisecond(), inputs.origin_ms);
        (
            window.history_start.as_millisecond(),
            window.series_end.as_millisecond(),
        )
    }

    /// The issue-time inputs with Fingrid data read like the served
    /// forecast: actuals in `[history_start, min(t, series_end))`, the
    /// selected vintages with targets in `[history_start, series_end)`.
    /// Prices stay as the backtest reads them; this oracle covers Fingrid
    /// data only.
    fn with_served_fingrid(data: &BacktestData, inputs: &IssueTimeInputs) -> IssueTimeInputs {
        let (history_start, series_end) = served_window(inputs);
        let actual_end = inputs.t_ms.min(series_end);
        let actual = data
            .actuals
            .iter()
            .map(|(id, rows)| {
                let kept = rows
                    .iter()
                    .filter(|r| (history_start..actual_end).contains(&r.start.as_millisecond()))
                    .copied()
                    .collect();
                (*id, kept)
            })
            .collect();
        let forecast = data
            .vintages
            .iter()
            .map(|(id, rows)| {
                let kept = select_vintages_as_of(rows, Some(inputs.t_ms))
                    .into_iter()
                    .filter(|v| (history_start..series_end).contains(&v.start.as_millisecond()))
                    .collect();
                (*id, kept)
            })
            .collect();
        IssueTimeInputs {
            actual,
            forecast,
            ..inputs.clone()
        }
    }

    #[test]
    fn the_gap_fixture_reaches_actuals_older_than_the_served_history() {
        let data = build_wind_gap_data();
        let t = ms(GAP_ISSUE);
        let inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime).unwrap();
        let (history_start, _) = served_window(&inputs);
        assert!(
            data.actuals[&75]
                .iter()
                .any(|r| r.start.as_millisecond() < history_start)
        );
        let last_wind_target = select_vintages_as_of(&data.vintages[&245], Some(t))
            .iter()
            .map(|v| v.start.as_millisecond())
            .max()
            .unwrap();
        assert!(t - last_wind_target >= 2 * DAY_MS);
        let first_extended = last_wind_target + QUARTER_MS;
        let weeks = i64::from(ForecastVariant::default().wind_extension_weeks);
        assert!(first_extended - weeks * 7 * DAY_MS < history_start);
    }

    #[test]
    fn issue_time_fingrid_inputs_stay_inside_the_served_window() {
        let data = build_wind_gap_data();
        for mode in [AsOfMode::IssueTime, AsOfMode::Latest] {
            let inputs = reconstruct_issue_time(&data, ms(GAP_ISSUE), mode).unwrap();
            let (history_start, series_end) = served_window(&inputs);
            let actual_end = inputs.t_ms.min(series_end);
            for (id, rows) in &inputs.actual {
                assert!(!rows.is_empty(), "{id}");
                for r in rows {
                    let start = r.start.as_millisecond();
                    assert!((history_start..actual_end).contains(&start), "{id} {start}");
                }
            }
            for (id, rows) in &inputs.forecast {
                assert!(!rows.is_empty(), "{id}");
                for v in rows {
                    let start = v.start.as_millisecond();
                    assert!((history_start..series_end).contains(&start), "{id} {start}");
                }
            }
        }
    }

    #[test]
    fn after_a_wind_forecast_gap_the_backtest_forecasts_like_the_served_window() {
        let data = build_wind_gap_data();
        let t = ms(GAP_ISSUE);
        let inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime).unwrap();
        let served = with_served_fingrid(&data, &inputs);
        let every_earlier_actual = IssueTimeInputs {
            actual: data
                .actuals
                .iter()
                .map(|(id, rows)| {
                    let known = rows
                        .iter()
                        .filter(|r| r.start.as_millisecond() < t)
                        .copied()
                        .collect();
                    (*id, known)
                })
                .collect(),
            ..served.clone()
        };
        let served_fc = forecast_at_issue_time(&served).unwrap();
        // The fixture is sensitive: the older actuals change the forecast.
        assert_ne!(
            forecast_at_issue_time(&every_earlier_actual)
                .unwrap()
                .predicted,
            served_fc.predicted
        );
        assert_eq!(forecast_at_issue_time(&inputs).unwrap(), served_fc);
        assert_eq!(inputs.actual, served.actual);
        assert_eq!(inputs.forecast, served.forecast);
    }

    #[test]
    fn extension_weeks_above_the_cap_add_nothing_inside_the_served_window() {
        let data = build_wind_gap_data();
        let inputs = reconstruct_issue_time(&data, ms(GAP_ISSUE), AsOfMode::IssueTime).unwrap();
        let weeks = |n: u32| ForecastVariant {
            wind_extension_weeks: n,
            ..ForecastVariant::default()
        };
        let at = |n: u32| forecast_at_issue_time_with(&inputs, weeks(n)).unwrap();
        assert_eq!(ForecastVariant::default().wind_extension_weeks, 4);
        assert_ne!(at(3).predicted, at(4).predicted);
        assert_eq!(at(5), at(4));
        assert_eq!(at(8), at(4));
    }

    // Served price window and series start

    const NEIGHBOR_AREAS: [&str; 3] = ["SE1", "SE3", "EE"];

    /// FI and neighbour prices for every quarter of 2026-01-01 to
    /// 2026-11-01, without Fingrid data.
    #[expect(clippy::cast_precision_loss, reason = "q is a small index")]
    fn build_price_year() -> BacktestData {
        let start = ms("2026-01-01T00:00:00Z");
        let end = ms("2026-11-01T00:00:00Z");
        let mut data = BacktestData::default();
        let mut q = 0;
        while start + q * QUARTER_MS < end {
            let at = start + q * QUARTER_MS;
            let fi = 2.0 + (q % 96) as f64 * 0.01;
            data.prices.push(price("FI", at, fi));
            for area in NEIGHBOR_AREAS {
                data.prices.push(price(area, at, fi + 0.1));
            }
            q += 1;
        }
        data
    }

    #[test]
    fn the_series_starts_at_the_cet_delivery_day_boundary() {
        let data = build_price_year();
        // (issue time, first quarter that Nord Pool has not published).
        // Delivery day D is published at 12:00 UTC on D − 1 and runs from
        // 00:00 CET/CEST to 24:00 CET/CEST.
        let cases = [
            ("2026-02-10T06:00:00Z", "2026-02-10T23:00:00Z"),
            ("2026-02-10T13:00:00Z", "2026-02-11T23:00:00Z"),
            ("2026-07-10T13:00:00Z", "2026-07-11T22:00:00Z"),
            ("2026-07-10T23:30:00Z", "2026-07-11T22:00:00Z"),
            // 2026-03-29 has 23 hours, 2026-10-25 has 25 hours.
            ("2026-03-28T13:00:00Z", "2026-03-29T22:00:00Z"),
            ("2026-10-24T13:00:00Z", "2026-10-25T23:00:00Z"),
        ];
        for (issue, want) in cases {
            let want = ms(want);
            for mode in [AsOfMode::IssueTime, AsOfMode::Latest] {
                let inputs = reconstruct_issue_time(&data, ms(issue), mode).unwrap();
                assert_eq!(inputs.origin_ms, want, "{issue}");
                assert_eq!(inputs.last_published_ms, Some(want - QUARTER_MS), "{issue}");
                let fc = forecast_at_issue_time(&inputs).unwrap();
                assert_eq!(fc.series_start_ms, want, "{issue}");
                let prices = inputs
                    .fi_prices
                    .keys()
                    .chain(inputs.neighbor_prices.values().flat_map(|s| s.keys()));
                for key in prices {
                    assert!(key.as_millisecond() < want, "{issue} {key:?}");
                }
                assert!(find_leaking_inputs(&inputs).is_empty(), "{issue}");
            }
        }
    }

    #[test]
    fn neighbour_prices_stop_at_the_fi_series_start() {
        let fi_until = ms("2026-02-11T12:00:00Z");
        let mut data = build_price_year();
        data.prices
            .retain(|p| p.area != "FI" || p.start.as_millisecond() < fi_until);
        let inputs =
            reconstruct_issue_time(&data, ms("2026-02-10T13:00:00Z"), AsOfMode::IssueTime).unwrap();
        assert_eq!(inputs.origin_ms, fi_until);
        // The neighbours have prices up to the publication horizon.
        assert!(data.prices.iter().any(|p| p.area != "FI"
            && (fi_until..ms("2026-02-11T23:00:00Z")).contains(&p.start.as_millisecond())));
        for (area, series) in &inputs.neighbor_prices {
            let last = series.keys().next_back().unwrap().as_millisecond();
            assert_eq!(last, fi_until - QUARTER_MS, "{area}");
        }
    }

    /// 46 days of prices. Before 2026-01-10, FI and neighbour prices sit
    /// 5 c/kWh higher, so any read of them moves the price mean.
    fn build_old_price_data() -> BacktestData {
        let old_until = ms("2026-01-10T00:00:00Z");
        let mut data = build_wind_gap_data();
        for p in &mut data.prices {
            if p.start.as_millisecond() < old_until {
                p.spot_cents_kwh += 5.0;
            }
        }
        data
    }

    /// The issue-time inputs with prices read like the served forecast:
    /// FI and neighbour prices in `[history_start, series_start)`.
    fn with_served_prices(data: &BacktestData, inputs: &IssueTimeInputs) -> IssueTimeInputs {
        let (history_start, _) = served_window(inputs);
        let window = history_start..inputs.origin_ms;
        let mut fi_prices = BTreeMap::new();
        let mut neighbor_prices: BTreeMap<String, BTreeMap<QuarterStart, f64>> = BTreeMap::new();
        for p in &data.prices {
            let start = p.start.as_millisecond();
            if !window.contains(&start) {
                continue;
            }
            let key = QuarterStart::from_millisecond(start);
            if p.area == "FI" {
                fi_prices.insert(key, p.spot_cents_kwh);
            } else {
                neighbor_prices
                    .entry(p.area.clone())
                    .or_default()
                    .insert(key, p.spot_cents_kwh);
            }
        }
        IssueTimeInputs {
            fi_prices,
            neighbor_prices,
            ..inputs.clone()
        }
    }

    #[test]
    fn issue_time_prices_stay_inside_the_served_window() {
        let data = build_old_price_data();
        for mode in [AsOfMode::IssueTime, AsOfMode::Latest] {
            let inputs = reconstruct_issue_time(&data, ms(GAP_ISSUE), mode).unwrap();
            let (history_start, _) = served_window(&inputs);
            assert!(
                data.prices
                    .iter()
                    .any(|p| p.start.as_millisecond() < history_start)
            );
            let served = with_served_prices(&data, &inputs);
            assert!(!served.fi_prices.is_empty());
            assert_eq!(
                served.neighbor_prices.keys().collect::<Vec<_>>(),
                ["EE", "SE1", "SE3"]
            );
            let first = |series: &BTreeMap<QuarterStart, f64>| {
                series.keys().next().map(|k| k.as_millisecond())
            };
            assert_eq!(first(&inputs.fi_prices), Some(history_start), "FI");
            for (area, series) in &inputs.neighbor_prices {
                assert_eq!(first(series), Some(history_start), "{area}");
            }
            assert_eq!(inputs.fi_prices, served.fi_prices);
            assert_eq!(inputs.neighbor_prices, served.neighbor_prices);
        }
    }

    #[test]
    fn older_prices_move_the_forecast_and_the_backtest_reads_the_served_window() {
        let data = build_old_price_data();
        let inputs = reconstruct_issue_time(&data, ms(GAP_ISSUE), AsOfMode::IssueTime).unwrap();
        let served = with_served_prices(&data, &inputs);
        let every_earlier_price = IssueTimeInputs {
            fi_prices: data
                .prices
                .iter()
                .filter(|p| p.area == "FI" && p.start.as_millisecond() < inputs.origin_ms)
                .map(|p| (QuarterStart::floor(p.start), p.spot_cents_kwh))
                .collect(),
            ..served.clone()
        };
        let served_fc = forecast_at_issue_time(&served).unwrap();
        // The fixture is sensitive: the older prices change the forecast.
        assert_ne!(
            forecast_at_issue_time(&every_earlier_price)
                .unwrap()
                .predicted,
            served_fc.predicted
        );
        assert_eq!(forecast_at_issue_time(&inputs).unwrap(), served_fc);
    }

    #[test]
    fn the_last_published_day_baseline_reads_the_day_before_the_series_start() {
        let data = build_data(0.0);
        let realized: BTreeMap<i64, f64> = data
            .prices
            .iter()
            .filter(|p| p.area == "FI")
            .map(|p| (p.start.as_millisecond(), p.spot_cents_kwh))
            .collect();
        let t = ms("2026-02-10T13:00:00Z");
        let options = BacktestOptions {
            restrict_origins_ms: Some(BTreeSet::from([t])),
            ..BacktestOptions::default()
        };
        let summary = run_backtest(&data, &options).unwrap();
        let inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime).unwrap();
        let fc = forecast_at_issue_time(&inputs).unwrap();
        let day_before = inputs.origin_ms - DAY_MS;
        let errors: Vec<f64> = fc
            .predicted
            .iter()
            .filter_map(|(key, _)| {
                let target = key.as_millisecond();
                let actual = realized.get(&target)?;
                let same_time = day_before + (target - inputs.origin_ms).rem_euclid(DAY_MS);
                Some((realized[&same_time] - actual).abs())
            })
            .collect();
        assert_eq!(errors.len(), summary.residuals.len());
        #[expect(clippy::cast_precision_loss, reason = "at most 288 errors")]
        let want = errors.iter().sum::<f64>() / errors.len() as f64;
        let got = summary.baseline_mae.last_published_day.unwrap();
        assert!((got - want).abs() < 1e-12, "{got} {want}");
    }

    #[test]
    fn at_the_cet_series_start_the_rank_metrics_react_to_the_variant() {
        let cmp =
            compare_variants(&build_data(0.0), ForecastVariant::default(), strong_ridge()).unwrap();
        let d1 = cmp.paired_by_horizon[0].spearman;
        assert!(d1.paired > 0, "{d1:?}");
        assert!(d1.wins + d1.losses > 0, "{d1:?}");
    }

    #[test]
    fn arm_b_never_scores_an_origin_that_arm_a_skipped() {
        let data = build_late_vintage_data();
        let cmp = compare_variants(&data, ForecastVariant::default(), strong_ridge()).unwrap();
        assert!(cmp.a.pre_vintage_origins > 0);
        assert!(cmp.shared_origins > 0);
        assert_eq!(cmp.b.scored_origins_ms, cmp.a.scored_origins_ms);
        assert_eq!(cmp.b.pre_vintage_origins, 0);
        assert!(cmp.origins_match);
    }
}
