//! Closed-loop test for the audit log hash chain (CLAUDE.md red line #5.3).
//!
//! Verifies:
//! 1. Writes through AuditSink populate prev_hash and entry_hash.
//! 2. The chain verifies clean on a fresh database and after multiple inserts.
//! 3. Tampering with a row (changing `result` without recomputing the hash)
//!    makes verify_chain return HashMismatch at the tampered id.
//! 4. Deleting a row breaks the chain (HeadMismatch at the next entry).
//! 5. AppRuntime::start_background refuses to start on a tampered chain.

use magiclaw::adapters::sqlite_audit::{verify_chain, SqliteAuditSink};
use magiclaw::adapters::sqlite_audit_query::SqliteAuditQuery;
use magiclaw::application::audit::query_audit_logs;
use magiclaw::domain::ports::audit_sink::AuditSink;
use magiclaw::domain::services::audit_chain::ChainError;
use magiclaw::infrastructure::config::AppConfig;
use magiclaw::infrastructure::db::{init_db, DbPool};
use magiclaw::infrastructure::runtime::AppRuntime;

fn make_pool() -> DbPool {
    DbPool::new(init_db(":memory:").unwrap())
}

#[test]
fn empty_chain_verifies_with_no_head() {
    let pool = make_pool();
    let head = verify_chain(&pool).unwrap();
    assert!(head.is_none());
}

#[test]
fn each_record_populates_chain_columns() {
    let pool = make_pool();
    let sink = SqliteAuditSink::new(pool.clone());
    sink.record(Some("wechat/c1"), "send", "sent");
    sink.record(Some("wechat/c1"), "dead_letter", "max retries");
    sink.record(None, "startup", "ok");

    let q = SqliteAuditQuery::new(pool.clone());
    let records = query_audit_logs(&q, None, 10).unwrap();
    assert_eq!(records.len(), 3);

    // Re-query via the SQL store to confirm chain columns are populated.
    type ChainTuple = (i64, Option<String>, Option<String>);
    let entries: Vec<ChainTuple> = pool
        .query(|conn| -> Result<Vec<ChainTuple>, rusqlite::Error> {
            let mut stmt = conn
                .prepare("SELECT id, prev_hash, entry_hash FROM audit_log ORDER BY id ASC")?;
            let mapped = stmt.query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
            mapped.into_iter().collect::<Result<Vec<_>, _>>()
        })
        .unwrap();
    assert_eq!(entries.len(), 3);
    // Genesis entry: prev_hash NULL.
    assert!(entries[0].1.is_none());
    // Subsequent entries: prev_hash matches prior entry_hash.
    assert_eq!(entries[1].1.as_ref().unwrap(), entries[0].2.as_ref().unwrap());
    assert_eq!(entries[2].1.as_ref().unwrap(), entries[1].2.as_ref().unwrap());
    // All entries have a non-NULL entry_hash of length 64 (SHA-256 hex).
    for (id, _, h) in &entries {
        let h = h.as_ref().unwrap_or_else(|| panic!("entry {} missing entry_hash", id));
        assert_eq!(h.len(), 64, "entry {} entry_hash wrong length", id);
    }
}

#[test]
fn clean_chain_verifies_after_many_inserts() {
    let pool = make_pool();
    let sink = SqliteAuditSink::new(pool.clone());
    for i in 0..50 {
        sink.record(
            Some(&format!("wechat/conv{}", i % 5)),
            "send",
            &format!("attempt {}", i),
        );
    }
    let head = verify_chain(&pool).unwrap();
    assert!(head.is_some(), "chain should have a head after inserts");
    assert_eq!(head.unwrap().len(), 64);
}

#[test]
fn tampered_result_is_detected() {
    let pool = make_pool();
    let sink = SqliteAuditSink::new(pool.clone());
    sink.record(Some("wechat/c1"), "send", "sent");
    sink.record(Some("wechat/c1"), "send", "sent");
    sink.record(Some("wechat/c1"), "send", "sent");

    // Tamper: change result of id=2 without recomputing entry_hash.
    pool.execute(|conn| {
        conn.execute(
            "UPDATE audit_log SET result = 'TAMPERED' WHERE id = 2",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    let err = verify_chain(&pool).unwrap_err();
    assert!(
        matches!(err, ChainError::HashMismatch { id: 2, .. }),
        "expected HashMismatch at id=2, got {:?}",
        err
    );
}

#[test]
fn deleted_middle_entry_is_detected() {
    let pool = make_pool();
    let sink = SqliteAuditSink::new(pool.clone());
    sink.record(Some("wechat/c1"), "send", "sent");
    sink.record(Some("wechat/c1"), "send", "sent");
    sink.record(Some("wechat/c1"), "send", "sent");

    // Delete id=2. Entry id=3's prev_hash now points to id=1's hash, but the
    // chain verifier expects it to point to id=2's hash.
    pool.execute(|conn| {
        conn.execute("DELETE FROM audit_log WHERE id = 2", [])?;
        Ok(())
    })
    .unwrap();

    let err = verify_chain(&pool).unwrap_err();
    assert!(
        matches!(err, ChainError::HeadMismatch { id: 3, .. }),
        "expected HeadMismatch at id=3, got {:?}",
        err
    );
}

#[tokio::test]
#[allow(clippy::field_reassign_with_default)]
async fn app_runtime_refuses_to_start_on_tampered_chain() {
    let db_path = std::env::temp_dir().join(format!(
        "magiclaw_audit_tamper_{}.db",
        uuid::Uuid::new_v4()
    ));
    let mut config = AppConfig::default();
    config.db_path = db_path.to_string_lossy().to_string();

    let runtime = AppRuntime::new(config).unwrap();
    runtime.audit_sink.record(Some("seed/c1"), "send", "sent");
    runtime.audit_sink.record(Some("seed/c2"), "send", "sent");
    runtime.audit_sink.record(Some("seed/c3"), "send", "sent");

    // Tamper one row directly through SQL.
    runtime.db_pool().execute(|conn| {
        conn.execute(
            "UPDATE audit_log SET result = 'TAMPERED' WHERE id = 2",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    let result = runtime.start_background().await;
    assert!(
        result.is_err(),
        "start_background should refuse on tampered chain"
    );
    let msg = result.unwrap_err();
    assert!(
        msg.contains("audit log chain"),
        "unexpected error message: {}",
        msg
    );

    let _ = std::fs::remove_file(db_path);
}