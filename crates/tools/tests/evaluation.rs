//! Behaviour of the evaluation report (`backtest --eval`): issuances before
//! and after publication, horizons and leads, paired accuracy, schedules,
//! exclusions, the block bootstrap, the split, and the criteria.

#[cfg(test)]
mod evaluation {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    use jiff::Timestamp;
    use spot_price_domain::forecast::{QuarterMap, QuarterStart};
    use spot_price_tools::backtest::{ForecastVariant, IssueTimeInputs, publication_horizon_ms};
    use spot_price_tools::bootstrap::{BootstrapOptions, MIN_OVERLAP_BLOCK_DAYS};
    use spot_price_tools::cli::{Context, Output, Stream, run};
    use spot_price_tools::criteria::{Criterion, Op, Outcome, Which, check};
    use spot_price_tools::data::{BacktestData, GridActual, PricePoint, Vintage, to_fixture_json};
    use spot_price_tools::evaluation::{
        ArmKind, EvalOptions, Evaluation, Exclusion, IssueStatus, ModelArm, Segment, Split,
        Statistic, TargetRecord, TaskKind, evaluate, schedule_arms, statistics,
    };
    use spot_price_tools::schedule::{CostBasis, Pricer, ScheduleTask};

    const QUARTER_MS: i64 = 15 * 60 * 1000;
    const HOUR_MS: i64 = 60 * 60 * 1000;
    const DAY_MS: i64 = 24 * HOUR_MS;

    /// Few resamples keep the tests fast; the defaults are in the report.
    const FAST: BootstrapOptions = BootstrapOptions {
        seed: 153,
        resamples: 200,
        block_days: 7,
    };

    fn ms(text: &str) -> i64 {
        text.parse::<Timestamp>().unwrap().as_millisecond()
    }

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_millisecond(ms).unwrap()
    }

    /// Mean-zero pseudo-noise in [-1, 1] per quarter index.
    fn noise_at(q: i64) -> f64 {
        #[expect(clippy::cast_precision_loss, reason = "q is a small index")]
        let s = (q as f64 * 12.9898).sin() * 43758.5453;
        (s - s.floor() - 0.5) * 2.0
    }

    /// `days` days of quarters from `start`. The FI price follows the grid
    /// with noise. Wind vintages are issued 2, 30, 60, 100, and 120 h before the
    /// target, so every lead of the served window has one. Consumption
    /// vintages stop at 60 h, so long leads lack them.
    #[expect(clippy::cast_precision_loss, reason = "q is a small index")]
    fn build(start: &str, days: i64, flat_price: Option<f64>) -> BacktestData {
        let start = ms(start);
        let mut data = BacktestData::default();
        let (mut wind75, mut cons124, mut wind245, mut cons165) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for q in 0..days * 96 {
            let at = start + q * QUARTER_MS;
            let consumption = 8000.0 + (q % 96) as f64 * 15.0;
            let wind = 2000.0 + ((q * 131) % 1800) as f64;
            let fi = flat_price.unwrap_or(0.0012 * (consumption - wind) + 2.0 + 1.5 * noise_at(q));
            for (area, offset) in [("FI", 0.0), ("SE1", -0.5), ("SE3", 0.3), ("EE", 0.1)] {
                data.prices.push(PricePoint {
                    area: area.to_owned(),
                    start: ts(at),
                    spot_cents_kwh: fi + offset,
                });
            }
            let actual = |value, dataset_id| GridActual {
                dataset_id,
                start: ts(at),
                end: ts(at + QUARTER_MS),
                value,
            };
            cons124.push(actual(consumption, 124));
            wind75.push(actual(wind, 75));
            let vintage = |value, dataset_id, lead_h: i64| Vintage {
                dataset_id,
                issued_at: ts(at - lead_h * HOUR_MS),
                start: ts(at),
                end: ts(at + QUARTER_MS),
                value,
            };
            for lead in [2, 30, 60, 100, 120] {
                wind245.push(vintage(wind + 3.0 * (lead as f64), 245, lead));
            }
            for lead in [2, 30, 60] {
                cons165.push(vintage(consumption, 165, lead));
            }
        }
        data.actuals = BTreeMap::from([(75, wind75), (124, cons124)]);
        data.vintages = BTreeMap::from([(245, wind245), (165, cons165)]);
        data
    }

    fn arm(name: &str, variant: ForecastVariant) -> ModelArm {
        ModelArm {
            name: name.to_owned(),
            variant,
            kind: ArmKind::Model,
        }
    }

    fn options(hours: &[u8], arms: Vec<ModelArm>) -> EvalOptions {
        EvalOptions {
            issuance_hours_utc: hours.to_vec(),
            arms,
            cost: CostBasis::Spot,
            paired: Vec::new(),
        }
    }

    fn default_eval(data: &BacktestData) -> Evaluation {
        evaluate(
            data,
            &options(&[6, 13], vec![arm("A", ForecastVariant::default())]),
        )
        .unwrap()
    }

    fn stat<'a>(stats: &'a [Statistic], id: &str) -> &'a Statistic {
        stats
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("no statistic {id}"))
    }

    // Issuances, horizons, and leads

    #[test]
    fn both_issuances_score_their_own_delivery_days_and_leads() {
        let eval = default_eval(&build("2026-02-01T00:00:00Z", 24, None));
        let days = |hour: u8| -> BTreeSet<i64> {
            eval.targets
                .iter()
                .filter(|t| t.hour_utc == hour && t.actual.is_some())
                .map(|t| t.day_ahead)
                .collect()
        };
        // 06:00 UTC is before publication: the first unpublished delivery
        // day is tomorrow. 13:00 UTC is after it: tomorrow is published.
        assert_eq!(days(6), BTreeSet::from([1, 2, 3]));
        assert_eq!(days(13), BTreeSet::from([2, 3, 4]));
        let min_lead = |hour: u8, day_ahead: i64| {
            eval.targets
                .iter()
                .filter(|t| t.hour_utc == hour && t.day_ahead == day_ahead)
                .map(TargetRecord::lead_hours)
                .fold(f64::INFINITY, f64::min)
        };
        // February: the CET delivery day starts at 23:00 UTC.
        assert_eq!(min_lead(6, 1), 17.0);
        assert_eq!(min_lead(13, 2), 34.0);
        let local: BTreeSet<u8> = eval.issues.iter().map(|i| i.local_hour).collect();
        assert_eq!(local, BTreeSet::from([8, 15]));
    }

    #[test]
    fn a_target_is_never_a_quarter_published_at_the_issue_time() {
        let eval = default_eval(&build("2026-02-01T00:00:00Z", 20, None));
        assert_ne!(eval.targets.len(), 0);
        // After the last stored day no later day is published, so the last
        // issuances start early; their targets have no realised price.
        for t in eval.targets.iter().filter(|t| t.actual.is_some()) {
            assert!(t.target_ms >= publication_horizon_ms(t.t_ms).unwrap());
        }
    }

    #[test]
    fn the_first_horizon_moves_at_the_publication_hour() {
        let data = build("2026-02-01T00:00:00Z", 20, None);
        let eval = evaluate(
            &data,
            &options(&[11, 12], vec![arm("A", ForecastVariant::default())]),
        )
        .unwrap();
        let first = |hour: u8| {
            eval.targets
                .iter()
                .filter(|t| t.hour_utc == hour && t.actual.is_some())
                .map(|t| t.day_ahead)
                .min()
        };
        assert_eq!(first(11), Some(1));
        assert_eq!(first(12), Some(2));
    }

    #[test]
    fn every_issuance_is_leak_free_and_forecasts_every_arm() {
        let eval = default_eval(&build("2026-02-01T00:00:00Z", 20, None));
        let forecasts: Vec<_> = eval
            .issues
            .iter()
            .filter_map(|i| match &i.status {
                IssueStatus::Forecast(f) => Some(f),
                _ => None,
            })
            .collect();
        assert!(forecasts.len() > 30);
        for f in forecasts {
            assert!(f.leak_free);
            assert_eq!(f.forecast_quarters, vec![f.expected_quarters]);
        }
    }

    #[test]
    fn input_coverage_follows_the_vintage_ladder() {
        let eval = default_eval(&build("2026-02-01T00:00:00Z", 24, None));
        let paired: Vec<_> = eval.targets.iter().filter(|t| t.complete()).collect();
        assert!(paired.iter().all(|t| t.wind_vintage));
        // Consumption vintages reach only 60 h ahead.
        assert!(
            paired
                .iter()
                .filter(|t| t.lead_hours() > 61.0)
                .all(|t| !t.consumption_vintage)
        );
        assert!(
            paired
                .iter()
                .filter(|t| t.lead_hours() < 59.0)
                .all(|t| t.consumption_vintage)
        );
    }

    // Paired accuracy

    #[test]
    fn every_accuracy_statistic_of_a_horizon_uses_one_target_set() {
        let data = build("2026-02-01T00:00:00Z", 24, None);
        let strong = ForecastVariant {
            ridge_lambda: 50.0,
            ..ForecastVariant::default()
        };
        let eval = evaluate(
            &data,
            &options(
                &[6, 13],
                vec![arm("A", ForecastVariant::default()), arm("B", strong)],
            ),
        )
        .unwrap();
        let stats = statistics(&eval, Segment::All, None, &FAST);
        let keys: Vec<String> = stats
            .iter()
            .filter_map(|s| s.id.strip_prefix("mae/last_week/").map(str::to_owned))
            .collect();
        assert_eq!(keys.len(), 6);
        for key in keys {
            let n = stat(&stats, &format!("mae/last_week/{key}")).n;
            assert!(n > 0);
            for id in [
                format!("mae/last_published_day/{key}"),
                format!("mae/A/{key}"),
                format!("mae/B/{key}"),
                format!("rmae/A/last_week/{key}"),
                format!("rmae/B/last_published_day/{key}"),
                format!("mae_delta/B-A/{key}"),
            ] {
                assert_eq!(stat(&stats, &id).n, n, "{id}");
            }
            // rMAE is the ratio of the MAEs on the same targets.
            let ratio = stat(&stats, &format!("mae/A/{key}"))
                .estimate
                .point
                .unwrap()
                / stat(&stats, &format!("mae/last_week/{key}"))
                    .estimate
                    .point
                    .unwrap();
            let rmae = stat(&stats, &format!("rmae/A/last_week/{key}"))
                .estimate
                .point
                .unwrap();
            assert!((ratio - rmae).abs() < 1e-9);
        }
    }

    #[test]
    fn an_arm_against_itself_gives_zero_deltas() {
        let data = build("2026-02-01T00:00:00Z", 20, None);
        let eval = evaluate(
            &data,
            &options(
                &[6, 13],
                vec![
                    arm("A", ForecastVariant::default()),
                    arm("B", ForecastVariant::default()),
                ],
            ),
        )
        .unwrap();
        let stats = statistics(&eval, Segment::All, None, &FAST);
        let deltas: Vec<_> = stats
            .iter()
            .filter(|s| s.id.starts_with("mae_delta/") || s.id.starts_with("cost_delta/B-A/"))
            .collect();
        assert_ne!(deltas.len(), 0);
        for s in deltas {
            assert_eq!(s.estimate.point, Some(0.0), "{}", s.id);
            assert_eq!(s.estimate.high, Some(0.0), "{}", s.id);
        }
    }

    #[test]
    fn a_zero_baseline_error_leaves_rmae_undefined() {
        let eval = default_eval(&build("2026-02-01T00:00:00Z", 20, Some(5.0)));
        let stats = statistics(&eval, Segment::All, None, &FAST);
        let id = "rmae/A/last_week/13/D+2";
        assert_eq!(
            stat(&stats, "mae/last_week/13/D+2").estimate.point,
            Some(0.0)
        );
        let rmae = stat(&stats, id);
        assert!(rmae.n > 0);
        assert_eq!(rmae.estimate.point, None);
        let criterion = Criterion {
            id: id.to_owned(),
            statistic: Which::Point,
            op: Op::Lt,
            value: 1.0,
        };
        assert_eq!(check(&criterion, &stats), Outcome::Undefined);
    }

    // Schedules

    #[test]
    fn hindsight_has_zero_regret_and_no_arm_beats_it() {
        let eval = default_eval(&build("2026-02-01T00:00:00Z", 24, None));
        let hindsight = eval.schedule_arm_names().len() - 1;
        let scored: Vec<_> = eval
            .tasks
            .iter()
            .filter_map(|t| t.outcome.as_ref().ok())
            .collect();
        assert!(scored.len() > 50);
        for costs in scored {
            for c in costs {
                assert!(*c >= costs[hindsight] - 1e-12);
            }
        }
        let stats = statistics(&eval, Segment::All, None, &FAST);
        assert_eq!(
            stat(&stats, "regret/hindsight/across_day/4h/13")
                .estimate
                .point,
            Some(0.0)
        );
    }

    #[test]
    fn task_windows_follow_local_days_and_the_decision_span() {
        // 2026-03-29 has 23 hours in Helsinki.
        let eval = default_eval(&build("2026-03-15T00:00:00Z", 24, None));
        for t in &eval.tasks {
            assert!(t.task.earliest_start_ms >= t.t_ms);
            assert!(
                t.task.deadline_ms <= publication_horizon_ms(t.t_ms).unwrap() + 3 * DAY_MS,
                "a task ends after the nominal series end"
            );
        }
        let short_day = ms("2026-03-28T22:00:00Z");
        let task = eval
            .tasks
            .iter()
            .find(|t| t.kind == TaskKind::WithinDay && t.task.earliest_start_ms == short_day)
            .unwrap();
        assert_eq!(task.window_quarters, 92);
        assert_eq!(task.task.deadline_ms - short_day, 23 * HOUR_MS);
    }

    #[test]
    fn mixed_and_fully_published_windows_are_visible() {
        let eval = default_eval(&build("2026-02-01T00:00:00Z", 24, None));
        let at13 = |kind| {
            eval.tasks
                .iter()
                .filter(move |t| t.hour_utc == 13 && t.kind == kind)
        };
        assert!(at13(TaskKind::WithinDay).any(|t| t.outcome == Err(Exclusion::FullyPublished)));
        assert!(at13(TaskKind::AcrossDay).any(|t| {
            t.outcome.is_ok()
                && t.published_quarters > 0
                && t.published_quarters < t.window_quarters
        }));
        // At 06:00 UTC the across-day window of today mixes today's
        // published afternoon with tomorrow's forecast.
        assert!(eval.tasks.iter().any(|t| {
            t.hour_utc == 6
                && t.kind == TaskKind::AcrossDay
                && t.outcome.is_ok()
                && t.published_quarters > 0
        }));
    }

    #[test]
    fn a_missing_price_day_is_excluded_and_counted() {
        let mut data = build("2026-02-01T00:00:00Z", 24, None);
        let gap = ms("2026-02-12T23:00:00Z");
        data.prices.retain(|p| {
            !(p.area == "FI" && (gap..gap + DAY_MS).contains(&p.start.as_millisecond()))
        });
        let eval = default_eval(&data);
        let reasons: BTreeSet<Exclusion> = eval
            .tasks
            .iter()
            .filter_map(|t| t.outcome.as_ref().err().copied())
            .collect();
        assert!(reasons.contains(&Exclusion::MissingRealised));
        // Without the day, the issuance before it starts its series a day
        // early, so the nominal last day has no forecast.
        assert!(reasons.contains(&Exclusion::MissingForecast(0)));
        assert!(
            eval.targets
                .iter()
                .any(|t| (gap..gap + DAY_MS).contains(&t.target_ms) && t.actual.is_none())
        );
    }

    // schedule_arms on hand-made inputs

    const ORIGIN: &str = "2026-02-10T23:00:00Z";

    /// Issue-time inputs whose published prices end at `ORIGIN`.
    /// `published` holds the quarters before `ORIGIN`; `history` sets the
    /// last-week and last-published-day values of the quarters after it.
    fn hand_inputs(published: &[f64], history: f64) -> (IssueTimeInputs, BTreeMap<i64, f64>) {
        let origin = ms(ORIGIN);
        let mut fi_prices = QuarterMap::new();
        let mut realised = BTreeMap::new();
        for (i, v) in published.iter().enumerate() {
            let q = origin - (i64::try_from(published.len() - i).unwrap()) * QUARTER_MS;
            fi_prices.insert(QuarterStart::from_millisecond(q), *v);
            realised.insert(q, *v);
        }
        for i in 0..16 {
            let q = origin + i * QUARTER_MS;
            for back in [DAY_MS, 7 * DAY_MS] {
                fi_prices.insert(QuarterStart::from_millisecond(q - back), history);
            }
        }
        let inputs = IssueTimeInputs {
            t_ms: origin - 10 * HOUR_MS,
            last_published_ms: Some(origin - QUARTER_MS),
            origin_ms: origin,
            fi_prices,
            neighbor_prices: BTreeMap::new(),
            forecast: BTreeMap::new(),
            forecast_ladder: BTreeMap::new(),
            actual: BTreeMap::new(),
            older_fi_prices: QuarterMap::new(),
            older_neighbor_prices: BTreeMap::new(),
            older_actual: BTreeMap::new(),
        };
        (inputs, realised)
    }

    fn quarters_from(start: i64, values: &[f64]) -> BTreeMap<i64, f64> {
        values
            .iter()
            .enumerate()
            .map(|(i, v)| (start + i64::try_from(i).unwrap() * QUARTER_MS, *v))
            .collect()
    }

    fn hand_task(before: i64, after: i64, quarters: usize) -> ScheduleTask {
        let origin = ms(ORIGIN);
        ScheduleTask {
            earliest_start_ms: origin - before * QUARTER_MS,
            deadline_ms: origin + after * QUARTER_MS,
            quarters,
        }
    }

    fn spot() -> Pricer {
        Pricer::new(&CostBasis::Spot).unwrap()
    }

    #[test]
    fn every_arm_gets_the_same_published_prices() {
        let origin = ms(ORIGIN);
        let (inputs, mut realised) = hand_inputs(&[9.0, 0.0, 0.0, 9.0], 50.0);
        realised.extend(quarters_from(origin, &[5.0, 5.0, 5.0, 5.0]));
        // The forecast also has values for published quarters; they are
        // never read.
        let mut forecast = quarters_from(origin - 4 * QUARTER_MS, &[-100.0; 4]);
        forecast.extend(quarters_from(origin, &[6.0; 4]));
        let costs = schedule_arms(
            &inputs,
            &hand_task(4, 4, 2),
            &[forecast],
            &realised,
            &spot(),
        )
        .unwrap();
        // A, last_week, last_published_day, earliest_start, hindsight.
        assert_eq!(costs, vec![0.0, 0.0, 0.0, 4.5, 0.0]);
    }

    #[test]
    fn the_forecast_decides_only_the_unpublished_part() {
        let origin = ms(ORIGIN);
        let (inputs, mut realised) = hand_inputs(&[4.0, 4.0], 3.0);
        realised.extend(quarters_from(origin, &[8.0, 1.0, 1.0, 8.0]));
        let good = quarters_from(origin, &[9.0, 0.0, 0.0, 9.0]);
        let bad = quarters_from(origin, &[0.0, 9.0, 9.0, 0.0]);
        let costs = schedule_arms(
            &inputs,
            &hand_task(2, 4, 2),
            &[good, bad],
            &realised,
            &spot(),
        )
        .unwrap();
        // good, bad, last_week (flat 3: the earliest of the tied starts), last
        // published day (same), earliest_start, hindsight.
        assert_eq!(costs, vec![1.0, 6.0, 4.5, 4.5, 4.0, 1.0]);
    }

    #[test]
    fn a_tie_takes_the_earliest_start_for_every_arm() {
        let origin = ms(ORIGIN);
        let (inputs, mut realised) = hand_inputs(&[], 2.0);
        realised.extend(quarters_from(origin, &[2.0, 7.0, 2.0, 7.0, 2.0, 7.0]));
        let flat = quarters_from(origin, &[2.0; 6]);
        let costs =
            schedule_arms(&inputs, &hand_task(0, 6, 1), &[flat], &realised, &spot()).unwrap();
        assert_eq!(costs, vec![2.0, 2.0, 2.0, 2.0, 2.0]);
    }

    #[test]
    fn a_task_is_excluded_for_every_arm_when_one_arm_cannot_choose() {
        let origin = ms(ORIGIN);
        let (inputs, mut realised) = hand_inputs(&[1.0, 1.0], 3.0);
        realised.extend(quarters_from(origin, &[1.0; 4]));
        let short = quarters_from(origin, &[1.0; 3]);
        let task = hand_task(2, 4, 2);
        assert_eq!(
            schedule_arms(&inputs, &task, &[short], &realised, &spot()),
            Err(Exclusion::MissingForecast(0))
        );
        let full = quarters_from(origin, &[1.0; 4]);
        let mut no_history = inputs.clone();
        no_history
            .fi_prices
            .retain(|q, _| q.as_millisecond() >= origin - DAY_MS);
        assert_eq!(
            schedule_arms(
                &no_history,
                &task,
                std::slice::from_ref(&full),
                &realised,
                &spot()
            ),
            Err(Exclusion::MissingLastWeek)
        );
        let mut gap = realised.clone();
        gap.remove(&(origin + QUARTER_MS));
        assert_eq!(
            schedule_arms(&inputs, &task, std::slice::from_ref(&full), &gap, &spot()),
            Err(Exclusion::MissingRealised)
        );
        assert_eq!(
            schedule_arms(
                &inputs,
                &hand_task(2, 0, 2),
                std::slice::from_ref(&full),
                &realised,
                &spot()
            ),
            Err(Exclusion::FullyPublished)
        );
        assert_eq!(
            schedule_arms(&inputs, &hand_task(0, 3, 4), &[full], &realised, &spot()),
            Err(Exclusion::Infeasible)
        );
    }

    // Bootstrap and split

    #[test]
    fn the_bootstrap_is_reproducible_from_its_seed() {
        let eval = default_eval(&build("2026-02-01T00:00:00Z", 24, None));
        let a = statistics(&eval, Segment::All, None, &FAST);
        assert_eq!(a, statistics(&eval, Segment::All, None, &FAST));
        let other = statistics(
            &eval,
            Segment::All,
            None,
            &BootstrapOptions { seed: 7, ..FAST },
        );
        assert_ne!(a, other);
        let points = |s: &[Statistic]| s.iter().map(|x| x.estimate.point).collect::<Vec<_>>();
        assert_eq!(points(&a), points(&other));
    }

    #[test]
    fn a_default_block_covers_the_target_span_of_one_issuance() {
        let eval = default_eval(&build("2026-02-01T00:00:00Z", 24, None));
        let max_lead = eval
            .targets
            .iter()
            .map(TargetRecord::lead_hours)
            .fold(0.0, f64::max);
        #[expect(clippy::cast_precision_loss, reason = "a small constant")]
        let block_hours = MIN_OVERLAP_BLOCK_DAYS as f64 * 24.0;
        assert!(max_lead < block_hours, "{max_lead} h");
    }

    #[test]
    fn the_split_keeps_both_issuances_of_a_day_in_one_segment() {
        let eval = default_eval(&build("2026-02-01T00:00:00Z", 24, None));
        let split = Some(Split {
            selection_start_ms: ms("2026-02-09T00:00:00Z"),
            test_start_ms: ms("2026-02-17T00:00:00Z"),
        });
        let segments = [Segment::Train, Segment::Selection, Segment::Test];
        for issue in &eval.issues {
            let inside = segments
                .iter()
                .filter(|s| s.contains(issue.t_ms, split))
                .count();
            assert_eq!(inside, 1);
        }
        let by_day: BTreeMap<i64, BTreeSet<Segment>> =
            eval.issues.iter().fold(BTreeMap::new(), |mut acc, i| {
                let segment = *segments.iter().find(|s| s.contains(i.t_ms, split)).unwrap();
                acc.entry(i.t_ms.div_euclid(DAY_MS))
                    .or_default()
                    .insert(segment);
                acc
            });
        assert!(by_day.values().all(|s| s.len() == 1));
        let test = statistics(&eval, Segment::Test, split, &FAST);
        let all = statistics(&eval, Segment::All, None, &FAST);
        let id = "mae/A/13/D+2";
        assert!(stat(&test, id).n < stat(&all, id).n);
    }

    #[test]
    fn a_segment_never_scores_targets_or_tasks_in_the_next_segment_period() {
        let eval = default_eval(&build("2026-02-01T00:00:00Z", 24, None));
        let (selection_start, test_start) =
            (ms("2026-02-09T00:00:00Z"), ms("2026-02-17T00:00:00Z"));
        let split = Some(Split {
            selection_start_ms: selection_start,
            test_start_ms: test_start,
        });
        for (segment, from, end) in [
            (Segment::Train, i64::MIN, selection_start),
            (Segment::Selection, selection_start, test_start),
        ] {
            let issued = |t_ms: i64| (from..end).contains(&t_ms);
            // The issuances near the boundary do score targets after it, so
            // the check below is not empty.
            assert!(
                eval.targets
                    .iter()
                    .any(|t| issued(t.t_ms) && t.complete() && t.target_ms >= end)
            );
            let stats = statistics(&eval, segment, split, &FAST);
            for key in ["06/D+1", "06/D+3", "13/D+2", "13/D+4"] {
                let day_ahead: i64 = key[4..].parse().unwrap();
                let hour: u8 = key[..2].parse().unwrap();
                let inside = eval
                    .targets
                    .iter()
                    .filter(|t| {
                        issued(t.t_ms)
                            && t.hour_utc == hour
                            && t.day_ahead == day_ahead
                            && t.complete()
                            && t.target_ms < end
                    })
                    .count();
                let got = stats
                    .iter()
                    .find(|s| s.id == format!("mae/A/{key}"))
                    .map_or(0, |s| s.n);
                assert_eq!(got, inside, "{segment:?} mae/A/{key}");
            }
            for kind in [TaskKind::WithinDay, TaskKind::AcrossDay] {
                let key = format!("{}/4h/13", kind.label());
                let inside = eval
                    .tasks
                    .iter()
                    .filter(|t| {
                        issued(t.t_ms)
                            && t.hour_utc == 13
                            && t.kind == kind
                            && t.task.quarters == 16
                            && t.outcome.is_ok()
                            && t.task.deadline_ms <= end
                    })
                    .count();
                let got = stats
                    .iter()
                    .find(|s| s.id == format!("cost/A/{key}"))
                    .map_or(0, |s| s.n);
                assert_eq!(got, inside, "{segment:?} cost/A/{key}");
            }
        }
    }

    // CLI

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "spot-price-tools-eval-{}-{name}",
            std::process::id()
        ))
    }

    fn run_cli(args: &[&str]) -> Output {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let env = |_: &str| None;
        let ctx = Context {
            env: &env,
            now: "2026-10-20T00:00:00Z".parse().unwrap(),
            workspace_root: &root,
        };
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        run(&args, &ctx)
    }

    fn fixture(name: &str) -> PathBuf {
        let file = temp_file(name);
        std::fs::write(
            &file,
            to_fixture_json(&build("2026-02-01T00:00:00Z", 24, None)),
        )
        .unwrap();
        file
    }

    #[test]
    fn the_report_names_its_version_inputs_assumptions_and_issuances() {
        let file = fixture("report.json");
        let got = run_cli(&[
            "backtest",
            "--eval",
            "--data",
            file.to_str().unwrap(),
            "--resamples",
            "100",
        ]);
        std::fs::remove_file(&file).unwrap();
        let stdout = got.text(Stream::Stdout);
        assert_eq!(got.code, 0, "{}", got.text(Stream::Stderr));
        assert!(stdout.contains("FI forecast evaluation (forecast-eval v1"));
        assert!(stdout.contains("input snapshot:"));
        assert!(stdout.contains("issuances (UTC):     06:00, 13:00"));
        assert!(stdout.contains("A1 publication:"));
        assert!(stdout.contains("A3 grid actuals:"));
        assert!(stdout.contains("cost basis:          spot price"));
        assert!(stdout.contains("== segment: all"));
        assert!(stdout.contains("regret/A/across_day/2h/06"));
        assert!(stdout.contains("rmae/A/last_published_day/13/D+4"));
        assert!(stdout.contains("not a year-round result"));
    }

    #[test]
    fn the_report_declares_a_contract_cost_scenario() {
        let file = fixture("contract.json");
        let got = run_cli(&[
            "backtest",
            "--eval",
            "--data",
            file.to_str().unwrap(),
            "--resamples",
            "50",
            "--cost",
            "contract:margin=0.5,day=4,night=2,tax=2.8,vat=25.5,night_start=22,night_end=7",
        ]);
        std::fs::remove_file(&file).unwrap();
        let stdout = got.text(Stream::Stdout);
        assert!(stdout.contains(
            "declared contract scenario margin=0.5 day=4 night=2 tax=2.8 vat=25.5% night=22–07"
        ));
        assert!(stdout.contains("Not a historical user setting."));
    }

    #[test]
    fn the_test_segment_is_sealed_until_criteria_are_declared() {
        let file = fixture("sealed.json");
        let data = file.to_str().unwrap();
        let sealed = run_cli(&[
            "backtest",
            "--eval",
            "--data",
            data,
            "--resamples",
            "50",
            "--split",
            "2026-02-09,2026-02-17",
        ]);
        let stdout = sealed.text(Stream::Stdout);
        assert!(stdout.contains("== segment: train"));
        assert!(stdout.contains("== segment: selection"));
        assert!(stdout.contains("SEALED"));
        assert!(stdout.contains("crosses segment boundary"));
        assert!(!stdout.contains("== segment: test  issuance days"));

        let criteria = temp_file("criteria.json");
        std::fs::write(
            &criteria,
            r#"{"experiment":"self check","declared":"2026-10-20",
                "success":[{"id":"mae/A/13/D+2","statistic":"point","op":">=","value":0}],
                "regression":[{"id":"rmae/A/last_week/13/D+2","statistic":"ci_low","op":">","value":1000},
                              {"id":"mae/A/13/D+9","statistic":"point","op":">","value":0}]}"#,
        )
        .unwrap();
        let opened = run_cli(&[
            "backtest",
            "--eval",
            "--data",
            data,
            "--resamples",
            "50",
            "--split",
            "2026-02-09,2026-02-17",
            "--criteria",
            criteria.to_str().unwrap(),
        ]);
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_file(&criteria).unwrap();
        let stdout = opened.text(Stream::Stdout);
        assert!(stdout.contains("== segment: test  issuance days"));
        assert!(stdout.contains("== criteria on segment test: \"self check\" declared 2026-10-20"));
        assert!(stdout.contains("success:    MET"));
        assert!(stdout.contains("→ MISSING"));
        assert!(stdout.contains("regression: UNDETERMINED"));
    }

    #[test]
    fn flag_errors_are_reported_before_any_data_is_read() {
        let cases = [
            (
                vec!["--issuances", "24"],
                "--issuances must be a whole number",
            ),
            (vec!["--issuances", "6,6"], "--issuances repeats hour 6"),
            (
                vec!["--cost", "contract:margin=1"],
                "--cost contract needs day",
            ),
            (vec!["--cost", "fixed"], "--cost must be spot or contract"),
            (
                vec!["--split", "2026-03-01,2026-02-01"],
                "--split selection start must be before the test start",
            ),
            (
                vec!["--compare"],
                "--eval and --compare cannot be used together",
            ),
            (vec!["--block-days", "0"], "--block-days must be at least 1"),
            (vec!["--ab", "nope=1"], "--ab key must be"),
        ];
        for (extra, message) in cases {
            let mut args = vec!["backtest", "--eval", "--db"];
            args.extend(extra.iter());
            let got = run_cli(&args);
            assert_eq!(got.code, 1, "{extra:?}");
            assert!(
                got.text(Stream::Stderr).contains(message),
                "{extra:?}: {}",
                got.text(Stream::Stderr)
            );
            assert!(!got.text(Stream::Stderr).contains("DATABASE_PUBLIC_URL"));
        }
    }

    #[test]
    fn a_criteria_file_without_criteria_or_with_unknown_keys_is_rejected() {
        let cases = [
            (
                r#"{"experiment":"x","declared":"d","success":[],"regression":[]}"#,
                "declares no criterion",
            ),
            (
                r#"{"experiment":"x","declared":"d","success":[{"id":"a","statistic":"point","op":"<","value":1,"extra":2}],"regression":[]}"#,
                "unknown field",
            ),
            (
                r#"{"experiment":"x","declared":"d","success":[{"id":"a","statistic":"mean","op":"<","value":1}],"regression":[]}"#,
                "unknown variant",
            ),
        ];
        for (index, (text, message)) in cases.iter().enumerate() {
            let file = temp_file(&format!("bad-criteria-{index}.json"));
            std::fs::write(&file, text).unwrap();
            let got = run_cli(&[
                "backtest",
                "--eval",
                "--db",
                "--criteria",
                file.to_str().unwrap(),
            ]);
            std::fs::remove_file(&file).unwrap();
            assert_eq!(got.code, 1);
            assert!(
                got.text(Stream::Stderr).contains(message),
                "{}",
                got.text(Stream::Stderr)
            );
        }
    }

    #[test]
    fn thin_data_is_not_a_verdict() {
        let file = temp_file("thin.json");
        std::fs::write(
            &file,
            to_fixture_json(&build("2026-02-01T00:00:00Z", 8, None)),
        )
        .unwrap();
        let got = run_cli(&[
            "backtest",
            "--eval",
            "--data",
            file.to_str().unwrap(),
            "--resamples",
            "20",
        ]);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(got.code, 1);
        assert!(got.text(Stream::Stderr).contains("not yet a verdict"));
    }

    #[test]
    fn the_usage_documents_eval() {
        let usage = run_cli(&["backtest", "--eval"]);
        assert_eq!(usage.code, 2);
        let text = usage.text(Stream::Stderr);
        assert!(text.contains("--eval  the evaluation report"));
        assert!(text.contains("--criteria <file.json>"));
    }
}
