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
use sqlx::PgPool;

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

/// A Fingrid client without request spacing.
pub fn fingrid(server: &FakeServer) -> FingridClient {
    FingridClient::new(http_client(), server.url("/api/data")).with_request_spacing(Duration::ZERO)
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

/// Gives each `user_settings` row without a terms version the default
/// terms of a new account as its first version.
pub const ADD_DEFAULT_TERMS: &str = "
    INSERT INTO contract_terms
      (user_id, valid_from, margin_cents_kwh, transfer_day_cents_kwh, transfer_night_cents_kwh,
       tax_cents_kwh, vat_percent, night_start_hour, night_end_hour)
    SELECT user_id, DATE '1970-01-01', 0.49, 2.92, 1.37, 2.9179, 25.5, 22, 7
    FROM user_settings s
    WHERE NOT EXISTS (SELECT 1 FROM contract_terms t WHERE t.user_id = s.user_id)";

/// The `user_settings` and `"session"` tables of the TypeScript schema, as
/// temporary tables. On their connection they hide the tables of the same
/// names, so the unchanged golden seed loads into them.
const TYPESCRIPT_SEED_TABLES: &str = r#"
    CREATE TEMP TABLE user_settings (
      user_id TEXT PRIMARY KEY,
      margin_cents_kwh DOUBLE PRECISION NOT NULL DEFAULT 0.49,
      transfer_day_cents_kwh DOUBLE PRECISION NOT NULL DEFAULT 2.92,
      transfer_night_cents_kwh DOUBLE PRECISION NOT NULL DEFAULT 1.37,
      tax_cents_kwh DOUBLE PRECISION NOT NULL DEFAULT 2.82752,
      vat_percent DOUBLE PRECISION NOT NULL DEFAULT 25.5,
      night_start_hour INTEGER NOT NULL DEFAULT 22,
      night_end_hour INTEGER NOT NULL DEFAULT 7,
      timezone TEXT NOT NULL DEFAULT 'Europe/Helsinki',
      area TEXT NOT NULL DEFAULT 'FI',
      updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
    );
    CREATE TEMP TABLE "session" (
      "id" TEXT NOT NULL PRIMARY KEY,
      "expiresAt" TIMESTAMPTZ NOT NULL,
      "token" TEXT NOT NULL UNIQUE,
      "createdAt" TIMESTAMPTZ NOT NULL,
      "updatedAt" TIMESTAMPTZ NOT NULL,
      "ipAddress" TEXT,
      "userAgent" TEXT,
      "userId" TEXT NOT NULL
    )"#;

/// Moves the rows of the temporary tables to the schema of the service:
/// each settings row with its terms as the first version, like migration 4,
/// and each session with a 32-byte token as an app session of that token.
///
/// The seed has prices without VAT, and the service stores prices with VAT.
/// The move adds the VAT, and it fails when the service does not get the
/// seed price back bit for bit: the recorded price bodies depend on that.
const MOVE_TYPESCRIPT_SEED: &str = r#"
    INSERT INTO public.user_settings (user_id, timezone, area, updated_at)
    SELECT user_id, timezone, area, updated_at FROM pg_temp.user_settings;
    INSERT INTO contract_terms
      (user_id, valid_from, margin_cents_kwh, transfer_day_cents_kwh, transfer_night_cents_kwh,
       tax_cents_kwh, vat_percent, night_start_hour, night_end_hour)
    SELECT user_id, DATE '1970-01-01',
           margin_cents_kwh * (100 + vat_percent) / 100,
           transfer_day_cents_kwh * (100 + vat_percent) / 100,
           transfer_night_cents_kwh * (100 + vat_percent) / 100,
           tax_cents_kwh * (100 + vat_percent) / 100,
           vat_percent, night_start_hour, night_end_hour
    FROM pg_temp.user_settings;
    DO $$
    BEGIN
      IF EXISTS (
        SELECT 1 FROM contract_terms t JOIN pg_temp.user_settings s USING (user_id)
        WHERE t.margin_cents_kwh * 100 / (100 + t.vat_percent) <> s.margin_cents_kwh
           OR t.transfer_day_cents_kwh * 100 / (100 + t.vat_percent) <> s.transfer_day_cents_kwh
           OR t.transfer_night_cents_kwh * 100 / (100 + t.vat_percent)
              <> s.transfer_night_cents_kwh
           OR t.tax_cents_kwh * 100 / (100 + t.vat_percent) <> s.tax_cents_kwh
      ) THEN
        RAISE EXCEPTION 'a seed price does not survive the VAT round trip';
      END IF;
    END
    $$;
    INSERT INTO app_sessions (token_hash, user_id, created_at, expires_at)
    SELECT sha256(convert_to(token, 'UTF8')), "userId", "createdAt", "expiresAt"
    FROM pg_temp."session" WHERE octet_length(token) = 32;
    DROP TABLE pg_temp.user_settings, pg_temp."session""#;

/// Loads the seed of the HTTP golden cases, then `setup`. The seed has the
/// TypeScript schema: it writes the term columns of `user_settings` and the
/// table `"session"`, which the schema of the service does not have.
pub async fn load_golden_seed(pool: &PgPool, seed: &str, setup: Option<&str>) {
    // The temporary tables exist on one connection only.
    let mut connection = pool.acquire().await.unwrap();
    for sql in [Some(TYPESCRIPT_SEED_TABLES), Some(seed), setup] {
        let Some(sql) = sql else { continue };
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    sqlx::raw_sql(MOVE_TYPESCRIPT_SEED)
        .execute(&mut *connection)
        .await
        .unwrap();
}
