//! The FI price forecast: a closed-form ridge regression over Fingrid grid
//! data, FI and neighbour price lags, and the UTC calendar. Everything here
//! is pure; the caller passes all data and instants in. Series are keyed by
//! UTC quarters. The forecast predicts spot c/kWh.

mod conformal;
mod conformal_artifact;
mod correction;
mod entries;
mod features;
mod holiday;
mod issuance;
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
pub use correction::{
    BASELINE_FEATURE_NAME, CORRECTION_HISTORY_DAYS, CorrectionDiagnostics, CorrectionExample,
    CorrectionExamples, Decomposition, HOLIDAY_FEATURE_NAME, QuarterCorrection,
    build_correction_examples, correction_feature_names,
};
pub use entries::{EntryBand, ForecastEntry, to_forecast_entries};
pub use features::{
    BuiltTraining, FEATURE_COUNT, FEATURE_NAMES, FeatureContext, LatestPublishedRule,
    NEIGHBOR_AREAS, NeighborPrices, PrevDayBasis, PriceIndex, TRAINING_HISTORY_DAYS,
    build_feature_vector, build_feature_vector_indexed, build_forecast_price_index,
    build_price_index_with, build_training_matrix,
};
pub use holiday::{easter_sunday, is_finnish_public_holiday};
pub use issuance::{
    ExcludedExamples, GridVintage, ISSUANCE_EXTRA_FEATURE_NAMES, ISSUANCE_FEATURE_COUNT,
    InputSource, IssuanceExamples, IssuanceHistory, IssuanceSummary, LagAvailability,
    PUBLICATION_HOUR_UTC, TrainingExample, TrainingRule, build_issuance_examples,
    issuance_feature_names, publication_horizon,
};
pub use model::{
    DEFAULT_RIDGE_LAMBDA, FALLBACK_PREDICTION, FitMeta, FitOptions, FittedModel, MIN_FIT_SAMPLES,
    PriceTransform, TrainingData, fit_ridge, fit_ridge_weighted,
};
pub use pipeline::{
    CONSUMPTION_EXTENSION_WEEKS, FeatureRows, ForecastDiagnostics, ForecastInput, ForecastOptions,
    ForecastResult, GridGapRule, SERVED_SHAPE_WEEKS, WIND_EXTENSION_WEEKS, build_forecast,
    feature_rows,
};
pub use quarter::{QuarterMap, QuarterStart, iso_millis, quarter_map};
pub use route::{
    Assessment, Confidence, DELIVERY_DAY_TIME_ZONE, FLOOR_HISTORY_DAYS, FORECAST_AREA,
    FORECAST_DAYS, ForecastWindow, REASON_FI_ONLY, REASON_FIT_DEFAULT, REASON_NO_GRID_DATA,
    REASON_SETTINGS_NOT_FOUND, REASON_ZERO_SEEDED, RouteForecast, assess, forecast_for_route,
    has_grid_data,
};
pub use series::{
    BuiltSeries, ForecastSpotPoint, GAP_FILL_LOOKBACK_WEEKS, GridRecord, LinearFit, LinearFitError,
    PREVIOUS_PUBLISHED_DAY_LOOKBACK_DAYS, ProfileValues, SANITY_MARGIN_K,
    SANITY_MIN_FALLBACK_CENTS_KWH, SanityBound, ShapeProfile, ShapedPrediction, align_series,
    apply_within_day_shape, apply_within_day_shape_with, bucket_records, build_predicted_series,
    expand_hourly_to_quarters, extend_with_last_week, fill_gaps_from_earlier_weeks, fit_linear,
    profile_values, sanity_bound_from_history,
};

/// A forecast instant outside the range that Jiff supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ForecastError {
    #[error("forecast instant out of range")]
    OutOfRange,
}
