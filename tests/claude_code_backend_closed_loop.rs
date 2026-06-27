//! System-level closed-loop test for the `claude_code` AI backend.
//!
//! Verifies the full inbound→reply loop with the local-CLI backend: an inbound
//! message flows through the assembled pipeline
//! (Normalize → Permission → AI[claude_code] → Formatter → OutboxStage) and
//! produces a durable outbound `OutboxEntry(pending)` carrying the CLI's reply.
//! A deterministic stub `claude` script keeps the test hermetic (no network).
//!
//! Also covers graceful degradation (missing binary → echo fallback, no panic)
//! and the audit trail (red line 2.6: AI decisions are recorded).

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use magiclaw::adapters::conversation_store::ConversationStore;
use magiclaw::adapters::sqlite_audit::SqliteAuditSink;
use magiclaw::adapters::sqlite_audit_query::SqliteAuditQuery;
use magiclaw::adapters::sqlite_outbox::SqliteOutboxRepo;
use magiclaw::application::audit::query_audit_logs;
use magiclaw::core::ai::backend::AiBackend;
use magiclaw::core::ai::claude_code::ClaudeCodeBackend;
use magiclaw::core::pipeline::ai::AiMiddleware;
use magiclaw::core::pipeline::formatter::Formatter;
use magiclaw::core::pipeline::normalize::Normalize;
use magiclaw::core::pipeline::outbox::OutboxStage;
use magiclaw::core::pipeline::permission::Permission;
use magiclaw::core::pipeline::Pipeline;
use magiclaw::domain::entities::message::{Direction, Message, MessageContent};
use magiclaw::domain::ports::audit_sink::AuditSink;
use magiclaw::domain::ports::conversation_queue::ConversationQueue;
use magiclaw::domain::ports::outbox_repo::OutboxRepo;
use magiclaw::domain::value_objects::route_key::{ChannelId, ConversationType, RouteKey};
use magiclaw::infrastructure::config::{AppConfig, ClaudeCodeConfig};
use magiclaw::infrastructure::db::{init_db, DbPool};

fn write_stub(name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "magiclaw_cc_closed_loop_{}_{}",
        std::process::id(),
        name
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("claude");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    let mut perms = f.metadata().unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

fn build_pipeline(
    backend: Arc<dyn AiBackend>,
    audit: Arc<dyn AuditSink>,
    outbox: Arc<dyn OutboxRepo>,
) -> Arc<Pipeline> {
    Arc::new(
        Pipeline::new()
            .with(Box::new(Normalize))
            .with(Box::new(Permission))
            .with(Box::new(AiMiddleware::with_audit(backend, audit)))
            .with(Box::new(Formatter))
            .with(Box::new(OutboxStage::new(outbox))),
    )
}

fn inbound(key: &RouteKey, text: &str) -> Message {
    Message {
        id: "cc-inbound-1".into(),
        route_key: key.clone(),
        sequence: Some(1),
        timestamp_ms: 100,
        direction: Direction::Inbound,
        content: MessageContent::Text(text.into()),
        audit_mark: None,
    }
}

#[tokio::test]
async fn claude_code_backend_reply_reaches_outbox_with_audit() {
    let stub = write_stub(
        "ok",
        "#!/bin/sh\nprintf '%s' '{\"type\":\"result\",\"is_error\":false,\"result\":\"PONG-FROM-STUB\"}'\n",
    );

    let pool = DbPool::new(init_db(":memory:").unwrap());
    let outbox = Arc::new(SqliteOutboxRepo::new(pool.clone()));
    let audit = Arc::new(SqliteAuditSink::new(pool.clone()));

    let backend: Arc<dyn AiBackend> = Arc::new(ClaudeCodeBackend::new(ClaudeCodeConfig {
        binary_path: stub.to_string_lossy().into_owned(),
        timeout_secs: 10,
        max_output_bytes: 16_384,
        // No restricted-mode args needed for the stub; the real config injects
        // `--permission-mode plan`.
        extra_args: vec![],
    }));

    let pipeline = build_pipeline(
        backend,
        audit.clone() as Arc<dyn AuditSink>,
        outbox.clone() as Arc<dyn OutboxRepo>,
    );
    let mut app_config = AppConfig::default();
    app_config.ai.backend = "claude_code".to_string();
    let store = ConversationStore::new(256, 1800, 0, Some(pipeline), app_config, None);

    let key = RouteKey::new(
        ChannelId::new("wechat"),
        "conv_cc",
        "user_cc",
        ConversationType::Direct,
    );
    store.enqueue(&key, inbound(&key, "ping")).unwrap();

    // Allow the per-route worker to run the pipeline (spawns the stub process).
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let pending = outbox.fetch_pending(10).unwrap();
    assert_eq!(pending.len(), 1, "pipeline should queue exactly one reply");

    let content: MessageContent = serde_json::from_str(&pending[0].payload).unwrap();
    match content {
        MessageContent::Text(t) => {
            assert!(
                t.contains("PONG-FROM-STUB"),
                "reply must come from the claude_code backend, got: {t}"
            );
            assert!(!t.contains("[echo]"), "must not be the echo backend: {t}");
        }
        other => panic!("expected text reply, got {other:?}"),
    }

    // Red line 2.6: the AI decision is audit-logged.
    let rk = serde_json::to_string(&key).unwrap();
    let q = SqliteAuditQuery::new(pool.clone());
    let records = query_audit_logs(&q, Some(&rk), 10).unwrap();
    assert!(
        records.iter().any(|r| r.action == "ai_generate" && r.result.contains("claude_code")),
        "expected an ai_generate audit record for claude_code, got: {records:?}"
    );
}

#[tokio::test]
async fn missing_binary_degrades_without_panic() {
    let pool = DbPool::new(init_db(":memory:").unwrap());
    let outbox = Arc::new(SqliteOutboxRepo::new(pool.clone()));
    let audit = Arc::new(SqliteAuditSink::new(pool.clone()));

    let backend: Arc<dyn AiBackend> = Arc::new(ClaudeCodeBackend::new(ClaudeCodeConfig {
        binary_path: "/nonexistent/claude_does_not_exist".into(),
        timeout_secs: 5,
        max_output_bytes: 16_384,
        extra_args: vec![],
    }));

    let pipeline = build_pipeline(
        backend,
        audit as Arc<dyn AuditSink>,
        outbox.clone() as Arc<dyn OutboxRepo>,
    );
    let store = ConversationStore::new(256, 1800, 0, Some(pipeline), AppConfig::default(), None);

    let key = RouteKey::new(
        ChannelId::new("wechat"),
        "conv_cc_fail",
        "user_cc",
        ConversationType::Direct,
    );
    store.enqueue(&key, inbound(&key, "ping")).unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // The pipeline still produces a durable reply via graceful degradation.
    let pending = outbox.fetch_pending(10).unwrap();
    assert_eq!(pending.len(), 1, "degraded path must still queue a reply");
    let content: MessageContent = serde_json::from_str(&pending[0].payload).unwrap();
    match content {
        MessageContent::Text(t) => {
            assert!(t.contains("ping"), "degraded reply should echo input: {t}");
            assert!(t.contains("ai error"), "degraded reply should mark the error: {t}");
        }
        other => panic!("expected text reply, got {other:?}"),
    }
}
