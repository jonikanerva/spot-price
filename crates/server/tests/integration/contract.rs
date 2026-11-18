//! The committed contract and the setup page, without a database.
//!
//! - `contract/openapi.json` is the export of `spot-price export-openapi`.
//! - The document has the meaning of the TypeScript document
//!   (`golden/http/openapi.ts.json`): equal paths, methods, parameters,
//!   schemas, responses, tags, security, servers, and info. The JSON key
//!   order and the entries of [`INTENTIONAL`] differ.
//! - The setup page is byte-identical to `fixtures/ui-home.html`. The
//!   fixture is the TypeScript `renderHomePage()` with the chart fix, the
//!   contract terms versions, and the dashboard layout of the setup page; the
//!   HTTP golden replay lists that difference.

use serde_json::Value;
use spot_price_server::http::openapi::{document, document_json};
use spot_price_server::http::ui::render_home;

const CONTRACT: &str = include_str!("../../../../contract/openapi.json");
const TS_DOCUMENT: &str = include_str!("../golden/http/openapi.ts.json");
const HOME: &str = include_str!("../fixtures/ui-home.html");

/// The differences from the TypeScript document, by design, as
/// [`differences`] reports them. The test fails on any other difference and
/// on a listed difference that no longer occurs. After the first entry, the
/// entries are these two changes:
/// - The money fields of the settings include VAT and have `InclVat` in the
///   name. The price fields of `TotalPrice` say that they have no VAT.
/// - The versioned contract terms: `validFrom` in the settings schema and
///   the settings body, the two version routes and their schemas, and the
///   descriptions that state the new behaviour.
pub const INTENTIONAL: [&str; 34] = [
    "/components/schemas/ForecastResponse/properties/generatedAt/description: extra",
    "/components/schemas/TotalPrice/properties/marginCentsKwh/description: extra",
    "/components/schemas/TotalPrice/properties/taxCentsKwh/description: extra",
    "/components/schemas/TotalPrice/properties/transferCentsKwh/description: extra",
    "/components/schemas/TotalPrice/properties/vatCentsKwh/description: extra",
    "/components/schemas/UserSettings/properties/marginCentsKwh: missing",
    "/components/schemas/UserSettings/properties/taxCentsKwh: missing",
    "/components/schemas/UserSettings/properties/transferDayCentsKwh: missing",
    "/components/schemas/UserSettings/properties/transferNightCentsKwh: missing",
    "/components/schemas/UserSettings/properties/vatPercent/description: extra",
    "/components/schemas/UserSettings/properties/marginInclVatCentsKwh: extra",
    "/components/schemas/UserSettings/properties/taxInclVatCentsKwh: extra",
    "/components/schemas/UserSettings/properties/transferDayInclVatCentsKwh: extra",
    "/components/schemas/UserSettings/properties/transferNightInclVatCentsKwh: extra",
    "/components/schemas/UserSettings/properties/validFrom: extra",
    r#"/components/schemas/UserSettings/required: want ["userId","marginCentsKwh","transferDayCentsKwh","transferNightCentsKwh","taxCentsKwh","vatPercent","nightStartHour","nightEndHour","timezone","area"] got ["userId","marginInclVatCentsKwh","transferDayInclVatCentsKwh","transferNightInclVatCentsKwh","taxInclVatCentsKwh","vatPercent","nightStartHour","nightEndHour","timezone","area","validFrom"]"#,
    "/components/schemas/ContractTermsVersion: extra",
    "/components/schemas/ContractTermsVersions: extra",
    r#"/paths/~1api~1v1~1me~1settings/get/description: want "Returns the authenticated user's electricity contract settings." got "Returns the authenticated user's electricity contract settings: the timezone, the area, and the contract terms that apply today. The margin, the transfer prices, and the tax include VAT, as on an invoice; the price routes return them without VAT. validFrom is the date of the terms version that holds those terms.""#,
    r#"/paths/~1api~1v1~1me~1settings/put/description: want "Update one or more electricity contract settings. Only provided fields are changed." got "Update one or more electricity contract settings. Only provided fields are changed. The margin, the transfer prices, and the tax include VAT, as on an invoice; vatPercent applies to the spot price only. The timezone and the area have one current value. The contract terms are versioned: the request saves the terms version that applies from validFrom (default: today in the user's timezone). A term that the request does not give keeps the value that applies on validFrom. A version with the same validFrom is replaced. No version is created when the terms equal the terms that already apply on validFrom. The response has the terms that apply on validFrom and the validFrom of their version. A user can hold at most 100 versions.""#,
    "/paths/~1api~1v1~1me~1settings/put/requestBody/content/application~1json/schema/properties/marginCentsKwh: missing",
    "/paths/~1api~1v1~1me~1settings/put/requestBody/content/application~1json/schema/properties/taxCentsKwh: missing",
    "/paths/~1api~1v1~1me~1settings/put/requestBody/content/application~1json/schema/properties/transferDayCentsKwh: missing",
    "/paths/~1api~1v1~1me~1settings/put/requestBody/content/application~1json/schema/properties/transferNightCentsKwh: missing",
    r#"/paths/~1api~1v1~1me~1settings/put/requestBody/content/application~1json/schema/properties/vatPercent/description: want "VAT percentage (0-100)" got "VAT percentage (0-100) of the spot price. The other prices already include it.""#,
    "/paths/~1api~1v1~1me~1settings/put/requestBody/content/application~1json/schema/properties/marginInclVatCentsKwh: extra",
    "/paths/~1api~1v1~1me~1settings/put/requestBody/content/application~1json/schema/properties/taxInclVatCentsKwh: extra",
    "/paths/~1api~1v1~1me~1settings/put/requestBody/content/application~1json/schema/properties/transferDayInclVatCentsKwh: extra",
    "/paths/~1api~1v1~1me~1settings/put/requestBody/content/application~1json/schema/properties/transferNightInclVatCentsKwh: extra",
    "/paths/~1api~1v1~1me~1settings/put/requestBody/content/application~1json/schema/properties/validFrom: extra",
    r#"/paths/~1api~1v1~1me~1settings/put/responses/400/description: want "Validation error" got "Validation error, or the version limit is reached""#,
    r#"/paths/~1api~1v1~1price~1history/get/description: want "Returns all stored total prices for the inclusive local date range [from, to]. Totals apply the user's CURRENT contract settings to the historical public spot prices — there is no historical settings versioning. Dates are YYYY-MM-DD interpreted in the user's timezone; the maximum span is 31 days (inclusive). Check the 'available' field: it is false with an empty 'prices' array when no data is stored for the range." got "Returns all stored total prices for the inclusive local date range [from, to]. Each interval uses the contract terms that applied at that interval: the terms version of the local date of the interval start. Dates are YYYY-MM-DD interpreted in the user's timezone; the maximum span is 31 days (inclusive). Check the 'available' field: it is false with an empty 'prices' array when no data is stored for the range.""#,
    "/paths//api/v1/me/settings/versions: extra",
    "/paths//api/v1/me/settings/versions/{validFrom}: extra",
];

/// Every difference between two JSON values, as `<pointer>: <detail>`.
pub fn differences(want: &Value, got: &Value, at: &str, out: &mut Vec<String>) {
    match (want, got) {
        (Value::Object(want), Value::Object(got)) => {
            for (key, value) in want {
                let path = format!("{at}/{}", key.replace('~', "~0").replace('/', "~1"));
                match got.get(key) {
                    Some(other) => differences(value, other, &path, out),
                    None => out.push(format!("{path}: missing")),
                }
            }
            for key in got.keys().filter(|key| !want.contains_key(*key)) {
                out.push(format!("{at}/{key}: extra"));
            }
        }
        (Value::Array(want), Value::Array(got)) if want.len() == got.len() => {
            for (index, (value, other)) in want.iter().zip(got).enumerate() {
                differences(value, other, &format!("{at}/{index}"), out);
            }
        }
        _ if want == got => {}
        _ => out.push(format!("{at}: want {want} got {got}")),
    }
}

#[test]
fn the_committed_contract_is_the_export() {
    assert_eq!(
        document_json().unwrap(),
        CONTRACT,
        "run `cargo run -p spot-price-server -- export-openapi contract/openapi.json`"
    );
}

#[test]
fn the_document_means_the_typescript_document() {
    let want: Value = serde_json::from_str(TS_DOCUMENT).unwrap();
    let mut found = Vec::new();
    differences(&want, &document().unwrap(), "", &mut found);
    assert_eq!(found, INTENTIONAL, "OpenAPI differences");
}

#[test]
fn the_setup_page_is_the_committed_page() {
    assert!(
        render_home() == HOME,
        "the setup page differs from ui-home.html"
    );
}
