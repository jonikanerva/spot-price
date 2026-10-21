//! The shape-profile comparison of the evaluation report (`backtest --eval
//! --profiles`): the profile arms against arm A and the baselines, missing
//! profile prices, the DST change, the selection, the flags, and the
//! declared criteria files.

#[cfg(test)]
mod profile_eval {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use jiff::Timestamp;
    use spot_price_tools::backtest::{AsOfMode, ForecastVariant, reconstruct_issue_time};
    use spot_price_tools::backtest_cli::parse_variant;
    use spot_price_tools::bootstrap::BootstrapOptions;
    use spot_price_tools::cli::{Context, Output, Stream, run};
    use spot_price_tools::criteria::load_criteria;
    use spot_price_tools::data::{BacktestData, GridActual, PricePoint, Vintage, to_fixture_json};
    use spot_price_tools::eval_cli::{parse_profiles, profile_arms};
    use spot_price_tools::evaluation::{
        ArmKind, EvalOptions, Evaluation, IssueStatus, ModelArm, Segment, Split, TargetRecord,
        evaluate, statistics,
    };
    use spot_price_tools::profile::{ShapeFamily, profile_at_issue_time};
    use spot_price_tools::schedule::CostBasis;

    const QUARTER_MS: i64 = 15 * 60 * 1000;
    const HOUR_MS: i64 = 60 * 60 * 1000;
    const DAY_MS: i64 = 24 * HOUR_MS;

    const FAST: BootstrapOptions = BootstrapOptions {
        seed: 156,
        resamples: 50,
        block_days: 7,
    };

    fn ms(text: &str) -> i64 {
        text.parse::<Timestamp>().unwrap().as_millisecond()
    }

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_millisecond(ms).unwrap()
    }

    /// A small integer as a float.
    fn int(value: i64) -> f64 {
        f64::from(i32::try_from(value).unwrap())
    }

    /// Mean-zero pseudo-noise in [-1, 1] per quarter index.
    fn noise_at(q: i64) -> f64 {
        let s = (int(q % 100_000) * 12.9898).sin() * 43758.5453;
        (s - s.floor() - 0.5) * 2.0
    }

    /// `days` days of quarters from `start`. The FI price follows the grid,
    /// a weekday step, and noise. Wind vintages cover every lead of the
    /// served window; consumption vintages stop at 60 h.
    fn build(start: &str, days: i64) -> BacktestData {
        let start = ms(start);
        let mut data = BacktestData::default();
        let (mut wind75, mut cons124, mut wind245, mut cons165) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for q in 0..days * 96 {
            let at = start + q * QUARTER_MS;
            let consumption = 8000.0 + int(q % 96) * 15.0;
            let wind = 2000.0 + int((q * 131) % 1800);
            let weekday = int(at.div_euclid(DAY_MS) % 7);
            let fi = 0.0012 * (consumption - wind) + 2.0 + 0.4 * weekday + 1.5 * noise_at(q);
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
                wind245.push(vintage(wind + 3.0 * int(lead), 245, lead));
            }
            for lead in [2, 30, 60] {
                cons165.push(vintage(consumption, 165, lead));
            }
        }
        data.actuals = BTreeMap::from([(75, wind75), (124, cons124)]);
        data.vintages = BTreeMap::from([(245, wind245), (165, cons165)]);
        data
    }

    fn arm_a() -> ModelArm {
        ModelArm {
            name: "A".to_owned(),
            variant: ForecastVariant::default(),
            kind: ArmKind::Model,
        }
    }

    fn eval_with(data: &BacktestData, families: &[ShapeFamily]) -> Evaluation {
        eval_against(data, arm_a(), families)
    }

    fn eval_against(data: &BacktestData, a: ModelArm, families: &[ShapeFamily]) -> Evaluation {
        let mut arms = vec![a];
        arms.extend(profile_arms(families));
        evaluate(
            data,
            &EvalOptions {
                issuance_hours_utc: vec![6, 13],
                arms,
                cost: CostBasis::Spot,
            },
        )
        .unwrap()
    }

    fn index(eval: &Evaluation, name: &str) -> usize {
        eval.arm_names.iter().position(|n| n == name).unwrap()
    }

    fn value(t: &TargetRecord, arm: usize) -> Option<f64> {
        t.predictions[arm]
    }

    // Arms

    #[test]
    fn the_profile_arms_are_named_by_family_and_kind() {
        let families = parse_profiles("prev_day, prev_week,mean3w").unwrap();
        let arms = profile_arms(&families);
        let names: Vec<&str> = arms.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "shape_prev_day",
                "shape_prev_week",
                "shape_mean3w",
                "pure_prev_day",
                "pure_prev_week",
                "pure_mean3w"
            ]
        );
        for arm in &arms {
            let mut rest = arm.variant;
            rest.shape_profile = ShapeFamily::SERVED;
            assert_eq!(rest, ForecastVariant::default(), "{}", arm.name);
            assert!(!arm.name.contains(['-', '/']));
        }
        assert_eq!(arms[2].variant.shape_profile, ShapeFamily::WeekdayMean(3));
        assert_eq!(arms[5].kind, ArmKind::PureProfile);
    }

    #[test]
    fn arm_a_has_the_served_family_and_persistence_is_a_listed_family() {
        assert_eq!(ShapeFamily::SERVED, ShapeFamily::WeekdayMean(4));
        assert_eq!(
            ForecastVariant::default().shape_profile,
            ShapeFamily::SERVED
        );
        assert_eq!(
            parse_profiles("persistence,mean3w").unwrap(),
            [ShapeFamily::Persistence, ShapeFamily::WeekdayMean(3)]
        );
        let names: Vec<String> = profile_arms(&[ShapeFamily::Persistence])
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(names, ["shape_persistence", "pure_persistence"]);
        assert_eq!(
            parse_variant("shape_profile=persistence")
                .unwrap()
                .shape_profile,
            ShapeFamily::Persistence
        );
    }

    #[test]
    fn arm_a_is_the_served_forecast_with_or_without_profile_arms() {
        let data = build("2026-05-01T00:00:00Z", 16);
        let alone = eval_with(&data, &[]);
        let with = eval_with(
            &data,
            &[ShapeFamily::PreviousDay, ShapeFamily::WeekdayMean(2)],
        );
        assert_eq!(alone.targets.len(), with.targets.len());
        for (a, b) in alone.targets.iter().zip(&with.targets) {
            assert_eq!(a.target_ms, b.target_ms);
            assert_eq!(value(a, 0).map(f64::to_bits), value(b, 0).map(f64::to_bits));
        }
    }

    #[test]
    fn each_family_reads_the_persistence_profile_where_the_persistence_rule_reads_it() {
        // May has no DST change, so the fixed offsets of the persistence rule
        // are whole local days.
        let data = build("2026-05-01T00:00:00Z", 20);
        let (mut first, mut later) = (0, 0);
        let mut t_ms = ms("2026-05-09T06:00:00Z");
        while t_ms < ms("2026-05-17T00:00:00Z") {
            let inputs = reconstruct_issue_time(&data, t_ms, AsOfMode::IssueTime).unwrap();
            let profile = |f| profile_at_issue_time(&inputs, f).unwrap();
            let (persistence, day, week) = (
                profile(ShapeFamily::Persistence),
                profile(ShapeFamily::PreviousDay),
                profile(ShapeFamily::PreviousWeek),
            );
            assert_eq!(persistence.missing + day.missing + week.missing, 0);
            for (key, value) in &persistence.values {
                // The first unpublished delivery day reads the day before;
                // later days read the week before.
                if key.as_millisecond() < inputs.origin_ms + DAY_MS {
                    assert_eq!(day.values.get(key), Some(value));
                    first += 1;
                } else {
                    assert_eq!(week.values.get(key), Some(value));
                    later += 1;
                }
            }
            t_ms += 7 * HOUR_MS;
        }
        assert!(first > 1000 && later > 2000, "{first} {later}");

        // The model arms differ from arm A, since each reads one profile
        // on every day.
        let eval = eval_with(
            &data,
            &[ShapeFamily::PreviousDay, ShapeFamily::PreviousWeek],
        );
        for name in ["shape_prev_day", "shape_prev_week"] {
            let arm = index(&eval, name);
            let differ = eval
                .targets
                .iter()
                .filter(|t| t.complete() && value(t, 0) != value(t, arm))
                .count();
            assert!(differ > 500, "{name} {differ}");
        }
    }

    #[test]
    fn the_pure_previous_day_and_week_equal_the_fixed_baselines_without_dst() {
        let data = build("2026-05-01T00:00:00Z", 20);
        let eval = eval_with(
            &data,
            &[ShapeFamily::PreviousDay, ShapeFamily::PreviousWeek],
        );
        let (day, week) = (
            index(&eval, "pure_prev_day"),
            index(&eval, "pure_prev_week"),
        );
        let mut checked = 0;
        // From the second week, both baselines and both profiles have their
        // whole history.
        let from = ms("2026-05-09T00:00:00Z");
        for t in eval
            .targets
            .iter()
            .filter(|t| t.actual.is_some() && t.t_ms >= from)
        {
            if let Some(baseline) = t.last_published_day {
                assert_eq!(value(t, day), Some(baseline), "{t:?}");
            }
            if let Some(baseline) = t.last_week {
                assert_eq!(value(t, week), Some(baseline), "{t:?}");
                checked += 1;
            }
        }
        assert!(checked > 1000);
    }

    // Unavailable profiles

    #[test]
    fn a_missing_profile_price_is_counted_and_unpairs_its_target() {
        let mut data = build("2026-05-01T00:00:00Z", 20);
        // Remove one FI hour; a week later its profile has no price.
        let gone = ms("2026-05-10T09:00:00Z")..ms("2026-05-10T10:00:00Z");
        data.prices
            .retain(|p| p.area != "FI" || !gone.contains(&p.start.as_millisecond()));
        let eval = eval_with(&data, &[ShapeFamily::WeekdayMean(2)]);
        let (shape, pure) = (index(&eval, "shape_mean2w"), index(&eval, "pure_mean2w"));
        let fallback = |arm: usize| -> usize {
            eval.issues
                .iter()
                .filter_map(|i| match &i.status {
                    IssueStatus::Forecast(f) => Some(f.shape_fallback[arm]),
                    _ => None,
                })
                .sum()
        };
        assert!(fallback(shape) > 0);
        assert_eq!(fallback(shape), fallback(pure));
        let target = ms("2026-05-17T09:15:00Z");
        let records: Vec<_> = eval
            .targets
            .iter()
            .filter(|t| t.target_ms == target)
            .collect();
        assert_ne!(records.len(), 0);
        for t in records {
            // The model keeps a value (the flat daily level), the pure
            // profile has none, so no arm is paired on the target.
            assert!(value(t, shape).is_some());
            assert_eq!(value(t, pure), None);
            assert!(!t.complete());
        }
    }

    // DST: Europe/Oslo skips 02:00–03:00 local on Sunday 29 March 2026.

    #[test]
    fn the_skipped_hour_is_a_counted_dst_gap_of_the_calendar_profiles() {
        let data = build("2026-03-10T00:00:00Z", 30);
        let eval = eval_with(
            &data,
            &[ShapeFamily::Persistence, ShapeFamily::PreviousWeek],
        );
        let (persistence, shape, pure) = (
            index(&eval, "shape_persistence"),
            index(&eval, "shape_prev_week"),
            index(&eval, "pure_prev_week"),
        );
        let gap = |arm: usize| -> usize {
            eval.issues
                .iter()
                .filter_map(|i| match &i.status {
                    IssueStatus::Forecast(f) => Some(f.dst_gap[arm]),
                    _ => None,
                })
                .sum()
        };
        // The fixed offsets of persistence never hit the skipped hour; the
        // served four-week mean (arm A) reads calendar days and does.
        assert_eq!(gap(persistence), 0);
        assert!(gap(0) > 0);
        assert!(gap(shape) > 0);
        assert_eq!(gap(shape), gap(pure));
        // Sunday 5 April 02:15 CEST reads 02:15 on 29 March, which does not
        // exist; 03:15 CEST reads 03:15 CEST.
        let at = |text| {
            eval.targets
                .iter()
                .find(|t| t.target_ms == ms(text) && t.actual.is_some())
                .unwrap()
        };
        assert_eq!(value(at("2026-04-05T00:15:00Z"), pure), None);
        let after = at("2026-04-05T01:15:00Z");
        let source = data
            .prices
            .iter()
            .find(|p| p.area == "FI" && p.start.as_millisecond() == ms("2026-03-29T01:15:00Z"))
            .unwrap();
        assert_eq!(value(after, pure), Some(source.spot_cents_kwh));
    }

    // Selection and criteria

    #[test]
    fn the_selection_segment_names_a_window_and_a_candidate() {
        let file = fixture("selection.json", 36);
        let got = run_cli(&[
            "backtest",
            "--eval",
            "--data",
            file.to_str().unwrap(),
            "--resamples",
            "50",
            "--profiles",
            "prev_day,prev_week,mean2w,mean3w",
            "--split",
            "2026-05-20,2026-05-28",
        ]);
        // A profile through --ab alone is one arm B, not a comparison.
        let ab = run_cli(&[
            "backtest",
            "--eval",
            "--data",
            file.to_str().unwrap(),
            "--resamples",
            "50",
            "--ab",
            "shape_profile=prev_week",
            "--split",
            "2026-05-20,2026-05-28",
        ]);
        std::fs::remove_file(&file).unwrap();
        let ab = ab.text(Stream::Stdout);
        assert!(ab.contains("arm B:") && ab.contains(" shape_profile=prev_week"));
        assert!(ab.contains("profile availability"));
        assert!(!ab.contains("profile selection on segment"));
        let stdout = got.text(Stream::Stdout);
        assert!(stdout.contains("SHAPE PROFILES"), "{stdout}");
        assert!(stdout.contains("P5 DST:"));
        assert!(stdout.contains("arm shape_prev_day:"));
        assert!(stdout.contains("shape_profile=prev_day"));
        assert!(stdout.contains("arm pure_mean3w:               pure profile mean3w"));
        assert!(stdout.contains("profile availability"));
        assert_eq!(stdout.matches("profile selection on segment").count(), 1);
        assert!(stdout.contains("profile selection on segment selection"));
        assert!(stdout.contains("selected weekday-mean window: shape_mean"));
        assert!(stdout.contains("selected candidate: "));
        assert!(stdout.contains("SEALED"));
        assert!(stdout.contains("mae_delta/shape_mean3w-A/13/D+3"));
    }

    #[test]
    fn profile_flag_errors_are_reported_before_any_data_is_read() {
        for (raw, message) in [
            ("served", "--profiles a shape profile cannot be \"served\""),
            (
                "mean4w",
                "--profiles cannot list mean4w: arm A is the served forecast",
            ),
            ("prev_day,prev_day", "--profiles repeats prev_day"),
            ("mean5w", "--profiles a shape profile must be"),
            ("", "--profiles a shape profile must be"),
        ] {
            let got = run_cli(&["backtest", "--eval", "--db", "--profiles", raw]);
            assert_eq!(got.code, 1, "{raw}");
            assert!(got.text(Stream::Stderr).contains(message), "{raw}");
        }
        let got = run_cli(&["backtest", "--eval", "--db", "--ab", "shape_profile=weekly"]);
        assert!(
            got.text(Stream::Stderr)
                .contains("--ab a shape profile must be")
        );
    }

    fn criteria_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("criteria/profile-families")
    }

    #[test]
    fn every_declared_criterion_names_a_statistic_of_the_comparison() {
        let data = build("2026-05-01T00:00:00Z", 44);
        // The files were declared against arm A with the persistence shape,
        // which was the served shape then.
        let reference = ModelArm {
            variant: ForecastVariant {
                shape_profile: ShapeFamily::Persistence,
                ..ForecastVariant::default()
            },
            ..arm_a()
        };
        let families = [
            ShapeFamily::PreviousDay,
            ShapeFamily::PreviousWeek,
            ShapeFamily::WeekdayMean(2),
            ShapeFamily::WeekdayMean(3),
            ShapeFamily::WeekdayMean(4),
        ];
        let eval = eval_against(&data, reference, &families);
        let split = Split {
            selection_start_ms: ms("2026-05-30T00:00:00Z"),
            test_start_ms: ms("2026-06-06T00:00:00Z"),
        };
        let stats = statistics(&eval, Segment::Test, Some(split), &FAST);
        let ids: Vec<&str> = stats.iter().map(|s| s.id.as_str()).collect();
        let mut files: Vec<PathBuf> = std::fs::read_dir(criteria_dir())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        let names: Vec<String> = files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "mean2w.json",
                "mean3w.json",
                "mean4w.json",
                "prev_day.json",
                "prev_week.json"
            ]
        );
        for file in files {
            let declared = load_criteria(&file).unwrap();
            let c = &declared.criteria;
            let family = file.file_stem().unwrap().to_string_lossy().into_owned();
            assert!(
                !c.success.is_empty() && !c.regression.is_empty(),
                "{family}"
            );
            for criterion in c.success.iter().chain(&c.regression) {
                assert!(
                    ids.contains(&criterion.id.as_str()),
                    "{family}: {} is not a statistic",
                    criterion.id
                );
                assert!(
                    criterion.id.contains(&format!("shape_{family}-A/")),
                    "{family}: {}",
                    criterion.id
                );
            }
        }
    }

    // CLI helpers

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "spot-price-tools-profile-{}-{name}",
            std::process::id()
        ))
    }

    fn fixture(name: &str, days: i64) -> PathBuf {
        let file = temp_file(name);
        std::fs::write(&file, to_fixture_json(&build("2026-04-20T00:00:00Z", days))).unwrap();
        file
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
}
