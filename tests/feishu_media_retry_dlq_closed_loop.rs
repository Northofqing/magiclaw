//! Closed-loop test for Feishu media send failure -> retry -> dead letter -> replay.
//!
//! This test uses the real channel registry path and outbox worker:
//!   outbox(pending) -> sender(feishu channel) -> retrying -> dead_letter -> replay -> outbox(pending)

use std::sync::Arc;

use magiclaw::application::dlq_manager;
use magiclaw::application::outbox_worker::{self, OutboxMessageSender};
use magiclaw::application::send_message;
use magiclaw::domain::entities::message::MessageContent;
use magiclaw::domain::ports::audit_sink::AuditSink;
use magiclaw::domain::ports::dead_letter_repo::DeadLetterRepo;
use magiclaw::domain::ports::outbox_repo::OutboxRepo;
use magiclaw::domain::storage::outbox::{OutboxEntry, RetryConfig};
use magiclaw::domain::value_objects::route_key::{ConversationType, RouteKey};
use magiclaw::infrastructure::config::AppConfig;
use magiclaw::infrastructure::runtime::AppRuntime;
use async_trait::async_trait;

struct NoopAudit;
impl AuditSink for NoopAudit {
    fn record(&self, _route_key: Option<&str>, _action: &str, _detail: &str) {}
}

struct RegistrySender {
    registry: Arc<magiclaw::channels::registry::ChannelRegistry>,
}

#[async_trait]
impl OutboxMessageSender for RegistrySender {
    async fn send(&self, entry: &OutboxEntry) -> Result<(), String> {
        let route_key: RouteKey =
            serde_json::from_str(&entry.route_key).map_err(|e| format!("invalid route_key: {}", e))?;
        let payload: MessageContent =
            serde_json::from_str(&entry.payload).map_err(|e| format!("invalid payload: {}", e))?;

        self.registry
            .send_via(&route_key.channel, &route_key.peer_id, &payload)
            .await
            .map(|_| ())
    }
}

#[tokio::test]
#[allow(clippy::field_reassign_with_default)]
async fn feishu_media_failure_goes_retry_then_dlq_and_can_replay() {
    let db_path = std::env::temp_dir().join(format!("magiclaw_feishu_dlq_{}.db", uuid::Uuid::new_v4()));

    let mut config = AppConfig::default();
    config.db_path = db_path.to_string_lossy().to_string();
    config.feishu.enabled = true;
    config.feishu.tenant_access_token = "dummy-token".to_string();
    // Keep default base_url; failure is triggered before network by nonexistent local file.

    let runtime = AppRuntime::new(config).unwrap();
    let sender = RegistrySender {
        registry: runtime.channel_registry.clone(),
    };
    let audit = NoopAudit;

    let message_id = send_message::submit_outbound_for_delivery(
        runtime.outbox_repo.as_ref(),
        "feishu",
        "chat_dlq_1",
        "open_user_1",
        ConversationType::Direct,
        MessageContent::File {
            url: "file:///definitely/not/exist/magiclaw_missing_media.bin".to_string(),
            name: "missing.bin".to_string(),
            size: 12,
        },
    )
    .unwrap();

    let retry = RetryConfig {
        max_retries: 2,
        base_backoff_ms: 0,
        max_backoff_ms: 0,
        jitter: 0.0,
    };

    // 1st attempt from pending -> should become retrying (count=1)
    outbox_worker::process_pending(
        runtime.outbox_repo.as_ref(),
        runtime.dead_letter_repo.as_ref(),
        &retry,
        &sender,
        &audit,
        64,
    )
    .await;

    // 2nd attempt from retrying -> should become dead_letter (count=2 reaches max)
    outbox_worker::process_retries(
        runtime.outbox_repo.as_ref(),
        runtime.dead_letter_repo.as_ref(),
        &retry,
        &sender,
        &audit,
        64,
    )
    .await;

    let dlq_entries = runtime.dead_letter_repo.list(10).unwrap();
    assert!(
        dlq_entries.iter().any(|e| e.id == message_id),
        "message should be moved to dead letter"
    );

    // Replay should move the same id back to outbox pending.
    dlq_manager::replay_dead_letter(runtime.dead_letter_repo.as_ref(), &message_id).unwrap();

    let dlq_after_replay = runtime.dead_letter_repo.list(10).unwrap();
    assert!(
        !dlq_after_replay.iter().any(|e| e.id == message_id),
        "message should be removed from dead letter after replay"
    );

    let pending_after_replay = runtime.outbox_repo.fetch_pending(20).unwrap();
    assert!(
        pending_after_replay.iter().any(|e| e.id == message_id),
        "replayed message should return to outbox pending"
    );

    let _ = std::fs::remove_file(db_path);
}
