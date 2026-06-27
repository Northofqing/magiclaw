//! CLI command type definitions.

use serde::{Deserialize, Serialize};

/// Top-level CLI command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    Daemon,
    Mcp,
    Send(SendCommand),
    Auth(AuthCommand),
    WeChat(WechatCommand),
    BindImport(ImportCommand),
    PushImport(ImportCommand),
    PushRun(String),
    ProjectList,
    BindingList(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthCommand {
    Issue(AuthIssueCommand),
    List(AuthListCommand),
    Revoke(AuthRevokeCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WechatCommand {
    Login(WechatLoginCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WechatLoginCommand {
    pub data_dir: Option<String>,
    pub account_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthIssueCommand {
    pub project_id: String,
    pub client_name: String,
    pub scopes: Vec<String>,
    pub ttl_secs: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthListCommand {
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRevokeCommand {
    pub token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendChannel {
    Wechat,
    Feishu,
}

impl SendChannel {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "wechat" | "weixin" | "wx" => Ok(SendChannel::Wechat),
            "feishu" | "lark" => Ok(SendChannel::Feishu),
            other => Err(format!(
                "unknown channel: '{}' (expected: wechat | feishu)",
                other
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendCommand {
    pub channel: SendChannel,
    pub data_dir: String,
    pub to: Option<String>,
    pub context_token: Option<String>,
    pub receive_id_type: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportCommand {
    pub format: ImportFormat,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportFormat {
    Jsonl,
    Csv,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectWechatAccount {
    pub token: String,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    #[serde(rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "userId", default)]
    pub user_id: Option<String>,
    #[serde(rename = "savedAt", skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<String>,
}
