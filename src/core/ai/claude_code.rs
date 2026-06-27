use async_trait::async_trait;

use crate::domain::error::AiError;

use super::backend::AiBackend;
use super::cli_agent::run_cli_agent;
use crate::infrastructure::config::{ClaudeCodeConfig, CliAgentConfig};

/// AI backend that invokes the locally installed `claude` CLI (Claude Code) in
/// headless, read-only mode and returns its reply.
///
/// This is a thin, security-restricted preset over the shared CLI-agent
/// spawner (see [`crate::core::ai::cli_agent`]):
/// - The user prompt is passed as a single argv argument — never through a
///   shell — so shell metacharacters cannot be interpreted (no command
///   injection).
/// - Runs with `--permission-mode plan` (read-only) by default, so the agent
///   cannot edit files or execute commands on the host.
/// - A hard timeout kills the child; stdout and stderr are read separately and
///   capped; the `--output-format json` `result` field is extracted.
///
/// Any failure returns `Err`, which `AiMiddleware` degrades gracefully by
/// echoing the input — the main pipeline never panics or blocks.
pub struct ClaudeCodeBackend {
    config: ClaudeCodeConfig,
}

impl ClaudeCodeBackend {
    pub fn new(config: ClaudeCodeConfig) -> Self {
        Self { config }
    }

    /// Map the claude-specific config onto the generic CLI-agent invocation:
    /// `claude -p <prompt> --output-format json <extra_args...>`, extracting the
    /// reply from the JSON `result` field.
    fn to_cli_config(&self) -> CliAgentConfig {
        let mut args = vec![
            "-p".to_string(),
            "{prompt}".to_string(),
            "--output-format".to_string(),
            "json".to_string(),
        ];
        args.extend(self.config.extra_args.iter().cloned());
        CliAgentConfig {
            binary_path: self.config.binary_path.clone(),
            args,
            timeout_secs: self.config.timeout_secs,
            max_output_bytes: self.config.max_output_bytes,
            result_json_pointer: Some("/result".to_string()),
            read_output_file: false,
        }
    }
}

#[async_trait]
impl AiBackend for ClaudeCodeBackend {
    fn name(&self) -> &'static str {
        "claude_code"
    }

    async fn generate(&self, input: &str, _context: Option<&str>) -> Result<String, AiError> {
        run_cli_agent(&self.to_cli_config(), input).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Write an executable stub script to a temp path and return it.
    fn write_stub(name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("magiclaw_claude_stub_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        let mut perms = f.metadata().unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn cfg(binary: std::path::PathBuf, extra: Vec<String>) -> ClaudeCodeConfig {
        ClaudeCodeConfig {
            binary_path: binary.to_string_lossy().into_owned(),
            timeout_secs: 15,
            max_output_bytes: 16_384,
            extra_args: extra,
        }
    }

    #[tokio::test]
    async fn returns_reply_from_json_result() {
        // Stub prints a claude-style JSON result regardless of args.
        let stub = write_stub(
            "claude_ok",
            "#!/bin/sh\nprintf '%s' '{\"type\":\"result\",\"is_error\":false,\"result\":\"hello world\"}'\n",
        );
        let be = ClaudeCodeBackend::new(cfg(stub, vec![]));
        let out = be.generate("hi", None).await.unwrap();
        assert_eq!(out, "hello world");
    }

    #[tokio::test]
    async fn prompt_metacharacters_are_passed_literally() {
        // Stub echoes its prompt arg back as the json result ($2 is the value
        // after `-p`). If a shell had interpreted it, the substitution would
        // have changed the text.
        let stub = write_stub(
            "claude_echo_arg",
            "#!/bin/sh\nprintf '{\"is_error\":false,\"result\":\"%s\"}' \"$2\"\n",
        );
        let be = ClaudeCodeBackend::new(cfg(stub, vec![]));
        let danger = "$(echo PWNED)";
        let out = be.generate(danger, None).await.unwrap();
        assert_eq!(out, danger, "prompt must be passed literally, not shell-expanded");
    }

    #[tokio::test]
    async fn timeout_kills_child_and_errors() {
        let stub = write_stub("claude_hang", "#!/bin/sh\nsleep 30\n");
        let mut c = cfg(stub, vec![]);
        c.timeout_secs = 1;
        let be = ClaudeCodeBackend::new(c);
        let err = be.generate("hi", None).await.unwrap_err();
        assert!(err.to_string().contains("timeout"), "got: {}", err);
    }

    #[tokio::test]
    async fn missing_binary_errors() {
        let be = ClaudeCodeBackend::new(cfg(
            std::path::PathBuf::from("/nonexistent/claude_xyz"),
            vec![],
        ));
        let err = be.generate("hi", None).await.unwrap_err();
        assert!(err.to_string().contains("failed to spawn"), "got: {}", err);
    }

    #[tokio::test]
    async fn non_zero_exit_errors() {
        let stub = write_stub("claude_fail", "#!/bin/sh\necho 'boom' 1>&2\nexit 3\n");
        let be = ClaudeCodeBackend::new(cfg(stub, vec![]));
        let err = be.generate("hi", None).await.unwrap_err();
        assert!(err.to_string().contains("status"), "got: {}", err);
    }
}
