//! The FI price forecast: a closed-form ridge regression over Fingrid grid
//! data, FI and neighbour price lags, and the UTC calendar. Everything here
//! is pure; the caller passes all data and instants in. Series are keyed by
//! UTC quarters. The forecast predicts spot c/kWh.

mod conformal;
mod conformal_artifact;
mod entries;
mod features;
mod js;
mod model;
mod pipeline;
mod quarter;
mod route;
mod series;

pub use conformal::{
    BandMethod, BandOffsets, COVERAGE_GATE_THRESHOLD, CalibratedBands, DeriveOptions,
    DerivedOffsets, HorizonResidual, HourOffsets, MIN_HOUR_CALIBRATION_SAMPLES,
    MIN_TOTAL_CALIBRATION_SAMPLES, NOMINAL_COVERAGE, SpotBand, UtcHour, apply_band, build_artifact,
    derive_band_offsets, observed_coverage_of, uncalibrated_bands,
};
pub use conformal_artifact::CALIBRATED_BANDS;
pub use entries::{EntryBand, ForecastEntry, to_forecast_entries};
pub use features::{
    BuiltTraining, FEATURE_COUNT, FEATURE_NAMES, FeatureContext, LatestPublishedRule,
    NEIGHBOR_AREAS, NeighborPrices, PriceIndex, TRAINING_HISTORY_DAYS, build_feature_vector,
    build_feature_vector_indexed, build_forecast_price_index, build_training_matrix,
};
pub use model::{
    DEFAULT_RIDGE_LAMBDA, FALLBACK_PREDICTION, FitMeta, FitOptions, FittedModel, MIN_FIT_SAMPLES,
    PriceTransform, TrainingData, fit_ridge,
};
pub use pipeline::{
    CONSUMPTION_EXTENSION_WEEKS, ForecastDiagnostics, ForecastInput, ForecastOptions,
    ForecastResult, WIND_EXTENSION_WEEKS, build_forecast,
};
pub use quarter::{QuarterMap, QuarterStart, iso_millis, quarter_map};
pub use route::{
    Assessment, Confidence, FLOOR_HISTORY_DAYS, FORECAST_AREA, FORECAST_DAYS, ForecastWindow,
    REASON_FI_ONLY, REASON_FIT_DEFAULT, REASON_NO_GRID_DATA, REASON_SETTINGS_NOT_FOUND,
    REASON_ZERO_SEEDED, RouteForecast, assess, forecast_for_route,
};
pub use series::{
    BuiltSeries, ForecastSpotPoint, GridRecord, LinearFit, LinearFitError, SANITY_MARGIN_K,
    SANITY_MIN_FALLBACK_CENTS_KWH, SanityBound, ShapedPrediction, align_series,
    apply_within_day_shape, bucket_records, build_predicted_series, expand_hourly_to_quarters,
    extend_with_last_week, fit_linear, sanity_bound_from_history,
};

/// A forecast instant outside the range that Jiff supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ForecastError {
    #[error("forecast instant out of range")]
    OutOfRange,
}
