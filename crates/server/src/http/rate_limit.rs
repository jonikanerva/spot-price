//! Fixed-window rate limits in process memory, like the `hono-rate-limiter`
//! 0.5 memory store of the TypeScript service.
//!
//! | Limiter | Key | Limit |
//! | --- | --- | --- |
//! | global (every path except `/health`) | client address | 120 per minute |
//! | login (`POST /api/session/login-or-signup`) | client address | 10 per 15 minutes |
//! | API key (`/api/v1/price/*`, after the key check) | user id | 60 per minute |
//!
//! A window starts at the first request of a key and lasts its full length.
//! Each response gets the draft-6 `RateLimit-*` headers of the innermost
//! limiter that ran. A full limiter answers 429 with `Retry-After`.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;

use super::wire;

/// The most keys of one limiter. When it is full, the expired windows go
/// first and then the window that ends first.
pub const MAX_KEYS: usize = 10_000;

/// One fixed-window limiter.
#[derive(Debug)]
pub struct Limiter {
    limit: u32,
    window_ms: i64,
    message: &'static str,
    windows: Mutex<HashMap<String, Window>>,
}

#[derive(Debug, Clone, Copy)]
struct Window {
    hits: u32,
    /// The end of the window, in Unix milliseconds.
    reset_at_ms: i64,
}

/// The decision of a limiter for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub allowed: bool,
    pub limit: u32,
    pub remaining: u32,
    /// Whole seconds until the window ends, rounded up.
    pub reset_seconds: i64,
    pub window_seconds: i64,
}

impl Limiter {
    /// A limiter of `limit` requests in `window_ms` milliseconds. `message` is
    /// the `error` text of its 429 answer.
    #[must_use]
    pub fn new(limit: u32, window_ms: i64, message: &'static str) -> Self {
        Self {
            limit,
            window_ms,
            message,
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// Counts one request of `key` at `now_ms` (Unix milliseconds).
    pub fn check(&self, key: &str, now_ms: i64) -> Decision {
        let mut windows = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        if !windows.contains_key(key) && windows.len() >= MAX_KEYS {
            windows.retain(|_, window| window.reset_at_ms > now_ms);
            if windows.len() >= MAX_KEYS
                && let Some(first) = windows
                    .iter()
                    .min_by_key(|(_, window)| window.reset_at_ms)
                    .map(|(key, _)| key.clone())
            {
                windows.remove(&first);
            }
        }
        let window = windows.entry(key.to_owned()).or_insert(Window {
            hits: 0,
            reset_at_ms: now_ms.saturating_add(self.window_ms),
        });
        if window.reset_at_ms <= now_ms {
            *window = Window {
                hits: 0,
                reset_at_ms: now_ms.saturating_add(self.window_ms),
            };
        }
        window.hits = window.hits.saturating_add(1);
        let left_ms = window.reset_at_ms.saturating_sub(now_ms);
        Decision {
            allowed: window.hits <= self.limit,
            limit: self.limit,
            remaining: self.limit.saturating_sub(window.hits),
            reset_seconds: ceil_seconds(left_ms),
            window_seconds: ceil_seconds(self.window_ms),
        }
    }

    /// The number of tracked keys.
    #[must_use]
    pub fn len(&self) -> usize {
        self.windows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The 429 answer of this limiter.
    #[must_use]
    pub fn limited(&self, decision: &Decision) -> Response {
        let mut response = wire::error(StatusCode::TOO_MANY_REQUESTS, self.message);
        set_headers(response.headers_mut(), decision);
        if let Ok(value) = HeaderValue::from_str(&decision.reset_seconds.to_string()) {
            response
                .headers_mut()
                .insert(HeaderName::from_static("retry-after"), value);
        }
        response
    }
}

fn ceil_seconds(ms: i64) -> i64 {
    ms.div_euclid(1000) + i64::from(ms.rem_euclid(1000) > 0)
}

/// Sets the draft-6 headers of a decision. `RateLimit-Reset` is left out
/// when it is 0.
pub fn set_headers(headers: &mut HeaderMap, decision: &Decision) {
    let mut set = |name: &'static str, value: String| {
        if let Ok(value) = HeaderValue::from_str(&value) {
            headers.insert(HeaderName::from_static(name), value);
        }
    };
    set(
        "ratelimit-policy",
        format!("{};w={}", decision.limit, decision.window_seconds),
    );
    set("ratelimit-limit", decision.limit.to_string());
    set("ratelimit-remaining", decision.remaining.to_string());
    if decision.reset_seconds != 0 {
        set("ratelimit-reset", decision.reset_seconds.to_string());
    } else {
        headers.remove(HeaderName::from_static("ratelimit-reset"));
    }
}

/// The three limiters of the service.
#[derive(Debug)]
pub struct RateLimits {
    pub global: Limiter,
    pub login: Limiter,
    pub api_key: Limiter,
}

impl Default for RateLimits {
    fn default() -> Self {
        Self {
            global: Limiter::new(120, 60_000, "Too many requests. Try again later."),
            login: Limiter::new(
                10,
                15 * 60_000,
                "Too many login attempts. Try again in 15 minutes.",
            ),
            api_key: Limiter::new(60, 60_000, "Rate limit exceeded. Try again later."),
        }
    }
}

/// The client key of the global and login limiters: `X-Real-IP`, else the
/// first `X-Forwarded-For` entry, else `unknown`. An empty header value is a
/// key too, like in the TypeScript service.
#[must_use]
pub fn client_key(headers: &HeaderMap) -> String {
    let joined = |name: &str| {
        let values: Vec<&str> = headers
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .map(|value| value.trim_matches([' ', '\t']))
            .collect();
        (!values.is_empty()).then(|| values.join(", "))
    };
    if let Some(real_ip) = joined("x-real-ip") {
        return real_ip;
    }
    if let Some(forwarded) = joined("x-forwarded-for") {
        let first = forwarded.split(',').next().unwrap_or_default();
        return spot_price_domain::js::js_trim(first).to_owned();
    }
    "unknown".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_in_a_fixed_window() {
        let limiter = Limiter::new(2, 60_000, "slow down");
        let t0 = 1_000_000;
        let first = limiter.check("a", t0);
        assert_eq!(
            first,
            Decision {
                allowed: true,
                limit: 2,
                remaining: 1,
                reset_seconds: 60,
                window_seconds: 60
            }
        );
        assert!(limiter.check("a", t0 + 1).allowed);
        let third = limiter.check("a", t0 + 1_500);
        assert!(!third.allowed);
        assert_eq!(third.remaining, 0);
        assert_eq!(third.reset_seconds, 59);
        assert!(limiter.check("b", t0).allowed);
        let next = limiter.check("a", t0 + 60_000);
        assert!(next.allowed);
        assert_eq!(next.remaining, 1);
        assert_eq!(next.reset_seconds, 60);
    }

    #[test]
    fn keeps_a_bounded_number_of_keys() {
        let limiter = Limiter::new(1, 60_000, "x");
        for n in 0..MAX_KEYS + 5 {
            limiter.check(&n.to_string(), 0);
        }
        assert_eq!(limiter.len(), MAX_KEYS);
        limiter.check("late", 60_001);
        assert_eq!(limiter.len(), 1);
    }

    #[test]
    fn the_429_has_the_headers_and_the_message() {
        let limiter = Limiter::new(1, 900_000, "too many");
        limiter.check("k", 0);
        let decision = limiter.check("k", 1);
        let response = limiter.limited(&decision);
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(header("ratelimit-policy"), "1;w=900");
        assert_eq!(header("ratelimit-limit"), "1");
        assert_eq!(header("ratelimit-remaining"), "0");
        assert_eq!(header("ratelimit-reset"), "900");
        assert_eq!(header("retry-after"), "900");
        assert_eq!(header("content-type"), "application/json");
    }

    #[test]
    fn finds_the_client_key() {
        let mut headers = HeaderMap::new();
        assert_eq!(client_key(&headers), "unknown");
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static(" 1.2.3.4 , 5.6.7.8"),
        );
        assert_eq!(client_key(&headers), "1.2.3.4");
        headers.insert("x-forwarded-for", HeaderValue::from_static(""));
        assert_eq!(client_key(&headers), "");
        headers.insert("x-real-ip", HeaderValue::from_static("9.9.9.9"));
        assert_eq!(client_key(&headers), "9.9.9.9");
    }
}
