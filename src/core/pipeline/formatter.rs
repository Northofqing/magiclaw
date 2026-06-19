use async_trait::async_trait;

use crate::domain::entities::message::{Direction, MessageContent};

use super::middleware::{Middleware, PipelineContext};

/// Formatter middleware: transforms AI response into an outbound Message.
pub struct Formatter;

#[async_trait]
impl Middleware for Formatter {
    fn name(&self) -> &'static str { "formatter" }

    async fn process(&self, mut ctx: PipelineContext) -> Result<PipelineContext, String> {
        if let Some(response) = ctx.ai_response.take() {
            let responder = if let Some(agent) = ctx.user_agent_selection.as_deref() {
                agent
            } else if ctx.short_circuit {
                "system"
            } else {
                ctx.config.ai.backend.as_str()
            };

            let attributed = add_responder_prefix(response, responder);
            ctx.message.direction = Direction::Outbound;
            ctx.message.content = MessageContent::Text(attributed);
            tracing::debug!(message_id = %ctx.message.id, "formatter: response formatted");
        }
        Ok(ctx)
    }
}

fn add_responder_prefix(mut text: String, responder: &str) -> String {
    // Avoid duplicate prefixes like "[echo] [echo] ...".
    let prefix = format!("[{}] ", responder);
    if text.starts_with(&prefix) {
        return text;
    }
    text.insert_str(0, &prefix);
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::aggregates::conversation::Conversation;
    use crate::domain::entities::message::{Direction, Message};
    use crate::domain::value_objects::route_key::{ChannelId, ConversationType, RouteKey};
    use crate::infrastructure::config::AppConfig;

    fn make_ctx() -> PipelineContext {
        let rk = RouteKey::new(ChannelId::new("wechat"), "c1", "p1", ConversationType::Direct);
        PipelineContext {
            message: Message {
                id: "m1".into(),
                route_key: rk.clone(),
                sequence: None,
                timestamp_ms: 1,
                direction: Direction::Inbound,
                content: MessageContent::Text("hi".into()),
                audit_mark: None,
            },
            conversation: Conversation::new(rk, 200),
            config: AppConfig::default(),
            ai_response: None,
            short_circuit: false,
            user_agent_selection: None,
        }
    }

    #[tokio::test]
    async fn formatter_adds_agent_prefix() {
        let fmt = Formatter;
        let mut ctx = make_ctx();
        ctx.user_agent_selection = Some("claude_code".to_string());
        ctx.ai_response = Some("hello".to_string());

        let out = fmt.process(ctx).await.unwrap();
        match out.message.content {
            MessageContent::Text(t) => assert_eq!(t, "[claude_code] hello"),
            _ => panic!("expected text response"),
        }
    }

    #[tokio::test]
    async fn formatter_adds_system_prefix_for_short_circuit() {
        let fmt = Formatter;
        let mut ctx = make_ctx();
        ctx.short_circuit = true;
        ctx.ai_response = Some("已切换到 claude_code".to_string());

        let out = fmt.process(ctx).await.unwrap();
        match out.message.content {
            MessageContent::Text(t) => assert_eq!(t, "[system] 已切换到 claude_code"),
            _ => panic!("expected text response"),
        }
    }

    #[tokio::test]
    async fn formatter_avoids_double_prefix() {
        let fmt = Formatter;
        let mut ctx = make_ctx();
        ctx.config.ai.backend = "echo".to_string();
        ctx.ai_response = Some("[echo] hello".to_string());

        let out = fmt.process(ctx).await.unwrap();
        match out.message.content {
            MessageContent::Text(t) => assert_eq!(t, "[echo] hello"),
            _ => panic!("expected text response"),
        }
    }
}
