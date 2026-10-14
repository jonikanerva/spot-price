//! Behaviour of the forecast-revision study and its CLI layer.

#[cfg(test)]
mod revision {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use jiff::Timestamp;
    use spot_price_tools::data::{DatasetId, Vintage};
    use spot_price_tools::metrics::{rms, standard_deviation};
    use spot_price_tools::revision::{
        ActualRecord, DatasetRevisionSummary, GO_ATTENUATION, MARGINAL_ATTENUATION,
        MIN_BAND_SAMPLES, MIN_SAMPLES_PER_BIN, REFERENCE_MAX_LEAD_H, RevisionStudyInput, Verdict,
        recommendation, run_revision_study,
    };
    use spot_price_tools::revision_cli::{load_vintage_fixture, to_fixture_json};

    const HOUR_MS: i64 = 60 * 60 * 1000;
    const WIND: DatasetId = 245;
    const CONSUMPTION: DatasetId = 165;

    fn ms(text: &str) -> i64 {
        text.parse::<Timestamp>().unwrap().as_millisecond()
    }

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_millisecond(ms).unwrap()
    }

    fn base() -> i64 {
        ms("2026-06-25T00:00:00Z")
    }

    /// A vintage issued `lead_h` hours before delivery (negative: after it).
    fn vintage(dataset_id: DatasetId, target_ms: i64, lead_h: i64, value: f64) -> Vintage {
        Vintage {
            dataset_id,
            issued_at: ts(target_ms - lead_h * HOUR_MS),
            start: ts(target_ms),
            end: ts(target_ms + 15 * 60 * 1000),
            value,
        }
    }

    fn input(dataset_id: DatasetId, vintages: Vec<Vintage>) -> RevisionStudyInput {
        RevisionStudyInput {
            vintages_by_dataset: BTreeMap::from([(dataset_id, vintages)]),
            actuals_by_dataset: BTreeMap::new(),
        }
    }

    fn dataset(input: &RevisionStudyInput, id: DatasetId) -> DatasetRevisionSummary {
        run_revision_study(input)
            .datasets
            .into_iter()
            .find(|d| d.dataset_id == id)
            .unwrap()
    }

    fn close(got: Option<f64>, want: f64) {
        let got = got.unwrap();
        assert!((got - want).abs() < 1e-6, "got {got}, want {want}");
    }

    /// `n` delivered targets with a post-delivery reference
    /// `ref_base + t * ref_step` and one issuance per lead at
    /// `reference + revision`.
    #[expect(clippy::cast_precision_loss, reason = "t is a small index")]
    fn gen_dataset(
        dataset_id: DatasetId,
        n: i64,
        ref_base: f64,
        ref_step: f64,
        revision: f64,
    ) -> Vec<Vintage> {
        let mut out = Vec::new();
        for t in 0..n {
            let target = base() + t * 6 * HOUR_MS;
            let reference = ref_base + t as f64 * ref_step;
            out.push(vintage(dataset_id, target, -1, reference));
            for lead in [6, 12, 24, 36] {
                out.push(vintage(dataset_id, target, lead, reference + revision));
            }
        }
        out
    }

    // Reference selection, bucketing, admissibility

    #[test]
    fn uses_the_freshest_issuance_and_buckets_positive_leads() {
        let t = ms("2026-07-01T12:00:00Z");
        let vintages = [
            (-1, 500.0),
            (3, 510.0),
            (9, 520.0),
            (15, 540.0),
            (21, 570.0),
        ]
        .into_iter()
        .chain([(30, 600.0), (42, 650.0), (60, 700.0)])
        .map(|(lead, value)| vintage(WIND, t, lead, value))
        .collect();
        let d = dataset(&input(WIND, vintages), WIND);
        assert_eq!(d.admissible_targets, 1);
        assert_eq!(d.references_post_delivery, 1);
        assert_eq!(d.references_pre_delivery_within_tol, 0);
        assert_eq!(d.future_targets, 0);
        close(d.empirical_max_lead_h, 60.0);
        for b in &d.buckets {
            assert_eq!(b.samples, 1, "{}", b.label);
            assert_eq!(b.targets, 1, "{}", b.label);
            assert!(!b.sufficient);
            assert_eq!(b.rms_revision, None);
        }
    }

    #[test]
    fn a_reference_within_tolerance_before_delivery_counts() {
        let t = ms("2026-07-02T00:00:00Z");
        let anchor = t + 3 * HOUR_MS;
        let vintages = vec![
            vintage(WIND, t, 1, 1000.0),
            vintage(WIND, t, 8, 1100.0),
            vintage(WIND, anchor, -1, 900.0),
        ];
        let d = dataset(&input(WIND, vintages), WIND);
        assert_eq!(d.references_pre_delivery_within_tol, 1);
        assert_eq!(d.references_post_delivery, 1);
        assert_eq!(d.admissible_targets, 2);
        assert_eq!(d.buckets.iter().map(|b| b.samples).sum::<usize>(), 1);
        assert_eq!(REFERENCE_MAX_LEAD_H, 2);
    }

    #[test]
    fn skips_future_targets_and_excludes_stale_references() {
        let future = base() + 200 * HOUR_MS;
        let stale = base() + 50 * HOUR_MS;
        let anchor = base() + 60 * HOUR_MS;
        let vintages = vec![
            vintage(WIND, base(), -1, 300.0),
            vintage(WIND, stale, 6, 700.0),
            vintage(WIND, stale, 12, 740.0),
            vintage(WIND, anchor, -1, 800.0),
            vintage(WIND, future, 6, 400.0),
            vintage(WIND, future, 12, 420.0),
        ];
        let d = dataset(&input(WIND, vintages), WIND);
        assert_eq!(d.admissible_targets, 2);
        assert_eq!(d.excluded_targets, 1);
        assert_eq!(d.future_targets, 1);
    }

    // Bin statistics

    #[test]
    #[expect(clippy::cast_precision_loss, reason = "t is a small index")]
    fn computes_the_statistics_of_a_known_bin() {
        let n = 120;
        let mut vintages = Vec::new();
        let mut refs = Vec::new();
        for t in 0..n {
            let target = base() + t * 6 * HOUR_MS;
            let reference = 1000.0 + t as f64 * 10.0;
            refs.push(reference);
            vintages.push(vintage(WIND, target, -1, reference));
            let delta = if t % 2 == 0 { 100.0 } else { -100.0 };
            vintages.push(vintage(WIND, target, 12, reference + delta));
        }
        let d = dataset(&input(WIND, vintages), WIND);
        let bin = d.buckets.iter().find(|b| b.label == "6-12h").unwrap();
        assert_eq!(bin.samples, 120);
        assert!(bin.sufficient);
        close(bin.mean_abs_revision, 100.0);
        close(bin.median_abs_revision, 100.0);
        close(bin.p90_abs_revision, 100.0);
        close(bin.rms_revision, 100.0);
        close(bin.signed_bias_revision, 0.0);
        let sd = standard_deviation(&refs).unwrap();
        close(d.sd_reference, sd);
        let nsr = 100.0 / sd;
        close(bin.noise_to_signal, nsr);
        close(bin.attenuation, 1.0 / (1.0 + nsr * nsr));
        let mean_ref = refs.iter().sum::<f64>() / refs.len() as f64;
        close(bin.rel_mean_abs, 100.0 / mean_ref);
    }

    #[test]
    fn a_thin_bin_is_insufficient_and_ignored() {
        let n = i64::try_from(MIN_SAMPLES_PER_BIN).unwrap() - 1;
        let mut vintages = Vec::new();
        for t in 0..n {
            let target = base() + t * 6 * HOUR_MS;
            vintages.push(vintage(WIND, target, -1, 1000.0));
            vintages.push(vintage(WIND, target, 12, 1100.0));
        }
        let d = dataset(&input(WIND, vintages), WIND);
        let bin = d.buckets.iter().find(|b| b.label == "6-12h").unwrap();
        assert_eq!(bin.samples, MIN_SAMPLES_PER_BIN - 1);
        assert!(!bin.sufficient);
        assert_eq!(bin.mean_abs_revision, None);
        assert_eq!(bin.rms_revision, None);
        assert_eq!(d.production_band_samples, 0);
    }

    // Aggregate NSR and attenuation

    #[test]
    #[expect(clippy::cast_precision_loss, reason = "t is a small index")]
    fn aggregates_rms_over_sd_on_the_sufficient_bins() {
        let n = 150;
        let d = dataset(
            &input(WIND, gen_dataset(WIND, n, 1000.0, 20.0, 400.0)),
            WIND,
        );
        let refs: Vec<f64> = (0..n).map(|t| 1000.0 + t as f64 * 20.0).collect();
        let sd = standard_deviation(&refs).unwrap();
        close(d.sd_reference, sd);
        close(d.production_band_rms, 400.0);
        close(rms(&vec![400.0; 600]), 400.0);
        let nsr = 400.0 / sd;
        close(d.production_band_nsr, nsr);
        close(d.attenuation_illustration, 1.0 / (1.0 + nsr * nsr));
        assert_eq!(d.production_band_samples, 600);
    }

    #[test]
    fn checks_the_reference_against_overlapping_actuals() {
        let mut study = input(
            WIND,
            vec![
                vintage(WIND, base(), -1, 500.0),
                vintage(WIND, base(), 12, 560.0),
            ],
        );
        study.actuals_by_dataset.insert(
            75,
            vec![ActualRecord {
                dataset_id: 75,
                start: ts(base()),
                value: 490.0,
            }],
        );
        let check = dataset(&study, WIND).actual_check.unwrap();
        assert_eq!(check.targets_compared, 1);
        close(check.median_abs_ref_minus_actual, 10.0);
    }

    // Recommendation

    #[test]
    fn go_when_the_attenuation_is_at_or_below_the_threshold() {
        let study = input(WIND, gen_dataset(WIND, 150, 1000.0, 20.0, 2000.0));
        assert_eq!(
            recommendation(&run_revision_study(&study)).verdict,
            Verdict::Go
        );
        let d = dataset(&study, WIND);
        assert!(d.attenuation_illustration.unwrap() <= GO_ATTENUATION);
        assert!(d.production_band_samples >= MIN_BAND_SAMPLES);
    }

    #[test]
    fn marginal_and_provisional_at_or_above_095() {
        let study = input(
            CONSUMPTION,
            gen_dataset(CONSUMPTION, 150, 8000.0, 50.0, 1.0),
        );
        let result = run_revision_study(&study);
        let rec = recommendation(&result);
        let d = dataset(&study, CONSUMPTION);
        assert!(d.attenuation_illustration.unwrap() >= MARGINAL_ATTENUATION);
        assert_eq!(rec.verdict, Verdict::Marginal);
        assert!(rec.reason.contains("PROVISIONAL"));
        assert!(rec.reason.contains("#80"));
    }

    #[test]
    fn marginal_and_modest_between_090_and_095() {
        let study = input(WIND, gen_dataset(WIND, 150, 1000.0, 20.0, 250.0));
        let rec = recommendation(&run_revision_study(&study));
        let att = dataset(&study, WIND).attenuation_illustration.unwrap();
        assert!(att > GO_ATTENUATION && att < MARGINAL_ATTENUATION);
        assert_eq!(rec.verdict, Verdict::Marginal);
        assert!(rec.reason.contains("#80"));
    }

    #[test]
    fn defer_when_every_bin_is_thin() {
        let study = input(WIND, gen_dataset(WIND, 50, 1000.0, 20.0, 2000.0));
        assert_eq!(
            recommendation(&run_revision_study(&study)).verdict,
            Verdict::Defer
        );
    }

    #[test]
    fn never_recommends_closing_the_training_question() {
        for revision in [1.0, 250.0, 2000.0] {
            let study = input(WIND, gen_dataset(WIND, 150, 1000.0, 20.0, revision));
            let rec = recommendation(&run_revision_study(&study));
            assert!(matches!(
                rec.verdict,
                Verdict::Go | Verdict::Marginal | Verdict::Defer
            ));
            assert!(!rec.reason.to_lowercase().contains("close #81 as"));
        }
    }

    // Empty input

    #[test]
    fn an_empty_input_gives_no_window_and_empty_summaries() {
        let result = run_revision_study(&RevisionStudyInput::default());
        assert_eq!(result.window, None);
        assert_eq!(result.datasets.len(), 2);
        for d in &result.datasets {
            assert_eq!(d.admissible_targets, 0);
            assert_eq!(d.production_band_samples, 0);
        }
        assert_eq!(recommendation(&result).verdict, Verdict::Defer);
    }

    // CLI fixture round trip

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spot-price-tools-rev-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[expect(clippy::cast_precision_loss, reason = "t is a small index")]
    fn store_shaped() -> RevisionStudyInput {
        let mut study = RevisionStudyInput::default();
        let (mut wind, mut cons, mut wind_actual, mut cons_actual) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for t in 0..40_i64 {
            let target = base() + t * 6 * HOUR_MS;
            let w = 2000.0 + ((t * 131) % 1500) as f64;
            let c = 8000.0 + (t % 24) as f64 * 120.0;
            wind.push(vintage(WIND, target, -1, w));
            wind.push(vintage(WIND, target, 12, w + 200.0));
            wind.push(vintage(WIND, target, 24, w + 350.0));
            cons.push(vintage(CONSUMPTION, target, -1, c));
            cons.push(vintage(CONSUMPTION, target, 12, c + 40.0));
            wind_actual.push(ActualRecord {
                dataset_id: 75,
                start: ts(target),
                value: w - 5.0,
            });
            cons_actual.push(ActualRecord {
                dataset_id: 124,
                start: ts(target),
                value: c + 3.0,
            });
        }
        study.vintages_by_dataset = BTreeMap::from([(WIND, wind), (CONSUMPTION, cons)]);
        study.actuals_by_dataset = BTreeMap::from([(75, wind_actual), (124, cons_actual)]);
        study
    }

    #[test]
    fn the_export_loads_back_unchanged() {
        let study = store_shaped();
        let file = temp_file("vintages.json");
        std::fs::write(&file, to_fixture_json(&study)).unwrap();
        assert_eq!(load_vintage_fixture(&file).unwrap(), study);
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn the_study_of_the_export_equals_the_study_of_the_input() {
        let study = store_shaped();
        let file = temp_file("snapshot.json");
        std::fs::write(&file, to_fixture_json(&study)).unwrap();
        let loaded = load_vintage_fixture(&file).unwrap();
        assert_eq!(run_revision_study(&loaded), run_revision_study(&study));
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn a_fixture_without_actuals_loads_and_runs() {
        let mut study = store_shaped();
        study.actuals_by_dataset.clear();
        let file = temp_file("no-actuals.json");
        std::fs::write(&file, to_fixture_json(&study)).unwrap();
        let loaded = load_vintage_fixture(&file).unwrap();
        assert_eq!(loaded, study);
        let d = dataset(&loaded, WIND);
        assert_eq!(d.actual_check, None);
        assert!(d.admissible_targets > 0);
        std::fs::remove_file(file).unwrap();
    }
}
