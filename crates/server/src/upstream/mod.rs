//! Outbound adapters of Nord Pool, Fingrid, and `OpenWeatherMap`
//! (STACK-RUST.md §2.3).
//!
//! One shared [`reqwest::Client`] sends every request: it has an explicit
//! User-Agent, no redirects, and a deadline for each attempt that includes
//! the body. Each adapter decodes the body through [`serde_json::Value`] and
//! checks the JSON types by hand, like the zod schemas of the TypeScript
//! service: an object must be an object, never an array.

pub mod fingrid;
pub mod nordpool;
pub mod weather;

use std::time::Duration;

use reqwest::{Response, Url, redirect};
use serde_json::{Map, Value};

/// Limits of the outbound requests.
#[derive(Debug, Clone, Copy)]
pub struct HttpSettings {
    pub connect_timeout: Duration,
    /// The deadline of one attempt, from the connection to the last body byte.
    pub request_timeout: Duration,
    /// The largest accepted body after decompression.
    pub max_body_bytes: usize,
}

impl Default for HttpSettings {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            // A 34-day Fingrid window of four datasets is about 2 MiB.
            max_body_bytes: 16 * 1024 * 1024,
        }
    }
}

/// The base URLs of the upstream APIs. Tests point them at a local server.
#[derive(Debug, Clone)]
pub struct UpstreamUrls {
    pub nordpool: Url,
    pub fingrid: Url,
    pub openweathermap: Url,
}

impl UpstreamUrls {
    /// The production endpoints.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when a constant URL does not parse.
    pub fn production() -> Result<Self, ClientError> {
        let parse = |url: &str| Url::parse(url).map_err(|_| ClientError);
        Ok(Self {
            nordpool: parse("https://dataportal-api.nordpoolgroup.com/api/DayAheadPrices")?,
            fingrid: parse("https://data.fingrid.fi/api/data")?,
            openweathermap: parse("https://api.openweathermap.org/data/3.0/onecall")?,
        })
    }
}

/// The HTTP client could not be built.
#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
#[error("the HTTP client could not be built")]
pub struct ClientError;

/// The shared client of all upstream adapters.
#[derive(Debug, Clone)]
pub struct HttpClient {
    http: reqwest::Client,
    max_body_bytes: usize,
}

impl HttpClient {
    /// Builds the client.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError`] when the TLS backend cannot start.
    pub fn new(settings: HttpSettings) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("spot-price/", env!("CARGO_PKG_VERSION")))
            // Fingrid gets its key in a header and OpenWeatherMap in the
            // query. A redirect must not carry either key anywhere.
            .redirect(redirect::Policy::none())
            .connect_timeout(settings.connect_timeout)
            .timeout(settings.request_timeout)
            .build()
            .map_err(|_| ClientError)?;
        Ok(Self {
            http,
            max_body_bytes: settings.max_body_bytes,
        })
    }

    pub(crate) fn inner(&self) -> &reqwest::Client {
        &self.http
    }

    /// Reads the whole body within the size limit.
    pub(crate) async fn read_body(
        &self,
        mut response: Response,
    ) -> Result<Vec<u8>, TransportError> {
        if response
            .content_length()
            .is_some_and(|length| length > self.max_body_bytes as u64)
        {
            return Err(TransportError::BodyTooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| TransportError::from_reqwest(&error))?
        {
            if body.len() + chunk.len() > self.max_body_bytes {
                return Err(TransportError::BodyTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

/// A failed transport. The variants carry no URL, header, or body, so they
/// are safe to log.
#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
pub enum TransportError {
    #[error("the request timed out")]
    Timeout,
    #[error("the connection failed")]
    Connect,
    #[error("the request failed")]
    Request,
    #[error("the response body is larger than the limit")]
    BodyTooLarge,
}

impl TransportError {
    /// Maps a reqwest error to a fixed kind. The error text can contain the
    /// request URL with a key, so it never leaves this function.
    pub(crate) fn from_reqwest(error: &reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout
        } else if error.is_connect() {
            Self::Connect
        } else {
            Self::Request
        }
    }

    /// A fixed code for diagnostics.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Connect => "connect",
            Self::Request => "request",
            Self::BodyTooLarge => "body_too_large",
        }
    }
}

/// A JSON number field of `object`, like `z.number()`.
pub(crate) fn number_field(object: &Map<String, Value>, key: &str) -> Option<f64> {
    object.get(key).and_then(Value::as_f64)
}

/// A JSON string field of `object`, like `z.string()`.
pub(crate) fn string_field<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

/// A JSON array field of `object`, like `z.array(...)`.
pub(crate) fn array_field<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a [Value]> {
    object.get(key).and_then(Value::as_array).map(Vec::as_slice)
}
