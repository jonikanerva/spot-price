//! The forecast pipeline: features, ridge fit, prediction, within-day shape,
//! sanity clamp, predicted series, and bands.

use jiff::Timestamp;

use super::conformal::{CalibratedBands, UtcHour, apply_band};
use super::conformal_artifact::CALIBRATED_BANDS;
use super::features::{
    FEATURE_NAMES, FeatureContext, NeighborPrices, build_feature_vector_indexed,
    build_forecast_price_index, training_matrix_at,
};
use super::js::{js_max, js_min};
use super::model::{DEFAULT_RIDGE_LAMBDA, FitOptions, PriceTransform, TrainingData, fit_ridge};
use super::quarter::{QUARTER_MS, QuarterMap, QuarterStart};
use super::series::{
    ForecastSpotPoint, GridRecord, SanityBound, apply_within_day_shape, bucket_records,
    build_predicted_series, expand_hourly_to_quarters, extend_with_last_week,
};

/// Wind tail extension: the mean over this many weeks back.
pub const WIND_EXTENSION_WEEKS: u32 = 4;
/// Consumption tail extension: the same quarter this many weeks back.
pub const CONSUMPTION_EXTENSION_WEEKS: u32 = 1;

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
}

impl Default for ForecastOptions<'static> {
    fn default() -> Self {
        Self {
            wind_extension_weeks: WIND_EXTENSION_WEEKS,
            consumption_extension_weeks: CONSUMPTION_EXTENSION_WEEKS,
            sanity_bound: None,
            ridge_lambda: DEFAULT_RIDGE_LAMBDA,
            bands: &CALIBRATED_BANDS,
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
    pub filled_quarters: usize,
    pub zero_seeded_quarters: usize,
    /// Quarters with no 1-day or 7-day lag; not a degradation.
    pub shape_fallback_quarters: usize,
    /// Quarters that the sanity bound clamped; expected 0.
    pub sanity_clamped_quarters: usize,
    pub band_calibrated: bool,
    /// Quarters that have band bounds.
    pub band_hour_buckets: usize,
}

/// The predicted spot series and its diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct ForecastResult {
    pub series: Vec<ForecastSpotPoint>,
    pub diagnostics: ForecastDiagnostics,
}

/// The grid series after bucketing, tail extension, and merging.
struct GridSeries {
    wind: QuarterMap,
    consumption: QuarterMap,
    wind_extended_quarters: usize,
    consumption_extended_quarters: usize,
}

fn grid_series(
    input: &ForecastInput<'_>,
    opts: &ForecastOptions<'_>,
    series_end_ms: i64,
) -> GridSeries {
    let wind_forecast = bucket_records(input.wind_forecast);
    let wind_actual = bucket_records(input.wind_actual);
    let consumption_forecast = bucket_records(input.consumption_forecast);
    let consumption_actual = expand_hourly_to_quarters(input.consumption_actual);

    let consumption_extended = extend_with_last_week(
        &consumption_forecast,
        &consumption_actual,
        series_end_ms,
        opts.consumption_extension_weeks,
    );
    let wind_extended = extend_with_last_week(
        &wind_forecast,
        &wind_actual,
        series_end_ms,
        opts.wind_extension_weeks,
    );

    // The forecast value wins where an actual also exists.
    let mut wind = wind_actual;
    wind.extend(wind_extended.iter().map(|(&k, &v)| (k, v)));
    let mut consumption = consumption_actual;
    consumption.extend(consumption_extended.iter().map(|(&k, &v)| (k, v)));

    GridSeries {
        wind,
        consumption,
        wind_extended_quarters: wind_extended.len().saturating_sub(wind_forecast.len()),
        consumption_extended_quarters: consumption_extended
            .len()
            .saturating_sub(consumption_forecast.len()),
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
    let index = build_forecast_price_index(&ctx);
    let training = training_matrix_at(series_start.as_millisecond(), &ctx, Some(&index));
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

    let span_ms = series_end.as_millisecond() - series_start.as_millisecond();
    let num_quarters = usize::try_from((span_ms / QUARTER_MS).max(0)).unwrap_or(0);

    let mut predicted = QuarterMap::new();
    let mut cursor = series_start;
    for _ in 0..num_quarters {
        let features = build_feature_vector_indexed(cursor, &ctx, &index);
        predicted.insert(cursor, fitted.predict(&features));
        cursor = cursor.offset(QUARTER_MS);
    }

    let shaped = apply_within_day_shape(&predicted, input.spot_prices);
    let mut predicted = shaped.result;
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
            fit_samples: fitted.meta.sample_count,
            feature_count: fitted.meta.feature_count,
            fit_used_default: fitted.meta.used_fallback,
            consumption_extended_quarters: grid.consumption_extended_quarters,
            wind_extended_quarters: grid.wind_extended_quarters,
            filled_quarters: built.filled_quarters,
            zero_seeded_quarters: built.zero_seeded_quarters,
            shape_fallback_quarters: shaped.shape_fallback_quarters,
            sanity_clamped_quarters,
            band_calibrated: opts.bands.calibrated,
            band_hour_buckets,
        },
    }
}
