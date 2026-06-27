use async_trait::async_trait;

use crate::domain::error::AiError;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::backend::AiBackend;
use crate::infrastructure::config::CliAgentConfig;

/// AI backend that invokes an arbitrary locally installed CLI agent in
/// headless mode and returns its reply. This generalises the claude_code
/// backend so codex / copilot / hermes / openclaw or any custom agent can be
/// wired purely via configuration.
///
/// Security (shared with the claude_code backend, see design doc §5):
/// - The user prompt is passed as a single argv argument — never through a
///   shell — so shell metacharacters cannot be interpreted (no command
///   injection). This holds even with the `{prompt}` placeholder, because the
///   substitution happens inside one argv token.
/// - A hard timeout kills the child (`kill_on_drop` + explicit kill); stdout
///   and stderr are read separately and capped, so oversized output cannot
///   stall the worker. stderr never leaves the host (logs only).
///
/// Any failure returns `Err`, which `AiMiddleware` degrades gracefully by
/// echoing the input — the main pipeline never panics or blocks.
pub struct CliAgentBackend {
    name: &'static str,
    config: CliAgentConfig,
}

impl CliAgentBackend {
    /// `name` is the backend label used in logs/audit. It is leaked once so it
    /// can satisfy the `&'static str` contract of `AiBackend::name`; exactly one
    /// backend is created per process, so this leak is bounded.
    pub fn new(name: &str, config: CliAgentConfig) -> Self {
        let name: &'static str = Box::leak(name.to_string().into_boxed_str());
        Self { name, config }
    }
}

#[async_trait]
impl AiBackend for CliAgentBackend {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn generate(&self, input: &str, _context: Option<&str>) -> Result<String, AiError> {
        run_cli_agent(&self.config, input).await
    }
}

/// Built-in presets for common agents. User-defined `agents` entries override
/// these, so the presets are just sensible, restricted defaults.
pub fn preset(name: &str) -> Option<CliAgentConfig> {
    match name {
        // OpenAI Codex CLI, restricted to a read-only sandbox. The final
        // assistant message is written to a temp file via `-o` (clean plain
        // text), which we read back instead of parsing the JSONL event stream.
        "codex" => Some(CliAgentConfig {
            binary_path: "codex".into(),
            args: [
                "exec",
                "--skip-git-repo-check",
                "--sandbox",
                "read-only",
                "--color",
                "never",
                "-o",
                "{output_file}",
                "{prompt}",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            timeout_secs: 120,
            max_output_bytes: 16_384,
            result_json_pointer: None,
            read_output_file: true,
        }),
        // GitHub Copilot CLI in headless prompt mode. Requires the standalone
        // public `copilot` CLI on PATH (the VS Code-bundled helper is not it);
        // override via config if your install differs.
        "copilot" => Some(CliAgentConfig {
            binary_path: "copilot".into(),
            args: ["-p", "{prompt}"].iter().map(|s| s.to_string()).collect(),
            timeout_secs: 90,
            max_output_bytes: 16_384,
            result_json_pointer: None,
            read_output_file: false,
        }),
        _ => None,
    }
}

/// Run a CLI agent with the given config and prompt. Shared by both the generic
/// [`CliAgentBackend`] and the specialised claude_code backend.
pub(crate) async fn run_cli_agent(cfg: &CliAgentConfig, prompt: &str) -> Result<String, AiError> {
    let cap = cfg.max_output_bytes;

    // Optional reply file for agents that write their final message to disk
    // (e.g. codex `-o <FILE>`), rather than to stdout.
    let out_file: Option<PathBuf> = if cfg.read_output_file {
        Some(std::env::temp_dir().join(format!(
            "magiclaw_agent_{}_{}.out",
            std::process::id(),
            uuid::Uuid::new_v4()
        )))
    } else {
        None
    };

    // Substitute placeholders. The prompt always lands inside a single argv
    // token, so shell metacharacters can never be interpreted.
    let mut have_prompt = false;
    let mut argv: Vec<String> = Vec::with_capacity(cfg.args.len() + 1);
    for a in &cfg.args {
        if a.contains("{prompt}") {
            have_prompt = true;
            argv.push(a.replace("{prompt}", prompt));
        } else if a.contains("{output_file}") {
            match &out_file {
                Some(p) => argv.push(a.replace("{output_file}", &p.to_string_lossy())),
                None => argv.push(a.clone()),
            }
        } else {
            argv.push(a.clone());
        }
    }
    if !have_prompt {
        argv.push(prompt.to_string());
    }

    let mut cmd = Command::new(&cfg.binary_path);
    cmd.args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            cleanup_file(&out_file);
            return Err(AiError::Transport(format!("failed to spawn agent '{}': {}", cfg.binary_path, e)));
        }
    };

    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            cleanup_file(&out_file);
            return Err(AiError::Internal("no stdout pipe".into()));
        }
    };
    let stderr = match child.stderr.take() {
        Some(s) => s,
        None => {
            cleanup_file(&out_file);
            return Err(AiError::Internal("no stderr pipe".into()));
        }
    };

    let read_out = read_capped(stdout, cap);
    let read_err = read_capped(stderr, cap);
    let wait = child.wait();

    let timeout = Duration::from_secs(cfg.timeout_secs);
    let joined =
        tokio::time::timeout(timeout, async { tokio::join!(wait, read_out, read_err) }).await;

    let (status, out_bytes, err_bytes) = match joined {
        Ok(t) => t,
        Err(_) => {
            // Timed out — kill and reap the child so no zombie/orphan remains.
            let _ = child.start_kill();
            let _ = child.wait().await;
            cleanup_file(&out_file);
            return Err(AiError::Timeout(cfg.timeout_secs * 1000));
        }
    };

    let stderr_text = String::from_utf8_lossy(&err_bytes);
    let status = match status {
        Ok(s) => s,
        Err(e) => {
            cleanup_file(&out_file);
            return Err(AiError::Transport(format!("failed to wait for agent: {}", e)));
        }
    };
    if !status.success() {
        // stderr stays on the host (logs only); never returned to WeChat.
        tracing::warn!(
            binary = %cfg.binary_path,
            code = ?status.code(),
            stderr = %stderr_text.trim(),
            "agent exited with non-zero status"
        );
        cleanup_file(&out_file);
        return Err(AiError::Backend {
            status: status.code().unwrap_or(0) as u16,
            body: format!(
                "agent '{}' exited with status {:?}",
                cfg.binary_path,
                status.code()
            ),
        });
    }

    // Source of the reply: the output file (if requested) or stdout.
    let raw = if let Some(p) = &out_file {
        let content = read_file_capped(p, cap);
        cleanup_file(&out_file);
        match content {
            Some(c) => c,
            None => {
                return Err(AiError::Backend {
                    status: 0,
                    body: format!(
                        "agent '{}' did not write its output file",
                        cfg.binary_path
                    ),
                })
            }
        }
    } else {
        String::from_utf8_lossy(&out_bytes).into_owned()
    };

    let reply = extract_reply(&raw, &cfg.result_json_pointer)?;
    Ok(sanitize(&reply, cap))
}

/// Read up to `limit` bytes from a child pipe, draining the rest.
async fn read_capped<R>(mut reader: R, limit: usize) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf = Vec::with_capacity(limit.min(8192));
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() < limit {
                    let take = n.min(limit - buf.len());
                    buf.extend_from_slice(&chunk[..take]);
                }
                // Keep draining the pipe to avoid blocking the child, but stop
                // storing once the cap is reached.
            }
            Err(_) => break,
        }
    }
    buf
}

/// Read up to `limit` bytes from a file written by the agent, if it exists.
fn read_file_capped(path: &PathBuf, limit: usize) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let slice = if bytes.len() > limit { &bytes[..limit] } else { &bytes[..] };
    Some(String::from_utf8_lossy(slice).into_owned())
}

/// Best-effort cleanup of the temp output file.
fn cleanup_file(path: &Option<PathBuf>) {
    if let Some(p) = path {
        let _ = std::fs::remove_file(p);
    }
}

/// Extract the reply text from the captured agent output. When a JSON pointer
/// is configured, parse the output as JSON and read the string there (also
/// honouring a conventional `is_error` flag); otherwise use the trimmed raw
/// text. Falls back to the raw text if a pointer is set but the output is not
/// JSON.
fn extract_reply(raw: &str, pointer: &Option<String>) -> Result<String, String> {
    match pointer {
        Some(ptr) => match serde_json::from_str::<serde_json::Value>(raw.trim()) {
            Ok(v) => {
                if v.get("is_error").and_then(|e| e.as_bool()) == Some(true) {
                    return Err(format!(
                        "agent reported error: {}",
                        v.get("subtype").and_then(|s| s.as_str()).unwrap_or("unknown")
                    ));
                }
                match v.pointer(ptr).and_then(|r| r.as_str()) {
                    Some(text) if !text.is_empty() => Ok(text.to_string()),
                    _ => Err(format!("agent json missing non-empty value at pointer '{}'", ptr)),
                }
            }
            // Not JSON (e.g. plain-text output): use the raw text.
            Err(_) if !raw.trim().is_empty() => Ok(raw.trim().to_string()),
            Err(e) => Err(format!("failed to parse agent output: {}", e)),
        },
        None => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                Err("agent produced empty output".to_string())
            } else {
                Ok(trimmed.to_string())
            }
        }
    }
}

/// Strip ASCII control characters (except tab/newline) and cap length, so model
/// output is treated as untrusted text before it reaches a channel.
fn sanitize(raw: &str, limit: usize) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.len() > limit {
        let mut cut = limit;
        while cut > 0 && !trimmed.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}…(truncated)", &trimmed[..cut])
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Write an executable stub script to a temp path and return it.
    fn write_stub(name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("magiclaw_agent_stub_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        let mut perms = f.metadata().unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn cfg(binary: std::path::PathBuf, args: Vec<&str>) -> CliAgentConfig {
        CliAgentConfig {
            binary_path: binary.to_string_lossy().into_owned(),
            args: args.into_iter().map(|s| s.to_string()).collect(),
            timeout_secs: 15,
            max_output_bytes: 16_384,
            result_json_pointer: None,
            read_output_file: false,
        }
    }

    #[tokio::test]
    async fn raw_stdout_is_returned_trimmed() {
        let stub = write_stub("agent_raw", "#!/bin/sh\nprintf '  hello there  \\n'\n");
        let be = CliAgentBackend::new("codex", cfg(stub, vec![]));
        let out = be.generate("hi", None).await.unwrap();
        assert_eq!(out, "hello there");
    }

    #[tokio::test]
    async fn prompt_placeholder_is_passed_literally() {
        // Stub echoes back the arg that replaced {prompt}. If a shell had
        // interpreted it, the substitution would have changed the text.
        let stub = write_stub("agent_echo", "#!/bin/sh\nprintf '%s' \"$2\"\n");
        let be = CliAgentBackend::new("codex", cfg(stub, vec!["--flag", "{prompt}"]));
        let danger = "$(echo PWNED)";
        let out = be.generate(danger, None).await.unwrap();
        assert_eq!(out, danger, "prompt must be passed literally, not shell-expanded");
    }

    #[tokio::test]
    async fn prompt_is_appended_when_no_placeholder() {
        // No {prompt} token -> prompt appended as the final arg ($1 here).
        let stub = write_stub("agent_append", "#!/bin/sh\nprintf '%s' \"$1\"\n");
        let be = CliAgentBackend::new("custom", cfg(stub, vec![]));
        let out = be.generate("appended", None).await.unwrap();
        assert_eq!(out, "appended");
    }

    #[tokio::test]
    async fn json_pointer_extracts_nested_field() {
        let stub = write_stub(
            "agent_json",
            "#!/bin/sh\nprintf '%s' '{\"is_error\":false,\"result\":\"deep\"}'\n",
        );
        let mut c = cfg(stub, vec![]);
        c.result_json_pointer = Some("/result".to_string());
        let be = CliAgentBackend::new("custom", c);
        let out = be.generate("hi", None).await.unwrap();
        assert_eq!(out, "deep");
    }

    #[tokio::test]
    async fn output_file_is_read_and_cleaned_up() {
        // Stub writes its reply to the file path passed after `-o`.
        let stub = write_stub(
            "agent_outfile",
            "#!/bin/sh\nwhile [ \"$1\" != \"-o\" ]; do shift; done\nprintf 'from-file' > \"$2\"\n",
        );
        let mut c = cfg(stub, vec!["-o", "{output_file}", "{prompt}"]);
        c.read_output_file = true;
        let be = CliAgentBackend::new("codex", c);
        let out = be.generate("hi", None).await.unwrap();
        assert_eq!(out, "from-file");
    }

    #[tokio::test]
    async fn timeout_kills_child_and_errors() {
        let stub = write_stub("agent_hang", "#!/bin/sh\nsleep 30\n");
        let mut c = cfg(stub, vec![]);
        c.timeout_secs = 1;
        let be = CliAgentBackend::new("custom", c);
        let err = be.generate("hi", None).await.unwrap_err();
        assert!(err.to_string().contains("timeout"), "got: {}", err);
    }

    #[tokio::test]
    async fn missing_binary_errors() {
        let be = CliAgentBackend::new(
            "custom",
            cfg(std::path::PathBuf::from("/nonexistent/agent_xyz"), vec![]),
        );
        let err = be.generate("hi", None).await.unwrap_err();
        assert!(err.to_string().contains("failed to spawn"), "got: {}", err);
    }

    #[tokio::test]
    async fn non_zero_exit_errors() {
        let stub = write_stub("agent_fail", "#!/bin/sh\necho boom 1>&2\nexit 3\n");
        let be = CliAgentBackend::new("custom", cfg(stub, vec![]));
        let err = be.generate("hi", None).await.unwrap_err();
        assert!(err.to_string().contains("status"), "got: {}", err);
    }

    #[test]
    fn sanitize_strips_control_and_caps() {
        let s = sanitize("a\u{0007}b\nc", 100);
        assert_eq!(s, "ab\nc");
        let long = "x".repeat(50);
        let capped = sanitize(&long, 10);
        assert!(capped.ends_with("…(truncated)"));
    }

    #[test]
    fn extract_reply_flags_is_error() {
        let err = extract_reply(r#"{"is_error":true,"subtype":"auth"}"#, &Some("/result".into()))
            .unwrap_err();
        assert!(err.to_string().contains("auth"));
    }

    #[test]
    fn extract_reply_raw_when_no_pointer() {
        assert_eq!(extract_reply("  plain  ", &None).unwrap(), "plain");
        assert!(extract_reply("   ", &None).is_err());
    }

    #[test]
    fn codex_preset_is_restricted() {
        let p = preset("codex").unwrap();
        assert_eq!(p.binary_path, "codex");
        assert!(p.read_output_file);
        assert!(p.args.iter().any(|a| a == "read-only"));
        assert!(p.args.iter().any(|a| a == "{prompt}"));
        assert!(p.args.iter().any(|a| a == "{output_file}"));
    }

    #[test]
    fn unknown_preset_is_none() {
        assert!(preset("hermes").is_none());
        assert!(preset("openclaw").is_none());
    }
}
