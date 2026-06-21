//! System-level closed-loop test for Phase 5 resilience (red line 2.5).
//!
//! Verifies the ResilienceGate is wired into the live pipeline AI path: a
//! failing AI backend behind a ResilientAiBackend trips its circuit breaker
//! after the failure threshold; subsequent messages are rejected at the gate
//! (the inner backend is no longer called), while the AI middleware still
//! degrades gracefully so every inbound message yields an outbound reply in the
//! Outbox.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use magiclaw::adapters::conversation_store::ConversationStore;
use magiclaw::adapters::sqlite_outbox::SqliteOutboxRepo;
use magiclaw::core::ai::backend::AiBackend;
use magiclaw::core::ai::resilient::ResilientAiBackend;
use magiclaw::core::pipeline::ai::AiMiddleware;
use magiclaw::core::pipeline::formatter::Formatter;
use magiclaw::core::pipeline::normalize::Normalize;
use magiclaw::core::pipeline::outbox::OutboxStage;
use magiclaw::core::pipeline::Pipeline;
use magiclaw::core::resilience::circuit_breaker::{BreakerConfig, CircuitState};
use magiclaw::core::resilience::gate::ResilienceGate;
use magiclaw::domain::entities::message::{Direction, Message, MessageContent};
use magiclaw::domain::ports::conversation_queue::ConversationQueue;
use magiclaw::domain::ports::outbox_repo::OutboxRepo;
use magiclaw::domain::value_objects::route_key::{ChannelId, ConversationType, RouteKey};
use magiclaw::infrastructure::config::AppConfig;
use magiclaw::infrastructure::db::{init_db, DbPool};

struct CountingFailBackend {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl AiBackend for CountingFailBackend {
    fn name(&self) -> &'static str {
        "counting-fail"
    }
    async fn generate(&self, _input: &str, _context: Option<&str>) -> Result<String, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err("ai down".to_string())
    }
}

#[tokio::test]
async fn failing_ai_trips_breaker_but_pipeline_keeps_producing_replies() {
    let outbox = Arc::new(SqliteOutboxRepo::new(DbPool::new(init_db(":memory:").unwrap())));

    let calls = Arc::new(AtomicUsize::new(0));
    let inner = Arc::new(CountingFailBackend {
        calls: calls.clone(),
    });
    let gate = Arc::new(ResilienceGate::new(
        BreakerConfig {
            failure_threshold: 2,
            timeout: Duration::from_secs(60),
            half_open_max: 1,
        },
        5,
    ));
    let ai_backend: Arc<dyn AiBackend> = Arc::new(ResilientAiBackend::new(inner, gate.clone()));

    let pipeline = Arc::new(
        Pipeline::new()
            .with(Box::new(Normalize))
            .with(Box::new(AiMiddleware::new(ai_backend)))
            .with(Box::new(Formatter))
            .with(Box::new(OutboxStage::new(
                outbox.clone() as Arc<dyn OutboxRepo>,
            ))),
    );

    // Single route key → messages serialize through one worker (deterministic).
    let store = ConversationStore::new(256, 1800, 0, Some(pipeline), AppConfig::default(), None);
    let key = RouteKey::new(
        ChannelId::new("wechat"),
        "conv_res",
        "user_a",
        ConversationType::Direct,
    );

    for seq in 1..=3 {
        let msg = Message {
            id: format!("m{seq}"),
            route_key: key.clone(),
            sequence: Some(seq),
            timestamp_ms: seq * 100,
            direction: Direction::Inbound,
            content: MessageContent::Text(format!("ping{seq}")),
            audit_mark: None,
        };
        store.enqueue(&key, msg).unwrap();
    }

    tokio::time::sleep(Duration::from_millis(80)).await;

    // Breaker opened after 2 failures; the 3rd message was rejected at the gate
    // so the inner backend was called only twice.
    assert_eq!(gate.circuit_state(), CircuitState::Open);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "inner AI backend must stop being called once the breaker is open"
    );

    // Graceful degradation: all three inbound messages still produced a reply.
    let pending = outbox.fetch_pending(10).unwrap();
    assert_eq!(
        pending.len(),
        3,
        "pipeline keeps producing degraded replies despite AI failure"
    );
}
