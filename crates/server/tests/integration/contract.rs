//! The committed contract and the setup page, without a database.
//!
//! - `contract/openapi.json` is the export of `spot-price export-openapi`.
//! - The document has the meaning of the TypeScript document
//!   (`golden/http/openapi.ts.json`): equal paths, methods, parameters,
//!   schemas, responses, tags, security, servers, and info. Only the JSON key
//!   order differs.
//! - The setup page is byte-identical to the TypeScript `renderHomePage()`.

use serde_json::Value;
use spot_price_server::http::openapi::{document, document_json};
use spot_price_server::http::ui::render_home;

const CONTRACT: &str = include_str!("../../../../contract/openapi.json");
const TS_DOCUMENT: &str = include_str!("../golden/http/openapi.ts.json");
const TS_HOME: &str = include_str!("../fixtures/ui-home.html");

/// Every difference between two JSON values, as `<pointer>: <detail>`.
fn differences(want: &Value, got: &Value, at: &str, out: &mut Vec<String>) {
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
    assert!(
        found.is_empty(),
        "OpenAPI differences:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_setup_page_is_the_typescript_page() {
    assert!(
        render_home() == TS_HOME,
        "the setup page differs from ui-home.html"
    );
}
