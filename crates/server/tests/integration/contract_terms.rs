//! The versions of the contract terms: the settings routes that save, list,
//! and delete them, and the price routes across a day on which the terms
//! change.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use spot_price_domain::areas::Area;
use spot_price_domain::price::apply_contract_terms;
use spot_price_domain::settings::{Hour, UserId, UserSettings};
use spot_price_domain::time::UserTimeZone;
use spot_price_server::clock::FixedClock;
use spot_price_server::http::{AppState, router};
use sqlx::PgPool;
use tower::ServiceExt as _;

use crate::common::{COPY_SETTINGS_TO_TERMS, ts};

/// 14:00 in Helsinki on 2026-11-30, the day before the terms change.
const EVE: &str = "2026-11-30T12:00:00Z";
/// The local date of the change. It starts at 2026-11-30T22:00Z in Helsinki.
const CHANGE: &str = "2026-12-01";
const ORIGIN: &str = "http://localhost:3000";

fn app_at(pool: PgPool, now: &str) -> Router {
    router(AppState::new(
        pool,
        Arc::new(FixedClock(ts(now))),
        &ORIGIN.parse().unwrap(),
    ))
}

/// Sends a request with the session cookie or the API key `auth`.
async fn send(
    app: &Router,
    method: &str,
    path: &str,
    auth: &str,
    body: Option<&str>,
) -> (StatusCode, String) {
    let mut request = Request::builder().method(method).uri(path);
    request = if auth.starts_with("sp_") {
        request.header("authorization", format!("Bearer {auth}"))
    } else if auth.is_empty() {
        request
    } else {
        request.header("cookie", auth)
    };
    let body = match body {
        Some(text) => {
            request = request.header("content-type", "application/json");
            Body::from(text.to_owned())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 24)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

/// The JSON body of a request that must answer 200.
async fn ok(app: &Router, method: &str, path: &str, auth: &str, body: Option<&str>) -> Value {
    let (status, text) = send(app, method, path, auth, body).await;
    assert_eq!(status, StatusCode::OK, "{method} {path}: {text}");
    serde_json::from_str(&text).unwrap()
}

/// `spot_session=<hex>` of a new account with the default settings.
async fn sign_up(app: &Router) -> String {
    let request = Request::post("/api/session/login-or-signup")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"username":"terms","password":"hunter2hunter2"}"#,
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers().get("set-cookie").unwrap();
    cookie
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn api_key(app: &Router, cookie: &str) -> String {
    let body = ok(app, "POST", "/api/keys/regenerate", cookie, None).await;
    body["apiKey"].as_str().unwrap().to_owned()
}

/// Stores one FI quarter of 50 EUR/MWh per 15 minutes in `[from, to)`.
async fn store_fi(pool: &PgPool, from: &str, to: &str) {
    sqlx::query(
        "INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area)
         SELECT t, t + interval '15 minutes', 50, 'FI'
         FROM generate_series($1::timestamptz, $2::timestamptz - interval '15 minutes',
                              interval '15 minutes') AS t",
    )
    .bind(from)
    .bind(to)
    .execute(pool)
    .await
    .unwrap();
}

/// `(validFrom, marginCentsKwh)` of each version, in response order.
async fn versions(app: &Router, cookie: &str) -> Vec<(String, f64)> {
    let body = ok(app, "GET", "/api/v1/me/settings/versions", cookie, None).await;
    body["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|version| {
            (
                version["validFrom"].as_str().unwrap().to_owned(),
                version["marginCentsKwh"].as_f64().unwrap(),
            )
        })
        .collect()
}

fn pairs(list: &[(&str, f64)]) -> Vec<(String, f64)> {
    list.iter()
        .map(|(date, margin)| ((*date).to_owned(), *margin))
        .collect()
}

/// The default settings of a new user with the given margin and date.
fn settings_body(user_id: &Value, margin: f64, valid_from: &str) -> Value {
    json!({
        "userId": user_id,
        "marginCentsKwh": margin,
        "transferDayCentsKwh": 2.92,
        "transferNightCentsKwh": 1.37,
        "taxCentsKwh": 2.82752,
        "vatPercent": 25.5,
        "nightStartHour": 22,
        "nightEndHour": 7,
        "timezone": "Europe/Helsinki",
        "area": "FI",
        "validFrom": valid_from,
    })
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_save_for_a_new_date_adds_a_version_and_the_same_date_replaces_it(pool: PgPool) {
    let app = app_at(pool, EVE);
    let cookie = sign_up(&app).await;
    let before = ok(&app, "GET", "/api/v1/me/settings", &cookie, None).await;
    let user_id = before["userId"].clone();
    assert_eq!(before, settings_body(&user_id, 0.49, "1970-01-01"));
    assert_eq!(
        versions(&app, &cookie).await,
        pairs(&[("1970-01-01", 0.49)])
    );

    let saved = ok(
        &app,
        "PUT",
        "/api/v1/me/settings",
        &cookie,
        Some(r#"{"validFrom":"2026-12-01","marginCentsKwh":0.6}"#),
    )
    .await;
    // The response has the terms of the requested date; today keeps the old terms.
    assert_eq!(saved, settings_body(&user_id, 0.6, CHANGE));
    assert_eq!(
        ok(&app, "GET", "/api/v1/me/settings", &cookie, None).await,
        before
    );
    assert_eq!(
        versions(&app, &cookie).await,
        pairs(&[("1970-01-01", 0.49), (CHANGE, 0.6)])
    );

    let replaced = ok(
        &app,
        "PUT",
        "/api/v1/me/settings",
        &cookie,
        Some(r#"{"validFrom":"2026-12-01","marginCentsKwh":0.7}"#),
    )
    .await;
    assert_eq!(replaced, settings_body(&user_id, 0.7, CHANGE));
    assert_eq!(
        versions(&app, &cookie).await,
        pairs(&[("1970-01-01", 0.49), (CHANGE, 0.7)])
    );
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_save_of_the_terms_that_already_apply_creates_no_version(pool: PgPool) {
    let app = app_at(pool, EVE);
    let cookie = sign_up(&app).await;
    let put = |body: &'static str| {
        let (app, cookie) = (app.clone(), cookie.clone());
        async move { ok(&app, "PUT", "/api/v1/me/settings", &cookie, Some(body)).await }
    };
    put(r#"{"validFrom":"2026-12-01","marginCentsKwh":0.6}"#).await;
    let two = pairs(&[("1970-01-01", 0.49), (CHANGE, 0.6)]);

    // Equal terms for a later date, with every term given or with none.
    let full = put(
        r#"{"validFrom":"2027-01-01","marginCentsKwh":0.6,"transferDayCentsKwh":2.92,"transferNightCentsKwh":1.37,"taxCentsKwh":2.82752,"vatPercent":25.5,"nightStartHour":22,"nightEndHour":7}"#,
    )
    .await;
    assert_eq!(full["validFrom"], CHANGE);
    assert_eq!(full["marginCentsKwh"], 0.6);
    assert_eq!(
        put(r#"{"validFrom":"2027-06-01"}"#).await["validFrom"],
        CHANGE
    );
    // Today, between the versions, and without a body.
    assert_eq!(
        put(r#"{"marginCentsKwh":0.49}"#).await["validFrom"],
        "1970-01-01"
    );
    assert_eq!(put("{}").await["validFrom"], "1970-01-01");
    assert_eq!(versions(&app, &cookie).await, two);

    // A changed term for the same date does create a version.
    assert_eq!(
        put(r#"{"validFrom":"2027-01-01","vatPercent":24}"#).await["validFrom"],
        "2027-01-01"
    );
    assert_eq!(versions(&app, &cookie).await.len(), 3);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_default_date_is_today_in_the_time_zone_after_the_update(pool: PgPool) {
    // 00:30 on 2026-11-18 in Helsinki, 23:30 on 2026-11-17 in Stockholm.
    let app = app_at(pool, "2026-11-17T22:30:00Z");
    let cookie = sign_up(&app).await;
    let helsinki = ok(
        &app,
        "PUT",
        "/api/v1/me/settings",
        &cookie,
        Some(r#"{"marginCentsKwh":0.6}"#),
    )
    .await;
    assert_eq!(helsinki["validFrom"], "2026-11-18");
    let stockholm = ok(
        &app,
        "PUT",
        "/api/v1/me/settings",
        &cookie,
        Some(r#"{"marginCentsKwh":0.7,"timezone":"Europe/Stockholm"}"#),
    )
    .await;
    assert_eq!(stockholm["validFrom"], "2026-11-17");
    assert_eq!(stockholm["timezone"], "Europe/Stockholm");
    assert_eq!(
        versions(&app, &cookie).await,
        pairs(&[
            ("1970-01-01", 0.49),
            ("2026-11-17", 0.7),
            ("2026-11-18", 0.6)
        ])
    );
    // Today in Stockholm is 2026-11-17.
    let today = ok(&app, "GET", "/api/v1/me/settings", &cookie, None).await;
    assert_eq!(
        (
            today["validFrom"].as_str(),
            today["marginCentsKwh"].as_f64()
        ),
        (Some("2026-11-17"), Some(0.7))
    );
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn an_omitted_term_keeps_the_value_that_applies_on_the_date(pool: PgPool) {
    let app = app_at(pool, EVE);
    let cookie = sign_up(&app).await;
    ok(
        &app,
        "PUT",
        "/api/v1/me/settings",
        &cookie,
        Some(r#"{"validFrom":"2026-12-01","marginCentsKwh":0.6,"vatPercent":24}"#),
    )
    .await;
    // The terms of 2026-12-15 start from the version of 2026-12-01.
    let later = ok(
        &app,
        "PUT",
        "/api/v1/me/settings",
        &cookie,
        Some(r#"{"validFrom":"2026-12-15","taxCentsKwh":3}"#),
    )
    .await;
    assert_eq!(
        (
            later["marginCentsKwh"].as_f64(),
            later["vatPercent"].as_f64(),
            later["taxCentsKwh"].as_f64(),
            later["validFrom"].as_str()
        ),
        (Some(0.6), Some(24.0), Some(3.0), Some("2026-12-15"))
    );
    // The terms of 2026-06-01 start from the first version.
    let earlier = ok(
        &app,
        "PUT",
        "/api/v1/me/settings",
        &cookie,
        Some(r#"{"validFrom":"2026-06-01","nightEndHour":6}"#),
    )
    .await;
    assert_eq!(
        (
            earlier["marginCentsKwh"].as_f64(),
            earlier["vatPercent"].as_f64(),
            earlier["nightEndHour"].as_i64()
        ),
        (Some(0.49), Some(25.5), Some(6))
    );
    // The versions after the new one keep their own terms.
    let all = ok(&app, "GET", "/api/v1/me/settings/versions", &cookie, None).await;
    assert_eq!(
        all["versions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|version| version["nightEndHour"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [7, 6, 7, 7]
    );
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_delete_removes_one_version_but_never_the_only_one(pool: PgPool) {
    let app = app_at(pool, EVE);
    let cookie = sign_up(&app).await;
    let versions_path = "/api/v1/me/settings/versions";
    ok(
        &app,
        "PUT",
        "/api/v1/me/settings",
        &cookie,
        Some(r#"{"validFrom":"2026-12-01","marginCentsKwh":0.6}"#),
    )
    .await;

    let (status, body) = send(
        &app,
        "DELETE",
        &format!("{versions_path}/2026-12-02"),
        &cookie,
        None,
    )
    .await;
    assert_eq!(
        (status, body.as_str()),
        (
            StatusCode::NOT_FOUND,
            r#"{"error":"Contract terms version not found"}"#
        )
    );
    for (date, text) in [
        ("1.12.2026", "validFrom: Invalid ISO date"),
        ("2026-02-30", "validFrom: Invalid ISO date"),
        ("%FF", "validFrom: Invalid ISO date"),
        (
            "1969-12-31",
            "validFrom: Too small: expected date to be >=1970-01-01",
        ),
    ] {
        let (status, body) = send(
            &app,
            "DELETE",
            &format!("{versions_path}/{date}"),
            &cookie,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{date}");
        assert_eq!(body, json!({ "error": text }).to_string());
    }

    // The first version can go: the later version then applies to every date.
    let remaining = ok(
        &app,
        "DELETE",
        &format!("{versions_path}/1970-01-01"),
        &cookie,
        None,
    )
    .await;
    assert_eq!(remaining["versions"].as_array().unwrap().len(), 1);
    assert_eq!(remaining["versions"][0]["validFrom"], CHANGE);
    let today = ok(&app, "GET", "/api/v1/me/settings", &cookie, None).await;
    assert_eq!(
        (
            today["validFrom"].as_str(),
            today["marginCentsKwh"].as_f64()
        ),
        (Some(CHANGE), Some(0.6))
    );

    let (status, body) = send(
        &app,
        "DELETE",
        &format!("{versions_path}/{CHANGE}"),
        &cookie,
        None,
    )
    .await;
    assert_eq!(
        (status, body.as_str()),
        (
            StatusCode::BAD_REQUEST,
            r#"{"error":"Cannot delete the only contract terms version"}"#
        )
    );
    assert_eq!(versions(&app, &cookie).await, pairs(&[(CHANGE, 0.6)]));

    // Without a session the routes answer 401.
    for (method, path) in [
        ("GET", versions_path.to_owned()),
        ("DELETE", format!("{versions_path}/{CHANGE}")),
    ] {
        let (status, body) = send(&app, method, &path, "", None).await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::UNAUTHORIZED, r#"{"error":"Unauthorized"}"#)
        );
    }
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_delete_from_another_origin_is_forbidden(pool: PgPool) {
    let app = app_at(pool, EVE);
    let cookie = sign_up(&app).await;
    ok(
        &app,
        "PUT",
        "/api/v1/me/settings",
        &cookie,
        Some(r#"{"validFrom":"2026-12-01","marginCentsKwh":0.6}"#),
    )
    .await;
    let delete = |origin: &'static str| {
        Request::delete(format!("/api/v1/me/settings/versions/{CHANGE}"))
            .header("cookie", &cookie)
            .header("origin", origin)
            .body(Body::empty())
            .unwrap()
    };
    let cross = app
        .clone()
        .oneshot(delete("https://evil.example"))
        .await
        .unwrap();
    assert_eq!(cross.status(), StatusCode::FORBIDDEN);
    assert_eq!(versions(&app, &cookie).await.len(), 2);
    let same = app.clone().oneshot(delete(ORIGIN)).await.unwrap();
    assert_eq!(same.status(), StatusCode::OK);
    assert_eq!(versions(&app, &cookie).await.len(), 1);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_user_holds_at_most_100_versions(pool: PgPool) {
    let app = app_at(pool.clone(), EVE);
    let cookie = sign_up(&app).await;
    // 99 more versions: 2027-01-01 to 2027-04-09, each with its own margin.
    sqlx::query(
        "INSERT INTO contract_terms
           (user_id, valid_from, margin_cents_kwh, transfer_day_cents_kwh,
            transfer_night_cents_kwh, tax_cents_kwh, vat_percent, night_start_hour, night_end_hour)
         SELECT user_id, DATE '2027-01-01' + n, n, 2.92, 1.37, 2.82752, 25.5, 22, 7
         FROM user_settings, generate_series(0, 98) AS n",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(versions(&app, &cookie).await.len(), 100);

    let (status, body) = send(
        &app,
        "PUT",
        "/api/v1/me/settings",
        &cookie,
        Some(r#"{"validFrom":"2028-01-01","marginCentsKwh":0.6,"area":"SE3"}"#),
    )
    .await;
    assert_eq!(
        (status, body.as_str()),
        (
            StatusCode::BAD_REQUEST,
            r#"{"error":"Too many contract terms versions: the maximum is 100"}"#
        )
    );
    // The refused save wrote nothing, also not the area of the same request.
    let settings = ok(&app, "GET", "/api/v1/me/settings", &cookie, None).await;
    assert_eq!(settings["area"], "FI");
    assert_eq!(versions(&app, &cookie).await.len(), 100);

    // A replace and a save without a new version still work at the limit.
    let replaced = ok(
        &app,
        "PUT",
        "/api/v1/me/settings",
        &cookie,
        Some(r#"{"validFrom":"2027-01-01","marginCentsKwh":0.6}"#),
    )
    .await;
    assert_eq!(replaced["marginCentsKwh"], 0.6);
    let kept = ok(
        &app,
        "PUT",
        "/api/v1/me/settings",
        &cookie,
        Some(r#"{"validFrom":"2028-01-01","area":"SE3"}"#),
    )
    .await;
    assert_eq!(
        (kept["area"].as_str(), kept["validFrom"].as_str()),
        (Some("SE3"), Some("2027-04-09"))
    );
    assert_eq!(versions(&app, &cookie).await.len(), 100);

    // After a delete, a new version fits again.
    ok(
        &app,
        "DELETE",
        "/api/v1/me/settings/versions/2027-04-09",
        &cookie,
        None,
    )
    .await;
    ok(
        &app,
        "PUT",
        "/api/v1/me/settings",
        &cookie,
        Some(r#"{"validFrom":"2028-01-01","marginCentsKwh":0.6}"#),
    )
    .await;
    assert_eq!(versions(&app, &cookie).await.len(), 100);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn an_invalid_valid_from_answers_400_and_saves_nothing(pool: PgPool) {
    let app = app_at(pool, EVE);
    let cookie = sign_up(&app).await;
    for (body, text) in [
        (
            r#"{"validFrom":"1.12.2026","marginCentsKwh":0.6}"#,
            "validFrom: Invalid ISO date",
        ),
        (
            r#"{"validFrom":"2026-13-01","marginCentsKwh":0.6}"#,
            "validFrom: Invalid ISO date",
        ),
        (
            r#"{"validFrom":"","marginCentsKwh":0.6}"#,
            "validFrom: Invalid ISO date",
        ),
        (
            r#"{"validFrom":"1969-12-31","marginCentsKwh":0.6}"#,
            "validFrom: Too small: expected date to be >=1970-01-01",
        ),
        (
            r#"{"validFrom":20261201,"marginCentsKwh":0.6}"#,
            "validFrom: Invalid input: expected string, received number",
        ),
        (
            r#"{"validFrom":null,"marginCentsKwh":-1}"#,
            "marginCentsKwh: Too small: expected number to be >=0; validFrom: Invalid input: expected string, received null",
        ),
    ] {
        let (status, answer) = send(&app, "PUT", "/api/v1/me/settings", &cookie, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(answer, json!({ "error": text }).to_string(), "{body}");
    }
    assert_eq!(
        versions(&app, &cookie).await,
        pairs(&[("1970-01-01", 0.49)])
    );
    // The earliest date is valid and replaces the first version.
    let first = ok(
        &app,
        "PUT",
        "/api/v1/me/settings",
        &cookie,
        Some(r#"{"validFrom":"1970-01-01","marginCentsKwh":0.6}"#),
    )
    .await;
    assert_eq!(first["validFrom"], "1970-01-01");
    assert_eq!(versions(&app, &cookie).await, pairs(&[("1970-01-01", 0.6)]));
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn a_user_without_a_version_gets_the_generic_500(pool: PgPool) {
    let app = app_at(pool.clone(), EVE);
    let cookie = sign_up(&app).await;
    let key = api_key(&app, &cookie).await;
    sqlx::query("DELETE FROM contract_terms")
        .execute(&pool)
        .await
        .unwrap();
    for (method, path, auth, body) in [
        ("GET", "/api/v1/price/now", key.as_str(), None),
        (
            "GET",
            "/api/v1/price/history?from=2026-11-30&to=2026-12-01",
            key.as_str(),
            None,
        ),
        ("GET", "/api/v1/price/forecast", key.as_str(), None),
        ("GET", "/api/v1/me/settings", cookie.as_str(), None),
        ("GET", "/api/v1/me/settings/versions", cookie.as_str(), None),
        ("GET", "/api/v1/me/chart", cookie.as_str(), None),
        (
            "PUT",
            "/api/v1/me/settings",
            cookie.as_str(),
            Some(r#"{"marginCentsKwh":0.6}"#),
        ),
    ] {
        let (status, text) = send(&app, method, path, auth, body).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{method} {path}");
        assert_eq!(text, "Internal Server Error", "{method} {path}");
    }
    // The failed save created no version.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM contract_terms")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

/// `(deliveryStart, marginCentsKwh, totalCentsKwh)` of each price.
fn margins(prices: &Value) -> Vec<(String, f64, f64)> {
    prices
        .as_array()
        .unwrap()
        .iter()
        .map(|price| {
            (
                price["deliveryStart"].as_str().unwrap().to_owned(),
                price["marginCentsKwh"].as_f64().unwrap(),
                price["totalCentsKwh"].as_f64().unwrap(),
            )
        })
        .collect()
}

/// An account with the default terms and a margin of `margin` from
/// [`CHANGE`], an API key, and FI prices of 50 EUR/MWh for the Helsinki days
/// 2026-11-30 and 2026-12-01.
async fn across_the_change(pool: &PgPool, margin: f64) -> (String, String) {
    store_fi(pool, "2026-11-29T22:00:00Z", "2026-12-01T22:00:00Z").await;
    let app = app_at(pool.clone(), EVE);
    let cookie = sign_up(&app).await;
    let key = api_key(&app, &cookie).await;
    let body = json!({ "validFrom": CHANGE, "marginCentsKwh": margin }).to_string();
    ok(&app, "PUT", "/api/v1/me/settings", &cookie, Some(&body)).await;
    (cookie, key)
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn history_uses_the_terms_that_applied_at_each_interval(pool: PgPool) {
    let (_, key) = across_the_change(&pool, 0.6).await;
    // Long after the change, the old day still has the old margin.
    let app = app_at(pool, "2027-03-01T12:00:00Z");
    let history = ok(
        &app,
        "GET",
        "/api/v1/price/history?from=2026-11-30&to=2026-12-01",
        &key,
        None,
    )
    .await;
    let prices = margins(&history["prices"]);
    assert_eq!(prices.len(), 192);
    // Spot 5 c/kWh: night transfer 1.37, day transfer 2.92, tax 2.82752, VAT 25.5 %.
    let expected = |start: &str| -> (f64, f64) {
        let new = start >= "2026-11-30T22:00:00.000Z";
        let hour: u32 = start[11..13].parse().unwrap();
        // Local 22:00 to 07:00 is 20:00Z to 05:00Z.
        let night = !(5..20).contains(&hour);
        match (new, night) {
            (false, true) => (0.49, 12.158),
            (false, false) => (0.49, 14.103),
            (true, true) => (0.6, 12.296),
            (true, false) => (0.6, 14.241),
        }
    };
    for (start, margin, total) in &prices {
        assert_eq!((*margin, *total), expected(start), "{start}");
    }
    // The last quarter of the old day and the first quarter of the new day.
    assert_eq!(
        prices[95],
        ("2026-11-30T21:45:00.000Z".to_owned(), 0.49, 12.158)
    );
    assert_eq!(
        prices[96],
        ("2026-11-30T22:00:00.000Z".to_owned(), 0.6, 12.296)
    );
    assert_eq!(history["minTotalCentsKwh"], 12.158);
    assert_eq!(history["maxTotalCentsKwh"], 14.241);
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_lists_of_the_eve_price_the_next_day_with_the_new_terms(pool: PgPool) {
    let (cookie, key) = across_the_change(&pool, 0.6).await;
    let app = app_at(pool.clone(), EVE);
    let all_margins = |body: &Value| -> Vec<f64> {
        let mut found: Vec<f64> = margins(body).iter().map(|(_, margin, _)| *margin).collect();
        found.dedup();
        found
    };
    let today = ok(&app, "GET", "/api/v1/price/today", &key, None).await;
    assert_eq!(all_margins(&today["prices"]), [0.49]);
    let tomorrow = ok(&app, "GET", "/api/v1/price/tomorrow", &key, None).await;
    assert_eq!(tomorrow["available"], true);
    assert_eq!(all_margins(&tomorrow["prices"]), [0.6]);
    assert_eq!(margins(&tomorrow["prices"]).len(), 96);
    let all = ok(&app, "GET", "/api/v1/price/all", &key, None).await;
    assert_eq!(all_margins(&all["prices"]), [0.49, 0.6]);
    let chart = ok(&app, "GET", "/api/v1/me/chart", &cookie, None).await;
    assert_eq!(all_margins(&chart["today"]), [0.49]);
    assert_eq!(all_margins(&chart["tomorrow"]), [0.6]);

    let now = ok(&app, "GET", "/api/v1/price/now", &key, None).await;
    assert_eq!(
        (
            now["marginCentsKwh"].as_f64(),
            now["totalCentsKwh"].as_f64()
        ),
        (Some(0.49), Some(14.103))
    );
    // The first quarter of the new local day, and the quarter before it.
    for (clock, margin, total) in [
        ("2026-11-30T21:59:59Z", 0.49, 12.158),
        ("2026-11-30T22:00:00Z", 0.6, 12.296),
    ] {
        let app = app_at(pool.clone(), clock);
        let now = ok(&app, "GET", "/api/v1/price/now", &key, None).await;
        assert_eq!(
            (
                now["marginCentsKwh"].as_f64(),
                now["totalCentsKwh"].as_f64()
            ),
            (Some(margin), Some(total)),
            "{clock}"
        );
    }
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_cheapest_window_compares_the_terms_of_both_days(pool: PgPool) {
    // From the change, the margin is 0: the night of the new day is cheaper.
    let (_, key) = across_the_change(&pool, 0.0).await;
    let app = app_at(pool, EVE);
    let window = ok(
        &app,
        "GET",
        "/api/v1/price/cheapest?duration=60",
        &key,
        None,
    )
    .await;
    // With one set of terms for both days, the first night quarter, 20:00Z,
    // wins the tie.
    assert_eq!(window["start"], "2026-11-30T22:00:00.000Z");
    assert_eq!(window["averageTotalCentsKwh"], 11.543);
    assert_eq!(
        margins(&window["prices"])
            .iter()
            .map(|(_, margin, _)| *margin)
            .collect::<Vec<_>>(),
        [0.0; 4]
    );

    // A window that ends before the change uses the old terms.
    let old = ok(
        &app,
        "GET",
        "/api/v1/price/cheapest?duration=60&endTime=2026-11-30T22:00:00Z",
        &key,
        None,
    )
    .await;
    assert_eq!(old["start"], "2026-11-30T20:00:00.000Z");
    assert_eq!(old["averageTotalCentsKwh"], 12.158);

    // maxPrice filters on the total of the version of each interval: only
    // the night of the new day (11.543) is at or below 12.
    let limited = ok(
        &app,
        "GET",
        "/api/v1/price/cheapest?duration=120&maxPrice=12",
        &key,
        None,
    )
    .await;
    assert_eq!(limited["start"], "2026-11-30T22:00:00.000Z");
    assert_eq!(limited["maxTotalCentsKwh"], 11.543);
    let (status, body) = send(
        &app,
        "GET",
        "/api/v1/price/cheapest?duration=60&maxPrice=12&endTime=2026-11-30T22:00:00Z",
        &key,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body,
        r#"{"error":"Not enough contiguous price data at or below 12 c/kWh to form a 60-minute window"}"#
    );
}

/// The API key of alice in the golden seed, a Helsinki FI user.
const ALICE_KEY: &str = "sp_a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_forecast_uses_the_terms_of_the_local_date_of_each_quarter(pool: PgPool) {
    // The seed of the HTTP golden cases gives a forecast for the Helsinki
    // days 2026-10-26 to 2026-10-28 at this clock.
    let seed = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/http/seed.sql"
    ))
    .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(seed))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(COPY_SETTINGS_TO_TERMS)
        .execute(&pool)
        .await
        .unwrap();
    // New terms from 2026-10-27: every term differs from the first version.
    sqlx::raw_sql(
        "INSERT INTO contract_terms
           (user_id, valid_from, margin_cents_kwh, transfer_day_cents_kwh,
            transfer_night_cents_kwh, tax_cents_kwh, vat_percent, night_start_hour, night_end_hour)
         VALUES ('goldenUserAlice00000000000000001', '2026-10-27', 1.5, 4, 2, 3, 24, 23, 6)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let app = app_at(pool, "2026-10-24T09:07:30Z");
    let forecast = ok(&app, "GET", "/api/v1/price/forecast", ALICE_KEY, None).await;
    let entries = forecast["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 288);

    let terms = |margin, day, night, tax, vat, start: u8, end: u8| UserSettings {
        user_id: UserId::new("alice".to_owned()),
        margin_cents_kwh: margin,
        transfer_day_cents_kwh: day,
        transfer_night_cents_kwh: night,
        tax_cents_kwh: tax,
        vat_percent: vat,
        night_start_hour: Hour::new(start).unwrap(),
        night_end_hour: Hour::new(end).unwrap(),
        timezone: UserTimeZone::parse("Europe/Helsinki").unwrap(),
        area: Area::Fi,
    };
    let old = terms(0.49, 2.92, 1.37, 2.82752, 25.5, 22, 7);
    let new = terms(1.5, 4.0, 2.0, 3.0, 24.0, 23, 6);
    let mut per_date = std::collections::BTreeMap::new();
    for entry in entries {
        let local = entry["localStart"].as_str().unwrap();
        let (date, hour) = (&local[..10], local[11..13].parse::<u8>().unwrap());
        let settings = if date < "2026-10-27" { &old } else { &new };
        let spot = entry["estimatedSpotCentsKwh"].as_f64().unwrap();
        assert_eq!(
            entry["estimatedTotalCentsKwh"].as_f64().unwrap(),
            apply_contract_terms(spot, settings, hour).total_cents_kwh,
            "{local}"
        );
        *per_date.entry(date.to_owned()).or_insert(0) += 1;
    }
    // The change is at the local midnight between the first two days.
    assert_eq!(
        per_date.into_iter().collect::<Vec<_>>(),
        [
            ("2026-10-26".to_owned(), 96),
            ("2026-10-27".to_owned(), 96),
            ("2026-10-28".to_owned(), 96)
        ]
    );
    assert_eq!(entries[95]["localStart"], "2026-10-26T23:45:00+02:00");
    assert_eq!(entries[96]["start"], "2026-10-26T22:00:00.000Z");
}

#[sqlx::test(migrator = "spot_price_server::db::MIGRATOR")]
async fn the_forecast_band_totals_use_the_terms_of_each_quarter(pool: PgPool) {
    use spot_price_domain::forecast::{RouteForecast, SpotBand};
    use spot_price_server::http::{forecast, price};
    use spot_price_server::store::user_settings::get_user_settings;

    const ALICE: &str = "goldenUserAlice00000000000000001";
    let seed = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/http/seed.sql"
    ))
    .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(seed))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(COPY_SETTINGS_TO_TERMS)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(
        "INSERT INTO contract_terms
           (user_id, valid_from, margin_cents_kwh, transfer_day_cents_kwh,
            transfer_night_cents_kwh, tax_cents_kwh, vat_percent, night_start_hour, night_end_hour)
         VALUES ('goldenUserAlice00000000000000001', '2026-10-27', 1.5, 4, 2, 3, 24, 23, 6)",
    )
    .execute(&pool)
    .await
    .unwrap();

    // The shipped band artifact is not calibrated, so the route has no band
    // fields. Give every quarter of a real result a band of spot -1 and +2.
    let mut snapshot = spot_price_server::forecast_cache::build(&pool, ts("2026-10-24T09:07:30Z"))
        .await
        .unwrap();
    let RouteForecast::Ready { result, .. } = &mut snapshot.route else {
        panic!("the seed gives a forecast");
    };
    for point in &mut result.series {
        point.band = Some(SpotBand {
            low: point.estimated_spot_cents_kwh - 1.0,
            high: point.estimated_spot_cents_kwh + 2.0,
        });
    }
    let row = get_user_settings(&pool, ALICE).await.unwrap().unwrap();
    let schedule = price::schedule_of(&row).unwrap();
    let body = forecast::respond(&snapshot, &schedule, Area::Fi).unwrap();
    assert_eq!(body.entries.len(), 288);

    let helsinki = UserTimeZone::parse("Europe/Helsinki").unwrap();
    let terms = |margin, day, night, tax, vat, start: u8, end: u8| UserSettings {
        user_id: UserId::new(ALICE.to_owned()),
        margin_cents_kwh: margin,
        transfer_day_cents_kwh: day,
        transfer_night_cents_kwh: night,
        tax_cents_kwh: tax,
        vat_percent: vat,
        night_start_hour: Hour::new(start).unwrap(),
        night_end_hour: Hour::new(end).unwrap(),
        timezone: helsinki.clone(),
        area: Area::Fi,
    };
    let old = terms(0.49, 2.92, 1.37, 2.82752, 25.5, 22, 7);
    let new = terms(1.5, 4.0, 2.0, 3.0, 24.0, 23, 6);
    let mut new_quarters = 0;
    for entry in &body.entries {
        let (date, hour) = (
            &entry.local_start[..10],
            entry.local_start[11..13].parse::<u8>().unwrap(),
        );
        let settings = if date < "2026-10-27" {
            &old
        } else {
            new_quarters += 1;
            &new
        };
        let total = |spot: f64| apply_contract_terms(spot, settings, hour).total_cents_kwh;
        let spot = entry.estimated_spot_cents_kwh;
        assert_eq!(
            (
                entry.estimated_total_low_cents_kwh,
                entry.estimated_total_cents_kwh,
                entry.estimated_total_high_cents_kwh
            ),
            (
                Some(total(spot - 1.0)),
                total(spot),
                Some(total(spot + 2.0))
            ),
            "{}",
            entry.local_start
        );
    }
    assert_eq!(new_quarters, 192);
}
