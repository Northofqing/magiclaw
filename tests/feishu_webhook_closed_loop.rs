//! Closed-loop test for Feishu webhook ingress.
//!
//! Verifies `POST /api/feishu/webhook` end-to-end:
//!   - signature + verification token validation,
//!   - inbound persistence (inbox),
//!   - route enqueue and pipeline execution,
//!   - outbox generation for downstream recoverable delivery.

use std::net::TcpListener;
use std::time::Duration;

use magiclaw::domain::ports::inbox_repo::InboxRepo;
use magiclaw::domain::ports::outbox_repo::OutboxRepo;
use magiclaw::infrastructure::config::AppConfig;
use magiclaw::infrastructure::runtime::AppRuntime;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn sign(ts: &str, nonce: &str, secret: &str, body: &str) -> String {
    let payload = format!("{}{}{}{}", ts, nonce, secret, body);
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(payload.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

#[tokio::test]
async fn feishu_webhook_ingress_persists_and_routes_to_outbox() {
    let port = free_port();
    let db_path = std::env::temp_dir().join(format!("magiclaw_feishu_{}.db", uuid::Uuid::new_v4()));

    let mut config = AppConfig::default();
    config.db_path = db_path.to_string_lossy().to_string();
    config.feishu.verification_token = "verify-token".into();
    config.feishu.signing_secret = "signing-secret".into();

    let runtime = AppRuntime::new(config).unwrap();
    runtime
        .start_http_api(&format!("127.0.0.1:{}", port))
        .unwrap();

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", port);

    let mut ready = false;
    for _ in 0..50 {
        if let Ok(resp) = client.get(format!("{}/api/health", base)).send().await {
            if resp.status() == 200 {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ready, "HTTP API did not become ready");

    let body = serde_json::json!({
        "header": {
            "event_id": "evt_test_1",
            "event_type": "im.message.receive_v1",
            "create_time": "1718000000000",
            "token": "verify-token"
        },
        "event": {
            "sender": {
                "sender_id": {
                    "open_id": "ou_test_user"
                }
            },
            "message": {
                "message_id": "om_test_1",
                "chat_id": "oc_test_group",
                "chat_type": "group",
                "message_type": "text",
                "content": "{\"text\":\"hello webhook\"}",
                "create_time": "1718000000001"
            }
        }
    })
    .to_string();

    let ts = "1718000000";
    let nonce = "nonce-closed-loop";
    let signature = sign(ts, nonce, "signing-secret", &body);

    let resp = client
        .post(format!("{}/api/feishu/webhook", base))
        .header("x-lark-request-timestamp", ts)
        .header("x-lark-request-nonce", nonce)
        .header("x-lark-signature", signature)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Inbox persistence should be immediate.
    assert!(runtime.inbox_repo.exists("om_test_1").unwrap());

    // The route worker executes asynchronously; poll briefly for outbox production.
    let mut outbox_seen = false;
    for _ in 0..100 {
        let pending = runtime.outbox_repo.fetch_pending(20).unwrap();
        if pending.iter().any(|entry| {
            entry.id == "om_test_1" || (entry.route_key.contains("feishu") && entry.route_key.contains("oc_test_group"))
        }) {
            outbox_seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(outbox_seen, "feishu inbound did not produce outbox entry");

    let _ = std::fs::remove_file(db_path);
}
