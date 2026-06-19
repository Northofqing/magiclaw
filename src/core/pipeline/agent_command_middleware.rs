/// Middleware for handling user agent preference switching via commands.
use async_trait::async_trait;

use crate::application::agent_preferences;
use crate::domain::entities::message::MessageContent;
use crate::infrastructure::config::{default_agent_aliases, AgentConfig};
use crate::infrastructure::db::DbPool;

use super::agent_command::AgentCommandParser;
use super::middleware::{Middleware, PipelineContext};

/// AgentCommand middleware: intercepts and handles agent switching commands.
/// This runs early in the pipeline before any message processing.
pub struct AgentCommandMiddleware {
    db: DbPool,
    parser: AgentCommandParser,
    config: AgentConfig,
}

impl AgentCommandMiddleware {
    pub fn new(db: DbPool, config: AgentConfig) -> Self {
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
        Self { db, parser, config }
    }
}

#[async_trait]
impl Middleware for AgentCommandMiddleware {
    fn name(&self) -> &'static str { "agent_command" }

    async fn process(&self, mut ctx: PipelineContext) -> Result<PipelineContext, String> {
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
                agent_preferences::set_user_agent(&self.db, channel, account_scope, peer_id, &agent_name)?;

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
                match agent_preferences::get_user_agent(&self.db, channel, account_scope, peer_id)? {
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
                agent_preferences::set_user_agent(&self.db, channel, account_scope, peer_id, &agent_name)?;

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

                match agent_preferences::get_user_agent(&self.db, channel, account_scope, peer_id)? {
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
    use crate::domain::entities::message::{Direction, Message};
    use crate::domain::value_objects::route_key::{ChannelId, ConversationType, RouteKey};
    use crate::domain::aggregates::conversation::Conversation;
    use crate::infrastructure::config::AppConfig;
    use std::collections::HashMap;

    fn make_test_mw(db: DbPool) -> AgentCommandMiddleware {
        let mut aliases = HashMap::new();
        aliases.insert(
            "claude_code".to_string(),
            vec!["cc".to_string(), "claude".to_string()],
        );
        aliases.insert(
            "codex".to_string(),
            vec!["cx".to_string(), "codex".to_string()],
        );

        let config = AgentConfig {
            enable_user_preferences: true,
            aliases,
        };

        AgentCommandMiddleware::new(db, config)
    }

    #[tokio::test]
    async fn test_parse_switch_command() {
        let conn = crate::infrastructure::db::init_db(":memory:").unwrap();
        let db = DbPool::new(conn);
        let mw = make_test_mw(db.clone());

        let rk = RouteKey::new(
            ChannelId::new("wechat"),
            "conv1",
            "user1",
            ConversationType::Direct,
        );
        let msg = Message {
            id: "m1".into(),
            route_key: rk.clone(),
            sequence: None,
            timestamp_ms: 1000,
            direction: Direction::Inbound,
            content: MessageContent::Text("cc".into()),
            audit_mark: None,
        };

        let ctx = PipelineContext {
            message: msg,
            conversation: Conversation::new(rk, 200),
            config: AppConfig::default(),
            ai_response: None,
            short_circuit: false,
            user_agent_selection: None,
        };

        let result = mw.process(ctx).await.unwrap();
        assert!(result.short_circuit);
        assert!(result.ai_response.is_some());
        assert!(result.ai_response.as_ref().unwrap().contains("claude_code"));

        // Verify preference was saved
        let saved = agent_preferences::get_user_agent(&db, "wechat", "default", "user1")
            .unwrap();
        assert_eq!(saved, Some("claude_code".to_string()));
    }

    #[tokio::test]
    async fn test_query_command_no_preference() {
        let conn = crate::infrastructure::db::init_db(":memory:").unwrap();
        let db = DbPool::new(conn);
        let mw = make_test_mw(db);

        let rk = RouteKey::new(
            ChannelId::new("wechat"),
            "conv1",
            "user1",
            ConversationType::Direct,
        );
        let msg = Message {
            id: "m1".into(),
            route_key: rk.clone(),
            sequence: None,
            timestamp_ms: 1000,
            direction: Direction::Inbound,
            content: MessageContent::Text("/agent".into()),
            audit_mark: None,
        };

        let ctx = PipelineContext {
            message: msg,
            conversation: Conversation::new(rk, 200),
            config: AppConfig::default(),
            ai_response: None,
            short_circuit: false,
            user_agent_selection: None,
        };

        let result = mw.process(ctx).await.unwrap();
        assert!(result.short_circuit);
        assert!(result
            .ai_response
            .as_ref()
            .unwrap()
            .contains("还未选择"));
    }

    #[tokio::test]
    async fn test_regular_message_no_preference() {
        let conn = crate::infrastructure::db::init_db(":memory:").unwrap();
        let db = DbPool::new(conn);
        let mw = make_test_mw(db);

        let rk = RouteKey::new(
            ChannelId::new("wechat"),
            "conv1",
            "user1",
            ConversationType::Direct,
        );
        let msg = Message {
            id: "m1".into(),
            route_key: rk.clone(),
            sequence: None,
            timestamp_ms: 1000,
            direction: Direction::Inbound,
            content: MessageContent::Text("hello".into()),
            audit_mark: None,
        };

        let ctx = PipelineContext {
            message: msg,
            conversation: Conversation::new(rk, 200),
            config: AppConfig::default(),
            ai_response: None,
            short_circuit: false,
            user_agent_selection: None,
        };

        let result = mw.process(ctx).await.unwrap();
        assert!(result.short_circuit); // Should prompt for agent selection
        assert!(result.ai_response.is_some());
        assert!(result.ai_response.as_ref().unwrap().contains("请先选择"));
    }

    #[tokio::test]
    async fn test_regular_message_with_preference() {
        let conn = crate::infrastructure::db::init_db(":memory:").unwrap();
        let db = DbPool::new(conn);

        // Set a preference first
        agent_preferences::set_user_agent(&db, "wechat", "default", "user1", "claude_code").unwrap();

        let mw = make_test_mw(db);

        let rk = RouteKey::new(
            ChannelId::new("wechat"),
            "conv1",
            "user1",
            ConversationType::Direct,
        );
        let msg = Message {
            id: "m1".into(),
            route_key: rk.clone(),
            sequence: None,
            timestamp_ms: 1000,
            direction: Direction::Inbound,
            content: MessageContent::Text("hello".into()),
            audit_mark: None,
        };

        let ctx = PipelineContext {
            message: msg,
            conversation: Conversation::new(rk, 200),
            config: AppConfig::default(),
            ai_response: None,
            short_circuit: false,
            user_agent_selection: None,
        };

        let result = mw.process(ctx).await.unwrap();
        assert!(!result.short_circuit); // Should continue to next middleware
        assert!(result.user_agent_selection.is_some());
        assert_eq!(result.user_agent_selection.as_ref().unwrap(), "claude_code");
    }

    #[tokio::test]
    async fn test_switch_and_process() {
        let conn = crate::infrastructure::db::init_db(":memory:").unwrap();
        let db = DbPool::new(conn);
        let mw = make_test_mw(db.clone());

        let rk = RouteKey::new(
            ChannelId::new("wechat"),
            "conv1",
            "user1",
            ConversationType::Direct,
        );
        let msg = Message {
            id: "m1".into(),
            route_key: rk.clone(),
            sequence: None,
            timestamp_ms: 1000,
            direction: Direction::Inbound,
            content: MessageContent::Text("cc 帮我总结一下".into()),
            audit_mark: None,
        };

        let ctx = PipelineContext {
            message: msg,
            conversation: Conversation::new(rk, 200),
            config: AppConfig::default(),
            ai_response: None,
            short_circuit: false,
            user_agent_selection: None,
        };

        let result = mw.process(ctx).await.unwrap();
        assert!(!result.short_circuit); // Should continue to process
        assert_eq!(result.user_agent_selection.as_ref().unwrap(), "claude_code");
        
        // Message content should be updated
        if let MessageContent::Text(t) = &result.message.content {
            assert_eq!(t, "帮我总结一下");
        } else {
            panic!("Expected text content");
        }

        // Preference should be saved
        let saved =
            agent_preferences::get_user_agent(&db, "wechat", "default", "user1").unwrap();
        assert_eq!(saved, Some("claude_code".to_string()));
    }
}
