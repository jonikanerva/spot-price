//! The API key check and the per-user rate limit of `/api/v1/price/*`.

use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

use super::{AppState, PassThrough, rate_limit, wire};
use crate::store::api_keys;

/// The user of a valid API key, set by [`authenticate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiUser(pub String);

const MISSING: &str = "Missing or invalid Authorization header. Use: Bearer <api-key>";
const INVALID: &str = "Invalid API key";

/// The token of `Authorization: Bearer <token>`. The scheme is
/// case-insensitive; the header must be the scheme, white space, and one
/// token without white space, like `/^Bearer\s+(\S+)$/i`.
#[must_use]
pub fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    // Fetch `Headers` drop white space around a value, like an HTTP parser.
    let value = headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .trim_matches([' ', '\t']);
    let scheme = value.get(..6)?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let rest = &value[6..];
    let token = rest.trim_start_matches(char::is_whitespace);
    if token.len() == rest.len() || token.is_empty() || token.contains(char::is_whitespace) {
        return None;
    }
    Some(token)
}

/// Middleware: answers 401 without a valid API key; otherwise adds
/// [`ApiUser`] to the request and sets `last_used_at` of the key.
pub async fn authenticate(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(token) = bearer_token(request.headers()) else {
        return wire::error(StatusCode::UNAUTHORIZED, MISSING);
    };
    match api_keys::resolve_api_key(&state.pool, token, state.now()).await {
        Ok(Some(user_id)) => {
            request.extensions_mut().insert(ApiUser(user_id));
            next.run(request).await
        }
        Ok(None) => wire::error(StatusCode::UNAUTHORIZED, INVALID),
        Err(error) => {
            tracing::error!(code = error.code(), "an API key could not be checked");
            wire::internal_error()
        }
    }
}

/// Middleware after [`authenticate`]: 60 requests per minute per user.
pub async fn limit(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(ApiUser(user_id)) = request.extensions().get::<ApiUser>().cloned() else {
        return wire::internal_error();
    };
    let limiter = &state.limits.api_key;
    let decision = limiter.check(&format!("apikey:{user_id}"), state.now().as_millisecond());
    if !decision.allowed {
        return limiter.limited(&decision);
    }
    let mut response = next.run(request).await;
    if response.extensions().get::<PassThrough>().is_none() {
        rate_limit::set_headers(response.headers_mut(), &decision);
    }
    response
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn token(value: &'static str) -> Option<String> {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static(value));
        bearer_token(&headers).map(str::to_owned)
    }

    #[test]
    fn reads_the_bearer_token() {
        assert_eq!(token("Bearer sp_abc"), Some("sp_abc".to_owned()));
        assert_eq!(token("bEaReR \t sp_abc"), Some("sp_abc".to_owned()));
        assert_eq!(token("Bearer"), None);
        assert_eq!(token("Bearer "), None);
        assert_eq!(token("Bearersp_abc"), None);
        assert_eq!(token("Bearer a b"), None);
        assert_eq!(token(" Bearer a "), Some("a".to_owned()));
        assert_eq!(token("Bearer a b "), None);
        assert_eq!(token("Basic abc"), None);
        assert_eq!(bearer_token(&HeaderMap::new()), None);
    }
}
