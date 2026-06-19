//! Minimal bearer-token authentication for the REST/HTTP adapter.
//!
//! Red line 2.4: the REST adapter must enable minimal authentication when
//! exposed — no naked, default-open ports. This module provides a reusable
//! axum middleware that enforces a configured bearer token on all routes
//! except the liveness probe (`/api/health`).
//!
//! Failure / closed-by-default policy:
//!   - If no token is configured (empty), every protected request is rejected
//!     with `401 Unauthorized`. The port is therefore never open by default.
//!   - A missing, malformed, or mismatched `Authorization` header yields `401`.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Liveness probe path that is intentionally left unauthenticated.
pub const HEALTH_PATH: &str = "/api/health";

/// Configured authentication state for the HTTP adapter.
#[derive(Clone)]
pub struct HttpAuth {
    /// The expected bearer token. Empty means "closed by default".
    pub token: String,
}

impl HttpAuth {
    pub fn new(token: impl Into<String>) -> Self {
        Self { token: token.into() }
    }
}

/// Constant-time byte comparison to avoid leaking the token via timing.
///
/// The length is compared first, which only reveals token length — acceptable
/// for this threat model.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Decide whether a request is authorized given the configured token and the
/// raw `Authorization` header value.
///
/// Returns `false` (closed) when the expected token is empty, the header is
/// absent, not a `Bearer` scheme, or does not match.
pub fn is_bearer_authorized(expected: &str, auth_header: Option<&str>) -> bool {
    if expected.is_empty() {
        return false;
    }
    let Some(value) = auth_header else {
        return false;
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return false;
    };
    constant_time_eq(token.trim().as_bytes(), expected.as_bytes())
}

/// Axum middleware enforcing bearer-token auth on every route except the
/// liveness probe (`/api/health`).
pub async fn require_bearer_auth(
    State(auth): State<Arc<HttpAuth>>,
    request: Request,
    next: Next,
) -> Response {
    if request.uri().path() == HEALTH_PATH {
        return next.run(request).await;
    }

    let header_value = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    if is_bearer_authorized(&auth.token, header_value) {
        next.run(request).await
    } else {
        (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_token_is_closed_by_default() {
        assert!(!is_bearer_authorized("", Some("Bearer anything")));
        assert!(!is_bearer_authorized("", None));
    }

    #[test]
    fn missing_or_malformed_header_is_rejected() {
        assert!(!is_bearer_authorized("secret", None));
        assert!(!is_bearer_authorized("secret", Some("secret")));
        assert!(!is_bearer_authorized("secret", Some("Basic secret")));
        assert!(!is_bearer_authorized("secret", Some("Bearer wrong")));
    }

    #[test]
    fn matching_bearer_token_is_authorized() {
        assert!(is_bearer_authorized("secret", Some("Bearer secret")));
        assert!(is_bearer_authorized("secret", Some("Bearer  secret ")));
    }

    #[test]
    fn constant_time_eq_matches_std_eq() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
