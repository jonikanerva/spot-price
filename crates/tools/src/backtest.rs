//! Rolling-origin backtest of the FI price forecast, keyed by issue time.
//! Pure. Each origin sees only what the route knew: prices through the last
//! published CET/CEST delivery day, Fingrid actuals strictly before the issue
//! time, and per target the freshest vintage issued strictly before it.
//! Prices and Fingrid data stay inside the window of the served forecast.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::str::FromStr;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use spot_price_domain::areas::Area;
use spot_price_domain::forecast::{
    CalibratedBands, DeriveOptions, FLOOR_HISTORY_DAYS, FORECAST_DAYS, ForecastInput,
    ForecastOptions, GridRecord, HorizonResidual, LatestPublishedRule, NeighborPrices,
    PrevDayBasis, QuarterMap, QuarterStart, UtcHour, build_artifact, build_forecast,
    derive_band_offsets, observed_coverage_of, sanity_bound_from_history, uncalibrated_bands,
};
use spot_price_domain::time::{next_calendar_day, start_of_local_day_utc};

use crate::data::{
    BacktestData, DATASET_CONSUMPTION_ACTUAL, DATASET_CONSUMPTION_FORECAST, DATASET_WIND_ACTUAL,
    DATASET_WIND_FORECAST, DatasetId, GridActual, Vintage,
};
use crate::jsfmt::{count_f64, js_sum};
use crate::metrics::{RankMode, median, percentile, precision_at_n, spearman};

const QUARTER_MS: i64 = 15 * 60 * 1000;
const HOUR_MS: i64 = 60 * 60 * 1000;
const DAY_MS: i64 = 24 * HOUR_MS;
/// FI day-ahead prices for the next day appear at about 12:00 UTC.
const PUBLICATION_HOUR_UTC: i64 = 12;
/// Nord Pool day-ahead delivery days run from 00:00 to 24:00 CET/CEST.
const DELIVERY_DAY_TIME_ZONE: &str = "Europe/Oslo";

/// Labels of forecast days 0, 1, and 2 after the series start.
pub const HORIZON_LABELS: [&str; 3] = ["d+1", "d+2", "d+3"];

/// precision@N window: 16 quarters, a 4-hour flexible-load block.
pub const PRECISION_N_QUARTERS: usize = 16;

/// Minimum scored-origin span (days) before a calibrated band may ship.
pub const MIN_CALIBRATION_WINDOW_DAYS: f64 = 90.0;

/// The forecast datasets, in the order the ladder diagnostic builds them.
const FORECAST_DATASET_IDS: [DatasetId; 2] = [DATASET_WIND_FORECAST, DATASET_CONSUMPTION_FORECAST];

/// A forecast instant outside the range of `jiff::Timestamp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("backtest instant out of range")]
pub struct OutOfRange;

fn instant(ms: i64) -> Result<Timestamp, OutOfRange> {
    Timestamp::from_millisecond(ms).map_err(|_| OutOfRange)
}

/// A backtest run that cannot continue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BacktestError {
    #[error(transparent)]
    OutOfRange(#[from] OutOfRange),
    /// The bundled time zone database has no [`DELIVERY_DAY_TIME_ZONE`].
    #[error("time zone {DELIVERY_DAY_TIME_ZONE} is not in the bundled database")]
    DeliveryDayTimeZone,
}

/// How a reconstructed issue time selects forecast vintages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsOfMode {
    /// Per target, the freshest vintage issued strictly before the issue
    /// time. With the hour-truncated `issued_at`, this can be up to about one
    /// hour staler than production: a conservative result.
    IssueTime,
    /// Per target, the freshest vintage at any time. Leaky by design; only
    /// for the optimism comparison.
    Latest,
}

/// Per target, the freshest vintage issued strictly before `as_of_ms`
/// (`None`: no bound). Targets keep the order of their first admissible
/// vintage in the input.
#[must_use]
pub fn select_vintages_as_of(vintages: &[Vintage], as_of_ms: Option<i64>) -> Vec<Vintage> {
    let mut selected: Vec<Vintage> = Vec::new();
    let mut index_by_target: HashMap<i64, usize> = HashMap::new();
    for v in vintages {
        let issued_ms = v.issued_at.as_millisecond();
        if as_of_ms.is_some_and(|bound| issued_ms >= bound) {
            continue;
        }
        let target = v.start.as_millisecond();
        if let Some(existing) = index_by_target
            .get(&target)
            .and_then(|&i| selected.get_mut(i))
        {
            if issued_ms > existing.issued_at.as_millisecond() {
                *existing = *v;
            }
        } else {
            index_by_target.insert(target, selected.len());
            selected.push(*v);
        }
    }
    selected
}

/// The inputs knowable at issue time `t_ms`. The model run and the leakage
/// guard both read this one value.
#[derive(Debug, Clone, PartialEq)]
pub struct IssueTimeInputs {
    pub t_ms: i64,
    /// The last published FI delivery start (UTC ms).
    pub last_published_ms: Option<i64>,
    /// One quarter after the last published delivery, or `t_ms`: the series
    /// start of the served forecast. Price lags must be strictly before it;
    /// published day-ahead prices can be later than the wall clock `t_ms`.
    pub origin_ms: i64,
    /// FI spot (c/kWh) in the served window `[history_start, origin_ms)`.
    pub fi_prices: QuarterMap,
    /// Neighbour spot (c/kWh) by area code, in the same window as `fi_prices`.
    pub neighbor_prices: BTreeMap<String, QuarterMap>,
    /// One selected vintage per target and forecast dataset, for targets in
    /// the served window `[history_start, series_end)`.
    pub forecast: BTreeMap<DatasetId, Vec<Vintage>>,
    /// Fingrid actuals in `[history_start, min(t_ms, series_end))`.
    pub actual: BTreeMap<DatasetId, Vec<GridActual>>,
}

/// The Fingrid read window of the served forecast for a series that starts
/// at `origin_ms`: `(history_start, series_end)` in UTC ms, the rule of the
/// domain `ForecastWindow`.
fn served_window_ms(origin_ms: i64) -> (i64, i64) {
    (
        origin_ms.saturating_sub(FLOOR_HISTORY_DAYS * DAY_MS),
        origin_ms.saturating_add(FORECAST_DAYS * DAY_MS),
    )
}

fn delivery_zone() -> Result<TimeZone, BacktestError> {
    TimeZone::get(DELIVERY_DAY_TIME_ZONE).map_err(|_| BacktestError::DeliveryDayTimeZone)
}

/// Prices delivered before this instant are published at issue time `t_ms`.
/// Delivery day D is published at [`PUBLICATION_HOUR_UTC`] on the UTC date
/// D − 1, and it ends at 24:00 in [`DELIVERY_DAY_TIME_ZONE`].
fn publication_horizon_ms(t_ms: i64) -> Result<i64, BacktestError> {
    let utc = instant(t_ms)?.to_zoned(TimeZone::UTC);
    let published_day = if i64::from(utc.hour()) >= PUBLICATION_HOUR_UTC {
        next_calendar_day(utc.date()).map_err(|_| OutOfRange)?
    } else {
        utc.date()
    };
    let zone = delivery_zone()?;
    let next_day = next_calendar_day(published_day).map_err(|_| OutOfRange)?;
    let end = start_of_local_day_utc(next_day, &zone).map_err(|_| OutOfRange)?;
    Ok(end.as_millisecond())
}

/// Builds the issue-time inputs.
///
/// # Errors
/// [`BacktestError`] when the publication horizon of `t_ms` leaves the Jiff
/// range or the delivery-day time zone is missing.
pub fn reconstruct_issue_time(
    data: &BacktestData,
    t_ms: i64,
    mode: AsOfMode,
) -> Result<IssueTimeInputs, BacktestError> {
    let horizon = publication_horizon_ms(t_ms)?;
    let last_published_ms = data
        .prices
        .iter()
        .filter(|p| p.area == "FI")
        .map(|p| p.start.as_millisecond())
        .filter(|start_ms| *start_ms < horizon)
        .max();
    let origin_ms = last_published_ms.map_or(t_ms, |last| last + QUARTER_MS);
    let (history_start_ms, series_end_ms) = served_window_ms(origin_ms);

    let mut fi_prices = QuarterMap::new();
    let mut neighbor_prices: BTreeMap<String, QuarterMap> = BTreeMap::new();
    for p in &data.prices {
        let start_ms = p.start.as_millisecond();
        if !(history_start_ms..origin_ms).contains(&start_ms) {
            continue;
        }
        let key = QuarterStart::from_millisecond(start_ms);
        if p.area == "FI" {
            fi_prices.insert(key, p.spot_cents_kwh);
        } else {
            neighbor_prices
                .entry(p.area.clone())
                .or_default()
                .insert(key, p.spot_cents_kwh);
        }
    }

    let as_of_ms = match mode {
        AsOfMode::IssueTime => Some(t_ms),
        AsOfMode::Latest => None,
    };
    let forecast = data
        .vintages
        .iter()
        .map(|(id, vintages)| {
            let mut selected = select_vintages_as_of(vintages, as_of_ms);
            selected
                .retain(|v| (history_start_ms..series_end_ms).contains(&v.start.as_millisecond()));
            (*id, selected)
        })
        .collect();
    let actual_end_ms = t_ms.min(series_end_ms);
    let actual = data
        .actuals
        .iter()
        .map(|(id, records)| {
            let known = records
                .iter()
                .filter(|r| (history_start_ms..actual_end_ms).contains(&r.start.as_millisecond()))
                .copied()
                .collect();
            (*id, known)
        })
        .collect();

    Ok(IssueTimeInputs {
        t_ms,
        last_published_ms,
        origin_ms,
        fi_prices,
        neighbor_prices,
        forecast,
        actual,
    })
}

/// One input timestamp and the boundary it must respect. A `None` boundary
/// has no upper bound; only the Fingrid forecast targets look forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputTimestamp {
    pub source: String,
    pub ms: i64,
    pub boundary_ms: Option<i64>,
    pub forward_looking_allowed: bool,
}

/// Every input timestamp of the issue-time set. Price lags are bounded by
/// `origin_ms`; actuals and vintage issuances are bounded by `t_ms`.
#[must_use]
pub fn collect_input_timestamps(inputs: &IssueTimeInputs) -> Vec<InputTimestamp> {
    let bounded = |source: String, ms: i64, boundary: i64| InputTimestamp {
        source,
        ms,
        boundary_ms: Some(boundary),
        forward_looking_allowed: false,
    };
    let mut out = Vec::new();
    for key in inputs.fi_prices.keys() {
        out.push(bounded(
            "fi_price".to_owned(),
            key.as_millisecond(),
            inputs.origin_ms,
        ));
    }
    for (area, series) in &inputs.neighbor_prices {
        for key in series.keys() {
            out.push(bounded(
                format!("neighbor_{area}"),
                key.as_millisecond(),
                inputs.origin_ms,
            ));
        }
    }
    for (dataset, records) in &inputs.actual {
        for r in records {
            out.push(bounded(
                format!("fingrid_actual_{dataset}"),
                r.start.as_millisecond(),
                inputs.t_ms,
            ));
        }
    }
    for (dataset, records) in &inputs.forecast {
        for r in records {
            out.push(InputTimestamp {
                source: format!("fingrid_forecast_{dataset}"),
                ms: r.start.as_millisecond(),
                boundary_ms: None,
                forward_looking_allowed: true,
            });
            // A target may be in the future, but its issuance must not be.
            out.push(bounded(
                format!("fingrid_forecast_vintage_{dataset}"),
                r.issued_at.as_millisecond(),
                inputs.t_ms,
            ));
        }
    }
    out
}

/// The inputs that leak future information. Empty means no leakage.
#[must_use]
pub fn find_leaking_inputs(inputs: &IssueTimeInputs) -> Vec<InputTimestamp> {
    collect_input_timestamps(inputs)
        .into_iter()
        .filter(|t| !t.forward_looking_allowed && t.boundary_ms.is_some_and(|b| t.ms >= b))
        .collect()
}

/// The forecast of one issue time.
#[derive(Debug, Clone, PartialEq)]
pub struct IssueForecast {
    /// Predicted spot (c/kWh) per quarter, in chronological order.
    pub predicted: Vec<(QuarterStart, f64)>,
    pub series_start_ms: i64,
    pub series_end_ms: i64,
    pub used_fallback: bool,
    /// Quarters that the sanity bound clamped.
    pub sanity_clamped_quarters: usize,
}

fn grid_records<'a, I>(records: I) -> Vec<GridRecord>
where
    I: IntoIterator<Item = (Timestamp, f64)> + 'a,
{
    records
        .into_iter()
        .map(|(start, value)| GridRecord { start, value })
        .collect()
}

fn forecast_records(inputs: &IssueTimeInputs, id: DatasetId) -> Vec<GridRecord> {
    grid_records(
        inputs
            .forecast
            .get(&id)
            .into_iter()
            .flatten()
            .map(|v| (v.start, v.value)),
    )
}

fn actual_records(inputs: &IssueTimeInputs, id: DatasetId) -> Vec<GridRecord> {
    grid_records(
        inputs
            .actual
            .get(&id)
            .into_iter()
            .flatten()
            .map(|r| (r.start, r.value)),
    )
}

/// The day basis of the previous-day price statistics in a [`ForecastVariant`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DayBasis {
    /// The UTC day, [`PrevDayBasis::UtcDay`].
    #[default]
    Utc,
    /// The delivery day in [`DELIVERY_DAY_TIME_ZONE`].
    Delivery,
}

/// The forecast options that a backtest can vary. The default is the
/// production model. Every variant gets the sanity bound of the served
/// forecast, which can clamp a point prediction. The bands stay at their
/// [`ForecastOptions`] default; they do not change a point prediction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForecastVariant {
    pub ridge_lambda: f64,
    /// Not checked here. Weeks above
    /// [`MAX_EXTENSION_WEEKS`](crate::backtest_cli::MAX_EXTENSION_WEEKS) look
    /// back only before the served Fingrid window, so they equal that cap;
    /// the `--ab` parser rejects them so that arm B is not arm A relabelled.
    pub wind_extension_weeks: u32,
    /// Not checked here; see `wind_extension_weeks`.
    pub consumption_extension_weeks: u32,
    pub latest_published: LatestPublishedRule,
    pub prev_day_basis: DayBasis,
}

impl Default for ForecastVariant {
    fn default() -> Self {
        let production = ForecastOptions::default();
        Self {
            ridge_lambda: production.ridge_lambda,
            wind_extension_weeks: production.wind_extension_weeks,
            consumption_extension_weeks: production.consumption_extension_weeks,
            latest_published: production.latest_published,
            prev_day_basis: match production.prev_day_basis {
                PrevDayBasis::UtcDay => DayBasis::Utc,
                PrevDayBasis::DeliveryDay(_) => DayBasis::Delivery,
            },
        }
    }
}

impl ForecastVariant {
    /// The pipeline options of this variant. `delivery_zone` must be the
    /// [`DELIVERY_DAY_TIME_ZONE`] zone; only [`DayBasis::Delivery`] reads it.
    /// `fi_prices` must be the FI prices that the forecast gets: the served
    /// forecast takes its sanity bound from them.
    #[must_use]
    pub fn forecast_options<'a>(
        self,
        delivery_zone: &'a TimeZone,
        fi_prices: &QuarterMap,
    ) -> ForecastOptions<'a> {
        ForecastOptions {
            ridge_lambda: self.ridge_lambda,
            wind_extension_weeks: self.wind_extension_weeks,
            consumption_extension_weeks: self.consumption_extension_weeks,
            latest_published: self.latest_published,
            prev_day_basis: match self.prev_day_basis {
                DayBasis::Utc => PrevDayBasis::UtcDay,
                DayBasis::Delivery => PrevDayBasis::DeliveryDay(delivery_zone),
            },
            sanity_bound: sanity_bound_from_history(fi_prices),
            ..ForecastOptions::default()
        }
    }
}

/// Runs the production forecast with the issue-time inputs and the default
/// options.
///
/// # Errors
/// [`BacktestError`] when the series window leaves the `Timestamp` range or
/// the delivery-day time zone is missing.
pub fn forecast_at_issue_time(inputs: &IssueTimeInputs) -> Result<IssueForecast, BacktestError> {
    forecast_at_issue_time_with(inputs, ForecastVariant::default())
}

/// Runs the forecast of `variant` with the issue-time inputs.
///
/// # Errors
/// [`BacktestError`] when the series window leaves the `Timestamp` range or
/// the delivery-day time zone is missing.
pub fn forecast_at_issue_time_with(
    inputs: &IssueTimeInputs,
    variant: ForecastVariant,
) -> Result<IssueForecast, BacktestError> {
    let series_start_ms = inputs.origin_ms;
    let (_, series_end_ms) = served_window_ms(series_start_ms);
    // The forecast reads only the neighbour areas it knows by exact code.
    let neighbor_prices: NeighborPrices = inputs
        .neighbor_prices
        .iter()
        .filter_map(|(code, series)| Area::from_str(code).ok().map(|a| (a, series.clone())))
        .collect();
    let wind_forecast = forecast_records(inputs, DATASET_WIND_FORECAST);
    let wind_actual = actual_records(inputs, DATASET_WIND_ACTUAL);
    let consumption_forecast = forecast_records(inputs, DATASET_CONSUMPTION_FORECAST);
    let consumption_actual = actual_records(inputs, DATASET_CONSUMPTION_ACTUAL);
    let result = build_forecast(
        &ForecastInput {
            spot_prices: &inputs.fi_prices,
            neighbor_prices: &neighbor_prices,
            wind_forecast: &wind_forecast,
            wind_actual: &wind_actual,
            consumption_forecast: &consumption_forecast,
            consumption_actual: &consumption_actual,
            series_start: instant(series_start_ms)?,
            series_end: instant(series_end_ms)?,
        },
        &variant.forecast_options(&delivery_zone()?, &inputs.fi_prices),
    );
    Ok(IssueForecast {
        predicted: result
            .series
            .iter()
            .map(|p| (p.start, p.estimated_spot_cents_kwh))
            .collect(),
        series_start_ms,
        series_end_ms,
        used_fallback: result.diagnostics.fit_used_default,
        sanity_clamped_quarters: result.diagnostics.sanity_clamped_quarters,
    })
}

/// Mean absolute error of `(predicted, actual)` pairs; `None` when empty.
#[must_use]
pub fn mae(pairs: &[(f64, f64)]) -> Option<f64> {
    if pairs.is_empty() {
        return None;
    }
    Some(js_sum(pairs.iter().map(|(p, a)| (p - a).abs())) / count_f64(pairs.len()))
}

/// Symmetric MAPE in `[0, 2]`. A pair with `|p| + |a|` below 1e-9 is skipped.
#[must_use]
pub fn smape(pairs: &[(f64, f64)]) -> Option<f64> {
    let mut sum = 0.0;
    let mut n = 0_usize;
    for (p, a) in pairs {
        let denom = p.abs() + a.abs();
        if denom < 1e-9 {
            continue;
        }
        sum += (p - a).abs() / denom;
        n += 1;
    }
    (n > 0).then(|| (2.0 * sum) / count_f64(n))
}

/// Model MAE over baseline MAE; below 1 the model wins. `None` when either
/// is missing or the baseline is 0.
#[must_use]
pub fn rmae(model_mae: Option<f64>, baseline_mae: Option<f64>) -> Option<f64> {
    match (model_mae, baseline_mae) {
        (Some(model), Some(baseline)) if baseline != 0.0 => Some(model / baseline),
        _ => None,
    }
}

/// A naive baseline that is knowable at issue time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Baseline {
    /// The same quarter one week earlier.
    LastWeek,
    /// The same quarter in the 24 hours before the series start, the last
    /// published delivery day.
    LastPublishedDay,
}

/// A value per baseline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerBaseline<T> {
    pub last_week: T,
    pub last_published_day: T,
}

fn baseline_for(baseline: Baseline, target_ms: i64, inputs: &IssueTimeInputs) -> Option<f64> {
    let key_ms = match baseline {
        Baseline::LastWeek => target_ms - 7 * DAY_MS,
        Baseline::LastPublishedDay => {
            let series_start = inputs.last_published_ms? + QUARTER_MS;
            series_start - DAY_MS + (target_ms - series_start).rem_euclid(DAY_MS)
        }
    };
    inputs
        .fi_prices
        .get(&QuarterStart::from_millisecond(key_ms))
        .copied()
}

/// The metrics of one origin in one horizon. A rank metric is `None` with
/// fewer than two scored quarters or a constant series (Spearman), or a
/// constant prediction (precision@N).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OriginMetrics {
    /// Mean absolute error (c/kWh) over the pairs of this origin.
    pub mae: Option<f64>,
    pub spearman: Option<f64>,
    pub precision_at_n_cheap: Option<f64>,
    pub precision_at_n_peak: Option<f64>,
}

/// Metrics of one horizon. Rank metrics are per origin, then averaged.
#[derive(Debug, Clone, PartialEq)]
pub struct HorizonMetrics {
    pub mae: Option<f64>,
    pub smape: Option<f64>,
    pub spearman: Option<f64>,
    pub precision_at_n_cheap: Option<f64>,
    pub precision_at_n_peak: Option<f64>,
    pub band_coverage: Option<f64>,
    pub pairs: usize,
    /// The metrics of each origin with a scored pair in this horizon, keyed
    /// by the issue time (UTC ms).
    pub by_origin: BTreeMap<i64, OriginMetrics>,
}

/// The result of one backtest run.
#[derive(Debug, Clone, PartialEq)]
pub struct BacktestSummary {
    pub origins: usize,
    pub fallback_origins: usize,
    /// Origins skipped because no wind forecast vintage was knowable.
    pub pre_vintage_origins: usize,
    /// Quarters that the sanity bound clamped, summed over the scored
    /// origins. A quarter without a realised price also counts.
    pub sanity_clamped_quarters: usize,
    pub scored_window_start_ms: Option<i64>,
    pub scored_window_end_ms: Option<i64>,
    /// 0 with fewer than two scored origins.
    pub scored_window_span_days: f64,
    pub scored_origins_ms: BTreeSet<i64>,
    pub model_mae: Option<f64>,
    pub model_smape: Option<f64>,
    pub baseline_mae: PerBaseline<Option<f64>>,
    pub r_mae: PerBaseline<Option<f64>>,
    /// True when every origin was leakage-free.
    pub leak_free: bool,
    /// Indexed like [`HORIZON_LABELS`].
    pub by_horizon: [HorizonMetrics; 3],
    /// Out-of-sample residuals of the scored pairs, the band input.
    pub residuals: Vec<HorizonResidual>,
}

/// Options of [`run_backtest`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BacktestOptions {
    pub as_of_mode: Option<AsOfMode>,
    /// When set, only these origins (UTC ms) are scored, and the pre-vintage
    /// skip does not apply.
    pub restrict_origins_ms: Option<BTreeSet<i64>>,
    pub variant: ForecastVariant,
}

#[derive(Debug, Default)]
struct OriginSeries {
    t_ms: i64,
    preds: Vec<f64>,
    acts: Vec<f64>,
}

#[derive(Debug, Default)]
struct HorizonAcc {
    pairs: Vec<(f64, f64)>,
    residuals: Vec<HorizonResidual>,
    origin_series: Vec<OriginSeries>,
}

fn horizon_of(target_ms: i64, origin_ms: i64) -> Option<usize> {
    let index = (target_ms - origin_ms).div_euclid(DAY_MS);
    usize::try_from(index)
        .ok()
        .filter(|i| *i < HORIZON_LABELS.len())
}

/// The pairs and counters of the scored origins.
#[derive(Debug, Default)]
struct Scoring {
    model_pairs: Vec<(f64, f64)>,
    residuals: Vec<HorizonResidual>,
    last_week_pairs: Vec<(f64, f64)>,
    last_published_day_pairs: Vec<(f64, f64)>,
    horizon_acc: [HorizonAcc; 3],
    origins: usize,
    fallback_origins: usize,
    sanity_clamped_quarters: usize,
    scored_origins_ms: BTreeSet<i64>,
}

impl Scoring {
    /// Scores the forecast of one origin against the realised prices. An
    /// origin without any realised price is not counted.
    fn score_origin(
        &mut self,
        inputs: &IssueTimeInputs,
        fc: &IssueForecast,
        realized: &BTreeMap<QuarterStart, f64>,
    ) {
        let mut origin_by_horizon: [OriginSeries; 3] = std::array::from_fn(|_| OriginSeries {
            t_ms: inputs.t_ms,
            ..OriginSeries::default()
        });
        let mut scored = false;
        for &(key, pred) in &fc.predicted {
            let Some(&actual) = realized.get(&key) else {
                continue;
            };
            scored = true;
            self.model_pairs.push((pred, actual));
            let residual = HorizonResidual {
                utc_hour: UtcHour::of(key),
                predicted_raw: pred,
                actual_raw: actual,
            };
            self.residuals.push(residual);
            let target_ms = key.as_millisecond();
            if let Some(h) = horizon_of(target_ms, fc.series_start_ms)
                && let (Some(acc), Some(series)) =
                    (self.horizon_acc.get_mut(h), origin_by_horizon.get_mut(h))
            {
                acc.pairs.push((pred, actual));
                acc.residuals.push(residual);
                series.preds.push(pred);
                series.acts.push(actual);
            }
            if let Some(b) = baseline_for(Baseline::LastWeek, target_ms, inputs) {
                self.last_week_pairs.push((b, actual));
            }
            if let Some(b) = baseline_for(Baseline::LastPublishedDay, target_ms, inputs) {
                self.last_published_day_pairs.push((b, actual));
            }
        }
        if scored {
            self.origins += 1;
            self.scored_origins_ms.insert(inputs.t_ms);
            if fc.used_fallback {
                self.fallback_origins += 1;
            }
            self.sanity_clamped_quarters += fc.sanity_clamped_quarters;
            for (acc, series) in self.horizon_acc.iter_mut().zip(origin_by_horizon) {
                if !series.preds.is_empty() {
                    acc.origin_series.push(series);
                }
            }
        }
    }

    fn into_summary(self, pre_vintage_origins: usize, leak_free: bool) -> BacktestSummary {
        let model_mae = mae(&self.model_pairs);
        let baseline_mae = PerBaseline {
            last_week: mae(&self.last_week_pairs),
            last_published_day: mae(&self.last_published_day_pairs),
        };
        let first = self.scored_origins_ms.first().copied();
        let last = self.scored_origins_ms.last().copied();
        #[expect(clippy::cast_precision_loss, reason = "a span in ms is far below 2^53")]
        let scored_window_span_days = match (first, last) {
            (Some(first), Some(last)) => (last - first) as f64 / DAY_MS as f64,
            _ => 0.0,
        };
        BacktestSummary {
            origins: self.origins,
            fallback_origins: self.fallback_origins,
            pre_vintage_origins,
            sanity_clamped_quarters: self.sanity_clamped_quarters,
            scored_window_start_ms: first,
            scored_window_end_ms: last,
            scored_window_span_days,
            scored_origins_ms: self.scored_origins_ms,
            model_mae,
            model_smape: smape(&self.model_pairs),
            baseline_mae,
            r_mae: PerBaseline {
                last_week: rmae(model_mae, baseline_mae.last_week),
                last_published_day: rmae(model_mae, baseline_mae.last_published_day),
            },
            leak_free,
            by_horizon: self.horizon_acc.map(|acc| horizon_metrics(&acc)),
            residuals: self.residuals,
        }
    }
}

/// Scores one issue origin per day at 13:00 UTC from the first to the last
/// FI price day, and aggregates.
///
/// # Errors
/// [`BacktestError`] when a forecast window or a publication horizon leaves
/// the `Timestamp` range, or the delivery-day time zone is missing.
pub fn run_backtest(
    data: &BacktestData,
    options: &BacktestOptions,
) -> Result<BacktestSummary, BacktestError> {
    let mode = options.as_of_mode.unwrap_or(AsOfMode::IssueTime);
    let mut realized: BTreeMap<QuarterStart, f64> = BTreeMap::new();
    let mut range: Option<(i64, i64)> = None;
    for p in data.prices.iter().filter(|p| p.area == "FI") {
        let start_ms = p.start.as_millisecond();
        realized.insert(QuarterStart::from_millisecond(start_ms), p.spot_cents_kwh);
        range = Some(range.map_or((start_ms, start_ms), |(lo, hi)| {
            (lo.min(start_ms), hi.max(start_ms))
        }));
    }

    let mut scoring = Scoring::default();
    let mut pre_vintage_origins = 0;
    let mut leak_free = true;
    let wind_vintages = data
        .vintages
        .get(&DATASET_WIND_FORECAST)
        .map_or(&[][..], Vec::as_slice);

    if let Some((lo, hi)) = range {
        let mut day = lo.div_euclid(DAY_MS) * DAY_MS;
        while day <= hi {
            let t_ms = day + PUBLICATION_HOUR_UTC * HOUR_MS + HOUR_MS;
            day += DAY_MS;

            if let Some(restrict) = &options.restrict_origins_ms {
                if !restrict.contains(&t_ms) {
                    continue;
                }
            } else if select_vintages_as_of(wind_vintages, Some(t_ms)).is_empty() {
                pre_vintage_origins += 1;
                continue;
            }

            let inputs = reconstruct_issue_time(data, t_ms, mode)?;
            if !find_leaking_inputs(&inputs).is_empty() {
                leak_free = false;
            }
            let fc = forecast_at_issue_time_with(&inputs, options.variant)?;
            scoring.score_origin(&inputs, &fc, &realized);
        }
    }
    Ok(scoring.into_summary(pre_vintage_origins, leak_free))
}

/// The mean of the present values; `None` when there are none.
fn mean_of_present(xs: &[Option<f64>]) -> Option<f64> {
    let present: Vec<f64> = xs.iter().flatten().copied().collect();
    if present.is_empty() {
        return None;
    }
    Some(js_sum(present.iter().copied()) / count_f64(present.len()))
}

/// The band coverage of `residuals` against bands derived from themselves;
/// `None` for no residuals.
fn self_coverage(residuals: &[HorizonResidual]) -> Option<f64> {
    if residuals.is_empty() {
        return None;
    }
    let derived = derive_band_offsets(residuals, &DeriveOptions::default());
    Some(observed_coverage_of(
        residuals,
        &derived.offsets_by_hour,
        derived.global_offsets,
    ))
}

fn origin_metrics(s: &OriginSeries) -> OriginMetrics {
    let pairs: Vec<(f64, f64)> = s
        .preds
        .iter()
        .copied()
        .zip(s.acts.iter().copied())
        .collect();
    if s.preds.len() < 2 {
        return OriginMetrics {
            mae: mae(&pairs),
            ..OriginMetrics::default()
        };
    }
    // The two series have equal lengths of at least 2, and N > 0, so the
    // metrics cannot return an error.
    let precision = |mode| {
        precision_at_n(&s.preds, &s.acts, PRECISION_N_QUARTERS, mode)
            .ok()
            .flatten()
    };
    OriginMetrics {
        mae: mae(&pairs),
        spearman: spearman(&s.preds, &s.acts).ok().flatten(),
        precision_at_n_cheap: precision(RankMode::Cheap),
        precision_at_n_peak: precision(RankMode::Peak),
    }
}

fn horizon_metrics(acc: &HorizonAcc) -> HorizonMetrics {
    let by_origin: BTreeMap<i64, OriginMetrics> = acc
        .origin_series
        .iter()
        .map(|s| (s.t_ms, origin_metrics(s)))
        .collect();
    let mean_of = |metric: fn(&OriginMetrics) -> Option<f64>| {
        mean_of_present(&by_origin.values().map(metric).collect::<Vec<_>>())
    };
    HorizonMetrics {
        mae: mae(&acc.pairs),
        smape: smape(&acc.pairs),
        spearman: mean_of(|r| r.spearman),
        precision_at_n_cheap: mean_of(|r| r.precision_at_n_cheap),
        precision_at_n_peak: mean_of(|r| r.precision_at_n_peak),
        band_coverage: self_coverage(&acc.residuals),
        pairs: acc.pairs.len(),
        by_origin,
    }
}

/// The backtest and the band artifact from its residuals.
#[derive(Debug, Clone, PartialEq)]
pub struct DerivedBands {
    pub summary: BacktestSummary,
    pub bands: CalibratedBands,
    /// The measured coverage, also when the shipped artifact is dark and its
    /// own `observed_coverage` is `None`.
    pub observed_coverage: Option<f64>,
}

/// Runs the backtest and builds the band artifact. The artifact is dark
/// when the coverage gate fails or the scored window is shorter than
/// [`MIN_CALIBRATION_WINDOW_DAYS`].
///
/// # Errors
/// [`BacktestError`] when a forecast window or a publication horizon leaves
/// the `Timestamp` range, or the delivery-day time zone is missing.
pub fn derive_bands_from_backtest(
    data: &BacktestData,
    generated_at: String,
) -> Result<DerivedBands, BacktestError> {
    let summary = run_backtest(data, &BacktestOptions::default())?;
    let candidate = build_artifact(&summary.residuals, generated_at);
    let observed_coverage = candidate.observed_coverage;
    let bands = if summary.scored_window_span_days < MIN_CALIBRATION_WINDOW_DAYS {
        CalibratedBands {
            nominal_coverage: candidate.nominal_coverage,
            ..uncalibrated_bands()
        }
    } else {
        candidate
    };
    Ok(DerivedBands {
        summary,
        bands,
        observed_coverage,
    })
}

/// Deltas of one horizon, leaked − honest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HorizonDelta {
    pub delta_mae: Option<f64>,
    pub delta_smape: Option<f64>,
    pub delta_band_coverage: Option<f64>,
}

/// The vintage ladder depth of one forecast dataset. Thin ladders make the
/// as-of value fresher than a real gap allows, so the delta is understated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LadderDiagnostic {
    /// Median issuances per target.
    pub median_depth: Option<f64>,
    /// p90 issuances per target.
    pub expected_depth: Option<f64>,
    /// Fraction of targets with depth below half of `expected_depth`.
    pub sparse_target_fraction: Option<f64>,
}

/// The honest and the leaked run over the same origins.
#[derive(Debug, Clone, PartialEq)]
pub struct OptimismComparison {
    pub leaked: BacktestSummary,
    pub honest: BacktestSummary,
    pub scored_origins: usize,
    pub delta_mae: Option<f64>,
    pub delta_smape: Option<f64>,
    pub delta_band_coverage: Option<f64>,
    /// Indexed like [`HORIZON_LABELS`].
    pub delta_by_horizon: [HorizonDelta; 3],
    /// Ascending dataset id, the order of a JavaScript integer-keyed object.
    pub ladder_diagnostic: BTreeMap<DatasetId, LadderDiagnostic>,
}

fn sub(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    Some(a? - b?)
}

fn ladder_diagnostic_for(vintages: &[Vintage]) -> LadderDiagnostic {
    let mut depth_by_target: HashMap<i64, usize> = HashMap::new();
    for v in vintages {
        *depth_by_target.entry(v.start.as_millisecond()).or_insert(0) += 1;
    }
    let depths: Vec<f64> = depth_by_target.into_values().map(count_f64).collect();
    let expected_depth = percentile(&depths, 90.0);
    let sparse_target_fraction = expected_depth
        .filter(|_| !depths.is_empty())
        .map(|expected| {
            let sparse = depths.iter().filter(|d| **d < 0.5 * expected).count();
            count_f64(sparse) / count_f64(depths.len())
        });
    LadderDiagnostic {
        median_depth: median(&depths),
        expected_depth,
        sparse_target_fraction,
    }
}

/// Runs the backtest twice over the same origins: honest (issue-time
/// vintages) and leaked (latest vintages). Delta = leaked − honest.
///
/// # Errors
/// [`BacktestError`] when a forecast window or a publication horizon leaves
/// the `Timestamp` range, or the delivery-day time zone is missing.
pub fn compare_optimism(data: &BacktestData) -> Result<OptimismComparison, BacktestError> {
    let honest = run_backtest(
        data,
        &BacktestOptions {
            as_of_mode: Some(AsOfMode::IssueTime),
            ..BacktestOptions::default()
        },
    )?;
    let leaked = run_backtest(
        data,
        &BacktestOptions {
            as_of_mode: Some(AsOfMode::Latest),
            restrict_origins_ms: Some(honest.scored_origins_ms.clone()),
            ..BacktestOptions::default()
        },
    )?;
    let delta_by_horizon = std::array::from_fn(|h| HorizonDelta {
        delta_mae: sub(leaked.by_horizon[h].mae, honest.by_horizon[h].mae),
        delta_smape: sub(leaked.by_horizon[h].smape, honest.by_horizon[h].smape),
        delta_band_coverage: sub(
            leaked.by_horizon[h].band_coverage,
            honest.by_horizon[h].band_coverage,
        ),
    });
    let ladder_diagnostic = FORECAST_DATASET_IDS
        .iter()
        .map(|id| {
            let vintages = data.vintages.get(id).map_or(&[][..], Vec::as_slice);
            (*id, ladder_diagnostic_for(vintages))
        })
        .collect();
    Ok(OptimismComparison {
        scored_origins: honest.scored_origins_ms.len(),
        delta_mae: sub(leaked.model_mae, honest.model_mae),
        delta_smape: sub(leaked.model_smape, honest.model_smape),
        delta_band_coverage: sub(
            self_coverage(&leaked.residuals),
            self_coverage(&honest.residuals),
        ),
        delta_by_horizon,
        ladder_diagnostic,
        leaked,
        honest,
    })
}

/// Deltas of the arm means of one horizon, B − A. A positive rank delta
/// means B ranks better; a negative MAE delta means B is more accurate. The
/// rank means of the two arms can cover different origins; the paired
/// deltas are the comparison on the same origins.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HorizonVariantDelta {
    pub delta_spearman: Option<f64>,
    pub delta_precision_at_n_cheap: Option<f64>,
    pub delta_precision_at_n_peak: Option<f64>,
    pub delta_mae: Option<f64>,
}

/// Arm A and arm B scored on the same issue-time origins.
#[derive(Debug, Clone, PartialEq)]
pub struct VariantComparison {
    pub variant_a: ForecastVariant,
    pub variant_b: ForecastVariant,
    pub a: BacktestSummary,
    pub b: BacktestSummary,
    /// The origins that A scored. B runs only on these origins.
    pub shared_origins: usize,
    /// True when B scored exactly the origins of A, with equal pair counts
    /// per horizon. Otherwise the deltas do not compare the same targets and
    /// are not a verdict.
    pub origins_match: bool,
    /// Indexed like [`HORIZON_LABELS`].
    pub delta_by_horizon: [HorizonVariantDelta; 3],
    /// Origin-by-origin MAE and rank deltas. Indexed like [`HORIZON_LABELS`].
    pub paired_by_horizon: [HorizonPairedDelta; 3],
}

/// Which direction of a metric is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Better {
    /// Rank metrics: a win for B is a delta B − A above 0.
    Higher,
    /// MAE: a win for B (more accurate) is a delta B − A below 0.
    Lower,
}

/// One metric of arm B against arm A, origin by origin. A win is a delta
/// B − A in the [`Better`] direction, a loss is the other sign, and a tie is
/// an exactly equal value (delta == 0.0). An origin where only one arm has
/// a value, or where the delta is not finite, is excluded and counted. An
/// origin where neither arm has a value is not counted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PairedDelta {
    /// Origins where both arms have a value; `wins + losses + ties`.
    pub paired: usize,
    /// The mean delta B − A over the paired origins; `None` when none.
    pub mean_delta: Option<f64>,
    pub wins: usize,
    pub losses: usize,
    pub ties: usize,
    pub excluded: usize,
}

/// The paired deltas of one horizon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HorizonPairedDelta {
    /// Lower is better: a win is a negative delta (B more accurate).
    pub mae: PairedDelta,
    /// Rank metrics: higher is better, a win is a positive delta.
    pub spearman: PairedDelta,
    pub precision_at_n_cheap: PairedDelta,
    pub precision_at_n_peak: PairedDelta,
}

/// Pairs the per-origin values of one metric by origin key. See
/// [`PairedDelta`] for the counting rules.
#[must_use]
pub fn paired_delta(
    a: &BTreeMap<i64, Option<f64>>,
    b: &BTreeMap<i64, Option<f64>>,
    better: Better,
) -> PairedDelta {
    let origins: BTreeSet<i64> = a.keys().chain(b.keys()).copied().collect();
    let mut deltas = Vec::new();
    let (mut wins, mut losses, mut ties, mut excluded) = (0, 0, 0, 0);
    for origin in origins {
        let va = a.get(&origin).copied().flatten();
        let vb = b.get(&origin).copied().flatten();
        let (va, vb) = match (va, vb) {
            (Some(va), Some(vb)) => (va, vb),
            (None, None) => continue,
            _ => {
                excluded += 1;
                continue;
            }
        };
        let delta = vb - va;
        if !delta.is_finite() {
            excluded += 1;
            continue;
        }
        let gain = match better {
            Better::Higher => delta,
            Better::Lower => -delta,
        };
        if gain > 0.0 {
            wins += 1;
        } else if gain < 0.0 {
            losses += 1;
        } else {
            ties += 1;
        }
        deltas.push(delta);
    }
    PairedDelta {
        paired: deltas.len(),
        mean_delta: (!deltas.is_empty())
            .then(|| js_sum(deltas.iter().copied()) / count_f64(deltas.len())),
        wins,
        losses,
        ties,
        excluded,
    }
}

/// The paired deltas of arm `b` against arm `a` in one horizon, one
/// [`paired_delta`] per metric over [`HorizonMetrics::by_origin`].
#[must_use]
pub fn paired_horizon(a: &HorizonMetrics, b: &HorizonMetrics) -> HorizonPairedDelta {
    let pair = |metric: fn(&OriginMetrics) -> Option<f64>, better: Better| {
        let values = |m: &HorizonMetrics| -> BTreeMap<i64, Option<f64>> {
            m.by_origin.iter().map(|(t, r)| (*t, metric(r))).collect()
        };
        paired_delta(&values(a), &values(b), better)
    };
    HorizonPairedDelta {
        mae: pair(|r| r.mae, Better::Lower),
        spearman: pair(|r| r.spearman, Better::Higher),
        precision_at_n_cheap: pair(|r| r.precision_at_n_cheap, Better::Higher),
        precision_at_n_peak: pair(|r| r.precision_at_n_peak, Better::Higher),
    }
}

/// Runs the honest issue-time backtest for arm `a`, then for arm `b` on the
/// origins that `a` scored. Delta = B − A.
///
/// # Errors
/// [`BacktestError`] when a forecast window or a publication horizon leaves
/// the `Timestamp` range, or the delivery-day time zone is missing.
pub fn compare_variants(
    data: &BacktestData,
    a: ForecastVariant,
    b: ForecastVariant,
) -> Result<VariantComparison, BacktestError> {
    let run_a = run_backtest(
        data,
        &BacktestOptions {
            as_of_mode: Some(AsOfMode::IssueTime),
            restrict_origins_ms: None,
            variant: a,
        },
    )?;
    let run_b = run_backtest(
        data,
        &BacktestOptions {
            as_of_mode: Some(AsOfMode::IssueTime),
            restrict_origins_ms: Some(run_a.scored_origins_ms.clone()),
            variant: b,
        },
    )?;
    let delta_by_horizon = std::array::from_fn(|h| {
        let (ma, mb) = (&run_a.by_horizon[h], &run_b.by_horizon[h]);
        HorizonVariantDelta {
            delta_spearman: sub(mb.spearman, ma.spearman),
            delta_precision_at_n_cheap: sub(mb.precision_at_n_cheap, ma.precision_at_n_cheap),
            delta_precision_at_n_peak: sub(mb.precision_at_n_peak, ma.precision_at_n_peak),
            delta_mae: sub(mb.mae, ma.mae),
        }
    });
    let paired_by_horizon =
        std::array::from_fn(|h| paired_horizon(&run_a.by_horizon[h], &run_b.by_horizon[h]));
    Ok(VariantComparison {
        variant_a: a,
        variant_b: b,
        shared_origins: run_a.scored_origins_ms.len(),
        origins_match: run_a.scored_origins_ms == run_b.scored_origins_ms
            && run_a
                .by_horizon
                .iter()
                .zip(&run_b.by_horizon)
                .all(|(ma, mb)| ma.pairs == mb.pairs),
        delta_by_horizon,
        paired_by_horizon,
        a: run_a,
        b: run_b,
    })
}
