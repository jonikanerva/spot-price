//! The session routes of the setup UI under `/api/v1/me`: contract settings,
//! the versions of the contract terms, and the chart data.

use axum::Extension;
use axum::body::Bytes;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use spot_price_domain::endpoints::PRICE_UNIT;
use spot_price_domain::price::calculate_scheduled_total_prices;
use spot_price_domain::query::parse_valid_from;
use spot_price_domain::settings::{MAX_TERMS_VERSIONS, applicable_version};
use spot_price_domain::time::{
    UserTimeZone, current_and_next_date, local_date, utc_range_for_local_date,
};

use super::dto::{
    ChartDataDto, ContractTermsVersionsDto, ErrorDto, TotalPriceDto, UserSettingsDto,
};
use super::input::{self, SettingsBody};
use super::price::{
    Internal, SettingsFailure, intervals, load_prices, out_of_range, resolution, schedule_of,
    tomorrow_available,
};
use super::session::{self, SignedIn};
use super::{AppState, wire};
use crate::store::user_settings::{self, DeleteOutcome, SaveError, UserSettingsRow};

/// The 404 text of a version delete for a date without a version.
const VERSION_NOT_FOUND: &str = "Contract terms version not found";
/// The 400 text of a delete of the only version.
const LAST_VERSION: &str = "Cannot delete the only contract terms version";

/// The settings row of the user; the default row and its first terms
/// version are created first when none is stored.
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
    description = "Returns the authenticated user's electricity contract settings: the timezone, the area, and the contract terms that apply today. The margin, the transfer prices, and the tax include VAT, as on an invoice; the price routes return them without VAT. validFrom is the date of the terms version that holds those terms.",
    responses((status = 200, description = "Current user settings", body = UserSettingsDto))
)]
pub async fn get_settings(
    State(state): State<AppState>,
    Extension(user): Extension<SignedIn>,
) -> Response {
    settings_answer(&state, user.user.id)
        .await
        .unwrap_or_else(Response::from)
}

async fn settings_answer(state: &AppState, user_id: String) -> Result<Response, Internal> {
    let row = ensure(state, &user_id).await?;
    let fail = |failure: SettingsFailure| {
        failure.log(&user_id);
        Internal
    };
    let zone = UserTimeZone::parse(&row.timezone).map_err(|_| fail(SettingsFailure::Invalid))?;
    let today = local_date(state.now(), zone.time_zone());
    let terms = applicable_version(row.versions.iter(), |version| version.valid_from, today)
        .ok_or_else(|| fail(SettingsFailure::NoVersion))?;
    let body = UserSettingsDto::new(user_id.clone(), row.timezone, row.area, terms);
    Ok(wire::json(StatusCode::OK, &body))
}

/// Changes the given settings fields and keeps the others.
#[utoipa::path(
    put,
    path = "/api/v1/me/settings",
    tag = "User",
    summary = "Update user settings",
    description = "Update one or more electricity contract settings. Only provided fields are changed. The margin, the transfer prices, and the tax include VAT, as on an invoice; vatPercent applies to the spot price only. The timezone and the area have one current value. The contract terms are versioned: the request saves the terms version that applies from validFrom (default: today in the user's timezone). A term that the request does not give keeps the value that applies on validFrom. A version with the same validFrom is replaced. No version is created when the terms equal the terms that already apply on validFrom. The response has the terms that apply on validFrom and the validFrom of their version. A user can hold at most 100 versions.",
    request_body(content = inline(SettingsUpdateBody), content_type = "application/json"),
    responses(
        (status = 200, description = "Updated user settings", body = UserSettingsDto),
        (status = 400, description = "Validation error, or the version limit is reached", body = ErrorDto)
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
    let request = match input::settings_body(content_type, &body) {
        SettingsBody::Valid(request) => request,
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
    if let Err(error) = ensure(&state, &user_id).await {
        return error.into();
    }
    let saved = user_settings::save_settings(
        &state.pool,
        &user_id,
        &request.update,
        request.valid_from,
        state.now(),
    )
    .await;
    match saved {
        Ok(saved) => wire::json(
            StatusCode::OK,
            &UserSettingsDto::new(user_id, saved.timezone, saved.area, &saved.applied),
        ),
        Err(SaveError::TooManyVersions) => wire::error(
            StatusCode::BAD_REQUEST,
            &format!("Too many contract terms versions: the maximum is {MAX_TERMS_VERSIONS}"),
        ),
        Err(SaveError::InvalidSettings) => {
            SettingsFailure::Invalid.log(&user_id);
            wire::internal_error()
        }
        Err(SaveError::NoVersion) => {
            SettingsFailure::NoVersion.log(&user_id);
            wire::internal_error()
        }
        Err(SaveError::Store(error)) => {
            tracing::error!(code = error.code(), "user settings could not be saved");
            wire::internal_error()
        }
    }
}

/// Every contract terms version of the signed-in user.
#[utoipa::path(
    get,
    path = "/api/v1/me/settings/versions",
    tag = "User",
    summary = "List contract terms versions",
    description = "Returns every contract terms version of the authenticated user, ascending by validFrom. An interval uses the latest version with a validFrom on or before the local date of the interval start; an interval before the first version uses the first version.",
    responses((status = 200, description = "The contract terms versions", body = ContractTermsVersionsDto))
)]
pub async fn list_versions(
    State(state): State<AppState>,
    Extension(user): Extension<SignedIn>,
) -> Response {
    let user_id = user.user.id;
    match ensure(&state, &user_id).await {
        Ok(row) if row.versions.is_empty() => {
            SettingsFailure::NoVersion.log(&user_id);
            wire::internal_error()
        }
        Ok(row) => wire::json(
            StatusCode::OK,
            &ContractTermsVersionsDto::new(&row.versions),
        ),
        Err(error) => error.into(),
    }
}

/// Deletes one contract terms version, but never the only one.
#[utoipa::path(
    delete,
    path = "/api/v1/me/settings/versions/{validFrom}",
    tag = "User",
    summary = "Delete a contract terms version",
    description = "Deletes the contract terms version with this validFrom. The intervals of that version then use the version before it, or the next version when the deleted version was the first. The only remaining version cannot be deleted.",
    params(("validFrom" = String, Path, description = "The validFrom of the version, YYYY-MM-DD.")),
    responses(
        (status = 200, description = "The remaining contract terms versions", body = ContractTermsVersionsDto),
        (status = 400, description = "Invalid date, or the version is the only version", body = ErrorDto),
        (status = 404, description = "No version has this validFrom", body = ErrorDto)
    )
)]
pub async fn delete_version(
    State(state): State<AppState>,
    Extension(user): Extension<SignedIn>,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
) -> Response {
    if !session::same_origin(&state, &headers) {
        return session::forbidden();
    }
    // A path that is not UTF-8 after percent-decoding is not a date.
    let text = path.map_or_default(|Path(text)| text);
    let valid_from = match parse_valid_from(&text) {
        Ok(date) => date,
        Err(issue) => return wire::error(StatusCode::BAD_REQUEST, &issue.to_string()),
    };
    match user_settings::delete_version(&state.pool, &user.user.id, valid_from).await {
        Ok(DeleteOutcome::Deleted(versions)) => {
            wire::json(StatusCode::OK, &ContractTermsVersionsDto::new(&versions))
        }
        Ok(DeleteOutcome::NotFound) => wire::error(StatusCode::NOT_FOUND, VERSION_NOT_FOUND),
        Ok(DeleteOutcome::LastVersion) => wire::error(StatusCode::BAD_REQUEST, LAST_VERSION),
        Err(error) => {
            tracing::error!(code = error.code(), "a terms version could not be deleted");
            wire::internal_error()
        }
    }
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
    let schedule = schedule_of(&row).map_err(|failure| {
        failure.log(user_id);
        Internal
    })?;
    let zone = schedule.timezone().time_zone();
    let dates = current_and_next_date(state.now(), zone).map_err(out_of_range)?;
    let today_range = utc_range_for_local_date(dates.today, zone).map_err(out_of_range)?;
    let tomorrow_range = utc_range_for_local_date(dates.tomorrow, zone).map_err(out_of_range)?;
    let today = load_prices(state, today_range, schedule.area()).await?;
    let tomorrow = load_prices(state, tomorrow_range, schedule.area()).await?;
    let totals = |rows| -> Result<Vec<TotalPriceDto>, Internal> {
        Ok(
            calculate_scheduled_total_prices(&intervals(rows)?, &schedule)
                .iter()
                .map(TotalPriceDto::from)
                .collect(),
        )
    };
    Ok(wire::json(
        StatusCode::OK,
        &ChartDataDto {
            today: totals(&today)?,
            tomorrow_available: tomorrow_available(&tomorrow, tomorrow_range),
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
    /// Retailer margin (c/kWh), VAT included, as on the electricity sales invoice.
    #[schema(minimum = 0, example = 0.5)]
    margin_incl_vat_cents_kwh: Option<f64>,
    /// Day transfer price (c/kWh), VAT included, as on the grid invoice.
    #[schema(minimum = 0, example = 2.96)]
    transfer_day_incl_vat_cents_kwh: Option<f64>,
    /// Night transfer price (c/kWh), VAT included, as on the grid invoice.
    #[schema(minimum = 0, example = 1.56)]
    transfer_night_incl_vat_cents_kwh: Option<f64>,
    /// Electricity tax plus the security of supply fee (c/kWh), VAT included, as on the grid invoice.
    #[schema(minimum = 0, example = 2.9179)]
    tax_incl_vat_cents_kwh: Option<f64>,
    /// VAT percentage (0-100) of the spot price. The other prices already include it.
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
    /// The local date (YYYY-MM-DD, 1970-01-01 or later) from which the contract terms of this request apply. Default: today in the user's timezone.
    #[schema(format = Date, example = "2026-12-01")]
    valid_from: Option<String>,
}
