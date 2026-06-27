use async_trait::async_trait;

use crate::domain::entities::message::Message;
use crate::domain::value_objects::ConversationSnapshot;
use crate::infrastructure::config::AppConfig;

/// Context flowing through the middleware chain.
pub struct PipelineContext {
    pub message: Message,
    pub conversation: ConversationSnapshot,
    pub config: AppConfig,
    /// AI-generated response, populated by the AI middleware.
    pub ai_response: Option<String>,
    /// Whether to short-circuit the pipeline (skip remaining middleware).
    pub short_circuit: bool,
    /// Current user's selected AI agent name (populated by AgentCommandMiddleware).
    /// If None, will be determined by global config.ai.backend.
    pub user_agent_selection: Option<String>,
}

/// A single middleware step in the processing pipeline.
#[async_trait]
pub trait Middleware: Send + Sync {
    /// Name of this middleware for logging.
    fn name(&self) -> &'static str;

    /// Whether this middleware is terminal (must always run, even on short-circuit).
    /// Terminal middleware like Formatter and OutboxStage are never skipped.
    fn is_terminal(&self) -> bool {
        false
    }

    /// Process the context. Return Ok(ctx) to continue, or set short_circuit to true.
    async fn process(&self, ctx: PipelineContext) -> Result<PipelineContext, String>;
}

/// The pipeline executes a chain of middleware in order.
pub struct Pipeline {
    steps: Vec<Box<dyn Middleware>>,
}

impl Pipeline {
    pub fn new() -> Self {
        Self { steps: Vec::new() }
    }

    pub fn with(mut self, mw: Box<dyn Middleware>) -> Self {
        self.steps.push(mw);
        self
    }

    pub async fn run(&self, ctx: PipelineContext) -> Result<PipelineContext, String> {
        let mut ctx = ctx;
        for step in &self.steps {
            if ctx.short_circuit {
                // Short-circuit means skip business-expensive steps (e.g. AI),
                // but still allow terminal delivery steps to run so prompts/
                // switch confirmations can be sent back to the user.
                if !step.is_terminal() {
                    continue;
                }
            }
            tracing::debug!(middleware = step.name(), message_id = %ctx.message.id, "processing");
            ctx = step.process(ctx).await?;
        }
        Ok(ctx)
    }
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestMw {
        name: &'static str,
        append: String,
    }

    #[async_trait]
    impl Middleware for TestMw {
        fn name(&self) -> &'static str { self.name }
        async fn process(&self, mut ctx: PipelineContext) -> Result<PipelineContext, String> {
            let existing = ctx.ai_response.unwrap_or_default();
            ctx.ai_response = Some(format!("{}{}", existing, self.append));
            Ok(ctx)
        }
    }

    #[tokio::test]
    async fn pipeline_runs_in_order() {
        let p = Pipeline::new()
            .with(Box::new(TestMw { name: "a", append: "A".into() }))
            .with(Box::new(TestMw { name: "b", append: "B".into() }));

        let ctx = PipelineContext {
            message: crate::domain::entities::message::Message {
                id: "m1".into(),
                route_key: crate::domain::value_objects::route_key::RouteKey::new(
                    crate::domain::value_objects::route_key::ChannelId::new("wechat"),
                    "c1", "p1", crate::domain::value_objects::route_key::ConversationType::Direct,
                ),
                sequence: None, timestamp_ms: 1,
                direction: crate::domain::entities::message::Direction::Inbound,
                content: crate::domain::entities::message::MessageContent::Text("hi".into()),
                audit_mark: None,
            },
            conversation: crate::domain::value_objects::ConversationSnapshot {
                route_key: crate::domain::value_objects::route_key::RouteKey::new(
                    crate::domain::value_objects::route_key::ChannelId::new("wechat"),
                    "c1", "p1", crate::domain::value_objects::route_key::ConversationType::Direct,
                ),
                conversation_id: "c1".into(),
                peer_id: "p1".into(),
                conversation_type: crate::domain::value_objects::route_key::ConversationType::Direct,
                participants: vec![],
                message_count: 0,
                last_active_secs: 0,
            },
            config: AppConfig::default(),
            ai_response: None,
            short_circuit: false,
            user_agent_selection: None,
        };

        let result = p.run(ctx).await.unwrap();
        assert_eq!(result.ai_response, Some("AB".into()));
    }
}
