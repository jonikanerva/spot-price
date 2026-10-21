//! Series helpers of the forecast pipeline: bucketing, tail extension, the
//! within-day shape, the output sanity bound, and the predicted series.

use std::collections::{BTreeMap, HashMap};

use jiff::civil::Date;
use jiff::tz::{AmbiguousOffset, TimeZone};
use jiff::{Span, Timestamp};

use super::conformal::SpotBand;
use super::js::{count_f64, js_min};
use super::quarter::{DAY_MS, QUARTER_MS, QuarterMap, QuarterStart, WEEK_MS};
use crate::js::round3;
use crate::time::{next_calendar_day, start_of_local_day_utc};

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

/// The most weeks back that [`fill_gaps_from_earlier_weeks`] reads.
pub const GAP_FILL_LOOKBACK_WEEKS: u32 = 4;

/// Values for the quarters in `[first actual quarter, series_end_ms)` that
/// `known` lacks. A value is the mean of the same weekday and quarter of the
/// nearest `weeks` earlier weeks that `actual` holds, within
/// `max(weeks, GAP_FILL_LOOKBACK_WEEKS)` weeks back. For a quarter whose
/// first `weeks` weeks all hold a value, this equals
/// [`extend_with_last_week`]. A quarter without such a week stays missing.
#[must_use]
pub fn fill_gaps_from_earlier_weeks(
    known: &QuarterMap,
    actual: &QuarterMap,
    series_end_ms: i64,
    weeks: u32,
) -> QuarterMap {
    let mut out = QuarterMap::new();
    let Some(&first) = actual.keys().next() else {
        return out;
    };
    if weeks == 0 {
        return out;
    }
    let lookback = i64::from(weeks.max(GAP_FILL_LOOKBACK_WEEKS));
    let wanted = usize::try_from(weeks).unwrap_or(usize::MAX);
    let mut cursor = first.as_millisecond();
    while cursor < series_end_ms {
        let key = QuarterStart::from_millisecond(cursor);
        if !known.contains_key(&key) {
            let mut sum = 0.0;
            let mut found = 0_usize;
            for week in 1..=lookback {
                if found == wanted {
                    break;
                }
                let previous = QuarterStart::from_millisecond(cursor - week * WEEK_MS);
                if let Some(value) = actual.get(&previous) {
                    sum += value;
                    found += 1;
                }
            }
            if found > 0 {
                out.insert(key, sum / count_f64(found));
            }
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

/// How far back [`ShapeProfile::PreviousPublishedDay`] looks for a complete
/// local day.
pub const PREVIOUS_PUBLISHED_DAY_LOOKBACK_DAYS: i64 = 7;

/// The price profile that gives the within-day shape. The daily level is the
/// same for every profile; only the lagged price per quarter changes. The
/// served forecast uses [`ShapeProfile::WeekdayMean`]; see
/// [`ForecastOptions::served`](super::ForecastOptions::served).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeProfile<'a> {
    /// The price 24 h earlier, else 168 h earlier (fixed offsets).
    Persistence,
    /// The same local time on the latest earlier local day in the zone that
    /// has a price for every quarter.
    PreviousPublishedDay(&'a TimeZone),
    /// The mean of the same local time on the same weekday of each of the last
    /// `weeks` weeks. Every week must have a price; `weeks = 1` is the
    /// previous week's same weekday.
    WeekdayMean { zone: &'a TimeZone, weeks: u8 },
}

/// The profile prices of some quarters; see [`profile_values`].
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileValues {
    pub values: QuarterMap,
    /// Quarters with no profile price.
    pub missing: usize,
    /// Missing quarters whose local time does not exist on a source day: the
    /// skipped hour of a 23-hour day.
    pub dst_gap: usize,
}

/// Why a quarter has no source quarter on an earlier local day.
enum SourceMiss {
    /// The local time is in the skipped hour of the source day.
    Gap,
    /// The source date or instant is outside the Jiff range.
    OutOfRange,
}

/// The quarter at the same local wall-clock time `days_back` local days
/// before `key`. In the repeated hour of a 25-hour source day, it takes the
/// instant with the UTC offset of `key`, else the earlier one.
fn same_local_time_days_back(
    key: QuarterStart,
    zone: &TimeZone,
    days_back: i64,
) -> Result<QuarterStart, SourceMiss> {
    let zoned = key
        .timestamp()
        .map_err(|_| SourceMiss::OutOfRange)?
        .to_zoned(zone.clone());
    let date = Span::new()
        .try_days(days_back)
        .and_then(|span| zoned.date().checked_sub(span))
        .map_err(|_| SourceMiss::OutOfRange)?;
    let ambiguous = zone.to_ambiguous_timestamp(date.to_datetime(zoned.time()));
    let source = match ambiguous.offset() {
        AmbiguousOffset::Unambiguous { .. } => ambiguous.compatible(),
        AmbiguousOffset::Gap { .. } => return Err(SourceMiss::Gap),
        AmbiguousOffset::Fold { after, .. } if after == zoned.offset() => ambiguous.later(),
        AmbiguousOffset::Fold { .. } => ambiguous.earlier(),
    };
    source
        .map(QuarterStart::floor)
        .map_err(|_| SourceMiss::OutOfRange)
}

/// Whether `prices` has every quarter of the local day `date` in `zone`.
fn complete_local_day(date: Date, zone: &TimeZone, prices: &QuarterMap) -> bool {
    let bounds = start_of_local_day_utc(date, zone).and_then(|start| {
        next_calendar_day(date)
            .and_then(|next| start_of_local_day_utc(next, zone))
            .map(|end| (start.as_millisecond(), end.as_millisecond()))
    });
    let Ok((start, end)) = bounds else {
        return false;
    };
    let mut cursor = start;
    while cursor < end {
        if !prices.contains_key(&QuarterStart::from_millisecond(cursor)) {
            return false;
        }
        cursor += QUARTER_MS;
    }
    start < end
}

/// The profile price of each key. A key without one is counted, not filled.
#[must_use]
pub fn profile_values<I>(
    profile: ShapeProfile<'_>,
    keys: I,
    spot_prices: &QuarterMap,
) -> ProfileValues
where
    I: IntoIterator<Item = QuarterStart>,
{
    let mut out = ProfileValues {
        values: QuarterMap::new(),
        missing: 0,
        dst_gap: 0,
    };
    let mut complete_days: HashMap<Date, bool> = HashMap::new();
    for key in keys {
        let value = match profile {
            ShapeProfile::Persistence => Ok(persistence_at(key, spot_prices)),
            ShapeProfile::PreviousPublishedDay(zone) => {
                previous_published_day_at(key, zone, spot_prices, &mut complete_days)
            }
            ShapeProfile::WeekdayMean { zone, weeks } => {
                weekday_mean_at(key, zone, weeks, spot_prices)
            }
        };
        match value {
            Ok(Some(price)) => {
                out.values.insert(key, price);
            }
            Ok(None) | Err(SourceMiss::OutOfRange) => out.missing += 1,
            Err(SourceMiss::Gap) => {
                out.missing += 1;
                out.dst_gap += 1;
            }
        }
    }
    out
}

fn previous_published_day_at(
    key: QuarterStart,
    zone: &TimeZone,
    spot_prices: &QuarterMap,
    complete_days: &mut HashMap<Date, bool>,
) -> Result<Option<f64>, SourceMiss> {
    let target_date = key
        .timestamp()
        .map_err(|_| SourceMiss::OutOfRange)?
        .to_zoned(zone.clone())
        .date();
    for days_back in 1..=PREVIOUS_PUBLISHED_DAY_LOOKBACK_DAYS {
        let Ok(date) = Span::new()
            .try_days(days_back)
            .and_then(|span| target_date.checked_sub(span))
        else {
            return Err(SourceMiss::OutOfRange);
        };
        let complete = *complete_days
            .entry(date)
            .or_insert_with(|| complete_local_day(date, zone, spot_prices));
        if complete {
            let source = same_local_time_days_back(key, zone, days_back)?;
            return Ok(spot_prices.get(&source).copied());
        }
    }
    Ok(None)
}

fn weekday_mean_at(
    key: QuarterStart,
    zone: &TimeZone,
    weeks: u8,
    spot_prices: &QuarterMap,
) -> Result<Option<f64>, SourceMiss> {
    if weeks == 0 {
        return Ok(None);
    }
    let mut sum = 0.0;
    for week in 1..=i64::from(weeks) {
        let source = same_local_time_days_back(key, zone, 7 * week)?;
        let Some(price) = spot_prices.get(&source) else {
            return Ok(None);
        };
        sum += price;
    }
    Ok(Some(sum / f64::from(weeks)))
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
    /// Quarters with no profile price; they keep the flat daily level.
    pub shape_fallback_quarters: usize,
}

/// Replace each prediction with its UTC-day mean plus the shape of
/// [`ShapeProfile::Persistence`]:
/// `day mean of predicted + (persistence(q) − day mean of persistence)`.
/// The persistence day mean covers only the quarters that have a lag.
#[must_use]
pub fn apply_within_day_shape(
    predicted: &QuarterMap,
    spot_prices: &QuarterMap,
) -> ShapedPrediction {
    apply_within_day_shape_with(predicted, spot_prices, ShapeProfile::Persistence)
}

/// [`apply_within_day_shape`] with the profile price of `profile`. The
/// day mean of the prediction, the daily level, stays on the UTC day.
#[must_use]
pub fn apply_within_day_shape_with(
    predicted: &QuarterMap,
    spot_prices: &QuarterMap,
    profile: ShapeProfile<'_>,
) -> ShapedPrediction {
    let profile = profile_values(profile, predicted.keys().copied(), spot_prices).values;
    let mut by_day: BTreeMap<i64, DayAggregate> = BTreeMap::new();
    for (&key, &value) in predicted {
        let day = by_day.entry(key.utc_day_start_ms()).or_default();
        day.ridge_sum += value;
        day.ridge_count += 1;
        if let Some(&persistence) = profile.get(&key) {
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
        match profile.get(&key) {
            Some(&persistence) if day.persistence_count > 0 => {
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
