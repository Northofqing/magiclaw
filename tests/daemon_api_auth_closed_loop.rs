//! System-level closed-loop test for the daemon HTTP API bearer auth wiring.
//!
//! Covers the regression where `/api/send` (and other protected endpoints)
//! returned 401 for everyone because the runtime never sourced an auth token.
//! Exercises the REAL compiled binary end to end:
//!   - `AICLAW_API_TOKEN` is wired into the runtime auth (red line 2.4),
//!   - `/api/health` stays open (no auth),
//!   - protected `/api/window_status` is 401 without / with a wrong token,
//!   - the same endpoint is 200 with the correct bearer token.
//!
//! The daemon runs against an empty `WECHAT_CHANNEL_DIR` (skeleton channel, no
//! network) so the test is hermetic.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
/// Kills the daemon child even if an assertion panics.
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    // Bind to :0, read the assigned port, then release it for the daemon.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[tokio::test]
async fn api_token_env_gates_protected_endpoints_end_to_end() {
    let token = "stress-token-abc123";
    let port = free_port();
    let addr = format!("127.0.0.1:{}", port);

    // Empty data dir -> wechat skeleton, no outbound network.
    let data_dir = std::env::temp_dir().join(format!("aiclaw_test_wd_{}", port));
    std::fs::create_dir_all(&data_dir).unwrap();
    let work_dir = std::env::temp_dir().join(format!("aiclaw_test_cwd_{}", port));
    std::fs::create_dir_all(&work_dir).unwrap();

    let child = Command::new(env!("CARGO_BIN_EXE_aiclaw"))
        .current_dir(&work_dir)
        .env("AICLAW_API_TOKEN", token)
        .env("AICLAW_API_ADDR", &addr)
        .env("WECHAT_CHANNEL_DIR", &data_dir)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn aiclaw daemon");
    let _guard = ChildGuard(child);

    let base = format!("http://{}", addr);
    let client = reqwest::Client::new();

    // Wait for the HTTP API to bind (health probe is unauthenticated).
    let mut ready = false;
    for _ in 0..100 {
        if let Ok(resp) = client.get(format!("{}/api/health", base)).send().await {
            if resp.status() == 200 {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "daemon HTTP API did not become ready");

    // Protected endpoint with no Authorization header => 401.
    let no_token = client
        .get(format!("{}/api/window_status", base))
        .send()
        .await
        .unwrap();
    assert_eq!(no_token.status(), 401, "missing token must be rejected");

    // Wrong token => 401.
    let wrong = client
        .get(format!("{}/api/window_status", base))
        .bearer_auth("not-the-token")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401, "wrong token must be rejected");

    // Correct token => 200 and a well-formed body.
    let ok = client
        .get(format!("{}/api/window_status", base))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200, "correct token must be accepted");
    let body: serde_json::Value = ok.json().await.unwrap();
    assert_eq!(body["ok"], true);
}
