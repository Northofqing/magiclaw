//! Closed-loop test for the REST/HTTP adapter minimal auth (red line 2.4).
//!
//! Verifies, over a real TCP server, that the bearer-token middleware:
//!   - leaves the `/api/health` liveness probe open,
//!   - rejects protected requests with no / wrong token (401),
//!   - rejects scope-mismatched tokens with 403,
//!   - accepts protected requests carrying a valid scoped token (200).

use std::sync::Arc;

use magiclaw::adapters::api_client_registry::ApiClientRegistry;
use magiclaw::adapters::http_auth::{require_bearer_auth, HttpAuth};
use magiclaw::infrastructure::db::{init_db, DbPool};
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::net::TcpListener;

async fn spawn() -> (String, tokio::task::JoinHandle<()>, ApiClientRegistry) {
    let registry = ApiClientRegistry::new(DbPool::new(init_db(":memory:").unwrap()));
    let auth = Arc::new(HttpAuth::new(Arc::new(registry.clone())));
    let app = Router::new()
        .route(
            "/api/send",
            post(|| async { (axum::http::StatusCode::OK, Json(serde_json::json!({"ok": true}))) }),
        )
        .route(
            "/api/health",
            get(|| async { (axum::http::StatusCode::OK, Json(serde_json::json!({"ok": true}))) }),
        )
        .layer(axum::middleware::from_fn_with_state(
            auth,
            require_bearer_auth,
        ));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{}", addr), handle, registry)
}

#[tokio::test]
async fn health_is_open_protected_requires_token() {
    let (base, handle, registry) = spawn().await;
    let client = reqwest::Client::new();
    let issued = registry
        .issue_token(
            "proj-a",
            "rest-test-client",
            &["send".to_string()],
            3600,
            None,
        )
        .unwrap();

    // Liveness probe is reachable without auth.
    let health = client.get(format!("{}/api/health", base)).send().await.unwrap();
    assert_eq!(health.status(), 200);

    // Protected endpoint without token => 401.
    let no_token = client
        .post(format!("{}/api/send", base))
        .json(&serde_json::json!({"to": "peer", "text": "hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(no_token.status(), 401);

    // Wrong token => 401.
    let wrong = client
        .post(format!("{}/api/send", base))
        .header("Authorization", "Bearer nope")
        .json(&serde_json::json!({"to": "peer", "text": "hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);

    // Correct token => 200.
    let ok = client
        .post(format!("{}/api/send", base))
        .header("Authorization", format!("Bearer {}", issued.raw_token))
        .json(&serde_json::json!({"to": "peer", "text": "hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);

    // Scope mismatch should return forbidden.
    let forbidden = client
        .get(format!("{}/api/window_status", base))
        .header("Authorization", format!("Bearer {}", issued.raw_token))
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), 403);

    handle.abort();
}

#[tokio::test]
async fn unknown_token_keeps_protected_endpoints_closed() {
    let (base, handle, _registry) = spawn().await;
    let client = reqwest::Client::new();

    // Health still open.
    let health = client.get(format!("{}/api/health", base)).send().await.unwrap();
    assert_eq!(health.status(), 200);

    // Even with a bearer header, protected endpoint is closed by default.
    let attempt = client
        .post(format!("{}/api/send", base))
        .header("Authorization", "Bearer anything")
        .json(&serde_json::json!({"to": "peer", "text": "hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(attempt.status(), 401);

    handle.abort();
}
