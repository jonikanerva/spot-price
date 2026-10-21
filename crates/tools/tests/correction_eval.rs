//! The quarter-correction arms of the evaluation report (`backtest --eval
//! --corrections`): the flags, the older inputs and the leak guard, the
//! paired deltas against the baseline arm, the selection and its alias, and
//! the declared criteria files.

#[cfg(test)]
mod correction_eval {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use jiff::Timestamp;
    use spot_price_domain::forecast::{CORRECTION_HISTORY_DAYS, Decomposition};
    use spot_price_tools::backtest::{
        AsOfMode, ForecastVariant, correction_examples_at, find_leaking_inputs,
        forecast_at_issue_time, forecast_at_issue_time_with, reconstruct_issue_time,
    };
    use spot_price_tools::backtest_cli::parse_variant;
    use spot_price_tools::cli::{Context, Output, Stream, run};
    use spot_price_tools::correction::{CORRECTION_PROFILE, CorrectionSpec, arm_name};
    use spot_price_tools::criteria::load_criteria;
    use spot_price_tools::data::{BacktestData, GridActual, PricePoint, Vintage, to_fixture_json};
    use spot_price_tools::eval_cli::{correction_arm, parse_corrections, parse_flags};

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
        let s = (int(q % 100_000) * 12.9898).sin() * 43758.5453;
        (s - s.floor() - 0.5) * 2.0
    }

    /// `days` days of prices and actuals from `start`. The FI price follows
    /// the grid, a weekday step, and noise. Vintages start `vintage_days`
    /// after `start`, so earlier issuances are pre-vintage but their prices
    /// are history for the correction.
    fn build(start: &str, days: i64, vintage_days: i64) -> BacktestData {
        let start = ms(start);
        let vintage_from = start + vintage_days * DAY_MS;
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
                if at - lead * HOUR_MS >= vintage_from {
                    wind245.push(vintage(wind + 3.0 * int(lead), 245, lead));
                }
            }
            for lead in [2, 30, 60] {
                if at - lead * HOUR_MS >= vintage_from {
                    cons165.push(vintage(consumption, 165, lead));
                }
            }
        }
        data.actuals = BTreeMap::from([(75, wind75), (124, cons124)]);
        data.vintages = BTreeMap::from([(245, wind245), (165, cons165)]);
        data
    }

    /// [`build`] with a vintage ladder like the archive: a 245 issuance every
    /// hour covers 6 h back and 72 h ahead, a 165 issuance 6 h back and 30 h
    /// ahead.
    fn build_archive(start: &str, days: i64) -> BacktestData {
        let mut data = build(start, days, days);
        let start = ms(start);
        let end = start + days * DAY_MS;
        let wind_at = |q: i64| 2000.0 + int((q * 131) % 1800);
        let consumption_at = |q: i64| 8000.0 + int(q % 96) * 15.0;
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
            issued += HOUR_MS;
        }
        data.vintages = BTreeMap::from([(245, wind245), (165, cons165)]);
        data
    }

    fn spec(token: &str) -> CorrectionSpec {
        CorrectionSpec::parse(token).unwrap()
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn the_served_variant_has_no_correction_and_the_ab_key_adds_one() {
        assert_eq!(ForecastVariant::default().correction, None);
        let variant = parse_variant("shape_profile=mean4w,correction=dev_l100_w14_h7").unwrap();
        assert_eq!(variant.correction, Some(spec("dev_l100_w14_h7")));
        assert_eq!(variant.shape_profile, CORRECTION_PROFILE);
        let joint = spec("joint_l1000_w30_h0_hol");
        assert_eq!(joint.decomposition, Decomposition::Joint);
        assert_eq!(
            (joint.ridge_lambda, joint.window_days, joint.half_life_days),
            (1000, 30, 0)
        );
        assert!(joint.holiday);
        assert!(parse_variant("correction=joint_l1_w99_h0").is_err());
    }

    #[test]
    fn the_correction_flags_add_named_arms_and_their_deltas() {
        let flags = parse_flags(&args(&[
            "--eval",
            "--profiles",
            "persistence",
            "--corrections",
            "joint_l100_w30_h0,dev_l10_w14_h7",
            "--correction-holiday",
        ]))
        .unwrap();
        let names: Vec<&str> = flags.options.arms.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "A",
                "shape_persistence",
                "pure_persistence",
                "corr_joint_l100_w30_h0",
                "corr_dev_l10_w14_h7",
                "corr_joint_l100_w30_h0_hol",
                "corr_dev_l10_w14_h7_hol",
            ]
        );
        assert_eq!(
            flags.options.arms[3],
            correction_arm(spec("joint_l100_w30_h0"))
        );
        // Arm A is the baseline: the served forecast has the correction
        // profile and no correction.
        assert_eq!(
            flags.options.arms[0].variant.shape_profile,
            CORRECTION_PROFILE
        );
        assert_eq!(flags.options.arms[0].variant.correction, None);
        assert_eq!(
            ForecastVariant {
                correction: None,
                ..flags.options.arms[3].variant
            },
            ForecastVariant::default()
        );
        let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(
            flags.options.paired,
            [
                pair("corr_joint_l100_w30_h0_hol", "corr_joint_l100_w30_h0"),
                pair("corr_dev_l10_w14_h7_hol", "corr_dev_l10_w14_h7"),
            ]
        );
        assert!(parse_flags(&args(&["--eval", "--corrections", "dev_l1_w30_h0"])).is_ok());
        let grid = parse_corrections("grid").unwrap();
        assert_eq!(grid.len(), 32);
        assert!(grid.iter().all(|s| !s.holiday));
    }

    #[test]
    fn correction_flag_errors_are_reported_before_any_data_is_read() {
        for (list, message) in [
            (
                &["--corrections", "joint_l1_w30_h0_hol"][..],
                "--correction-holiday adds the holiday twins",
            ),
            (
                &["--corrections", "dev_l1_w30_h0,dev_l1_w30_h0"][..],
                "--corrections repeats dev_l1_w30_h0",
            ),
            (
                &["--corrections", "ridge"][..],
                "--corrections a correction must be",
            ),
            (
                &["--correction-holiday"][..],
                "--correction-holiday needs --corrections",
            ),
        ] {
            let mut all = vec!["backtest", "--eval", "--db"];
            all.extend_from_slice(list);
            let got = run_cli(&all);
            assert_eq!(got.code, 1, "{list:?}");
            assert!(
                got.text(Stream::Stderr).contains(message),
                "{list:?}: {}",
                got.text(Stream::Stderr)
            );
        }
    }

    #[test]
    fn the_older_inputs_end_at_the_served_window_and_pass_the_leak_guard() {
        let data = build("2026-01-01T00:00:00Z", 80, 40);
        let t = ms("2026-03-15T13:00:00Z");
        let inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime).unwrap();
        let history_start = inputs.origin_ms - 30 * DAY_MS;
        let older_start = history_start - CORRECTION_HISTORY_DAYS * DAY_MS;
        let first = inputs
            .older_fi_prices
            .keys()
            .next()
            .unwrap()
            .as_millisecond();
        let last = inputs
            .older_fi_prices
            .keys()
            .next_back()
            .unwrap()
            .as_millisecond();
        assert_eq!(first, older_start);
        assert_eq!(last, history_start - QUARTER_MS);
        assert_eq!(inputs.older_neighbor_prices.len(), 3);
        assert!(inputs.older_actual.values().flatten().all(|r| {
            let at = r.start.as_millisecond();
            (older_start..history_start).contains(&at)
        }));
        assert_eq!(find_leaking_inputs(&inputs), Vec::new());

        let mut leaked = inputs.clone();
        leaked.older_actual.entry(75).or_default().push(GridActual {
            dataset_id: 75,
            start: ts(t),
            end: ts(t + QUARTER_MS),
            value: 1.0,
        });
        assert_eq!(find_leaking_inputs(&leaked).len(), 1);
    }

    #[test]
    fn the_correction_arm_reads_older_prices_and_leaves_arm_a_alone() {
        let data = build("2026-01-01T00:00:00Z", 80, 30);
        let t = ms("2026-03-15T13:00:00Z");
        let inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime).unwrap();
        let variant = ForecastVariant {
            shape_profile: CORRECTION_PROFILE,
            correction: Some(spec("joint_l100_w30_h0")),
            ..ForecastVariant::default()
        };
        let examples = correction_examples_at(&inputs, variant, false).unwrap();
        assert!(
            examples.examples.len() > 2000,
            "{}",
            examples.examples.len()
        );
        assert!(examples.without_baseline * 20 < examples.examples.len());
        let narrow = spot_price_tools::backtest::IssueTimeInputs {
            older_fi_prices: BTreeMap::new(),
            older_neighbor_prices: BTreeMap::new(),
            older_actual: BTreeMap::new(),
            ..inputs.clone()
        };
        let narrow_examples = correction_examples_at(&narrow, variant, false).unwrap();
        assert!(narrow_examples.without_baseline > narrow_examples.examples.len());

        let a = forecast_at_issue_time(&inputs).unwrap();
        assert_eq!(a, forecast_at_issue_time(&narrow).unwrap());
        assert_eq!(a.correction, None);
        let corrected = forecast_at_issue_time_with(&inputs, variant).unwrap();
        let diagnostics = corrected.correction.unwrap();
        assert!(!diagnostics.used_default);
        assert_eq!(diagnostics.fit_samples, examples.examples.len());
        let baseline = forecast_at_issue_time_with(
            &inputs,
            ForecastVariant {
                shape_profile: CORRECTION_PROFILE,
                ..ForecastVariant::default()
            },
        )
        .unwrap();
        assert_eq!(corrected.predicted.len(), baseline.predicted.len());
        assert_ne!(corrected.predicted, baseline.predicted);
    }

    fn criteria_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("criteria/quarter-correction")
    }

    #[test]
    fn the_committed_criteria_files_name_the_aliases() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(criteria_dir())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        let names: Vec<String> = files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["holiday.json", "selected.json"]);
        for file in &files {
            let declared = load_criteria(file).unwrap();
            let c = &declared.criteria;
            assert!(!c.success.is_empty() && !c.regression.is_empty());
            let reference = if file.ends_with("holiday.json") {
                "corr_selected_hol-corr_selected/"
            } else {
                "corr_selected-A/"
            };
            for criterion in c.success.iter().chain(&c.regression) {
                assert!(criterion.id.contains(reference), "{}", criterion.id);
            }
        }
    }

    /// One run with the committed criteria: every criterion resolves, and
    /// the alias repeats the statistics of the selected arm.
    #[test]
    fn the_paired_run_selects_an_arm_and_resolves_every_declared_criterion() {
        let file = temp_file("paired.json");
        std::fs::write(
            &file,
            to_fixture_json(&build("2026-01-01T00:00:00Z", 76, 45)),
        )
        .unwrap();
        let criteria = criteria_dir().join("selected.json");
        let holiday = criteria_dir().join("holiday.json");
        let run_with = |criteria: &PathBuf| {
            run_cli(&[
                "backtest",
                "--eval",
                "--data",
                file.to_str().unwrap(),
                "--corrections",
                "joint_l100_w30_h0,dev_l100_w14_h7",
                "--correction-holiday",
                "--split",
                "2026-03-02,2026-03-09",
                "--criteria",
                criteria.to_str().unwrap(),
                "--resamples",
                "50",
            ])
        };
        let got = run_with(&criteria);
        let stdout = got.text(Stream::Stdout);
        assert!(stdout.contains("QUARTER CORRECTION"), "{stdout}");
        assert!(stdout.contains("arm corr_dev_l100_w14_h7:"));
        assert!(stdout.contains("shape_profile=mean4w correction=dev_l100_w14_h7"));
        assert!(stdout.contains("quarter correction (summed over issuances"));
        let selection = stdout
            .lines()
            .find(|l| l.contains("correction selection on segment selection"))
            .unwrap();
        let selected = ["corr_joint_l100_w30_h0", "corr_dev_l100_w14_h7"]
            .into_iter()
            .find(|name| selection.contains(&format!(": {name} as corr_selected")))
            .unwrap();
        assert!(selection.contains(&format!(
            "holiday twin: {selected}_hol as corr_selected_hol"
        )));
        assert!(stdout.contains("== criteria on segment test"));
        assert!(!stdout.contains("MISSING"), "{stdout}");
        // The alias row carries the numbers of the selected arm.
        let row = |id: &str| {
            stdout
                .lines()
                .rev()
                .find(|l| l.trim_start().starts_with(&format!("{id} ")))
                .map(|l| l.split_whitespace().skip(1).collect::<Vec<_>>())
                .unwrap()
        };
        for cell in ["13/D+2", "06/D+1"] {
            assert_eq!(
                row(&format!("mae_delta/corr_selected-A/{cell}")),
                row(&format!("mae_delta/{selected}-A/{cell}"))
            );
        }
        std::fs::remove_file(&file).unwrap();
        // The holiday file names statistics of the same test segment.
        let test_ids: Vec<&str> = stdout
            .split("== segment: test")
            .nth(1)
            .unwrap()
            .lines()
            .filter_map(|l| l.split_whitespace().next())
            .collect();
        let declared = load_criteria(&holiday).unwrap();
        for criterion in declared
            .criteria
            .success
            .iter()
            .chain(&declared.criteria.regression)
        {
            assert!(
                test_ids.contains(&criterion.id.as_str()),
                "{}",
                criterion.id
            );
        }
        assert_eq!(arm_name(spec("dev_l1_w2_h3")), "corr_dev_l1_w2_h3");
    }

    // CLI helpers

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "spot-price-tools-correction-{}-{name}",
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

    /// The time of one forecast recompute with a correction on
    /// archive-sized input, examples included. Run it in release under
    /// `/usr/bin/time -l` for the peak memory; the procedure is in the PR.
    fn measure(variant: ForecastVariant) {
        let data = build_archive("2026-01-01T00:00:00Z", 80);
        let t = ms("2026-03-15T13:00:00Z");
        let inputs = reconstruct_issue_time(&data, t, AsOfMode::IssueTime).unwrap();
        let mut times = Vec::new();
        for _ in 0..5 {
            let begin = std::time::Instant::now();
            let fc = forecast_at_issue_time_with(&inputs, variant).unwrap();
            times.push(begin.elapsed().as_secs_f64() * 1000.0);
            assert!(!fc.used_fallback);
        }
        println!(
            "correction={:?} recompute_ms={times:.1?}",
            variant.correction.map(CorrectionSpec::token)
        );
    }

    /// The wall time of one evaluation with the whole grid and its holiday
    /// twins on the fixture of the paired-run test.
    #[test]
    #[ignore = "a measurement; run on demand in release"]
    fn measure_a_grid_evaluation() {
        let file = temp_file("grid.json");
        std::fs::write(
            &file,
            to_fixture_json(&build("2026-01-01T00:00:00Z", 76, 45)),
        )
        .unwrap();
        let begin = std::time::Instant::now();
        let got = run_cli(&[
            "backtest",
            "--eval",
            "--data",
            file.to_str().unwrap(),
            "--corrections",
            "grid",
            "--correction-holiday",
            "--split",
            "2026-03-02,2026-03-09",
            "--resamples",
            "50",
        ]);
        let seconds = begin.elapsed().as_secs_f64();
        std::fs::remove_file(&file).unwrap();
        let stdout = got.text(Stream::Stdout);
        let forecasts = stdout.matches("correction selection on segment").count();
        println!("grid evaluation: {seconds:.1} s, selection lines {forecasts}");
    }

    #[test]
    #[ignore = "a measurement; run on demand in release"]
    fn measure_one_recompute_with_the_baseline() {
        measure(ForecastVariant {
            shape_profile: CORRECTION_PROFILE,
            ..ForecastVariant::default()
        });
    }

    #[test]
    #[ignore = "a measurement; run on demand in release"]
    fn measure_one_recompute_with_a_joint_correction() {
        measure(ForecastVariant {
            shape_profile: CORRECTION_PROFILE,
            correction: Some(spec("joint_l1000_w30_h7_hol")),
            ..ForecastVariant::default()
        });
    }

    #[test]
    #[ignore = "a measurement; run on demand in release"]
    fn measure_one_recompute_with_a_deviation_correction() {
        measure(ForecastVariant {
            shape_profile: CORRECTION_PROFILE,
            correction: Some(spec("dev_l1000_w30_h7")),
            ..ForecastVariant::default()
        });
    }
}
