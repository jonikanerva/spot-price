//! The Nord Pool Data Portal adapter: day-ahead prices of one delivery date.

use std::time::Duration;

use jiff::Timestamp;
use jiff::civil::Date;
use reqwest::{StatusCode, Url};
use serde_json::Value;
use spot_price_domain::areas::Area;

use super::{HttpClient, TransportError, array_field, string_field};
use crate::store::prices::PriceRow;

/// Retry limits of one fetch.
#[derive(Debug, Clone, Copy)]
pub struct RetrySettings {
    pub attempts: u32,
    pub delay: Duration,
}

impl Default for RetrySettings {
    fn default() -> Self {
        Self {
            attempts: 3,
            delay: Duration::from_secs(7),
        }
    }
}

/// A fetch that failed on every attempt with a transport error or an HTTP
/// error status. Schema drift is not an error: it degrades to no prices.
#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
pub enum NordPoolError {
    #[error("Nord Pool API returned HTTP {0}")]
    Status(u16),
    #[error("Nord Pool request failed: {0}")]
    Transport(TransportError),
}

impl NordPoolError {
    /// A fixed code for diagnostics.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Status(_) => "status",
            Self::Transport(error) => error.code(),
        }
    }
}

/// The body is not JSON or its envelope does not have the expected shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaDrift;

/// The outcome of one attempt.
enum Attempt {
    Prices(Vec<PriceRow>),
    Failed(NordPoolError),
    Drift,
}

/// The day-ahead price adapter.
#[derive(Debug, Clone)]
pub struct NordPoolClient {
    http: HttpClient,
    url: Url,
    retry: RetrySettings,
}

impl NordPoolClient {
    #[must_use]
    pub fn new(http: HttpClient, url: Url, retry: RetrySettings) -> Self {
        Self { http, url, retry }
    }

    /// The prices of `areas` for the delivery `date`. An empty list means
    /// "not published yet": HTTP 204 or 404, an empty body, or no price of
    /// the requested areas. These cases are not retried.
    ///
    /// A transport error, an HTTP error status, or schema drift is retried.
    /// When schema drift is the outcome of the last attempt, the fetch logs
    /// `NORDPOOL_SCHEMA_DRIFT` and returns no prices.
    ///
    /// # Errors
    ///
    /// Returns [`NordPoolError`] when the last attempt failed with a
    /// transport error or an HTTP error status.
    pub async fn fetch_day_ahead_prices(
        &self,
        date: Date,
        areas: &[Area],
    ) -> Result<Vec<PriceRow>, NordPoolError> {
        let attempts = self.retry.attempts.max(1);
        let mut attempt = 1;
        loop {
            let outcome = self.attempt(date, areas).await;
            let code = match outcome {
                Attempt::Prices(prices) => return Ok(prices),
                Attempt::Failed(error) if attempt >= attempts => return Err(error),
                Attempt::Drift if attempt >= attempts => {
                    tracing::error!(
                        code = "NORDPOOL_SCHEMA_DRIFT",
                        attempts,
                        "the Nord Pool body failed validation on every attempt; no prices"
                    );
                    return Ok(Vec::new());
                }
                Attempt::Failed(error) => error.code(),
                Attempt::Drift => "schema_drift",
            };
            tracing::warn!(
                code,
                attempt,
                attempts,
                "a Nord Pool fetch attempt failed; retrying"
            );
            tokio::time::sleep(self.retry.delay).await;
            attempt += 1;
        }
    }

    async fn attempt(&self, date: Date, areas: &[Area]) -> Attempt {
        let delivery_areas = areas
            .iter()
            .map(|area| area.code())
            .collect::<Vec<_>>()
            .join(",");
        let request = self.http.inner().get(self.url.clone()).query(&[
            ("date", date.to_string().as_str()),
            ("market", "DayAhead"),
            ("deliveryArea", delivery_areas.as_str()),
            ("currency", "EUR"),
        ]);
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                return Attempt::Failed(NordPoolError::Transport(TransportError::from_reqwest(
                    &error,
                )));
            }
        };
        let status = response.status();
        if status == StatusCode::NO_CONTENT || status == StatusCode::NOT_FOUND {
            return Attempt::Prices(Vec::new());
        }
        if !status.is_success() {
            return Attempt::Failed(NordPoolError::Status(status.as_u16()));
        }
        match self.http.read_body(response).await {
            Err(error) => Attempt::Failed(NordPoolError::Transport(error)),
            Ok(body) => match parse_day_ahead(&body, areas) {
                Ok(prices) => Attempt::Prices(prices),
                Err(SchemaDrift) => Attempt::Drift,
            },
        }
    }
}

/// Decodes a 2xx body. An empty body is "no data". The envelope must be an
/// object with a `multiAreaEntries` array; otherwise the body has drifted.
///
/// Each entry is checked on its own, so one malformed entry cannot drop the
/// good ones. An entry needs string `deliveryStart` and `deliveryEnd`, which
/// must be instants with an offset, and an `entryPerArea` object whose values
/// are all numbers or null. A null or absent price yields no row for that
/// area.
///
/// # Errors
///
/// Returns [`SchemaDrift`] when the body is not JSON or the envelope is
/// invalid.
pub fn parse_day_ahead(body: &[u8], areas: &[Area]) -> Result<Vec<PriceRow>, SchemaDrift> {
    if body.is_empty() {
        return Ok(Vec::new());
    }
    // `Value` keeps the last of repeated keys, like `JSON.parse`.
    let value: Value = serde_json::from_slice(body).map_err(|_| SchemaDrift)?;
    let entries = value
        .as_object()
        .and_then(|envelope| array_field(envelope, "multiAreaEntries"))
        .ok_or(SchemaDrift)?;
    let mut prices = Vec::new();
    for entry in entries {
        let Some(entry) = Entry::decode(entry) else {
            continue;
        };
        for &area in areas {
            if let Some(Some(price)) = entry.per_area.get(area.code()) {
                prices.push(PriceRow {
                    delivery_start: entry.delivery_start,
                    delivery_end: entry.delivery_end,
                    price_eur_mwh: *price,
                    area,
                });
            }
        }
    }
    Ok(prices)
}

/// One valid `multiAreaEntries` item.
struct Entry {
    delivery_start: Timestamp,
    delivery_end: Timestamp,
    per_area: std::collections::HashMap<String, Option<f64>>,
}

impl Entry {
    fn decode(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let delivery_start = string_field(object, "deliveryStart")?.parse().ok()?;
        let delivery_end = string_field(object, "deliveryEnd")?.parse().ok()?;
        let per_area = object
            .get("entryPerArea")?
            .as_object()?
            .iter()
            .map(|(code, price)| match price {
                Value::Null => Some((code.clone(), None)),
                Value::Number(number) => number.as_f64().map(|p| (code.clone(), Some(p))),
                _ => None,
            })
            .collect::<Option<_>>()?;
        Some(Self {
            delivery_start,
            delivery_end,
            per_area,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse(body: &Value) -> Result<Vec<PriceRow>, SchemaDrift> {
        parse_day_ahead(body.to_string().as_bytes(), &[Area::Fi])
    }

    fn fi(start: &str, end: &str, price: f64) -> PriceRow {
        PriceRow {
            delivery_start: start.parse().unwrap(),
            delivery_end: end.parse().unwrap(),
            price_eur_mwh: price,
            area: Area::Fi,
        }
    }

    #[test]
    fn parses_a_valid_body() {
        let body = json!({"multiAreaEntries": [{
            "deliveryStart": "2026-06-09T00:00:00Z",
            "deliveryEnd": "2026-06-09T01:00:00Z",
            "entryPerArea": {"FI": 42.5}
        }]});
        assert_eq!(
            parse(&body).unwrap(),
            [fi("2026-06-09T00:00:00Z", "2026-06-09T01:00:00Z", 42.5)]
        );
    }

    #[test]
    fn ignores_additive_envelope_and_entry_fields() {
        let body = json!({
            "deliveryDateCET": "2026-06-09",
            "updatedAt": "2026-06-08T11:00:00Z",
            "currency": "EUR",
            "areaStates": [{"state": "Final", "areas": ["FI"]}],
            "somethingBrandNew": {"nested": true},
            "multiAreaEntries": [{
                "deliveryStart": "2026-06-09T00:00:00Z",
                "deliveryEnd": "2026-06-09T01:00:00Z",
                "entryPerArea": {"FI": 50},
                "futureField": "ignored"
            }]
        });
        let prices = parse(&body).unwrap();
        assert_eq!(prices.len(), 1);
        assert!((prices[0].price_eur_mwh - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn skips_one_malformed_entry_and_keeps_the_good_price() {
        let body = json!({"multiAreaEntries": [
            {
                "deliveryStart": "2026-06-09T00:00:00Z",
                "deliveryEnd": "2026-06-09T01:00:00Z",
                "entryPerArea": {"FI": 31.2}
            },
            {"entryPerArea": "broken"},
            ["2026-06-09T01:00:00Z", "2026-06-09T02:00:00Z", {"FI": 1}],
            {
                "deliveryStart": "not an instant",
                "deliveryEnd": "2026-06-09T02:00:00Z",
                "entryPerArea": {"FI": 1}
            }
        ]});
        assert_eq!(
            parse(&body).unwrap(),
            [fi("2026-06-09T00:00:00Z", "2026-06-09T01:00:00Z", 31.2)]
        );
    }

    #[test]
    fn envelope_drift_and_invalid_json_are_drift_but_an_empty_body_is_no_data() {
        assert_eq!(parse(&json!({"unexpected": "shape"})), Err(SchemaDrift));
        assert_eq!(parse(&json!([{"multiAreaEntries": []}])), Err(SchemaDrift));
        assert_eq!(parse(&json!({"multiAreaEntries": {}})), Err(SchemaDrift));
        assert_eq!(
            parse_day_ahead(b"this is not json", &[Area::Fi]),
            Err(SchemaDrift)
        );
        assert_eq!(parse_day_ahead(b"", &[Area::Fi]), Ok(Vec::new()));
    }

    #[test]
    fn an_absent_area_or_a_wrong_price_type_yields_no_price() {
        let entry = |per_area: Value| {
            json!({"multiAreaEntries": [{
                "deliveryStart": "2026-06-09T00:00:00Z",
                "deliveryEnd": "2026-06-09T01:00:00Z",
                "entryPerArea": per_area
            }]})
        };
        assert_eq!(parse(&entry(json!({"SE1": 50}))), Ok(Vec::new()));
        assert_eq!(parse(&entry(json!({"FI": "x"}))), Ok(Vec::new()));
        assert_eq!(parse(&entry(json!({"FI": null}))), Ok(Vec::new()));
        // A wrong type in a sibling area fails the whole entry, like zod.
        assert_eq!(parse(&entry(json!({"FI": 1, "SE1": "x"}))), Ok(Vec::new()));
        assert_eq!(
            parse(&entry(json!({"FI": 31.2, "SE1": null}))).unwrap(),
            [fi("2026-06-09T00:00:00Z", "2026-06-09T01:00:00Z", 31.2)]
        );
    }

    #[test]
    fn the_last_of_repeated_keys_wins_like_json_parse() {
        let body = br#"{"multiAreaEntries": [{
            "deliveryStart": "2026-06-09T00:00:00Z",
            "deliveryEnd": "2026-06-09T01:00:00Z",
            "entryPerArea": {"FI": 1, "FI": 2}
        }]}"#;
        let prices = parse_day_ahead(body, &[Area::Fi]).unwrap();
        assert!((prices[0].price_eur_mwh - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn returns_one_row_per_requested_area_in_request_order() {
        let body = json!({"multiAreaEntries": [{
            "deliveryStart": "2026-06-09T00:00:00Z",
            "deliveryEnd": "2026-06-09T00:15:00Z",
            "entryPerArea": {"SE1": 2, "FI": 1, "EE": 3}
        }]});
        let prices = parse_day_ahead(
            body.to_string().as_bytes(),
            &[Area::Fi, Area::Se1, Area::Ee],
        )
        .unwrap();
        let areas: Vec<Area> = prices.iter().map(|p| p.area).collect();
        assert_eq!(areas, [Area::Fi, Area::Se1, Area::Ee]);
    }
}
