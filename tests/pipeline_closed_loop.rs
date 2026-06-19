//! System-level closed-loop test for Phase 4 (pipeline + pluggable AI).
//!
//! Verifies the full inbound→reply loop: an inbound message flows through the
//! assembled pipeline (Normalize → Permission → AI[echo] → Formatter →
//! OutboxStage) and produces a durable outbound `OutboxEntry(pending)` carrying
//! the AI-generated reply, ready for recoverable delivery.

use std::sync::Arc;
use std::time::Duration;

use aiclaw::adapters::conversation_store::ConversationStore;
use aiclaw::adapters::sqlite_outbox::SqliteOutboxRepo;
use aiclaw::core::ai::backend::AiBackend;
use aiclaw::core::ai::echo::EchoBackend;
use aiclaw::core::pipeline::ai::AiMiddleware;
use aiclaw::core::pipeline::formatter::Formatter;
use aiclaw::core::pipeline::normalize::Normalize;
use aiclaw::core::pipeline::outbox::OutboxStage;
use aiclaw::core::pipeline::permission::Permission;
use aiclaw::core::pipeline::Pipeline;
use aiclaw::domain::entities::message::{Direction, Message, MessageContent};
use aiclaw::domain::ports::conversation_queue::ConversationQueue;
use aiclaw::domain::ports::outbox_repo::OutboxRepo;
use aiclaw::domain::value_objects::route_key::{ChannelId, ConversationType, RouteKey};
use aiclaw::infrastructure::config::AppConfig;
use aiclaw::infrastructure::db::{init_db, DbPool};

#[tokio::test]
async fn inbound_message_produces_outbound_reply_in_outbox() {
    let outbox = Arc::new(SqliteOutboxRepo::new(DbPool::new(init_db(":memory:").unwrap())));

    let ai_backend: Arc<dyn AiBackend> = Arc::new(EchoBackend);
    let pipeline = Arc::new(
        Pipeline::new()
            .with(Box::new(Normalize))
            .with(Box::new(Permission))
            .with(Box::new(AiMiddleware::new(ai_backend)))
            .with(Box::new(Formatter))
            .with(Box::new(OutboxStage::new(
                outbox.clone() as Arc<dyn OutboxRepo>,
            ))),
    );

    let store = ConversationStore::new(
        256,
        1800,
        0, // reorder window 0 → message becomes ready immediately
        Some(pipeline),
        AppConfig::default(),
        None,
    );

    let key = RouteKey::new(
        ChannelId::new("wechat"),
        "conv_pipe",
        "user_a",
        ConversationType::Direct,
    );
    let inbound = Message {
        id: "inbound-1".into(),
        route_key: key.clone(),
        sequence: Some(1),
        timestamp_ms: 100,
        direction: Direction::Inbound,
        content: MessageContent::Text("ping".into()),
        audit_mark: None,
    };

    store.enqueue(&key, inbound).unwrap();

    // Allow the per-route worker to run the pipeline asynchronously.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let pending = outbox.fetch_pending(10).unwrap();
    assert_eq!(pending.len(), 1, "pipeline should queue exactly one reply");

    let entry = &pending[0];
    let content: MessageContent = serde_json::from_str(&entry.payload).unwrap();
    match content {
        MessageContent::Text(t) => {
            assert!(t.contains("ping"), "reply should echo input: {t}");
            assert!(t.contains("[echo]"), "reply should come from echo backend: {t}");
        }
        other => panic!("expected text reply, got {other:?}"),
    }

    // The reply targets the same conversation it came from.
    let reply_key: RouteKey = serde_json::from_str(&entry.route_key).unwrap();
    assert_eq!(reply_key.conversation_id, "conv_pipe");
    assert_eq!(reply_key.peer_id, "user_a");
}
