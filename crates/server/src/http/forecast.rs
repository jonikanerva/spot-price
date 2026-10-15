//! `GET /api/v1/price/forecast`: the FI price estimate.
//!
//! The handler reads the stored prices and Fingrid series and runs the pure
//! domain forecast on the blocking thread pool, because one run takes
//! milliseconds of CPU time.

use std::collections::BTreeMap;

use axum::Extension;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::Response;
use jiff::Timestamp;
use spot_price_domain::areas::Area;
use spot_price_domain::endpoints::PRICE_UNIT;
use spot_price_domain::forecast::{
    FORECAST_AREA, ForecastInput, ForecastWindow, GridRecord, NEIGHBOR_AREAS, NeighborPrices,
    QuarterMap, REASON_FI_ONLY, REASON_NO_GRID_DATA, REASON_SETTINGS_NOT_FOUND, RouteForecast,
    forecast_for_route, quarter_map, to_forecast_entries,
};
use spot_price_domain::price::{apply_contract_terms, eur_mwh_to_cents_kwh};
use spot_price_domain::settings::UserSettings;

use super::api_key::ApiUser;
use super::dto::{ForecastBandsDto, ForecastEntryDto, ForecastResponseDto};
use super::input::{self, Query};
use super::price::{Internal, load_settings, resolution};
use super::{AppState, wire};
use crate::store::fingrid::{
    self, DATASET_CONSUMPTION_ACTUAL, DATASET_CONSUMPTION_FORECAST, DATASET_WIND_ACTUAL,
    DATASET_WIND_FORECAST, FingridRecord,
};
use crate::store::prices::{self, PriceRow};

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
    let inputs = load_inputs(state, now).await?;
    let body = tokio::task::spawn_blocking(move || compute(&inputs, &settings, area, now))
        .await
        .map_err(|_| {
            tracing::error!(code = "forecast_task", "the forecast task stopped");
            Internal
        })??;
    Ok(wire::json(StatusCode::OK, &body))
}

/// The stored data of one forecast run.
#[derive(Debug, Clone)]
pub struct Inputs {
    pub window: ForecastWindow,
    pub spot: QuarterMap,
    pub neighbors: NeighborPrices,
    pub wind_forecast: Vec<GridRecord>,
    pub wind_actual: Vec<GridRecord>,
    pub consumption_forecast: Vec<GridRecord>,
    pub consumption_actual: Vec<GridRecord>,
}

fn spot_map(rows: &[PriceRow]) -> QuarterMap {
    quarter_map(
        rows.iter()
            .map(|row| (row.delivery_start, eur_mwh_to_cents_kwh(row.price_eur_mwh))),
    )
}

fn grid(records: Vec<FingridRecord>) -> Vec<GridRecord> {
    records
        .into_iter()
        .map(|record| GridRecord {
            start: record.start_time,
            value: record.value,
        })
        .collect()
}

fn read_failed(error: crate::db::StoreError) -> Internal {
    tracing::error!(code = error.code(), "forecast data could not be read");
    Internal
}

/// Reads the inputs like the TypeScript route: FI prices and neighbour
/// prices in `[history_start, series_start)`, the latest forecast vintage
/// of each target, and the actuals in `[history_start, series_end)`.
///
/// # Errors
///
/// Returns [`Internal`] when a query fails.
pub async fn load_inputs(state: &AppState, now: Timestamp) -> Result<Inputs, Internal> {
    let pool = &state.pool;
    let last = prices::get_latest_delivery_start(pool, FORECAST_AREA)
        .await
        .map_err(read_failed)?;
    let window = ForecastWindow::new(last, now).map_err(|_| {
        tracing::error!(code = "time_range", "the forecast window is out of range");
        Internal
    })?;
    let (from, series_start, until) =
        (window.history_start, window.series_start, window.series_end);
    let fi = prices::get_prices_by_range(pool, from, series_start, FORECAST_AREA)
        .await
        .map_err(read_failed)?;
    let neighbors = prices::get_prices_by_areas(pool, from, series_start, &NEIGHBOR_AREAS)
        .await
        .map_err(read_failed)?;
    let (wind_forecast, wind_actual, consumption_forecast, consumption_actual) = tokio::try_join!(
        fingrid::get_fingrid_forecast_vintages_latest(pool, DATASET_WIND_FORECAST, from, until),
        fingrid::get_fingrid_records_by_range(pool, DATASET_WIND_ACTUAL, from, until),
        fingrid::get_fingrid_forecast_vintages_latest(
            pool,
            DATASET_CONSUMPTION_FORECAST,
            from,
            until
        ),
        fingrid::get_fingrid_records_by_range(pool, DATASET_CONSUMPTION_ACTUAL, from, until),
    )
    .map_err(read_failed)?;
    Ok(Inputs {
        window,
        spot: spot_map(&fi),
        neighbors: neighbors
            .iter()
            .map(|(area, rows)| (*area, spot_map(rows)))
            .collect::<BTreeMap<_, _>>(),
        wind_forecast: grid(wind_forecast),
        wind_actual: grid(wind_actual),
        consumption_forecast: grid(consumption_forecast),
        consumption_actual: grid(consumption_actual),
    })
}

/// The response body of a forecast run. Pure: no I/O and no clock.
///
/// # Errors
///
/// Returns [`Internal`] when a quarter leaves the supported time range.
pub fn compute(
    inputs: &Inputs,
    settings: &UserSettings,
    area: Area,
    generated_at: Timestamp,
) -> Result<ForecastResponseDto, Internal> {
    let input = ForecastInput {
        spot_prices: &inputs.spot,
        neighbor_prices: &inputs.neighbors,
        wind_forecast: &inputs.wind_forecast,
        wind_actual: &inputs.wind_actual,
        consumption_forecast: &inputs.consumption_forecast,
        consumption_actual: &inputs.consumption_actual,
        series_start: inputs.window.series_start,
        series_end: inputs.window.series_end,
    };
    let (result, assessment) = match forecast_for_route(&input) {
        RouteForecast::NoGridData => {
            return Ok(unavailable(area, REASON_NO_GRID_DATA, generated_at));
        }
        RouteForecast::Ready { result, assessment } => (result, assessment),
    };
    let zone = settings.timezone.time_zone();
    let entries = to_forecast_entries(&result, zone, |spot, hour| {
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
