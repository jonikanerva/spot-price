//! The setup page `GET /` and the API reference page `GET /api/docs`.
//!
//! `ui/index.html` and `ui/client.js` are the bytes of the TypeScript page
//! with placeholders for the generated parts. Prettier must not format them
//! (`.prettierignore`): a test compares the rendered page byte for byte.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use spot_price_domain::areas::{ALL_AREAS, supported_timezones};

use super::{AppState, wire};

const PAGE_TEMPLATE: &str = include_str!("ui/index.html");
const CLIENT_TEMPLATE: &str = include_str!("ui/client.js");
/// The Scalar page of the TypeScript service, which loads Scalar from the
/// jsDelivr CDN and reads `/api/v1/openapi.json`.
const DOCS_PAGE: &str = include_str!("ui/docs.html");

/// The pages and the `OpenAPI` document, rendered once at start-up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pages {
    pub home: String,
    /// `None` when the document could not be serialized.
    pub openapi: Option<Vec<u8>>,
}

impl Pages {
    #[must_use]
    pub fn render() -> Self {
        let openapi = super::openapi::document()
            .and_then(|document| serde_json::to_vec(&document))
            .inspect_err(|_| {
                tracing::error!(code = "openapi", "the OpenAPI document could not be built");
            })
            .ok();
        Self {
            home: render_home(),
            openapi,
        }
    }
}

/// The text of a JSON string, like `JSON.stringify` of a string.
fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

/// The setup page with the area and time zone options of the domain.
#[must_use]
pub fn render_home() -> String {
    let area_options = ALL_AREAS
        .iter()
        .map(|area| {
            format!(
                "<option value=\"{}\">{} ({})</option>",
                area.code(),
                area.name(),
                area.code()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let timezone_options = supported_timezones()
        .into_iter()
        .map(|zone| format!("<option value=\"{zone}\">{zone}</option>"))
        .collect::<Vec<_>>()
        .join("\n");
    let area_timezones = ALL_AREAS
        .iter()
        .map(|area| {
            format!(
                "{}: {}",
                json_string(area.code()),
                json_string(area.timezone())
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let script = CLIENT_TEMPLATE.replacen(
        "{{AREA_TIMEZONE_MAP}}",
        &format!("{{ {area_timezones} }}"),
        1,
    );
    PAGE_TEMPLATE
        .replacen("{{AREA_OPTIONS}}", &area_options, 1)
        .replacen("{{TIMEZONE_OPTIONS}}", &timezone_options, 1)
        .replacen("{{CLIENT_SCRIPT}}", &script, 1)
}

/// `GET /`: the setup page.
pub async fn home(State(state): State<AppState>) -> Response {
    wire::body(StatusCode::OK, wire::HTML, state.pages.home.clone())
}

/// `GET /api/docs`: the interactive API reference.
pub async fn docs() -> Response {
    wire::body(StatusCode::OK, wire::HTML, DOCS_PAGE)
}
