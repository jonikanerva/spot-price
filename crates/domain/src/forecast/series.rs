//! Series helpers of the forecast pipeline: bucketing, tail extension, the
//! within-day shape, the output sanity bound, and the predicted series.

use std::collections::BTreeMap;

use jiff::Timestamp;

use super::conformal::SpotBand;
use super::js::{count_f64, js_min};
use super::quarter::{DAY_MS, QUARTER_MS, QuarterMap, QuarterStart, WEEK_MS};
use crate::js::round3;

/// Multiplier on the observed price spread that widens the output sanity clamp.
pub const SANITY_MARGIN_K: f64 = 3.0;
/// Backstop lower bound (c/kWh) of the sanity clamp for a flat history.
pub const SANITY_MIN_FALLBACK_CENTS_KWH: f64 = -50.0;

/// One Fingrid observation: the start instant and the value in MW.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridRecord {
    pub start: Timestamp,
    pub value: f64,
}

/// Bucket records by quarter, with the mean of the samples in each quarter.
/// A record with a non-finite value is skipped.
#[must_use]
pub fn bucket_records(records: &[GridRecord]) -> QuarterMap {
    let mut sums: BTreeMap<QuarterStart, (f64, usize)> = BTreeMap::new();
    for record in records {
        if !record.value.is_finite() {
            continue;
        }
        let entry = sums
            .entry(QuarterStart::floor(record.start))
            .or_insert((0.0, 0));
        entry.0 += record.value;
        entry.1 += 1;
    }
    sums.into_iter()
        .map(|(key, (sum, count))| (key, sum / count_f64(count)))
        .collect()
}

/// Bucket records by quarter, and fill the `:15`, `:30`, and `:45` quarters of
/// an hour that has only its top-of-hour value. Quarter-resolution input
/// keeps its own values.
#[must_use]
pub fn expand_hourly_to_quarters(records: &[GridRecord]) -> QuarterMap {
    let quarters = bucket_records(records);
    let mut out = quarters.clone();
    for (&key, &value) in &quarters {
        if key.as_millisecond().rem_euclid(super::quarter::HOUR_MS) == 0 {
            for offset_quarters in 1..=3 {
                let fill = key.offset(offset_quarters * QUARTER_MS);
                if !quarters.contains_key(&fill) {
                    out.insert(fill, value);
                }
            }
        }
    }
    out
}

/// Fill the quarters after the last `forecast` quarter, up to `series_end_ms`
/// (exclusive), with the mean of the same weekday and quarter over the last
/// `weeks` weeks of `actual`. A quarter with no history stays unfilled.
#[must_use]
pub fn extend_with_last_week(
    forecast: &QuarterMap,
    actual: &QuarterMap,
    series_end_ms: i64,
    weeks: u32,
) -> QuarterMap {
    let mut out = forecast.clone();
    let Some(&last_known) = forecast.keys().next_back() else {
        return out;
    };
    let mut cursor = last_known.as_millisecond().saturating_add(QUARTER_MS);
    while cursor < series_end_ms {
        let mut sum = 0.0;
        let mut found = 0_usize;
        for week in 1..=i64::from(weeks) {
            let previous = QuarterStart::from_millisecond(cursor - week * WEEK_MS);
            if let Some(value) = actual.get(&previous) {
                sum += value;
                found += 1;
            }
        }
        if found > 0 {
            out.insert(
                QuarterStart::from_millisecond(cursor),
                sum / count_f64(found),
            );
        }
        cursor += QUARTER_MS;
    }
    out
}

/// Aligned `(x, y)` pairs over the quarters present in both maps, in
/// chronological order. `x` comes from `residuals`, `y` from `prices`.
#[must_use]
pub fn align_series(prices: &QuarterMap, residuals: &QuarterMap) -> (Vec<f64>, Vec<f64>) {
    prices
        .iter()
        .filter_map(|(key, &y)| residuals.get(key).map(|&x| (x, y)))
        .unzip()
}

/// The coefficients of `y = slope * x + intercept`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinearFit {
    pub slope: f64,
    pub intercept: f64,
}

/// Why [`fit_linear`] has no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LinearFitError {
    #[error("Need >= 2 matching points to fit.")]
    TooFewPoints,
    #[error("Zero variance in x; cannot fit.")]
    ZeroVariance,
}

/// Closed-form two-parameter ordinary least squares.
///
/// # Errors
///
/// Returns [`LinearFitError::TooFewPoints`] for fewer than two points or
/// slices of different lengths, and [`LinearFitError::ZeroVariance`] when all
/// `x` values are equal.
pub fn fit_linear(xs: &[f64], ys: &[f64]) -> Result<LinearFit, LinearFitError> {
    let n = xs.len();
    if n < 2 || n != ys.len() {
        return Err(LinearFitError::TooFewPoints);
    }
    let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
    for (&x, &y) in xs.iter().zip(ys) {
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
    }
    let n = count_f64(n);
    let denom = n * sxx - sx * sx;
    if denom == 0.0 {
        return Err(LinearFitError::ZeroVariance);
    }
    let slope = (n * sxy - sx * sy) / denom;
    let intercept = (sy - slope * sx) / n;
    Ok(LinearFit { slope, intercept })
}

/// The published spot price one day earlier, else one week earlier.
fn persistence_at(key: QuarterStart, spot_prices: &QuarterMap) -> Option<f64> {
    spot_prices
        .get(&key.offset(-DAY_MS))
        .or_else(|| spot_prices.get(&key.offset(-WEEK_MS)))
        .copied()
}

#[derive(Debug, Default)]
struct DayAggregate {
    ridge_sum: f64,
    ridge_count: usize,
    persistence_sum: f64,
    persistence_count: usize,
}

/// The output of [`apply_within_day_shape`].
#[derive(Debug, Clone, PartialEq)]
pub struct ShapedPrediction {
    pub result: QuarterMap,
    /// Quarters with no 1-day or 7-day lag; they keep the flat daily level.
    pub shape_fallback_quarters: usize,
}

/// Replace each prediction with its UTC-day mean plus the persistence shape:
/// `day mean of predicted + (persistence(q) − day mean of persistence)`.
/// The persistence day mean covers only the quarters that have a lag.
#[must_use]
pub fn apply_within_day_shape(
    predicted: &QuarterMap,
    spot_prices: &QuarterMap,
) -> ShapedPrediction {
    let mut by_day: BTreeMap<i64, DayAggregate> = BTreeMap::new();
    for (&key, &value) in predicted {
        let day = by_day.entry(key.utc_day_start_ms()).or_default();
        day.ridge_sum += value;
        day.ridge_count += 1;
        if let Some(persistence) = persistence_at(key, spot_prices) {
            day.persistence_sum += persistence;
            day.persistence_count += 1;
        }
    }

    let mut result = QuarterMap::new();
    let mut shape_fallback_quarters = 0;
    for (&key, &value) in predicted {
        let Some(day) = by_day
            .get(&key.utc_day_start_ms())
            .filter(|day| day.ridge_count > 0)
        else {
            result.insert(key, value);
            continue;
        };
        let ridge_daily_mean = day.ridge_sum / count_f64(day.ridge_count);
        match persistence_at(key, spot_prices) {
            Some(persistence) if day.persistence_count > 0 => {
                let persistence_daily_mean = day.persistence_sum / count_f64(day.persistence_count);
                result.insert(
                    key,
                    ridge_daily_mean + (persistence - persistence_daily_mean),
                );
            }
            _ => {
                result.insert(key, ridge_daily_mean);
                shape_fallback_quarters += 1;
            }
        }
    }
    ShapedPrediction {
        result,
        shape_fallback_quarters,
    }
}

/// An inclusive output clamp for predicted spot c/kWh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SanityBound {
    pub min: f64,
    pub max: f64,
}

/// The output sanity clamp from the observed price extremes: the range
/// widened by [`SANITY_MARGIN_K`] spreads on each side, with a minimum of at
/// most `min(0, SANITY_MIN_FALLBACK_CENTS_KWH)`. It is not a price floor.
/// Non-finite prices are ignored; no finite price gives `None`.
#[must_use]
pub fn sanity_bound_from_history(history: &QuarterMap) -> Option<SanityBound> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for &price in history.values() {
        if !price.is_finite() {
            continue;
        }
        if price < lo {
            lo = price;
        }
        if price > hi {
            hi = price;
        }
    }
    if !lo.is_finite() || !hi.is_finite() {
        return None;
    }
    let spread = hi - lo;
    Some(SanityBound {
        min: js_min(
            0.0,
            js_min(lo - SANITY_MARGIN_K * spread, SANITY_MIN_FALLBACK_CENTS_KWH),
        ),
        max: hi + SANITY_MARGIN_K * spread,
    })
}

/// One quarter of the predicted series, in spot c/kWh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForecastSpotPoint {
    pub start: QuarterStart,
    /// Rounded to 0.001 c/kWh like `Math.round`; it can be `-0.0`.
    pub estimated_spot_cents_kwh: f64,
    /// Present only for a quarter with a real prediction and calibrated bands.
    pub band: Option<SpotBand>,
}

/// The output of [`build_predicted_series`].
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltSeries {
    pub series: Vec<ForecastSpotPoint>,
    pub filled_quarters: usize,
    pub zero_seeded_quarters: usize,
}

/// Exactly `num_quarters` chronological points from `series_start`. A missing
/// quarter takes the last predicted value. Leading missing quarters take the
/// first predicted value; with no prediction at all, they are zero.
#[must_use]
pub fn build_predicted_series(
    predicted: &QuarterMap,
    series_start: QuarterStart,
    num_quarters: usize,
) -> BuiltSeries {
    let keys: Vec<QuarterStart> = (0..num_quarters)
        .scan(series_start.as_millisecond(), |ms, _| {
            let key = QuarterStart::from_millisecond(*ms);
            *ms = ms.saturating_add(QUARTER_MS);
            Some(key)
        })
        .collect();
    let mut last_predicted = keys.iter().find_map(|key| predicted.get(key).copied());

    let mut series = Vec::with_capacity(keys.len());
    let mut filled_quarters = 0;
    let mut zero_seeded_quarters = 0;
    for start in keys {
        let estimate = if let Some(&value) = predicted.get(&start) {
            last_predicted = Some(value);
            round3(value)
        } else if let Some(value) = last_predicted {
            filled_quarters += 1;
            round3(value)
        } else {
            zero_seeded_quarters += 1;
            0.0
        };
        series.push(ForecastSpotPoint {
            start,
            estimated_spot_cents_kwh: estimate,
            band: None,
        });
    }
    BuiltSeries {
        series,
        filled_quarters,
        zero_seeded_quarters,
    }
}
