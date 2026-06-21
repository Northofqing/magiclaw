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

use crate::adapters::api_client_registry::{ApiClientRegistry, AuthFailure};

/// Liveness probe path that is intentionally left unauthenticated.
pub const HEALTH_PATH: &str = "/api/health";
/// Feishu webhook path uses provider token/signature verification instead of
/// bearer auth.
pub const FEISHU_WEBHOOK_PATH: &str = "/api/feishu/webhook";

/// Configured authentication state for the HTTP adapter.
#[derive(Clone)]
pub struct HttpAuth {
    pub registry: Arc<ApiClientRegistry>,
}

impl HttpAuth {
    pub fn new(registry: Arc<ApiClientRegistry>) -> Self {
        Self { registry }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathAuthPolicy {
    Public,
    Scoped(&'static str),
    Deny,
}

fn auth_policy_for_path(path: &str) -> PathAuthPolicy {
    match path {
        HEALTH_PATH | FEISHU_WEBHOOK_PATH => PathAuthPolicy::Public,
        "/api/send" => PathAuthPolicy::Scoped("send"),
        "/api/window_status" => PathAuthPolicy::Scoped("window_status"),
        _ => PathAuthPolicy::Deny,
    }
}

/// Axum middleware enforcing bearer-token auth on every route except the
/// liveness probe (`/api/health`).
pub async fn require_bearer_auth(
    State(auth): State<Arc<HttpAuth>>,
    request: Request,
    next: Next,
) -> Response {
    let header_value = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    match auth_policy_for_path(request.uri().path()) {
        PathAuthPolicy::Public => next.run(request).await,
        PathAuthPolicy::Scoped(required_scope) => match auth.registry.authorize_bearer(header_value, required_scope) {
            Ok(_) => next.run(request).await,
            Err(AuthFailure::Forbidden) => (StatusCode::FORBIDDEN, "forbidden").into_response(),
            Err(AuthFailure::Unauthorized) => (StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
        },
        PathAuthPolicy::Deny => (StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::api_client_registry::ApiClientRegistry;
    use crate::infrastructure::db::{init_db, DbPool};
    use std::sync::Arc;

    #[test]
    fn path_auth_policy_maps_known_routes() {
        assert_eq!(auth_policy_for_path(HEALTH_PATH), PathAuthPolicy::Public);
        assert_eq!(auth_policy_for_path(FEISHU_WEBHOOK_PATH), PathAuthPolicy::Public);
        assert_eq!(auth_policy_for_path("/api/send"), PathAuthPolicy::Scoped("send"));
        assert_eq!(auth_policy_for_path("/api/window_status"), PathAuthPolicy::Scoped("window_status"));
        assert_eq!(auth_policy_for_path("/api/unknown"), PathAuthPolicy::Deny);
    }

    #[test]
    fn registry_auth_distinguishes_scope_errors() {
        let conn = init_db(":memory:").unwrap();
        let registry = ApiClientRegistry::new(DbPool::new(conn));
        let issued = registry.issue_token("proj", "client", &["send".into()], 3600, None).unwrap();
        let auth = HttpAuth::new(Arc::new(registry));

        let authorized = auth
            .registry
            .authorize_bearer(Some(&format!("Bearer {}", issued.raw_token)), "send")
            .unwrap();
        assert_eq!(authorized.project_id, "proj");

        assert!(matches!(
            auth.registry.authorize_bearer(Some(&format!("Bearer {}", issued.raw_token)), "window_status"),
            Err(AuthFailure::Forbidden)
        ));
    }

}
