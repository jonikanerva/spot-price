//! The session routes of the setup UI under `/api/v1/me`: contract settings
//! and the chart data.

use axum::Extension;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use spot_price_domain::endpoints::PRICE_UNIT;
use spot_price_domain::price::calculate_total_prices;
use spot_price_domain::time::{current_and_next_date, utc_range_for_local_date};

use super::dto::{ChartDataDto, ErrorDto, TotalPriceDto, UserSettingsDto};
use super::input::{self, SettingsBody};
use super::price::{Internal, domain_settings, intervals, load_prices, out_of_range, resolution};
use super::session::{self, SignedIn};
use super::{AppState, wire};
use crate::store::user_settings::{self, UserSettingsRow};

/// The settings row of the user; the default row is created first when none
/// is stored.
async fn ensure(state: &AppState, user_id: &str) -> Result<UserSettingsRow, Internal> {
    user_settings::ensure_user_settings(&state.pool, user_id, state.now())
        .await
        .map_err(|error| {
            tracing::error!(code = error.code(), "user settings could not be read");
            Internal
        })
}

/// The contract settings of the signed-in user.
#[utoipa::path(
    get,
    path = "/api/v1/me/settings",
    tag = "User",
    summary = "Get user settings",
    description = "Returns the authenticated user's electricity contract settings.",
    responses((status = 200, description = "Current user settings", body = UserSettingsDto))
)]
pub async fn get_settings(
    State(state): State<AppState>,
    Extension(user): Extension<SignedIn>,
) -> Response {
    match ensure(&state, &user.user.id).await {
        Ok(row) => wire::json(StatusCode::OK, &UserSettingsDto::from(row)),
        Err(error) => error.into(),
    }
}

/// Changes the given settings fields and keeps the others.
#[utoipa::path(
    put,
    path = "/api/v1/me/settings",
    tag = "User",
    summary = "Update user settings",
    description = "Update one or more electricity contract settings. Only provided fields are changed.",
    request_body(content = inline(SettingsUpdateBody), content_type = "application/json"),
    responses(
        (status = 200, description = "Updated user settings", body = UserSettingsDto),
        (status = 400, description = "Validation error", body = ErrorDto)
    )
)]
pub async fn put_settings(
    State(state): State<AppState>,
    Extension(user): Extension<SignedIn>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !session::same_origin(&state, &headers) {
        return session::forbidden();
    }
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    let update = match input::settings_body(content_type, &body) {
        SettingsBody::Valid(update) => update,
        SettingsBody::Invalid(text) => return wire::error(StatusCode::BAD_REQUEST, &text),
        SettingsBody::Malformed => {
            return wire::body(
                StatusCode::BAD_REQUEST,
                wire::MALFORMED_TEXT,
                "Malformed JSON in request body",
            );
        }
    };
    let user_id = user.user.id;
    let current = match ensure(&state, &user_id).await {
        Ok(row) => row,
        Err(error) => return error.into(),
    };
    let next = UserSettingsRow {
        user_id,
        margin_cents_kwh: update.margin_cents_kwh.unwrap_or(current.margin_cents_kwh),
        transfer_day_cents_kwh: update
            .transfer_day_cents_kwh
            .unwrap_or(current.transfer_day_cents_kwh),
        transfer_night_cents_kwh: update
            .transfer_night_cents_kwh
            .unwrap_or(current.transfer_night_cents_kwh),
        tax_cents_kwh: update.tax_cents_kwh.unwrap_or(current.tax_cents_kwh),
        vat_percent: update.vat_percent.unwrap_or(current.vat_percent),
        night_start_hour: update
            .night_start_hour
            .map_or(current.night_start_hour, |hour| i32::from(hour.get())),
        night_end_hour: update
            .night_end_hour
            .map_or(current.night_end_hour, |hour| i32::from(hour.get())),
        timezone: update
            .timezone
            .map_or(current.timezone, |zone| zone.name().to_owned()),
        area: update
            .area
            .map_or(current.area, |area| area.code().to_owned()),
    };
    if let Err(error) = user_settings::upsert_user_settings(&state.pool, &next, state.now()).await {
        tracing::error!(code = error.code(), "user settings could not be saved");
        return wire::internal_error();
    }
    wire::json(StatusCode::OK, &UserSettingsDto::from(next))
}

/// The total prices of today and tomorrow for the chart of the setup UI.
#[utoipa::path(
    get,
    path = "/api/v1/me/chart",
    tag = "User",
    summary = "Chart data (total prices)",
    description = "Returns today's and tomorrow's total prices calculated with the user's contract settings.",
    responses((status = 200, description = "Chart data for today and tomorrow", body = ChartDataDto))
)]
pub async fn chart(
    State(state): State<AppState>,
    Extension(user): Extension<SignedIn>,
) -> Response {
    chart_answer(&state, &user.user.id)
        .await
        .unwrap_or_else(Response::from)
}

async fn chart_answer(state: &AppState, user_id: &str) -> Result<Response, Internal> {
    let row = ensure(state, user_id).await?;
    let Some(settings) = domain_settings(row) else {
        tracing::error!(
            code = "invalid_settings",
            user_id,
            "stored user settings are invalid"
        );
        return Err(Internal);
    };
    let zone = settings.timezone.time_zone();
    let dates = current_and_next_date(state.now(), zone).map_err(out_of_range)?;
    let today_range = utc_range_for_local_date(dates.today, zone).map_err(out_of_range)?;
    let tomorrow_range = utc_range_for_local_date(dates.tomorrow, zone).map_err(out_of_range)?;
    let today = load_prices(state, today_range, settings.area).await?;
    let tomorrow = load_prices(state, tomorrow_range, settings.area).await?;
    let totals = |rows| -> Result<Vec<TotalPriceDto>, Internal> {
        Ok(calculate_total_prices(&intervals(rows)?, &settings)
            .iter()
            .map(TotalPriceDto::from)
            .collect())
    };
    Ok(wire::json(
        StatusCode::OK,
        &ChartDataDto {
            today: totals(&today)?,
            tomorrow_available: !tomorrow.is_empty(),
            tomorrow: totals(&tomorrow)?,
            unit: PRICE_UNIT,
            resolution_minutes: resolution(),
        },
    ))
}

// The OpenAPI schema of the settings update body. Every field is optional.
// No doc comment: the derive would copy it into the document.
#[derive(Debug, utoipa::ToSchema)]
#[schema(rename_all = "camelCase")]
#[expect(
    dead_code,
    reason = "documents the body; input::settings_body reads it"
)]
pub struct SettingsUpdateBody {
    #[schema(minimum = 0, example = 0.5)]
    margin_cents_kwh: Option<f64>,
    #[schema(minimum = 0, example = 2.5)]
    transfer_day_cents_kwh: Option<f64>,
    #[schema(minimum = 0, example = 1.2)]
    transfer_night_cents_kwh: Option<f64>,
    #[schema(minimum = 0, example = 2.79372)]
    tax_cents_kwh: Option<f64>,
    /// VAT percentage (0-100)
    #[schema(minimum = 0, maximum = 100, example = 25.5)]
    vat_percent: Option<f64>,
    /// Night rate start hour (0-23)
    #[schema(minimum = 0, maximum = 23, example = 22)]
    night_start_hour: Option<i32>,
    /// Night rate end hour (0-23)
    #[schema(minimum = 0, maximum = 23, example = 7)]
    night_end_hour: Option<i32>,
    #[schema(example = "Europe/Helsinki")]
    timezone: Option<String>,
    #[schema(example = "FI")]
    area: Option<String>,
}
