//! The price routes: `/api/public/spot` and `/api/v1/price/*` except the
//! forecast. A query error answers 400 before the settings lookup, like the
//! zod validator of the TypeScript service.

use axum::Extension;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::Response;
use jiff::Timestamp;
use spot_price_domain::areas::Area;
use spot_price_domain::endpoints::{
    NO_CURRENT_PRICE, PRICE_UNIT, PriceList, RESOLUTION_MINUTES, SETTINGS_NOT_FOUND,
    cheapest_window, current_price,
};
use spot_price_domain::price::{
    DeliveryPeriod, PriceInterval, calculate_total_price, calculate_total_prices,
};
use spot_price_domain::settings::{Hour, UserId, UserSettings};
use spot_price_domain::time::{
    CurrentAndNextDate, UserTimeZone, UtcRange, covers_range, current_and_next_date, local_date,
    utc_range_for_local_date, utc_range_for_local_date_span,
};

use super::api_key::ApiUser;
use super::dto::{
    ErrorDto, PriceListDto, PriceWindowDto, PublicSpotDto, SpotPriceEntryDto, TotalPriceDto,
};
use super::input::{self, Query};
use super::{AppState, wire};
use crate::store::prices::{self, PriceRow};
use crate::store::user_settings::{self, UserSettingsRow};

/// A failure that answers the generic 500. The cause is logged where it
/// happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Internal;

impl From<Internal> for Response {
    fn from(_: Internal) -> Self {
        wire::internal_error()
    }
}

/// The domain settings of a stored row, or `None` when a stored value is
/// outside the domain rules.
#[must_use]
pub fn domain_settings(row: UserSettingsRow) -> Option<UserSettings> {
    Some(UserSettings {
        night_start_hour: Hour::try_from(row.night_start_hour).ok()?,
        night_end_hour: Hour::try_from(row.night_end_hour).ok()?,
        timezone: UserTimeZone::parse(&row.timezone).ok()?,
        area: row.area.parse().ok()?,
        user_id: UserId::new(row.user_id),
        margin_cents_kwh: row.margin_cents_kwh,
        transfer_day_cents_kwh: row.transfer_day_cents_kwh,
        transfer_night_cents_kwh: row.transfer_night_cents_kwh,
        tax_cents_kwh: row.tax_cents_kwh,
        vat_percent: row.vat_percent,
    })
}

/// The settings of a user: `Ok(None)` when no row is stored.
///
/// # Errors
///
/// Returns [`Internal`] when the query fails or the stored row breaks the
/// domain rules.
pub async fn load_settings(
    state: &AppState,
    user_id: &str,
) -> Result<Option<UserSettings>, Internal> {
    let row = user_settings::get_user_settings(&state.pool, user_id)
        .await
        .map_err(|error| {
            tracing::error!(code = error.code(), "user settings could not be read");
            Internal
        })?;
    let Some(row) = row else {
        return Ok(None);
    };
    domain_settings(row).map(Some).ok_or_else(|| {
        tracing::error!(
            code = "invalid_settings",
            user_id,
            "stored user settings are invalid"
        );
        Internal
    })
}

/// A date or range computation left the supported range.
pub fn out_of_range<E>(_: E) -> Internal {
    tracing::error!(code = "time_range", "a local date is out of range");
    Internal
}

/// The prices of `area` with a start in `range`, as domain intervals.
///
/// # Errors
///
/// Returns [`Internal`] when the query fails or a stored row has an empty
/// delivery interval.
pub async fn load_prices(
    state: &AppState,
    range: UtcRange,
    area: Area,
) -> Result<Vec<PriceRow>, Internal> {
    prices::get_prices_by_range(&state.pool, range.start, range.end, area)
        .await
        .map_err(|error| {
            tracing::error!(code = error.code(), "prices could not be read");
            Internal
        })
}

/// The `tomorrowAvailable` value: true only when the stored rows cover the
/// whole local day of `tomorrow`. Before the day-ahead auction, the store
/// already has the first hour of the next Helsinki day.
#[must_use]
pub fn tomorrow_available(rows: &[PriceRow], tomorrow: UtcRange) -> bool {
    covers_range(
        rows.iter()
            .map(|row| (row.delivery_start, row.delivery_end)),
        tomorrow,
    )
}

/// The domain intervals of stored price rows.
///
/// # Errors
///
/// Returns [`Internal`] when a row ends at or before its start.
pub fn intervals(rows: &[PriceRow]) -> Result<Vec<PriceInterval>, Internal> {
    rows.iter()
        .map(|row| {
            Ok(PriceInterval {
                period: DeliveryPeriod::new(row.delivery_start, row.delivery_end).map_err(
                    |_| {
                        tracing::error!(
                            code = "invalid_row",
                            "a stored price has an empty interval"
                        );
                        Internal
                    },
                )?,
                price_eur_mwh: row.price_eur_mwh,
                area: row.area,
            })
        })
        .collect()
}

fn dates(now: Timestamp, zone: &UserTimeZone) -> Result<CurrentAndNextDate, Internal> {
    current_and_next_date(now, zone.time_zone()).map_err(out_of_range)
}

/// The UTC range from the start of today to the end of tomorrow.
fn today_and_tomorrow(now: Timestamp, zone: &UserTimeZone) -> Result<UtcRange, Internal> {
    let dates = dates(now, zone)?;
    let today = utc_range_for_local_date(dates.today, zone.time_zone()).map_err(out_of_range)?;
    let tomorrow =
        utc_range_for_local_date(dates.tomorrow, zone.time_zone()).map_err(out_of_range)?;
    Ok(UtcRange {
        start: today.start,
        end: tomorrow.end,
    })
}

fn settings_not_found() -> Response {
    wire::error(StatusCode::NOT_FOUND, SETTINGS_NOT_FOUND)
}

/// The answer of a handler: a response either way.
type Answer = Result<Response, Internal>;

fn finish(answer: Answer) -> Response {
    answer.unwrap_or_else(Response::from)
}

fn price_list(list: &PriceList) -> Response {
    wire::json(StatusCode::OK, &PriceListDto::from(list))
}

// ---- Public spot -----------------------------------------------------------------

/// Public spot prices of today and tomorrow, without authentication.
#[utoipa::path(
    get,
    path = "/api/public/spot",
    tag = "Public",
    summary = "Public spot prices (today + tomorrow)",
    description = "Returns raw spot prices in c/kWh for the requested delivery area. No authentication required.",
    params(("area" = Option<String>, Query, description = "Nord Pool delivery area code")),
    responses(
        (status = 200, description = "Spot prices for today and tomorrow (if available)", body = PublicSpotDto)
    )
)]
pub async fn public_spot(State(state): State<AppState>, RawQuery(raw): RawQuery) -> Response {
    let area = match input::area_query(&Query::parse(raw.as_deref())) {
        Ok(area) => area,
        Err(text) => return wire::error(StatusCode::BAD_REQUEST, &text),
    };
    finish(public_spot_answer(&state, area).await)
}

async fn public_spot_answer(state: &AppState, area: Area) -> Answer {
    let zone = UserTimeZone::for_area(area).map_err(out_of_range)?;
    let dates = dates(state.now(), &zone)?;
    let today_range =
        utc_range_for_local_date(dates.today, zone.time_zone()).map_err(out_of_range)?;
    let tomorrow_range =
        utc_range_for_local_date(dates.tomorrow, zone.time_zone()).map_err(out_of_range)?;
    let today = load_prices(state, today_range, area).await?;
    let tomorrow = load_prices(state, tomorrow_range, area).await?;
    Ok(wire::json(
        StatusCode::OK,
        &PublicSpotDto {
            area: area.code().to_owned(),
            today: today.iter().map(SpotPriceEntryDto::from).collect(),
            tomorrow_available: tomorrow_available(&tomorrow, tomorrow_range),
            tomorrow: tomorrow.iter().map(SpotPriceEntryDto::from).collect(),
            unit: PRICE_UNIT,
            resolution_minutes: resolution(),
        },
    ))
}

/// The `resolutionMinutes` value of the responses.
#[must_use]
pub fn resolution() -> i32 {
    i32::try_from(RESOLUTION_MINUTES).unwrap_or(i32::MAX)
}

// ---- Current price -----------------------------------------------------------

/// The total price of the current 15-minute interval.
#[utoipa::path(
    get,
    path = "/api/v1/price/now",
    tag = "Price",
    summary = "Current total price",
    description = "Returns the total price breakdown for the current 15-minute delivery interval.",
    security(("BearerAuth" = [])),
    responses(
        (status = 200, description = "Current total price with full breakdown", body = TotalPriceDto),
        (status = 404, description = "No current price available or settings not found", body = ErrorDto)
    )
)]
pub async fn now(State(state): State<AppState>, Extension(user): Extension<ApiUser>) -> Response {
    finish(now_answer(&state, &user.0).await)
}

async fn now_answer(state: &AppState, user_id: &str) -> Answer {
    let Some(settings) = load_settings(state, user_id).await? else {
        return Ok(settings_not_found());
    };
    let now = state.now();
    let range = today_and_tomorrow(now, &settings.timezone)?;
    let rows = load_prices(state, range, settings.area).await?;
    let intervals = intervals(&rows)?;
    Ok(match current_price(&intervals, now) {
        Some(price) => wire::json(
            StatusCode::OK,
            &TotalPriceDto::from(&calculate_total_price(price, &settings)),
        ),
        None => wire::error(StatusCode::NOT_FOUND, NO_CURRENT_PRICE),
    })
}

// ---- Price lists ---------------------------------------------------------------

/// The total prices of today.
#[utoipa::path(
    get,
    path = "/api/v1/price/today",
    tag = "Price",
    summary = "Today's hourly total prices",
    description = "Returns all total prices for today, calculated with the user's contract settings.",
    security(("BearerAuth" = [])),
    responses(
        (status = 200, description = "Today's prices", body = PriceListDto),
        (status = 404, description = "User settings not found", body = ErrorDto)
    )
)]
pub async fn today(State(state): State<AppState>, Extension(user): Extension<ApiUser>) -> Response {
    finish(list_answer(&state, &user.0, ListKind::Today).await)
}

/// The total prices of tomorrow, when published.
#[utoipa::path(
    get,
    path = "/api/v1/price/tomorrow",
    tag = "Price",
    summary = "Tomorrow's hourly total prices",
    description = "Returns tomorrow's total prices if available. Prices are typically published after 12:00 UTC.",
    security(("BearerAuth" = [])),
    responses(
        (status = 200, description = "Tomorrow's prices (check 'available' field)", body = PriceListDto),
        (status = 404, description = "User settings not found", body = ErrorDto)
    )
)]
pub async fn tomorrow(
    State(state): State<AppState>,
    Extension(user): Extension<ApiUser>,
) -> Response {
    finish(list_answer(&state, &user.0, ListKind::Tomorrow).await)
}

/// Every published total price of today and tomorrow in one list.
#[utoipa::path(
    get,
    path = "/api/v1/price/all",
    tag = "Price",
    summary = "All currently-known hourly total prices in one list",
    description = "Returns every currently-known published total price in a single flat list (today always, tomorrow's intervals appended once Nord Pool has published them), so an automation can poll once instead of hitting /today and /tomorrow separately. The response is the same shape as /today: whether tomorrow is included is inferable from the 'end' timestamp. 'available' is true whenever any prices are known and false with an empty 'prices' array only when nothing is published — never an estimate. 'min/max/average' cover ALL currently-known prices, so the span widens as tomorrow publishes (~24h before, ~48h after) rather than being a stable daily figure. No 'expectedAt' hint is emitted here — that is /tomorrow's job. Published prices only — this is not a forecast.",
    security(("BearerAuth" = [])),
    responses(
        (status = 200, description = "All currently-known published prices in a single list", body = PriceListDto),
        (status = 404, description = "User settings not found", body = ErrorDto)
    )
)]
pub async fn all(State(state): State<AppState>, Extension(user): Extension<ApiUser>) -> Response {
    finish(list_answer(&state, &user.0, ListKind::All).await)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListKind {
    Today,
    Tomorrow,
    All,
}

async fn list_answer(state: &AppState, user_id: &str, kind: ListKind) -> Answer {
    let Some(settings) = load_settings(state, user_id).await? else {
        return Ok(settings_not_found());
    };
    let now = state.now();
    let zone = settings.timezone.time_zone();
    let range = match kind {
        ListKind::Today => {
            utc_range_for_local_date(local_date(now, zone), zone).map_err(out_of_range)?
        }
        ListKind::Tomorrow => {
            utc_range_for_local_date(dates(now, &settings.timezone)?.tomorrow, zone)
                .map_err(out_of_range)?
        }
        ListKind::All => {
            let dates = dates(now, &settings.timezone)?;
            utc_range_for_local_date_span(dates.today, dates.tomorrow, zone)
                .map_err(out_of_range)?
        }
    };
    let rows = load_prices(state, range, settings.area).await?;
    let totals = calculate_total_prices(&intervals(&rows)?, &settings);
    Ok(price_list(&match kind {
        ListKind::Tomorrow => PriceList::tomorrow_from_totals(totals),
        ListKind::Today | ListKind::All => PriceList::from_totals(totals),
    }))
}

// ---- Cheapest window -----------------------------------------------------------

/// The cheapest contiguous window of the requested length.
#[utoipa::path(
    get,
    path = "/api/v1/price/cheapest",
    tag = "Price",
    summary = "Cheapest contiguous window",
    description = "Finds the cheapest contiguous N-minute window from available future prices. Optionally constrain the search with startTime, endTime, and maxPrice.",
    security(("BearerAuth" = [])),
    params(
        ("duration" = i32, Query, description = "Window duration in minutes (1-1440)"),
        ("startTime" = Option<String>, Query, description = "Earliest allowed window start (ISO 8601)"),
        ("endTime" = Option<String>, Query, description = "Latest allowed window end (ISO 8601)"),
        ("maxPrice" = Option<String>, Query, description = "Maximum allowed total price in c/kWh for each interval in the selected window")
    ),
    responses(
        (status = 200, description = "Cheapest window with price breakdown per interval", body = PriceWindowDto),
        (status = 400, description = "Invalid query parameters", body = ErrorDto),
        (status = 404, description = "No price data available or insufficient data for requested window", body = ErrorDto)
    )
)]
pub async fn cheapest(
    State(state): State<AppState>,
    Extension(user): Extension<ApiUser>,
    RawQuery(raw): RawQuery,
) -> Response {
    let query = match input::cheapest_query(&Query::parse(raw.as_deref())) {
        Ok(query) => query,
        Err(text) => return wire::error(StatusCode::BAD_REQUEST, &text),
    };
    finish(cheapest_answer(&state, &user.0, &query).await)
}

async fn cheapest_answer(
    state: &AppState,
    user_id: &str,
    query: &spot_price_domain::query::CheapestQuery,
) -> Answer {
    let Some(settings) = load_settings(state, user_id).await? else {
        return Ok(settings_not_found());
    };
    let now = state.now();
    let range = today_and_tomorrow(now, &settings.timezone)?;
    let rows = load_prices(state, range, settings.area).await?;
    Ok(
        match cheapest_window(&intervals(&rows)?, &settings, query, now) {
            Ok(window) => wire::json(StatusCode::OK, &PriceWindowDto::from(&window)),
            Err(error) => wire::error(StatusCode::NOT_FOUND, &error.to_string()),
        },
    )
}

// ---- History -------------------------------------------------------------------

/// The total prices of an inclusive local date range.
#[utoipa::path(
    get,
    path = "/api/v1/price/history",
    tag = "Price",
    summary = "Historical total prices for a local date range",
    description = "Returns all stored total prices for the inclusive local date range [from, to]. Totals apply the user's CURRENT contract settings to the historical public spot prices — there is no historical settings versioning. Dates are YYYY-MM-DD interpreted in the user's timezone; the maximum span is 31 days (inclusive). Check the 'available' field: it is false with an empty 'prices' array when no data is stored for the range.",
    security(("BearerAuth" = [])),
    params(
        ("from" = String, Query, description = "Inclusive range start, YYYY-MM-DD, interpreted in the user's timezone."),
        ("to" = String, Query, description = "Inclusive range end, YYYY-MM-DD, interpreted in the user's timezone.")
    ),
    responses(
        (status = 200, description = "Historical prices for the range (check 'available'; false + empty when no data stored)", body = PriceListDto),
        (status = 400, description = "Invalid query parameters (bad date, from after to, or span over 31 days)", body = ErrorDto),
        (status = 404, description = "User settings not found", body = ErrorDto)
    )
)]
pub async fn history(
    State(state): State<AppState>,
    Extension(user): Extension<ApiUser>,
    RawQuery(raw): RawQuery,
) -> Response {
    let query = match input::history_query(&Query::parse(raw.as_deref())) {
        Ok(query) => query,
        Err(text) => return wire::error(StatusCode::BAD_REQUEST, &text),
    };
    finish(history_answer(&state, &user.0, query).await)
}

async fn history_answer(
    state: &AppState,
    user_id: &str,
    query: spot_price_domain::query::HistoryQuery,
) -> Answer {
    let Some(settings) = load_settings(state, user_id).await? else {
        return Ok(settings_not_found());
    };
    let range = utc_range_for_local_date_span(query.from, query.to, settings.timezone.time_zone())
        .map_err(out_of_range)?;
    let rows = load_prices(state, range, settings.area).await?;
    let totals = calculate_total_prices(&intervals(&rows)?, &settings);
    Ok(price_list(&PriceList::from_totals(totals)))
}
