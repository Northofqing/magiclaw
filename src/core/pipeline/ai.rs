use async_trait::async_trait;
use crate::domain::error::PipelineError;
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::ai::backend::AiBackend;
use crate::domain::entities::message::MessageContent;
use crate::domain::ports::audit_sink::{AuditSink, NoopAuditSink};

use super::middleware::{Middleware, PipelineContext};

/// AI middleware: calls the configured AI backend to generate a response.
/// Supports multiple backends and user agent selection (Phase A+).
pub struct AiMiddleware {
    default_backend: Arc<dyn AiBackend>,
    backend_map: HashMap<String, Arc<dyn AiBackend>>,
    audit: Arc<dyn AuditSink>,
}

impl AiMiddleware {
    pub fn new(backend: Arc<dyn AiBackend>) -> Self {
        Self { default_backend: backend, backend_map: HashMap::new(), audit: Arc::new(NoopAuditSink) }
    }

    /// Construct with an audit sink so every AI decision is recorded
    /// (red line 2.6: key send decisions are audit-logged).
    pub fn with_audit(backend: Arc<dyn AiBackend>, audit: Arc<dyn AuditSink>) -> Self {
        Self { default_backend: backend, backend_map: HashMap::new(), audit }
    }

    /// Construct with a backend map for Phase A+ user agent selection.
    /// Each key maps to a different AI backend that users can switch to.
    pub fn with_backends(
        default_backend: Arc<dyn AiBackend>,
        backend_map: HashMap<String, Arc<dyn AiBackend>>,
        audit: Arc<dyn AuditSink>,
    ) -> Self {
        Self { default_backend, backend_map, audit }
    }
}

#[async_trait]
impl Middleware for AiMiddleware {
    fn name(&self) -> &'static str { "ai" }

    async fn process(&self, mut ctx: PipelineContext) -> Result<PipelineContext, PipelineError> {
        let input = match &ctx.message.content {
            MessageContent::Text(t) => t.clone(),
            _ => return Ok(ctx), // skip non-text messages
        };

        // Phase A+: select backend based on user_agent_selection if set, otherwise use default
        let backend = if let Some(ref agent_name) = ctx.user_agent_selection {
            self.backend_map.get(agent_name).unwrap_or(&self.default_backend)
        } else {
            &self.default_backend
        };

        // Build context from conversation state
        let context_info = format!(
            "channel={} conversation={} participants={:?}",
            ctx.message.route_key.channel,
            ctx.message.route_key.conversation_id,
            ctx.conversation.participants,
        );

        // Serialized RouteKey for the audit trail (red line 2.6).
        let route_key_str = serde_json::to_string(&ctx.message.route_key).ok();

        match backend.generate(&input, Some(&context_info)).await {
            Ok(response) => {
                tracing::info!(
                    backend = backend.name(),
                    message_id = %ctx.message.id,
                    response_len = response.len(),
                    "AI response generated"
                );
                self.audit.record(
                    route_key_str.as_deref(),
                    "ai_generate",
                    &format!("ok backend={} len={}", backend.name(), response.len()),
                );
                ctx.ai_response = Some(response);
            }
            Err(e) => {
                tracing::error!(backend = backend.name(), error = %e, "AI backend error");
                self.audit.record(
                    route_key_str.as_deref(),
                    "ai_generate",
                    &format!("degraded backend={} error={}", backend.name(), e),
                );
                // Degrade gracefully: echo the input back
                ctx.ai_response = Some(format!("[ai error: {}] {}", e, input));
            }
        }

        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ai::echo::EchoBackend;
    use crate::domain::entities::message::{Direction, Message};
    use crate::domain::value_objects::route_key::{ChannelId, ConversationType, RouteKey};
    use crate::infrastructure::config::AppConfig;

    #[tokio::test]
    async fn ai_middleware_generates_response() {
        let ai = AiMiddleware::new(Arc::new(EchoBackend));
        let rk = RouteKey::new(ChannelId::new("wechat"), "c1", "p1", ConversationType::Direct);
        let ctx = PipelineContext {
            message: Message {
                id: "m1".into(), route_key: rk.clone(), sequence: None, timestamp_ms: 1,
                direction: Direction::Inbound, content: MessageContent::Text("hi".into()), audit_mark: None,
            },
            conversation: crate::domain::value_objects::ConversationSnapshot { route_key: rk.clone(), conversation_id: "c1".into(), peer_id: "p1".into(), conversation_type: crate::domain::value_objects::route_key::ConversationType::Direct, message_count: 0, participants: vec![], last_active_secs: 0 },
            config: AppConfig::default(),
            ai_response: None,
            short_circuit: false,
            user_agent_selection: None,
        };

        let result = ai.process(ctx).await.unwrap();
        assert!(result.ai_response.is_some());
        assert!(result.ai_response.unwrap().contains("hi"));
    }
}
