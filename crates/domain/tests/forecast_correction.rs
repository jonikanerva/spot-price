//! The quarter correction: an opt-in option that the served forecast does
//! not use. The fixture is synthetic; the price falls with the wind. The
//! tests check behaviour and information boundaries, not predictive gain.

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "a holiday indicator is exactly 0 or 1")]
mod correction {
    use std::collections::BTreeMap;
    use std::f64::consts::PI;

    use jiff::Timestamp;
    use jiff::tz::TimeZone;
    use spot_price_domain::areas::Area;
    use spot_price_domain::forecast::{
        CORRECTION_HISTORY_DAYS, CorrectionExamples, Decomposition, ForecastInput, ForecastOptions,
        ForecastResult, GridRecord, GridVintage, HOLIDAY_FEATURE_NAME, IssuanceHistory,
        NeighborPrices, QuarterCorrection, QuarterMap, QuarterStart, ShapeProfile,
        build_correction_examples, build_forecast, correction_feature_names, profile_values,
        publication_horizon,
    };

    const QUARTER_MS: i64 = 15 * 60 * 1000;
    const HOUR_MS: i64 = 60 * 60 * 1000;
    const DAY_MS: i64 = 24 * HOUR_MS;

    fn ms(text: &str) -> i64 {
        text.parse::<Timestamp>().unwrap().as_millisecond()
    }

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_millisecond(ms).unwrap()
    }

    fn q(ms: i64) -> QuarterStart {
        QuarterStart::from_millisecond(ms)
    }

    fn oslo() -> TimeZone {
        TimeZone::get("Europe/Oslo").unwrap()
    }

    fn helsinki() -> TimeZone {
        TimeZone::get("Europe/Helsinki").unwrap()
    }

    fn int(value: i64) -> f64 {
        f64::from(i32::try_from(value).unwrap())
    }

    fn wave(at_ms: i64, period_ms: i64) -> f64 {
        (2.0 * PI * int(at_ms.rem_euclid(period_ms) / 60_000) / int(period_ms / 60_000)).sin()
    }

    fn wind_actual(at_ms: i64) -> f64 {
        2500.0 + 1500.0 * wave(at_ms, 37 * HOUR_MS)
    }

    fn consumption_actual(at_ms: i64) -> f64 {
        9000.0 + 1000.0 * wave(at_ms, DAY_MS)
    }

    fn price(at_ms: i64) -> f64 {
        let level = int(at_ms.div_euclid(DAY_MS).rem_euclid(5)) * 0.3;
        6.0 + 2.0 * wave(at_ms, DAY_MS) - wind_actual(at_ms) / 1000.0 + level
    }

    struct Store {
        fi: QuarterMap,
        neighbours: NeighborPrices,
        wind_actual: Vec<GridRecord>,
        consumption_actual: Vec<GridRecord>,
        wind_vintages: Vec<GridVintage>,
        consumption_vintages: Vec<GridVintage>,
    }

    /// Prices and actuals from `from_ms`, and a 245/165 issuance every 3
    /// hours from `vintage_from_ms` (245: 6 h back to 72 h ahead, biased by
    /// the lead; 165: 6 h back to 30 h ahead).
    fn store(from_ms: i64, to_ms: i64, vintage_from_ms: i64) -> Store {
        let mut s = Store {
            fi: QuarterMap::new(),
            neighbours: NeighborPrices::new(),
            wind_actual: Vec::new(),
            consumption_actual: Vec::new(),
            wind_vintages: Vec::new(),
            consumption_vintages: Vec::new(),
        };
        let mut at = from_ms;
        while at < to_ms {
            s.fi.insert(q(at), price(at));
            for (area, scale) in [(Area::Se1, 0.7), (Area::Se3, 0.9), (Area::Ee, 1.1)] {
                s.neighbours
                    .entry(area)
                    .or_default()
                    .insert(q(at), price(at) * scale);
            }
            s.wind_actual.push(GridRecord {
                start: ts(at),
                value: wind_actual(at),
            });
            if at.rem_euclid(HOUR_MS) == 0 {
                s.consumption_actual.push(GridRecord {
                    start: ts(at),
                    value: consumption_actual(at),
                });
            }
            at += QUARTER_MS;
        }
        let mut issued = vintage_from_ms;
        while issued < to_ms {
            let mut target = issued - 6 * HOUR_MS;
            while target < issued + 72 * HOUR_MS {
                let lead_h = int((target - issued) / HOUR_MS);
                s.wind_vintages.push(GridVintage {
                    issued_at: ts(issued),
                    start: ts(target),
                    value: wind_actual(target) - 4.0 * lead_h,
                });
                if target < issued + 30 * HOUR_MS {
                    s.consumption_vintages.push(GridVintage {
                        issued_at: ts(issued),
                        start: ts(target),
                        value: consumption_actual(target) + lead_h,
                    });
                }
                target += QUARTER_MS;
            }
            issued += 3 * HOUR_MS;
        }
        s
    }

    /// The inputs of a run at `t_ms`: the served window, and the wider
    /// window that the correction examples read.
    struct Run {
        t_ms: i64,
        start_ms: i64,
        fi: QuarterMap,
        neighbours: NeighborPrices,
        wind_forecast: Vec<GridRecord>,
        consumption_forecast: Vec<GridRecord>,
        wind_actual: Vec<GridRecord>,
        consumption_actual: Vec<GridRecord>,
        wide_fi: QuarterMap,
        wide_neighbours: NeighborPrices,
        wide_wind_actual: Vec<GridRecord>,
        wide_consumption_actual: Vec<GridRecord>,
        wind_vintages: Vec<GridVintage>,
        consumption_vintages: Vec<GridVintage>,
    }

    fn latest_before(vintages: &[GridVintage], t_ms: i64, from: i64, to: i64) -> Vec<GridRecord> {
        let mut by_target: BTreeMap<i64, GridVintage> = BTreeMap::new();
        for v in vintages {
            let start = v.start.as_millisecond();
            if v.issued_at.as_millisecond() >= t_ms || !(from..to).contains(&start) {
                continue;
            }
            let slot = by_target.entry(start).or_insert(*v);
            if v.issued_at > slot.issued_at {
                *slot = *v;
            }
        }
        by_target
            .values()
            .map(|v| GridRecord {
                start: v.start,
                value: v.value,
            })
            .collect()
    }

    fn run_at(s: &Store, t_ms: i64) -> Run {
        let horizon = publication_horizon(ts(t_ms), &oslo())
            .unwrap()
            .as_millisecond();
        let last = s.fi.range(..q(horizon)).next_back().unwrap().0;
        let start_ms = last.as_millisecond() + QUARTER_MS;
        let (from, to) = (start_ms - 30 * DAY_MS, start_ms + 3 * DAY_MS);
        let wide_from = from - CORRECTION_HISTORY_DAYS * DAY_MS;
        let window = |m: &QuarterMap, from: i64| -> QuarterMap {
            m.range(q(from)..q(start_ms))
                .map(|(&k, &v)| (k, v))
                .collect()
        };
        let before = |records: &[GridRecord], from: i64| -> Vec<GridRecord> {
            records
                .iter()
                .filter(|r| (from..t_ms).contains(&r.start.as_millisecond()))
                .copied()
                .collect()
        };
        let issued_before = |vintages: &[GridVintage]| -> Vec<GridVintage> {
            vintages
                .iter()
                .filter(|v| v.issued_at.as_millisecond() < t_ms)
                .copied()
                .collect()
        };
        Run {
            t_ms,
            start_ms,
            fi: window(&s.fi, from),
            neighbours: s
                .neighbours
                .iter()
                .map(|(&a, m)| (a, window(m, from)))
                .collect(),
            wind_forecast: latest_before(&s.wind_vintages, t_ms, from, to),
            consumption_forecast: latest_before(&s.consumption_vintages, t_ms, from, to),
            wind_actual: before(&s.wind_actual, from),
            consumption_actual: before(&s.consumption_actual, from),
            wide_fi: window(&s.fi, wide_from),
            wide_neighbours: s
                .neighbours
                .iter()
                .map(|(&a, m)| (a, window(m, wide_from)))
                .collect(),
            wide_wind_actual: before(&s.wind_actual, wide_from),
            wide_consumption_actual: before(&s.consumption_actual, wide_from),
            wind_vintages: issued_before(&s.wind_vintages),
            consumption_vintages: issued_before(&s.consumption_vintages),
        }
    }

    impl Run {
        fn input(&self) -> ForecastInput<'_> {
            ForecastInput {
                spot_prices: &self.fi,
                neighbor_prices: &self.neighbours,
                wind_forecast: &self.wind_forecast,
                wind_actual: &self.wind_actual,
                consumption_forecast: &self.consumption_forecast,
                consumption_actual: &self.consumption_actual,
                series_start: ts(self.start_ms),
                series_end: ts(self.start_ms + 3 * DAY_MS),
            }
        }

        fn wide_input(&self) -> ForecastInput<'_> {
            ForecastInput {
                spot_prices: &self.wide_fi,
                neighbor_prices: &self.wide_neighbours,
                wind_actual: &self.wide_wind_actual,
                consumption_actual: &self.wide_consumption_actual,
                ..self.input()
            }
        }

        fn history<'a>(&'a self, zone: &'a TimeZone) -> IssuanceHistory<'a> {
            IssuanceHistory {
                issued_at: ts(self.t_ms),
                wind_vintages: &self.wind_vintages,
                consumption_vintages: &self.consumption_vintages,
                delivery_zone: zone,
            }
        }
    }

    /// A Sunday run at 13:00 UTC; its training issuances cross the DST
    /// change of 2026-03-29, and its window holds Easter 2026.
    const RUN: &str = "2026-04-12T13:00:00Z";

    fn fixture() -> Run {
        let t = ms(RUN);
        let s = store(t - 80 * DAY_MS, t + 2 * DAY_MS, t - 75 * DAY_MS);
        run_at(&s, t)
    }

    fn mean4w(zone: &TimeZone) -> ShapeProfile<'_> {
        ShapeProfile::WeekdayMean { zone, weeks: 4 }
    }

    /// The served options: the baseline of the correction.
    fn shaped_opts(zone: &TimeZone) -> ForecastOptions<'_> {
        let served = ForecastOptions::served(zone);
        assert_eq!(served.shape_profile, mean4w(zone));
        served
    }

    fn examples(run: &Run, zone: &TimeZone, holiday: Option<&TimeZone>) -> CorrectionExamples {
        build_correction_examples(
            &run.wide_input(),
            &shaped_opts(zone),
            &run.history(zone),
            holiday,
        )
    }

    fn corrected(
        input: &ForecastInput<'_>,
        zone: &TimeZone,
        examples: &CorrectionExamples,
        decomposition: Decomposition,
    ) -> ForecastResult {
        build_forecast(
            input,
            &ForecastOptions {
                correction: Some(QuarterCorrection {
                    examples,
                    decomposition,
                    ridge_lambda: 10.0,
                    window_days: 30,
                    half_life_days: None,
                }),
                ..shaped_opts(zone)
            },
        )
    }

    fn values(result: &ForecastResult) -> BTreeMap<QuarterStart, f64> {
        result
            .series
            .iter()
            .map(|p| (p.start, p.estimated_spot_cents_kwh))
            .collect()
    }

    /// Per quarter of each complete UTC day: the value minus its UTC-day mean.
    fn within_day(series: &BTreeMap<QuarterStart, f64>) -> BTreeMap<QuarterStart, f64> {
        let mut days: BTreeMap<i64, Vec<(QuarterStart, f64)>> = BTreeMap::new();
        for (&k, &v) in series {
            days.entry(k.as_millisecond().div_euclid(DAY_MS))
                .or_default()
                .push((k, v));
        }
        let mut out = BTreeMap::new();
        for members in days.values().filter(|m| m.len() == 96) {
            let mean = members.iter().map(|(_, v)| v).sum::<f64>() / 96.0;
            for (k, v) in members {
                out.insert(*k, v - mean);
            }
        }
        out
    }

    fn largest_change(a: &BTreeMap<QuarterStart, f64>, b: &BTreeMap<QuarterStart, f64>) -> f64 {
        assert_eq!(a.len(), b.len());
        assert!(!a.is_empty());
        a.iter().map(|(k, v)| (v - b[k]).abs()).fold(0.0, f64::max)
    }

    #[test]
    fn the_served_forecast_has_no_correction() {
        let zone = oslo();
        assert!(ForecastOptions::default().correction.is_none());
        assert!(ForecastOptions::served(&zone).correction.is_none());
        let run = fixture();
        let result = build_forecast(&run.input(), &ForecastOptions::served(&zone));
        assert_eq!(result.diagnostics.correction, None);
    }

    /// The behaviour check of the issue: a different within-day wind
    /// pattern in the future inputs, with the same UTC-day mean, changes
    /// the within-day shape of the corrected output. The baseline keeps its
    /// shape, because the shape step keeps only the UTC-day mean of the
    /// model. This shows that the correction can move the shape; it does
    /// not show a gain in accuracy.
    #[test]
    fn a_future_wind_pattern_changes_the_within_day_shape_only_with_the_correction() {
        let zone = oslo();
        let run = fixture();
        let ex = examples(&run, &zone, None);
        assert!(ex.examples.len() > 1000, "{}", ex.examples.len());
        let mut swapped = run.wind_forecast.clone();
        for r in &mut swapped {
            let at = r.start.as_millisecond();
            if at >= run.start_ms {
                // Zero mean over each UTC day: 2 cycles per day.
                r.value += 1500.0 * wave(at, 12 * HOUR_MS);
            }
        }
        let other = ForecastInput {
            wind_forecast: &swapped,
            ..run.input()
        };

        let baseline = within_day(&values(&build_forecast(&run.input(), &shaped_opts(&zone))));
        let baseline_other = within_day(&values(&build_forecast(&other, &shaped_opts(&zone))));
        // Only the 0.001 rounding of the output can differ.
        assert!(largest_change(&baseline, &baseline_other) <= 0.002);

        for decomposition in [Decomposition::Joint, Decomposition::DayDeviation] {
            let a = corrected(&run.input(), &zone, &ex, decomposition);
            let b = corrected(&other, &zone, &ex, decomposition);
            let diagnostics = a.diagnostics.correction.unwrap();
            assert!(!diagnostics.used_default, "{decomposition:?}");
            assert_eq!(diagnostics.uncorrected_quarters, 0, "{decomposition:?}");
            let change = largest_change(&within_day(&values(&a)), &within_day(&values(&b)));
            assert!(change > 0.3, "{decomposition:?}: {change}");
        }
    }

    #[test]
    fn the_correction_moves_only_its_own_arm() {
        let zone = oslo();
        let run = fixture();
        let ex = examples(&run, &zone, None);
        let input = run.input();
        let base = build_forecast(&input, &shaped_opts(&zone));
        let joint = corrected(&input, &zone, &ex, Decomposition::Joint);
        assert_ne!(values(&base), values(&joint));
        assert_eq!(build_forecast(&input, &shaped_opts(&zone)), base);
        assert_eq!(joint.diagnostics.fit_samples, base.diagnostics.fit_samples);
        assert_eq!(
            joint.diagnostics.correction.unwrap().fit_samples,
            ex.examples.len()
        );
    }

    #[test]
    fn day_deviation_keeps_the_daily_level_of_the_shape_step() {
        let zone = oslo();
        let run = fixture();
        let ex = examples(&run, &zone, None);
        let base = values(&build_forecast(&run.input(), &shaped_opts(&zone)));
        let dev = values(&corrected(
            &run.input(),
            &zone,
            &ex,
            Decomposition::DayDeviation,
        ));
        let mut sums: BTreeMap<i64, (f64, f64)> = BTreeMap::new();
        for (k, v) in &dev {
            let slot = sums
                .entry(k.as_millisecond().div_euclid(DAY_MS))
                .or_default();
            slot.0 += v - base[k];
            slot.1 += 1.0;
        }
        for (day, (sum, n)) in sums {
            // Each output carries at most 0.0005 of rounding.
            assert!((sum / n).abs() <= 0.001, "day {day}: {}", sum / n);
        }
    }

    /// An example's baseline uses only prices that its issuance had; with
    /// only the served price window, most examples have none.
    #[test]
    fn examples_take_the_baseline_published_at_their_issuance() {
        let zone = oslo();
        let run = fixture();
        let wide = examples(&run, &zone, None);
        assert!(wide.without_baseline * 50 < wide.examples.len());
        for e in wide.examples.iter().step_by(97) {
            let horizon = publication_horizon(e.issued_at, &zone).unwrap();
            let published: QuarterMap = run
                .wide_fi
                .range(..QuarterStart::floor(horizon))
                .map(|(&k, &v)| (k, v))
                .collect();
            let profile = profile_values(mean4w(&zone), [e.target], &published);
            assert_eq!(profile.values.get(&e.target), Some(&e.baseline));
            assert_eq!(e.features.last(), Some(&e.baseline));
        }

        let narrow =
            build_correction_examples(&run.input(), &shaped_opts(&zone), &run.history(&zone), None);
        assert!(narrow.without_baseline > narrow.examples.len());
        // The wider actuals also fill tail extensions of old issuances.
        assert!(wide.excluded.total() < narrow.excluded.total());
    }

    #[test]
    fn without_enough_examples_the_shape_step_output_stays() {
        let zone = oslo();
        let run = fixture();
        let mut empty = examples(&run, &zone, None);
        empty.examples.clear();
        let base = build_forecast(&run.input(), &shaped_opts(&zone));
        for decomposition in [Decomposition::Joint, Decomposition::DayDeviation] {
            let result = corrected(&run.input(), &zone, &empty, decomposition);
            let diagnostics = result.diagnostics.correction.unwrap();
            assert!(diagnostics.used_default);
            assert_eq!(diagnostics.corrected_quarters, 0);
            assert_eq!(result.series, base.series);
        }
    }

    #[test]
    fn the_window_and_the_half_life_change_the_fit() {
        let zone = oslo();
        let run = fixture();
        let ex = examples(&run, &zone, None);
        let input = run.input();
        let with = |window_days, half_life_days| {
            build_forecast(
                &input,
                &ForecastOptions {
                    correction: Some(QuarterCorrection {
                        examples: &ex,
                        decomposition: Decomposition::Joint,
                        ridge_lambda: 10.0,
                        window_days,
                        half_life_days,
                    }),
                    ..shaped_opts(&zone)
                },
            )
        };
        let full = with(30, None);
        let short = with(14, None);
        let weighted = with(30, Some(7.0));
        let samples = |r: &ForecastResult| r.diagnostics.correction.unwrap().fit_samples;
        assert!(samples(&short) < samples(&full));
        assert_eq!(samples(&weighted), samples(&full));
        assert_ne!(values(&short), values(&full));
        assert_ne!(values(&weighted), values(&full));
        // A window beyond the training history reads the same examples.
        assert_eq!(with(60, None), full);
    }

    #[test]
    fn the_holiday_column_marks_easter_targets() {
        let zone = oslo();
        let fi_zone = helsinki();
        let run = fixture();
        let ex = examples(&run, &zone, Some(&fi_zone));
        let names = correction_feature_names(true);
        assert_eq!(names.last(), Some(&HOLIDAY_FEATURE_NAME));
        let column = names.len() - 1;
        // Easter Monday 2026-04-06, local noon.
        let monday = q(ms("2026-04-06T09:00:00Z"));
        let tuesday = q(ms("2026-04-07T09:00:00Z"));
        let marks = |target| {
            ex.examples
                .iter()
                .filter(|e| e.target == target)
                .map(|e| e.features[column])
                .collect::<Vec<_>>()
        };
        assert_ne!(marks(monday), Vec::<f64>::new());
        assert!(marks(monday).iter().all(|v| *v == 1.0));
        assert!(marks(tuesday).iter().all(|v| *v == 0.0));
        let result = build_forecast(
            &run.input(),
            &ForecastOptions {
                correction: Some(QuarterCorrection {
                    examples: &ex,
                    decomposition: Decomposition::Joint,
                    ridge_lambda: 10.0,
                    window_days: 30,
                    half_life_days: None,
                }),
                ..shaped_opts(&zone)
            },
        );
        assert!(!result.diagnostics.correction.unwrap().used_default);
    }
}
