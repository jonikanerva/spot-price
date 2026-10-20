//! Issuance training of the FI forecast. A training example is one earlier
//! issuance and one target quarter of its forecast window. Its features use
//! only what the forecast had at that issuance: FI and neighbour prices
//! published by then, Fingrid forecast vintages issued strictly before it,
//! and Fingrid actuals that started strictly before it. A price lag that was
//! not available takes the fill of the served prediction, and an indicator
//! column marks it. An example without a wind or consumption value is left
//! out and counted, because a served prediction always has both. The
//! prediction rows use the same assembly.

use std::collections::BTreeMap;

use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::time::{next_calendar_day, start_of_local_day_utc};

use super::ForecastError;
use super::features::{
    FEATURE_COUNT, FEATURE_NAMES, FeatureContext, FeatureRules, NEIGHBOR_AREAS, NeighborPrices,
    PriceIndex, TRAINING_HISTORY_DAYS, build_feature_vector_indexed, build_price_index,
};
use super::model::{FitMeta, FitOptions, PriceTransform, TrainingData, fit_ridge};
use super::pipeline::{ForecastInput, ForecastOptions, GridSeries, grid_series};
use super::quarter::{DAY_MS, QUARTER_MS, QuarterMap, QuarterStart};
use super::route::{FLOOR_HISTORY_DAYS, FORECAST_DAYS, has_grid_data};
use super::series::GridRecord;

/// Day-ahead prices of delivery day D are published at this UTC hour on
/// the UTC date D − 1.
pub const PUBLICATION_HOUR_UTC: i64 = 12;

/// The end of the delivery days whose prices are published at `issued_at`.
/// Delivery day D is published at [`PUBLICATION_HOUR_UTC`] on the UTC date
/// D − 1, and it ends at 24:00 in `delivery_zone`. Stored prices have no
/// publication time, so this is a fixed rule, not an observation.
///
/// # Errors
/// [`ForecastError::OutOfRange`] when the end leaves the Jiff range.
pub fn publication_horizon(
    issued_at: Timestamp,
    delivery_zone: &TimeZone,
) -> Result<Timestamp, ForecastError> {
    let utc = issued_at.to_zoned(TimeZone::UTC);
    let published_day = if i64::from(utc.hour()) >= PUBLICATION_HOUR_UTC {
        next_calendar_day(utc.date()).map_err(|_| ForecastError::OutOfRange)?
    } else {
        utc.date()
    };
    let next_day = next_calendar_day(published_day).map_err(|_| ForecastError::OutOfRange)?;
    start_of_local_day_utc(next_day, delivery_zone).map_err(|_| ForecastError::OutOfRange)
}

/// One archived issuance of a Fingrid forecast value (MW). `issued_at` is
/// the hour-truncated fetch time of the issuance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridVintage {
    pub issued_at: Timestamp,
    pub start: Timestamp,
    pub value: f64,
}

/// The issuance history of one forecast run.
#[derive(Debug, Clone, Copy)]
pub struct IssuanceHistory<'a> {
    /// The issue time of this run. The training issuances are whole days
    /// before it, so they have the same time of day.
    pub issued_at: Timestamp,
    /// Every archived wind forecast (245) vintage issued before `issued_at`.
    pub wind_vintages: &'a [GridVintage],
    /// Every archived consumption forecast (165) vintage issued before
    /// `issued_at`.
    pub consumption_vintages: &'a [GridVintage],
    /// The zone of the Nord Pool delivery day, for the publication rule.
    pub delivery_zone: &'a TimeZone,
}

/// How the forecast builds its training rows.
#[derive(Debug, Clone, Copy, Default)]
pub enum TrainingRule<'a> {
    /// One row per published FI quarter of the training window, with the
    /// inputs that are known now: the served forecast.
    #[default]
    Realised,
    /// One row per earlier issuance and target quarter, with the inputs of
    /// that issuance, and the extra columns of
    /// [`ISSUANCE_EXTRA_FEATURE_NAMES`].
    Issuance(IssuanceHistory<'a>),
}

/// The columns that issuance training adds after [`FEATURE_NAMES`]. A
/// `*_missing` column is 1 when the lag took the neutral fill; a neighbour
/// column is the fraction of the neighbour areas without the lag.
pub const ISSUANCE_EXTRA_FEATURE_NAMES: [&str; 10] = [
    "lead_hours",
    "wind_x_lead",
    "consumption_x_lead",
    "wind_from_vintage",
    "consumption_from_vintage",
    "fi_1d_missing",
    "fi_7d_missing",
    "fi_prev_day_missing",
    "neighbor_1d_missing",
    "neighbor_7d_missing",
];

/// The number of feature columns of issuance training.
pub const ISSUANCE_FEATURE_COUNT: usize = FEATURE_COUNT + ISSUANCE_EXTRA_FEATURE_NAMES.len();

const WIND_COLUMN: usize = 1;
const CONSUMPTION_COLUMN: usize = 2;

/// The feature columns of issuance training in their fixed order.
#[must_use]
pub fn issuance_feature_names() -> Vec<&'static str> {
    FEATURE_NAMES
        .iter()
        .chain(&ISSUANCE_EXTRA_FEATURE_NAMES)
        .copied()
        .collect()
}

/// Where a wind or consumption value of an example comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputSource {
    /// A forecast vintage. `issued_at` is `None` only in a prediction row,
    /// where the caller selected the vintage.
    Vintage { issued_at: Option<Timestamp> },
    /// The tail extension from earlier actuals.
    Extension,
    /// An actual that started before the issuance.
    Actual,
    /// No value; the feature is 0.
    Missing,
}

/// Which price lags of an example come from published prices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LagAvailability {
    pub fi_1d: bool,
    pub fi_7d: bool,
    pub fi_prev_day: bool,
    /// Neighbour areas with a 1-day lag, of [`NEIGHBOR_AREAS`].
    pub neighbors_1d: usize,
    /// Neighbour areas with a 7-day lag, of [`NEIGHBOR_AREAS`].
    pub neighbors_7d: usize,
}

/// One training example: the inputs of one issuance for one target quarter.
#[derive(Debug, Clone, PartialEq)]
pub struct TrainingExample {
    pub issued_at: Timestamp,
    /// The first quarter that the issuance predicted.
    pub series_start: QuarterStart,
    pub target: QuarterStart,
    /// `target − issued_at` in milliseconds.
    pub lead_ms: i64,
    pub wind: InputSource,
    pub consumption: InputSource,
    pub lags: LagAvailability,
    /// The row in the order of [`issuance_feature_names`].
    pub features: Vec<f64>,
    /// The published FI price (c/kWh) of the target.
    pub realised: f64,
}

/// The inputs that one training issuance had.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IssuanceSummary {
    pub issued_at: Timestamp,
    pub series_start: QuarterStart,
    /// The first published FI quarter that the issuance had. The served
    /// window reaches [`FLOOR_HISTORY_DAYS`] back from its series start; an
    /// older issuance has only the part of that window inside the input.
    pub first_price: Option<QuarterStart>,
    pub price_quarters: usize,
    pub wind_vintage_targets: usize,
    pub consumption_vintage_targets: usize,
    /// False when the served route would answer that it has no grid data.
    pub has_grid_data: bool,
    pub examples: usize,
    /// Target quarters in the training window without a published price.
    pub targets_without_price: usize,
    /// Targets with a price that are not examples: no wind or consumption
    /// value.
    pub excluded: ExcludedExamples,
}

/// Targets left out because a wind or consumption value is missing. A
/// target with both values missing counts once, under `outside_input` when
/// either reason is that one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExcludedExamples {
    /// The issuance had no vintage of the dataset at all, and the tail
    /// extension has no forecast to extend: before the archive started.
    pub no_vintage: usize,
    /// The tail extension needs actuals from before the input: the input
    /// starts [`FLOOR_HISTORY_DAYS`] before the run's series start, so an
    /// old issuance lacks the actuals of its extension.
    pub outside_input: usize,
}

impl ExcludedExamples {
    /// The sum of both reasons.
    #[must_use]
    pub const fn total(self) -> usize {
        self.no_vintage + self.outside_input
    }

    fn add(&mut self, other: Self) {
        self.no_vintage += other.no_vintage;
        self.outside_input += other.outside_input;
    }
}

/// The training examples of a forecast run and the issuances they come from.
#[derive(Debug, Clone, PartialEq)]
pub struct IssuanceExamples {
    /// Newest issuance first.
    pub issuances: Vec<IssuanceSummary>,
    /// In issuance order, then target order.
    pub examples: Vec<TrainingExample>,
}

impl IssuanceExamples {
    /// The excluded targets of every issuance.
    #[must_use]
    pub fn excluded(&self) -> ExcludedExamples {
        let mut total = ExcludedExamples::default();
        for summary in &self.issuances {
            total.add(summary.excluded);
        }
        total
    }
}

/// The vintages of one dataset by target start (UTC ms), each list sorted
/// by issue time.
struct Ladder {
    by_target: BTreeMap<i64, Vec<(i64, f64)>>,
}

impl Ladder {
    fn new(vintages: &[GridVintage], before_ms: i64) -> Self {
        let mut by_target: BTreeMap<i64, Vec<(i64, f64)>> = BTreeMap::new();
        for v in vintages {
            let issued = v.issued_at.as_millisecond();
            if issued < before_ms {
                by_target
                    .entry(v.start.as_millisecond())
                    .or_default()
                    .push((issued, v.value));
            }
        }
        for list in by_target.values_mut() {
            list.sort_by_key(|(issued, _)| *issued);
        }
        Self { by_target }
    }

    /// Per target in `[from_ms, to_ms)`, the newest vintage issued strictly
    /// before `issued_ms`, and the issue time of each quarter.
    fn as_of(
        &self,
        issued_ms: i64,
        from_ms: i64,
        to_ms: i64,
    ) -> (Vec<GridRecord>, BTreeMap<QuarterStart, Timestamp>) {
        let mut records = Vec::new();
        let mut issued_by_quarter = BTreeMap::new();
        for (&start_ms, list) in self.by_target.range(from_ms..to_ms) {
            let known = list.partition_point(|(issued, _)| *issued < issued_ms);
            let Some(&(issued, value)) = known.checked_sub(1).and_then(|i| list.get(i)) else {
                continue;
            };
            let (Ok(start), Ok(issued_at)) = (
                Timestamp::from_millisecond(start_ms),
                Timestamp::from_millisecond(issued),
            ) else {
                continue;
            };
            records.push(GridRecord { start, value });
            let slot = issued_by_quarter
                .entry(QuarterStart::floor(start))
                .or_insert(issued_at);
            if issued_at > *slot {
                *slot = issued_at;
            }
        }
        (records, issued_by_quarter)
    }
}

/// The inputs of one issuance that a row reads.
struct View<'a> {
    issued_ms: i64,
    ctx: &'a FeatureContext<'a>,
    index: &'a PriceIndex,
    grid: &'a GridSeries,
    wind_issued: Option<&'a BTreeMap<QuarterStart, Timestamp>>,
    consumption_issued: Option<&'a BTreeMap<QuarterStart, Timestamp>>,
}

fn source(
    q: QuarterStart,
    forecast: &QuarterMap,
    extended: &QuarterMap,
    merged: &QuarterMap,
    issued: Option<&BTreeMap<QuarterStart, Timestamp>>,
) -> InputSource {
    if forecast.contains_key(&q) {
        InputSource::Vintage {
            issued_at: issued.and_then(|map| map.get(&q).copied()),
        }
    } else if extended.contains_key(&q) {
        InputSource::Extension
    } else if merged.contains_key(&q) {
        InputSource::Actual
    } else {
        InputSource::Missing
    }
}

/// Hours from `from_ms` to `to_ms`, at minute resolution.
fn hours_between(from_ms: i64, to_ms: i64) -> f64 {
    let minutes = (to_ms - from_ms).div_euclid(60_000);
    let minutes = i32::try_from(minutes).unwrap_or(if minutes < 0 { i32::MIN } else { i32::MAX });
    f64::from(minutes) / 60.0
}

fn indicator(flag: bool) -> f64 {
    if flag { 1.0 } else { 0.0 }
}

/// The fraction of [`NEIGHBOR_AREAS`] without a value; the count is at most
/// 3, so the conversion is exact.
fn missing_fraction(present: usize) -> f64 {
    let total = NEIGHBOR_AREAS.len();
    let missing = u32::try_from(total.saturating_sub(present)).unwrap_or(0);
    let total = u32::try_from(total).unwrap_or(1).max(1);
    f64::from(missing) / f64::from(total)
}

/// The row of `q` and what it was built from.
fn example_row(
    q: QuarterStart,
    view: &View<'_>,
) -> (Vec<f64>, InputSource, InputSource, LagAvailability) {
    let mut row = build_feature_vector_indexed(q, view.ctx, view.index);
    let wind = row.get(WIND_COLUMN).copied().unwrap_or(0.0);
    let consumption = row.get(CONSUMPTION_COLUMN).copied().unwrap_or(0.0);
    let lead_hours = hours_between(view.issued_ms, q.as_millisecond());
    let wind_source = source(
        q,
        &view.grid.wind_forecast,
        &view.grid.wind_extended,
        &view.grid.wind,
        view.wind_issued,
    );
    let consumption_source = source(
        q,
        &view.grid.consumption_forecast,
        &view.grid.consumption_extended,
        &view.grid.consumption,
        view.consumption_issued,
    );
    let day_ago = q.offset(-DAY_MS);
    let week_ago = q.offset(-7 * DAY_MS);
    let has_lag = |area, key: QuarterStart| {
        view.ctx
            .neighbor_prices
            .get(&area)
            .is_some_and(|series| series.contains_key(&key))
    };
    let lags = LagAvailability {
        fi_1d: view.ctx.fi_prices.contains_key(&day_ago),
        fi_7d: view.ctx.fi_prices.contains_key(&week_ago),
        fi_prev_day: view.index.has_previous_day(q),
        neighbors_1d: NEIGHBOR_AREAS
            .iter()
            .filter(|&&area| has_lag(area, day_ago))
            .count(),
        neighbors_7d: NEIGHBOR_AREAS
            .iter()
            .filter(|&&area| has_lag(area, week_ago))
            .count(),
    };
    let from_vintage = |s: InputSource| indicator(matches!(s, InputSource::Vintage { .. }));
    row.extend([
        lead_hours,
        wind * lead_hours,
        consumption * lead_hours,
        from_vintage(wind_source),
        from_vintage(consumption_source),
        indicator(!lags.fi_1d),
        indicator(!lags.fi_7d),
        indicator(!lags.fi_prev_day),
        missing_fraction(lags.neighbors_1d),
        missing_fraction(lags.neighbors_7d),
    ]);
    (row, wind_source, consumption_source, lags)
}

fn records_before(records: &[GridRecord], from_ms: i64, before_ms: i64) -> Vec<GridRecord> {
    records
        .iter()
        .filter(|r| (from_ms..before_ms).contains(&r.start.as_millisecond()))
        .copied()
        .collect()
}

fn feature_rules(opts: &ForecastOptions<'_>) -> FeatureRules {
    FeatureRules {
        latest_published: opts.latest_published,
        previous_day: opts.prev_day_basis.into(),
    }
}

/// The issuances whole days before the run that predicted a quarter of the
/// training window, at most this many.
const MAX_TRAINING_ISSUANCES: i64 = TRAINING_HISTORY_DAYS + FORECAST_DAYS + 2;

/// The series start of an issuance: one quarter after the last FI price in
/// the input that is published at `issued_at`. Without such a price, it is
/// the publication horizon itself. `None` when the horizon leaves the Jiff
/// range.
fn issuance_series_start(
    input: &ForecastInput<'_>,
    issued_at: Timestamp,
    delivery_zone: &TimeZone,
    run_start: QuarterStart,
) -> Option<QuarterStart> {
    let horizon = publication_horizon(issued_at, delivery_zone).ok()?;
    let published_end = QuarterStart::floor(horizon).min(run_start);
    Some(
        input
            .spot_prices
            .range(..published_end)
            .next_back()
            .map_or(published_end, |(&last, _)| last.offset(QUARTER_MS)),
    )
}

/// The owned inputs of one training issuance.
struct IssuanceInputs {
    issued_at: Timestamp,
    series_start: QuarterStart,
    series_end: QuarterStart,
    fi_prices: QuarterMap,
    neighbor_prices: NeighborPrices,
    wind_forecast: Vec<GridRecord>,
    wind_issued: BTreeMap<QuarterStart, Timestamp>,
    consumption_forecast: Vec<GridRecord>,
    consumption_issued: BTreeMap<QuarterStart, Timestamp>,
    wind_actual: Vec<GridRecord>,
    consumption_actual: Vec<GridRecord>,
}

impl IssuanceInputs {
    fn reconstruct(
        input: &ForecastInput<'_>,
        ladders: &[Ladder; 2],
        issued_at: Timestamp,
        series_start: QuarterStart,
    ) -> Self {
        let series_end = series_start.offset(FORECAST_DAYS * DAY_MS);
        let history_start = series_start.offset(-FLOOR_HISTORY_DAYS * DAY_MS);
        let visible = |series: &QuarterMap| -> QuarterMap {
            series
                .range(history_start..series_start)
                .map(|(&k, &v)| (k, v))
                .collect()
        };
        let issued_ms = issued_at.as_millisecond();
        let (from_ms, to_ms) = (history_start.as_millisecond(), series_end.as_millisecond());
        let [wind_ladder, consumption_ladder] = ladders;
        let (wind_forecast, wind_issued) = wind_ladder.as_of(issued_ms, from_ms, to_ms);
        let (consumption_forecast, consumption_issued) =
            consumption_ladder.as_of(issued_ms, from_ms, to_ms);
        Self {
            issued_at,
            series_start,
            series_end,
            fi_prices: visible(input.spot_prices),
            neighbor_prices: input
                .neighbor_prices
                .iter()
                .map(|(&area, series)| (area, visible(series)))
                .collect(),
            wind_forecast,
            wind_issued,
            consumption_forecast,
            consumption_issued,
            wind_actual: records_before(input.wind_actual, from_ms, issued_ms),
            consumption_actual: records_before(input.consumption_actual, from_ms, issued_ms),
        }
    }

    fn forecast_input(&self) -> Option<ForecastInput<'_>> {
        Some(ForecastInput {
            spot_prices: &self.fi_prices,
            neighbor_prices: &self.neighbor_prices,
            wind_forecast: &self.wind_forecast,
            wind_actual: &self.wind_actual,
            consumption_forecast: &self.consumption_forecast,
            consumption_actual: &self.consumption_actual,
            series_start: self.series_start.timestamp().ok()?,
            series_end: self.series_end.timestamp().ok()?,
        })
    }

    fn summary(&self, has_grid_data: bool) -> IssuanceSummary {
        IssuanceSummary {
            issued_at: self.issued_at,
            series_start: self.series_start,
            first_price: self.fi_prices.keys().next().copied(),
            price_quarters: self.fi_prices.len(),
            wind_vintage_targets: self.wind_issued.len(),
            consumption_vintage_targets: self.consumption_issued.len(),
            has_grid_data,
            examples: 0,
            targets_without_price: 0,
            excluded: ExcludedExamples::default(),
        }
    }
}

/// The examples of one issuance with grid data, appended to `out`.
fn issuance_examples(
    input: &ForecastInput<'_>,
    opts: &ForecastOptions<'_>,
    inputs: &IssuanceInputs,
    issuance_input: &ForecastInput<'_>,
    window: (QuarterStart, QuarterStart),
    summary: &mut IssuanceSummary,
    out: &mut Vec<TrainingExample>,
) {
    let grid = grid_series(issuance_input, opts, inputs.series_end.as_millisecond());
    let ctx = FeatureContext {
        fi_prices: &inputs.fi_prices,
        neighbor_prices: &inputs.neighbor_prices,
        wind: &grid.wind,
        consumption: &grid.consumption,
    };
    let index = build_price_index(&ctx, feature_rules(opts));
    let issued_ms = inputs.issued_at.as_millisecond();
    let view = View {
        issued_ms,
        ctx: &ctx,
        index: &index,
        grid: &grid,
        wind_issued: Some(&inputs.wind_issued),
        consumption_issued: Some(&inputs.consumption_issued),
    };
    let (window_start, run_start) = window;
    let mut q = inputs.series_start.max(window_start);
    while q < inputs.series_end.min(run_start) {
        if let Some(&realised) = input.spot_prices.get(&q) {
            let (features, wind, consumption, lags) = example_row(q, &view);
            let missing = |source, vintages: &BTreeMap<QuarterStart, Timestamp>| {
                (source == InputSource::Missing).then_some(vintages.is_empty())
            };
            let reasons = [
                missing(wind, &inputs.wind_issued),
                missing(consumption, &inputs.consumption_issued),
            ];
            if reasons.iter().any(Option::is_some) {
                if reasons.contains(&Some(false)) {
                    summary.excluded.outside_input += 1;
                } else {
                    summary.excluded.no_vintage += 1;
                }
                q = q.offset(QUARTER_MS);
                continue;
            }
            out.push(TrainingExample {
                issued_at: inputs.issued_at,
                series_start: inputs.series_start,
                target: q,
                lead_ms: q.as_millisecond() - issued_ms,
                wind,
                consumption,
                lags,
                features,
                realised,
            });
            summary.examples += 1;
        } else {
            summary.targets_without_price += 1;
        }
        q = q.offset(QUARTER_MS);
    }
}

/// The training examples of issuance training. The issuances are whole
/// days before `history.issued_at`. Issuance `i` predicts from its own
/// series start `S_i`: one quarter after the last FI price published at `i`
/// ([`publication_horizon`]). It sees prices in `[S_i − FLOOR_HISTORY_DAYS,
/// S_i)` that are in the input, vintages issued before `i`, and actuals
/// that started before `i`. An example is a target in `[S_i, S_i +
/// FORECAST_DAYS)` inside the training window before the run's series
/// start, with a published price. An issuance without grid data or without
/// a published price in the input gives no examples; its summary shows it.
#[must_use]
pub fn build_issuance_examples(
    input: &ForecastInput<'_>,
    opts: &ForecastOptions<'_>,
    history: &IssuanceHistory<'_>,
) -> IssuanceExamples {
    let run_start = QuarterStart::floor(input.series_start);
    let window_start = run_start.offset(-TRAINING_HISTORY_DAYS * DAY_MS);
    let run_ms = history.issued_at.as_millisecond();
    let ladders = [
        Ladder::new(history.wind_vintages, run_ms),
        Ladder::new(history.consumption_vintages, run_ms),
    ];
    let mut out = IssuanceExamples {
        issuances: Vec::new(),
        examples: Vec::new(),
    };
    for k in 1..=MAX_TRAINING_ISSUANCES {
        let Ok(issued_at) = Timestamp::from_millisecond(run_ms - k * DAY_MS) else {
            break;
        };
        let Some(series_start) =
            issuance_series_start(input, issued_at, history.delivery_zone, run_start)
        else {
            continue;
        };
        if series_start.offset(FORECAST_DAYS * DAY_MS) <= window_start {
            break;
        }
        let inputs = IssuanceInputs::reconstruct(input, &ladders, issued_at, series_start);
        let Some(issuance_input) = inputs.forecast_input() else {
            continue;
        };
        let mut summary = inputs.summary(has_grid_data(&issuance_input));
        // An issuance without a published price in the input lies before
        // the price window; its rows would hold only neutral fills.
        if summary.has_grid_data && summary.price_quarters > 0 {
            issuance_examples(
                input,
                opts,
                &inputs,
                &issuance_input,
                (window_start, run_start),
                &mut summary,
                &mut out.examples,
            );
        }
        out.issuances.push(summary);
    }
    out
}

/// The served inputs of the run that issuance training predicts from.
pub(super) struct ServedView<'a> {
    pub(super) grid: &'a GridSeries,
    pub(super) ctx: &'a FeatureContext<'a>,
    pub(super) index: &'a PriceIndex,
    pub(super) series_start: QuarterStart,
    pub(super) num_quarters: usize,
}

/// Fit on the issuance examples and predict the served quarters with rows
/// of the same assembly at the run's issue time.
pub(super) fn fit_and_predict(
    input: &ForecastInput<'_>,
    opts: &ForecastOptions<'_>,
    history: &IssuanceHistory<'_>,
    served: &ServedView<'_>,
) -> (QuarterMap, FitMeta, ExcludedExamples) {
    let examples = build_issuance_examples(input, opts, history);
    let rows: Vec<Vec<f64>> = examples
        .examples
        .iter()
        .map(|e| e.features.clone())
        .collect();
    let targets: Vec<f64> = examples.examples.iter().map(|e| e.realised).collect();
    let fitted = fit_ridge(
        &TrainingData {
            feature_count: ISSUANCE_FEATURE_COUNT,
            rows: &rows,
            targets: &targets,
        },
        &FitOptions {
            ridge_lambda: opts.ridge_lambda,
            price_transform: PriceTransform::Arcsinh,
        },
    );
    let view = View {
        issued_ms: history.issued_at.as_millisecond(),
        ctx: served.ctx,
        index: served.index,
        grid: served.grid,
        wind_issued: None,
        consumption_issued: None,
    };
    let mut predicted = QuarterMap::new();
    let mut cursor = served.series_start;
    for _ in 0..served.num_quarters {
        let (features, ..) = example_row(cursor, &view);
        predicted.insert(cursor, fitted.predict(&features));
        cursor = cursor.offset(QUARTER_MS);
    }
    (predicted, fitted.meta, examples.excluded())
}
