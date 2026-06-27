/// Middleware for handling user agent preference switching via commands.
use async_trait::async_trait;
use std::sync::Arc;
use crate::domain::error::PipelineError;

use crate::application::agent_preferences;
use crate::domain::entities::message::MessageContent;
use crate::domain::ports::user_preference_store::UserPreferenceStore;
use crate::infrastructure::config::{default_agent_aliases, AgentConfig};

use super::agent_command::AgentCommandParser;
use super::middleware::{Middleware, PipelineContext};

/// AgentCommand middleware: intercepts and handles agent switching commands.
/// This runs early in the pipeline before any message processing.
pub struct AgentCommandMiddleware {
    store: Arc<dyn UserPreferenceStore>,
    parser: AgentCommandParser,
    config: AgentConfig,
}

impl AgentCommandMiddleware {
    pub fn new(store: Arc<dyn UserPreferenceStore>, config: AgentConfig) -> Self {
        // Guardrail: if aliases are empty (misconfig), fall back to defaults
        // so `cc/cx/oc/h` still work in production.
        let aliases = if config.aliases.is_empty() {
            default_agent_aliases()
        } else {
            config.aliases.clone()
        };
        let parser = AgentCommandParser::new(aliases);
        tracing::info!(
            enable_user_preferences = config.enable_user_preferences,
            alias_agent_count = parser.aliases.len(),
            "agent command middleware initialized"
        );
        Self { store, parser, config }
    }
}

#[async_trait]
impl Middleware for AgentCommandMiddleware {
    fn name(&self) -> &'static str { "agent_command" }

    async fn process(&self, mut ctx: PipelineContext) -> Result<PipelineContext, PipelineError> {
        // Only process text messages
        let text = match &ctx.message.content {
            MessageContent::Text(t) => t,
            _ => return Ok(ctx), // Not a text message, pass through
        };

        // Try to parse as an agent command
        let command = self.parser.parse(text);
        tracing::info!(text = %text, parsed = ?command, "agent command parsed");

        // Extract route info for database operations
        let channel = ctx.message.route_key.channel.as_str();
        let _conversation_id = &ctx.message.route_key.conversation_id;
        let peer_id = &ctx.message.route_key.peer_id;
        // For now, use a fixed account_scope (could be extracted from RouteKey in future)
        let account_scope = ctx.config.wechat.account_id.as_str();

        use crate::core::pipeline::agent_command::AgentCommand;
        match command {
            AgentCommand::Switch(agent_name) => {
                // Save the preference
                agent_preferences::set_user_agent_via_port(self.store.as_ref(), channel, account_scope, peer_id, &agent_name)?;

                tracing::info!(
                    peer_id = peer_id,
                    agent = agent_name,
                    "user switched agent"
                );

                // Send a response and short-circuit
                ctx.ai_response = Some(format!("已切换到 {}", agent_name));
                ctx.short_circuit = true;
                Ok(ctx)
            }

            AgentCommand::Query => {
                // Get current preference and respond
                match agent_preferences::get_user_agent_via_port(self.store.as_ref(), channel, account_scope, peer_id)? {
                    Some(agent_name) => {
                        ctx.ai_response = Some(format!("当前使用 {}", agent_name));
                    }
                    None => {
                        ctx.ai_response = Some(
                            "还未选择 agent，请发送 cc/cx/oc/h 或完整名称进行切换".to_string(),
                        );
                    }
                }
                ctx.short_circuit = true;
                Ok(ctx)
            }

            AgentCommand::SwitchAndProcess(agent_name, new_text) => {
                // Save the preference
                agent_preferences::set_user_agent_via_port(self.store.as_ref(), channel, account_scope, peer_id, &agent_name)?;

                tracing::info!(
                    peer_id = peer_id,
                    agent = agent_name,
                    "user switched agent and processing message"
                );

                // Replace message content with the remaining text and mark user agent selection
                ctx.message.content = MessageContent::Text(new_text);
                ctx.user_agent_selection = Some(agent_name);
                Ok(ctx)
            }

            AgentCommand::NotCommand => {
                // Regular message: check if user has a preference
                if !self.config.enable_user_preferences {
                    // Feature disabled, proceed normally
                    return Ok(ctx);
                }

                match agent_preferences::get_user_agent_via_port(self.store.as_ref(), channel, account_scope, peer_id)? {
                    Some(agent_name) => {
                        // User has a preference, use it
                        ctx.user_agent_selection = Some(agent_name);
                        Ok(ctx)
                    }
                    None => {
                        // User hasn't selected an agent, prompt them and short-circuit
                        ctx.ai_response = Some(
                            "请先选择 AI agent。发送以下命令之一进行切换：\n\
                             • cc (claude code)\n\
                             • cx (codex)\n\
                             • oc (openclaw)\n\
                             • h (hermes)"
                                .to_string(),
                        );
                        ctx.short_circuit = true;
                        Ok(ctx)
                    }
                }
            }
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::sqlite_user_preference_store::SqliteUserPreferenceStore;
    use crate::domain::entities::message::{Direction, Message};
    use crate::domain::value_objects::route_key::{ChannelId, ConversationType, RouteKey};
    use crate::infrastructure::config::AppConfig;
    use crate::infrastructure::db::{init_db, DbPool};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_store() -> Arc<dyn UserPreferenceStore> {
        Arc::new(SqliteUserPreferenceStore::new(DbPool::new(
            init_db(":memory:").unwrap(),
        )))
    }

    fn make_test_mw(store: Arc<dyn UserPreferenceStore>) -> AgentCommandMiddleware {
        let mut aliases = HashMap::new();
        aliases.insert("claude_code".to_string(), vec!["cc".to_string(), "claude".to_string()]);
        aliases.insert("codex".to_string(), vec!["cx".to_string(), "codex".to_string()]);
        aliases.insert("openclaw".to_string(), vec!["oc".to_string(), "openclaw".to_string()]);
        aliases.insert("hermes".to_string(), vec!["h".to_string(), "hermes".to_string()]);
        let config = AgentConfig { enable_user_preferences: true, aliases };
        AgentCommandMiddleware::new(store, config)
    }

    fn snapshot(rk: &RouteKey) -> crate::domain::value_objects::ConversationSnapshot {
        crate::domain::value_objects::ConversationSnapshot {
            route_key: rk.clone(), conversation_id: "c1".into(), peer_id: "p1".into(),
            conversation_type: ConversationType::Direct,
            message_count: 0, participants: vec![], last_active_secs: 0,
        }
    }

    #[tokio::test]
    async fn test_parse_switch_command() {
        let store = make_store();
        let store_verify = store.clone();
        let mw = make_test_mw(store);
        let rk = RouteKey::new(ChannelId::new("wechat"), "conv1", "user1", ConversationType::Direct);
        let ctx = PipelineContext {
            message: Message { id: "m1".into(), route_key: rk.clone(), sequence: None, timestamp_ms: 1000, direction: Direction::Inbound, content: MessageContent::Text("cc".into()), audit_mark: None },
            conversation: snapshot(&rk), config: AppConfig::default(),
            ai_response: None, short_circuit: false, user_agent_selection: None,
        };
        let result = mw.process(ctx).await.unwrap();
        assert!(result.short_circuit);
        assert!(result.ai_response.as_ref().unwrap().contains("claude_code"));
        let saved = agent_preferences::get_user_agent_via_port(store_verify.as_ref(), "wechat", "default", "user1").unwrap();
        assert_eq!(saved, Some("claude_code".to_string()));
    }

    #[tokio::test]
    async fn test_query_command_no_preference() {
        let mw = make_test_mw(make_store());
        let rk = RouteKey::new(ChannelId::new("wechat"), "conv1", "user1", ConversationType::Direct);
        let ctx = PipelineContext {
            message: Message { id: "m1".into(), route_key: rk.clone(), sequence: None, timestamp_ms: 1000, direction: Direction::Inbound, content: MessageContent::Text("/agent".into()), audit_mark: None },
            conversation: snapshot(&rk), config: AppConfig::default(),
            ai_response: None, short_circuit: false, user_agent_selection: None,
        };
        let result = mw.process(ctx).await.unwrap();
        assert!(result.short_circuit);
        assert!(result.ai_response.as_ref().unwrap().contains("未选择"));
    }

    #[tokio::test]
    async fn test_regular_message_no_preference() {
        let mw = make_test_mw(make_store());
        let rk = RouteKey::new(ChannelId::new("wechat"), "conv1", "user1", ConversationType::Direct);
        let ctx = PipelineContext {
            message: Message { id: "m1".into(), route_key: rk.clone(), sequence: None, timestamp_ms: 1000, direction: Direction::Inbound, content: MessageContent::Text("hello".into()), audit_mark: None },
            conversation: snapshot(&rk), config: AppConfig::default(),
            ai_response: None, short_circuit: false, user_agent_selection: None,
        };
        let result = mw.process(ctx).await.unwrap();
        assert!(result.short_circuit);
        assert!(result.ai_response.as_ref().unwrap().contains("请先选择"));
    }

    #[tokio::test]
    async fn test_regular_message_with_preference() {
        let store = make_store();
        agent_preferences::set_user_agent_via_port(store.as_ref(), "wechat", "default", "user1", "claude_code").unwrap();
        let mw = make_test_mw(store);
        let rk = RouteKey::new(ChannelId::new("wechat"), "conv1", "user1", ConversationType::Direct);
        let ctx = PipelineContext {
            message: Message { id: "m1".into(), route_key: rk.clone(), sequence: None, timestamp_ms: 1000, direction: Direction::Inbound, content: MessageContent::Text("hello".into()), audit_mark: None },
            conversation: snapshot(&rk), config: AppConfig::default(),
            ai_response: None, short_circuit: false, user_agent_selection: None,
        };
        let result = mw.process(ctx).await.unwrap();
        assert!(!result.short_circuit);
        assert_eq!(result.user_agent_selection.as_ref().unwrap(), "claude_code");
    }

    #[tokio::test]
    async fn test_switch_and_process() {
        let store = make_store();
        let store_verify = store.clone();
        let mw = make_test_mw(store);
        let rk = RouteKey::new(ChannelId::new("wechat"), "conv1", "user1", ConversationType::Direct);
        let ctx = PipelineContext {
            message: Message { id: "m1".into(), route_key: rk.clone(), sequence: None, timestamp_ms: 1000, direction: Direction::Inbound, content: MessageContent::Text("cc 帮我总结一下".into()), audit_mark: None },
            conversation: snapshot(&rk), config: AppConfig::default(),
            ai_response: None, short_circuit: false, user_agent_selection: None,
        };
        let result = mw.process(ctx).await.unwrap();
        assert!(!result.short_circuit);
        assert_eq!(result.user_agent_selection.as_ref().unwrap(), "claude_code");
        if let MessageContent::Text(t) = &result.message.content {
            assert_eq!(t, "帮我总结一下");
        } else {
            panic!("Expected text content");
        }
        let saved = agent_preferences::get_user_agent_via_port(store_verify.as_ref(), "wechat", "default", "user1").unwrap();
        assert_eq!(saved, Some("claude_code".to_string()));
    }
}
