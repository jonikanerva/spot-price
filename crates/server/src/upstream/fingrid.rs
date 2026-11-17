//! The Fingrid Open Data adapter: public wind, consumption, and solar series.
//!
//! A fetch never fails with an error value that can stop a caller: every
//! failure is a [`Degraded`] reason, and the forecast job continues without
//! new data.

use std::time::Duration;

use jiff::Timestamp;
use reqwest::header::{ACCEPT, HeaderValue, WWW_AUTHENTICATE};
use reqwest::{StatusCode, Url};
use serde_json::Value;

use super::{HttpClient, TransportError, array_field, number_field, string_field};
use crate::config::Secret;
use crate::store::fingrid::{FORECAST_DATASETS, FingridRecord};

/// The page size of the request; one page holds the whole fetch window.
const MAX_PAGE_SIZE: u32 = 20_000;

/// The wait between two requests of one run. Fingrid allows one request
/// every two seconds per API key.
pub const REQUEST_SPACING: Duration = Duration::from_secs(2);

/// The reason of a degraded fetch. The text holds no URL or key.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Degraded(pub String);

/// Why Fingrid refused a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// HTTP 401: Fingrid answers it for a missing or invalid API key.
    KeyRejected,
    /// HTTP 403: something blocked the request. It is not the answer to an
    /// invalid key. `www_authenticate` is true when the response had a
    /// `WWW-Authenticate` header, which the Fingrid API management sends.
    Blocked { www_authenticate: bool },
}

impl Refusal {
    /// The refusal of a response with `status`, if the status is one.
    #[must_use]
    pub fn of(status: StatusCode, www_authenticate: bool) -> Option<Self> {
        match status {
            StatusCode::UNAUTHORIZED => Some(Self::KeyRejected),
            StatusCode::FORBIDDEN => Some(Self::Blocked { www_authenticate }),
            _ => None,
        }
    }

    /// A fixed code for diagnostics.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::KeyRejected => "fingrid_key_rejected",
            Self::Blocked { .. } => "fingrid_request_blocked",
        }
    }

    /// The fixed reason text of the degraded fetch.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::KeyRejected => "Fingrid rejected the API key (HTTP 401)",
            Self::Blocked { .. } => "Fingrid blocked the request (HTTP 403)",
        }
    }

    /// Logs the code. Only the presence of the header is logged, never its
    /// value.
    fn log(self) {
        match self {
            Self::KeyRejected => tracing::warn!(code = self.code(), "{}", self.reason()),
            Self::Blocked { www_authenticate } => {
                tracing::warn!(code = self.code(), www_authenticate, "{}", self.reason());
            }
        }
    }
}

/// The Fingrid adapter.
#[derive(Debug, Clone)]
pub struct FingridClient {
    http: HttpClient,
    url: Url,
    request_spacing: Duration,
}

impl FingridClient {
    #[must_use]
    pub fn new(http: HttpClient, url: Url) -> Self {
        Self {
            http,
            url,
            request_spacing: REQUEST_SPACING,
        }
    }

    /// The same client with another wait between two requests of one run.
    #[must_use]
    pub fn with_request_spacing(self, request_spacing: Duration) -> Self {
        Self {
            request_spacing,
            ..self
        }
    }

    /// Waits for the request spacing. Call it between two requests of one run.
    pub async fn wait_request_spacing(&self) {
        tokio::time::sleep(self.request_spacing).await;
    }

    /// The records of the four datasets of the served forecast with
    /// `startTime` in `[start, end)`.
    ///
    /// # Errors
    ///
    /// Returns [`Degraded`] as [`Self::fetch_datasets`] does.
    pub async fn fetch_series(
        &self,
        api_key: &Secret,
        start: Timestamp,
        end: Timestamp,
    ) -> Result<Vec<FingridRecord>, Degraded> {
        self.fetch_datasets(api_key, &FORECAST_DATASETS, start, end)
            .await
    }

    /// The records of `dataset_ids` with `startTime` in `[start, end)`, in
    /// one request. The body is not filtered by dataset.
    ///
    /// # Errors
    ///
    /// Returns [`Degraded`] for a [`Refusal`], another HTTP error status, a
    /// transport failure, or a body that fails validation.
    pub async fn fetch_datasets(
        &self,
        api_key: &Secret,
        dataset_ids: &[i32],
        start: Timestamp,
        end: Timestamp,
    ) -> Result<Vec<FingridRecord>, Degraded> {
        let mut key = HeaderValue::from_str(api_key.expose())
            .map_err(|_| Degraded("Fingrid API key is not a valid header value".to_owned()))?;
        key.set_sensitive(true);
        let datasets = dataset_ids
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let response = self
            .http
            .inner()
            .get(self.url.clone())
            .header("x-api-key", key)
            .header(ACCEPT, HeaderValue::from_static("application/json"))
            .query(&[
                ("datasets", datasets.as_str()),
                ("startTime", &iso_millis(start)),
                ("endTime", &iso_millis(end)),
                ("format", "json"),
                ("pageSize", &MAX_PAGE_SIZE.to_string()),
            ])
            .send()
            .await
            .map_err(|error| transport_degraded(TransportError::from_reqwest(&error)))?;

        let status = response.status();
        let www_authenticate = response.headers().contains_key(WWW_AUTHENTICATE);
        if let Some(refusal) = Refusal::of(status, www_authenticate) {
            refusal.log();
            return Err(Degraded(refusal.reason().to_owned()));
        }
        if status == StatusCode::UNPROCESSABLE_ENTITY {
            return Err(Degraded(
                "Fingrid rejected the request (HTTP 422)".to_owned(),
            ));
        }
        if !status.is_success() {
            return Err(Degraded(format!(
                "Fingrid returned HTTP {}",
                status.as_u16()
            )));
        }
        let body = self
            .http
            .read_body(response)
            .await
            .map_err(transport_degraded)?;
        parse_series(&body)
    }
}

/// Decodes the body. The envelope must be an object with a `data` array,
/// and every item must be valid: one invalid item fails the whole response.
///
/// # Errors
///
/// Returns [`Degraded`] when the body is not JSON or fails validation.
pub fn parse_series(body: &[u8]) -> Result<Vec<FingridRecord>, Degraded> {
    let invalid = || Degraded("Fingrid response failed schema validation".to_owned());
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| Degraded("Fingrid request failed: invalid JSON body".to_owned()))?;
    let items = value
        .as_object()
        .and_then(|envelope| array_field(envelope, "data"))
        .ok_or_else(invalid)?;
    items
        .iter()
        .map(|item| decode_record(item).ok_or_else(invalid))
        .collect()
}

fn decode_record(value: &Value) -> Option<FingridRecord> {
    let object = value.as_object()?;
    Some(FingridRecord {
        dataset_id: dataset_id(number_field(object, "datasetId")?)?,
        start_time: string_field(object, "startTime")?.parse().ok()?,
        end_time: string_field(object, "endTime")?.parse().ok()?,
        value: number_field(object, "value")?,
    })
}

/// A dataset id is an integer that fits the INTEGER column. JSON `245.0`
/// is the integer 245, as in JavaScript.
fn dataset_id(number: f64) -> Option<i32> {
    if number.fract() != 0.0 || !(f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&number) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the value is an integer inside the i32 range"
    )]
    Some(number as i32)
}

/// An instant as `YYYY-MM-DDTHH:MM:SS.mmmZ`, the format of
/// `Date.toISOString`.
#[must_use]
pub fn iso_millis(instant: Timestamp) -> String {
    instant.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

fn transport_degraded(error: TransportError) -> Degraded {
    match error {
        TransportError::Timeout => Degraded("Fingrid request timed out".to_owned()),
        other => Degraded(format!("Fingrid request failed: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse(body: &Value) -> Result<Vec<FingridRecord>, Degraded> {
        parse_series(body.to_string().as_bytes())
    }

    #[test]
    fn a_401_is_a_rejected_key_and_a_403_is_a_blocked_request() {
        for challenge in [false, true] {
            let rejected = Refusal::of(StatusCode::UNAUTHORIZED, challenge).unwrap();
            assert_eq!(rejected, Refusal::KeyRejected);
            assert_eq!(rejected.code(), "fingrid_key_rejected");
            assert_eq!(rejected.reason(), "Fingrid rejected the API key (HTTP 401)");

            let blocked = Refusal::of(StatusCode::FORBIDDEN, challenge).unwrap();
            assert_eq!(
                blocked,
                Refusal::Blocked {
                    www_authenticate: challenge
                }
            );
            assert_eq!(blocked.code(), "fingrid_request_blocked");
            assert_eq!(blocked.reason(), "Fingrid blocked the request (HTTP 403)");
        }
        for status in [
            StatusCode::OK,
            StatusCode::UNPROCESSABLE_ENTITY,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            assert_eq!(Refusal::of(status, true), None, "{status}");
        }
    }

    #[test]
    fn parses_a_valid_response() {
        let records = parse(&json!({
            "data": [{
                "datasetId": 245,
                "startTime": "2026-03-01T00:00:00.000Z",
                "endTime": "2026-03-01T00:15:00.000Z",
                "value": 4444.7
            }],
            "pagination": {"total": 1}
        }))
        .unwrap();
        assert_eq!(
            records,
            [FingridRecord {
                dataset_id: 245,
                start_time: "2026-03-01T00:00:00Z".parse().unwrap(),
                end_time: "2026-03-01T00:15:00Z".parse().unwrap(),
                value: 4444.7,
            }]
        );
    }

    #[test]
    fn one_invalid_item_fails_the_whole_response() {
        let valid = json!({
            "datasetId": 75,
            "startTime": "2026-03-01T00:00:00.000Z",
            "endTime": "2026-03-01T00:15:00.000Z",
            "value": 1
        });
        for invalid in [
            json!({"datasetId": "not-a-number"}),
            json!({"datasetId": 75.5, "startTime": "2026-03-01T00:00:00Z", "endTime": "2026-03-01T00:15:00Z", "value": 1}),
            json!({"datasetId": 75, "startTime": "yesterday", "endTime": "2026-03-01T00:15:00Z", "value": 1}),
            json!([75, "2026-03-01T00:00:00Z", "2026-03-01T00:15:00Z", 1]),
        ] {
            let error = parse(&json!({"data": [valid, invalid]})).unwrap_err();
            assert!(error.0.contains("schema"), "{invalid}");
        }
    }

    #[test]
    fn a_whole_number_dataset_id_is_an_integer() {
        let records = parse(&json!({"data": [{
            "datasetId": 245.0,
            "startTime": "2026-03-01T00:00:00Z",
            "endTime": "2026-03-01T00:15:00Z",
            "value": 1
        }]}))
        .unwrap();
        assert_eq!(records[0].dataset_id, 245);
    }

    #[test]
    fn an_invalid_envelope_or_body_degrades() {
        assert!(
            parse(&json!({"data": {}}))
                .unwrap_err()
                .0
                .contains("schema")
        );
        assert!(parse(&json!([])).unwrap_err().0.contains("schema"));
        assert!(parse_series(b"<html>").unwrap_err().0.contains("JSON"));
    }

    #[test]
    fn formats_instants_like_to_iso_string() {
        let instant: Timestamp = "2026-03-01T00:00:00.123456Z".parse().unwrap();
        assert_eq!(iso_millis(instant), "2026-03-01T00:00:00.123Z");
        let whole: Timestamp = "2026-03-01T00:00:00Z".parse().unwrap();
        assert_eq!(iso_millis(whole), "2026-03-01T00:00:00.000Z");
    }
}
