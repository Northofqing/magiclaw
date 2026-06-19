//! System-level closed-loop test for the audit red line (2.6).
//!
//! Verifies the runtime send-decision path produces immutable `audit_log`
//! rows: a successful send writes `send/sent`; an exhausted-retry send writes
//! `dead_letter`. This covers the wiring of `AuditSink` into the outbox worker
//! (process_pending) against real SQLite-backed repositories.

use async_trait::async_trait;

use aiclaw::adapters::sqlite_audit::SqliteAuditSink;
use aiclaw::adapters::sqlite_dead_letter::SqliteDeadLetterRepo;
use aiclaw::adapters::sqlite_outbox::SqliteOutboxRepo;
use aiclaw::application::audit::query_audit_logs;
use aiclaw::application::outbox_worker::{self, OutboxMessageSender};
use aiclaw::domain::ports::outbox_repo::OutboxRepo;
use aiclaw::domain::storage::outbox::{OutboxEntry, RetryConfig};
use aiclaw::infrastructure::db::{init_db, DbPool};

struct AlwaysOk;

#[async_trait]
impl OutboxMessageSender for AlwaysOk {
    async fn send(&self, _entry: &OutboxEntry) -> Result<(), String> {
        Ok(())
    }
}

struct AlwaysFail;

#[async_trait]
impl OutboxMessageSender for AlwaysFail {
    async fn send(&self, _entry: &OutboxEntry) -> Result<(), String> {
        Err("channel unavailable".to_string())
    }
}

fn shared_pool() -> DbPool {
    DbPool::new(init_db(":memory:").expect("init db"))
}

#[tokio::test]
async fn successful_send_writes_sent_audit() {
    let db = shared_pool();
    let outbox = SqliteOutboxRepo::new(db.clone());
    let dlq = SqliteDeadLetterRepo::new(db.clone());
    let audit = SqliteAuditSink::new(db.clone());

    let entry = OutboxEntry::new("msg-ok", "wechat/conv-ok", "{}", 1700000000);
    outbox.insert(&entry).unwrap();

    let retry = RetryConfig::default();
    outbox_worker::process_pending(&outbox, &dlq, &retry, &AlwaysOk, &audit, 16).await;

    let logs = query_audit_logs(&db, Some("wechat/conv-ok"), 16).unwrap();
    assert_eq!(logs.len(), 1, "expected one audit row");
    assert_eq!(logs[0].action, "send");
    assert_eq!(logs[0].result, "sent");
}

#[tokio::test]
async fn exhausted_retry_writes_dead_letter_audit() {
    let db = shared_pool();
    let outbox = SqliteOutboxRepo::new(db.clone());
    let dlq = SqliteDeadLetterRepo::new(db.clone());
    let audit = SqliteAuditSink::new(db.clone());

    let entry = OutboxEntry::new("msg-fail", "wechat/conv-fail", "{}", 1700000000);
    outbox.insert(&entry).unwrap();

    // max_retries = 1 → first failure (new_count = 1) goes straight to dead letter.
    let retry = RetryConfig {
        max_retries: 1,
        ..RetryConfig::default()
    };
    outbox_worker::process_pending(&outbox, &dlq, &retry, &AlwaysFail, &audit, 16).await;

    let logs = query_audit_logs(&db, Some("wechat/conv-fail"), 16).unwrap();
    assert_eq!(logs.len(), 1, "expected one audit row");
    assert_eq!(logs[0].action, "dead_letter");
    assert!(logs[0].result.contains("channel unavailable"));
}
