//! Forecast response entries: the user's contract terms on each predicted
//! spot quarter, and local times in the user's time zone.

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};

use super::ForecastError;
use super::pipeline::ForecastResult;

/// The band of one entry. Contract terms are monotone in spot, so
/// `total_low <= total <= total_high` holds when `spot_low <= spot <= spot_high`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntryBand {
    pub spot_low_cents_kwh: f64,
    pub spot_high_cents_kwh: f64,
    pub total_low_cents_kwh: f64,
    pub total_high_cents_kwh: f64,
}

/// One quarter of the forecast response.
#[derive(Debug, Clone, PartialEq)]
pub struct ForecastEntry {
    pub start: Timestamp,
    pub end: Timestamp,
    /// `YYYY-MM-DDTHH:MM:SS±HH:MM` in the user's time zone.
    pub local_start: String,
    pub local_end: String,
    pub estimated_spot_cents_kwh: f64,
    pub estimated_total_cents_kwh: f64,
    pub band: Option<EntryBand>,
}

fn local(at: Timestamp, time_zone: &TimeZone) -> String {
    at.to_zoned(time_zone.clone())
        .strftime("%Y-%m-%dT%H:%M:%S%:z")
        .to_string()
}

/// The response entries of `result`. `total_cents_kwh(spot, local_hour)`
/// applies the contract terms to a spot price in c/kWh; `local_hour` is the
/// hour (0–23) of the quarter start in `time_zone`.
///
/// # Errors
///
/// Returns [`ForecastError::OutOfRange`] when a quarter leaves the Jiff range.
pub fn to_forecast_entries<F>(
    result: &ForecastResult,
    time_zone: &TimeZone,
    total_cents_kwh: F,
) -> Result<Vec<ForecastEntry>, ForecastError>
where
    F: Fn(f64, i8) -> f64,
{
    result
        .series
        .iter()
        .map(|point| {
            let start = point.start.timestamp()?;
            let end = start
                .checked_add(SignedDuration::from_mins(15))
                .map_err(|_| ForecastError::OutOfRange)?;
            let hour = start.to_zoned(time_zone.clone()).hour();
            let spot = point.estimated_spot_cents_kwh;
            Ok(ForecastEntry {
                start,
                end,
                local_start: local(start, time_zone),
                local_end: local(end, time_zone),
                estimated_spot_cents_kwh: spot,
                estimated_total_cents_kwh: total_cents_kwh(spot, hour),
                band: point.band.map(|band| EntryBand {
                    spot_low_cents_kwh: band.low,
                    spot_high_cents_kwh: band.high,
                    total_low_cents_kwh: total_cents_kwh(band.low, hour),
                    total_high_cents_kwh: total_cents_kwh(band.high, hour),
                }),
            })
        })
        .collect()
}
