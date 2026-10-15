//! The `OpenWeatherMap` One Call API 3.0 adapter.
//!
//! Weather data is from `OpenWeatherMap` (<https://openweathermap.org>) under
//! the Open Data Commons Open Database License (`ODbL`). The service stores it
//! only to derive its own forecast and never redistributes it.
//!
//! One request per point returns an hourly and a daily block. The two blocks
//! are validated independently, so a failure in one block never drops the
//! records of the other.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use reqwest::header::{ACCEPT, HeaderValue};
use reqwest::{StatusCode, Url};
use serde_json::{Map, Value};

use super::{HttpClient, TransportError, array_field, number_field};
use crate::clock::truncate_to_utc_hour;
use crate::config::Secret;
use crate::store::weather::{WeatherDailyRecord, WeatherRecord};

/// A fixed collection point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeatherPoint {
    pub id: &'static str,
    pub lat: f64,
    pub lon: f64,
}

/// The southern demand and solar centre.
pub const HELSINKI: WeatherPoint = WeatherPoint {
    id: "helsinki",
    lat: 60.17,
    lon: 24.94,
};

/// The west-coast wind region.
pub const VAASA: WeatherPoint = WeatherPoint {
    id: "vaasa",
    lat: 63.1,
    lon: 21.62,
};

/// The points of the weather job. History accumulates only for points
/// collected from their first deploy, so adding one is an explicit decision.
pub const WEATHER_POINTS: [WeatherPoint; 2] = [HELSINKI, VAASA];

/// The largest absolute epoch second that JavaScript `Date` can hold. The
/// daily schema accepts only values inside it.
const MAX_EPOCH_SECONDS: f64 = 8.64e12;

/// The outcome of one point. `hourly` and `daily` fail independently; a
/// transport or HTTP failure fails both with the same reason.
#[derive(Debug, Clone, PartialEq)]
pub struct WeatherFetch {
    pub hourly: Result<Vec<WeatherRecord>, String>,
    pub daily: Result<Vec<WeatherDailyRecord>, String>,
}

impl WeatherFetch {
    fn degraded(reason: String) -> Self {
        Self {
            hourly: Err(reason.clone()),
            daily: Err(reason),
        }
    }
}

/// The weather adapter.
#[derive(Debug, Clone)]
pub struct WeatherClient {
    http: HttpClient,
    url: Url,
}

impl WeatherClient {
    #[must_use]
    pub fn new(http: HttpClient, url: Url) -> Self {
        Self { http, url }
    }

    /// The forecast of `point`, issued at `issued_at` truncated to the UTC
    /// hour. This is one billed call; `exclude` never names `daily`, so the
    /// daily block costs no extra call (STACK.md §9).
    pub async fn fetch_weather(
        &self,
        api_key: &Secret,
        point: WeatherPoint,
        issued_at: Timestamp,
    ) -> WeatherFetch {
        let response = match self
            .http
            .inner()
            .get(self.url.clone())
            .header(ACCEPT, HeaderValue::from_static("application/json"))
            .query(&[
                ("lat", point.lat.to_string().as_str()),
                ("lon", point.lon.to_string().as_str()),
                ("appid", api_key.expose()),
                ("units", "metric"),
                ("exclude", "current,minutely,alerts"),
            ])
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                return WeatherFetch::degraded(transport_reason(TransportError::from_reqwest(
                    &error,
                )));
            }
        };
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return WeatherFetch::degraded(format!(
                "OpenWeatherMap auth failed (HTTP {})",
                status.as_u16()
            ));
        }
        if !status.is_success() {
            return WeatherFetch::degraded(format!(
                "OpenWeatherMap returned HTTP {}",
                status.as_u16()
            ));
        }
        match self.http.read_body(response).await {
            Err(error) => WeatherFetch::degraded(transport_reason(error)),
            Ok(body) => parse_weather(&body, point, issued_at),
        }
    }
}

/// Decodes one body into both blocks.
#[must_use]
pub fn parse_weather(body: &[u8], point: WeatherPoint, issued_at: Timestamp) -> WeatherFetch {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return WeatherFetch::degraded(
            "OpenWeatherMap request failed: invalid JSON body".to_owned(),
        );
    };
    let issued_at = truncate_to_utc_hour(issued_at);
    WeatherFetch {
        hourly: parse_hourly(&value, point, issued_at),
        daily: parse_daily(&value, point, issued_at),
    }
}

fn parse_hourly(
    value: &Value,
    point: WeatherPoint,
    issued_at: Timestamp,
) -> Result<Vec<WeatherRecord>, String> {
    let invalid = || "OpenWeatherMap response failed schema validation".to_owned();
    let entries = value
        .as_object()
        .and_then(|envelope| array_field(envelope, "hourly"))
        .ok_or_else(invalid)?;
    entries
        .iter()
        .map(|entry| {
            let object = entry.as_object()?;
            Some(WeatherRecord {
                point_id: point.id.to_owned(),
                issued_at,
                target_time: epoch_seconds(number_field(object, "dt")?)?,
                temp: number_field(object, "temp")?,
                clouds: number_field(object, "clouds")?,
                uvi: number_field(object, "uvi")?,
                wind_speed: number_field(object, "wind_speed")?,
                wind_deg: number_field(object, "wind_deg")?,
            })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(invalid)
}

fn parse_daily(
    value: &Value,
    point: WeatherPoint,
    issued_at: Timestamp,
) -> Result<Vec<WeatherDailyRecord>, String> {
    let invalid = || "OpenWeatherMap daily block failed schema validation".to_owned();
    let entries = value
        .as_object()
        .and_then(|envelope| array_field(envelope, "daily"))
        .ok_or_else(invalid)?;
    entries
        .iter()
        .map(|entry| decode_daily(entry, point, issued_at))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(invalid)
}

fn decode_daily(
    value: &Value,
    point: WeatherPoint,
    issued_at: Timestamp,
) -> Option<WeatherDailyRecord> {
    let object = value.as_object()?;
    let temp = object.get("temp")?.as_object()?;
    let target_dt = bounded_epoch_seconds(number_field(object, "dt")?)?;
    Some(WeatherDailyRecord {
        point_id: point.id.to_owned(),
        issued_at,
        // The UTC calendar date; equal to the local date of any point between
        // UTC-11 and UTC+11. STACK.md §7 forbids local-time arithmetic here.
        target_date: target_dt.to_zoned(TimeZone::UTC).date(),
        target_dt,
        temp_morn: number_field(temp, "morn")?,
        temp_day: number_field(temp, "day")?,
        temp_eve: number_field(temp, "eve")?,
        temp_night: number_field(temp, "night")?,
        temp_min: number_field(temp, "min")?,
        temp_max: number_field(temp, "max")?,
        clouds: number_field(object, "clouds")?,
        uvi: number_field(object, "uvi")?,
        sunrise: optional_epoch(object, "sunrise")?,
        sunset: optional_epoch(object, "sunset")?,
    })
}

/// An optional epoch field: absent is `Some(None)`. `null` is invalid, like
/// zod `.optional()`.
#[expect(
    clippy::option_option,
    reason = "the outer None marks an invalid field for the collecting map"
)]
fn optional_epoch(object: &Map<String, Value>, key: &str) -> Option<Option<Timestamp>> {
    match object.get(key) {
        None => Some(None),
        Some(value) => bounded_epoch_seconds(value.as_f64()?).map(Some),
    }
}

/// Epoch seconds inside the `Date` range of the daily schema.
fn bounded_epoch_seconds(seconds: f64) -> Option<Timestamp> {
    if !(-MAX_EPOCH_SECONDS..=MAX_EPOCH_SECONDS).contains(&seconds) {
        return None;
    }
    epoch_seconds(seconds)
}

/// Epoch seconds as an instant, truncated to the millisecond like
/// `new Date(seconds * 1000)`. `None` outside the range of [`Timestamp`].
fn epoch_seconds(seconds: f64) -> Option<Timestamp> {
    let millis = (seconds * 1000.0).trunc();
    // 8.64e15 ms is the JavaScript Date range and fits i64.
    if !(-8.64e15..=8.64e15).contains(&millis) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the value is an integer inside the i64 range"
    )]
    Timestamp::from_millisecond(millis as i64).ok()
}

fn transport_reason(error: TransportError) -> String {
    match error {
        TransportError::Timeout => "OpenWeatherMap request timed out".to_owned(),
        other => format!("OpenWeatherMap request failed: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const ISSUED_AT: &str = "2026-06-13T13:37:42Z";

    fn epoch(iso: &str) -> i64 {
        iso.parse::<Timestamp>().unwrap().as_second()
    }

    fn valid_hourly() -> Value {
        json!({
            "dt": 1_781_442_000,
            "temp": 11.46,
            "clouds": 100,
            "uvi": 0.12,
            "wind_speed": 5.23,
            "wind_deg": 339,
            "feels_like": 10.9,
            "pressure": 1011,
            "humidity": 87,
            "pop": 0.4
        })
    }

    fn valid_daily() -> Value {
        json!({
            "dt": epoch("2026-06-14T09:00:00Z"),
            "sunrise": epoch("2026-06-14T01:03:00Z"),
            "sunset": epoch("2026-06-14T19:48:00Z"),
            "temp": {"morn": 9.5, "day": 15.2, "eve": 13.1, "night": 10.4, "min": 8.8, "max": 16.3},
            "clouds": 42,
            "uvi": 4.2,
            "feels_like": {"day": 14.8, "night": 9.9, "eve": 12.7, "morn": 8.9},
            "pressure": 1011,
            "summary": "Expect a day of partly cloudy weather",
            "moonrise": epoch("2026-06-14T02:10:00Z"),
            "moon_phase": 0.75
        })
    }

    fn parse(body: &Value) -> WeatherFetch {
        parse_weather(
            body.to_string().as_bytes(),
            HELSINKI,
            ISSUED_AT.parse().unwrap(),
        )
    }

    fn ts(iso: &str) -> Timestamp {
        iso.parse().unwrap()
    }

    #[test]
    fn maps_an_hourly_entry_and_truncates_the_issuance_to_the_hour() {
        let fetch = parse(&json!({"hourly": [valid_hourly()]}));
        let records = fetch.hourly.unwrap();
        assert_eq!(
            records,
            [WeatherRecord {
                point_id: "helsinki".to_owned(),
                issued_at: ts("2026-06-13T13:00:00Z"),
                target_time: ts("2026-06-14T13:00:00Z"),
                temp: 11.46,
                clouds: 100.0,
                uvi: 0.12,
                wind_speed: 5.23,
                wind_deg: 339.0,
            }]
        );
    }

    #[test]
    fn maps_a_daily_entry_with_the_utc_calendar_date() {
        let fetch = parse(&json!({"hourly": [valid_hourly()], "daily": [valid_daily()]}));
        let records = fetch.daily.unwrap();
        assert_eq!(
            records,
            [WeatherDailyRecord {
                point_id: "helsinki".to_owned(),
                issued_at: ts("2026-06-13T13:00:00Z"),
                target_date: jiff::civil::date(2026, 6, 14),
                target_dt: ts("2026-06-14T09:00:00Z"),
                temp_morn: 9.5,
                temp_day: 15.2,
                temp_eve: 13.1,
                temp_night: 10.4,
                temp_min: 8.8,
                temp_max: 16.3,
                clouds: 42.0,
                uvi: 4.2,
                sunrise: Some(ts("2026-06-14T01:03:00Z")),
                sunset: Some(ts("2026-06-14T19:48:00Z")),
            }]
        );
    }

    #[test]
    fn derives_the_target_date_on_the_utc_day_boundary() {
        let mut last = valid_daily();
        last["dt"] = json!(epoch("2026-06-14T23:59:59Z"));
        let mut first = valid_daily();
        first["dt"] = json!(epoch("2026-06-15T00:00:00Z"));
        let fetch = parse(&json!({"hourly": [], "daily": [last, first]}));
        let dates: Vec<_> = fetch.daily.unwrap().iter().map(|r| r.target_date).collect();
        assert_eq!(
            dates,
            [
                jiff::civil::date(2026, 6, 14),
                jiff::civil::date(2026, 6, 15)
            ]
        );
    }

    #[test]
    fn absent_solar_bounds_are_none_but_null_is_invalid() {
        let mut polar = valid_daily();
        polar.as_object_mut().unwrap().remove("sunrise");
        polar.as_object_mut().unwrap().remove("sunset");
        let records = parse(&json!({"hourly": [], "daily": [polar]}))
            .daily
            .unwrap();
        assert_eq!(records[0].sunrise, None);
        assert_eq!(records[0].sunset, None);
        assert!((records[0].uvi - 4.2).abs() < f64::EPSILON);

        let mut null = valid_daily();
        null["sunrise"] = Value::Null;
        assert!(
            parse(&json!({"hourly": [], "daily": [null]}))
                .daily
                .is_err()
        );
    }

    #[test]
    fn a_malformed_hourly_block_degrades_with_a_schema_reason() {
        let fetch = parse(&json!({"hourly": [{"dt": "not-a-number"}]}));
        assert!(fetch.hourly.unwrap_err().contains("schema"));
    }

    #[test]
    fn a_malformed_or_missing_daily_block_keeps_every_hourly_record() {
        for body in [
            json!({"hourly": [valid_hourly()], "daily": [{"dt": "not-a-number", "temp": "nonsense"}]}),
            json!({"hourly": [valid_hourly()]}),
        ] {
            let fetch = parse(&body);
            assert_eq!(fetch.hourly.unwrap().len(), 1);
            assert!(fetch.daily.unwrap_err().contains("daily"));
        }
    }

    #[test]
    fn a_malformed_hourly_block_keeps_a_valid_daily_block() {
        let fetch = parse(&json!({"hourly": [{"dt": "not-a-number"}], "daily": [valid_daily()]}));
        assert!(fetch.hourly.is_err());
        assert_eq!(fetch.daily.unwrap().len(), 1);
    }

    #[test]
    fn an_out_of_range_daily_epoch_fails_only_the_daily_block() {
        for (field, value) in [("dt", 1e13), ("sunrise", -1e13)] {
            let mut daily = valid_daily();
            daily[field] = json!(value);
            let fetch = parse(&json!({"hourly": [valid_hourly()], "daily": [daily]}));
            assert_eq!(fetch.hourly.unwrap().len(), 1, "{field}");
            let reason = fetch.daily.unwrap_err();
            assert!(reason.contains("schema validation"), "{field}");
        }
    }

    #[test]
    fn an_out_of_range_hourly_epoch_fails_only_the_hourly_block() {
        let mut hourly = valid_hourly();
        hourly["dt"] = json!(1e13);
        let fetch = parse(&json!({"hourly": [hourly], "daily": [valid_daily()]}));
        assert!(fetch.hourly.unwrap_err().contains("schema"));
        assert_eq!(fetch.daily.unwrap().len(), 1);
    }

    #[test]
    fn tolerates_unknown_daily_fields_and_any_array_length() {
        let daily: Vec<Value> = (0..8)
            .map(|i| {
                let mut entry = valid_daily();
                entry["dt"] = json!(epoch("2026-06-14T09:00:00Z") + i * 86_400);
                entry["brand_new_owm_field"] = json!("whatever");
                entry
            })
            .collect();
        let records = parse(&json!({"hourly": [valid_hourly()], "daily": daily}))
            .daily
            .unwrap();
        assert_eq!(records.len(), 8);
        assert_eq!(records[7].target_date, jiff::civil::date(2026, 6, 21));
    }

    #[test]
    fn invalid_json_or_a_non_object_degrades_both_blocks() {
        let fetch = parse_weather(b"nope", HELSINKI, ts(ISSUED_AT));
        assert!(fetch.hourly.is_err());
        assert!(fetch.daily.is_err());
        let fetch = parse(&json!([{"hourly": []}]));
        assert!(fetch.hourly.is_err());
        assert!(fetch.daily.is_err());
    }

    #[test]
    fn fractional_epoch_seconds_truncate_to_the_millisecond() {
        let mut hourly = valid_hourly();
        hourly["dt"] = json!(1_781_442_000.123_9);
        let records = parse(&json!({"hourly": [hourly]})).hourly.unwrap();
        assert_eq!(records[0].target_time, ts("2026-06-14T13:00:00.123Z"));
    }
}
