use async_trait::async_trait;
use std::sync::Arc;

use crate::domain::entities::message::Direction;
use crate::domain::ports::outbox_repo::OutboxRepo;
use crate::domain::storage::outbox::OutboxEntry;

use super::middleware::{Middleware, PipelineContext};

/// Outbox stage: the terminal pipeline step that submits a formatted outbound
/// reply into the durable Outbox for recoverable delivery.
///
/// This closes the inbound→reply loop: once the Formatter has turned the AI
/// response into an outbound `Message`, this stage persists it as an
/// `OutboxEntry(pending)` so the outbox worker can deliver it through the
/// channel registry. Storage failures are logged but never abort the
/// conversation worker (the inbound side is already persisted in the Inbox).
pub struct OutboxStage {
    outbox: Arc<dyn OutboxRepo>,
}

impl OutboxStage {
    pub fn new(outbox: Arc<dyn OutboxRepo>) -> Self {
        Self { outbox }
    }
}

#[async_trait]
impl Middleware for OutboxStage {
    fn name(&self) -> &'static str {
        "outbox"
    }

    async fn process(&self, ctx: PipelineContext) -> Result<PipelineContext, String> {
        // Only submit when the Formatter produced an outbound reply.
        if ctx.message.direction != Direction::Outbound {
            return Ok(ctx);
        }

        let route_key_json = match serde_json::to_string(&ctx.message.route_key) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "outbox stage: failed to serialize route_key");
                return Ok(ctx);
            }
        };
        let payload_json = match serde_json::to_string(&ctx.message.content) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "outbox stage: failed to serialize payload");
                return Ok(ctx);
            }
        };

        let entry = OutboxEntry::new(
            &ctx.message.id,
            route_key_json,
            payload_json,
            chrono::Utc::now().timestamp(),
        );

        if let Err(e) = self.outbox.insert(&entry) {
            tracing::error!(error = %e, message_id = %ctx.message.id, "outbox stage: failed to persist reply");
        } else {
            tracing::debug!(message_id = %ctx.message.id, "outbox stage: reply queued for delivery");
        }

        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::sqlite_outbox::SqliteOutboxRepo;
    use crate::domain::aggregates::conversation::Conversation;
    use crate::domain::entities::message::{Message, MessageContent};
    use crate::domain::value_objects::route_key::{ChannelId, ConversationType, RouteKey};
    use crate::infrastructure::config::AppConfig;
    use crate::infrastructure::db::{init_db, DbPool};

    fn make_ctx(direction: Direction) -> PipelineContext {
        let rk = RouteKey::new(ChannelId::new("wechat"), "c1", "p1", ConversationType::Direct);
        PipelineContext {
            message: Message {
                id: "m1".into(),
                route_key: rk.clone(),
                sequence: None,
                timestamp_ms: 1,
                direction,
                content: MessageContent::Text("reply".into()),
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
    async fn outbound_reply_is_persisted() {
        let outbox = Arc::new(SqliteOutboxRepo::new(DbPool::new(init_db(":memory:").unwrap())));
        let stage = OutboxStage::new(outbox.clone());
        stage.process(make_ctx(Direction::Outbound)).await.unwrap();
        assert_eq!(outbox.fetch_pending(10).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn inbound_message_is_not_persisted() {
        let outbox = Arc::new(SqliteOutboxRepo::new(DbPool::new(init_db(":memory:").unwrap())));
        let stage = OutboxStage::new(outbox.clone());
        stage.process(make_ctx(Direction::Inbound)).await.unwrap();
        assert_eq!(outbox.fetch_pending(10).unwrap().len(), 0);
    }
}
