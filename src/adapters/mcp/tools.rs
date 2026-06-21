use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::application::send_message;
use crate::domain::ports::outbox_repo::OutboxRepo;
use crate::domain::value_objects::route_key::ConversationType;

// ── Tool Definitions ──

#[derive(Debug, Serialize)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

pub fn all_tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "send",
            description: "Send a text message to a conversation",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "channel": {"type": "string", "description": "Channel: wechat, dingtalk, feishu"},
                    "conversation_id": {"type": "string", "description": "Conversation ID"},
                    "peer_id": {"type": "string", "description": "Peer user or group ID"},
                    "conversation_type": {"type": "string", "enum": ["direct", "group", "thread", "bot_session"]},
                    "content": {"type": "string", "description": "Text content to send"}
                },
                "required": ["channel", "conversation_id", "peer_id", "conversation_type", "content"]
            }),
        },
        ToolDef {
            name: "list_peers",
            description: "List peers discovered from the local WeChat channel directory",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "channel": {"type": "string", "description": "Channel: wechat, dingtalk, feishu"}
                },
                "required": ["channel"]
            }),
        },
        ToolDef {
            name: "login",
            description: "Inspect the configured WeChat account and login readiness",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "channel": {"type": "string", "description": "Channel: wechat, dingtalk, feishu"},
                    "account": {"type": "string", "description": "Account identifier"}
                },
                "required": ["channel", "account"]
            }),
        },
    ]
}

// ── Tool Argument Types ──

#[derive(Debug, Deserialize)]
pub struct SendArgs {
    pub channel: String,
    pub conversation_id: String,
    pub peer_id: String,
    pub conversation_type: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct ListPeersArgs {
    pub channel: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginArgs {
    pub channel: String,
    pub account: String,
}

#[derive(Debug, Deserialize)]
struct ProjectWechatAccount {
    #[serde(rename = "token")]
    _token: String,
    #[serde(rename = "baseUrl")]
    base_url: String,
    #[serde(rename = "accountId")]
    account_id: String,
    #[serde(rename = "userId", default)]
    user_id: Option<String>,
}

fn default_wechat_data_dir() -> PathBuf {
    if let Ok(dir) = env::var("WECHAT_CHANNEL_DIR") {
        return PathBuf::from(dir);
    }

    if let Ok(home) = env::var("HOME") {
        return Path::new(&home).join(".claude").join("channels").join("wechat");
    }

    PathBuf::from(".claude/channels/wechat")
}

fn resolve_wechat_data_dir() -> PathBuf {
    default_wechat_data_dir()
}

fn load_project_wechat_account(data_dir: &Path) -> Result<ProjectWechatAccount, String> {
    let account_path = data_dir.join("account.json");
    let content = fs::read_to_string(&account_path)
        .map_err(|e| format!("failed to read {}: {}", account_path.display(), e))?;
    serde_json::from_str(&content)
        .map_err(|e| format!("failed to parse {}: {}", account_path.display(), e))
}

fn load_project_context_tokens(data_dir: &Path) -> Result<BTreeMap<String, String>, String> {
    let ctx_path = data_dir.join("context_tokens.json");
    if !ctx_path.exists() {
        return Ok(BTreeMap::new());
    }

    let content = fs::read_to_string(&ctx_path)
        .map_err(|e| format!("failed to read {}: {}", ctx_path.display(), e))?;
    serde_json::from_str(&content)
        .map_err(|e| format!("failed to parse {}: {}", ctx_path.display(), e))
}

fn is_supported_channel(channel: &str) -> bool {
    matches!(channel, "wechat")
}

// ── Tool Dispatcher ──

pub struct ToolDispatcher;

impl ToolDispatcher {
    pub fn dispatch(
        tool_name: &str,
        args: Value,
        outbox: &dyn OutboxRepo,
    ) -> Result<Value, String> {
        match tool_name {
            "send" => {
                let args: SendArgs = serde_json::from_value(args)
                    .map_err(|e| format!("invalid arguments: {}", e))?;
                Self::handle_send(args, outbox)
            }
            "list_peers" => {
                let args: ListPeersArgs = serde_json::from_value(args)
                    .map_err(|e| format!("invalid arguments: {}", e))?;
                Self::handle_list_peers(args)
            }
            "login" => {
                let args: LoginArgs = serde_json::from_value(args)
                    .map_err(|e| format!("invalid arguments: {}", e))?;
                Self::handle_login(args)
            }
            _ => Err(format!("unknown tool: {}", tool_name)),
        }
    }

    fn handle_send(args: SendArgs, outbox: &dyn OutboxRepo) -> Result<Value, String> {
        let conv_type = match args.conversation_type.as_str() {
            "direct" => ConversationType::Direct,
            "group" => ConversationType::Group,
            "thread" => ConversationType::Thread,
            "bot_session" => ConversationType::BotSession,
            other => return Err(format!("unknown conversation_type: {}", other)),
        };

        let message_id = send_message::submit_text_for_delivery(
            outbox,
            &args.channel,
            &args.conversation_id,
            &args.peer_id,
            conv_type,
            &args.content,
        )
        .map_err(|e| format!("send failed: {}", e))?;

        Ok(serde_json::json!({
            "status": "pending",
            "message_id": message_id,
            "channel": args.channel,
            "conversation_id": args.conversation_id
        }))
    }

    fn handle_list_peers(args: ListPeersArgs) -> Result<Value, String> {
        if !is_supported_channel(&args.channel) {
            return Err(format!("unsupported channel: {}", args.channel));
        }

        let data_dir = resolve_wechat_data_dir();
        let account = load_project_wechat_account(&data_dir)?;
        let context_tokens = load_project_context_tokens(&data_dir)?;

        let mut peers: Vec<Value> = context_tokens
            .keys()
            .map(|peer_id| serde_json::json!({
                "peer_id": peer_id,
                "channel": args.channel,
                "has_context_token": true,
            }))
            .collect();

        if peers.is_empty() {
            let fallback_peer = account
                .user_id
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| account.account_id.clone());
            peers.push(serde_json::json!({
                "peer_id": fallback_peer,
                "channel": args.channel,
                "has_context_token": false,
            }));
        }

        Ok(serde_json::json!({
            "status": "ok",
            "channel": args.channel,
            "account_id": account.account_id,
            "base_url": account.base_url,
            "peer_count": peers.len(),
            "peers": peers
        }))
    }

    fn handle_login(args: LoginArgs) -> Result<Value, String> {
        if !is_supported_channel(&args.channel) {
            return Err(format!("unsupported channel: {}", args.channel));
        }

        let data_dir = resolve_wechat_data_dir();
        let account = load_project_wechat_account(&data_dir)?;
        if account.account_id != args.account {
            return Err(format!("account mismatch: requested {}, configured {}", args.account, account.account_id));
        }

        let context_tokens = load_project_context_tokens(&data_dir)?;
        let has_context_token = context_tokens
            .values()
            .any(|token| !token.trim().is_empty());

        Ok(serde_json::json!({
            "status": "ok",
            "channel": args.channel,
            "account_id": account.account_id,
            "base_url": account.base_url,
            "has_context_token": has_context_token,
            "data_dir": data_dir,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use std::sync::Arc;
    use crate::adapters::sqlite_outbox::SqliteOutboxRepo;
    use crate::infrastructure::db::{init_db, DbPool};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn make_outbox() -> Arc<SqliteOutboxRepo> {
        Arc::new(SqliteOutboxRepo::new(DbPool::new(init_db(":memory:").unwrap())))
    }

    #[test]
    fn dispatch_list_peers_returns_data() {
        let _guard = TestDirGuard::new(make_temp_wechat_dir());
        let args = serde_json::json!({"channel": "wechat"});
        let result = ToolDispatcher::dispatch("list_peers", args, make_outbox().as_ref());
        assert!(result.is_ok());
        let val = result.unwrap();
        assert_eq!(val["status"], "ok");
    }

    #[test]
    fn dispatch_login_returns_data() {
        let _guard = TestDirGuard::new(make_temp_wechat_dir());
        let args = serde_json::json!({"channel": "wechat", "account": "test_account"});
        let result = ToolDispatcher::dispatch("login", args, make_outbox().as_ref());
        assert!(result.is_ok());
        let val = result.unwrap();
        assert_eq!(val["status"], "ok");
    }

    #[test]
    fn dispatch_unknown_tool_returns_err() {
        let result = ToolDispatcher::dispatch("nonexistent", serde_json::json!({}), make_outbox().as_ref());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown tool"));
    }

    #[tokio::test]
    async fn dispatch_bad_conversation_type_returns_err() {
        let args = serde_json::json!({
            "channel": "wechat",
            "conversation_id": "c1",
            "peer_id": "p1",
            "conversation_type": "invalid",
            "content": "hi"
        });
        let result = ToolDispatcher::dispatch("send", args, make_outbox().as_ref());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown conversation_type"));
    }

    #[test]
    fn all_tools_has_three_entries() {
        let tools = all_tools();
        assert_eq!(tools.len(), 3);
        let names: Vec<&str> = tools.iter().map(|t| t.name).collect();
        assert!(names.contains(&"send"));
        assert!(names.contains(&"list_peers"));
        assert!(names.contains(&"login"));
    }

    fn make_temp_wechat_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("magiclaw-mcp-tools-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("account.json"),
            r#"{"token":"secret","baseUrl":"https://example.invalid","accountId":"test_account","userId":"test_user"}"#,
        )
        .unwrap();
        fs::write(
            dir.join("context_tokens.json"),
            r#"{"peer_a":"ctx-a","peer_b":"ctx-b"}"#,
        )
        .unwrap();
        dir
    }

    struct TestDirGuard {
        _lock: MutexGuard<'static, ()>,
    }

    impl TestDirGuard {
        fn new(path: PathBuf) -> Self {
            let lock = env_lock().lock().unwrap();
            env::set_var("WECHAT_CHANNEL_DIR", path);
            Self { _lock: lock }
        }
    }

    impl Drop for TestDirGuard {
        fn drop(&mut self) {
            env::remove_var("WECHAT_CHANNEL_DIR");
        }
    }
}
