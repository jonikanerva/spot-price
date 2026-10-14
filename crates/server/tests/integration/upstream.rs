//! The upstream adapters against a local fake server: requests, status
//! handling, retries, deadlines, body limits, and redirects.

use std::time::Duration;

use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::json;
use spot_price_domain::areas::{ALL_AREAS, Area};
use spot_price_server::config::Secret;
use spot_price_server::upstream::fingrid::FingridClient;
use spot_price_server::upstream::nordpool::{NordPoolClient, NordPoolError, RetrySettings};
use spot_price_server::upstream::weather::{HELSINKI, WeatherClient};
use spot_price_server::upstream::{HttpClient, HttpSettings, TransportError};

use crate::common::{FakeServer, fingrid, json, nordpool, raw, ts, weather};

fn date() -> jiff::civil::Date {
    jiff::civil::date(2026, 6, 9)
}

fn one_price_body() -> serde_json::Value {
    json!({"multiAreaEntries": [{
        "deliveryStart": "2026-06-09T00:00:00Z",
        "deliveryEnd": "2026-06-09T01:00:00Z",
        "entryPerArea": {"FI": 42.5}
    }]})
}

fn short_timeouts(max_body_bytes: usize) -> HttpClient {
    HttpClient::new(HttpSettings {
        connect_timeout: Duration::from_secs(1),
        request_timeout: Duration::from_millis(300),
        max_body_bytes,
    })
    .unwrap()
}

/// A 200 response whose body never ends.
fn endless_body() -> Response {
    let stream = futures_util::stream::pending::<Result<Vec<u8>, std::io::Error>>();
    Response::builder()
        .status(StatusCode::OK)
        .body(Body::from_stream(stream))
        .unwrap()
}

/// A local port with nothing listening on it.
async fn closed_port_url(path: &str) -> reqwest::Url {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}{path}").parse().unwrap()
}

// --- Nord Pool ---------------------------------------------------------------

#[tokio::test]
async fn nordpool_sends_the_typescript_query_and_parses_prices() {
    let server = FakeServer::start(|_| json(StatusCode::OK, &one_price_body())).await;
    let prices = nordpool(&server)
        .fetch_day_ahead_prices(date(), &ALL_AREAS)
        .await
        .unwrap();
    assert_eq!(prices.len(), 1);
    assert_eq!(prices[0].area, Area::Fi);
    assert_eq!(prices[0].delivery_start, ts("2026-06-09T00:00:00Z"));

    let requests = server.recorded();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.path, "/api/DayAheadPrices");
    assert_eq!(request.param("date").as_deref(), Some("2026-06-09"));
    assert_eq!(request.param("market").as_deref(), Some("DayAhead"));
    assert_eq!(request.param("currency").as_deref(), Some("EUR"));
    assert_eq!(
        request.param("deliveryArea").as_deref(),
        Some("FI,SE1,SE2,SE3,SE4,NO1,NO2,NO3,NO4,NO5,DK1,DK2,EE,LT,LV,AT,BE,FR,GER,NL,PL")
    );
    assert!(
        request
            .user_agent
            .as_deref()
            .unwrap()
            .starts_with("spot-price/")
    );
}

#[tokio::test]
async fn nordpool_no_data_answers_are_not_retried() {
    for status in [
        StatusCode::NO_CONTENT,
        StatusCode::NOT_FOUND,
        StatusCode::OK,
    ] {
        let server = FakeServer::start(move |_| raw(status, "")).await;
        let prices = nordpool(&server)
            .fetch_day_ahead_prices(date(), &[Area::Fi])
            .await
            .unwrap();
        assert_eq!(prices.len(), 0, "{status}");
        assert_eq!(server.recorded().len(), 1, "{status}");
    }
}

#[tokio::test]
async fn nordpool_schema_drift_is_retried_then_degrades_to_no_prices() {
    for body in [
        json!({"unexpected": "shape"}).to_string(),
        "this is not json".to_owned(),
    ] {
        let server = FakeServer::start(move |_| raw(StatusCode::OK, body.clone())).await;
        let prices = nordpool(&server)
            .fetch_day_ahead_prices(date(), &[Area::Fi])
            .await
            .unwrap();
        assert_eq!(prices.len(), 0);
        assert_eq!(server.recorded().len(), 3);
    }
}

#[tokio::test]
async fn nordpool_an_error_status_is_retried_then_returned() {
    let server = FakeServer::start(|_| raw(StatusCode::INTERNAL_SERVER_ERROR, "")).await;
    let result = nordpool(&server)
        .fetch_day_ahead_prices(date(), &[Area::Fi])
        .await;
    assert_eq!(result, Err(NordPoolError::Status(500)));
    assert_eq!(server.recorded().len(), 3);
}

#[tokio::test]
async fn nordpool_a_later_attempt_can_succeed() {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = calls.clone();
    let server = FakeServer::start(move |_| {
        if counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            raw(StatusCode::BAD_GATEWAY, "")
        } else {
            json(StatusCode::OK, &one_price_body())
        }
    })
    .await;
    let prices = nordpool(&server)
        .fetch_day_ahead_prices(date(), &[Area::Fi])
        .await
        .unwrap();
    assert_eq!(prices.len(), 1);
    assert_eq!(server.recorded().len(), 2);
}

#[tokio::test]
async fn nordpool_does_not_follow_a_redirect() {
    let target = FakeServer::start(|_| json(StatusCode::OK, &one_price_body())).await;
    let location = target.url("/elsewhere").to_string();
    let server = FakeServer::start(move |_| {
        Response::builder()
            .status(StatusCode::FOUND)
            .header("location", location.as_str())
            .body(Body::empty())
            .unwrap()
    })
    .await;
    let result = nordpool(&server)
        .fetch_day_ahead_prices(date(), &[Area::Fi])
        .await;
    assert_eq!(result, Err(NordPoolError::Status(302)));
    assert_eq!(target.recorded().len(), 0);
}

#[tokio::test]
async fn nordpool_limits_the_body_and_the_attempt_time() {
    let big = "x".repeat(4096);
    let server = FakeServer::start(move |_| raw(StatusCode::OK, big.clone())).await;
    let client = NordPoolClient::new(
        short_timeouts(1024),
        server.url("/"),
        RetrySettings {
            attempts: 1,
            delay: Duration::ZERO,
        },
    );
    assert_eq!(
        client.fetch_day_ahead_prices(date(), &[Area::Fi]).await,
        Err(NordPoolError::Transport(TransportError::BodyTooLarge))
    );

    // The deadline covers the body, not only the headers.
    let server = FakeServer::start(|_| endless_body()).await;
    let client = NordPoolClient::new(
        short_timeouts(1024),
        server.url("/"),
        RetrySettings {
            attempts: 2,
            delay: Duration::ZERO,
        },
    );
    assert_eq!(
        client.fetch_day_ahead_prices(date(), &[Area::Fi]).await,
        Err(NordPoolError::Transport(TransportError::Timeout))
    );
    assert_eq!(server.recorded().len(), 2);
}

#[tokio::test]
async fn nordpool_a_refused_connection_is_a_transport_error() {
    let client = NordPoolClient::new(
        short_timeouts(1024),
        closed_port_url("/").await,
        RetrySettings {
            attempts: 1,
            delay: Duration::ZERO,
        },
    );
    assert_eq!(
        client.fetch_day_ahead_prices(date(), &[Area::Fi]).await,
        Err(NordPoolError::Transport(TransportError::Connect))
    );
}

// --- Fingrid -----------------------------------------------------------------

fn fingrid_key() -> Secret {
    Secret::new("test-fingrid-key".to_owned())
}

#[tokio::test]
async fn fingrid_sends_the_key_header_and_the_typescript_query() {
    let server = FakeServer::start(|_| {
        json(
            StatusCode::OK,
            &json!({
                "data": [{
                    "datasetId": 245,
                    "startTime": "2026-03-01T00:00:00.000Z",
                    "endTime": "2026-03-01T00:15:00.000Z",
                    "value": 4444.7
                }],
                "pagination": {"total": 1}
            }),
        )
    })
    .await;
    let records = fingrid(&server)
        .fetch_series(
            &fingrid_key(),
            ts("2026-03-01T00:00:00Z"),
            ts("2026-03-02T00:00:00.5Z"),
        )
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].dataset_id, 245);

    let request = &server.recorded()[0];
    assert_eq!(request.path, "/api/data");
    assert_eq!(request.api_key.as_deref(), Some("test-fingrid-key"));
    assert_eq!(request.param("datasets").as_deref(), Some("245,75,165,124"));
    assert_eq!(
        request.param("startTime").as_deref(),
        Some("2026-03-01T00:00:00.000Z")
    );
    assert_eq!(
        request.param("endTime").as_deref(),
        Some("2026-03-02T00:00:00.500Z")
    );
    assert_eq!(request.param("format").as_deref(), Some("json"));
    assert_eq!(request.param("pageSize").as_deref(), Some("20000"));
    assert!(!request.query.contains("test-fingrid-key"));
}

#[tokio::test]
async fn fingrid_degrades_on_error_statuses_and_bad_bodies() {
    let cases = [
        (
            StatusCode::UNAUTHORIZED,
            "{}",
            "Fingrid auth failed (HTTP 401)",
        ),
        (
            StatusCode::FORBIDDEN,
            "{}",
            "Fingrid auth failed (HTTP 403)",
        ),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            "Fingrid rejected the request (HTTP 422)",
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "{}",
            "Fingrid returned HTTP 503",
        ),
        (
            StatusCode::OK,
            r#"{"data": [{"datasetId": "not-a-number"}]}"#,
            "Fingrid response failed schema validation",
        ),
    ];
    for (status, body, reason) in cases {
        let server = FakeServer::start(move |_| raw(status, body)).await;
        let degraded = fingrid(&server)
            .fetch_series(
                &fingrid_key(),
                ts("2026-03-01T00:00:00Z"),
                ts("2026-03-02T00:00:00Z"),
            )
            .await
            .unwrap_err();
        assert_eq!(degraded.0, reason);
    }
}

#[tokio::test]
async fn fingrid_degrades_on_a_timeout_or_a_refused_connection() {
    let server = FakeServer::start(|_| endless_body()).await;
    let client = FingridClient::new(short_timeouts(1024), server.url("/"));
    let degraded = client
        .fetch_series(
            &fingrid_key(),
            ts("2026-03-01T00:00:00Z"),
            ts("2026-03-02T00:00:00Z"),
        )
        .await
        .unwrap_err();
    assert_eq!(degraded.0, "Fingrid request timed out");

    let client = FingridClient::new(short_timeouts(1024), closed_port_url("/").await);
    let degraded = client
        .fetch_series(
            &fingrid_key(),
            ts("2026-03-01T00:00:00Z"),
            ts("2026-03-02T00:00:00Z"),
        )
        .await
        .unwrap_err();
    assert_eq!(degraded.0, "Fingrid request failed: the connection failed");
}

// --- OpenWeatherMap ------------------------------------------------------------

fn owm_key() -> Secret {
    Secret::new("test-owm-key".to_owned())
}

#[tokio::test]
async fn weather_sends_one_request_with_the_typescript_query() {
    let server = FakeServer::start(|_| {
        json(
            StatusCode::OK,
            &json!({
                "hourly": [{"dt": 1_781_442_000, "temp": 11.46, "clouds": 100, "uvi": 0.12,
                            "wind_speed": 5.23, "wind_deg": 339}],
                "daily": []
            }),
        )
    })
    .await;
    let fetch = weather(&server)
        .fetch_weather(&owm_key(), HELSINKI, ts("2026-06-13T13:37:42Z"))
        .await;
    let hourly = fetch.hourly.unwrap();
    assert_eq!(hourly[0].target_time, ts("2026-06-14T13:00:00Z"));
    assert_eq!(hourly[0].issued_at, ts("2026-06-13T13:00:00Z"));
    assert_eq!(fetch.daily.unwrap(), []);

    let requests = server.recorded();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.path, "/data/3.0/onecall");
    assert_eq!(request.param("lat").as_deref(), Some("60.17"));
    assert_eq!(request.param("lon").as_deref(), Some("24.94"));
    assert_eq!(request.param("appid").as_deref(), Some("test-owm-key"));
    assert_eq!(request.param("units").as_deref(), Some("metric"));
    assert_eq!(
        request.param("exclude").as_deref(),
        Some("current,minutely,alerts")
    );
}

#[tokio::test]
async fn weather_degrades_both_blocks_on_error_statuses() {
    let cases = [
        (
            StatusCode::UNAUTHORIZED,
            "OpenWeatherMap auth failed (HTTP 401)",
        ),
        (
            StatusCode::FORBIDDEN,
            "OpenWeatherMap auth failed (HTTP 403)",
        ),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "OpenWeatherMap returned HTTP 500",
        ),
    ];
    for (status, reason) in cases {
        let server = FakeServer::start(move |_| raw(status, "{}")).await;
        let fetch = weather(&server)
            .fetch_weather(&owm_key(), HELSINKI, ts("2026-06-13T13:00:00Z"))
            .await;
        assert_eq!(fetch.hourly, Err(reason.to_owned()));
        assert_eq!(fetch.daily, Err(reason.to_owned()));
    }
}

#[tokio::test]
async fn weather_transport_reasons_never_hold_the_key() {
    let server = FakeServer::start(|_| endless_body()).await;
    let client = WeatherClient::new(short_timeouts(1024), server.url("/"));
    let fetch = client
        .fetch_weather(&owm_key(), HELSINKI, ts("2026-06-13T13:00:00Z"))
        .await;
    assert_eq!(
        fetch.hourly,
        Err("OpenWeatherMap request timed out".to_owned())
    );

    let client = WeatherClient::new(short_timeouts(1024), closed_port_url("/").await);
    let fetch = client
        .fetch_weather(&owm_key(), HELSINKI, ts("2026-06-13T13:00:00Z"))
        .await;
    let reason = fetch.daily.unwrap_err();
    assert_eq!(
        reason,
        "OpenWeatherMap request failed: the connection failed"
    );
    assert!(!reason.contains("test-owm-key"));
}
