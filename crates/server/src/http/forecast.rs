//! `GET /api/v1/price/forecast`: the FI price estimate.
//!
//! The model result does not depend on the user. The handler serves the
//! precomputed result of [`ForecastCache`] and applies the contract terms
//! and the time zone of the user per request. When no usable result exists,
//! it builds one per request (`crate::forecast_cache`).

use axum::Extension;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::Response;
use jiff::Timestamp;
use spot_price_domain::areas::Area;
use spot_price_domain::endpoints::PRICE_UNIT;
use spot_price_domain::forecast::{
    FORECAST_AREA, REASON_FI_ONLY, REASON_NO_GRID_DATA, REASON_SETTINGS_NOT_FOUND, RouteForecast,
    to_forecast_entries,
};
use spot_price_domain::price::apply_contract_terms;
use spot_price_domain::settings::UserSettings;

use super::api_key::ApiUser;
use super::dto::{ForecastBandsDto, ForecastEntryDto, ForecastResponseDto};
use super::input::{self, Query};
use super::price::{Internal, load_settings, resolution};
use super::{AppState, wire};
use crate::forecast_cache::{self, ForecastCache, Snapshot};

/// A forecast estimate for the days that Nord Pool has not published.
#[utoipa::path(
    get,
    path = "/api/v1/price/forecast",
    tag = "Forecast",
    summary = "FI price forecast (estimate)",
    description = "Returns a clearly-labelled price ESTIMATE for the days Nord Pool has not published yet, derived from public Fingrid grid data plus stored price history using simple closed-form math (no ML). Structurally distinct from the real-price endpoints — every money field is named `estimated*` and the response carries `forecast: true`. FI only; other areas return available:false. When data is insufficient the response is degraded/low-confidence rather than a confident guess.",
    security(("BearerAuth" = [])),
    params(("area" = Option<String>, Query, description = "Delivery area. The forecast is FI-only; other areas return available:false.")),
    responses(
        (status = 200, description = "Forecast estimate (check available/degraded/confidence). Non-FI areas return available:false.", body = ForecastResponseDto),
        (status = 404, description = "User settings not found", body = ForecastResponseDto)
    )
)]
pub async fn forecast(
    State(state): State<AppState>,
    Extension(user): Extension<ApiUser>,
    RawQuery(raw): RawQuery,
) -> Response {
    let area = match input::area_query(&Query::parse(raw.as_deref())) {
        Ok(area) => area,
        Err(text) => return wire::error(StatusCode::BAD_REQUEST, &text),
    };
    answer(&state, &user.0, area)
        .await
        .unwrap_or_else(Response::from)
}

/// The body of a forecast without entries.
fn unavailable(area: Area, reason: &'static str, generated_at: Timestamp) -> ForecastResponseDto {
    ForecastResponseDto {
        forecast: true,
        area: area.code().to_owned(),
        available: false,
        degraded: true,
        confidence: "low",
        reason: Some(reason),
        generated_at: wire::iso(generated_at),
        unit: PRICE_UNIT,
        resolution_minutes: resolution(),
        bands: None,
        entries: Vec::new(),
    }
}

async fn answer(state: &AppState, user_id: &str, area: Area) -> Result<Response, Internal> {
    let now = state.now();
    let Some(settings) = load_settings(state, user_id).await? else {
        return Ok(wire::json(
            StatusCode::NOT_FOUND,
            &unavailable(area, REASON_SETTINGS_NOT_FOUND, now),
        ));
    };
    if area != FORECAST_AREA {
        return Ok(wire::json(
            StatusCode::OK,
            &unavailable(area, REASON_FI_ONLY, now),
        ));
    }
    let snapshot = snapshot(state, now).await?;
    let body = respond(&snapshot, &settings, area)?;
    Ok(wire::json(StatusCode::OK, &body))
}

/// The cached result, or a result built for this request when the cache is
/// empty or holds a result whose window came from the build clock.
async fn snapshot(state: &AppState, now: Timestamp) -> Result<std::sync::Arc<Snapshot>, Internal> {
    if let Some(cached) = cached(&state.forecast) {
        cached.warn_if_stale(now);
        return Ok(cached);
    }
    match forecast_cache::build(&state.pool, now).await {
        Ok(built) => Ok(std::sync::Arc::new(built)),
        Err(error) => {
            tracing::error!(code = error.code(), "forecast data could not be read");
            Err(Internal)
        }
    }
}

fn cached(cache: &ForecastCache) -> Option<std::sync::Arc<Snapshot>> {
    cache
        .current()
        .filter(|snapshot| !snapshot.window_from_clock)
}

/// The response body of a forecast result for one user. Pure: no I/O and no
/// clock.
///
/// # Errors
///
/// Returns [`Internal`] when a quarter leaves the supported time range.
pub fn respond(
    snapshot: &Snapshot,
    settings: &UserSettings,
    area: Area,
) -> Result<ForecastResponseDto, Internal> {
    let generated_at = snapshot.generated_at;
    let (result, assessment) = match &snapshot.route {
        RouteForecast::NoGridData => {
            return Ok(unavailable(area, REASON_NO_GRID_DATA, generated_at));
        }
        RouteForecast::Ready { result, assessment } => (result, assessment),
    };
    let zone = settings.timezone.time_zone();
    let entries = to_forecast_entries(result, zone, |spot, hour| {
        let hour = u8::try_from(hour).unwrap_or(0);
        apply_contract_terms(spot, settings, hour).total_cents_kwh
    })
    .map_err(|_| {
        tracing::error!(code = "time_range", "a forecast quarter is out of range");
        Internal
    })?;
    Ok(ForecastResponseDto {
        forecast: true,
        area: area.code().to_owned(),
        available: true,
        degraded: assessment.degraded,
        confidence: assessment.confidence.as_str(),
        reason: assessment.reason,
        generated_at: wire::iso(generated_at),
        unit: PRICE_UNIT,
        resolution_minutes: resolution(),
        bands: Some(ForecastBandsDto::shipped()),
        entries: entries.iter().map(ForecastEntryDto::from).collect(),
    })
}
