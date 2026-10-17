//! Behaviour tests of the forecast, ported from the TypeScript test suite.

#[cfg(test)]
mod behaviour {
    use std::borrow::Cow;

    use jiff::Timestamp;
    use spot_price_domain::areas::Area;
    use spot_price_domain::forecast::{
        BandMethod, BandOffsets, CALIBRATED_BANDS, COVERAGE_GATE_THRESHOLD, CalibratedBands,
        DeriveOptions, FALLBACK_PREDICTION, FEATURE_COUNT, FEATURE_NAMES, FeatureContext,
        FitOptions, ForecastInput, ForecastOptions, GridRecord, HorizonResidual,
        LatestPublishedRule, LinearFitError, MIN_FIT_SAMPLES, MIN_TOTAL_CALIBRATION_SAMPLES,
        NEIGHBOR_AREAS, NOMINAL_COVERAGE, NeighborPrices, PriceTransform, QuarterMap, QuarterStart,
        SANITY_MIN_FALLBACK_CENTS_KWH, SanityBound, TRAINING_HISTORY_DAYS, TrainingData, UtcHour,
        align_series, apply_band, apply_within_day_shape, bucket_records, build_artifact,
        build_feature_vector, build_forecast, build_predicted_series, build_training_matrix,
        derive_band_offsets, expand_hourly_to_quarters, extend_with_last_week, fit_linear,
        fit_ridge, observed_coverage_of, sanity_bound_from_history, uncalibrated_bands,
    };

    const QUARTER_MS: i64 = 15 * 60 * 1000;
    const HOUR_MS: i64 = 60 * 60 * 1000;
    const DAY_MS: i64 = 24 * HOUR_MS;
    const WEEK_MS: i64 = 7 * DAY_MS;

    fn ms(text: &str) -> i64 {
        text.parse::<Timestamp>().unwrap().as_millisecond()
    }

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_millisecond(ms).unwrap()
    }

    fn q(ms: i64) -> QuarterStart {
        QuarterStart::from_millisecond(ms)
    }

    /// A small integer as a float.
    fn int(value: i64) -> f64 {
        f64::from(i32::try_from(value).unwrap())
    }

    /// A count as a float.
    fn count(value: usize) -> f64 {
        f64::from(u32::try_from(value).unwrap())
    }

    fn anchor() -> i64 {
        ms("2026-03-01T00:00:00Z")
    }

    fn rec(start_ms: i64, value: f64) -> GridRecord {
        GridRecord {
            start: ts(start_ms),
            value,
        }
    }

    fn map(entries: &[(i64, f64)]) -> QuarterMap {
        entries.iter().map(|&(k, v)| (q(k), v)).collect()
    }

    fn assert_close(got: f64, want: f64, tolerance: f64) {
        assert!((got - want).abs() < tolerance, "got {got}, want {want}");
    }

    /// Spearman rank correlation with averaged ties.
    fn spearman(xs: &[f64], ys: &[f64]) -> f64 {
        fn rank(values: &[f64]) -> Vec<f64> {
            let mut order: Vec<(f64, usize)> = values.iter().copied().zip(0..).collect();
            order.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut ranks = vec![0.0; values.len()];
            let mut i = 0;
            while i < order.len() {
                let mut j = i;
                while j + 1 < order.len() && order[j + 1].0.total_cmp(&order[i].0).is_eq() {
                    j += 1;
                }
                let average = count(i + j) / 2.0 + 1.0;
                for item in &order[i..=j] {
                    ranks[item.1] = average;
                }
                i = j + 1;
            }
            ranks
        }
        fn pearson(a: &[f64], b: &[f64]) -> f64 {
            let n = count(a.len());
            let ma = a.iter().sum::<f64>() / n;
            let mb = b.iter().sum::<f64>() / n;
            let (mut cov, mut va, mut vb) = (0.0, 0.0, 0.0);
            for (x, y) in a.iter().zip(b) {
                cov += (x - ma) * (y - mb);
                va += (x - ma) * (x - ma);
                vb += (y - mb) * (y - mb);
            }
            if va == 0.0 || vb == 0.0 {
                0.0
            } else {
                cov / (va * vb).sqrt()
            }
        }
        pearson(&rank(xs), &rank(ys))
    }

    // ---------------------------------------------------------------------------
    // Series helpers
    // ---------------------------------------------------------------------------

    #[test]
    fn bucketing_averages_samples_in_one_quarter() {
        let out = bucket_records(&[rec(anchor(), 10.0), rec(anchor() + 60_000, 20.0)]);
        assert_eq!(out.get(&q(anchor())), Some(&15.0));
    }

    #[test]
    fn bucketing_skips_non_finite_values() {
        let out = bucket_records(&[
            rec(anchor(), 10.0),
            rec(anchor(), f64::NAN),
            rec(anchor(), f64::INFINITY),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out.get(&q(anchor())), Some(&10.0));
    }

    #[test]
    fn hourly_values_fill_their_quarters() {
        let out = expand_hourly_to_quarters(&[rec(anchor(), 100.0)]);
        assert_eq!(out.len(), 4);
        for k in 0..4 {
            assert_eq!(out.get(&q(anchor() + k * QUARTER_MS)), Some(&100.0));
        }
    }

    #[test]
    fn quarter_values_are_not_overwritten() {
        let out =
            expand_hourly_to_quarters(&[rec(anchor(), 100.0), rec(anchor() + QUARTER_MS, 200.0)]);
        assert_eq!(out.get(&q(anchor())), Some(&100.0));
        assert_eq!(out.get(&q(anchor() + QUARTER_MS)), Some(&200.0));
        assert_eq!(out.get(&q(anchor() + 2 * QUARTER_MS)), Some(&100.0));
    }

    #[test]
    fn the_tail_takes_the_same_quarter_a_week_back() {
        let forecast = map(&[(anchor(), 50.0)]);
        let target = anchor() + QUARTER_MS;
        let actual = map(&[(target - WEEK_MS, 77.0)]);
        let out = extend_with_last_week(&forecast, &actual, anchor() + 2 * QUARTER_MS, 1);
        assert_eq!(out.get(&q(target)), Some(&77.0));
    }

    #[test]
    fn the_tail_averages_several_weeks() {
        let forecast = map(&[(anchor(), 50.0)]);
        let target = anchor() + QUARTER_MS;
        let actual = map(&[(target - WEEK_MS, 60.0), (target - 2 * WEEK_MS, 80.0)]);
        let out = extend_with_last_week(&forecast, &actual, anchor() + 2 * QUARTER_MS, 2);
        assert_eq!(out.get(&q(target)), Some(&70.0));
    }

    #[test]
    fn the_tail_leaves_quarters_without_history_unfilled() {
        let out = extend_with_last_week(
            &map(&[(anchor(), 50.0)]),
            &QuarterMap::new(),
            anchor() + 4 * QUARTER_MS,
            1,
        );
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn the_tail_of_an_empty_forecast_is_empty() {
        assert!(
            extend_with_last_week(&QuarterMap::new(), &QuarterMap::new(), anchor(), 1).is_empty()
        );
    }

    #[test]
    fn fit_linear_recovers_a_known_line() {
        let xs = [0.0, 1.0, 2.0, 3.0, 4.0];
        let ys: Vec<f64> = xs.iter().map(|x| 2.0 * x + 1.0).collect();
        let fit = fit_linear(&xs, &ys).unwrap();
        assert_close(fit.slope, 2.0, 1e-9);
        assert_close(fit.intercept, 1.0, 1e-9);
    }

    #[test]
    fn fit_linear_rejects_too_few_points_and_zero_variance() {
        assert_eq!(
            fit_linear(&[1.0], &[1.0]),
            Err(LinearFitError::TooFewPoints)
        );
        assert_eq!(
            fit_linear(&[1.0, 2.0], &[1.0]),
            Err(LinearFitError::TooFewPoints)
        );
        assert_eq!(
            fit_linear(&[5.0, 5.0, 5.0], &[1.0, 2.0, 3.0]),
            Err(LinearFitError::ZeroVariance)
        );
    }

    #[test]
    fn align_series_keeps_common_quarters_in_order() {
        let price = map(&[
            (anchor(), 10.0),
            (anchor() + QUARTER_MS, 20.0),
            (anchor() + 2 * QUARTER_MS, 30.0),
        ]);
        let residual = map(&[
            (anchor() + 2 * QUARTER_MS, 300.0),
            (anchor() + QUARTER_MS, 200.0),
        ]);
        let (xs, ys) = align_series(&price, &residual);
        assert_eq!(xs, vec![200.0, 300.0]);
        assert_eq!(ys, vec![20.0, 30.0]);
    }

    #[test]
    fn the_sanity_bound_is_outside_the_observed_range() {
        let mut history = QuarterMap::new();
        for h in 0..10 {
            history.insert(q(anchor() + h * HOUR_MS), int(h) + 2.0);
            history.insert(q(anchor() + h * HOUR_MS + QUARTER_MS), int(h) + 2.5);
        }
        let bound = sanity_bound_from_history(&history).unwrap();
        assert!(bound.min <= 0.0);
        assert!(bound.min < 2.0);
        assert!(bound.max > 11.5);
    }

    #[test]
    fn a_flat_history_uses_the_backstop() {
        let history: QuarterMap = (0..8).map(|h| (q(anchor() + h * HOUR_MS), 4.0)).collect();
        let bound = sanity_bound_from_history(&history).unwrap();
        assert!(bound.min <= SANITY_MIN_FALLBACK_CENTS_KWH);
    }

    #[test]
    fn no_finite_history_gives_no_bound() {
        assert_eq!(sanity_bound_from_history(&QuarterMap::new()), None);
        assert_eq!(
            sanity_bound_from_history(&map(&[(anchor(), f64::NAN)])),
            None
        );
    }

    /// One UTC day of 4 quarters: the ridge level falls, the 1-day lag rises.
    fn anti_phase_day() -> (QuarterMap, QuarterMap, Vec<QuarterStart>) {
        let ridge = [10.0, 9.0, 8.0, 7.0];
        let lag = [1.0, 2.0, 3.0, 4.0];
        let mut predicted = QuarterMap::new();
        let mut spot = QuarterMap::new();
        let mut keys = Vec::new();
        for k in 0..4 {
            let at = anchor() + k * QUARTER_MS;
            keys.push(q(at));
            predicted.insert(q(at), ridge[usize::try_from(k).unwrap()]);
            spot.insert(q(at - DAY_MS), lag[usize::try_from(k).unwrap()]);
        }
        (predicted, spot, keys)
    }

    #[test]
    fn the_within_day_order_equals_the_persistence_order() {
        let (predicted, spot, keys) = anti_phase_day();
        let shaped = apply_within_day_shape(&predicted, &spot);
        let out: Vec<f64> = keys.iter().map(|k| shaped.result[k]).collect();
        let lag: Vec<f64> = keys.iter().map(|k| spot[&k.offset(-DAY_MS)]).collect();
        assert_close(spearman(&out, &lag), 1.0, 1e-12);
    }

    #[test]
    fn the_within_day_shape_keeps_the_daily_mean() {
        let (predicted, spot, keys) = anti_phase_day();
        let shaped = apply_within_day_shape(&predicted, &spot);
        let ridge_mean = keys.iter().map(|k| predicted[k]).sum::<f64>() / 4.0;
        let out_mean = keys.iter().map(|k| shaped.result[k]).sum::<f64>() / 4.0;
        assert_close(out_mean, ridge_mean, 1e-12);
    }

    #[test]
    fn the_one_day_lag_wins_over_the_one_week_lag() {
        let at = anchor();
        let predicted = map(&[(at, 5.0), (at + QUARTER_MS, 5.0)]);
        let spot = map(&[(at - DAY_MS, 100.0), (at - WEEK_MS, 1.0)]);
        let shaped = apply_within_day_shape(&predicted, &spot);
        assert_close(shaped.result[&q(at)], 5.0, 1e-12);
        assert_close(shaped.result[&q(at + QUARTER_MS)], 5.0, 1e-12);
        assert_eq!(shaped.shape_fallback_quarters, 1);
    }

    #[test]
    fn a_quarter_without_lag_takes_the_flat_daily_level() {
        let at = anchor();
        let predicted = map(&[(at, 8.0), (at + QUARTER_MS, 12.0)]);
        let spot = map(&[(at - DAY_MS, 3.0)]);
        let shaped = apply_within_day_shape(&predicted, &spot);
        assert_close(shaped.result[&q(at)], 10.0, 1e-12);
        assert_close(shaped.result[&q(at + QUARTER_MS)], 10.0, 1e-12);
        assert_eq!(shaped.shape_fallback_quarters, 1);
    }

    #[test]
    fn the_within_day_shape_groups_by_utc_day_across_dst() {
        let day = ms("2026-03-29T00:00:00Z");
        let before = day + 30 * 60 * 1000;
        let after = day + 90 * 60 * 1000;
        let next = day + DAY_MS;
        let predicted = map(&[(before, 4.0), (after, 6.0), (next, 100.0)]);
        let spot = map(&[
            (before - DAY_MS, 1.0),
            (after - DAY_MS, 9.0),
            (next - DAY_MS, 50.0),
        ]);
        let shaped = apply_within_day_shape(&predicted, &spot);
        assert_close(
            f64::midpoint(shaped.result[&q(before)], shaped.result[&q(after)]),
            5.0,
            1e-12,
        );
        assert_close(shaped.result[&q(next)], 100.0, 1e-12);
    }

    #[test]
    fn the_predicted_series_forward_fills_gaps() {
        let predicted = map(&[(anchor(), 5.0), (anchor() + 2 * QUARTER_MS, 7.0)]);
        let built = build_predicted_series(&predicted, q(anchor()), 3);
        let values: Vec<f64> = built
            .series
            .iter()
            .map(|p| p.estimated_spot_cents_kwh)
            .collect();
        assert_eq!(values, vec![5.0, 5.0, 7.0]);
        assert_eq!(built.filled_quarters, 1);
        assert_eq!(built.zero_seeded_quarters, 0);
    }

    #[test]
    fn the_predicted_series_seeds_leading_gaps_ahead() {
        let built =
            build_predicted_series(&map(&[(anchor() + 2 * QUARTER_MS, 9.0)]), q(anchor()), 3);
        let values: Vec<f64> = built
            .series
            .iter()
            .map(|p| p.estimated_spot_cents_kwh)
            .collect();
        assert_eq!(values, vec![9.0, 9.0, 9.0]);
        assert_eq!(built.zero_seeded_quarters, 0);
    }

    #[test]
    fn the_predicted_series_zero_seeds_a_hard_outage() {
        let built = build_predicted_series(&QuarterMap::new(), q(anchor()), 2);
        let values: Vec<f64> = built
            .series
            .iter()
            .map(|p| p.estimated_spot_cents_kwh)
            .collect();
        assert_eq!(values, vec![0.0, 0.0]);
        assert_eq!(built.zero_seeded_quarters, 2);
    }

    // ---------------------------------------------------------------------------
    // The pipeline
    // ---------------------------------------------------------------------------

    struct Synthetic {
        spot: QuarterMap,
        wind_actual: Vec<GridRecord>,
        cons_actual: Vec<GridRecord>,
        wind_forecast: Vec<GridRecord>,
        cons_forecast: Vec<GridRecord>,
        series_start_ms: i64,
    }

    /// A month of history and future grid data; `history` and `future` return
    /// `(spot, consumption, wind)` for a quarter index.
    fn synthetic(
        history: impl Fn(i64) -> (f64, f64, f64),
        future_quarters: i64,
        future: impl Fn(i64) -> (f64, f64),
    ) -> Synthetic {
        let mut s = Synthetic {
            spot: QuarterMap::new(),
            wind_actual: Vec::new(),
            cons_actual: Vec::new(),
            wind_forecast: Vec::new(),
            cons_forecast: Vec::new(),
            series_start_ms: anchor() + 30 * 96 * QUARTER_MS,
        };
        for k in 0..30 * 96 {
            let at = anchor() + k * QUARTER_MS;
            let (price, consumption, wind) = history(k);
            s.spot.insert(q(at), price);
            s.cons_actual.push(rec(at, consumption));
            s.wind_actual.push(rec(at, wind));
        }
        s.cons_forecast.clone_from(&s.cons_actual);
        s.wind_forecast.clone_from(&s.wind_actual);
        for k in 0..future_quarters {
            let at = s.series_start_ms + k * QUARTER_MS;
            let (consumption, wind) = future(k);
            s.cons_forecast.push(rec(at, consumption));
            s.wind_forecast.push(rec(at, wind));
        }
        s
    }

    fn run(
        s: &Synthetic,
        days: i64,
        opts: &ForecastOptions<'_>,
    ) -> spot_price_domain::forecast::ForecastResult {
        let neighbours = NeighborPrices::new();
        build_forecast(
            &ForecastInput {
                spot_prices: &s.spot,
                neighbor_prices: &neighbours,
                wind_forecast: &s.wind_forecast,
                wind_actual: &s.wind_actual,
                consumption_forecast: &s.cons_forecast,
                consumption_actual: &s.cons_actual,
                series_start: ts(s.series_start_ms),
                series_end: ts(s.series_start_ms + days * DAY_MS),
            },
            opts,
        )
    }

    fn price_for(consumption: f64, wind: f64) -> f64 {
        0.001 * (consumption - wind) + 3.0
    }

    fn residual_month() -> Synthetic {
        synthetic(
            |k| {
                let consumption = 8000.0 + int(k % 96) * 20.0;
                let wind = 2000.0 + int((k * 137) % 1500);
                (price_for(consumption, wind), consumption, wind)
            },
            3 * 96,
            |k| (8500.0 + int(k % 96) * 18.0, 2200.0 + int((k * 113) % 1400)),
        )
    }

    #[test]
    fn the_pipeline_fits_and_tracks_the_truth() {
        let s = residual_month();
        let result = run(&s, 3, &ForecastOptions::default());
        assert!(!result.diagnostics.fit_used_default);
        assert!(result.diagnostics.fit_samples >= MIN_FIT_SAMPLES);
        assert_eq!(result.diagnostics.feature_count, FEATURE_COUNT);
        assert_eq!(result.series.len(), 3 * 96);
        assert_eq!(result.diagnostics.zero_seeded_quarters, 0);
        let mut abs_error = 0.0;
        for (k, point) in (0..).zip(&result.series) {
            let truth = price_for(8500.0 + int(k % 96) * 18.0, 2200.0 + int((k * 113) % 1400));
            abs_error += (point.estimated_spot_cents_kwh - truth).abs();
        }
        assert!(abs_error / count(result.series.len()) < 0.5);
    }

    fn moved_quarters(
        a: &spot_price_domain::forecast::ForecastResult,
        b: &spot_price_domain::forecast::ForecastResult,
    ) -> usize {
        assert_eq!(a.series.len(), b.series.len());
        a.series
            .iter()
            .zip(&b.series)
            .filter(|(x, y)| {
                x.estimated_spot_cents_kwh.to_bits() != y.estimated_spot_cents_kwh.to_bits()
            })
            .count()
    }

    #[test]
    fn the_latest_published_rule_reaches_the_forecast() {
        let s = residual_month();
        let production = run(&s, 3, &ForecastOptions::default());
        let at_or_before = ForecastOptions {
            latest_published: LatestPublishedRule::AtOrBefore,
            ..ForecastOptions::default()
        };
        assert_eq!(run(&s, 3, &at_or_before), production);
        let before = ForecastOptions {
            latest_published: LatestPublishedRule::Before,
            ..ForecastOptions::default()
        };
        let variant = run(&s, 3, &before);
        assert_eq!(variant.diagnostics, production.diagnostics);
        assert!(moved_quarters(&variant, &production) > 0);
    }

    #[test]
    fn the_pipeline_falls_back_with_too_little_history() {
        let spot = map(&[(anchor(), 5.0)]);
        let neighbours = NeighborPrices::new();
        let start = anchor() + 96 * QUARTER_MS;
        let result = build_forecast(
            &ForecastInput {
                spot_prices: &spot,
                neighbor_prices: &neighbours,
                wind_forecast: &[],
                wind_actual: &[],
                consumption_forecast: &[],
                consumption_actual: &[],
                series_start: ts(start),
                series_end: ts(start + DAY_MS),
            },
            &ForecastOptions::default(),
        );
        assert!(result.diagnostics.fit_used_default);
        assert!(result.diagnostics.fit_samples < MIN_FIT_SAMPLES);
        assert_eq!(result.diagnostics.zero_seeded_quarters, 0);
        let first = result.series[0].estimated_spot_cents_kwh;
        assert!(
            result
                .series
                .iter()
                .all(|p| p.estimated_spot_cents_kwh.to_bits() == first.to_bits())
        );
    }

    #[test]
    fn the_sanity_bound_clamps_and_counts() {
        let spot = QuarterMap::new();
        let neighbours = NeighborPrices::new();
        let cons = [rec(anchor(), 100.0), rec(anchor() + QUARTER_MS, 100.0)];
        let result = build_forecast(
            &ForecastInput {
                spot_prices: &spot,
                neighbor_prices: &neighbours,
                wind_forecast: &[],
                wind_actual: &[],
                consumption_forecast: &cons,
                consumption_actual: &[],
                series_start: ts(anchor()),
                series_end: ts(anchor() + 2 * QUARTER_MS),
            },
            &ForecastOptions {
                sanity_bound: Some(SanityBound {
                    min: -1.0,
                    max: 1.0,
                }),
                ..ForecastOptions::default()
            },
        );
        assert_eq!(result.diagnostics.sanity_clamped_quarters, 2);
        for point in &result.series {
            assert!((-1.0..=1.0).contains(&point.estimated_spot_cents_kwh));
        }
    }

    #[test]
    fn cheap_quarters_stay_distinct_below_zero() {
        let shape = |k: i64| 3.0 * ((2.0 * std::f64::consts::PI * int(k % 96)) / 96.0).sin();
        let s = synthetic(
            |k| {
                let consumption = 7000.0 + int(k / 96) * 3.0;
                (
                    0.001 * (consumption - 6500.0) + 0.5 + shape(k),
                    consumption,
                    6500.0,
                )
            },
            96,
            |_| (7100.0, 6500.0),
        );
        let result = run(&s, 1, &ForecastOptions::default());
        let values: Vec<f64> = result
            .series
            .iter()
            .map(|p| p.estimated_spot_cents_kwh)
            .collect();
        assert_eq!(result.diagnostics.sanity_clamped_quarters, 0);
        assert!(values.iter().any(|&v| v < 0.0));
        let distinct = |vs: &[f64]| {
            let mut sorted = vs.to_vec();
            sorted.sort_by(f64::total_cmp);
            let mut cheapest: Vec<u64> = sorted[..16].iter().map(|v| v.to_bits()).collect();
            cheapest.dedup();
            cheapest.len()
        };
        let unclamped = distinct(&values);
        assert!(unclamped >= 8);
        let floored: Vec<f64> = values.iter().map(|v| v.max(0.0)).collect();
        assert!(distinct(&floored) < unclamped);
    }

    #[test]
    fn the_within_day_ranking_is_in_phase_with_the_lag() {
        let shape = |k: i64| 4.0 * ((2.0 * std::f64::consts::PI * int((k % 96) - 24)) / 96.0).sin();
        let s = synthetic(
            |k| {
                let consumption = 8000.0 + int(k / 96) * 5.0;
                (
                    0.001 * (consumption - 2000.0) + 10.0 + shape(k),
                    consumption,
                    2000.0,
                )
            },
            96,
            |_| (8200.0, 2000.0),
        );
        let result = run(&s, 1, &ForecastOptions::default());
        let mut out = Vec::new();
        let mut lag = Vec::new();
        for point in &result.series {
            let persistence = s
                .spot
                .get(&point.start.offset(-DAY_MS))
                .or_else(|| s.spot.get(&point.start.offset(-WEEK_MS)));
            if let Some(&p) = persistence {
                out.push(point.estimated_spot_cents_kwh);
                lag.push(p);
            }
        }
        assert!(out.len() > 80);
        assert!(spearman(&out, &lag) > 0.9);
        assert_eq!(result.diagnostics.shape_fallback_quarters, 0);
    }

    fn calibrated_global(global: BandOffsets) -> CalibratedBands {
        CalibratedBands {
            method: BandMethod::EmpiricalResidual,
            nominal_coverage: NOMINAL_COVERAGE,
            observed_coverage: Some(0.82),
            calibrated: true,
            offsets_by_hour: [None; 24],
            global_offsets: Some(global),
            generated_at: Cow::Borrowed("2026-06-01T00:00:00.000Z"),
        }
    }

    fn band_month() -> Synthetic {
        synthetic(
            |k| {
                let consumption = 8000.0 + int(k % 96) * 20.0;
                let wind = 2000.0 + int((k * 137) % 1500);
                (price_for(consumption, wind), consumption, wind)
            },
            3 * 96,
            |_| (8500.0, 2500.0),
        )
    }

    #[test]
    fn dark_bands_emit_no_bounds() {
        let dark = uncalibrated_bands();
        let result = run(
            &band_month(),
            3,
            &ForecastOptions {
                bands: &dark,
                ..ForecastOptions::default()
            },
        );
        assert!(!result.diagnostics.band_calibrated);
        assert_eq!(result.diagnostics.band_hour_buckets, 0);
        assert!(result.series.iter().all(|p| p.band.is_none()));
    }

    #[test]
    fn calibrated_bands_bound_every_predicted_quarter() {
        let bands = calibrated_global(BandOffsets {
            low_offset: -0.4,
            high_offset: 0.4,
        });
        let result = run(
            &band_month(),
            3,
            &ForecastOptions {
                bands: &bands,
                ..ForecastOptions::default()
            },
        );
        assert!(result.diagnostics.band_calibrated);
        assert_eq!(result.diagnostics.band_hour_buckets, result.series.len());
        for point in &result.series {
            let band = point.band.unwrap();
            assert!(band.low <= point.estimated_spot_cents_kwh);
            assert!(band.high >= point.estimated_spot_cents_kwh);
        }
    }

    #[test]
    fn bands_go_exactly_on_quarters_with_a_prediction() {
        let bands = calibrated_global(BandOffsets {
            low_offset: -0.4,
            high_offset: 0.4,
        });
        let mut s = band_month();
        s.wind_forecast.clone_from(&s.wind_actual);
        s.cons_forecast.clone_from(&s.cons_actual);
        let result = run(
            &s,
            3,
            &ForecastOptions {
                bands: &bands,
                ..ForecastOptions::default()
            },
        );
        let banded = result.series.iter().filter(|p| p.band.is_some()).count();
        assert_eq!(banded, result.diagnostics.band_hour_buckets);
        assert_eq!(result.diagnostics.zero_seeded_quarters, 0);
    }

    // ---------------------------------------------------------------------------
    // Features
    // ---------------------------------------------------------------------------

    fn wed_noon() -> i64 {
        ms("2026-03-04T12:00:00Z")
    }

    fn column(name: &str) -> usize {
        FEATURE_NAMES.iter().position(|n| *n == name).unwrap()
    }

    struct Ctx {
        fi: QuarterMap,
        neighbours: NeighborPrices,
        wind: QuarterMap,
        consumption: QuarterMap,
    }

    impl Ctx {
        fn empty() -> Self {
            Self {
                fi: QuarterMap::new(),
                neighbours: NeighborPrices::new(),
                wind: QuarterMap::new(),
                consumption: QuarterMap::new(),
            }
        }

        fn get(&self) -> FeatureContext<'_> {
            FeatureContext {
                fi_prices: &self.fi,
                neighbor_prices: &self.neighbours,
                wind: &self.wind,
                consumption: &self.consumption,
            }
        }
    }

    #[test]
    fn a_vector_has_every_column() {
        assert_eq!(
            build_feature_vector(q(wed_noon()), &Ctx::empty().get()).len(),
            FEATURE_NAMES.len()
        );
    }

    #[test]
    fn the_residual_and_interaction_come_from_wind_and_consumption() {
        let mut ctx = Ctx::empty();
        ctx.wind.insert(q(wed_noon()), 2000.0);
        ctx.consumption.insert(q(wed_noon()), 9000.0);
        let v = build_feature_vector(q(wed_noon()), &ctx.get());
        assert_eq!(v[column("wind_mw")], 2000.0);
        assert_eq!(v[column("consumption_mw")], 9000.0);
        assert_eq!(v[column("residual_mw")], 7000.0);
        assert_eq!(v[column("wind_x_consumption")], 2000.0 * 9000.0);
    }

    #[test]
    fn calendar_features_are_utc_and_cyclical() {
        let v = build_feature_vector(q(wed_noon()), &Ctx::empty().get());
        let tau = 2.0 * std::f64::consts::PI;
        assert_close(v[column("hour_sin")], 0.0, 1e-9);
        assert_close(v[column("hour_cos")], -1.0, 1e-9);
        assert_close(v[column("dow_sin")], (tau * 3.0 / 7.0).sin(), 1e-9);
        assert_close(v[column("dow_cos")], (tau * 3.0 / 7.0).cos(), 1e-9);
        assert_eq!(v[column("is_weekend")], 0.0);
        for weekend in ["2026-03-07T08:00:00Z", "2026-03-08T08:00:00Z"] {
            assert_eq!(
                build_feature_vector(q(ms(weekend)), &Ctx::empty().get())[column("is_weekend")],
                1.0
            );
        }
    }

    #[test]
    fn fi_lags_and_previous_day_statistics() {
        let mut ctx = Ctx::empty();
        ctx.fi.insert(q(wed_noon() - WEEK_MS), 22.0);
        let previous_day = wed_noon().div_euclid(DAY_MS) * DAY_MS - DAY_MS;
        ctx.fi.insert(q(previous_day + 4 * HOUR_MS), 1.0);
        ctx.fi.insert(q(previous_day + 8 * HOUR_MS), 3.0);
        ctx.fi.insert(q(previous_day + 12 * HOUR_MS), 11.0);
        let v = build_feature_vector(q(wed_noon()), &ctx.get());
        assert_eq!(v[column("fi_same_quarter_1d")], 11.0);
        assert_eq!(v[column("fi_same_quarter_7d")], 22.0);
        assert_eq!(v[column("fi_prev_day_min")], 1.0);
        assert_eq!(v[column("fi_prev_day_max")], 11.0);
        assert_close(v[column("fi_prev_day_mean")], 5.0, 1e-9);
        assert_eq!(v[column("fi_latest_published")], 11.0);
    }

    #[test]
    fn neighbour_lags_are_read() {
        let mut ctx = Ctx::empty();
        ctx.neighbours.insert(
            Area::Se1,
            map(&[(wed_noon() - DAY_MS, 7.0), (wed_noon() - WEEK_MS, 9.0)]),
        );
        let v = build_feature_vector(q(wed_noon()), &ctx.get());
        assert_eq!(v[column("se1_1d")], 7.0);
        assert_eq!(v[column("se1_7d")], 9.0);
    }

    #[test]
    fn missing_neighbours_take_the_fi_mean() {
        let mut ctx = Ctx::empty();
        ctx.fi = map(&[
            (wed_noon() - 2 * DAY_MS, 4.0),
            (wed_noon() - 3 * DAY_MS, 6.0),
        ]);
        let v = build_feature_vector(q(wed_noon()), &ctx.get());
        for area in NEIGHBOR_AREAS {
            let code = area.code().to_lowercase();
            assert_eq!(v[column(&format!("{code}_1d"))], 5.0);
            assert_eq!(v[column(&format!("{code}_7d"))], 5.0);
        }
    }

    #[test]
    fn no_fi_history_gives_a_zero_fill() {
        let v = build_feature_vector(q(wed_noon()), &Ctx::empty().get());
        assert_eq!(v[column("se1_1d")], 0.0);
        assert_eq!(v[column("fi_latest_published")], 0.0);
    }

    #[test]
    fn training_has_one_row_per_window_quarter() {
        let mut ctx = Ctx::empty();
        for i in 1..=200 {
            ctx.fi.insert(q(wed_noon() - i * QUARTER_MS), int(i % 10));
        }
        let built = build_training_matrix(ts(wed_noon()), &ctx.get(), None);
        assert_eq!(built.feature_names, &FEATURE_NAMES);
        assert_eq!(built.targets.len(), 200);
        assert_eq!(built.rows.len(), 200);
        assert!(
            built
                .rows
                .iter()
                .all(|row| row.len() == FEATURE_NAMES.len())
        );
    }

    #[test]
    fn training_excludes_the_origin_the_future_and_old_quarters() {
        let origin = wed_noon();
        let mut ctx = Ctx::empty();
        ctx.fi = map(&[
            (origin, 99.0),
            (origin + QUARTER_MS, 99.0),
            (origin - QUARTER_MS, 5.0),
            (origin - (TRAINING_HISTORY_DAYS + 1) * DAY_MS, 7.0),
        ]);
        assert_eq!(
            build_training_matrix(ts(origin), &ctx.get(), None).targets,
            vec![5.0]
        );
    }

    #[test]
    fn a_training_row_equals_the_prediction_vector() {
        let target = wed_noon() - QUARTER_MS;
        let mut ctx = Ctx::empty();
        ctx.fi.insert(q(target), 3.0);
        ctx.wind.insert(q(target), 1500.0);
        ctx.consumption.insert(q(target), 8000.0);
        let built = build_training_matrix(ts(wed_noon()), &ctx.get(), None);
        assert_eq!(built.rows[0], build_feature_vector(q(target), &ctx.get()));
    }

    // ---------------------------------------------------------------------------
    // The ridge model
    // ---------------------------------------------------------------------------

    fn fit(
        rows: &[Vec<f64>],
        targets: &[f64],
        feature_count: usize,
        opts: FitOptions,
    ) -> spot_price_domain::forecast::FittedModel {
        fit_ridge(
            &TrainingData {
                feature_count,
                rows,
                targets,
            },
            &opts,
        )
    }

    fn ols() -> FitOptions {
        FitOptions {
            ridge_lambda: 0.0,
            price_transform: PriceTransform::None,
        }
    }

    #[test]
    fn the_model_falls_back_below_the_minimum_samples() {
        let rows: Vec<Vec<f64>> = (0..MIN_FIT_SAMPLES - 1).map(|i| vec![count(i)]).collect();
        let targets: Vec<f64> = rows.iter().map(|r| r[0] * 2.0).collect();
        let fitted = fit(&rows, &targets, 1, FitOptions::default());
        assert!(fitted.meta.used_fallback);
        assert_eq!(fitted.meta.sample_count, MIN_FIT_SAMPLES - 1);
        assert_eq!(fitted.predict(&[10.0]), FALLBACK_PREDICTION);
    }

    #[test]
    fn the_model_falls_back_with_zero_features() {
        let rows = vec![Vec::new(); 50];
        let fitted = fit(&rows, &[1.0; 50], 0, FitOptions::default());
        assert!(fitted.meta.used_fallback);
        assert_eq!(fitted.predict(&[]), FALLBACK_PREDICTION);
    }

    #[test]
    fn the_model_falls_back_on_a_singular_system() {
        let rows: Vec<Vec<f64>> = (0..40).map(|i| vec![f64::from(i), f64::from(i)]).collect();
        let targets: Vec<f64> = rows.iter().map(|r| r[0] + 1.0).collect();
        assert!(fit(&rows, &targets, 2, ols()).meta.used_fallback);
    }

    #[test]
    fn lambda_zero_recovers_an_exact_linear_target() {
        let rows: Vec<Vec<f64>> = (0..60)
            .map(|i| vec![f64::from(i), f64::from((i * 7) % 13)])
            .collect();
        let targets: Vec<f64> = rows.iter().map(|r| 3.0 * r[0] - 2.0 * r[1] + 5.0).collect();
        let fitted = fit(&rows, &targets, 2, ols());
        assert!(!fitted.meta.used_fallback);
        for (x1, x2) in [(100.0, 3.0), (-5.0, 11.0), (1000.0, 0.0)] {
            assert_close(fitted.predict(&[x1, x2]), 3.0 * x1 - 2.0 * x2 + 5.0, 1e-6);
        }
    }

    #[test]
    fn lambda_zero_matches_a_hand_computed_line() {
        let base = [(1.0, 3.0), (2.0, 5.0), (3.0, 7.0), (4.0, 9.0)];
        let rows: Vec<Vec<f64>> = (0..10)
            .flat_map(|_| base.iter().map(|&(x, _)| vec![x]))
            .collect();
        let targets: Vec<f64> = (0..10).flat_map(|_| base.iter().map(|&(_, y)| y)).collect();
        let fitted = fit(&rows, &targets, 1, ols());
        assert_close(fitted.predict(&[0.0]), 1.0, 1e-6);
        assert_close(fitted.predict(&[5.0]), 11.0, 1e-6);
        assert_close(fitted.predict(&[10.0]), 21.0, 1e-6);
    }

    #[test]
    fn a_large_lambda_shrinks_toward_the_mean() {
        let rows: Vec<Vec<f64>> = (0..60).map(|i| vec![f64::from(i)]).collect();
        let targets: Vec<f64> = (0..60).map(|i| 2.0 * f64::from(i) + 1.0).collect();
        let mean = targets.iter().sum::<f64>() / 60.0;
        let small = fit(
            &rows,
            &targets,
            1,
            FitOptions {
                ridge_lambda: 0.001,
                price_transform: PriceTransform::None,
            },
        );
        let large = fit(
            &rows,
            &targets,
            1,
            FitOptions {
                ridge_lambda: 1e6,
                price_transform: PriceTransform::None,
            },
        );
        let (s, l) = (small.predict(&[200.0]), large.predict(&[200.0]));
        assert!((l - mean).abs() < (s - mean).abs());
        assert_close(l, mean, 0.5);
    }

    #[test]
    fn the_arcsinh_transform_round_trips() {
        for x in [-1234.5_f64, -3.2, -0.01, 0.0, 0.01, 3.2, 1234.5] {
            assert_close(x.asinh().sinh(), x, 1e-6);
        }
    }

    #[test]
    fn a_linear_target_in_arcsinh_space_is_recovered() {
        let rows: Vec<Vec<f64>> = (0..60).map(|i| vec![f64::from(i) / 10.0]).collect();
        let targets: Vec<f64> = rows.iter().map(|r| (0.5 * r[0] + 2.0).sinh()).collect();
        let fitted = fit(
            &rows,
            &targets,
            1,
            FitOptions {
                ridge_lambda: 0.0,
                price_transform: PriceTransform::Arcsinh,
            },
        );
        assert!(!fitted.meta.used_fallback);
        for x in [0.0_f64, 2.5, 5.0] {
            assert_close(fitted.predict(&[x]), (0.5 * x + 2.0).sinh(), 1e-4);
        }
    }

    #[test]
    fn standardization_cancels_units() {
        let targets: Vec<f64> = (0..60).map(|i| 4.0 * f64::from(i) + 7.0).collect();
        let raw: Vec<Vec<f64>> = (0..60).map(|i| vec![f64::from(i)]).collect();
        let scaled: Vec<Vec<f64>> = (0..60).map(|i| vec![1000.0 + 3.0 * f64::from(i)]).collect();
        let raw_fit = fit(&raw, &targets, 1, ols());
        let scaled_fit = fit(&scaled, &targets, 1, ols());
        assert_close(
            raw_fit.predict(&[30.0]),
            scaled_fit.predict(&[1090.0]),
            1e-4,
        );
        assert_close(raw_fit.predict(&[30.0]), 127.0, 1e-4);
    }

    #[test]
    fn a_constant_column_does_not_produce_nan() {
        let rows: Vec<Vec<f64>> = (0..60).map(|i| vec![f64::from(i), 42.0]).collect();
        let targets: Vec<f64> = (0..60).map(|i| 2.0 * f64::from(i) + 1.0).collect();
        let fitted = fit(
            &rows,
            &targets,
            2,
            FitOptions {
                ridge_lambda: 0.0001,
                price_transform: PriceTransform::None,
            },
        );
        assert!(!fitted.meta.used_fallback);
        let p = fitted.predict(&[10.0, 42.0]);
        assert!(p.is_finite());
        assert_close(p, 21.0, 0.5);
    }

    // ---------------------------------------------------------------------------
    // Bands
    // ---------------------------------------------------------------------------

    fn hour(h: u8) -> UtcHour {
        UtcHour::new(h).unwrap()
    }

    fn calibrated(
        by_hour: &[(usize, BandOffsets)],
        global: Option<BandOffsets>,
    ) -> CalibratedBands {
        let mut offsets_by_hour = [None; 24];
        for &(h, o) in by_hour {
            offsets_by_hour[h] = Some(o);
        }
        CalibratedBands {
            method: BandMethod::EmpiricalResidual,
            nominal_coverage: NOMINAL_COVERAGE,
            observed_coverage: Some(0.85),
            calibrated: true,
            offsets_by_hour,
            global_offsets: global,
            generated_at: Cow::Borrowed("2026-06-01T00:00:00.000Z"),
        }
    }

    fn offsets(low: f64, high: f64) -> BandOffsets {
        BandOffsets {
            low_offset: low,
            high_offset: high,
        }
    }

    #[test]
    fn utc_hour_rejects_out_of_range_values() {
        assert!(UtcHour::new(24).is_none());
        assert_eq!(UtcHour::new(23).map(UtcHour::get), Some(23));
    }

    #[test]
    fn dark_bands_give_no_band() {
        assert_eq!(apply_band(4.0, hour(12), &uncalibrated_bands()), None);
    }

    #[test]
    fn calibrated_bands_without_offsets_give_no_band() {
        assert_eq!(apply_band(4.0, hour(12), &calibrated(&[], None)), None);
    }

    #[test]
    fn equal_arcsinh_offsets_give_a_wider_top() {
        let band = apply_band(
            5.0,
            hour(12),
            &calibrated(&[(12, offsets(-0.5, 0.5))], None),
        )
        .unwrap();
        assert!(band.high - 5.0 > 5.0 - band.low);
        assert!(band.low <= 5.0 && band.high >= 5.0);
    }

    #[test]
    fn an_hour_without_offsets_uses_the_global_offsets() {
        let bands = calibrated(&[(0, offsets(-1.0, 1.0))], Some(offsets(-0.3, 0.3)));
        let band = apply_band(4.0, hour(18), &bands).unwrap();
        assert_close(band.low, (4.0_f64.asinh() - 0.3).sinh(), 1e-3);
        assert_close(band.high, (4.0_f64.asinh() + 0.3).sinh(), 1e-3);
    }

    #[test]
    fn a_negative_point_keeps_an_ordered_band() {
        let band = apply_band(
            -3.0,
            hour(12),
            &calibrated(&[(12, offsets(-2.0, 1.0))], None),
        )
        .unwrap();
        assert!(band.low <= -3.0 && band.high >= -3.0 && band.low < 0.0);
    }

    #[test]
    fn distinct_negative_points_keep_distinct_lower_bounds() {
        let bands = calibrated(&[(12, offsets(-0.5, 0.5))], None);
        let a = apply_band(-1.0, hour(12), &bands).unwrap();
        let b = apply_band(-4.0, hour(12), &bands).unwrap();
        assert!(b.low < a.low);
    }

    #[test]
    fn a_zero_width_band_keeps_the_order() {
        let band = apply_band(
            3.333,
            hour(12),
            &calibrated(&[(12, offsets(0.0, 0.0))], None),
        )
        .unwrap();
        assert!(band.low <= 3.333 && band.high >= 3.333);
    }

    fn residuals_at(utc_hour: u8, arcsinh_residuals: &[f64]) -> Vec<HorizonResidual> {
        arcsinh_residuals
            .iter()
            .map(|r| HorizonResidual {
                utc_hour: hour(utc_hour),
                predicted_raw: 5.0,
                actual_raw: (5.0_f64.asinh() + r).sinh(),
            })
            .collect()
    }

    fn spread(n: u32, from: f64, to: f64) -> Vec<f64> {
        (0..n)
            .map(|i| from + (to - from) * f64::from(i) / f64::from(n - 1))
            .collect()
    }

    fn round_robin(arcsinh_residuals: &[f64]) -> Vec<HorizonResidual> {
        arcsinh_residuals
            .iter()
            .zip((0..24_u8).cycle())
            .map(|(r, h)| HorizonResidual {
                utc_hour: hour(h),
                predicted_raw: 5.0,
                actual_raw: (5.0_f64.asinh() + r).sinh(),
            })
            .collect()
    }

    #[test]
    fn derived_offsets_cover_about_the_nominal_share() {
        let residuals = residuals_at(12, &spread(100, -1.0, 1.0));
        let derived = derive_band_offsets(&residuals, &DeriveOptions::default());
        let coverage =
            observed_coverage_of(&residuals, &derived.offsets_by_hour, derived.global_offsets);
        assert!((0.78..=0.92).contains(&coverage), "{coverage}");
        let own = derived.offsets_by_hour[12].unwrap();
        assert!(own.low_offset <= 0.0 && own.high_offset >= 0.0);
    }

    #[test]
    fn thin_hours_pool_to_the_global_offsets() {
        let derived = derive_band_offsets(
            &residuals_at(3, &spread(5, -0.5, 0.5)),
            &DeriveOptions::default(),
        );
        assert!(derived.offsets_by_hour[3].is_none());
        assert!(derived.global_offsets.is_some());
    }

    #[test]
    fn too_few_samples_ship_dark() {
        let artifact = build_artifact(
            &round_robin(&spread(
                u32::try_from(MIN_TOTAL_CALIBRATION_SAMPLES).unwrap() - 1,
                -1.0,
                1.0,
            )),
            "2026-06-01T00:00:00.000Z".into(),
        );
        assert!(!artifact.calibrated);
        assert_eq!(artifact.hour_count(), 0);
        assert_eq!(artifact.global_offsets, None);
        assert_eq!(artifact.generated_at, "");
    }

    #[test]
    fn well_behaved_residuals_ship_calibrated() {
        let artifact = build_artifact(
            &round_robin(&spread(480, -1.0, 1.0)),
            "2026-06-01T00:00:00.000Z".into(),
        );
        assert!(artifact.observed_coverage.unwrap() >= COVERAGE_GATE_THRESHOLD);
        assert!(artifact.calibrated);
        assert!(artifact.hour_count() > 0);
        assert_eq!(artifact.generated_at, "2026-06-01T00:00:00.000Z");
    }

    #[test]
    fn tight_offsets_under_cover_a_wider_set() {
        let narrow = derive_band_offsets(
            &round_robin(&spread(200, -0.05, 0.05)),
            &DeriveOptions::default(),
        );
        let coverage = observed_coverage_of(
            &round_robin(&spread(200, -3.0, 3.0)),
            &narrow.offsets_by_hour,
            narrow.global_offsets,
        );
        assert!(coverage < COVERAGE_GATE_THRESHOLD);
    }

    #[test]
    fn coverage_of_nothing_is_zero() {
        assert_eq!(observed_coverage_of(&[], &[None; 24], None), 0.0);
        assert_eq!(
            observed_coverage_of(&residuals_at(1, &[0.1]), &[None; 24], None),
            0.0
        );
    }

    #[test]
    fn the_committed_artifact_is_consistent_with_the_gate() {
        assert_eq!(CALIBRATED_BANDS.method.as_str(), "empirical-residual");
        if CALIBRATED_BANDS.calibrated {
            assert!(CALIBRATED_BANDS.observed_coverage.unwrap_or(0.0) >= COVERAGE_GATE_THRESHOLD);
            assert_eq!(CALIBRATED_BANDS.nominal_coverage, NOMINAL_COVERAGE);
            assert!(CALIBRATED_BANDS.hour_count() > 0);
            assert!(!CALIBRATED_BANDS.generated_at.is_empty());
        } else {
            assert_eq!(CALIBRATED_BANDS.hour_count(), 0);
            assert_eq!(CALIBRATED_BANDS.global_offsets, None);
            assert_eq!(CALIBRATED_BANDS.observed_coverage, None);
            assert_eq!(CALIBRATED_BANDS.generated_at, "");
        }
    }
}
