//! Test helpers: a fake upstream server, clients that use it, and time
//! helpers.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use jiff::Timestamp;
use reqwest::Url;
use spot_price_server::upstream::fingrid::FingridClient;
use spot_price_server::upstream::nordpool::{NordPoolClient, RetrySettings};
use spot_price_server::upstream::weather::WeatherClient;
use spot_price_server::upstream::{HttpClient, HttpSettings};

/// One request that the fake server received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub path: String,
    pub query: String,
    pub api_key: Option<String>,
    pub user_agent: Option<String>,
}

impl Recorded {
    /// The decoded value of query parameter `name`.
    pub fn param(&self, name: &str) -> Option<String> {
        Url::parse(&format!("http://fake/?{}", self.query))
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    }
}

type Log = Arc<Mutex<Vec<Recorded>>>;
type Responder = Arc<dyn Fn(&Recorded) -> Response + Send + Sync>;

/// A fake upstream. `respond` gets each request and builds the response.
pub struct FakeServer {
    pub addr: SocketAddr,
    requests: Log,
    task: tokio::task::JoinHandle<()>,
}

impl FakeServer {
    pub async fn start(respond: impl Fn(&Recorded) -> Response + Send + Sync + 'static) -> Self {
        let requests: Log = Arc::new(Mutex::new(Vec::new()));
        let responder: Responder = Arc::new(respond);
        let app = Router::new()
            .fallback(
                |State((requests, respond)): State<(Log, Responder)>, request: Request| async move {
                    let header = |name: &str| {
                        request
                            .headers()
                            .get(name)
                            .map(|value| value.to_str().unwrap().to_owned())
                    };
                    let recorded = Recorded {
                        path: request.uri().path().to_owned(),
                        query: request.uri().query().unwrap_or_default().to_owned(),
                        api_key: header("x-api-key"),
                        user_agent: header("user-agent"),
                    };
                    requests.lock().unwrap().push(recorded.clone());
                    respond(&recorded)
                },
            )
            .with_state((requests.clone(), responder));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            addr,
            requests,
            task,
        }
    }

    pub fn url(&self, path: &str) -> Url {
        format!("http://{}{path}", self.addr).parse().unwrap()
    }

    pub fn recorded(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub fn json(status: StatusCode, body: &serde_json::Value) -> Response {
    raw(status, body.to_string())
}

pub fn raw(status: StatusCode, body: impl Into<axum::body::Body>) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(body.into())
        .unwrap()
}

pub fn http_client() -> HttpClient {
    HttpClient::new(HttpSettings {
        connect_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(5),
        max_body_bytes: 16 * 1024 * 1024,
    })
    .unwrap()
}

/// A Nord Pool client without retry delay.
pub fn nordpool(server: &FakeServer) -> NordPoolClient {
    NordPoolClient::new(
        http_client(),
        server.url("/api/DayAheadPrices"),
        RetrySettings {
            attempts: 3,
            delay: Duration::ZERO,
        },
    )
}

pub fn fingrid(server: &FakeServer) -> FingridClient {
    FingridClient::new(http_client(), server.url("/api/data"))
}

pub fn weather(server: &FakeServer) -> WeatherClient {
    WeatherClient::new(http_client(), server.url("/data/3.0/onecall"))
}

pub fn ts(iso: &str) -> Timestamp {
    iso.parse().unwrap()
}

/// `instant` plus `hours`, which can be negative.
pub fn hours(instant: Timestamp, hours: i64) -> Timestamp {
    instant
        .checked_add(jiff::SignedDuration::from_hours(hours))
        .unwrap()
}

/// `instant` plus `minutes`, which can be negative.
pub fn minutes(instant: Timestamp, minutes: i64) -> Timestamp {
    instant
        .checked_add(jiff::SignedDuration::from_mins(minutes))
        .unwrap()
}

/// `instant` plus `days` fixed 24-hour days, which can be negative.
pub fn days(instant: Timestamp, days: i64) -> Timestamp {
    hours(instant, days * 24)
}
