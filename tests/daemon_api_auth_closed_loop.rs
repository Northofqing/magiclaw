//! System-level closed-loop test for the daemon HTTP API dynamic project-token auth wiring.
//!
//! Exercises the REAL compiled binary end to end:
//!   - project-level token issuance via `magiclaw auth issue`,
//!   - `/api/health` stays open (no auth),
//!   - protected `/api/window_status` is 401 without / with a wrong token,
//!   - the same endpoint is 200 with the correct bearer token,
//!   - scope denial returns 403,
//!   - revocation returns the endpoint to 401.
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
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn run_magiclaw(work_dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_magiclaw"))
        .current_dir(work_dir)
        .args(args)
        .output()
        .expect("failed to run magiclaw command")
}

fn parse_token(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("token="))
        .map(|value| value.trim().to_string())
        .expect("auth issue did not print token=")
}

#[tokio::test]
async fn project_token_auth_issue_scope_and_revoke_work_end_to_end() {
    let port = free_port();
    let addr = format!("127.0.0.1:{}", port);

    let data_dir = std::env::temp_dir().join(format!("magiclaw_test_wd_{}", port));
    std::fs::create_dir_all(&data_dir).unwrap();
    let work_dir = std::env::temp_dir().join(format!("magiclaw_test_cwd_{}", port));
    std::fs::create_dir_all(&work_dir).unwrap();

    let issue_output = run_magiclaw(
        &work_dir,
        &[
            "auth",
            "issue",
            "--project",
            "proj-a",
            "--name",
            "worker-a",
            "--scopes",
            "window_status",
            "--ttl-secs",
            "3600",
        ],
    );
    assert!(issue_output.status.success(), "auth issue failed: {}", String::from_utf8_lossy(&issue_output.stderr));
    let token = parse_token(&String::from_utf8_lossy(&issue_output.stdout));

    let child = Command::new(env!("CARGO_BIN_EXE_magiclaw"))
        .current_dir(&work_dir)
        .env("MAGICLAW_API_ADDR", &addr)
        .env("WECHAT_CHANNEL_DIR", &data_dir)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn magiclaw daemon");
    let _guard = ChildGuard(child);

    let base = format!("http://{}", addr);
    let client = reqwest::Client::new();

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

    let no_token = client
        .get(format!("{}/api/window_status", base))
        .send()
        .await
        .unwrap();
    assert_eq!(no_token.status(), 401, "missing token must be rejected");

    let wrong = client
        .get(format!("{}/api/window_status", base))
        .bearer_auth("not-the-token")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401, "wrong token must be rejected");

    let forbidden = client
        .post(format!("{}/api/send", base))
        .bearer_auth(&token)
        .json(&serde_json::json!({"to":"peer-a","text":"hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), 403, "scope mismatch must be rejected");

    let ok = client
        .get(format!("{}/api/window_status", base))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200, "correct token must be accepted");
    let body: serde_json::Value = ok.json().await.unwrap();
    assert_eq!(body["ok"], true);

    let revoke_output = run_magiclaw(&work_dir, &["auth", "revoke", "--token", &token]);
    assert!(revoke_output.status.success(), "auth revoke failed: {}", String::from_utf8_lossy(&revoke_output.stderr));

    let revoked = client
        .get(format!("{}/api/window_status", base))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 401, "revoked token must be rejected");
}
