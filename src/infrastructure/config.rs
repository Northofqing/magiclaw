use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::domain::value_objects::backpressure::BackpressureConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeChatConfig {
    /// Enable real ilink HTTP sending for WeChat channel.
    #[serde(default)]
    pub enabled: bool,
    /// ilink API base URL, for example: https://ilinkai.weixin.qq.com/
    #[serde(default)]
    pub base_url: String,
    /// ilink bearer token.
    #[serde(default)]
    pub token: String,
    /// Context token used by ilink sendmessage.
    #[serde(default)]
    pub context_token: String,
    /// Channel version sent in base_info.
    #[serde(default = "default_wechat_channel_version")]
    pub channel_version: String,
    /// Account identifier used to persist sync_buf per account.
    #[serde(default = "default_wechat_account_id")]
    pub account_id: String,
    /// HTTP timeout in milliseconds.
    #[serde(default = "default_wechat_timeout_ms")]
    pub timeout_ms: u64,
    /// Short polling timeout (ms) used to refresh context_token around sends.
    #[serde(default = "default_wechat_keepalive_timeout_ms")]
    pub keepalive_timeout_ms: u64,
}

impl Default for WeChatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            token: String::new(),
            context_token: String::new(),
            channel_version: default_wechat_channel_version(),
            account_id: default_wechat_account_id(),
            timeout_ms: default_wechat_timeout_ms(),
            keepalive_timeout_ms: default_wechat_keepalive_timeout_ms(),
        }
    }
}

/// Configuration for the local Claude Code CLI backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeCodeConfig {
    /// Path or name of the `claude` binary (resolved via PATH).
    #[serde(default = "default_claude_binary")]
    pub binary_path: String,
    /// Hard timeout (seconds) before the child process is killed.
    #[serde(default = "default_claude_timeout_secs")]
    pub timeout_secs: u64,
    /// Maximum bytes read from stdout/stderr before truncation.
    #[serde(default = "default_claude_max_output_bytes")]
    pub max_output_bytes: usize,
    /// Extra CLI args. Defaults to the read-only restricted plan mode.
    #[serde(default = "default_claude_extra_args")]
    pub extra_args: Vec<String>,
}

impl Default for ClaudeCodeConfig {
    fn default() -> Self {
        Self {
            binary_path: default_claude_binary(),
            timeout_secs: default_claude_timeout_secs(),
            max_output_bytes: default_claude_max_output_bytes(),
            extra_args: default_claude_extra_args(),
        }
    }
}

/// Generic configuration for an external CLI agent backend. Lets any agent
/// (codex, copilot, hermes, openclaw, or a custom one) be wired purely via
/// config — no code changes — as long as it can be invoked headlessly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliAgentConfig {
    /// Path or name of the agent binary (resolved via PATH).
    #[serde(default = "default_agent_binary")]
    pub binary_path: String,
    /// argv tokens. The token `{prompt}` is replaced by the user message inside
    /// a single argv argument (never a shell), so metacharacters cannot be
    /// interpreted. If no token contains `{prompt}`, the prompt is appended as
    /// the final argument. When `read_output_file` is true, `{output_file}` is
    /// replaced by a temp file path the agent writes its reply to.
    #[serde(default)]
    pub args: Vec<String>,
    /// Hard timeout (seconds) before the child process is killed.
    #[serde(default = "default_agent_timeout_secs")]
    pub timeout_secs: u64,
    /// Maximum bytes read from the agent output before truncation.
    #[serde(default = "default_agent_max_output_bytes")]
    pub max_output_bytes: usize,
    /// When set, the reply is the string at this JSON pointer in the captured
    /// output (e.g. "/result"). When None, the trimmed raw output is used.
    #[serde(default)]
    pub result_json_pointer: Option<String>,
    /// When true, the reply is read from the temp file referenced by
    /// `{output_file}` instead of stdout (e.g. codex `-o <FILE>`).
    #[serde(default)]
    pub read_output_file: bool,
}

impl Default for CliAgentConfig {
    fn default() -> Self {
        Self {
            binary_path: default_agent_binary(),
            args: Vec::new(),
            timeout_secs: default_agent_timeout_secs(),
            max_output_bytes: default_agent_max_output_bytes(),
            result_json_pointer: None,
            read_output_file: false,
        }
    }
}

/// AI backend selection and per-backend settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    /// Selected backend: "echo" (default), "claude_code", a built-in CLI agent
    /// preset ("codex", "copilot"), or any key present in `agents`.
    #[serde(default = "default_ai_backend")]
    pub backend: String,
    /// Settings for the claude_code backend.
    #[serde(default)]
    pub claude_code: ClaudeCodeConfig,
    /// User-defined CLI agents, keyed by backend name. Entries here override
    /// built-in presets of the same name, so codex/copilot/hermes/openclaw or
    /// any custom agent can be configured without code changes.
    #[serde(default)]
    pub agents: HashMap<String, CliAgentConfig>,
    /// Minimum interval (ms) between messages per conversation, enforced by the
    /// RateLimit middleware whenever a non-echo (cost-bearing) AI backend is
    /// active. Guards against runaway cost and reply loops.
    #[serde(default = "default_ai_rate_limit_min_interval_ms")]
    pub rate_limit_min_interval_ms: u64,
}

/// Configuration for per-user agent switching (Phase A+).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Whether to enable per-user agent preference switching.
    #[serde(default = "default_enable_user_preferences")]
    pub enable_user_preferences: bool,
    /// Alias mappings for agent shortcuts. Keys are agent names (claude_code, codex, openclaw, hermes).
    /// Values are lists of aliases/alternative names for that agent.
    /// Defaults:
    /// - claude_code: ["cc", "claude", "claude code"]
    /// - codex: ["cx", "codex"]
    /// - openclaw: ["oc", "openclaw"]
    /// - hermes: ["h", "hermes"]
    #[serde(default = "default_agent_aliases")]
    pub aliases: HashMap<String, Vec<String>>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            enable_user_preferences: default_enable_user_preferences(),
            aliases: default_agent_aliases(),
        }
    }
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            backend: default_ai_backend(),
            claude_code: ClaudeCodeConfig::default(),
            agents: HashMap::new(),
            rate_limit_min_interval_ms: default_ai_rate_limit_min_interval_ms(),
        }
    }
}

/// Application configuration, loaded from file or env.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Backpressure settings.
    #[serde(default)]
    pub backpressure: BackpressureConfig,

    /// Dedup cache TTL in seconds.
    #[serde(default = "default_dedup_ttl")]
    pub dedup_ttl_secs: u64,

    /// Dedup cache max capacity.
    #[serde(default = "default_dedup_capacity")]
    pub dedup_max_capacity: u64,

    /// Reorder window in milliseconds.
    #[serde(default = "default_reorder_window_ms")]
    pub reorder_window_ms: u64,

    /// GC: idle timeout in seconds before a conversation is reclaimed.
    #[serde(default = "default_idle_timeout_secs")]
    pub idle_timeout_secs: u64,

    /// GC: scan interval in seconds.
    #[serde(default = "default_gc_scan_interval_secs")]
    pub gc_scan_interval_secs: u64,

    /// SQLite database path. ":memory:" for testing.
    #[serde(default = "default_db_path")]
    pub db_path: String,

    /// Bearer token required by the REST/HTTP adapter. Empty means the
    /// protected endpoints are closed (no naked default-open port) — only
    /// the unauthenticated liveness probe stays reachable.
    #[serde(default)]
    pub api_auth_token: String,

    /// WeChat channel runtime configuration.
    #[serde(default)]
    pub wechat: WeChatConfig,

    /// AI backend configuration.
    #[serde(default)]
    pub ai: AiConfig,

    /// Per-user agent preference configuration (Phase A+).
    #[serde(default)]
    pub agent: AgentConfig,
}

fn default_dedup_ttl() -> u64 {
    300
}
fn default_dedup_capacity() -> u64 {
    2_000_000
}
fn default_reorder_window_ms() -> u64 {
    200
}
fn default_idle_timeout_secs() -> u64 {
    1800
}
fn default_gc_scan_interval_secs() -> u64 {
    60
}
fn default_db_path() -> String {
    "data/aiclaw.db".into()
}
fn default_wechat_channel_version() -> String {
    "0.1.0".into()
}
fn default_wechat_account_id() -> String {
    "default".into()
}
fn default_wechat_timeout_ms() -> u64 {
    15_000
}
fn default_wechat_keepalive_timeout_ms() -> u64 {
    4_000
}
fn default_ai_backend() -> String {
    "echo".into()
}
fn default_claude_binary() -> String {
    "claude".into()
}
fn default_claude_timeout_secs() -> u64 {
    60
}
fn default_claude_max_output_bytes() -> usize {
    16_384
}
fn default_claude_extra_args() -> Vec<String> {
    vec!["--permission-mode".into(), "plan".into()]
}
fn default_agent_binary() -> String {
    "echo".into()
}
fn default_agent_timeout_secs() -> u64 {
    120
}
fn default_agent_max_output_bytes() -> usize {
    16_384
}
fn default_ai_rate_limit_min_interval_ms() -> u64 {
    3_000
}
fn default_enable_user_preferences() -> bool {
    true
}
pub fn default_agent_aliases() -> HashMap<String, Vec<String>> {
    let mut aliases = HashMap::new();
    aliases.insert(
        "claude_code".to_string(),
        vec!["cc".to_string(), "claude".to_string(), "claude code".to_string()],
    );
    aliases.insert(
        "codex".to_string(),
        vec!["cx".to_string(), "codex".to_string()],
    );
    aliases.insert(
        "openclaw".to_string(),
        vec!["oc".to_string(), "openclaw".to_string()],
    );
    aliases.insert(
        "hermes".to_string(),
        vec!["h".to_string(), "hermes".to_string()],
    );
    aliases
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            backpressure: BackpressureConfig::default(),
            dedup_ttl_secs: default_dedup_ttl(),
            dedup_max_capacity: default_dedup_capacity(),
            reorder_window_ms: default_reorder_window_ms(),
            idle_timeout_secs: default_idle_timeout_secs(),
            gc_scan_interval_secs: default_gc_scan_interval_secs(),
            db_path: default_db_path(),
            api_auth_token: String::new(),
            wechat: WeChatConfig::default(),
            ai: AiConfig::default(),
            agent: AgentConfig::default(),
        }
    }
}

impl AppConfig {
    /// Load from a JSON file.
    pub fn load(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = serde_json::from_str(&content)?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_config_without_ai_defaults_to_echo() {
        // A config file predating the AI section must still load and default
        // to the echo backend (rollback-safe, no behaviour change).
        let cfg: AppConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.ai.backend, "echo");
        assert_eq!(cfg.db_path, "data/aiclaw.db");
    }

    #[test]
    fn claude_code_config_has_restricted_defaults() {
        let cfg = ClaudeCodeConfig::default();
        assert_eq!(cfg.binary_path, "claude");
        assert_eq!(cfg.timeout_secs, 60);
        assert_eq!(cfg.max_output_bytes, 16_384);
        assert_eq!(cfg.extra_args, vec!["--permission-mode", "plan"]);
    }

    #[test]
    fn ai_backend_selection_deserializes() {
        let cfg: AppConfig =
            serde_json::from_str(r#"{"ai":{"backend":"claude_code"}}"#).unwrap();
        assert_eq!(cfg.ai.backend, "claude_code");
        // Per-backend defaults still apply when omitted.
        assert_eq!(cfg.ai.claude_code.timeout_secs, 60);
    }

    #[test]
    fn ai_defaults_enable_rate_limit_and_empty_agents() {
        let cfg = AiConfig::default();
        assert_eq!(cfg.rate_limit_min_interval_ms, 3_000);
        assert!(cfg.agents.is_empty());
    }

    #[test]
    fn custom_agent_deserializes_with_defaults() {
        // A user can wire any agent (e.g. hermes) purely via config.
        let cfg: AppConfig = serde_json::from_str(
            r#"{"ai":{"backend":"hermes","agents":{"hermes":{"binary_path":"hermes","args":["chat","{prompt}"]}}}}"#,
        )
        .unwrap();
        assert_eq!(cfg.ai.backend, "hermes");
        let agent = cfg.ai.agents.get("hermes").unwrap();
        assert_eq!(agent.binary_path, "hermes");
        assert_eq!(agent.args, vec!["chat", "{prompt}"]);
        // Per-field defaults apply when omitted.
        assert_eq!(agent.timeout_secs, 120);
        assert_eq!(agent.max_output_bytes, 16_384);
        assert!(!agent.read_output_file);
        assert!(agent.result_json_pointer.is_none());
    }
}
