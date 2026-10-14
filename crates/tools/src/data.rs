//! The public price and Fingrid data of the tools, and the backtest fixture
//! format of `crates/tools/data/fixture.json` and of a `--db --export`
//! snapshot. Every timestamp is checked at load; an invalid one stops the
//! load.

use std::collections::BTreeMap;
use std::path::Path;

use jiff::Timestamp;
use serde::Deserialize;
use spot_price_domain::forecast::iso_millis;
use spot_price_domain::time::truncate_to_millisecond;

use crate::error::ToolError;
use crate::jsfmt::JsJson;

/// A Fingrid dataset id, also the fixture key in decimal text.
pub type DatasetId = i32;

pub const DATASET_WIND_ACTUAL: DatasetId = 75;
pub const DATASET_CONSUMPTION_ACTUAL: DatasetId = 124;
pub const DATASET_WIND_FORECAST: DatasetId = 245;
pub const DATASET_CONSUMPTION_FORECAST: DatasetId = 165;

/// One spot price in c/kWh. `area` is the Nord Pool area code.
#[derive(Debug, Clone, PartialEq)]
pub struct PricePoint {
    pub area: String,
    pub start: Timestamp,
    pub spot_cents_kwh: f64,
}

/// One settled Fingrid value (MW).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridActual {
    pub dataset_id: DatasetId,
    pub start: Timestamp,
    pub end: Timestamp,
    pub value: f64,
}

/// One archived issuance of a Fingrid forecast value (MW). `issued_at` is
/// the hour-truncated fetch time of the issuance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vintage {
    pub dataset_id: DatasetId,
    pub issued_at: Timestamp,
    pub start: Timestamp,
    pub end: Timestamp,
    pub value: f64,
}

/// Everything the backtest reads. Maps iterate in ascending dataset id, the
/// order in which JavaScript iterates integer object keys.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BacktestData {
    pub prices: Vec<PricePoint>,
    pub actuals: BTreeMap<DatasetId, Vec<GridActual>>,
    pub vintages: BTreeMap<DatasetId, Vec<Vintage>>,
}

/// An instant from a fixture or the database, cut to millisecond precision
/// like a JavaScript `Date`.
///
/// # Errors
/// A message when `text` is not an RFC 3339 instant.
pub fn parse_instant(text: &str) -> Result<Timestamp, String> {
    text.parse::<Timestamp>()
        .map(truncate_to_millisecond)
        .map_err(|_| format!("invalid timestamp {text:?}"))
}

/// A dataset key: the canonical decimal text of a non-negative id.
///
/// # Errors
/// A message when `key` is not canonical decimal text of an `i32`.
pub fn parse_dataset_key(key: &str) -> Result<DatasetId, String> {
    key.parse::<DatasetId>()
        .ok()
        .filter(|id| *id >= 0 && id.to_string() == key)
        .ok_or_else(|| format!("invalid dataset key {key:?}"))
}

/// Whether every array element in `value` is an object. Every record of the
/// fixture formats is an object; serde would also read an array as a record.
fn records_are_objects(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .all(|item| item.is_object() && records_are_objects(item)),
        serde_json::Value::Object(members) => members.values().all(records_are_objects),
        _ => true,
    }
}

/// Reads and parses a JSON file like `JSON.parse`: a repeated key keeps its
/// last value. A record that is not a JSON object is an error.
///
/// # Errors
/// [`ToolError`] when the file cannot be read or is not valid JSON for `T`.
pub fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, ToolError> {
    let text = std::fs::read_to_string(path).map_err(|source| ToolError::Read {
        path: path.display().to_string(),
        source,
    })?;
    let json_error = |source| ToolError::Json {
        path: path.display().to_string(),
        source,
    };
    let value: serde_json::Value = serde_json::from_str(&text).map_err(json_error)?;
    if !records_are_objects(&value) {
        return Err(ToolError::Fixture {
            path: path.display().to_string(),
            reason: "a record is not a JSON object".to_owned(),
        });
    }
    serde_json::from_value(value).map_err(json_error)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPrice {
    area: Option<String>,
    start: String,
    spot_cents_kwh: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawActual {
    start_time: String,
    end_time: String,
    value: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawVintage {
    issued_at: String,
    start_time: String,
    end_time: String,
    value: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawFixture {
    prices: Vec<RawPrice>,
    fingrid_actuals: Option<BTreeMap<String, Vec<RawActual>>>,
    fingrid_forecast_vintages: Option<BTreeMap<String, Vec<RawVintage>>>,
    /// The old shape: one map without issuance times.
    fingrid: Option<BTreeMap<String, Vec<RawActual>>>,
}

fn to_actuals(raw: &[RawActual], dataset_id: DatasetId) -> Result<Vec<GridActual>, String> {
    raw.iter()
        .map(|r| {
            Ok(GridActual {
                dataset_id,
                start: parse_instant(&r.start_time)?,
                end: parse_instant(&r.end_time)?,
                value: r.value,
            })
        })
        .collect()
}

fn to_vintages(raw: &[RawVintage], dataset_id: DatasetId) -> Result<Vec<Vintage>, String> {
    raw.iter()
        .map(|r| {
            Ok(Vintage {
                dataset_id,
                issued_at: parse_instant(&r.issued_at)?,
                start: parse_instant(&r.start_time)?,
                end: parse_instant(&r.end_time)?,
                value: r.value,
            })
        })
        .collect()
}

fn from_raw(raw: RawFixture) -> Result<BacktestData, String> {
    let prices = raw
        .prices
        .into_iter()
        .map(|p| {
            Ok(PricePoint {
                // A price without an area is FI, as in the oldest fixtures.
                area: p.area.unwrap_or_else(|| "FI".to_owned()),
                start: parse_instant(&p.start)?,
                spot_cents_kwh: p.spot_cents_kwh,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let mut data = BacktestData {
        prices,
        ..BacktestData::default()
    };
    if raw.fingrid_actuals.is_some() || raw.fingrid_forecast_vintages.is_some() {
        for (key, records) in raw.fingrid_actuals.unwrap_or_default() {
            let id = parse_dataset_key(&key)?;
            data.actuals.insert(id, to_actuals(&records, id)?);
        }
        for (key, records) in raw.fingrid_forecast_vintages.unwrap_or_default() {
            let id = parse_dataset_key(&key)?;
            data.vintages.insert(id, to_vintages(&records, id)?);
        }
    } else if let Some(old) = raw.fingrid {
        // The old shape has no issuance times. Never invent one: the forecast
        // datasets are dropped, so every origin is pre-vintage.
        for (key, records) in old {
            let id = parse_dataset_key(&key)?;
            if id != DATASET_WIND_FORECAST && id != DATASET_CONSUMPTION_FORECAST {
                data.actuals.insert(id, to_actuals(&records, id)?);
            }
        }
    }
    Ok(data)
}

/// Reads a backtest fixture. The spot values are already in c/kWh; they are
/// not converted again.
///
/// # Errors
/// [`ToolError`] when the file cannot be read, is not valid JSON, or has an
/// invalid timestamp or dataset key.
pub fn load_fixture(path: &Path) -> Result<BacktestData, ToolError> {
    let raw: RawFixture = read_json(path)?;
    from_raw(raw).map_err(|reason| ToolError::Fixture {
        path: path.display().to_string(),
        reason,
    })
}

fn text(at: Timestamp) -> JsJson {
    JsJson::Text(iso_millis(at))
}

fn member(key: &str, value: JsJson) -> (String, JsJson) {
    (key.to_owned(), value)
}

/// The data as fixture JSON that [`load_fixture`] reads back unchanged.
#[must_use]
pub fn to_fixture_json(data: &BacktestData) -> String {
    let prices = data
        .prices
        .iter()
        .map(|p| {
            JsJson::Object(vec![
                member("area", JsJson::Text(p.area.clone())),
                member("start", text(p.start)),
                member("spotCentsKwh", JsJson::Number(p.spot_cents_kwh)),
            ])
        })
        .collect();
    let actuals = data
        .actuals
        .iter()
        .map(|(id, records)| {
            let rows = records
                .iter()
                .map(|r| {
                    JsJson::Object(vec![
                        member("startTime", text(r.start)),
                        member("endTime", text(r.end)),
                        member("value", JsJson::Number(r.value)),
                    ])
                })
                .collect();
            (id.to_string(), JsJson::Array(rows))
        })
        .collect();
    let vintages = data
        .vintages
        .iter()
        .map(|(id, records)| {
            let rows = records
                .iter()
                .map(|r| {
                    JsJson::Object(vec![
                        member("issuedAt", text(r.issued_at)),
                        member("startTime", text(r.start)),
                        member("endTime", text(r.end)),
                        member("value", JsJson::Number(r.value)),
                    ])
                })
                .collect();
            (id.to_string(), JsJson::Array(rows))
        })
        .collect();
    JsJson::Object(vec![
        member("prices", JsJson::Array(prices)),
        member("fingridActuals", JsJson::Object(actuals)),
        member("fingridForecastVintages", JsJson::Object(vintages)),
    ])
    .stringify_pretty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_text(name: &str, text: &str) -> Result<BacktestData, ToolError> {
        let path = std::env::temp_dir().join(format!(
            "spot-price-tools-data-{}-{name}.json",
            std::process::id()
        ));
        std::fs::write(&path, text).unwrap();
        let loaded = load_fixture(&path);
        std::fs::remove_file(&path).unwrap();
        loaded
    }

    #[test]
    fn a_repeated_key_keeps_its_last_value_like_json_parse() {
        let text = r#"{"prices":[{"area":"SE1","start":"2026-03-02T00:00:00.000Z","spotCentsKwh":1,"spotCentsKwh":2}]}"#;
        let data = load_text("repeated", text).unwrap();
        assert_eq!(data.prices[0].spot_cents_kwh, 2.0);
        assert_eq!(data.prices[0].area, "SE1");
    }

    #[test]
    fn an_array_record_is_an_error() {
        let text = r#"{"prices":[["FI","2026-03-02T00:00:00.000Z",1]]}"#;
        assert!(matches!(
            load_text("array", text),
            Err(ToolError::Fixture { .. })
        ));
    }

    #[test]
    fn an_invalid_timestamp_or_dataset_key_stops_the_load() {
        let bad_time = r#"{"prices":[{"start":"yesterday","spotCentsKwh":1}]}"#;
        assert!(matches!(
            load_text("time", bad_time),
            Err(ToolError::Fixture { .. })
        ));
        let bad_key = r#"{"prices":[],"fingridActuals":{"075":[]}}"#;
        assert!(matches!(
            load_text("key", bad_key),
            Err(ToolError::Fixture { .. })
        ));
    }
}
