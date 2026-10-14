//! Empirical prediction bands (P10/P90 style) for the FI forecast.
//!
//! The band offsets come only from out-of-sample residuals of the offline
//! backtest, never from residuals of the rows that the model trained on.
//! All residual arithmetic is in arcsinh space, the space the model fits in.

use std::borrow::Cow;
use std::cmp::Ordering;

use super::js::{count_f64, js_max, js_min, round3};
use super::quarter::QuarterStart;

/// Minimum measured out-of-sample coverage for the band to ship.
pub const COVERAGE_GATE_THRESHOLD: f64 = 0.7;
/// Nominal coverage that the P10/P90 offsets target.
pub const NOMINAL_COVERAGE: f64 = 0.8;
/// Minimum residual samples for an own offset of one UTC hour. A thinner hour
/// uses the pooled global offsets.
pub const MIN_HOUR_CALIBRATION_SAMPLES: usize = 14;
/// Minimum residual samples before any calibration is attempted.
pub const MIN_TOTAL_CALIBRATION_SAMPLES: usize = 96;

/// A UTC hour of day, 0–23.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UtcHour(u8);

impl UtcHour {
    /// The hour `hour`, or `None` when it is not in 0–23.
    #[must_use]
    pub const fn new(hour: u8) -> Option<Self> {
        if hour < 24 { Some(Self(hour)) } else { None }
    }

    /// The UTC hour of the quarter start.
    #[must_use]
    pub const fn of(quarter: QuarterStart) -> Self {
        Self(quarter.utc_hour())
    }

    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }

    const fn index(self) -> usize {
        self.0 as usize
    }
}

/// Additive offsets in arcsinh space for one UTC hour: `low_offset <= 0` and
/// `high_offset >= 0`. A bound is `sinh(asinh(point) + offset)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandOffsets {
    pub low_offset: f64,
    pub high_offset: f64,
}

/// One out-of-sample residual observation from the backtest, in raw c/kWh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HorizonResidual {
    pub utc_hour: UtcHour,
    pub predicted_raw: f64,
    pub actual_raw: f64,
}

/// Offsets by UTC hour; index `h` holds hour `h`.
pub type HourOffsets = [Option<BandOffsets>; 24];

/// The band method. It is the only one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandMethod {
    EmpiricalResidual,
}

impl BandMethod {
    /// The wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EmpiricalResidual => "empirical-residual",
        }
    }
}

/// The band artifact. The request path emits bands only when `calibrated`.
#[derive(Debug, Clone, PartialEq)]
pub struct CalibratedBands {
    pub method: BandMethod,
    pub nominal_coverage: f64,
    /// Measured out-of-sample coverage, or `None` when uncalibrated.
    pub observed_coverage: Option<f64>,
    pub calibrated: bool,
    /// Empty (all `None`) when uncalibrated.
    pub offsets_by_hour: HourOffsets,
    /// Pooled offsets for an hour without its own offsets.
    pub global_offsets: Option<BandOffsets>,
    /// RFC 3339 instant of generation, or empty when uncalibrated.
    pub generated_at: Cow<'static, str>,
}

impl CalibratedBands {
    /// The number of UTC hours with their own offsets.
    #[must_use]
    pub fn hour_count(&self) -> usize {
        self.offsets_by_hour.iter().flatten().count()
    }
}

/// Options of [`derive_band_offsets`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeriveOptions {
    pub nominal_coverage: f64,
    pub min_hour_samples: usize,
}

impl Default for DeriveOptions {
    fn default() -> Self {
        Self {
            nominal_coverage: NOMINAL_COVERAGE,
            min_hour_samples: MIN_HOUR_CALIBRATION_SAMPLES,
        }
    }
}

/// The output of [`derive_band_offsets`].
#[derive(Debug, Clone, PartialEq)]
pub struct DerivedOffsets {
    pub offsets_by_hour: HourOffsets,
    pub global_offsets: Option<BandOffsets>,
}

/// A raw spot band, rounded to 0.001 c/kWh, with `low <= point <= high`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpotBand {
    pub low: f64,
    pub high: f64,
}

/// Nearest-rank percentile of an ascending slice, `p` in `[0, 1]`.
fn nearest_rank_percentile(sorted_asc: &[f64], p: f64) -> Option<f64> {
    let n = sorted_asc.len();
    if n == 0 {
        return None;
    }
    // ceil(p * n) in f64, then clamped to [1, n]. Integer rank arithmetic
    // gives other ranks, because (1 - 0.8) / 2 is 0.09999999999999998.
    let rank = (p * count_f64(n)).ceil().clamp(1.0, count_f64(n));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "rank is an integer in [1, n]; a NaN rank becomes 0 and gives None"
    )]
    let rank = rank as usize;
    sorted_asc.get(rank.checked_sub(1)?).copied()
}

fn residual_arcsinh(predicted_raw: f64, actual_raw: f64) -> f64 {
    actual_raw.asinh() - predicted_raw.asinh()
}

/// Offsets for one bucket of arcsinh residuals, clamped to `low <= 0 <= high`.
fn offsets_from_residuals(residuals: &[f64], nominal_coverage: f64) -> Option<BandOffsets> {
    if residuals.is_empty() {
        return None;
    }
    let mut sorted = residuals.to_vec();
    // A stable sort: -0.0 and 0.0 compare equal and keep their input order.
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let low_p = (1.0 - nominal_coverage) / 2.0;
    #[expect(
        clippy::manual_midpoint,
        reason = "the golden fixtures fix the arithmetic as (1 + c) / 2"
    )]
    let high_p = (1.0 + nominal_coverage) / 2.0;
    let low = nearest_rank_percentile(&sorted, low_p).unwrap_or(0.0);
    let high = nearest_rank_percentile(&sorted, high_p).unwrap_or(0.0);
    Some(BandOffsets {
        low_offset: js_min(0.0, low),
        high_offset: js_max(0.0, high),
    })
}

/// Per-UTC-hour band offsets from out-of-sample residuals, plus pooled global
/// offsets. A non-finite residual is skipped. An hour with fewer than
/// `min_hour_samples` residuals gets no own offsets.
#[must_use]
pub fn derive_band_offsets(residuals: &[HorizonResidual], opts: &DeriveOptions) -> DerivedOffsets {
    let mut by_hour: [Vec<f64>; 24] = std::array::from_fn(|_| Vec::new());
    let mut all = Vec::with_capacity(residuals.len());
    for r in residuals {
        let residual = residual_arcsinh(r.predicted_raw, r.actual_raw);
        if !residual.is_finite() {
            continue;
        }
        if let Some(bucket) = by_hour.get_mut(r.utc_hour.index()) {
            bucket.push(residual);
        }
        all.push(residual);
    }

    let global_offsets = offsets_from_residuals(&all, opts.nominal_coverage);
    let offsets_by_hour = by_hour.map(|bucket| {
        if bucket.is_empty() || bucket.len() < opts.min_hour_samples {
            None
        } else {
            offsets_from_residuals(&bucket, opts.nominal_coverage)
        }
    });
    DerivedOffsets {
        offsets_by_hour,
        global_offsets,
    }
}

fn offsets_for_hour(
    utc_hour: UtcHour,
    offsets_by_hour: &HourOffsets,
    global_offsets: Option<BandOffsets>,
) -> Option<BandOffsets> {
    offsets_by_hour
        .get(utc_hour.index())
        .copied()
        .flatten()
        .or(global_offsets)
}

/// The band around `point_raw`, or `None` when the bands are uncalibrated or
/// no offsets exist for the hour. `point_raw` must be the full point estimate
/// that the backtest residuals were measured against.
#[must_use]
pub fn apply_band(point_raw: f64, utc_hour: UtcHour, bands: &CalibratedBands) -> Option<SpotBand> {
    if !bands.calibrated {
        return None;
    }
    let offsets = offsets_for_hour(utc_hour, &bands.offsets_by_hour, bands.global_offsets)?;
    let point_a = point_raw.asinh();
    let low = js_min((point_a + offsets.low_offset).sinh(), point_raw);
    let high = js_max((point_a + offsets.high_offset).sinh(), point_raw);
    Some(SpotBand {
        low: round3(low),
        high: round3(high),
    })
}

/// The fraction of residuals whose actual price is inside the band of its
/// hour. A residual without offsets is not scored; no scored residual gives 0.
#[must_use]
pub fn observed_coverage_of(
    residuals: &[HorizonResidual],
    offsets_by_hour: &HourOffsets,
    global_offsets: Option<BandOffsets>,
) -> f64 {
    if residuals.is_empty() {
        return 0.0;
    }
    let mut covered = 0_usize;
    let mut scored = 0_usize;
    for r in residuals {
        let Some(offsets) = offsets_for_hour(r.utc_hour, offsets_by_hour, global_offsets) else {
            continue;
        };
        scored += 1;
        let point_a = r.predicted_raw.asinh();
        let low = (point_a + offsets.low_offset).sinh();
        let high = (point_a + offsets.high_offset).sinh();
        if r.actual_raw >= low && r.actual_raw <= high {
            covered += 1;
        }
    }
    if scored == 0 {
        0.0
    } else {
        count_f64(covered) / count_f64(scored)
    }
}

/// The dark artifact: no offsets, `calibrated: false`.
#[must_use]
pub const fn uncalibrated_bands() -> CalibratedBands {
    CalibratedBands {
        method: BandMethod::EmpiricalResidual,
        nominal_coverage: NOMINAL_COVERAGE,
        observed_coverage: None,
        calibrated: false,
        offsets_by_hour: [None; 24],
        global_offsets: None,
        generated_at: Cow::Borrowed(""),
    }
}

/// The artifact from out-of-sample residuals. It is dark with too few
/// samples, and dark but with the measured coverage when the coverage is
/// below [`COVERAGE_GATE_THRESHOLD`].
#[must_use]
pub fn build_artifact(residuals: &[HorizonResidual], generated_at: String) -> CalibratedBands {
    if residuals.len() < MIN_TOTAL_CALIBRATION_SAMPLES {
        return uncalibrated_bands();
    }
    let derived = derive_band_offsets(residuals, &DeriveOptions::default());
    if derived.global_offsets.is_none() && derived.offsets_by_hour.iter().all(Option::is_none) {
        return uncalibrated_bands();
    }
    let observed_coverage =
        observed_coverage_of(residuals, &derived.offsets_by_hour, derived.global_offsets);
    if observed_coverage < COVERAGE_GATE_THRESHOLD {
        return CalibratedBands {
            observed_coverage: Some(observed_coverage),
            ..uncalibrated_bands()
        };
    }
    CalibratedBands {
        method: BandMethod::EmpiricalResidual,
        nominal_coverage: NOMINAL_COVERAGE,
        observed_coverage: Some(observed_coverage),
        calibrated: true,
        offsets_by_hour: derived.offsets_by_hour,
        global_offsets: derived.global_offsets,
        generated_at: Cow::Owned(generated_at),
    }
}
