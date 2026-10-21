//! The forecast pipeline: features, ridge fit, prediction, within-day shape,
//! sanity clamp, predicted series, and bands.

use jiff::Timestamp;
use jiff::tz::TimeZone;

use super::conformal::{CalibratedBands, UtcHour, apply_band};
use super::conformal_artifact::CALIBRATED_BANDS;
use super::correction::{self, CorrectionDiagnostics, QuarterCorrection};
use super::features::{
    BuiltTraining, FEATURE_NAMES, FeatureContext, FeatureRules, LatestPublishedRule,
    NeighborPrices, PrevDayBasis, build_feature_vector_indexed, build_price_index,
    training_matrix_at,
};
use super::issuance::{self, ExcludedExamples, TrainingRule};
use super::js::{js_max, js_min};
use super::model::{
    DEFAULT_RIDGE_LAMBDA, FitMeta, FitOptions, PriceTransform, TrainingData, fit_ridge,
};
use super::quarter::{QUARTER_MS, QuarterMap, QuarterStart};
use super::route::{FLOOR_HISTORY_DAYS, FORECAST_DAYS};
use super::series::{
    ForecastSpotPoint, GAP_FILL_LOOKBACK_WEEKS, GridRecord, SanityBound, ShapeProfile,
    apply_within_day_shape_with, bucket_records, build_predicted_series, expand_hourly_to_quarters,
    extend_with_last_week, fill_gaps_from_earlier_weeks,
};

/// Wind tail extension: the mean over this many weeks back.
pub const WIND_EXTENSION_WEEKS: u32 = 4;
/// Consumption tail extension: the same quarter this many weeks back.
pub const CONSUMPTION_EXTENSION_WEEKS: u32 = 1;

/// What a wind or consumption feature reads for a quarter without a forecast
/// or an actual value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GridGapRule {
    /// Fill the quarter from earlier weeks of actuals
    /// ([`fill_gaps_from_earlier_weeks`]). A quarter that no earlier week
    /// fills reads 0 MW and is counted in [`ForecastDiagnostics`].
    #[default]
    EarlierWeeks,
    /// Read 0 MW, without a count. The TypeScript parity fixtures follow
    /// this rule.
    Zero,
}

/// The inputs of one forecast run. The caller loads them; the pipeline does
/// no I/O.
#[derive(Debug, Clone, Copy)]
pub struct ForecastInput<'a> {
    /// Published FI spot prices (c/kWh): the fit target and the lag source.
    pub spot_prices: &'a QuarterMap,
    /// Neighbour spot prices (c/kWh). A missing area is not a degradation.
    pub neighbor_prices: &'a NeighborPrices,
    pub wind_forecast: &'a [GridRecord],
    pub wind_actual: &'a [GridRecord],
    pub consumption_forecast: &'a [GridRecord],
    pub consumption_actual: &'a [GridRecord],
    /// The first quarter to predict; it is floored to its quarter.
    pub series_start: Timestamp,
    /// The exclusive end of the predicted window; it is floored to its quarter.
    pub series_end: Timestamp,
}

/// Options of one forecast run.
#[derive(Debug, Clone, Copy)]
pub struct ForecastOptions<'a> {
    pub wind_extension_weeks: u32,
    pub consumption_extension_weeks: u32,
    /// The output clamp. `None` emits the predictions unclamped.
    pub sanity_bound: Option<SanityBound>,
    pub ridge_lambda: f64,
    /// Bands go only on quarters with a real prediction.
    pub bands: &'a CalibratedBands,
    /// The served forecast uses [`LatestPublishedRule::Before`].
    pub latest_published: LatestPublishedRule,
    /// The served forecast uses [`PrevDayBasis::UtcDay`].
    pub prev_day_basis: PrevDayBasis<'a>,
    /// The served forecast uses [`ShapeProfile::WeekdayMean`] of
    /// [`SERVED_SHAPE_WEEKS`] weeks in the delivery-day zone.
    pub shape_profile: ShapeProfile<'a>,
    /// The served forecast uses [`TrainingRule::Realised`].
    pub training: TrainingRule<'a>,
    /// The served forecast uses `None`: no quarter correction.
    pub correction: Option<QuarterCorrection<'a>>,
    /// The served forecast uses [`GridGapRule::EarlierWeeks`].
    pub grid_gaps: GridGapRule,
}

/// The weeks of the weekday-mean shape profile of the served forecast.
pub const SERVED_SHAPE_WEEKS: u8 = 4;

// Every source day of the served profile is a published day inside the price
// read window, so only a store gap or a DST gap leaves a quarter flat.
const _: () = assert!(
    7 * (SERVED_SHAPE_WEEKS as i64) < FLOOR_HISTORY_DAYS && FORECAST_DAYS < 7,
    "the served shape profile reads outside the price window"
);

// The gap fill of a predicted quarter reads only actuals inside the Fingrid
// read window, which starts FLOOR_HISTORY_DAYS before the series start.
const _: () = assert!(
    7 * (GAP_FILL_LOOKBACK_WEEKS as i64) <= FLOOR_HISTORY_DAYS,
    "the gap fill reads outside the Fingrid window"
);

impl<'a> ForecastOptions<'a> {
    /// The options of the served forecast without a sanity bound.
    /// `delivery_zone` must be
    /// [`DELIVERY_DAY_TIME_ZONE`](super::DELIVERY_DAY_TIME_ZONE).
    #[must_use]
    pub fn served(delivery_zone: &'a TimeZone) -> Self {
        Self {
            shape_profile: ShapeProfile::WeekdayMean {
                zone: delivery_zone,
                weeks: SERVED_SHAPE_WEEKS,
            },
            ..ForecastOptions::default()
        }
    }
}

/// The served options with [`ShapeProfile::Persistence`]. The served shape
/// needs a time zone; use [`ForecastOptions::served`] for the served forecast.
impl Default for ForecastOptions<'static> {
    fn default() -> Self {
        Self {
            wind_extension_weeks: WIND_EXTENSION_WEEKS,
            consumption_extension_weeks: CONSUMPTION_EXTENSION_WEEKS,
            sanity_bound: None,
            ridge_lambda: DEFAULT_RIDGE_LAMBDA,
            bands: &CALIBRATED_BANDS,
            latest_published: LatestPublishedRule::Before,
            prev_day_basis: PrevDayBasis::UtcDay,
            shape_profile: ShapeProfile::Persistence,
            training: TrainingRule::Realised,
            correction: None,
            grid_gaps: GridGapRule::EarlierWeeks,
        }
    }
}

/// Counters of one forecast run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForecastDiagnostics {
    /// Aligned training samples that the fit used.
    pub fit_samples: usize,
    pub feature_count: usize,
    /// True when the fit uses the constant fallback.
    pub fit_used_default: bool,
    pub consumption_extended_quarters: usize,
    pub wind_extended_quarters: usize,
    /// [`GridGapRule::EarlierWeeks`] only: wind and consumption quarters
    /// that the gap fill filled, summed over both datasets.
    pub grid_gap_filled_quarters: usize,
    /// [`GridGapRule::EarlierWeeks`] only: predicted quarters whose wind or
    /// consumption feature reads 0 MW because no value exists. Expected 0.
    pub grid_missing_quarters: usize,
    /// [`GridGapRule::EarlierWeeks`] and [`TrainingRule::Realised`] only:
    /// training rows whose wind or consumption feature reads 0 MW because no
    /// value exists. Expected 0.
    pub training_grid_missing_rows: usize,
    pub filled_quarters: usize,
    pub zero_seeded_quarters: usize,
    /// Quarters with no profile price; not a degradation.
    pub shape_fallback_quarters: usize,
    /// Quarters that the sanity bound clamped; expected 0.
    pub sanity_clamped_quarters: usize,
    pub band_calibrated: bool,
    /// Quarters that have band bounds.
    pub band_hour_buckets: usize,
    /// Issuance training only: targets left out for a missing grid value.
    pub training_excluded: ExcludedExamples,
    /// The quarter correction only.
    pub correction: Option<CorrectionDiagnostics>,
}

/// The predicted spot series and its diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct ForecastResult {
    pub series: Vec<ForecastSpotPoint>,
    pub diagnostics: ForecastDiagnostics,
}

/// The grid series after bucketing, tail extension, and merging.
pub(super) struct GridSeries {
    pub(super) wind: QuarterMap,
    pub(super) consumption: QuarterMap,
    /// The bucketed forecast records.
    pub(super) wind_forecast: QuarterMap,
    pub(super) consumption_forecast: QuarterMap,
    /// The bucketed forecast records and their tail extension.
    pub(super) wind_extended: QuarterMap,
    pub(super) consumption_extended: QuarterMap,
    wind_extended_quarters: usize,
    consumption_extended_quarters: usize,
    gap_filled_quarters: usize,
}

impl GridSeries {
    /// Whether the wind or the consumption feature of `q` has no value.
    pub(super) fn lacks_value(&self, q: QuarterStart) -> bool {
        !self.wind.contains_key(&q) || !self.consumption.contains_key(&q)
    }
}

pub(super) fn grid_series(
    input: &ForecastInput<'_>,
    opts: &ForecastOptions<'_>,
    series_end_ms: i64,
) -> GridSeries {
    let wind_forecast = bucket_records(input.wind_forecast);
    let wind_actual = bucket_records(input.wind_actual);
    let consumption_forecast = bucket_records(input.consumption_forecast);
    let consumption_actual = expand_hourly_to_quarters(input.consumption_actual);

    let mut consumption_extended = extend_with_last_week(
        &consumption_forecast,
        &consumption_actual,
        series_end_ms,
        opts.consumption_extension_weeks,
    );
    let mut wind_extended = extend_with_last_week(
        &wind_forecast,
        &wind_actual,
        series_end_ms,
        opts.wind_extension_weeks,
    );

    let wind_extended_quarters = wind_extended.len().saturating_sub(wind_forecast.len());
    let consumption_extended_quarters = consumption_extended
        .len()
        .saturating_sub(consumption_forecast.len());

    // The forecast value wins where an actual also exists.
    let mut wind = wind_actual.clone();
    wind.extend(wind_extended.iter().map(|(&k, &v)| (k, v)));
    let mut consumption = consumption_actual.clone();
    consumption.extend(consumption_extended.iter().map(|(&k, &v)| (k, v)));

    let mut gap_filled_quarters = 0;
    if opts.grid_gaps == GridGapRule::EarlierWeeks {
        // A gap-filled quarter joins the extension, so an issuance row
        // reports its source as the extension.
        for (merged, extended, actual, weeks) in [
            (
                &mut wind,
                &mut wind_extended,
                &wind_actual,
                opts.wind_extension_weeks,
            ),
            (
                &mut consumption,
                &mut consumption_extended,
                &consumption_actual,
                opts.consumption_extension_weeks,
            ),
        ] {
            let filled = fill_gaps_from_earlier_weeks(merged, actual, series_end_ms, weeks);
            gap_filled_quarters += filled.len();
            merged.extend(filled.iter().map(|(&k, &v)| (k, v)));
            extended.extend(filled);
        }
    }

    GridSeries {
        wind,
        consumption,
        wind_forecast,
        consumption_forecast,
        wind_extended,
        consumption_extended,
        wind_extended_quarters,
        consumption_extended_quarters,
        gap_filled_quarters,
    }
}

/// Clamp each value into `bound`; returns the number of changed values.
fn clamp_predictions(predicted: &mut QuarterMap, bound: SanityBound) -> usize {
    let mut clamped = 0;
    for value in predicted.values_mut() {
        let bounded = js_min(bound.max, js_max(bound.min, *value));
        // NaN counts as changed; -0.0 and 0.0 count as equal.
        #[expect(clippy::float_cmp, reason = "an exact change check")]
        let changed = bounded != *value;
        if changed {
            clamped += 1;
        }
        *value = bounded;
    }
    clamped
}

/// Fit on the realised rows of the training window and predict the served
/// quarters. The count is the training rows without a wind or consumption
/// value.
fn fit_realised(
    opts: &ForecastOptions<'_>,
    served: &issuance::ServedView<'_>,
) -> (QuarterMap, FitMeta, usize) {
    let training = training_matrix_at(
        served.series_start.as_millisecond(),
        served.ctx,
        Some(served.index),
    );
    let missing_rows = training
        .keys
        .iter()
        .filter(|&&q| served.grid.lacks_value(q))
        .count();
    let fitted = fit_ridge(
        &TrainingData {
            feature_count: FEATURE_NAMES.len(),
            rows: &training.rows,
            targets: &training.targets,
        },
        &FitOptions {
            ridge_lambda: opts.ridge_lambda,
            price_transform: PriceTransform::Arcsinh,
        },
    );
    let mut predicted = QuarterMap::new();
    let mut cursor = served.series_start;
    for _ in 0..served.num_quarters {
        let features = build_feature_vector_indexed(cursor, served.ctx, served.index);
        predicted.insert(cursor, fitted.predict(&features));
        cursor = cursor.offset(QUARTER_MS);
    }
    (predicted, fitted.meta, missing_rows)
}

/// The realised training rows and the prediction rows of a run, as
/// [`build_forecast`] builds them with [`TrainingRule::Realised`].
#[derive(Debug, Clone, PartialEq)]
pub struct FeatureRows {
    pub training: BuiltTraining,
    /// One row per predicted quarter, in order.
    pub predicted: Vec<(QuarterStart, Vec<f64>)>,
}

/// The feature rows of a run, for inspection. `opts.training` is ignored.
#[must_use]
pub fn feature_rows(input: &ForecastInput<'_>, opts: &ForecastOptions<'_>) -> FeatureRows {
    let series_start = QuarterStart::floor(input.series_start);
    let series_end = QuarterStart::floor(input.series_end);
    let grid = grid_series(input, opts, series_end.as_millisecond());
    let ctx = FeatureContext {
        fi_prices: input.spot_prices,
        neighbor_prices: input.neighbor_prices,
        wind: &grid.wind,
        consumption: &grid.consumption,
    };
    let index = build_price_index(
        &ctx,
        FeatureRules {
            latest_published: opts.latest_published,
            previous_day: opts.prev_day_basis.into(),
        },
    );
    let mut predicted = Vec::new();
    let mut cursor = series_start;
    while cursor < series_end {
        predicted.push((cursor, build_feature_vector_indexed(cursor, &ctx, &index)));
        cursor = cursor.offset(QUARTER_MS);
    }
    FeatureRows {
        training: training_matrix_at(series_start.as_millisecond(), &ctx, Some(&index)),
        predicted,
    }
}

/// Run the pipeline over `[series_start, series_end)` at quarter resolution.
/// The series holds only predictions: published prices fit the model but are
/// never passed through.
#[must_use]
pub fn build_forecast(input: &ForecastInput<'_>, opts: &ForecastOptions<'_>) -> ForecastResult {
    let series_start = QuarterStart::floor(input.series_start);
    let series_end = QuarterStart::floor(input.series_end);
    let grid = grid_series(input, opts, series_end.as_millisecond());

    let ctx = FeatureContext {
        fi_prices: input.spot_prices,
        neighbor_prices: input.neighbor_prices,
        wind: &grid.wind,
        consumption: &grid.consumption,
    };
    let index = build_price_index(
        &ctx,
        FeatureRules {
            latest_published: opts.latest_published,
            previous_day: opts.prev_day_basis.into(),
        },
    );
    let span_ms = series_end.as_millisecond() - series_start.as_millisecond();
    let num_quarters = usize::try_from((span_ms / QUARTER_MS).max(0)).unwrap_or(0);

    let served = issuance::ServedView {
        grid: &grid,
        ctx: &ctx,
        index: &index,
        series_start,
        num_quarters,
    };
    let mut training_grid_missing_rows = 0;
    let (predicted, fit_meta, training_excluded) = match opts.training {
        TrainingRule::Realised => {
            let (predicted, meta, missing_rows) = fit_realised(opts, &served);
            training_grid_missing_rows = missing_rows;
            (predicted, meta, ExcludedExamples::default())
        }
        TrainingRule::Issuance(history) => {
            issuance::fit_and_predict(input, opts, &history, &served)
        }
    };

    let (grid_missing_quarters, training_grid_missing_rows) = match opts.grid_gaps {
        GridGapRule::EarlierWeeks => (
            predicted.keys().filter(|&&q| grid.lacks_value(q)).count(),
            training_grid_missing_rows,
        ),
        GridGapRule::Zero => (0, 0),
    };

    let shaped = apply_within_day_shape_with(&predicted, input.spot_prices, opts.shape_profile);
    let mut predicted = shaped.result;
    // The correction runs after the shape step, so the step cannot erase it.
    let correction = opts
        .correction
        .map(|c| correction::apply(&mut predicted, input, opts, &c, &served));
    let sanity_clamped_quarters = opts
        .sanity_bound
        .map_or(0, |bound| clamp_predictions(&mut predicted, bound));

    let built = build_predicted_series(&predicted, series_start, num_quarters);
    let mut band_hour_buckets = 0;
    let series = built
        .series
        .into_iter()
        .map(|point| {
            if !predicted.contains_key(&point.start) {
                return point;
            }
            let band = apply_band(
                point.estimated_spot_cents_kwh,
                UtcHour::of(point.start),
                opts.bands,
            );
            if band.is_some() {
                band_hour_buckets += 1;
            }
            ForecastSpotPoint { band, ..point }
        })
        .collect();

    ForecastResult {
        series,
        diagnostics: ForecastDiagnostics {
            fit_samples: fit_meta.sample_count,
            feature_count: fit_meta.feature_count,
            fit_used_default: fit_meta.used_fallback,
            consumption_extended_quarters: grid.consumption_extended_quarters,
            wind_extended_quarters: grid.wind_extended_quarters,
            grid_gap_filled_quarters: grid.gap_filled_quarters,
            grid_missing_quarters,
            training_grid_missing_rows,
            filled_quarters: built.filled_quarters,
            zero_seeded_quarters: built.zero_seeded_quarters,
            shape_fallback_quarters: shaped.shape_fallback_quarters,
            sanity_clamped_quarters,
            band_calibrated: opts.bands.calibrated,
            band_hour_buckets,
            training_excluded,
            correction,
        },
    }
}
