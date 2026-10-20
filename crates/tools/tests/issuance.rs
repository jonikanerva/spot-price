//! The issuance-training arm of the backtest and the evaluation report:
//! the `--ab training=issuance` option, the vintage ladder of an issue time,
//! the leak guard, the example dataset, and the predeclared criteria file.

#[cfg(test)]
mod issuance {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    use jiff::Timestamp;
    use spot_price_domain::forecast::{ExcludedExamples, ISSUANCE_FEATURE_COUNT, InputSource};
    use spot_price_tools::backtest::{
        AsOfMode, ForecastVariant, IssueTimeInputs, Training, find_leaking_inputs,
        forecast_at_issue_time, forecast_at_issue_time_with, issuance_examples_at,
        reconstruct_issue_time, select_vintages_as_of,
    };
    use spot_price_tools::backtest_cli::parse_variant;
    use spot_price_tools::cli::{Context, Output, Stream, run};
    use spot_price_tools::criteria::load_criteria;
    use spot_price_tools::data::{BacktestData, GridActual, PricePoint, Vintage, to_fixture_json};
    use spot_price_tools::profile::ShapeFamily;

    const QUARTER_MS: i64 = 15 * 60 * 1000;
    const HOUR_MS: i64 = 60 * 60 * 1000;
    const DAY_MS: i64 = 24 * HOUR_MS;

    fn ms(text: &str) -> i64 {
        text.parse::<Timestamp>().unwrap().as_millisecond()
    }

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_millisecond(ms).unwrap()
    }

    fn int(value: i64) -> f64 {
        f64::from(i32::try_from(value).unwrap())
    }

    /// Mean-zero pseudo-noise in [-1, 1] per quarter index.
    fn noise_at(q: i64) -> f64 {
        let s = (int(q % 100_000) * 12.9898).sin() * 43_758.545_3;
        (s - s.floor() - 0.5) * 2.0
    }

    /// `days` days of quarters from `start`, with a vintage ladder like the
    /// archive: a 245 issuance every 3 hours covers 6 h back and 72 h ahead,
    /// a 165 issuance 6 h back and 30 h ahead. The wind forecast is biased
    /// high in proportion to its lead.
    fn build(start: &str, days: i64) -> BacktestData {
        build_every(start, days, 3)
    }

    /// [`build`] with a 245/165 issuance every `step_h` hours.
    fn build_every(start: &str, days: i64, step_h: i64) -> BacktestData {
        let start = ms(start);
        let end = start + days * DAY_MS;
        let wind_at = |q: i64| 2000.0 + int((q * 131) % 1800);
        let consumption_at = |q: i64| 8000.0 + int(q % 96) * 15.0;
        let mut data = BacktestData::default();
        let (mut wind75, mut cons124) = (Vec::new(), Vec::new());
        for q in 0..days * 96 {
            let at = start + q * QUARTER_MS;
            let (wind, consumption) = (wind_at(q), consumption_at(q));
            let fi = 0.0012 * (consumption - wind) + 2.0 + 1.5 * noise_at(q);
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
            wind75.push(actual(wind, 75));
            cons124.push(actual(consumption, 124));
        }
        let (mut wind245, mut cons165) = (Vec::new(), Vec::new());
        let mut issued = start + DAY_MS;
        while issued < end {
            let mut target = issued - 6 * HOUR_MS;
            while target < (issued + 72 * HOUR_MS).min(end) {
                let q = (target - start) / QUARTER_MS;
                let lead_h = int((target - issued) / HOUR_MS);
                let vintage = |value, dataset_id| Vintage {
                    dataset_id,
                    issued_at: ts(issued),
                    start: ts(target),
                    end: ts(target + QUARTER_MS),
                    value,
                };
                wind245.push(vintage(wind_at(q) + 5.0 * lead_h, 245));
                if target < issued + 30 * HOUR_MS {
                    cons165.push(vintage(consumption_at(q), 165));
                }
                target += QUARTER_MS;
            }
            issued += step_h * HOUR_MS;
        }
        data.actuals = BTreeMap::from([(75, wind75), (124, cons124)]);
        data.vintages = BTreeMap::from([(245, wind245), (165, cons165)]);
        data
    }

    fn issuance() -> ForecastVariant {
        ForecastVariant {
            training: Training::Issuance,
            ..ForecastVariant::default()
        }
    }

    #[test]
    fn the_training_key_selects_the_arm_and_the_default_stays_served() {
        assert_eq!(ForecastVariant::default().training, Training::Realised);
        assert_eq!(parse_variant("training=issuance").unwrap(), issuance());
        assert_eq!(
            parse_variant("training=realised").unwrap(),
            ForecastVariant::default()
        );
        let err = parse_variant("training=latest").unwrap_err().to_string();
        assert!(
            err.contains("training must be realised or issuance"),
            "{err}"
        );
    }

    #[test]
    fn the_training_and_shape_profile_keys_combine() {
        let both = parse_variant("training=issuance,shape_profile=prev_week").unwrap();
        assert_eq!(
            both,
            ForecastVariant {
                training: Training::Issuance,
                shape_profile: ShapeFamily::PreviousWeek,
                ..ForecastVariant::default()
            }
        );
        let data = build("2026-02-01T00:00:00Z", 40);
        let t = ms("2026-03-05T13:00:00Z");
        let inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime).unwrap();
        let only_shape = ForecastVariant {
            training: Training::Realised,
            ..both
        };
        let a = forecast_at_issue_time_with(&inputs, only_shape).unwrap();
        let b = forecast_at_issue_time_with(&inputs, both).unwrap();
        assert!(!b.used_fallback);
        assert_ne!(a.predicted, b.predicted);
    }

    #[test]
    fn the_ladder_holds_every_admitted_vintage_and_the_served_selection() {
        let data = build("2026-02-01T00:00:00Z", 40);
        let t = ms("2026-03-05T13:00:00Z");
        let inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime).unwrap();
        for id in [245, 165] {
            let ladder = &inputs.forecast_ladder[&id];
            assert!(ladder.iter().all(|v| v.issued_at.as_millisecond() < t));
            // Every target has many vintages: the ladder is not a selection.
            let targets: BTreeSet<i64> = ladder.iter().map(|v| v.start.as_millisecond()).collect();
            assert!(ladder.len() > 3 * targets.len(), "{id}");
            let mut from_ladder = select_vintages_as_of(ladder, Some(t));
            let mut served = inputs.forecast[&id].clone();
            let key = |v: &Vintage| v.start.as_millisecond();
            from_ladder.sort_by_key(key);
            served.sort_by_key(key);
            assert_eq!(from_ladder, served, "{id}");
        }
        let latest = reconstruct_issue_time(&data, t, AsOfMode::Latest).unwrap();
        assert!(
            latest.forecast_ladder[&245]
                .iter()
                .any(|v| v.issued_at.as_millisecond() >= t)
        );
    }

    #[test]
    fn the_leak_guard_sees_a_future_ladder_vintage() {
        let data = build("2026-02-01T00:00:00Z", 40);
        let t = ms("2026-03-05T13:00:00Z");
        let mut inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime).unwrap();
        assert_eq!(find_leaking_inputs(&inputs), Vec::new());
        let mut future = inputs.forecast_ladder[&245][0];
        future.issued_at = ts(t);
        inputs.forecast_ladder.get_mut(&245).unwrap().push(future);
        let leaks = find_leaking_inputs(&inputs);
        assert_eq!(leaks.len(), 1);
        assert_eq!(leaks[0].source, "fingrid_forecast_ladder_245");
    }

    #[test]
    fn the_issuance_arm_trains_on_earlier_issuances_and_leaves_arm_a_alone() {
        let data = build("2026-02-01T00:00:00Z", 40);
        let t = ms("2026-03-05T13:00:00Z");
        let inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime).unwrap();
        let served = forecast_at_issue_time(&inputs).unwrap();
        let a = forecast_at_issue_time_with(&inputs, ForecastVariant::default()).unwrap();
        assert_eq!(a, served);
        let b = forecast_at_issue_time_with(&inputs, issuance()).unwrap();
        assert!(!b.used_fallback);
        assert_eq!(a.training_excluded.total(), 0);
        assert_eq!(b.training_excluded, examples_excluded(&inputs));
        assert_eq!(a.predicted.len(), b.predicted.len());
        assert_ne!(a.predicted, b.predicted);

        let examples = issuance_examples_at(&inputs, issuance()).unwrap();
        assert_eq!(
            examples,
            issuance_examples_at(&inputs, ForecastVariant::default()).unwrap()
        );
        assert!(examples.examples.len() > 96 * 30);
        for e in &examples.examples {
            let issued = e.issued_at.as_millisecond();
            assert!(issued < t);
            assert_eq!((t - issued) % DAY_MS, 0);
            assert!(e.target.as_millisecond() < inputs.origin_ms);
            assert_eq!(e.features.len(), ISSUANCE_FEATURE_COUNT);
            for source in [e.wind, e.consumption] {
                if let InputSource::Vintage { issued_at } = source {
                    assert!(issued_at.unwrap().as_millisecond() < issued);
                }
                assert_ne!(source, InputSource::Actual);
                assert_ne!(source, InputSource::Missing);
            }
        }
    }

    fn examples_excluded(inputs: &IssueTimeInputs) -> ExcludedExamples {
        issuance_examples_at(inputs, issuance()).unwrap().excluded()
    }

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "spot-price-tools-issuance-{}-{name}",
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

    fn criteria_file() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("criteria/issuance-training.json")
    }

    #[test]
    fn the_committed_criteria_file_is_valid() {
        let criteria = load_criteria(&criteria_file()).unwrap();
        assert_eq!(criteria.criteria.declared, "2026-10-20");
        assert_eq!(criteria.criteria.success.len(), 3);
        assert_eq!(criteria.criteria.regression.len(), 8);
    }

    #[test]
    fn the_paired_run_resolves_every_declared_criterion() {
        let file = temp_file("paired.json");
        std::fs::write(&file, to_fixture_json(&build("2026-02-01T00:00:00Z", 46))).unwrap();
        let criteria = criteria_file();
        let got = run_cli(&[
            "backtest",
            "--eval",
            "--data",
            file.to_str().unwrap(),
            "--ab",
            "training=issuance",
            "--split",
            "2026-03-07,2026-03-11",
            "--criteria",
            criteria.to_str().unwrap(),
            "--resamples",
            "50",
        ]);
        std::fs::remove_file(&file).unwrap();
        let stdout = got.text(Stream::Stdout);
        assert!(stdout.contains("training=issuance"), "{stdout}");
        assert!(stdout.contains("== criteria on segment test"), "{stdout}");
        assert!(
            stdout.contains("issuance training: training targets left out"),
            "{stdout}"
        );
        let excluded_line = stdout
            .lines()
            .find(|l| l.contains(" B ") && l.contains("outside input"))
            .unwrap();
        let outside: usize = excluded_line.rsplit(' ').next().unwrap().parse().unwrap();
        assert_ne!(outside, 0, "{excluded_line}");
        assert!(!stdout.contains("MISSING"), "{stdout}");
        assert!(!stdout.contains("UNDETERMINED"), "{stdout}");
    }

    /// The time of one forecast recompute of `variant` on archive-sized
    /// input: hourly 245 and 165 issuances, as stored in production. Run it
    /// in release under `/usr/bin/time -l` for the peak memory; the
    /// procedure is in the PR of this test.
    fn measure(variant: ForecastVariant) {
        let data = build_every("2026-02-01T00:00:00Z", 40, 1);
        let t = ms("2026-03-10T13:00:00Z");
        let inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime).unwrap();
        let ladder: usize = inputs.forecast_ladder.values().map(Vec::len).sum();
        let mut times = Vec::new();
        for _ in 0..5 {
            let begin = std::time::Instant::now();
            let fc = forecast_at_issue_time_with(&inputs, variant).unwrap();
            times.push(begin.elapsed().as_secs_f64() * 1000.0);
            assert!(!fc.used_fallback);
        }
        println!(
            "training={:?} ladder_rows={ladder} recompute_ms={times:.1?}",
            variant.training
        );
    }

    #[test]
    #[ignore = "a measurement; run on demand in release"]
    fn measure_one_recompute_with_realised_training() {
        measure(ForecastVariant::default());
    }

    #[test]
    #[ignore = "a measurement; run on demand in release"]
    fn measure_one_recompute_with_issuance_training() {
        measure(issuance());
    }
}
