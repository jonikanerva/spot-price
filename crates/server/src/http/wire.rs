//! The bytes of every response: JSON as `JSON.stringify` writes it, and the
//! exact header texts of the TypeScript service.
//!
//! A number is written like JavaScript `String(n)`: `5`, not `5.0`; `-0` is
//! `0`; and a value that is not finite is `null`. JSON key order is the field
//! order of the serialized struct.

use std::io;

use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderValue, StatusCode};
use axum::response::Response;
use jiff::Timestamp;
use serde::{Serialize, Serializer};
use spot_price_domain::js::number_to_string;

/// `Content-Type` of a JSON body.
pub const JSON: &str = "application/json";
/// `Content-Type` of a text body.
pub const TEXT: &str = "text/plain; charset=UTF-8";
/// `Content-Type` of the text of a request body that is not valid JSON.
pub const MALFORMED_TEXT: &str = "text/plain;charset=UTF-8";
/// `Content-Type` of an HTML page.
pub const HTML: &str = "text/html; charset=UTF-8";

/// The `serde_json` formatter of JavaScript numbers.
#[derive(Debug, Clone, Copy, Default)]
pub struct JsFormatter;

impl serde_json::ser::Formatter for JsFormatter {
    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(number_to_string(value).as_bytes())
    }

    fn write_f32<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        self.write_f64(writer, f64::from(value))
    }
}

/// The JSON text of `value`.
///
/// # Errors
///
/// Returns the error of a `Serialize` implementation.
pub fn to_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let mut bytes = Vec::with_capacity(256);
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, JsFormatter);
    value.serialize(&mut serializer)?;
    Ok(bytes)
}

/// A response with a body and a content type.
#[must_use]
pub fn body(status: StatusCode, content_type: &'static str, bytes: impl Into<Body>) -> Response {
    let mut response = Response::new(bytes.into());
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

/// A JSON response. A value that cannot be serialized gives the generic 500.
#[must_use]
pub fn json<T: Serialize + ?Sized>(status: StatusCode, value: &T) -> Response {
    if let Ok(bytes) = to_json(value) {
        body(status, JSON, bytes)
    } else {
        tracing::error!(
            code = "serialize",
            "a response body could not be serialized"
        );
        internal_error()
    }
}

/// The `{"error": message}` body of the API.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ErrorBody<'a> {
    pub error: &'a str,
}

/// A JSON error response `{"error": message}`.
#[must_use]
pub fn error(status: StatusCode, message: &str) -> Response {
    json(status, &ErrorBody { error: message })
}

/// The 404 of an unknown path or method: plain text `404 Not Found`.
#[must_use]
pub fn not_found() -> Response {
    body(StatusCode::NOT_FOUND, TEXT, "404 Not Found")
}

/// The generic 500 of an unexpected failure: plain text
/// `Internal Server Error`. The body never names the cause.
#[must_use]
pub fn internal_error() -> Response {
    body(
        StatusCode::INTERNAL_SERVER_ERROR,
        TEXT,
        "Internal Server Error",
    )
}

/// An instant as `Date.prototype.toISOString` writes it:
/// `YYYY-MM-DDTHH:MM:SS.sssZ`. A finer part is cut, not rounded.
#[must_use]
pub fn iso(at: Timestamp) -> String {
    let millis = spot_price_domain::time::truncate_to_millisecond(at);
    format!("{millis:.3}")
}

/// Serializes an instant with [`iso`].
///
/// # Errors
///
/// Returns the error of the serializer.
pub fn serialize_iso<S: Serializer>(at: &Timestamp, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&iso(*at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Numbers {
        integer: f64,
        negative_zero: f64,
        fraction: f64,
        large: f64,
        small: f64,
        not_a_number: f64,
        infinite: f64,
        int: i32,
    }

    #[test]
    fn writes_numbers_like_javascript() {
        let text = to_json(&Numbers {
            integer: 5.0,
            negative_zero: -0.0,
            fraction: 0.1 + 0.2,
            large: 1e21,
            small: 1e-7,
            not_a_number: f64::NAN,
            infinite: f64::NEG_INFINITY,
            int: -3,
        })
        .unwrap();
        assert_eq!(
            String::from_utf8(text).unwrap(),
            r#"{"integer":5,"negative_zero":0,"fraction":0.30000000000000004,"large":1e+21,"small":1e-7,"not_a_number":null,"infinite":null,"int":-3}"#
        );
        let more = to_json(&[123_456_789_012_345_680_000.0_f64, 2.5e-6, 12.345, -1.5]).unwrap();
        assert_eq!(
            String::from_utf8(more).unwrap(),
            "[123456789012345680000,0.0000025,12.345,-1.5]"
        );
    }

    #[test]
    fn escapes_strings_like_javascript() {
        let text = to_json("a\"b\\c\u{1}\n\u{7f}\u{2028}/").unwrap();
        assert_eq!(
            String::from_utf8(text).unwrap(),
            "\"a\\\"b\\\\c\\u0001\\n\u{7f}\u{2028}/\""
        );
    }

    #[test]
    fn writes_instants_with_milliseconds() {
        let at: Timestamp = "2026-02-27T10:00:00Z".parse().unwrap();
        assert_eq!(iso(at), "2026-02-27T10:00:00.000Z");
        let fine: Timestamp = "2026-02-27T10:00:00.123987Z".parse().unwrap();
        assert_eq!(iso(fine), "2026-02-27T10:00:00.123Z");
        let before_epoch: Timestamp = "1969-12-31T23:59:59.9999Z".parse().unwrap();
        assert_eq!(iso(before_epoch), "1969-12-31T23:59:59.999Z");
    }
}
