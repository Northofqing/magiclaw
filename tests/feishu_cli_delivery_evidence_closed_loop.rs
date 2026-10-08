//! Real CLI subprocesses, synthetic credentials, and loopback HTTP only.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

use magiclaw::cli::delivery_result::{sha256_bytes, FEISHU_DELIVERY_SCHEMA_V1};
use serde_json::{json, Value};

const APP: &str = "TEST_CODE_APP";
const SECRET: &str = "TEST_CODE_SECRET_NEVER_IN_RESULT";
const TARGET: &str = "oc_TEST_CODE_TARGET";
const TEXT: &str = " TEST_CODE_BODY\n中文 ";
const REMOTE_ID: &str = "om_x100b00000000000000000000000000";

#[derive(Clone, Copy, Debug)]
enum Mode {
    Success,
    AuthReject,
    AuthDrop,
    AuthMalformed,
    AuthRedirect,
    MessageDrop,
    MessageMalformed,
    MessageReject,
    MissingRemoteId,
    BlockAuth,
    BlockMessage,
}

struct Fixture {
    base_url: String,
    auth: Arc<AtomicUsize>,
    messages: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    events: mpsc::Receiver<String>,
    thread: Option<thread::JoinHandle<()>>,
}

fn request(stream: &mut TcpStream) -> Option<(String, Value)> {
    // macOS accepted sockets inherit the nonblocking listener setting.
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0; 2048];
    let header_end;
    loop {
        let size = stream.read(&mut buffer).ok()?;
        if size == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..size]);
        if let Some(at) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
            header_end = at + 4;
            break;
        }
    }
    let header = String::from_utf8_lossy(&bytes[..header_end]);
    let path = header.lines().next()?.split_whitespace().nth(1)?.to_owned();
    let length: usize = header
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())
                .flatten()
        })
        .unwrap_or(0);
    while bytes.len() < header_end + length {
        let size = stream.read(&mut buffer).ok()?;
        if size == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..size]);
    }
    let body = serde_json::from_slice(&bytes[header_end..header_end + length]).ok()?;
    Some((path, body))
}

impl Fixture {
    fn new(mode: Mode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let auth = Arc::new(AtomicUsize::new(0));
        let messages = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (events_tx, events) = mpsc::channel();
        let (a, m, done, saved) = (
            auth.clone(),
            messages.clone(),
            stop.clone(),
            requests.clone(),
        );
        let thread = thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(v) => v,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(e) => panic!("fixture listener: {e}"),
                };
                let Some((path, body)) = request(&mut stream) else {
                    continue;
                };
                let is_auth = path == "/open-apis/auth/v3/tenant_access_token/internal";
                if is_auth {
                    a.fetch_add(1, Ordering::SeqCst);
                } else {
                    assert!(path.starts_with("/open-apis/im/v1/messages?"));
                    m.fetch_add(1, Ordering::SeqCst);
                }
                saved.lock().unwrap().push((path, body));
                events_tx
                    .send(if is_auth { "auth" } else { "message" }.into())
                    .ok();
                if matches!(
                    (mode, is_auth),
                    (Mode::BlockAuth, true) | (Mode::BlockMessage, false)
                ) {
                    while !done.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(2));
                    }
                    continue;
                }
                if matches!(
                    (mode, is_auth),
                    (Mode::AuthDrop, true) | (Mode::MessageDrop, false)
                ) {
                    continue;
                }
                let response = if is_auth {
                    match mode {
                        Mode::AuthReject => json!({"code": 999, "msg": SECRET}).to_string(),
                        Mode::AuthMalformed => "{".into(),
                        _ => json!({"code": 0, "msg": "ok", "tenant_access_token": "TEST_CODE_SYNTHETIC_TOKEN"}).to_string(),
                    }
                } else {
                    match mode {
                        Mode::MessageMalformed => "{".into(),
                        Mode::MessageReject => json!({"code": 230001, "msg": SECRET}).to_string(),
                        Mode::MissingRemoteId => {
                            json!({"code": 0, "msg": "ok", "data": {}}).to_string()
                        }
                        _ => json!({"code": 0, "msg": "ok", "data": {"message_id": REMOTE_ID}})
                            .to_string(),
                    }
                };
                let (status, extra) = if is_auth && matches!(mode, Mode::AuthRedirect) {
                    (
                        "307 Temporary Redirect",
                        "Location: /open-apis/im/v1/messages?receive_id_type=chat_id\r\n",
                    )
                } else {
                    ("200 OK", "")
                };
                write!(stream, "HTTP/1.1 {status}\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).ok();
            }
        });
        Self {
            base_url,
            auth,
            messages,
            stop,
            requests,
            events,
            thread: Some(thread),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}

struct Cli {
    cwd: PathBuf,
    nonce: String,
}

impl Cli {
    fn new() -> Self {
        let nonce = uuid::Uuid::new_v4().to_string();
        let cwd = std::env::temp_dir().join(format!("magiclaw-evidence-{nonce}"));
        std::fs::create_dir(&cwd).unwrap();
        std::fs::write(cwd.join(".env"), "").unwrap();
        Self { cwd, nonce }
    }

    fn command(&self, fixture: &Fixture, typed: bool) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_magiclaw"));
        command
            .current_dir(&self.cwd)
            .env_clear()
            .env("FEISHU_APP_ID", APP)
            .env("FEISHU_APP_SECRET", SECRET)
            .env("FEISHU_BASE_URL", &fixture.base_url)
            .env("FEISHU_ACCOUNT_ID", "TEST_CODE_ACCOUNT")
            .env("FEISHU_RECEIVE_ID_TYPE", "open_id")
            .arg("send")
            .arg("--channel")
            .arg("feishu")
            .arg("--to")
            .arg(format!(" {TARGET} "))
            .arg("--message")
            .arg(TEXT)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Ok(profile) = std::env::var("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        if typed {
            command
                .arg("--delivery-result-json-v1")
                .arg("--invocation-id")
                .arg(&self.nonce);
        }
        command
    }

    fn output(&self, fixture: &Fixture) -> Output {
        self.command(fixture, true).output().unwrap()
    }

    fn json(&self, output: &Output) -> Value {
        let stdout = String::from_utf8(output.stdout.clone()).unwrap();
        assert!(stdout.ends_with('\n'));
        assert_eq!(stdout.lines().count(), 1, "stdout must be one JSON only");
        let value: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(value["schema"], FEISHU_DELIVERY_SCHEMA_V1);
        assert_eq!(value["invocation_id"], self.nonce);
        assert_eq!(value["channel"], "feishu");
        assert_eq!(value["account_id"], "TEST_CODE_ACCOUNT");
        assert_eq!(value["app_id_sha256"], sha256_bytes(APP.as_bytes()));
        assert_eq!(value["receive_id_type"], "chat_id");
        assert_eq!(value["target_sha256"], sha256_bytes(TARGET.as_bytes()));
        assert_eq!(value["content_sha256"], sha256_bytes(TEXT.as_bytes()));
        for secret in [SECRET, "TEST_CODE_SYNTHETIC_TOKEN", TARGET, TEXT] {
            assert!(!stdout.contains(secret));
        }
        value
    }
}

impl Drop for Cli {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.cwd).unwrap();
    }
}

#[test]
fn auth_failures_are_before_message_in_the_real_cli() {
    for mode in [
        Mode::AuthReject,
        Mode::AuthDrop,
        Mode::AuthMalformed,
        Mode::AuthRedirect,
    ] {
        let fixture = Fixture::new(mode);
        let cli = Cli::new();
        let output = cli.output(&fixture);
        let json = cli.json(&output);
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(json["kind"], "RejectedBeforeMessage");
        assert_eq!(json["phase"], "auth");
        assert_eq!(json["reason_code"], "feishu_auth_failed_before_message");
        assert_eq!(json["message_request_started"], false);
        assert!(json.get("receipt").is_none());
        assert_eq!(fixture.auth.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.messages.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn all_message_stage_failures_remain_uncertain() {
    for mode in [
        Mode::MessageDrop,
        Mode::MessageMalformed,
        Mode::MessageReject,
        Mode::MissingRemoteId,
    ] {
        let fixture = Fixture::new(mode);
        let cli = Cli::new();
        let output = cli.output(&fixture);
        let json = cli.json(&output);
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            json["kind"],
            "Uncertain",
            "mode={mode:?} auth={} message={} requests={:?} stderr={}",
            fixture.auth.load(Ordering::SeqCst),
            fixture.messages.load(Ordering::SeqCst),
            fixture.requests.lock().unwrap(),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(json["phase"], "message");
        assert_eq!(json["reason_code"], "feishu_message_result_unconfirmed");
        assert_eq!(json["message_request_started"], true);
        assert!(json.get("receipt").is_none());
        assert_eq!(fixture.auth.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.messages.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn success_has_remote_receipt_and_actual_request_binding() {
    let fixture = Fixture::new(Mode::Success);
    let cli = Cli::new();
    let output = cli.output(&fixture);
    let json = cli.json(&output);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(json["kind"], "Accepted");
    assert_eq!(json["receipt"]["platform_message_id"], REMOTE_ID);
    assert!(!json["receipt"]["message_id"].as_str().unwrap().is_empty());
    assert_eq!(fixture.auth.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.messages.load(Ordering::SeqCst), 1);
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests[0].1["app_id"], APP);
    assert_eq!(requests[1].1["receive_id"], TARGET);
    assert_eq!(requests[1].1["msg_type"], "text");
    assert_eq!(
        serde_json::from_str::<Value>(requests[1].1["content"].as_str().unwrap()).unwrap()["text"],
        TEXT
    );
    assert_eq!(
        std::fs::read_dir(&cli.cwd).unwrap().count(),
        1,
        "direct CLI must not initialize a database"
    );
}

#[test]
fn legacy_stdout_is_unchanged_and_has_one_physical_send() {
    let fixture = Fixture::new(Mode::Success);
    let cli = Cli::new();
    let output = cli.command(&fixture, false).output().unwrap();
    assert!(
        output.status.success(),
        "auth={} messages={} requests={:?} stderr={}",
        fixture.auth.load(Ordering::SeqCst),
        fixture.messages.load(Ordering::SeqCst),
        fixture.requests.lock().unwrap(),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("send ok (feishu): message_id="));
    assert!(stdout.ends_with(&format!(", platform_msg_id={REMOTE_ID}\n")));
    assert_eq!(stdout.lines().count(), 1);
    assert_eq!(fixture.auth.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.messages.load(Ordering::SeqCst), 1);
}

#[test]
fn explicit_receive_type_and_preflight_failure_are_visible() {
    let fixture = Fixture::new(Mode::Success);
    let cli = Cli::new();
    let output = cli
        .command(&fixture, true)
        .arg("--receive-id-type")
        .arg("invalid_TEST_CODE")
        .output()
        .unwrap();
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json["receive_id_type"], "invalid_TEST_CODE");
    assert_eq!(json["kind"], "Uncertain");
    assert_eq!(json["phase"], "preflight");
    assert_eq!(json["message_request_started"], false);
    assert!(json.get("receipt").is_none());
    assert_eq!(fixture.auth.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.messages.load(Ordering::SeqCst), 0);
}

fn wait_for(child: &mut Child, fixture: &Fixture, stage: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if fixture
            .events
            .recv_timeout(Duration::from_millis(100))
            .ok()
            .as_deref()
            == Some(stage)
        {
            return;
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "CLI exited before fixture stage"
        );
        assert!(Instant::now() < deadline, "CLI did not reach fixture stage");
    }
}

#[test]
fn killed_cli_has_no_terminal_evidence_and_isolated_next_invocation() {
    for (mode, stage, expected_messages) in [
        (Mode::BlockAuth, "auth", 0),
        (Mode::BlockMessage, "message", 1),
    ] {
        let fixture = Fixture::new(mode);
        let cli = Cli::new();
        let mut child = cli.command(&fixture, true).spawn().unwrap();
        wait_for(&mut child, &fixture, stage);
        child.kill().unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(
            output.stdout.is_empty(),
            "kill must not mint an auth rejection"
        );
        assert_eq!(fixture.messages.load(Ordering::SeqCst), expected_messages);
        fixture.stop.store(true, Ordering::SeqCst);
        let next = Fixture::new(Mode::Success);
        let fresh = Cli::new();
        assert_ne!(fresh.nonce, cli.nonce);
        assert_eq!(fresh.json(&fresh.output(&next))["kind"], "Accepted");
    }
}
