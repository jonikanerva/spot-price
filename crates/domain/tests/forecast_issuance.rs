//! Issuance training: the examples use only what each issuance had, and
//! the served default does not change. The fixture is synthetic; the
//! expected cutoffs come from the publication rule (12:00 UTC on D − 1,
//! delivery days in Europe/Oslo) and the stored `issued_at` semantics.

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "an unchanged input must give a bit-equal feature"
)]
mod issuance {
    use std::collections::BTreeMap;
    use std::f64::consts::PI;

    use jiff::Timestamp;
    use jiff::tz::TimeZone;
    use spot_price_domain::areas::Area;
    use spot_price_domain::forecast::{
        FEATURE_COUNT, FEATURE_NAMES, ForecastInput, ForecastOptions, GridRecord, GridVintage,
        ISSUANCE_EXTRA_FEATURE_NAMES, ISSUANCE_FEATURE_COUNT, InputSource, IssuanceExamples,
        IssuanceHistory, NeighborPrices, QuarterMap, QuarterStart, TrainingExample, TrainingRule,
        build_forecast, build_issuance_examples, issuance_feature_names, publication_horizon,
    };

    const QUARTER_MS: i64 = 15 * 60 * 1000;
    const HOUR_MS: i64 = 60 * 60 * 1000;
    const DAY_MS: i64 = 24 * HOUR_MS;

    /// The wind forecast (245) covers this many hours after its issuance.
    const WIND_HORIZON_H: i64 = 72;
    /// The consumption forecast (165) covers this many hours after its
    /// issuance.
    const CONSUMPTION_HORIZON_H: i64 = 30;
    /// The archive keeps vintages of targets from this many hours before
    /// the issuance.
    const BACKFILL_H: i64 = 6;

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
        6.0 + 2.0 * wave(at_ms, DAY_MS) - wind_actual(at_ms) / 2000.0 + level
    }

    /// Every stored input of the fixture: prices, actuals, and vintages
    /// issued every 3 hours from `vintage_from_ms`.
    struct Store {
        fi: QuarterMap,
        neighbours: NeighborPrices,
        wind_actual: Vec<GridRecord>,
        consumption_actual: Vec<GridRecord>,
        wind_vintages: Vec<GridVintage>,
        consumption_vintages: Vec<GridVintage>,
    }

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
            let mut target = issued - BACKFILL_H * HOUR_MS;
            while target < issued + WIND_HORIZON_H * HOUR_MS {
                let lead_h = int((target - issued) / HOUR_MS);
                s.wind_vintages.push(GridVintage {
                    issued_at: ts(issued),
                    start: ts(target),
                    value: wind_actual(target) - 4.0 * lead_h,
                });
                if target < issued + CONSUMPTION_HORIZON_H * HOUR_MS {
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

    /// The served inputs of a run at `t_ms` with series start `start_ms`:
    /// prices before the series start, actuals before `t_ms`, and the
    /// newest vintage issued before `t_ms` per target.
    struct Run {
        t_ms: i64,
        start_ms: i64,
        fi: QuarterMap,
        neighbours: NeighborPrices,
        wind_forecast: Vec<GridRecord>,
        consumption_forecast: Vec<GridRecord>,
        wind_actual: Vec<GridRecord>,
        consumption_actual: Vec<GridRecord>,
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
        let window = |m: &QuarterMap| -> QuarterMap {
            m.range(q(from)..q(start_ms))
                .map(|(&k, &v)| (k, v))
                .collect()
        };
        let before = |records: &[GridRecord]| -> Vec<GridRecord> {
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
            fi: window(&s.fi),
            neighbours: s.neighbours.iter().map(|(&a, m)| (a, window(m))).collect(),
            wind_forecast: latest_before(&s.wind_vintages, t_ms, from, to),
            consumption_forecast: latest_before(&s.consumption_vintages, t_ms, from, to),
            wind_actual: before(&s.wind_actual),
            consumption_actual: before(&s.consumption_actual),
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

        fn examples(&self, zone: &TimeZone) -> IssuanceExamples {
            build_issuance_examples(
                &self.input(),
                &ForecastOptions::default(),
                &self.history(zone),
            )
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

    /// The run of the fixture: 13:00 UTC, after the publication of the
    /// next delivery day. Its training issuances cross the DST change of
    /// 2026-03-29.
    const RUN: &str = "2026-04-05T13:00:00Z";

    fn fixture() -> (Store, Run) {
        let t = ms(RUN);
        let s = store(t - 45 * DAY_MS, t + 2 * DAY_MS, t - 40 * DAY_MS);
        let run = run_at(&s, t);
        (s, run)
    }

    fn column(name: &str) -> usize {
        issuance_feature_names()
            .iter()
            .position(|n| *n == name)
            .unwrap()
    }

    fn of_issuance(examples: &IssuanceExamples, issued_ms: i64) -> Vec<&TrainingExample> {
        examples
            .examples
            .iter()
            .filter(|e| e.issued_at.as_millisecond() == issued_ms)
            .collect()
    }

    #[test]
    fn the_served_forecast_keeps_the_realised_training_rule() {
        assert!(matches!(
            ForecastOptions::default().training,
            TrainingRule::Realised
        ));
        assert_eq!(
            issuance_feature_names().len(),
            FEATURE_COUNT + ISSUANCE_EXTRA_FEATURE_NAMES.len()
        );
        assert_eq!(issuance_feature_names()[..FEATURE_COUNT], FEATURE_NAMES);
    }

    #[test]
    fn the_publication_rule_moves_at_noon_utc_and_follows_dst() {
        let zone = oslo();
        let horizon = |t: &str| publication_horizon(ts(ms(t)), &zone).unwrap();
        // Before noon only the current delivery day is published.
        assert_eq!(
            horizon("2026-03-20T11:59:59Z"),
            ts(ms("2026-03-20T23:00:00Z"))
        );
        assert_eq!(
            horizon("2026-03-20T12:00:00Z"),
            ts(ms("2026-03-21T23:00:00Z"))
        );
        // Delivery day 2026-03-29 has 23 hours; 2026-03-30 starts at 22:00Z.
        assert_eq!(
            horizon("2026-03-28T13:00:00Z"),
            ts(ms("2026-03-29T22:00:00Z"))
        );
        assert_eq!(
            horizon("2026-03-27T13:00:00Z"),
            ts(ms("2026-03-28T23:00:00Z"))
        );
    }

    #[test]
    fn each_issuance_starts_after_the_prices_it_had() {
        let zone = oslo();
        let (_, run) = fixture();
        let examples = run.examples(&zone);
        assert!(
            examples.issuances.len() >= 30,
            "{}",
            examples.issuances.len()
        );
        for (k, summary) in (1..).zip(&examples.issuances) {
            let issued = summary.issued_at.as_millisecond();
            assert_eq!(issued, run.t_ms - k * DAY_MS);
            let horizon = publication_horizon(summary.issued_at, &zone)
                .unwrap()
                .as_millisecond();
            assert!(summary.series_start.as_millisecond() <= horizon);
            // The fixture has every quarter, so the series start is the horizon.
            assert_eq!(summary.series_start, q(horizon), "issuance {k}");
            assert!(summary.has_grid_data);
            assert_eq!(summary.examples > 0, summary.price_quarters > 0);
        }
        let start = |t: &str| {
            examples
                .issuances
                .iter()
                .find(|s| s.issued_at == ts(ms(t)))
                .unwrap()
                .series_start
        };
        assert_eq!(start("2026-03-27T13:00:00Z"), q(ms("2026-03-28T23:00:00Z")));
        assert_eq!(start("2026-03-28T13:00:00Z"), q(ms("2026-03-29T22:00:00Z")));
    }

    #[test]
    fn examples_are_targets_of_their_own_issuance_inside_the_training_window() {
        let zone = oslo();
        let (_, run) = fixture();
        let examples = run.examples(&zone);
        let window = (run.start_ms - 30 * DAY_MS)..run.start_ms;
        assert_ne!(examples.examples.len(), 0);
        for e in &examples.examples {
            let target = e.target.as_millisecond();
            let start = e.series_start.as_millisecond();
            assert!((start..start + 3 * DAY_MS).contains(&target));
            assert!(window.contains(&target));
            assert_eq!(e.lead_ms, target - e.issued_at.as_millisecond());
            assert_eq!(e.realised, run.fi[&e.target]);
            assert_eq!(e.features.len(), ISSUANCE_FEATURE_COUNT);
            let lead = e.features[column("lead_hours")];
            assert_eq!(lead, int(e.lead_ms / 60_000) / 60.0);
        }
        // Each quarter of the window, except the newest, has examples at
        // more than one lead.
        let mut per_target: BTreeMap<QuarterStart, usize> = BTreeMap::new();
        for e in &examples.examples {
            *per_target.entry(e.target).or_default() += 1;
        }
        let oldest_full = q(run.start_ms - 20 * DAY_MS);
        assert!(per_target.range(oldest_full..).all(|(_, &n)| n >= 2));
        let summed: usize = examples.issuances.iter().map(|s| s.examples).sum();
        assert_eq!(summed, examples.examples.len());
    }

    /// The inputs of the store with every value that issuance `issued_ms`
    /// with series start `start_ms` did not have changed.
    fn perturbed_after(s: &Store, issued_ms: i64, start_ms: i64) -> Store {
        let shift = |m: &QuarterMap| -> QuarterMap {
            m.iter()
                .map(|(&k, &v)| {
                    let late = k.as_millisecond() >= start_ms;
                    (k, if late { v + 37.0 } else { v })
                })
                .collect()
        };
        let actual = |records: &[GridRecord]| -> Vec<GridRecord> {
            records
                .iter()
                .map(|r| GridRecord {
                    value: if r.start.as_millisecond() >= issued_ms {
                        r.value * 3.0 + 11.0
                    } else {
                        r.value
                    },
                    ..*r
                })
                .collect()
        };
        let vintage = |vintages: &[GridVintage]| -> Vec<GridVintage> {
            vintages
                .iter()
                .map(|v| GridVintage {
                    value: if v.issued_at.as_millisecond() >= issued_ms {
                        -v.value - 500.0
                    } else {
                        v.value
                    },
                    ..*v
                })
                .collect()
        };
        Store {
            fi: shift(&s.fi),
            neighbours: s.neighbours.iter().map(|(&a, m)| (a, shift(m))).collect(),
            wind_actual: actual(&s.wind_actual),
            consumption_actual: actual(&s.consumption_actual),
            wind_vintages: vintage(&s.wind_vintages),
            consumption_vintages: vintage(&s.consumption_vintages),
        }
    }

    #[test]
    fn no_example_uses_information_after_its_issuance() {
        let zone = oslo();
        let (s, run) = fixture();
        let examples = run.examples(&zone);
        let mut checked = 0;
        let scored = examples.issuances.iter().filter(|s| s.examples > 0);
        for summary in scored.step_by(4) {
            let issued = summary.issued_at.as_millisecond();
            let start = summary.series_start.as_millisecond();
            let changed = run_at(&perturbed_after(&s, issued, start), run.t_ms);
            let changed = changed.examples(&zone);
            let before = of_issuance(&examples, issued);
            let after = of_issuance(&changed, issued);
            assert_ne!(before.len(), 0);
            assert_eq!(before.len(), after.len());
            for (a, b) in before.iter().zip(&after) {
                assert_eq!(a.target, b.target);
                assert_eq!(a.wind, b.wind);
                assert_eq!(a.consumption, b.consumption);
                assert_eq!(a.lags, b.lags);
                let bits = |e: &TrainingExample| -> Vec<u64> {
                    e.features.iter().map(|v| v.to_bits()).collect()
                };
                assert_eq!(bits(a), bits(b), "issuance {issued} target {:?}", a.target);
                checked += 1;
            }
            // The perturbation reaches the later issuances, so the check
            // can see a change.
            assert_ne!(examples.examples, changed.examples);
        }
        assert!(checked > 1000, "{checked}");
    }

    #[test]
    fn a_vintage_issued_before_the_issuance_reaches_its_examples() {
        let zone = oslo();
        let (mut s, _) = fixture();
        let t = ms(RUN);
        let issued = t - 10 * DAY_MS;
        let base = run_at(&s, t).examples(&zone);
        // The vintage of 10:00 is the newest one before the 13:00 issuance.
        let target = issued + 36 * HOUR_MS;
        for v in &mut s.wind_vintages {
            if v.issued_at.as_millisecond() == issued - 3 * HOUR_MS
                && v.start.as_millisecond() == target
            {
                v.value += 1000.0;
            }
        }
        let moved = run_at(&s, t).examples(&zone);
        let wind = column("wind_mw");
        let find = |set: &IssuanceExamples| {
            of_issuance(set, issued)
                .into_iter()
                .find(|e| e.target == q(target))
                .unwrap()
                .clone()
        };
        let (a, b) = (find(&base), find(&moved));
        assert_eq!(b.features[wind] - a.features[wind], 1000.0);
        assert_eq!(
            a.wind,
            InputSource::Vintage {
                issued_at: Some(ts(issued - 3 * HOUR_MS))
            }
        );
    }

    #[test]
    fn missing_lags_and_grid_sources_follow_the_lead() {
        let zone = oslo();
        let (_, run) = fixture();
        let examples = run.examples(&zone);
        let issued = run.t_ms - 12 * DAY_MS;
        let set = of_issuance(&examples, issued);
        let series_start = set[0].series_start.as_millisecond();
        let fi_1d = column("fi_same_quarter_1d");
        let mut neutral_fills = Vec::new();
        for e in &set {
            let since_start = e.target.as_millisecond() - series_start;
            let lead_h = e.lead_ms / HOUR_MS;
            // Only the first predicted day has published prices one day back.
            assert_eq!(e.lags.fi_1d, since_start < DAY_MS, "{:?}", e.target);
            assert_eq!(e.lags.neighbors_1d == 3, since_start < DAY_MS);
            assert!(e.lags.fi_7d);
            assert_eq!(
                e.features[column("fi_1d_missing")],
                if e.lags.fi_1d { 0.0 } else { 1.0 }
            );
            if !e.lags.fi_1d {
                neutral_fills.push(e.features[fi_1d]);
                assert_eq!(e.features[column("neighbor_1d_missing")], 1.0);
            }
            // The UTC day before the target is partly or fully unpublished
            // from the second UTC day on.
            let day_before = e.target.as_millisecond().div_euclid(DAY_MS) * DAY_MS - DAY_MS;
            if day_before >= series_start {
                assert!(!e.lags.fi_prev_day);
            }
            // The wind forecast covers 72 h from the newest issuance before
            // the issuance; later quarters use the tail extension.
            let wind_expected = if lead_h < WIND_HORIZON_H - 3 {
                matches!(e.wind, InputSource::Vintage { issued_at: Some(at) } if at.as_millisecond() < issued)
            } else if lead_h >= WIND_HORIZON_H {
                e.wind == InputSource::Extension
            } else {
                true
            };
            assert!(wind_expected, "lead {lead_h}: {:?}", e.wind);
            let consumption_expected = if lead_h < CONSUMPTION_HORIZON_H - 3 {
                matches!(e.consumption, InputSource::Vintage { .. })
            } else if lead_h >= CONSUMPTION_HORIZON_H {
                e.consumption == InputSource::Extension
            } else {
                true
            };
            assert!(consumption_expected, "lead {lead_h}: {:?}", e.consumption);
            let from_vintage = e.features[column("wind_from_vintage")];
            assert_eq!(
                from_vintage,
                if matches!(e.wind, InputSource::Vintage { .. }) {
                    1.0
                } else {
                    0.0
                }
            );
        }
        // A missing lag takes one neutral fill: the mean price of the issuance.
        assert_ne!(neutral_fills.len(), 0);
        assert!(neutral_fills.iter().all(|v| *v == neutral_fills[0]));
    }

    #[test]
    fn no_actual_takes_the_place_of_a_historical_forecast() {
        let zone = oslo();
        let t = ms(RUN);
        // The archive starts 20 days before the run, so the older issuances
        // had no vintage at all.
        let s = store(t - 45 * DAY_MS, t + 2 * DAY_MS, t - 20 * DAY_MS);
        let run = run_at(&s, t);
        let examples = run.examples(&zone);
        let mut pre_archive = 0;
        for e in &examples.examples {
            assert!(e.target.as_millisecond() >= e.issued_at.as_millisecond());
            assert_ne!(e.wind, InputSource::Actual);
            assert_ne!(e.consumption, InputSource::Actual);
            if e.issued_at.as_millisecond() <= t - 21 * DAY_MS {
                pre_archive += 1;
                // The tail extension needs a forecast to extend, as in the
                // served forecast.
                assert_eq!(e.wind, InputSource::Missing);
                assert_eq!(e.features[column("wind_mw")], 0.0);
                assert_eq!(e.features[column("wind_from_vintage")], 0.0);
            }
        }
        assert!(pre_archive > 0);
        let old = examples
            .issuances
            .iter()
            .filter(|s| s.issued_at.as_millisecond() <= t - 21 * DAY_MS);
        for summary in old {
            assert_eq!(summary.wind_vintage_targets, 0);
            assert_eq!(summary.consumption_vintage_targets, 0);
        }
    }

    #[test]
    fn an_issuance_before_noon_predicts_one_more_day() {
        let zone = oslo();
        let t = ms("2026-04-05T11:00:00Z");
        let s = store(t - 45 * DAY_MS, t + 2 * DAY_MS, t - 40 * DAY_MS);
        let morning = run_at(&s, t);
        let afternoon = run_at(&s, t + 2 * HOUR_MS);
        assert_eq!(afternoon.start_ms - morning.start_ms, DAY_MS);
        let first = |run: &Run| run.examples(&zone).issuances[0];
        let (m, a) = (first(&morning), first(&afternoon));
        assert_eq!(m.series_start, q(ms("2026-04-04T22:00:00Z")));
        assert_eq!(a.series_start, q(ms("2026-04-05T22:00:00Z")));
        // The morning issuance has no price of its own next day.
        let examples = morning.examples(&zone);
        let next_day = of_issuance(&examples, m.issued_at.as_millisecond());
        assert!(next_day.iter().all(|e| e.lead_ms > 0));
        let max_lead = next_day.iter().map(|e| e.lead_ms).max().unwrap();
        assert!(max_lead < 3 * DAY_MS + 12 * HOUR_MS);
    }

    #[test]
    fn the_issuance_rule_fits_every_column_and_changes_only_that_arm() {
        let zone = oslo();
        let (_, run) = fixture();
        let input = run.input();
        let served = build_forecast(&input, &ForecastOptions::default());
        let issuance = build_forecast(
            &input,
            &ForecastOptions {
                training: TrainingRule::Issuance(run.history(&zone)),
                ..ForecastOptions::default()
            },
        );
        let examples = run.examples(&zone);
        assert!(!issuance.diagnostics.fit_used_default);
        assert_eq!(issuance.diagnostics.feature_count, ISSUANCE_FEATURE_COUNT);
        assert_eq!(issuance.diagnostics.fit_samples, examples.examples.len());
        assert_eq!(served.diagnostics.feature_count, FEATURE_COUNT);
        assert_eq!(served.series.len(), issuance.series.len());
        let moved = served
            .series
            .iter()
            .zip(&issuance.series)
            .filter(|(a, b)| a.estimated_spot_cents_kwh != b.estimated_spot_cents_kwh)
            .count();
        assert!(moved > 0);
        // The same served input with the default rule gives the same result.
        assert_eq!(build_forecast(&input, &ForecastOptions::default()), served);
    }

    #[test]
    fn a_run_without_earlier_issuances_falls_back() {
        let zone = oslo();
        let (_, run) = fixture();
        let history = IssuanceHistory {
            issued_at: ts(run.t_ms),
            wind_vintages: &[],
            consumption_vintages: &[],
            delivery_zone: &zone,
        };
        // Without prices, no issuance has a target with a published price.
        let empty = QuarterMap::new();
        let input = ForecastInput {
            spot_prices: &empty,
            ..run.input()
        };
        let out = build_issuance_examples(&input, &ForecastOptions::default(), &history);
        assert_eq!(out.examples, Vec::new());
        let result = build_forecast(
            &input,
            &ForecastOptions {
                training: TrainingRule::Issuance(history),
                ..ForecastOptions::default()
            },
        );
        assert!(result.diagnostics.fit_used_default);
    }
}
