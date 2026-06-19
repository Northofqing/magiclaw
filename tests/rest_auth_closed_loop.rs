//! Closed-loop test for the REST/HTTP adapter minimal auth (red line 2.4).
//!
//! Verifies, over a real TCP server, that the bearer-token middleware:
//!   - leaves the `/api/health` liveness probe open,
//!   - rejects protected requests with no / wrong token (401),
//!   - accepts protected requests carrying the correct bearer token (200),
//!   - stays closed by default when no token is configured.

use std::sync::Arc;

use aiclaw::adapters::http_auth::{require_bearer_auth, HttpAuth};
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::net::TcpListener;

async fn spawn(token: &str) -> (String, tokio::task::JoinHandle<()>) {
    let auth = Arc::new(HttpAuth::new(token.to_string()));
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
    (format!("http://{}", addr), handle)
}

#[tokio::test]
async fn health_is_open_protected_requires_token() {
    let (base, handle) = spawn("s3cret").await;
    let client = reqwest::Client::new();

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
        .header("Authorization", "Bearer s3cret")
        .json(&serde_json::json!({"to": "peer", "text": "hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);

    handle.abort();
}

#[tokio::test]
async fn empty_token_keeps_protected_endpoints_closed() {
    let (base, handle) = spawn("").await;
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
