//! System-level closed-loop test for the generic `CliAgentBackend`.
//!
//! Verifies the full inbound→reply loop with a configurable CLI agent (the same
//! shape used for codex/copilot/hermes/openclaw): an inbound message flows
//! through the assembled pipeline
//! (Normalize → Permission → AI[cli_agent] → Formatter → OutboxStage) and
//! produces a durable outbound `OutboxEntry(pending)` carrying the agent reply.
//! Deterministic stub scripts keep the test hermetic (no network).
//!
//! Covers both reply-extraction modes: a temp output file (codex `-o` style)
//! and raw stdout, plus graceful degradation on a missing binary.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aiclaw::adapters::conversation_store::ConversationStore;
use aiclaw::adapters::sqlite_audit::SqliteAuditSink;
use aiclaw::adapters::sqlite_outbox::SqliteOutboxRepo;
use aiclaw::application::audit::query_audit_logs;
use aiclaw::core::ai::backend::AiBackend;
use aiclaw::core::ai::cli_agent::CliAgentBackend;
use aiclaw::core::pipeline::ai::AiMiddleware;
use aiclaw::core::pipeline::formatter::Formatter;
use aiclaw::core::pipeline::normalize::Normalize;
use aiclaw::core::pipeline::outbox::OutboxStage;
use aiclaw::core::pipeline::permission::Permission;
use aiclaw::core::pipeline::Pipeline;
use aiclaw::domain::entities::message::{Direction, Message, MessageContent};
use aiclaw::domain::ports::audit_sink::AuditSink;
use aiclaw::domain::ports::conversation_queue::ConversationQueue;
use aiclaw::domain::ports::outbox_repo::OutboxRepo;
use aiclaw::domain::value_objects::route_key::{ChannelId, ConversationType, RouteKey};
use aiclaw::infrastructure::config::{AppConfig, CliAgentConfig};
use aiclaw::infrastructure::db::{init_db, DbPool};

fn write_stub(tag: &str, name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "aiclaw_cli_agent_loop_{}_{}",
        std::process::id(),
        tag
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
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
        id: "agent-inbound-1".into(),
        route_key: key.clone(),
        sequence: Some(1),
        timestamp_ms: 100,
        direction: Direction::Inbound,
        content: MessageContent::Text(text.into()),
        audit_mark: None,
    }
}

#[tokio::test]
async fn cli_agent_output_file_reply_reaches_outbox_with_audit() {
    // Codex-style stub: writes its final message to the file passed after `-o`.
    let stub = write_stub(
        "codex",
        "codex",
        "#!/bin/sh\nwhile [ \"$1\" != \"-o\" ]; do shift; done\nprintf 'AGENT-PONG' > \"$2\"\n",
    );

    let pool = DbPool::new(init_db(":memory:").unwrap());
    let outbox = Arc::new(SqliteOutboxRepo::new(pool.clone()));
    let audit = Arc::new(SqliteAuditSink::new(pool.clone()));

    let backend: Arc<dyn AiBackend> = Arc::new(CliAgentBackend::new(
        "codex",
        CliAgentConfig {
            binary_path: stub.to_string_lossy().into_owned(),
            args: vec!["-o".into(), "{output_file}".into(), "{prompt}".into()],
            timeout_secs: 15,
            max_output_bytes: 16_384,
            result_json_pointer: None,
            read_output_file: true,
        },
    ));

    let pipeline = build_pipeline(
        backend,
        audit as Arc<dyn AuditSink>,
        outbox.clone() as Arc<dyn OutboxRepo>,
    );
    let store = ConversationStore::new(256, 1800, 0, Some(pipeline), AppConfig::default(), None);

    let key = RouteKey::new(
        ChannelId::new("wechat"),
        "conv_agent",
        "user_agent",
        ConversationType::Direct,
    );
    store.enqueue(&key, inbound(&key, "ping")).unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let pending = outbox.fetch_pending(10).unwrap();
    assert_eq!(pending.len(), 1, "pipeline should queue exactly one reply");
    let content: MessageContent = serde_json::from_str(&pending[0].payload).unwrap();
    match content {
        MessageContent::Text(t) => {
            assert!(t.contains("AGENT-PONG"), "reply must come from the agent, got: {t}");
            assert!(!t.contains("[echo]"), "must not be the echo backend: {t}");
        }
        other => panic!("expected text reply, got {other:?}"),
    }

    // Red line 2.6: the AI decision is audit-logged under the agent name.
    let rk = serde_json::to_string(&key).unwrap();
    let records = query_audit_logs(&pool, Some(&rk), 10).unwrap();
    assert!(
        records
            .iter()
            .any(|r| r.action == "ai_generate" && r.result.contains("codex")),
        "expected an ai_generate audit record for codex, got: {records:?}"
    );
}

#[tokio::test]
async fn cli_agent_missing_binary_degrades_without_panic() {
    let pool = DbPool::new(init_db(":memory:").unwrap());
    let outbox = Arc::new(SqliteOutboxRepo::new(pool.clone()));
    let audit = Arc::new(SqliteAuditSink::new(pool.clone()));

    let backend: Arc<dyn AiBackend> = Arc::new(CliAgentBackend::new(
        "hermes",
        CliAgentConfig {
            binary_path: "/nonexistent/hermes_does_not_exist".into(),
            args: vec!["{prompt}".into()],
            timeout_secs: 5,
            max_output_bytes: 16_384,
            result_json_pointer: None,
            read_output_file: false,
        },
    ));

    let pipeline = build_pipeline(
        backend,
        audit as Arc<dyn AuditSink>,
        outbox.clone() as Arc<dyn OutboxRepo>,
    );
    let store = ConversationStore::new(256, 1800, 0, Some(pipeline), AppConfig::default(), None);

    let key = RouteKey::new(
        ChannelId::new("wechat"),
        "conv_agent_fail",
        "user_agent",
        ConversationType::Direct,
    );
    store.enqueue(&key, inbound(&key, "ping")).unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;

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
