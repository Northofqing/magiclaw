use crate::domain::ports::audit_sink::AuditSink;
use crate::domain::services::audit_chain::{compute_entry_hash, ChainError, ChainRow};
use crate::infrastructure::db::DbPool;

/// SQLite-backed audit sink. Writes to the immutable `audit_log` table.
///
/// Red line #5.3: each insert is appended with a chained SHA-256 hash so
/// tampering with any row (or reordering) breaks the chain and is detected
/// at startup via `verify_chain`.
///
/// Red line #2.6: 关键数据流与发送决策留痕。Writes are best-effort: a storage
/// failure is logged but never propagated to the caller, so auditing can
/// never break the business path.
pub struct SqliteAuditSink {
    db: DbPool,
}

impl SqliteAuditSink {
    pub fn new(db: DbPool) -> Self {
        Self { db }
    }
}

impl AuditSink for SqliteAuditSink {
    fn record(&self, route_key: Option<&str>, action: &str, result: &str) {
        let rk = route_key.map(|s| s.to_string());
        let rk_for_hash = rk.clone().unwrap_or_default();
        let action_owned = action.to_string();
        let result_owned = result.to_string();
        let action_for_log = action_owned.clone();
        let write = self.db.execute(move |conn| {
            // Read chain head (last entry_hash) under the same connection so
            // the read+write are serialised. Note: in-memory SQLite serialises
            // writes by default; for file-backed SQLite this is still safe
            // because each `execute` takes a connection from the pool.
            let prev_hash: Option<String> = conn
                .query_row(
                    "SELECT entry_hash FROM audit_log ORDER BY id DESC LIMIT 1",
                    [],
                    |row| row.get::<_, Option<String>>(0),
                )
                .unwrap_or(None);

            let now: i64 = conn
                .query_row("SELECT CAST(strftime('%s','now') AS INTEGER)", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap_or_else(|_| chrono::Utc::now().timestamp());

            let prev_for_hash = prev_hash.clone().unwrap_or_default();
            let entry_hash = compute_entry_hash(
                &prev_for_hash,
                &rk_for_hash,
                &action_owned,
                &result_owned,
                now,
            );

            conn.execute(
                "INSERT INTO audit_log (route_key, action, result, created_at, prev_hash, entry_hash) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![rk, action_owned, result_owned, now, prev_hash, entry_hash],
            )?;
            Ok(())
        });
        if let Err(e) = write {
            tracing::error!(error = %e, action = %action_for_log, "failed to write audit log");
        }
    }
}

/// Walk the audit chain and verify hash continuity. Returns the head hash on
/// success or the first tamper/corruption detected.
///
/// Should be called once at startup. A mismatch is logged at `error` level;
/// the daemon refuses to start (caller decides policy — see
/// `application/audit.rs::verify_chain_on_startup`).
pub fn verify_chain(db: &DbPool) -> Result<Option<String>, ChainError> {
    let rows = db
        .query(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, prev_hash, entry_hash, route_key, action, result, created_at \
                     FROM audit_log ORDER BY id ASC",
                )?;
            let it = stmt.query_map([], |row| {
                Ok(ChainRow {
                    id: row.get(0)?,
                    prev_hash: row.get(1)?,
                    entry_hash: row.get(2)?,
                    route_key: row.get(3)?,
                    action: row.get(4)?,
                    result: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?;
            let mut out = Vec::new();
            for r in it {
                out.push(r?);
            }
            Ok(out)
        })
        .map_err(|e| ChainError::HeadMismatch {
            id: 0,
            expected: None,
            actual: Some(e),
        })?;
    crate::domain::services::audit_chain::verify(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::audit::query_audit_logs;
    use crate::infrastructure::db::init_db;

    fn make_pool() -> DbPool {
        DbPool::new(init_db(":memory:").unwrap())
    }

    #[test]
    fn record_persists_audit_entry() {
        let db = make_pool();
        let sink = SqliteAuditSink::new(db.clone());
        sink.record(Some("wechat/conv1"), "send", "sent");
        sink.record(Some("wechat/conv1"), "dead_letter", "max retries");

        let q = crate::adapters::sqlite_audit_query::SqliteAuditQuery::new(db);
        let records = query_audit_logs(&q, Some("wechat/conv1"), 10).unwrap();
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|r| r.action == "send" && r.result == "sent"));
        assert!(records.iter().any(|r| r.action == "dead_letter"));
    }
}
