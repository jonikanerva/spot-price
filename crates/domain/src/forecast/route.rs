//! The pure decisions of the forecast endpoint: the time window, the
//! degraded state, and the reason texts. The server loads the data.

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};

use crate::areas::Area;

use super::ForecastError;
use super::pipeline::{
    ForecastDiagnostics, ForecastInput, ForecastOptions, ForecastResult, build_forecast,
};
use super::series::sanity_bound_from_history;

/// The only forecast area: Fingrid is the Finnish grid operator.
pub const FORECAST_AREA: Area = Area::Fi;
/// The zone of the Nord Pool delivery day (00:00 to 24:00 CET/CEST). The
/// served shape profile reads calendar days of this zone.
pub const DELIVERY_DAY_TIME_ZONE: &str = "Europe/Oslo";
/// The predicted series covers this many 24-hour periods.
pub const FORECAST_DAYS: i64 = 3;
/// The price history before the series start that the fit, the lags, and the
/// sanity bound read, in 24-hour periods.
pub const FLOOR_HISTORY_DAYS: i64 = 30;

pub const REASON_SETTINGS_NOT_FOUND: &str = "User settings not found";
pub const REASON_FI_ONLY: &str = "Forecast available for FI only";
pub const REASON_NO_GRID_DATA: &str = "No Fingrid grid data available for the forecast window";
pub const REASON_FIT_DEFAULT: &str =
    "Insufficient price history to fit the model; using a default estimate";
pub const REASON_ZERO_SEEDED: &str = "Some quarters had no input data and were zero-seeded";

/// The instants that bound the reads and the predicted series. The values
/// are not floored; the pipeline floors the series bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForecastWindow {
    /// One quarter after the last published FI price, else `now`.
    pub series_start: Timestamp,
    /// `series_start` + [`FORECAST_DAYS`] × 24 h, exclusive.
    pub series_end: Timestamp,
    /// `series_start` − [`FLOOR_HISTORY_DAYS`] × 24 h. Read prices in
    /// `[history_start, series_start)` and Fingrid data in
    /// `[history_start, series_end)`.
    pub history_start: Timestamp,
}

impl ForecastWindow {
    /// The window for the latest published FI delivery start.
    ///
    /// # Errors
    ///
    /// Returns [`ForecastError::OutOfRange`] when an instant leaves the Jiff
    /// range.
    pub fn new(last_published: Option<Timestamp>, now: Timestamp) -> Result<Self, ForecastError> {
        let series_start = match last_published {
            Some(last) => add(last, SignedDuration::from_mins(15))?,
            None => now,
        };
        Ok(Self {
            series_start,
            series_end: add(series_start, SignedDuration::from_hours(FORECAST_DAYS * 24))?,
            history_start: add(
                series_start,
                SignedDuration::from_hours(-FLOOR_HISTORY_DAYS * 24),
            )?,
        })
    }
}

fn add(at: Timestamp, duration: SignedDuration) -> Result<Timestamp, ForecastError> {
    at.checked_add(duration)
        .map_err(|_| ForecastError::OutOfRange)
}

/// The confidence of a forecast response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    Normal,
    Low,
}

impl Confidence {
    /// The wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Low => "low",
        }
    }
}

/// The degraded state of a forecast result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assessment {
    pub degraded: bool,
    pub confidence: Confidence,
    /// Present only when degraded.
    pub reason: Option<&'static str>,
}

/// Degraded when the fit used the fallback or a quarter was zero-seeded. Tail
/// extension and missing neighbours do not degrade the result.
#[must_use]
pub const fn assess(diagnostics: &ForecastDiagnostics) -> Assessment {
    if diagnostics.fit_used_default {
        Assessment {
            degraded: true,
            confidence: Confidence::Low,
            reason: Some(REASON_FIT_DEFAULT),
        }
    } else if diagnostics.zero_seeded_quarters > 0 {
        Assessment {
            degraded: true,
            confidence: Confidence::Low,
            reason: Some(REASON_ZERO_SEEDED),
        }
    } else {
        Assessment {
            degraded: false,
            confidence: Confidence::Normal,
            reason: None,
        }
    }
}

/// The outcome of the forecast endpoint for FI.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteForecast {
    /// No Fingrid record at all: answer `available: false` with
    /// [`REASON_NO_GRID_DATA`].
    NoGridData,
    Ready {
        result: ForecastResult,
        assessment: Assessment,
    },
}

/// False when all four Fingrid inputs are empty. Then the endpoint answers
/// [`RouteForecast::NoGridData`]; the backtest skips such an origin.
#[must_use]
pub const fn has_grid_data(input: &ForecastInput<'_>) -> bool {
    !(input.wind_forecast.is_empty()
        && input.wind_actual.is_empty()
        && input.consumption_forecast.is_empty()
        && input.consumption_actual.is_empty())
}

/// The endpoint sequence after the reads: the grid-data check, the sanity
/// bound from the FI history, the served pipeline, and the assessment.
/// `delivery_zone` must be [`DELIVERY_DAY_TIME_ZONE`].
#[must_use]
pub fn forecast_for_route(input: &ForecastInput<'_>, delivery_zone: &TimeZone) -> RouteForecast {
    if !has_grid_data(input) {
        return RouteForecast::NoGridData;
    }
    let opts = ForecastOptions {
        sanity_bound: sanity_bound_from_history(input.spot_prices),
        ..ForecastOptions::served(delivery_zone)
    };
    let result = build_forecast(input, &opts);
    let assessment = assess(&result.diagnostics);
    RouteForecast::Ready { result, assessment }
}
