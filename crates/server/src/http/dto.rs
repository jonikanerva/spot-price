//! The JSON bodies of the routes. The field order is the key order of the
//! TypeScript responses, and the schema names, examples, and descriptions
//! are the ones of its `OpenAPI` document. Instants are `toISOString`
//! text.

use serde::Serialize;
use spot_price_domain::forecast::{CALIBRATED_BANDS, ForecastEntry as DomainEntry};
use spot_price_domain::price::{PriceWindow, TotalPrice as DomainTotal};
use utoipa::ToSchema;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{ObjectBuilder, Schema, SchemaType, Type};

use super::wire::iso;
use crate::store::prices::PriceRow;

/// A schema `{"type": t, "example": example}` for `schema_with`.
#[expect(
    deprecated,
    reason = "the TypeScript document uses `example`, like the derive output"
)]
fn typed(kind: Type, example: serde_json::Value) -> ObjectBuilder {
    ObjectBuilder::new()
        .schema_type(SchemaType::new(kind))
        .example(Some(example))
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = TotalPrice)]
pub struct TotalPriceDto {
    #[schema(example = "2026-02-27T10:00:00.000Z")]
    pub delivery_start: String,
    #[schema(example = "2026-02-27T10:15:00.000Z")]
    pub delivery_end: String,
    #[schema(example = "2026-02-27T12:00:00.000+02:00")]
    pub local_start: String,
    #[schema(example = "2026-02-27T12:15:00.000+02:00")]
    pub local_end: String,
    #[schema(example = 5.23)]
    pub spot_cents_kwh: f64,
    #[schema(example = 0.5)]
    pub margin_cents_kwh: f64,
    #[schema(example = 2.5)]
    pub transfer_cents_kwh: f64,
    #[schema(example = 2.79372)]
    pub tax_cents_kwh: f64,
    #[schema(example = 2.81)]
    pub vat_cents_kwh: f64,
    /// Total price (c/kWh) the household pays. Floored at the contract-cost minimum: negative spot is treated as 0 (a retailer does not credit negatives), so the total never drops below (margin + transfer + tax) × (1 + VAT). The displayed spotCentsKwh stays raw and may be negative.
    #[schema(example = 13.83)]
    pub total_cents_kwh: f64,
    #[schema(example = false)]
    pub is_night_rate: bool,
}

impl From<&DomainTotal> for TotalPriceDto {
    fn from(total: &DomainTotal) -> Self {
        let breakdown = &total.breakdown;
        Self {
            delivery_start: iso(total.period.start()),
            delivery_end: iso(total.period.end()),
            local_start: total.local_start.clone(),
            local_end: total.local_end.clone(),
            spot_cents_kwh: breakdown.spot_cents_kwh,
            margin_cents_kwh: breakdown.margin_cents_kwh,
            transfer_cents_kwh: breakdown.transfer_cents_kwh,
            tax_cents_kwh: breakdown.tax_cents_kwh,
            vat_cents_kwh: breakdown.vat_cents_kwh,
            total_cents_kwh: breakdown.total_cents_kwh,
            is_night_rate: breakdown.is_night_rate,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PriceWindow)]
pub struct PriceWindowDto {
    #[schema(example = "2026-02-27T22:00:00.000Z")]
    pub start: String,
    #[schema(example = "2026-02-28T01:00:00.000Z")]
    pub end: String,
    #[schema(example = "2026-02-28T00:00:00.000+02:00")]
    pub start_local: String,
    #[schema(example = "2026-02-28T03:00:00.000+02:00")]
    pub end_local: String,
    #[schema(example = 6.21)]
    pub min_total_cents_kwh: f64,
    #[schema(example = 11.05)]
    pub max_total_cents_kwh: f64,
    #[schema(example = 8.42)]
    pub average_total_cents_kwh: f64,
    pub prices: Vec<TotalPriceDto>,
}

impl From<&PriceWindow> for PriceWindowDto {
    fn from(window: &PriceWindow) -> Self {
        Self {
            start: iso(window.start()),
            end: iso(window.end()),
            start_local: window.start_local().to_owned(),
            end_local: window.end_local().to_owned(),
            min_total_cents_kwh: window.min_total_cents_kwh(),
            max_total_cents_kwh: window.max_total_cents_kwh(),
            average_total_cents_kwh: window.average_total_cents_kwh(),
            prices: window.prices().iter().map(TotalPriceDto::from).collect(),
        }
    }
}

impl PriceWindowDto {
    /// The window fields of a list that Nord Pool has not published: empty
    /// texts, zero totals, and no prices.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            start: String::new(),
            end: String::new(),
            start_local: String::new(),
            end_local: String::new(),
            min_total_cents_kwh: 0.0,
            max_total_cents_kwh: 0.0,
            average_total_cents_kwh: 0.0,
            prices: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PriceList)]
pub struct PriceListDto {
    #[serde(flatten)]
    pub window: PriceWindowDto,
    #[schema(example = true)]
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "12:00 UTC", nullable = false)]
    pub expected_at: Option<&'static str>,
}

impl From<&spot_price_domain::endpoints::PriceList> for PriceListDto {
    fn from(list: &spot_price_domain::endpoints::PriceList) -> Self {
        match list {
            spot_price_domain::endpoints::PriceList::Available(window) => Self {
                window: PriceWindowDto::from(window),
                available: true,
                expected_at: None,
            },
            spot_price_domain::endpoints::PriceList::Unavailable { expected_at } => Self {
                window: PriceWindowDto::empty(),
                available: false,
                expected_at: *expected_at,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SpotPriceEntry)]
pub struct SpotPriceEntryDto {
    #[schema(example = "2026-02-27T10:00:00.000Z")]
    pub delivery_start: String,
    #[schema(example = "2026-02-27T10:15:00.000Z")]
    pub delivery_end: String,
    #[schema(example = 52.3)]
    pub price_eur_mwh: f64,
    #[schema(example = "FI")]
    pub area: String,
    #[schema(example = 5.23)]
    pub spot_cents_kwh: f64,
}

impl From<&PriceRow> for SpotPriceEntryDto {
    fn from(row: &PriceRow) -> Self {
        Self {
            delivery_start: iso(row.delivery_start),
            delivery_end: iso(row.delivery_end),
            price_eur_mwh: row.price_eur_mwh,
            area: row.area.code().to_owned(),
            spot_cents_kwh: spot_price_domain::price::eur_mwh_to_cents_kwh(row.price_eur_mwh),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PublicSpot)]
pub struct PublicSpotDto {
    #[schema(example = "FI")]
    pub area: String,
    pub today: Vec<SpotPriceEntryDto>,
    pub tomorrow: Vec<SpotPriceEntryDto>,
    #[schema(example = true)]
    pub tomorrow_available: bool,
    #[schema(example = "c/kWh")]
    pub unit: &'static str,
    #[schema(example = 15)]
    pub resolution_minutes: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = UserSettings)]
pub struct UserSettingsDto {
    #[schema(example = "abc123")]
    pub user_id: String,
    #[schema(example = 0.5)]
    pub margin_cents_kwh: f64,
    #[schema(example = 2.5)]
    pub transfer_day_cents_kwh: f64,
    #[schema(example = 1.2)]
    pub transfer_night_cents_kwh: f64,
    #[schema(example = 2.79372)]
    pub tax_cents_kwh: f64,
    #[schema(example = 25.5)]
    pub vat_percent: f64,
    #[schema(example = 22)]
    pub night_start_hour: i32,
    #[schema(example = 7)]
    pub night_end_hour: i32,
    #[schema(example = "Europe/Helsinki")]
    pub timezone: String,
    #[schema(example = "FI")]
    pub area: String,
}

impl From<crate::store::user_settings::UserSettingsRow> for UserSettingsDto {
    fn from(row: crate::store::user_settings::UserSettingsRow) -> Self {
        Self {
            user_id: row.user_id,
            margin_cents_kwh: row.margin_cents_kwh,
            transfer_day_cents_kwh: row.transfer_day_cents_kwh,
            transfer_night_cents_kwh: row.transfer_night_cents_kwh,
            tax_cents_kwh: row.tax_cents_kwh,
            vat_percent: row.vat_percent,
            night_start_hour: row.night_start_hour,
            night_end_hour: row.night_end_hour,
            timezone: row.timezone,
            area: row.area,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ChartData)]
pub struct ChartDataDto {
    pub today: Vec<TotalPriceDto>,
    pub tomorrow: Vec<TotalPriceDto>,
    #[schema(example = true)]
    pub tomorrow_available: bool,
    #[schema(example = "c/kWh")]
    pub unit: &'static str,
    #[schema(example = 15)]
    pub resolution_minutes: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[schema(as = Error)]
pub struct ErrorDto {
    #[schema(example = "Description of what went wrong")]
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ForecastEntry)]
pub struct ForecastEntryDto {
    #[schema(example = "2026-02-28T22:00:00.000Z")]
    pub start: String,
    #[schema(example = "2026-02-28T22:15:00.000Z")]
    pub end: String,
    #[schema(example = "2026-03-01T00:00:00.000+02:00")]
    pub local_start: String,
    #[schema(example = "2026-03-01T00:15:00.000+02:00")]
    pub local_end: String,
    #[schema(example = 4.12)]
    pub estimated_spot_cents_kwh: f64,
    /// Estimated total price (c/kWh). Floored at the contract-cost minimum: negative spot is treated as 0 (a retailer does not credit negatives), so the total never drops below (margin + transfer + tax) × (1 + VAT). The displayed estimatedSpotCentsKwh stays raw and may be negative.
    #[schema(example = 11.74)]
    pub estimated_total_cents_kwh: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Lower empirical band bound (spot). Present only with a calibrated band.
    #[schema(example = 2.1, nullable = false)]
    pub estimated_spot_low_cents_kwh: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Upper empirical band bound (spot). Present only with a calibrated band.
    #[schema(example = 7.8, nullable = false)]
    pub estimated_spot_high_cents_kwh: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Lower empirical band bound (total, contract terms applied).
    #[schema(example = 9.6, nullable = false)]
    pub estimated_total_low_cents_kwh: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Upper empirical band bound (total, contract terms applied).
    #[schema(example = 16.3, nullable = false)]
    pub estimated_total_high_cents_kwh: Option<f64>,
}

impl From<&DomainEntry> for ForecastEntryDto {
    fn from(entry: &DomainEntry) -> Self {
        Self {
            start: iso(entry.start),
            end: iso(entry.end),
            local_start: entry.local_start.clone(),
            local_end: entry.local_end.clone(),
            estimated_spot_cents_kwh: entry.estimated_spot_cents_kwh,
            estimated_total_cents_kwh: entry.estimated_total_cents_kwh,
            estimated_spot_low_cents_kwh: entry.band.map(|band| band.spot_low_cents_kwh),
            estimated_spot_high_cents_kwh: entry.band.map(|band| band.spot_high_cents_kwh),
            estimated_total_low_cents_kwh: entry.band.map(|band| band.total_low_cents_kwh),
            estimated_total_high_cents_kwh: entry.band.map(|band| band.total_high_cents_kwh),
        }
    }
}

fn method_schema() -> impl Into<RefOr<Schema>> {
    typed(Type::String, "empirical-residual".into()).enum_values(Some(["empirical-residual"]))
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(
    as = ForecastBands,
    description = "Empirical prediction-band descriptor. An estimate of recent residual spread, NOT a guaranteed coverage probability — prices can fall outside it, especially at spikes. Currently ships dark (calibrated:false) until a regenerated artifact clears the coverage gate."
)]
pub struct ForecastBandsDto {
    #[schema(schema_with = method_schema)]
    pub method: &'static str,
    /// Target fraction the P10/P90 offsets aim to contain.
    #[schema(example = 0.8)]
    pub nominal_coverage: f64,
    /// Measured out-of-sample coverage of the shipped offsets, or null when uncalibrated.
    #[schema(required = true,
        example = json!(null))]
    pub observed_coverage: Option<f64>,
    /// When false, bands are dark (no bound fields emitted) — coverage did not clear the ship gate or history is too thin.
    #[schema(example = false)]
    pub calibrated: bool,
    /// When the band artifact was generated, or empty when dark.
    #[schema(example = "")]
    pub generated_at: String,
}

impl ForecastBandsDto {
    /// The descriptor of the shipped band artifact.
    #[must_use]
    pub fn shipped() -> Self {
        Self {
            method: CALIBRATED_BANDS.method.as_str(),
            nominal_coverage: CALIBRATED_BANDS.nominal_coverage,
            observed_coverage: CALIBRATED_BANDS.observed_coverage,
            calibrated: CALIBRATED_BANDS.calibrated,
            generated_at: CALIBRATED_BANDS.generated_at.to_string(),
        }
    }
}

fn forecast_flag_schema() -> impl Into<RefOr<Schema>> {
    typed(Type::Boolean, true.into()).enum_values(Some([true]))
}

fn confidence_schema() -> impl Into<RefOr<Schema>> {
    typed(Type::String, "normal".into()).enum_values(Some(["normal", "low"]))
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ForecastResponse)]
pub struct ForecastResponseDto {
    #[schema(schema_with = forecast_flag_schema)]
    pub forecast: bool,
    #[schema(example = "FI")]
    pub area: String,
    #[schema(example = true)]
    pub available: bool,
    #[schema(example = false)]
    pub degraded: bool,
    #[schema(schema_with = confidence_schema)]
    pub confidence: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Present when available is false or degraded is true
    #[schema(example = "Forecast available for FI only", nullable = false)]
    pub reason: Option<&'static str>,
    #[schema(example = "2026-02-28T13:00:00.000Z")]
    pub generated_at: String,
    #[schema(example = "c/kWh")]
    pub unit: &'static str,
    #[schema(example = 15)]
    pub resolution_minutes: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub bands: Option<ForecastBandsDto>,
    pub entries: Vec<ForecastEntryDto>,
}

/// `{"apiKey", "createdAt"}` of the key routes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyDto {
    pub api_key: String,
    pub created_at: String,
}

/// The user object of the Better Auth responses, in its key order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserDto {
    pub name: String,
    pub email: String,
    pub email_verified: bool,
    pub image: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub id: String,
}

impl From<&crate::store::accounts::UserRow> for UserDto {
    fn from(user: &crate::store::accounts::UserRow) -> Self {
        Self {
            id: user.id.clone(),
            name: user.name.clone(),
            email: user.email.clone(),
            email_verified: user.email_verified,
            image: user.image.clone(),
            created_at: iso(user.created_at),
            updated_at: iso(user.updated_at),
        }
    }
}

/// The `session` field of `GET /api/session`: the Better Auth session
/// fields in their key order, without the token, the id, and the always
/// empty IP address and user agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfoDto {
    pub expires_at: String,
    pub created_at: String,
    pub user_id: String,
}

/// `{"session": {...}, "user": {...}}` of a signed-in request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionDto {
    pub session: SessionInfoDto,
    pub user: UserDto,
}

/// The body of `GET /api/session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionResponseDto {
    pub session: Option<SessionDto>,
    pub username: Option<String>,
}

/// The body of a successful sign-in. `token` is always `null`: the session
/// token goes only into the `HttpOnly` cookie.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignInDto {
    pub redirect: bool,
    pub token: Option<String>,
    pub user: UserDto,
}

/// The body of a successful sign-up. `token` is always `null`, as in
/// [`SignInDto`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignUpDto {
    pub token: Option<String>,
    pub user: UserDto,
}

/// A Better Auth error body: `{"message", "code"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AuthErrorDto {
    pub message: &'static str,
    pub code: &'static str,
}

/// `{"success": true}` of the sign-out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SuccessDto {
    pub success: bool,
}
