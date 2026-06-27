use crate::domain::ports::audit_sink::AuditSink;
use crate::infrastructure::db::DbPool;

/// SQLite-backed audit sink. Writes to the immutable `audit_log` table.
///
/// Red line 2.6: 审计日志不可篡改(仅追加),关键数据流与发送决策留痕。
/// Writes are best-effort: a storage failure is logged but never propagated to
/// the caller, so auditing can never break the business path.
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
        let action_owned = action.to_string();
        let result = result.to_string();
        let action_for_log = action_owned.clone();
        let write = self.db.execute(move |conn| {
            conn.execute(
                "INSERT INTO audit_log (route_key, action, result, created_at) VALUES (?1, ?2, ?3, unixepoch())",
                rusqlite::params![rk, action_owned, result],
            )?;
            Ok(())
        });
        if let Err(e) = write {
            tracing::error!(error = %e, action = %action_for_log, "failed to write audit log");
        }
    }
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
