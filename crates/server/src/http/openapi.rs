//! The `OpenAPI` 3.1 document of the API (`contract/openapi.json`), served at
//! `GET /api/v1/openapi.json`.
//!
//! The documented routes register through `utoipa-axum`, so the router and
//! the document come from the same lists. [`document`] then gives the
//! document the shape of the TypeScript (zod-openapi) document: no operation
//! ids, no number formats, and the query parameter schemas of the zod
//! schemas. The annotations do not prove what a handler answers; the HTTP
//! tests check the answers.
//!
//! Outside the document, like in the TypeScript service: `/health`, `/`,
//! `/api/docs`, `/api/session*`, and `/api/keys*`.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Map, Value, json};
use spot_price_domain::areas::{ALL_AREAS, supported_timezones};
use utoipa::openapi::security::{Http, HttpAuthScheme, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::{OpenApiRouter, UtoipaMethodRouterExt as _};
use utoipa_axum::routes;

use super::dto::{
    ChartDataDto, ErrorDto, ForecastBandsDto, ForecastEntryDto, ForecastResponseDto, PriceListDto,
    PriceWindowDto, PublicSpotDto, SpotPriceEntryDto, TotalPriceDto, UserSettingsDto,
};
use super::{AppState, forecast, me, not_found, price, wire};

#[derive(Debug, OpenApi)]
#[openapi(
    info(
        title = "Spot Price API",
        version = "1.0.0",
        description = "Finnish and Nordic spot electricity price API with total price calculation, cheapest window finder, and per-user contract settings."
    ),
    servers((url = "https://spot.calmdonut.com")),
    security(("BearerAuth" = [])),
    tags(
        (name = "Price", description = "Electricity price endpoints (API key required)"),
        (name = "Forecast", description = "FI price forecast — a clearly-labelled estimate, separate from real prices (API key required)"),
        (name = "User", description = "User settings and chart data (session required)"),
        (name = "Public", description = "Unauthenticated endpoints")
    ),
    components(schemas(
        PublicSpotDto,
        SpotPriceEntryDto,
        TotalPriceDto,
        ErrorDto,
        PriceListDto,
        PriceWindowDto,
        ForecastResponseDto,
        ForecastBandsDto,
        ForecastEntryDto,
        UserSettingsDto,
        ChartDataDto
    )),
    modifiers(&Security)
)]
struct ApiDoc;

/// Adds the bearer scheme and removes the licence and contact that the
/// derive takes from the crate manifest.
#[derive(Debug)]
struct Security;

impl Modify for Security {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        openapi.info.license = None;
        openapi.info.contact = None;
        let components = openapi
            .components
            .get_or_insert_with(utoipa::openapi::Components::new);
        let mut bearer = Http::new(HttpAuthScheme::Bearer);
        bearer.description = Some(
            "API key obtained from the web dashboard. Use as: Authorization: Bearer <api-key>"
                .to_owned(),
        );
        components.add_security_scheme("BearerAuth", SecurityScheme::Http(bearer));
    }
}

/// `/api/public/spot`.
pub fn public_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(price::public_spot).map(|r| r.fallback(not_found)))
}

/// `/api/v1/price/*`, behind the API key check.
pub fn price_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(price::now).map(|r| r.fallback(not_found)))
        .routes(routes!(price::today).map(|r| r.fallback(not_found)))
        .routes(routes!(price::tomorrow).map(|r| r.fallback(not_found)))
        .routes(routes!(price::cheapest).map(|r| r.fallback(not_found)))
        .routes(routes!(price::history).map(|r| r.fallback(not_found)))
        .routes(routes!(price::all).map(|r| r.fallback(not_found)))
        .routes(routes!(forecast::forecast).map(|r| r.fallback(not_found)))
}

/// `/api/v1/me/*`, behind the session check.
pub fn me_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(me::get_settings, me::put_settings).map(|r| r.fallback(not_found)))
        .routes(routes!(me::chart).map(|r| r.fallback(not_found)))
}

/// The document as `utoipa` builds it from the routes.
fn raw_document() -> utoipa::openapi::OpenApi {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .merge(public_routes())
        .merge(price_routes())
        .merge(me_routes())
        .into_openapi()
}

/// The finished document.
///
/// # Errors
///
/// Returns a JSON error when the document cannot be serialized.
pub fn document() -> Result<Value, serde_json::Error> {
    let mut value = serde_json::to_value(raw_document())?;
    finish(&mut value);
    Ok(value)
}

/// The document as pretty JSON with a final newline, for
/// `spot-price export-openapi`.
///
/// # Errors
///
/// Returns a JSON error when the document cannot be serialized.
pub fn document_json() -> Result<String, serde_json::Error> {
    let mut text = serde_json::to_string_pretty(&document()?)?;
    text.push('\n');
    Ok(text)
}

/// `GET /api/v1/openapi.json`.
pub async fn serve(State(state): State<AppState>) -> Response {
    match &state.pages.openapi {
        Some(bytes) => wire::body(StatusCode::OK, wire::JSON, bytes.clone()),
        None => wire::internal_error(),
    }
}

fn area_codes() -> Vec<Value> {
    ALL_AREAS
        .iter()
        .map(|area| Value::from(area.code()))
        .collect()
}

fn timezones() -> Vec<Value> {
    supported_timezones().into_iter().map(Value::from).collect()
}

/// The zod schema of a query parameter, with its description repeated in
/// the schema like zod-openapi does.
fn parameter_schema(path: &str, name: &str, description: &str) -> Option<Value> {
    let mut schema = match (path, name) {
        ("/api/public/spot" | "/api/v1/price/forecast", "area") => json!({
            "type": "string", "enum": area_codes(), "default": "FI", "example": "FI"
        }),
        (_, "duration") => {
            json!({"type": "integer", "minimum": 1, "maximum": 1440, "example": 180})
        }
        (_, "startTime") => json!({"type": "string", "example": "2026-02-27T14:00:00+02:00"}),
        (_, "endTime") => json!({"type": "string", "example": "2026-02-28T06:00:00+02:00"}),
        (_, "maxPrice") => json!({"type": "string", "minLength": 1, "example": 12}),
        (_, "from") => json!({"type": "string", "format": "date", "example": "2026-04-01"}),
        (_, "to") => json!({"type": "string", "format": "date", "example": "2026-05-01"}),
        _ => return None,
    };
    if let Some(object) = schema.as_object_mut() {
        object.insert("description".to_owned(), Value::from(description));
    }
    Some(schema)
}

/// Gives the utoipa document the shape of the TypeScript document.
fn finish(document: &mut Value) {
    if let Some(root) = document.as_object_mut() {
        if let Some(components) = root.get_mut("components").and_then(Value::as_object_mut) {
            components
                .entry("parameters")
                .or_insert_with(|| Value::Object(Map::new()));
        }
        root.entry("webhooks")
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if let Some(paths) = document.get_mut("paths").and_then(Value::as_object_mut) {
        for (path, item) in paths.iter_mut() {
            let Some(item) = item.as_object_mut() else {
                continue;
            };
            for operation in item.values_mut().filter_map(Value::as_object_mut) {
                operation.remove("operationId");
                finish_parameters(path, operation);
                if let Some(body) = operation
                    .get_mut("requestBody")
                    .and_then(Value::as_object_mut)
                {
                    body.remove("required");
                    body.remove("description");
                }
            }
        }
    }
    finish_settings_body(document);
    strip_formats(document);
}

fn finish_parameters(path: &str, operation: &mut Map<String, Value>) {
    let Some(parameters) = operation
        .get_mut("parameters")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for parameter in parameters.iter_mut().filter_map(Value::as_object_mut) {
        let name = parameter
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let description = parameter
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if let Some(schema) = parameter_schema(path, &name, &description) {
            parameter.insert("schema".to_owned(), schema);
        }
    }
}

/// The settings update body: enums of the supported values and no `null`.
fn finish_settings_body(document: &mut Value) {
    let Some(properties) = document
        .pointer_mut("/paths/~1api~1v1~1me~1settings/put/requestBody/content/application~1json/schema/properties")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for (name, property) in properties.iter_mut() {
        let Some(property) = property.as_object_mut() else {
            continue;
        };
        match name.as_str() {
            "timezone" => {
                property.insert("enum".to_owned(), Value::from(timezones()));
            }
            "area" => {
                property.insert("enum".to_owned(), Value::from(area_codes()));
            }
            _ => {}
        }
        drop_null_type(property);
    }
}

/// `["number", "null"]` to `"number"`.
fn drop_null_type(schema: &mut Map<String, Value>) {
    if let Some(Value::Array(types)) = schema.get("type") {
        let kept: Vec<Value> = types
            .iter()
            .filter(|t| t.as_str() != Some("null"))
            .cloned()
            .collect();
        if let [single] = kept.as_slice() {
            schema.insert("type".to_owned(), single.clone());
        }
    }
}

/// Removes the numeric formats that zod-openapi does not emit.
fn strip_formats(value: &mut Value) {
    match value {
        Value::Object(object) => {
            let numeric = matches!(
                object.get("format").and_then(Value::as_str),
                Some("double" | "float" | "int32" | "int64")
            );
            if numeric {
                object.remove("format");
            }
            object.values_mut().for_each(strip_formats);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_formats),
        _ => {}
    }
}
