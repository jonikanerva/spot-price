//! Replays the HTTP golden cases of the TypeScript service
//! (`tests/golden/http/`, see its README) through the Rust router.
//!
//! Each case gets the seed in a clean database, a fresh service state, and a
//! clock pinned to the case. Every step must match status, headers, cookies,
//! and body bytes, except the differences in [`INTENTIONAL`]. A listed
//! difference that no longer occurs also fails the test, so the list stays
//! exact. `GOLDEN_ONLY=<substring>` replays only the matching cases.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::Value;
use spot_price_server::clock::Clock;
use spot_price_server::http::{AppState, router};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt as _;

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/http");

/// The differences from the TypeScript service, by design. Each entry is
/// `(case, step, kind, reason)`; `kind` is the first word of the mismatch.
const INTENTIONAL: &[(&str, usize, &str, &str)] = &[
    (
        "openapi-and-docs",
        0,
        "key-order",
        "the document is deep-equal to the TypeScript one; serde_json writes object keys in sorted order",
    ),
    (
        "session-signup-full-flow",
        13,
        "better-auth-cookies",
        "sign-out clears only the session cookie; Better Auth also cleared its session_data and dont_remember cookies, which the Rust service never sets",
    ),
    (
        "session-login-existing-user",
        9,
        "better-auth-cookies",
        "sign-out clears only the session cookie; Better Auth also cleared its session_data and dont_remember cookies, which the Rust service never sets",
    ),
    (
        "session-login-existing-user",
        10,
        "better-auth-cookies",
        "sign-out clears only the session cookie; Better Auth also cleared its session_data and dont_remember cookies, which the Rust service never sets",
    ),
    (
        "production-cookie-flow",
        5,
        "better-auth-cookies",
        "sign-out clears only the session cookie; Better Auth also cleared its session_data and dont_remember cookies, which the Rust service never sets",
    ),
    (
        "session-csrf-headers",
        0,
        "status",
        "the owner's Origin check: a state change from another origin answers 403 {\"error\":\"Forbidden\"}; the login limiter counted the request, so its headers are on the answer",
    ),
    (
        "session-csrf-headers",
        0,
        "extra-header",
        "the owner's Origin check: a state change from another origin answers 403 {\"error\":\"Forbidden\"}; the login limiter counted the request, so its headers are on the answer",
    ),
    (
        "session-csrf-headers",
        0,
        "cookie",
        "the owner's Origin check: a state change from another origin answers 403 {\"error\":\"Forbidden\"}; the login limiter counted the request, so its headers are on the answer",
    ),
    (
        "session-csrf-headers",
        0,
        "body",
        "the owner's Origin check: a state change from another origin answers 403 {\"error\":\"Forbidden\"}; the login limiter counted the request, so its headers are on the answer",
    ),
    (
        "session-csrf-headers",
        2,
        "status",
        "the owner's Origin check: a state change from another origin answers 403 {\"error\":\"Forbidden\"}; the login limiter counted the request, so its headers are on the answer",
    ),
    (
        "session-csrf-headers",
        2,
        "extra-header",
        "the owner's Origin check: a state change from another origin answers 403 {\"error\":\"Forbidden\"}; the login limiter counted the request, so its headers are on the answer",
    ),
    (
        "session-csrf-headers",
        2,
        "cookie",
        "the owner's Origin check: a state change from another origin answers 403 {\"error\":\"Forbidden\"}; the login limiter counted the request, so its headers are on the answer",
    ),
    (
        "session-csrf-headers",
        2,
        "body",
        "the owner's Origin check: a state change from another origin answers 403 {\"error\":\"Forbidden\"}; the login limiter counted the request, so its headers are on the answer",
    ),
    (
        "session-csrf-headers",
        3,
        "status",
        "the owner's Origin check: a state change from another origin answers 403 {\"error\":\"Forbidden\"}; the login limiter counted the request, so its headers are on the answer",
    ),
    (
        "session-csrf-headers",
        3,
        "extra-header",
        "the owner's Origin check: a state change from another origin answers 403 {\"error\":\"Forbidden\"}; the login limiter counted the request, so its headers are on the answer",
    ),
    (
        "session-csrf-headers",
        3,
        "cookie",
        "the owner's Origin check: a state change from another origin answers 403 {\"error\":\"Forbidden\"}; the login limiter counted the request, so its headers are on the answer",
    ),
    (
        "session-csrf-headers",
        3,
        "body",
        "the owner's Origin check: a state change from another origin answers 403 {\"error\":\"Forbidden\"}; the login limiter counted the request, so its headers are on the answer",
    ),
    (
        "session-csrf-headers",
        3,
        "volatile",
        "the owner's Origin check: a state change from another origin answers 403 {\"error\":\"Forbidden\"}; the login limiter counted the request, so its headers are on the answer",
    ),
    (
        "session-seeded-cookies",
        2,
        "body",
        "an expired session answers like no session; the TypeScript service accepted it because pg returned TIMESTAMPTZ as text",
    ),
    (
        "session-seeded-cookies",
        13,
        "status",
        "an expired session answers like no session; the TypeScript service accepted it because pg returned TIMESTAMPTZ as text",
    ),
    (
        "session-seeded-cookies",
        13,
        "body",
        "an expired session answers like no session; the TypeScript service accepted it because pg returned TIMESTAMPTZ as text",
    ),
    (
        "session-seeded-cookies",
        15,
        "status",
        "an expired session answers like no session; the TypeScript service accepted it because pg returned TIMESTAMPTZ as text",
    ),
    (
        "session-seeded-cookies",
        15,
        "body",
        "an expired session answers like no session; the TypeScript service accepted it because pg returned TIMESTAMPTZ as text",
    ),
    (
        "session-seeded-cookies",
        3,
        "cookie",
        "a session older than 1 day since its last renewal gets the expiry now + 7 days and the cookie again; the TypeScript service never renewed",
    ),
    (
        "session-seeded-cookies",
        3,
        "body",
        "a session older than 1 day since its last renewal gets the expiry now + 7 days and the cookie again; the TypeScript service never renewed",
    ),
    (
        "session-seeded-cookies",
        4,
        "body",
        "a session older than 1 day since its last renewal gets the expiry now + 7 days and the cookie again; the TypeScript service never renewed",
    ),
    (
        "session-chart",
        1,
        "cookie",
        "a session older than 1 day since its last renewal gets the expiry now + 7 days and the cookie again; the TypeScript service never renewed",
    ),
    (
        "forecast-success",
        0,
        "forecast-values",
        FORECAST_BEFORE_RULE,
    ),
    (
        "forecast-success",
        1,
        "forecast-values",
        FORECAST_BEFORE_RULE,
    ),
    (
        "forecast-success-stockholm-user",
        0,
        "forecast-values",
        FORECAST_BEFORE_RULE,
    ),
    (
        "forecast-success-stockholm-user",
        1,
        "forecast-values",
        FORECAST_BEFORE_RULE,
    ),
    (
        "forecast-one-day-history",
        0,
        "forecast-values",
        FORECAST_BEFORE_RULE,
    ),
    (
        "forecast-partial-fingrid-outage",
        0,
        "forecast-values",
        FORECAST_BEFORE_RULE,
    ),
    (
        "forecast-actuals-only",
        0,
        "forecast-values",
        FORECAST_BEFORE_RULE,
    ),
    (
        "forecast-sparse-fingrid",
        0,
        "forecast-values",
        FORECAST_BEFORE_RULE,
    ),
];

/// The reason of each `forecast-values` entry: the served forecast trains
/// `fi_latest_published` on the last FI price strictly before each quarter,
/// and the TypeScript bodies used the price at or before it.
const FORECAST_BEFORE_RULE: &str = "the forecast trains fi_latest_published on the last FI price strictly before each quarter, so only the estimate numbers differ; owner decision: https://github.com/jonikanerva/spot-price/issues/120#issuecomment-6246267911";

/// The forecast entry fields that hold estimate numbers. A `forecast-values`
/// difference may change only these values.
const FORECAST_VALUE_FIELDS: [&str; 6] = [
    "estimatedSpotCentsKwh",
    "estimatedTotalCentsKwh",
    "estimatedSpotLowCentsKwh",
    "estimatedSpotHighCentsKwh",
    "estimatedTotalLowCentsKwh",
    "estimatedTotalHighCentsKwh",
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    name: String,
    clock: String,
    profile: String,
    setup_sql: Option<String>,
    steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Step {
    clock: Option<String>,
    request: StepRequest,
    repeat: Option<usize>,
    repeat_statuses: Option<Vec<StatusRun>>,
    special: Option<String>,
    response: StepResponse,
    #[serde(default)]
    volatile: Vec<Volatile>,
}

#[derive(Debug, Deserialize)]
struct StepRequest {
    method: String,
    path: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StatusRun {
    status: u16,
    count: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StepResponse {
    status: u16,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    set_cookie: Vec<String>,
    body: Option<String>,
    body_file: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Volatile {
    pointer: String,
    pattern: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CookieVector {
    token: String,
    cookie_value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthVectors {
    cookie_vectors: Vec<CookieVector>,
}

/// A clock that the harness moves between steps.
#[derive(Debug)]
struct StepClock(Mutex<Timestamp>);

impl Clock for StepClock {
    fn now(&self) -> Timestamp {
        *self.0.lock().unwrap()
    }
}

/// One recorded response of the Rust service.
#[derive(Debug, Clone)]
struct Got {
    status: u16,
    headers: Vec<(String, String)>,
    set_cookie: Vec<String>,
    body: String,
}

/// A difference of one step: `(case, step, kind, detail)`.
type Mismatch = (String, usize, String, String);

fn read(path: &str) -> String {
    std::fs::read_to_string(format!("{DIR}/{path}")).unwrap()
}

/// The two patterns of the fixtures.
fn matches_pattern(pattern: &str, text: &str) -> bool {
    match pattern {
        "^[A-Za-z0-9]{32}$" => text.len() == 32 && text.bytes().all(|b| b.is_ascii_alphanumeric()),
        "^sp_[0-9a-f]{64}$" => text.strip_prefix("sp_").is_some_and(|hex| {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }),
        other => unreachable!("unknown pattern {other}"),
    }
}

const TS_SESSION_COOKIES: [&str; 2] = [
    "better-auth.session_token",
    "__Secure-better-auth.session_token",
];
const RUST_SESSION_COOKIES: [&str; 2] = ["spot_session", "__Host-spot_session"];

/// The JSON pointers that the Rust bodies do not have; see [`rust_shape`].
const ABSENT_POINTERS: [&str; 3] = ["/token", "/session/session/id", "/session/session/token"];

/// The TypeScript body in the documented Rust shape:
/// - A sign-in or sign-up body has `"token":null`: the session token goes
///   only into the `HttpOnly` cookie.
/// - The `session` object of `GET /api/session` has only `expiresAt`,
///   `createdAt`, and `userId`: no token, no id, no `updatedAt`, and no
///   always-null IP address and user agent.
fn rust_shape(body: &str) -> String {
    let mut out = body.to_owned();
    if let Some(rest) = out.strip_prefix(r#"{"session":{"session":"#)
        && let Some(end) = rest.find(r#","user":"#)
    {
        let inner: Value = serde_json::from_str(&rest[..end]).unwrap();
        let short = format!(
            r#"{{"expiresAt":{},"createdAt":{},"userId":{}}}"#,
            inner["expiresAt"], inner["createdAt"], inner["userId"]
        );
        out = format!(r#"{{"session":{{"session":{short}{}"#, &rest[end..]);
    }
    for prefix in [r#"{"token":""#, r#"{"redirect":false,"token":""#] {
        if let Some(rest) = out.strip_prefix(prefix) {
            let end = rest.find('"').unwrap();
            out = format!("{}null{}", &prefix[..prefix.len() - 1], &rest[end + 1..]);
            break;
        }
    }
    out
}

/// A `Set-Cookie` value with the session cookie name as `<session>` and a
/// non-empty value as `<volatile>`. Other Better Auth cookies give `None`.
fn normalize_cookie(line: &str) -> Option<String> {
    let (pair, attributes) = line.split_once("; ").unwrap_or((line, ""));
    let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
    if !TS_SESSION_COOKIES.contains(&name) && !RUST_SESSION_COOKIES.contains(&name) {
        return None;
    }
    let value = if value.is_empty() { "" } else { "<volatile>" };
    Some(format!("<session>={value}; {attributes}"))
}

/// The cookie text `<name>=<value>` of the session cookie of a response.
fn session_cookie(got: &Got) -> Option<String> {
    got.set_cookie.iter().find_map(|line| {
        let pair = line.split(';').next()?;
        let (name, _) = pair.split_once('=')?;
        RUST_SESSION_COOKIES
            .contains(&name)
            .then(|| pair.to_owned())
    })
}

/// `body` with each value of [`FORECAST_VALUE_FIELDS`] replaced by `#`.
/// `None` when a value is not a finite JSON number.
fn mask_forecast_values(body: &str) -> Option<String> {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    loop {
        let next = FORECAST_VALUE_FIELDS
            .iter()
            .filter_map(|field| {
                let key = format!("\"{field}\":");
                rest.find(&key).map(|at| (at, key.len()))
            })
            .min();
        let Some((at, key_len)) = next else {
            out.push_str(rest);
            return Some(out);
        };
        if rest[..at].ends_with('\\') {
            return None;
        }
        let value_start = at + key_len;
        let value_len = rest[value_start..]
            .find(|c: char| !matches!(c, '0'..='9' | '-' | '+' | '.' | 'e' | 'E'))
            .unwrap_or(rest.len() - value_start);
        let token = &rest[value_start..value_start + value_len];
        let number: serde_json::Number = serde_json::from_str(token).ok()?;
        if !number.as_f64().is_some_and(f64::is_finite) {
            return None;
        }
        out.push_str(&rest[..value_start]);
        out.push('#');
        rest = &rest[value_start + value_len..];
    }
}

/// The kind of a body difference: `key-order` for equal JSON values,
/// `forecast-values` for two forecast bodies that are byte for byte equal
/// after [`mask_forecast_values`], and `body` for any other difference.
fn body_difference(got: &str, want: &str) -> &'static str {
    let mine: Option<Value> = serde_json::from_str(got).ok();
    let theirs: Option<Value> = serde_json::from_str(want).ok();
    if mine.is_some() && mine == theirs {
        return "key-order";
    }
    let is_forecast = |value: &Option<Value>| {
        value.as_ref().and_then(|v| v.get("forecast")) == Some(&Value::Bool(true))
    };
    if is_forecast(&mine)
        && is_forecast(&theirs)
        && let (Some(got), Some(want)) = (mask_forecast_values(got), mask_forecast_values(want))
        && got == want
    {
        return "forecast-values";
    }
    "body"
}

struct Replay<'a> {
    case: &'a Case,
    app: Router,
    clock: Arc<StepClock>,
    pool: PgPool,
    history: Vec<Got>,
    mismatches: Vec<Mismatch>,
    seeded_cookies: &'a BTreeMap<String, String>,
}

impl Replay<'_> {
    fn mismatch(&mut self, step: usize, kind: &str, detail: String) {
        self.mismatches
            .push((self.case.name.clone(), step, kind.to_owned(), detail));
    }

    /// Fills `{{s<i>.json:<ptr>}}` and `{{s<i>.cookie:<name>}}`.
    fn fill(&self, value: &str) -> String {
        let mut out = value.to_owned();
        while let Some(start) = out.find("{{s") {
            let end = start + out[start..].find("}}").unwrap();
            let inner = &out[start + 3..end];
            let (index, rest) = inner.split_once('.').unwrap();
            let got = &self.history[index.parse::<usize>().unwrap()];
            let text = if let Some(pointer) = rest.strip_prefix("json:") {
                let body: Value = serde_json::from_str(&got.body).unwrap_or(Value::Null);
                body.pointer(pointer)
                    .and_then(Value::as_str)
                    .unwrap_or("<missing>")
                    .to_owned()
            } else {
                session_cookie(got).unwrap_or_else(|| "missing=1".to_owned())
            };
            out.replace_range(start..end + 2, &text);
        }
        out
    }

    /// Rewrites a signed seeded Better Auth cookie of the right name into
    /// the Rust cookie of the same token. Other cookies stay as they are.
    fn map_cookie(&self, header: &str) -> String {
        let (ts_name, rust_name) = if self.case.profile == "production" {
            (TS_SESSION_COOKIES[1], RUST_SESSION_COOKIES[1])
        } else {
            (TS_SESSION_COOKIES[0], RUST_SESSION_COOKIES[0])
        };
        header
            .split("; ")
            .map(|pair| match pair.split_once('=') {
                Some((name, value)) if name == ts_name => self
                    .seeded_cookies
                    .get(value)
                    .map_or_else(|| pair.to_owned(), |token| format!("{rust_name}={token}")),
                _ => pair.to_owned(),
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    async fn send(&self, step: &Step) -> Got {
        let mut builder = Request::builder()
            .method(step.request.method.as_str())
            .uri(&step.request.path);
        for (name, value) in &step.request.headers {
            let mut value = self.fill(value);
            if name == "cookie" {
                value = self.map_cookie(&value);
            }
            builder = builder.header(name, value);
        }
        let body = step
            .request
            .body
            .clone()
            .map_or_else(Body::empty, Body::from);
        let response: Response<Body> = self
            .app
            .clone()
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status().as_u16();
        let mut headers = Vec::new();
        let mut set_cookie = Vec::new();
        for (name, value) in response.headers() {
            let value = value.to_str().unwrap().to_owned();
            if name == "set-cookie" {
                set_cookie.push(value);
            } else {
                headers.push((name.as_str().to_owned(), value));
            }
        }
        let bytes = axum::body::to_bytes(response.into_body(), 16 << 20)
            .await
            .unwrap();
        let body = if step.request.method == "HEAD" {
            String::new()
        } else {
            String::from_utf8(bytes.to_vec()).unwrap()
        };
        Got {
            status,
            headers,
            set_cookie,
            body,
        }
    }

    async fn run(&mut self) {
        let case = self.case;
        for (index, step) in case.steps.iter().enumerate() {
            if let Some(clock) = &step.clock {
                *self.clock.0.lock().unwrap() = clock.parse().unwrap();
            }
            if step.special.as_deref() == Some("close-db") {
                self.pool.close().await;
            }
            let mut statuses = Vec::new();
            let mut got = self.send(step).await;
            statuses.push(got.status);
            for _ in 1..step.repeat.unwrap_or(1) {
                got = self.send(step).await;
                statuses.push(got.status);
            }
            if let Some(runs) = &step.repeat_statuses {
                let expected: Vec<u16> = runs
                    .iter()
                    .flat_map(|run| std::iter::repeat_n(run.status, run.count))
                    .collect();
                if expected != statuses {
                    self.mismatch(index, "repeat", format!("{statuses:?}"));
                }
            }
            self.compare(index, step, &got);
            self.history.push(got);
        }
    }

    fn compare(&mut self, index: usize, step: &Step, got: &Got) {
        let want = &step.response;
        if got.status != want.status {
            self.mismatch(
                index,
                "status",
                format!("want {} got {}", want.status, got.status),
            );
        }
        let got_headers: BTreeMap<&str, &str> = got
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        for (name, value) in &want.headers {
            if got_headers.get(name.as_str()) != Some(&value.as_str()) {
                self.mismatch(
                    index,
                    "header",
                    format!(
                        "{name}: want {value:?} got {:?}",
                        got_headers.get(name.as_str())
                    ),
                );
            }
        }
        for (name, value) in &got_headers {
            if !want.headers.contains_key(*name) && *name != "content-length" {
                self.mismatch(index, "extra-header", format!("{name}: {value}"));
            }
        }
        let want_cookies: Vec<String> = want
            .set_cookie
            .iter()
            .filter_map(|c| normalize_cookie(c))
            .collect();
        let got_cookies: Vec<String> = got
            .set_cookie
            .iter()
            .filter_map(|c| normalize_cookie(c))
            .collect();
        if want_cookies != got_cookies {
            self.mismatch(
                index,
                "cookie",
                format!("want {want_cookies:?} got {got_cookies:?}"),
            );
        }
        let other_cookies: Vec<&String> = want
            .set_cookie
            .iter()
            .filter(|c| normalize_cookie(c).is_none())
            .collect();
        if !other_cookies.is_empty() {
            self.mismatch(index, "better-auth-cookies", format!("{other_cookies:?}"));
        }
        self.compare_body(index, step, got);
    }

    fn compare_body(&mut self, index: usize, step: &Step, got: &Got) {
        let want = &step.response;
        let want_body = rust_shape(&match (&want.body, &want.body_file) {
            (Some(body), _) => body.clone(),
            (None, Some(file)) => read(file),
            (None, None) => String::new(),
        });
        let mut body = got.body.clone();
        if !step.volatile.is_empty() {
            let parsed: Value = serde_json::from_str(&got.body).unwrap_or(Value::Null);
            for volatile in step
                .volatile
                .iter()
                .filter(|volatile| !ABSENT_POINTERS.contains(&volatile.pointer.as_str()))
            {
                match parsed.pointer(&volatile.pointer).and_then(Value::as_str) {
                    Some(text) if matches_pattern(&volatile.pattern, text) => {
                        body = body.replace(&format!("\"{text}\""), "\"<volatile>\"");
                    }
                    other => self.mismatch(
                        index,
                        "volatile",
                        format!("{}: {other:?}", volatile.pointer),
                    ),
                }
            }
        }
        if body == want_body {
            return;
        }
        let kind = body_difference(&body, &want_body);
        if kind == "key-order" {
            self.mismatch(index, kind, "equal JSON values".to_owned());
        } else {
            let at = body
                .bytes()
                .zip(want_body.bytes())
                .position(|(a, b)| a != b)
                .unwrap_or(body.len().min(want_body.len()));
            let from = at.saturating_sub(60);
            let window = |text: &str| {
                text.get(from..(at + 100).min(text.len()))
                    .unwrap_or("")
                    .to_owned()
            };
            self.mismatch(
                index,
                kind,
                format!(
                    "at byte {at}: want …{}… got …{}…",
                    window(&want_body),
                    window(&body)
                ),
            );
        }
    }
}

/// Clears every application table and loads the seed and the case setup.
async fn reset(pool: &PgPool, seed: &str, setup: Option<&str>) {
    sqlx::raw_sql(
        r#"TRUNCATE app_sessions, api_keys, user_settings, usernames, "session", account,
           verification, "user", prices, fingrid_actuals, fingrid_forecasts,
           weather_forecasts, weather_daily_forecasts RESTART IDENTITY CASCADE"#,
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(seed.to_owned()))
        .execute(pool)
        .await
        .unwrap();
    if let Some(setup) = setup {
        sqlx::raw_sql(sqlx::AssertSqlSafe(setup.to_owned()))
            .execute(pool)
            .await
            .unwrap();
    }
    // Each seeded Better Auth session gets an app session of the same token.
    sqlx::raw_sql(
        r#"INSERT INTO app_sessions (token_hash, user_id, created_at, expires_at)
           SELECT sha256(convert_to(token, 'UTF8')), "userId", "createdAt", "expiresAt"
           FROM "session" WHERE octet_length(token) = 32"#,
    )
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_typescript_cases_replay(pool: PgPool) {
    let cases: Vec<Case> = serde_json::from_str(&read("cases.json")).unwrap();
    let vectors: AuthVectors = serde_json::from_str(&read("auth-vectors.json")).unwrap();
    let seeded_cookies: BTreeMap<String, String> = vectors
        .cookie_vectors
        .iter()
        .map(|v| {
            (
                v.cookie_value.clone(),
                spot_price_server::auth::random::hex(v.token.as_bytes()),
            )
        })
        .collect();
    let seed = read("seed.sql");
    let only = std::env::var("GOLDEN_ONLY").ok();
    let options = (*pool.connect_options()).clone();
    let mut mismatches = Vec::new();
    let mut replayed = 0;
    for case in &cases {
        if only
            .as_deref()
            .is_some_and(|only| !case.name.contains(only))
        {
            continue;
        }
        reset(&pool, &seed, case.setup_sql.as_deref()).await;
        let case_pool = PgPoolOptions::new()
            .max_connections(4)
            .connect_with(options.clone())
            .await
            .unwrap();
        let clock = Arc::new(StepClock(Mutex::new(case.clock.parse().unwrap())));
        let base_url = if case.profile == "production" {
            "https://spot.calmdonut.com"
        } else {
            "http://localhost:3000"
        };
        let state = AppState::new(case_pool.clone(), clock.clone(), &base_url.parse().unwrap());
        let mut replay = Replay {
            case,
            app: router(state),
            clock,
            pool: case_pool.clone(),
            history: Vec::new(),
            mismatches: Vec::new(),
            seeded_cookies: &seeded_cookies,
        };
        replay.run().await;
        mismatches.extend(replay.mismatches);
        case_pool.close().await;
        replayed += 1;
    }
    let listed: BTreeSet<(String, usize, String)> = INTENTIONAL
        .iter()
        .filter(|(case, ..)| only.as_deref().is_none_or(|only| case.contains(only)))
        .map(|(case, step, kind, _)| ((*case).to_owned(), *step, (*kind).to_owned()))
        .collect();
    let found: BTreeSet<(String, usize, String)> = mismatches
        .iter()
        .map(|(case, step, kind, _)| (case.clone(), *step, kind.clone()))
        .collect();
    let unexpected: Vec<&Mismatch> = mismatches
        .iter()
        .filter(|(case, step, kind, _)| !listed.contains(&(case.clone(), *step, kind.clone())))
        .collect();
    let stale: Vec<&(String, usize, String)> = listed.difference(&found).collect();
    let report = unexpected
        .iter()
        .map(|(case, step, kind, detail)| format!("{case} #{step} {kind}: {detail}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(replayed > 0, "no case matched GOLDEN_ONLY");
    assert!(
        unexpected.is_empty() && stale.is_empty(),
        "{} unexpected mismatches:\n{report}\nstale intentional entries: {stale:?}",
        unexpected.len()
    );
}

/// A forecast body with two entries; `{a}`, `{b}`, and `{rest}` fill in.
fn forecast_body(a: &str, b: &str, rest: &str) -> String {
    format!(
        r#"{{"forecast":true,"area":"FI","available":true,"degraded":false,"entries":[{{"start":"2026-10-25T22:00:00.000Z","estimatedSpotCentsKwh":{a},"estimatedTotalCentsKwh":13.951}},{{"start":"2026-10-25T22:15:00.000Z","estimatedSpotCentsKwh":{b},"estimatedTotalCentsKwh":14.905,"estimatedSpotLowCentsKwh":1.5,"estimatedSpotHighCentsKwh":9}}]{rest}}}"#
    )
}

#[test]
fn forecast_values_accepts_only_changed_estimate_numbers() {
    let want = forecast_body("6.429", "7.189", "");
    for got in [
        forecast_body("6.497", "7.189", ""),
        forecast_body("-0.5", "1e2", ""),
        want.replace("13.951", "14.037").replace(":9}", ":10.25}"),
    ] {
        assert_eq!(body_difference(&got, &want), "forecast-values", "{got}");
    }
}

#[test]
fn forecast_values_rejects_a_difference_outside_the_estimate_numbers() {
    let want = forecast_body("6.429", "7.189", "");
    let changed_number = forecast_body("6.497", "7.189", "");
    for got in [
        // Another field changes together with an estimate number.
        changed_number.replace(r#""degraded":false"#, r#""degraded":true"#),
        changed_number.replace("22:15:00.000Z", "22:30:00.000Z"),
        changed_number.replace(r#""area":"FI""#, r#""area":"SE3""#),
        // An estimate is not a finite JSON number.
        forecast_body("null", "7.189", ""),
        forecast_body(r#""6.497""#, "7.189", ""),
        forecast_body("+6.497", "7.189", ""),
        // A field, an entry, or a key goes missing, moves, or is added.
        changed_number.replace(r#","estimatedTotalCentsKwh":13.951"#, ""),
        changed_number.replace(
            r#""estimatedSpotCentsKwh":6.497,"estimatedTotalCentsKwh":13.951"#,
            r#""estimatedTotalCentsKwh":13.951,"estimatedSpotCentsKwh":6.497"#,
        ),
        forecast_body("6.497", "7.189", r#","reason":"x""#),
    ] {
        assert_eq!(body_difference(&got, &want), "body", "{got}");
    }
    // A body that is not a forecast never gets the forecast kind.
    let plain = |v: &str| format!(r#"{{"estimatedSpotCentsKwh":{v}}}"#);
    assert_eq!(body_difference(&plain("2"), &plain("1")), "body");
}

#[test]
fn equal_json_values_are_still_a_key_order_difference() {
    let want = forecast_body("6.429", "7.189", "");
    let got = want.replace(
        r#""area":"FI","available":true"#,
        r#""available":true,"area":"FI""#,
    );
    assert_eq!(body_difference(&got, &want), "key-order");
}
