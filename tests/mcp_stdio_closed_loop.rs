//! System-level closed-loop test for the MCP stdio server (phase 1.5).
//!
//! Covers the regression where `McpServer::run_blocking` built a nested tokio
//! runtime inside `#[tokio::main]` and panicked with "Cannot start a runtime
//! from within a runtime". Exercises the REAL compiled binary over stdio:
//!   - `initialize` returns protocolVersion 2024-11-05,
//!   - `tools/list` returns the send / list_peers / login tools,
//!   - responses are emitted as Content-Length framed JSON on stdout,
//!   - stdout carries protocol output ONLY (no log lines),
//!   - the process exits cleanly on stdin EOF (no panic, success status).
//!
//! Runs against an empty `WECHAT_CHANNEL_DIR` (skeleton channel, no network).

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn mcp_stdio_initialize_and_tools_list_over_real_binary() {
    let data_dir = std::env::temp_dir().join("aiclaw_mcp_test_wd");
    std::fs::create_dir_all(&data_dir).unwrap();
    let work_dir = std::env::temp_dir().join("aiclaw_mcp_test_cwd");
    std::fs::create_dir_all(&work_dir).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_aiclaw"))
        .arg("--mcp")
        .current_dir(&work_dir)
        .env("WECHAT_CHANNEL_DIR", &data_dir)
        .env("RUST_LOG", "warn")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn aiclaw --mcp");

    // Single-line JSON is accepted by the transport. Closing stdin signals EOF,
    // which makes the server shut down gracefully.
    {
        let mut stdin = child.stdin.take().unwrap();
        let requests = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{}}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            "\n",
        );
        stdin.write_all(requests.as_bytes()).unwrap();
        // stdin dropped here -> EOF.
    }

    let output = child.wait_with_output().expect("failed to wait for --mcp");

    // The nested-runtime regression panicked -> non-zero exit. Require success.
    assert!(
        output.status.success(),
        "MCP server exited with failure: status={:?}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout must be UTF-8");

    // stdout zero-pollution: every non-empty line is either a Content-Length
    // header or a JSON body — never a tracing log line.
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        let is_frame_header = line.starts_with("Content-Length:");
        let is_json_body = line.trim_start().starts_with('{');
        assert!(
            is_frame_header || is_json_body,
            "stdout polluted with non-protocol line: {:?}",
            line
        );
    }

    assert!(
        stdout.contains("Content-Length:"),
        "responses must be Content-Length framed"
    );
    assert!(
        stdout.contains(r#""protocolVersion":"2024-11-05""#),
        "initialize must report protocol version, got:\n{}",
        stdout
    );
    // tools/list must advertise the three MCP tools.
    assert!(stdout.contains(r#""name":"send""#), "missing send tool");
    assert!(stdout.contains(r#""name":"list_peers""#), "missing list_peers tool");
    assert!(stdout.contains(r#""name":"login""#), "missing login tool");
}
