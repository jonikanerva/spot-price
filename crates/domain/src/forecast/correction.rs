//! The quarter correction of the FI forecast: an opt-in option that the
//! served forecast does not use. A closed-form ridge fit learns, per target
//! quarter, the error of a historical price baseline from issuance training
//! examples. The correction is added after the within-day shape step, so the
//! step cannot erase a quarter-level change.
//!
//! The baseline `b(q)` is the profile price of the shape profile of the run.
//! Training issuance `i` reads it from the prices published at `i`.

use std::collections::BTreeMap;

use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::time::local_date;

use super::features::TRAINING_HISTORY_DAYS;
use super::holiday::is_finnish_public_holiday;
use super::issuance::{
    ExcludedExamples, IssuanceHistory, ServedView, build_issuance_examples, issuance_feature_names,
    served_rows,
};
use super::js::count_f64;
use super::model::{FitOptions, PriceTransform, TrainingData, fit_ridge_weighted};
use super::pipeline::{ForecastInput, ForecastOptions};
use super::quarter::{DAY_MS, QuarterMap, QuarterStart};
use super::route::{FLOOR_HISTORY_DAYS, FORECAST_DAYS};
use super::series::profile_values;

/// The days of prices and actuals before the served price window that the
/// examples need. The oldest training issuance is about
/// `TRAINING_HISTORY_DAYS + FORECAST_DAYS` days before the series start, and
/// it reads its own window of [`FLOOR_HISTORY_DAYS`] days.
pub const CORRECTION_HISTORY_DAYS: i64 = TRAINING_HISTORY_DAYS + FORECAST_DAYS + 1;

/// The column after the issuance-training columns: the baseline price.
pub const BASELINE_FEATURE_NAME: &str = "baseline_price";
/// The optional last column: 1 on a Finnish public holiday.
pub const HOLIDAY_FEATURE_NAME: &str = "fi_holiday";

/// How the correction changes the forecast.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Decomposition {
    /// One ridge fit on `realised − b(q)`. The output is `b(q) + c(q)`: the
    /// correction sets the daily level and the within-day shape.
    #[default]
    Joint,
    /// The daily level of the shape step stays. A ridge fit on the
    /// within-day deviation, `(y − ȳ) − (b − b̄)` per issuance and UTC day,
    /// adds `d(q) − d̄` to the shape-step output.
    DayDeviation,
}

/// One training example of the correction.
#[derive(Debug, Clone, PartialEq)]
pub struct CorrectionExample {
    pub issued_at: Timestamp,
    pub target: QuarterStart,
    /// The issuance-training row, the baseline price, and the optional
    /// holiday column; see [`correction_feature_names`].
    pub features: Vec<f64>,
    /// The baseline price `b(q)` (c/kWh) at the issuance.
    pub baseline: f64,
    /// The published FI price (c/kWh) of the target.
    pub realised: f64,
}

/// The training examples of one correction run. Build them once per run
/// with [`build_correction_examples`]; every correction setting of the run
/// can share them.
#[derive(Debug, Clone, PartialEq)]
pub struct CorrectionExamples {
    /// The issue time of the run.
    pub issued_at: Timestamp,
    pub examples: Vec<CorrectionExample>,
    /// The zone of the holiday column; `None` without the column.
    pub holiday_zone: Option<TimeZone>,
    /// Issuance-training targets left out for a missing grid value.
    pub excluded: ExcludedExamples,
    /// Issuance-training examples left out because the issuance had no
    /// profile price for the target.
    pub without_baseline: usize,
}

/// The feature columns of the correction in their fixed order.
#[must_use]
pub fn correction_feature_names(holiday: bool) -> Vec<&'static str> {
    let mut names = issuance_feature_names();
    names.push(BASELINE_FEATURE_NAME);
    if holiday {
        names.push(HOLIDAY_FEATURE_NAME);
    }
    names
}

fn holiday_column(q: QuarterStart, zone: &TimeZone) -> f64 {
    let Ok(at) = q.timestamp() else {
        return 0.0;
    };
    if is_finnish_public_holiday(local_date(at, zone)) {
        1.0
    } else {
        0.0
    }
}

/// The correction examples of a run. `input` holds the served inputs plus
/// the older prices and actuals of [`CORRECTION_HISTORY_DAYS`]; its series
/// bounds are the run's. Each issuance-training example (see
/// [`build_issuance_examples`]) gets the profile price of `opts.shape_profile`
/// from the FI prices that its issuance had. An example without one is left
/// out and counted, not filled.
#[must_use]
pub fn build_correction_examples(
    input: &ForecastInput<'_>,
    opts: &ForecastOptions<'_>,
    history: &IssuanceHistory<'_>,
    holiday_zone: Option<&TimeZone>,
) -> CorrectionExamples {
    let issuance = build_issuance_examples(input, opts, history);
    let mut by_issuance: BTreeMap<QuarterStart, Vec<usize>> = BTreeMap::new();
    for (i, e) in issuance.examples.iter().enumerate() {
        by_issuance.entry(e.series_start).or_default().push(i);
    }
    let mut baselines: Vec<Option<f64>> = vec![None; issuance.examples.len()];
    for (series_start, indices) in &by_issuance {
        let history_start = series_start.offset(-FLOOR_HISTORY_DAYS * DAY_MS);
        let published: QuarterMap = input
            .spot_prices
            .range(history_start..*series_start)
            .map(|(&k, &v)| (k, v))
            .collect();
        let keys = indices
            .iter()
            .filter_map(|&i| issuance.examples.get(i).map(|e| e.target));
        let profile = profile_values(opts.shape_profile, keys, &published).values;
        for &i in indices {
            if let (Some(slot), Some(e)) = (baselines.get_mut(i), issuance.examples.get(i)) {
                *slot = profile.get(&e.target).copied();
            }
        }
    }
    let mut out = CorrectionExamples {
        issued_at: history.issued_at,
        examples: Vec::new(),
        holiday_zone: holiday_zone.cloned(),
        excluded: issuance.excluded(),
        without_baseline: 0,
    };
    for (e, baseline) in issuance.examples.into_iter().zip(baselines) {
        let Some(baseline) = baseline else {
            out.without_baseline += 1;
            continue;
        };
        let mut features = e.features;
        features.push(baseline);
        if let Some(zone) = holiday_zone {
            features.push(holiday_column(e.target, zone));
        }
        out.examples.push(CorrectionExample {
            issued_at: e.issued_at,
            target: e.target,
            features,
            baseline,
            realised: e.realised,
        });
    }
    out
}

/// The correction setting of one forecast run.
#[derive(Debug, Clone, Copy)]
pub struct QuarterCorrection<'a> {
    pub examples: &'a CorrectionExamples,
    pub decomposition: Decomposition,
    pub ridge_lambda: f64,
    /// Only examples with a target at most this many days before the series
    /// start train the fit; at most [`TRAINING_HISTORY_DAYS`] count.
    pub window_days: i64,
    /// The weight of an example halves for each this many days of age of its
    /// issuance before the run. `None` weights every example the same.
    pub half_life_days: Option<f64>,
}

/// The counters of one correction run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CorrectionDiagnostics {
    pub fit_samples: usize,
    /// True when the fit has too few samples or no solution. The output is
    /// then the shape-step output: no correction.
    pub used_default: bool,
    pub corrected_quarters: usize,
    /// Quarters without a profile price at the run; they keep the
    /// shape-step output.
    pub uncorrected_quarters: usize,
    /// Training targets left out for a missing grid value.
    pub excluded: ExcludedExamples,
    /// Training examples left out without a profile price.
    pub without_baseline: usize,
}

/// The rows, targets, and weights of the fit.
struct Training {
    rows: Vec<Vec<f64>>,
    targets: Vec<f64>,
    weights: Vec<f64>,
}

fn training(correction: &QuarterCorrection<'_>, series_start: QuarterStart) -> Training {
    let window_start =
        series_start.offset(-correction.window_days.min(TRAINING_HISTORY_DAYS) * DAY_MS);
    let run_ms = correction.examples.issued_at.as_millisecond();
    let selected: Vec<&CorrectionExample> = correction
        .examples
        .examples
        .iter()
        .filter(|e| e.target >= window_start && e.target < series_start)
        .collect();
    let weight = |e: &CorrectionExample| match correction.half_life_days {
        Some(half_life) if half_life > 0.0 => {
            #[expect(clippy::cast_precision_loss, reason = "an age in ms is far below 2^53")]
            let age_days = (run_ms - e.issued_at.as_millisecond()) as f64 / DAY_MS as f64;
            0.5_f64.powf(age_days / half_life)
        }
        _ => 1.0,
    };
    let mut out = Training {
        rows: Vec::with_capacity(selected.len()),
        targets: Vec::with_capacity(selected.len()),
        weights: selected.iter().map(|e| weight(e)).collect(),
    };
    match correction.decomposition {
        Decomposition::Joint => {
            for e in &selected {
                out.rows.push(e.features.clone());
                out.targets.push(e.realised - e.baseline);
            }
        }
        Decomposition::DayDeviation => {
            let mut groups: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
            for (i, e) in selected.iter().enumerate() {
                groups
                    .entry((e.issued_at.as_millisecond(), e.target.utc_day_start_ms()))
                    .or_default()
                    .push(i);
            }
            let mut demeaned: Vec<(usize, Vec<f64>, f64)> = Vec::with_capacity(selected.len());
            for indices in groups.values() {
                let members: Vec<&CorrectionExample> = indices
                    .iter()
                    .filter_map(|&i| selected.get(i).copied())
                    .collect();
                let n = count_f64(members.len());
                let width = members.first().map_or(0, |e| e.features.len());
                let mut mean_row = vec![0.0; width];
                let mut mean_residual = 0.0;
                for e in &members {
                    for (slot, v) in mean_row.iter_mut().zip(&e.features) {
                        *slot += v / n;
                    }
                    mean_residual += (e.realised - e.baseline) / n;
                }
                for (&i, e) in indices.iter().zip(&members) {
                    let row = e
                        .features
                        .iter()
                        .zip(&mean_row)
                        .map(|(v, m)| v - m)
                        .collect();
                    demeaned.push((i, row, e.realised - e.baseline - mean_residual));
                }
            }
            demeaned.sort_by_key(|(i, ..)| *i);
            for (_, row, target) in demeaned {
                out.rows.push(row);
                out.targets.push(target);
            }
        }
    }
    out
}

/// Fit the correction and add it to `predicted`, the shape-step output.
pub(super) fn apply(
    predicted: &mut QuarterMap,
    input: &ForecastInput<'_>,
    opts: &ForecastOptions<'_>,
    correction: &QuarterCorrection<'_>,
    served: &ServedView<'_>,
) -> CorrectionDiagnostics {
    let examples = correction.examples;
    let fit_data = training(correction, served.series_start);
    let width = correction_feature_names(examples.holiday_zone.is_some()).len();
    let fitted = fit_ridge_weighted(
        &TrainingData {
            feature_count: width,
            rows: &fit_data.rows,
            targets: &fit_data.targets,
        },
        &fit_data.weights,
        &FitOptions {
            ridge_lambda: correction.ridge_lambda,
            price_transform: PriceTransform::None,
        },
    );
    let mut diagnostics = CorrectionDiagnostics {
        fit_samples: fitted.meta.sample_count,
        used_default: fitted.meta.used_fallback,
        corrected_quarters: 0,
        uncorrected_quarters: 0,
        excluded: examples.excluded,
        without_baseline: examples.without_baseline,
    };
    let profile = profile_values(
        opts.shape_profile,
        predicted.keys().copied(),
        input.spot_prices,
    )
    .values;
    if fitted.meta.used_fallback {
        diagnostics.uncorrected_quarters = predicted.len();
        return diagnostics;
    }
    let mut corrections: BTreeMap<QuarterStart, (f64, f64)> = BTreeMap::new();
    for (q, mut row) in served_rows(served, examples.issued_at) {
        let (Some(&baseline), true) = (profile.get(&q), predicted.contains_key(&q)) else {
            continue;
        };
        row.push(baseline);
        if let Some(zone) = &examples.holiday_zone {
            row.push(holiday_column(q, zone));
        }
        corrections.insert(q, (baseline, fitted.predict(&row)));
    }
    match correction.decomposition {
        Decomposition::Joint => {
            for (q, (baseline, c)) in &corrections {
                if let Some(slot) = predicted.get_mut(q) {
                    *slot = baseline + c;
                }
            }
        }
        Decomposition::DayDeviation => {
            let mut day_sums: BTreeMap<i64, (f64, usize)> = BTreeMap::new();
            for (q, (_, d)) in &corrections {
                let slot = day_sums.entry(q.utc_day_start_ms()).or_insert((0.0, 0));
                slot.0 += d;
                slot.1 += 1;
            }
            for (q, (_, d)) in &corrections {
                let mean = day_sums
                    .get(&q.utc_day_start_ms())
                    .map_or(0.0, |(sum, n)| sum / count_f64(*n));
                if let Some(slot) = predicted.get_mut(q) {
                    *slot += d - mean;
                }
            }
        }
    }
    diagnostics.corrected_quarters = corrections.len();
    diagnostics.uncorrected_quarters = predicted.len() - corrections.len();
    diagnostics
}
