//! Parity of the Rust forecast with the TypeScript forecast. The fixtures in
//! `tests/golden/forecast/` come from the TypeScript modules
//! (`tests/golden/README.md`). Floats before rounding may differ by a few ULPs
//! (V8 and Rust use different `asinh`, `sinh`, `sin`, and `cos`); values
//! after the 0.001 rounding must be equal.

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "a golden value must be equal bit for bit unless a tolerance is counted"
)]
mod golden {
    use std::borrow::Cow;
    use std::cell::Cell;
    use std::collections::BTreeMap;

    use jiff::Timestamp;
    use jiff::tz::TimeZone;
    use serde::Deserialize;
    use spot_price_domain::areas::Area;
    use spot_price_domain::forecast::{
        BandMethod, BandOffsets, CalibratedBands, DELIVERY_DAY_TIME_ZONE, DeriveOptions,
        FeatureContext, FitOptions, ForecastDiagnostics, ForecastInput, ForecastOptions,
        ForecastWindow, GridGapRule, GridRecord, HorizonResidual, LatestPublishedRule,
        NeighborPrices, PriceTransform, QuarterMap, QuarterStart, RouteForecast, SanityBound,
        ShapeProfile, TrainingData, UtcHour, align_series, apply_band, apply_within_day_shape,
        bucket_records, build_artifact, build_feature_vector_indexed, build_forecast,
        build_predicted_series, build_price_index_with, build_training_matrix, derive_band_offsets,
        expand_hourly_to_quarters, extend_with_last_week, fit_linear, fit_ridge,
        forecast_for_route, iso_millis, observed_coverage_of, sanity_bound_from_history,
        to_forecast_entries,
    };
    use spot_price_domain::price::apply_contract_terms;
    use spot_price_domain::settings::{Hour, UserId, UserSettings};
    use spot_price_domain::time::UserTimeZone;

    // ---------------------------------------------------------------------------
    // Comparison with counted tolerance
    // ---------------------------------------------------------------------------

    /// Counts of compared values. `tolerant` values are not bit-equal but within
    /// 1e-9 relative (1e-9 absolute near zero). `boundary` values are rounded
    /// values that differ only because the value before rounding lies within
    /// 1e-9 of a rounding boundary.
    #[derive(Default)]
    struct Stats {
        compared: Cell<usize>,
        tolerant: Cell<usize>,
        rounded_count: Cell<usize>,
        boundary: Cell<usize>,
        edge: Cell<usize>,
    }

    impl Stats {
        fn close(&self, got: f64, want: f64, what: &str) {
            self.compared.set(self.compared.get() + 1);
            if got == want {
                return;
            }
            let diff = (got - want).abs();
            let scale = got.abs().max(want.abs()).max(1.0);
            assert!(diff <= 1e-9 * scale, "{what}: got {got}, want {want}");
            self.tolerant.set(self.tolerant.get() + 1);
        }

        fn all_close(&self, got: &[f64], want: &[f64], what: &str) {
            assert_eq!(got.len(), want.len(), "{what}: length");
            for (i, (g, w)) in got.iter().zip(want).enumerate() {
                self.close(*g, *w, &format!("{what}[{i}]"));
            }
        }

        /// A value after `Math.round(x * 1000) / 1000`. `pre` is the Rust value
        /// before rounding; NaN when it is not available, which requires equality.
        fn rounded(&self, got: f64, want: f64, pre: f64, what: &str) {
            self.rounded_count.set(self.rounded_count.get() + 1);
            if got == want {
                return;
            }
            let scaled = pre * 1000.0;
            let boundary = (scaled.floor() + 0.5) / 1000.0;
            assert!(
                (pre - boundary).abs() <= 1e-9 && (got - want).abs() <= 0.001 + 1e-12,
                "{what}: got {got}, want {want}, before rounding {pre}"
            );
            self.boundary.set(self.boundary.get() + 1);
        }

        fn report(&self, name: &str) {
            println!(
                "{name}: {} floats compared ({} within tolerance, not bit-equal; {} coverage values differ only on band-edge samples); {} rounded values compared ({} at a rounding boundary)",
                self.compared.get(),
                self.tolerant.get(),
                self.edge.get(),
                self.rounded_count.get(),
                self.boundary.get()
            );
        }
    }

    /// Compare an observed coverage with the TypeScript value. The offset of a
    /// bucket is one of its own residuals, so that residual lies on its band
    /// edge, where a 1-ULP difference in `asinh` or `sinh` decides "covered".
    /// The covered counts may differ by at most the number of such edge
    /// residuals (within 1e-9 of a bound).
    fn check_coverage(
        got: f64,
        want: f64,
        residuals: &[HorizonResidual],
        offsets_by_hour: &[Option<BandOffsets>; 24],
        global_offsets: Option<BandOffsets>,
        stats: &Stats,
        what: &str,
    ) {
        let mut scored = 0_u32;
        let mut edge = 0_u32;
        for r in residuals {
            let Some(off) = offsets_by_hour[usize::from(r.utc_hour.get())].or(global_offsets)
            else {
                continue;
            };
            scored += 1;
            let point = r.predicted_raw.asinh();
            let near =
                |bound: f64| (r.actual_raw - bound).abs() <= 1e-9 * r.actual_raw.abs().max(1.0);
            if near((point + off.low_offset).sinh()) || near((point + off.high_offset).sinh()) {
                edge += 1;
            }
        }
        stats.compared.set(stats.compared.get() + 1);
        if got == want {
            return;
        }
        let difference = ((got - want) * f64::from(scored)).abs().round();
        assert!(
            difference <= f64::from(edge),
            "{what}: coverage {got} vs {want}: {difference} samples differ, {edge} edge samples"
        );
        stats.edge.set(stats.edge.get() + 1);
        println!(
            "{what}: coverage {got} vs TypeScript {want}: {difference} of {scored} samples differ; {edge} samples lie on a band edge"
        );
    }

    fn read(name: &str) -> String {
        let path = format!(
            "{}/tests/golden/forecast/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_millisecond(ms).unwrap()
    }

    fn q(ms: i64) -> QuarterStart {
        QuarterStart::from_millisecond(ms)
    }

    // ---------------------------------------------------------------------------
    // Fixture types
    // ---------------------------------------------------------------------------

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Run {
        start_ms: i64,
        step_ms: i64,
        values: Vec<f64>,
        gaps: Vec<usize>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Columns {
        start_ms: Vec<i64>,
        value: Vec<f64>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Dataset {
        fi: Run,
        se1: Run,
        se3: Run,
        ee: Run,
        wind_actual: Columns,
        cons_actual: Columns,
        wind_forecast: Columns,
        cons_forecast: Columns,
    }

    type Range = Option<[i64; 2]>;

    fn in_range(ms: i64, range: [i64; 2]) -> bool {
        ms >= range[0] && ms < range[1]
    }

    fn run_map(run: &Run, range: Range) -> QuarterMap {
        let Some(range) = range else {
            return QuarterMap::new();
        };
        run.values
            .iter()
            .enumerate()
            .filter(|(i, _)| !run.gaps.contains(i))
            .map(|(i, &v)| (run.start_ms + i64::try_from(i).unwrap() * run.step_ms, v))
            .filter(|&(ms, _)| in_range(ms, range))
            .map(|(ms, v)| (q(ms), v))
            .collect()
    }

    fn records(cols: &Columns, range: Range) -> Vec<GridRecord> {
        let Some(range) = range else {
            return Vec::new();
        };
        cols.start_ms
            .iter()
            .zip(&cols.value)
            .filter(|&(&ms, _)| in_range(ms, range))
            .map(|(&ms, &value)| GridRecord {
                start: ts(ms),
                value,
            })
            .collect()
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct KeyedValues {
        key_ms: Vec<i64>,
        value: Vec<f64>,
    }

    impl KeyedValues {
        fn to_map(&self) -> QuarterMap {
            self.key_ms
                .iter()
                .zip(&self.value)
                .map(|(&ms, &v)| (q(ms), v))
                .collect()
        }

        fn check(&self, got: &QuarterMap, stats: &Stats, what: &str) {
            let keys: Vec<i64> = got.keys().map(|k| k.as_millisecond()).collect();
            assert_eq!(keys, self.key_ms, "{what}: keys");
            let values: Vec<f64> = got.values().copied().collect();
            stats.all_close(&values, &self.value, what);
        }
    }

    #[derive(Deserialize, Clone, Copy)]
    #[serde(rename_all = "camelCase")]
    struct Offsets {
        low_offset: f64,
        high_offset: f64,
    }

    impl From<Offsets> for BandOffsets {
        fn from(o: Offsets) -> Self {
            Self {
                low_offset: o.low_offset,
                high_offset: o.high_offset,
            }
        }
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ArtifactJson {
        observed_coverage: Option<f64>,
        calibrated: bool,
        offsets_by_hour: Vec<Option<Offsets>>,
        global_offsets: Option<Offsets>,
        generated_at: String,
    }

    impl ArtifactJson {
        fn to_bands(&self) -> CalibratedBands {
            let mut offsets_by_hour = [None; 24];
            for (slot, offsets) in offsets_by_hour.iter_mut().zip(&self.offsets_by_hour) {
                *slot = offsets.map(BandOffsets::from);
            }
            CalibratedBands {
                method: BandMethod::EmpiricalResidual,
                nominal_coverage: 0.8,
                observed_coverage: self.observed_coverage,
                calibrated: self.calibrated,
                offsets_by_hour,
                global_offsets: self.global_offsets.map(BandOffsets::from),
                generated_at: Cow::Owned(self.generated_at.clone()),
            }
        }

        fn check(
            &self,
            got: &CalibratedBands,
            residuals: &[HorizonResidual],
            stats: &Stats,
            what: &str,
        ) {
            assert_eq!(got.calibrated, self.calibrated, "{what}: calibrated");
            assert_eq!(got.generated_at, self.generated_at, "{what}: generatedAt");
            match (got.observed_coverage, self.observed_coverage) {
                (Some(g), Some(w)) => {
                    let derived = derive_band_offsets(residuals, &DeriveOptions::default());
                    check_coverage(
                        g,
                        w,
                        residuals,
                        &derived.offsets_by_hour,
                        derived.global_offsets,
                        stats,
                        what,
                    );
                }
                (g, w) => assert_eq!(g, w, "{what}: observedCoverage"),
            }
            check_offsets(&got.offsets_by_hour, &self.offsets_by_hour, stats, what);
            check_offset(got.global_offsets, self.global_offsets, stats, what);
        }
    }

    fn check_offset(got: Option<BandOffsets>, want: Option<Offsets>, stats: &Stats, what: &str) {
        match (got, want) {
            (Some(g), Some(w)) => {
                stats.close(g.low_offset, w.low_offset, what);
                stats.close(g.high_offset, w.high_offset, what);
            }
            (None, None) => {}
            (g, w) => panic!("{what}: got {g:?}, want present={}", w.is_some()),
        }
    }

    fn check_offsets(
        got: &[Option<BandOffsets>; 24],
        want: &[Option<Offsets>],
        stats: &Stats,
        what: &str,
    ) {
        assert_eq!(want.len(), 24);
        for (hour, (g, w)) in got.iter().zip(want).enumerate() {
            check_offset(*g, *w, stats, &format!("{what} hour {hour}"));
        }
    }

    #[derive(Deserialize, Clone, Copy)]
    struct Bound {
        min: f64,
        max: f64,
    }

    // ---------------------------------------------------------------------------
    // End-to-end scenarios
    // ---------------------------------------------------------------------------

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Ranges {
        fi: Range,
        se1: Range,
        se3: Range,
        ee: Range,
        wind_actual: Range,
        cons_actual: Range,
        wind_forecast: Range,
        cons_forecast: Range,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ScenarioOptions {
        sanity_bound: Option<Bound>,
        ridge_lambda: Option<f64>,
        wind_extension_weeks: Option<u32>,
        consumption_extension_weeks: Option<u32>,
        stub_bands: Option<bool>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Training {
        count: usize,
        sample_indices: Vec<usize>,
        sample_keys_ms: Vec<i64>,
        sample_rows: Vec<Vec<f64>>,
        sample_targets: Vec<f64>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SeriesColumns {
        start_ms: Vec<i64>,
        estimate: Vec<f64>,
        banded_count: usize,
        low: Vec<f64>,
        high: Vec<f64>,
    }

    #[derive(Deserialize, PartialEq, Eq, Debug)]
    #[serde(rename_all = "camelCase")]
    struct Diagnostics {
        fit_samples: usize,
        feature_count: usize,
        fit_used_default: bool,
        consumption_extended_quarters: usize,
        wind_extended_quarters: usize,
        filled_quarters: usize,
        zero_seeded_quarters: usize,
        shape_fallback_quarters: usize,
        sanity_clamped_quarters: usize,
        band_calibrated: bool,
        band_hour_buckets: usize,
    }

    impl From<ForecastDiagnostics> for Diagnostics {
        fn from(d: ForecastDiagnostics) -> Self {
            Self {
                fit_samples: d.fit_samples,
                feature_count: d.feature_count,
                fit_used_default: d.fit_used_default,
                consumption_extended_quarters: d.consumption_extended_quarters,
                wind_extended_quarters: d.wind_extended_quarters,
                filled_quarters: d.filled_quarters,
                zero_seeded_quarters: d.zero_seeded_quarters,
                shape_fallback_quarters: d.shape_fallback_quarters,
                sanity_clamped_quarters: d.sanity_clamped_quarters,
                band_calibrated: d.band_calibrated,
                band_hour_buckets: d.band_hour_buckets,
            }
        }
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct AssessmentJson {
        degraded: bool,
        reason: Option<String>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Settings {
        margin_cents_kwh: f64,
        transfer_day_cents_kwh: f64,
        transfer_night_cents_kwh: f64,
        tax_cents_kwh: f64,
        vat_percent: f64,
        night_start_hour: i8,
        night_end_hour: i8,
    }

    /// The settings of a case. `apply_contract_terms` reads only the money
    /// fields and the night hours, so the zone and the area are placeholders.
    fn user_settings(s: &Settings) -> UserSettings {
        UserSettings {
            user_id: UserId::new("golden".to_owned()),
            margin_cents_kwh: s.margin_cents_kwh,
            transfer_day_cents_kwh: s.transfer_day_cents_kwh,
            transfer_night_cents_kwh: s.transfer_night_cents_kwh,
            tax_cents_kwh: s.tax_cents_kwh,
            vat_percent: s.vat_percent,
            night_start_hour: Hour::try_from(i32::from(s.night_start_hour)).unwrap(),
            night_end_hour: Hour::try_from(i32::from(s.night_end_hour)).unwrap(),
            timezone: UserTimeZone::for_area(Area::Fi).unwrap(),
            area: Area::Fi,
        }
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct EntriesCase {
        name: String,
        time_zone: String,
        settings: Settings,
        start: Vec<String>,
        end: Vec<String>,
        local_start: Vec<String>,
        local_end: Vec<String>,
        spot: Vec<f64>,
        total: Vec<f64>,
        spot_low: Vec<f64>,
        spot_high: Vec<f64>,
        total_low: Vec<f64>,
        total_high: Vec<f64>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Expected {
        grid_record_count: usize,
        route_sanity_bound: Option<Bound>,
        training: Training,
        neutral: f64,
        raw_predictions: Vec<f64>,
        shaped: Vec<f64>,
        clamped: Vec<f64>,
        series: SeriesColumns,
        diagnostics: Diagnostics,
        assessment: AssessmentJson,
        entries: Vec<EntriesCase>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Scenario {
        name: String,
        series_start_ms: i64,
        series_end_ms: i64,
        ranges: Ranges,
        options: ScenarioOptions,
        route: bool,
        expected: Expected,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ConformalFixture {
        residuals: ResidualColumns,
        derived: DerivedJson,
        derived_wide: DerivedJson,
        below_gate_residuals: ResidualColumns,
        artifacts: Artifacts,
        stub_bands: ArtifactJson,
        apply_points: Vec<f64>,
        applied: Vec<AppliedHour>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ResidualColumns {
        utc_hour: Vec<u8>,
        predicted_raw: Vec<f64>,
        actual_raw: Vec<f64>,
    }

    impl ResidualColumns {
        fn residuals(&self) -> Vec<HorizonResidual> {
            self.utc_hour
                .iter()
                .zip(&self.predicted_raw)
                .zip(&self.actual_raw)
                .map(|((&hour, &predicted_raw), &actual_raw)| HorizonResidual {
                    utc_hour: UtcHour::new(hour).unwrap(),
                    predicted_raw,
                    actual_raw,
                })
                .collect()
        }
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct DerivedJson {
        offsets_by_hour: Vec<Option<Offsets>>,
        global_offsets: Option<Offsets>,
        coverage: f64,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Artifacts {
        too_few: ArtifactJson,
        calibrated: ArtifactJson,
        below_gate: ArtifactJson,
    }

    #[derive(Deserialize, Clone, Copy)]
    struct BandJson {
        low: f64,
        high: f64,
    }

    #[derive(Deserialize)]
    struct AppliedHour {
        hour: u8,
        bands: Vec<Option<BandJson>>,
    }

    fn stub_bands() -> CalibratedBands {
        let fixture: ConformalFixture = serde_json::from_str(&read("conformal.json")).unwrap();
        fixture.stub_bands.to_bands()
    }

    struct ScenarioData {
        spot: QuarterMap,
        neighbours: NeighborPrices,
        wind_forecast: Vec<GridRecord>,
        wind_actual: Vec<GridRecord>,
        cons_forecast: Vec<GridRecord>,
        cons_actual: Vec<GridRecord>,
    }

    impl ScenarioData {
        fn new(dataset: &Dataset, ranges: &Ranges) -> Self {
            let mut neighbours = NeighborPrices::new();
            for (area, run, range) in [
                (Area::Se1, &dataset.se1, ranges.se1),
                (Area::Se3, &dataset.se3, ranges.se3),
                (Area::Ee, &dataset.ee, ranges.ee),
            ] {
                if range.is_some() {
                    neighbours.insert(area, run_map(run, range));
                }
            }
            Self {
                spot: run_map(&dataset.fi, ranges.fi),
                neighbours,
                wind_forecast: records(&dataset.wind_forecast, ranges.wind_forecast),
                wind_actual: records(&dataset.wind_actual, ranges.wind_actual),
                cons_forecast: records(&dataset.cons_forecast, ranges.cons_forecast),
                cons_actual: records(&dataset.cons_actual, ranges.cons_actual),
            }
        }

        fn input(&self, s: &Scenario) -> ForecastInput<'_> {
            ForecastInput {
                spot_prices: &self.spot,
                neighbor_prices: &self.neighbours,
                wind_forecast: &self.wind_forecast,
                wind_actual: &self.wind_actual,
                consumption_forecast: &self.cons_forecast,
                consumption_actual: &self.cons_actual,
                series_start: ts(s.series_start_ms),
                series_end: ts(s.series_end_ms),
            }
        }
    }

    /// The steps of `build_forecast` with the public functions, before rounding.
    fn intermediates(
        data: &ScenarioData,
        s: &Scenario,
        opts: &ForecastOptions<'_>,
        stats: &Stats,
    ) -> Vec<f64> {
        let exp = &s.expected;
        let start = q(s.series_start_ms);
        let end = q(s.series_end_ms);
        let wind_q = bucket_records(&data.wind_forecast);
        let wind_actual_q = bucket_records(&data.wind_actual);
        let cons_q = bucket_records(&data.cons_forecast);
        let cons_actual_q = expand_hourly_to_quarters(&data.cons_actual);
        let cons_extended = extend_with_last_week(
            &cons_q,
            &cons_actual_q,
            end.as_millisecond(),
            opts.consumption_extension_weeks,
        );
        let wind_extended = extend_with_last_week(
            &wind_q,
            &wind_actual_q,
            end.as_millisecond(),
            opts.wind_extension_weeks,
        );
        let mut wind = wind_actual_q;
        wind.extend(wind_extended);
        let mut consumption = cons_actual_q;
        consumption.extend(cons_extended);
        let ctx = FeatureContext {
            fi_prices: &data.spot,
            neighbor_prices: &data.neighbours,
            wind: &wind,
            consumption: &consumption,
        };
        let index = build_price_index_with(&ctx, opts.latest_published);
        stats.close(index.neutral(), exp.neutral, "neutral");
        let training = build_training_matrix(ts(start.as_millisecond()), &ctx, Some(&index));
        assert_eq!(training.targets.len(), exp.training.count, "training count");
        for (k, &i) in exp.training.sample_indices.iter().enumerate() {
            assert_eq!(
                training.keys[i].as_millisecond(),
                exp.training.sample_keys_ms[k]
            );
            stats.all_close(
                &training.rows[i],
                &exp.training.sample_rows[k],
                "training row",
            );
            stats.close(
                training.targets[i],
                exp.training.sample_targets[k],
                "training target",
            );
        }
        let fitted = fit_ridge(
            &TrainingData {
                feature_count: training.feature_names.len(),
                rows: &training.rows,
                targets: &training.targets,
            },
            &FitOptions {
                ridge_lambda: opts.ridge_lambda,
                price_transform: PriceTransform::Arcsinh,
            },
        );
        let mut raw = QuarterMap::new();
        let mut cursor = start;
        while cursor < end {
            raw.insert(
                cursor,
                fitted.predict(&build_feature_vector_indexed(cursor, &ctx, &index)),
            );
            cursor = cursor.offset(15 * 60 * 1000);
        }
        let raw_values: Vec<f64> = raw.values().copied().collect();
        stats.all_close(&raw_values, &exp.raw_predictions, "raw prediction");
        let shaped = apply_within_day_shape(&raw, &data.spot);
        let shaped_values: Vec<f64> = shaped.result.values().copied().collect();
        stats.all_close(&shaped_values, &exp.shaped, "shaped");
        let clamped: Vec<f64> = shaped_values
            .iter()
            .map(|&v| opts.sanity_bound.map_or(v, |b| v.max(b.min).min(b.max)))
            .collect();
        stats.all_close(&clamped, &exp.clamped, "clamped");
        clamped
    }

    fn check_entries(
        result: &spot_price_domain::forecast::ForecastResult,
        case: &EntriesCase,
        stats: &Stats,
    ) {
        let tz = TimeZone::get(&case.time_zone).unwrap();
        let settings = user_settings(&case.settings);
        let entries = to_forecast_entries(result, &tz, |spot, hour| {
            apply_contract_terms(spot, &settings, u8::try_from(hour).unwrap()).total_cents_kwh
        })
        .unwrap();
        assert_eq!(
            entries.len(),
            case.start.len(),
            "{}: entry count",
            case.name
        );
        let mut banded = 0;
        for (i, entry) in entries.iter().enumerate() {
            let what = format!("{} entry {i}", case.name);
            assert_eq!(iso_millis(entry.start), case.start[i], "{what}");
            assert_eq!(iso_millis(entry.end), case.end[i], "{what}");
            assert_eq!(entry.local_start, case.local_start[i], "{what}");
            assert_eq!(entry.local_end, case.local_end[i], "{what}");
            assert!(
                entry.estimated_spot_cents_kwh == case.spot[i],
                "{what}: spot"
            );
            stats.rounded(
                entry.estimated_total_cents_kwh,
                case.total[i],
                f64::NAN,
                &what,
            );
            if let Some(band) = entry.band {
                assert!(
                    band.spot_low_cents_kwh == case.spot_low[banded],
                    "{what}: spot low"
                );
                assert!(
                    band.spot_high_cents_kwh == case.spot_high[banded],
                    "{what}: spot high"
                );
                assert!(
                    band.total_low_cents_kwh == case.total_low[banded],
                    "{what}: total low"
                );
                assert!(
                    band.total_high_cents_kwh == case.total_high[banded],
                    "{what}: total high"
                );
                banded += 1;
            }
        }
        assert_eq!(banded, case.spot_low.len(), "{}: banded entries", case.name);
    }

    /// The route of a route scenario: the served rules, the TypeScript
    /// diagnostics, and the TypeScript assessment. `opts` holds the route
    /// sanity bound and the persistence shape of the TypeScript service.
    fn check_route(s: &Scenario, input: &ForecastInput<'_>, opts: &ForecastOptions<'_>) {
        let exp = &s.expected;
        let zone = TimeZone::get(DELIVERY_DAY_TIME_ZONE).unwrap();
        match forecast_for_route(input, &zone) {
            RouteForecast::NoGridData => assert_eq!(exp.grid_record_count, 0, "{}", s.name),
            RouteForecast::Ready {
                result: route_result,
                assessment,
            } => {
                assert!(exp.grid_record_count > 0, "{}", s.name);
                let persistence = ForecastOptions {
                    latest_published: LatestPublishedRule::default(),
                    grid_gaps: GridGapRule::default(),
                    ..*opts
                };
                let served = ForecastOptions {
                    shape_profile: ForecastOptions::served(&zone).shape_profile,
                    ..persistence
                };
                assert_eq!(
                    *route_result,
                    build_forecast(input, &served),
                    "{}: route result",
                    s.name
                );
                // Only the shape fallback count depends on the shape profile.
                let typescript_shape = build_forecast(input, &persistence).diagnostics;
                assert_eq!(
                    Diagnostics::from(ForecastDiagnostics {
                        shape_fallback_quarters: typescript_shape.shape_fallback_quarters,
                        ..route_result.diagnostics
                    }),
                    exp.diagnostics,
                    "{}: route diagnostics",
                    s.name
                );
                assert_eq!(assessment.degraded, exp.assessment.degraded, "{}", s.name);
                assert_eq!(
                    assessment.reason,
                    exp.assessment.reason.as_deref(),
                    "{}",
                    s.name
                );
            }
        }
    }

    #[test]
    fn scenarios_match_typescript() {
        let dataset: Dataset = serde_json::from_str(&read("dataset.json")).unwrap();
        let scenarios: Vec<Scenario> = serde_json::from_str(&read("scenarios.json")).unwrap();
        let stub = stub_bands();
        let stats = Stats::default();
        for s in &scenarios {
            let exp = &s.expected;
            let data = ScenarioData::new(&dataset, &s.ranges);
            let input = data.input(s);

            let route_sanity = sanity_bound_from_history(&data.spot);
            match (route_sanity, exp.route_sanity_bound) {
                (Some(g), Some(w)) => {
                    stats.close(g.min, w.min, "route sanity min");
                    stats.close(g.max, w.max, "route sanity max");
                }
                (None, None) => {}
                (g, _) => panic!("{}: route sanity bound {g:?}", s.name),
            }

            // The TypeScript service used the at-or-before rule, the
            // persistence shape, and 0 MW for a missing grid value.
            let mut opts = ForecastOptions {
                latest_published: LatestPublishedRule::AtOrBefore,
                shape_profile: ShapeProfile::Persistence,
                grid_gaps: GridGapRule::Zero,
                ..ForecastOptions::default()
            };
            if let Some(b) = s.options.sanity_bound {
                opts.sanity_bound = Some(SanityBound {
                    min: b.min,
                    max: b.max,
                });
            }
            if let Some(lambda) = s.options.ridge_lambda {
                opts.ridge_lambda = lambda;
            }
            if let Some(weeks) = s.options.wind_extension_weeks {
                opts.wind_extension_weeks = weeks;
            }
            if let Some(weeks) = s.options.consumption_extension_weeks {
                opts.consumption_extension_weeks = weeks;
            }
            if s.options.stub_bands == Some(true) {
                opts.bands = &stub;
            }
            if s.route {
                opts.sanity_bound = route_sanity;
            }

            let pre_round = intermediates(&data, s, &opts, &stats);
            let result = build_forecast(&input, &opts);

            assert_eq!(
                Diagnostics::from(result.diagnostics),
                exp.diagnostics,
                "{}",
                s.name
            );
            let start_ms: Vec<i64> = result
                .series
                .iter()
                .map(|p| p.start.as_millisecond())
                .collect();
            assert_eq!(start_ms, exp.series.start_ms, "{}: starts", s.name);
            for (i, point) in result.series.iter().enumerate() {
                stats.rounded(
                    point.estimated_spot_cents_kwh,
                    exp.series.estimate[i],
                    pre_round[i],
                    &format!("{} estimate {i}", s.name),
                );
            }
            let bands: Vec<_> = result.series.iter().filter_map(|p| p.band).collect();
            assert_eq!(bands.len(), exp.series.banded_count, "{}: banded", s.name);
            for (i, band) in bands.iter().enumerate() {
                assert!(band.low == exp.series.low[i], "{}: low {i}", s.name);
                assert!(band.high == exp.series.high[i], "{}: high {i}", s.name);
            }

            if s.route {
                check_route(s, &input, &opts);
            }

            for case in &exp.entries {
                check_entries(&result, case, &stats);
            }
        }
        stats.report("scenarios");
    }

    #[test]
    fn the_window_matches_the_route_arithmetic() {
        let last = ts(1_774_396_800_000);
        let window = ForecastWindow::new(Some(last), ts(0)).unwrap();
        assert_eq!(
            window.series_start.as_millisecond(),
            1_774_396_800_000 + 900_000
        );
        assert_eq!(
            window.series_end.as_millisecond(),
            window.series_start.as_millisecond() + 3 * 86_400_000
        );
        assert_eq!(
            window.history_start.as_millisecond(),
            window.series_start.as_millisecond() - 30 * 86_400_000
        );
        let now = ts(1_774_400_000_123);
        assert_eq!(ForecastWindow::new(None, now).unwrap().series_start, now);
    }

    // ---------------------------------------------------------------------------
    // Unit vectors
    // ---------------------------------------------------------------------------

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SeriesFixture {
        quarter_keys: Vec<QuarterKeyCase>,
        bucket: RecordsCase,
        expand_hourly: RecordsCase,
        extend: Vec<ExtendCase>,
        align: AlignCase,
        shape: ShapeCase,
        predicted_series: PredictedSeriesCase,
        zero_seeded: ZeroSeededCase,
        sanity: Vec<SanityCase>,
    }

    #[derive(Deserialize)]
    struct QuarterKeyCase {
        ms: i64,
        key: String,
    }

    #[derive(Deserialize)]
    struct RecordsCase {
        input: Columns,
        expected: KeyedValues,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ExtendCase {
        weeks: u32,
        series_end_ms: i64,
        forecast: KeyedValues,
        actual: KeyedValues,
        expected: KeyedValues,
    }

    #[derive(Deserialize)]
    struct AlignCase {
        price: KeyedValues,
        residual: KeyedValues,
        xs: Vec<f64>,
        ys: Vec<f64>,
        slope: f64,
        intercept: f64,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ShapeCase {
        predicted: KeyedValues,
        spot: KeyedValues,
        expected: KeyedValues,
        shape_fallback_quarters: usize,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PredictedSeriesCase {
        predicted: KeyedValues,
        series_start_ms: i64,
        num_quarters: usize,
        estimate: Vec<f64>,
        filled_quarters: usize,
        zero_seeded_quarters: usize,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ZeroSeededCase {
        estimate: Vec<f64>,
        zero_seeded_quarters: usize,
    }

    #[derive(Deserialize)]
    struct SanityCase {
        history: KeyedValues,
        expected: Option<Bound>,
    }

    #[test]
    fn series_helpers_match_typescript() {
        let f: SeriesFixture = serde_json::from_str(&read("series.json")).unwrap();
        let stats = Stats::default();

        for case in &f.quarter_keys {
            assert_eq!(q(case.ms).key().unwrap(), case.key);
        }

        let all = Some([i64::MIN, i64::MAX]);
        f.bucket.expected.check(
            &bucket_records(&records(&f.bucket.input, all)),
            &stats,
            "bucket",
        );
        f.expand_hourly.expected.check(
            &expand_hourly_to_quarters(&records(&f.expand_hourly.input, all)),
            &stats,
            "expand hourly",
        );

        for case in &f.extend {
            let out = extend_with_last_week(
                &case.forecast.to_map(),
                &case.actual.to_map(),
                case.series_end_ms,
                case.weeks,
            );
            case.expected.check(&out, &stats, "extend");
        }

        let (xs, ys) = align_series(&f.align.price.to_map(), &f.align.residual.to_map());
        stats.all_close(&xs, &f.align.xs, "align xs");
        stats.all_close(&ys, &f.align.ys, "align ys");
        let fit = fit_linear(&xs, &ys).unwrap();
        stats.close(fit.slope, f.align.slope, "slope");
        stats.close(fit.intercept, f.align.intercept, "intercept");

        let shaped = apply_within_day_shape(&f.shape.predicted.to_map(), &f.shape.spot.to_map());
        f.shape.expected.check(&shaped.result, &stats, "shape");
        assert_eq!(
            shaped.shape_fallback_quarters,
            f.shape.shape_fallback_quarters
        );

        let ps = &f.predicted_series;
        let built = build_predicted_series(
            &ps.predicted.to_map(),
            q(ps.series_start_ms),
            ps.num_quarters,
        );
        let estimates: Vec<f64> = built
            .series
            .iter()
            .map(|p| p.estimated_spot_cents_kwh)
            .collect();
        assert_eq!(estimates, ps.estimate);
        stats
            .rounded_count
            .set(stats.rounded_count.get() + estimates.len());
        assert_eq!(built.filled_quarters, ps.filled_quarters);
        assert_eq!(built.zero_seeded_quarters, ps.zero_seeded_quarters);

        let built = build_predicted_series(&QuarterMap::new(), q(ps.series_start_ms), 3);
        let estimates: Vec<f64> = built
            .series
            .iter()
            .map(|p| p.estimated_spot_cents_kwh)
            .collect();
        assert_eq!(estimates, f.zero_seeded.estimate);
        assert_eq!(
            built.zero_seeded_quarters,
            f.zero_seeded.zero_seeded_quarters
        );

        for case in &f.sanity {
            match (
                sanity_bound_from_history(&case.history.to_map()),
                case.expected,
            ) {
                (Some(g), Some(w)) => {
                    stats.close(g.min, w.min, "sanity min");
                    stats.close(g.max, w.max, "sanity max");
                }
                (None, None) => {}
                (g, _) => panic!("sanity bound {g:?}"),
            }
        }
        stats.report("series");
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FeaturesFixture {
        feature_names: Vec<String>,
        neighbour_ranges: BTreeMap<String, [i64; 2]>,
        instants_ms: Vec<i64>,
        vectors: Vec<Vec<f64>>,
        training: FeatureTraining,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FeatureTraining {
        origin_ms: i64,
        count: usize,
        first_key_ms: i64,
        last_key_ms: i64,
        sample_indices: Vec<usize>,
        sample_rows: Vec<Vec<f64>>,
        sample_targets: Vec<f64>,
    }

    #[test]
    fn features_match_typescript() {
        let dataset: Dataset = serde_json::from_str(&read("dataset.json")).unwrap();
        let f: FeaturesFixture = serde_json::from_str(&read("features.json")).unwrap();
        let stats = Stats::default();
        assert_eq!(f.feature_names, spot_price_domain::forecast::FEATURE_NAMES);

        let all = Some([i64::MIN, i64::MAX]);
        let spot = run_map(&dataset.fi, all);
        let mut neighbours = NeighborPrices::new();
        neighbours.insert(
            Area::Se1,
            run_map(&dataset.se1, Some(f.neighbour_ranges["SE1"])),
        );
        neighbours.insert(
            Area::Ee,
            run_map(&dataset.ee, Some(f.neighbour_ranges["EE"])),
        );
        let wind = bucket_records(&records(&dataset.wind_actual, all));
        let consumption = expand_hourly_to_quarters(&records(&dataset.cons_actual, all));
        let ctx = FeatureContext {
            fi_prices: &spot,
            neighbor_prices: &neighbours,
            wind: &wind,
            consumption: &consumption,
        };
        // The TypeScript service used the at-or-before rule.
        let index = build_price_index_with(&ctx, LatestPublishedRule::AtOrBefore);
        for (ms, want) in f.instants_ms.iter().zip(&f.vectors) {
            stats.all_close(
                &build_feature_vector_indexed(q(*ms), &ctx, &index),
                want,
                &format!("vector at {ms}"),
            );
        }
        let training = build_training_matrix(ts(f.training.origin_ms), &ctx, Some(&index));
        assert_eq!(training.targets.len(), f.training.count);
        assert_eq!(training.keys[0].as_millisecond(), f.training.first_key_ms);
        assert_eq!(
            training.keys.last().unwrap().as_millisecond(),
            f.training.last_key_ms
        );
        for (k, &i) in f.training.sample_indices.iter().enumerate() {
            stats.all_close(
                &training.rows[i],
                &f.training.sample_rows[k],
                "training row",
            );
            stats.close(
                training.targets[i],
                f.training.sample_targets[k],
                "training target",
            );
        }
        stats.report("features");
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ModelCase {
        name: String,
        feature_count: usize,
        rows: Vec<Vec<f64>>,
        targets: Vec<f64>,
        options: ModelOptions,
        probes: Vec<Vec<f64>>,
        meta: Meta,
        predictions: Vec<f64>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ModelOptions {
        ridge_lambda: Option<f64>,
        price_transform: Option<String>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Meta {
        used_fallback: bool,
        sample_count: usize,
        feature_count: usize,
    }

    #[test]
    fn ridge_fits_match_typescript() {
        let cases: Vec<ModelCase> = serde_json::from_str(&read("model.json")).unwrap();
        let stats = Stats::default();
        for case in &cases {
            let mut opts = FitOptions::default();
            if let Some(lambda) = case.options.ridge_lambda {
                opts.ridge_lambda = lambda;
            }
            if case.options.price_transform.as_deref() == Some("none") {
                opts.price_transform = PriceTransform::None;
            }
            let fitted = fit_ridge(
                &TrainingData {
                    feature_count: case.feature_count,
                    rows: &case.rows,
                    targets: &case.targets,
                },
                &opts,
            );
            assert_eq!(
                fitted.meta.used_fallback, case.meta.used_fallback,
                "{}",
                case.name
            );
            assert_eq!(
                fitted.meta.sample_count, case.meta.sample_count,
                "{}",
                case.name
            );
            assert_eq!(
                fitted.meta.feature_count, case.meta.feature_count,
                "{}",
                case.name
            );
            let predictions: Vec<f64> = case.probes.iter().map(|x| fitted.predict(x)).collect();
            stats.all_close(&predictions, &case.predictions, &case.name);
        }
        stats.report("model");
    }

    #[test]
    fn bands_match_typescript() {
        let f: ConformalFixture = serde_json::from_str(&read("conformal.json")).unwrap();
        let stats = Stats::default();
        let residuals = f.residuals.residuals();

        let derived = derive_band_offsets(&residuals, &DeriveOptions::default());
        check_offsets(
            &derived.offsets_by_hour,
            &f.derived.offsets_by_hour,
            &stats,
            "derived",
        );
        check_offset(
            derived.global_offsets,
            f.derived.global_offsets,
            &stats,
            "derived global",
        );
        check_coverage(
            observed_coverage_of(&residuals, &derived.offsets_by_hour, derived.global_offsets),
            f.derived.coverage,
            &residuals,
            &derived.offsets_by_hour,
            derived.global_offsets,
            &stats,
            "coverage",
        );

        let wide = derive_band_offsets(
            &residuals,
            &DeriveOptions {
                nominal_coverage: 0.5,
                min_hour_samples: 30,
            },
        );
        check_offsets(
            &wide.offsets_by_hour,
            &f.derived_wide.offsets_by_hour,
            &stats,
            "wide",
        );
        check_offset(
            wide.global_offsets,
            f.derived_wide.global_offsets,
            &stats,
            "wide global",
        );
        check_coverage(
            observed_coverage_of(&residuals, &wide.offsets_by_hour, wide.global_offsets),
            f.derived_wide.coverage,
            &residuals,
            &wide.offsets_by_hour,
            wide.global_offsets,
            &stats,
            "wide coverage",
        );

        let generated_at = "2026-06-01T00:00:00.000Z".to_owned();
        f.artifacts.too_few.check(
            &build_artifact(&residuals[..95], generated_at.clone()),
            &residuals[..95],
            &stats,
            "too few",
        );
        f.artifacts.calibrated.check(
            &build_artifact(&residuals, generated_at.clone()),
            &residuals,
            &stats,
            "calibrated",
        );
        let below = f.below_gate_residuals.residuals();
        f.artifacts.below_gate.check(
            &build_artifact(&below, generated_at),
            &below,
            &stats,
            "below gate",
        );

        let stub = f.stub_bands.to_bands();
        for applied in &f.applied {
            let hour = UtcHour::new(applied.hour).unwrap();
            for (point, want) in f.apply_points.iter().zip(&applied.bands) {
                let got = apply_band(*point, hour, &stub);
                match (got, want) {
                    (Some(g), Some(w)) => {
                        let what = format!("band at {point}, hour {}", applied.hour);
                        stats.rounded(g.low, w.low, f64::NAN, &what);
                        stats.rounded(g.high, w.high, f64::NAN, &what);
                    }
                    (None, None) => {}
                    (g, _) => panic!("band at {point}: {g:?}"),
                }
            }
        }
        stats.report("conformal");
    }
}
