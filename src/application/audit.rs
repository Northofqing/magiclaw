use serde::{Deserialize, Serialize};

use crate::domain::ports::audit_query::AuditQuery;
use crate::domain::ports::audit_sink::AuditSink;
use crate::infrastructure::db::DbPool;

/// An audit log entry for querying.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    pub id: i64,
    pub route_key: Option<String>,
    pub action: String,
    pub result: String,
    pub created_at: i64,
}

/// Audit query via the `AuditQuery` port — preferred entry point for the
/// application layer (no DbPool, no rusqlite).
///
/// `route_key = None` returns recent records across all routes.
pub fn query_audit_logs(
    store: &dyn AuditQuery,
    route_key: Option<&str>,
    limit: usize,
) -> Result<Vec<AuditRecord>, String> {
    match route_key {
        Some(k) => store.query_by_route(k, limit),
        None => store.query_all(limit),
    }
}

/// Audit write via the `AuditSink` port — preferred entry point for the
/// application layer (no DbPool, no rusqlite).
pub fn write_audit_via_sink(
    sink: &dyn AuditSink,
    route_key: Option<&str>,
    action: &str,
    result: &str,
) {
    sink.record(route_key, action, result);
}

/// Audit write via raw DbPool — kept for callers not yet wired up to the
/// `AuditSink` port. New code should prefer `write_audit_via_sink`.
#[deprecated(note = "use write_audit_via_sink with an AuditSink port")]
pub fn write_audit(
    db: &DbPool,
    route_key: Option<&str>,
    action: &str,
    result: &str,
) -> Result<(), String> {
    let rk = route_key.map(|s| s.to_string());
    let action = action.to_string();
    let result = result.to_string();
    db.execute(move |conn| {
        conn.execute(
            "INSERT INTO audit_log (route_key, action, result, created_at) VALUES (?1, ?2, ?3, unixepoch())",
            rusqlite::params![rk, action, result],
        )?;
        Ok(())
    }).map_err(|e| format!("audit write error: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::sqlite_audit::SqliteAuditSink;
    use crate::adapters::sqlite_audit_query::SqliteAuditQuery;
    use crate::infrastructure::db::init_db;

    fn make_pool() -> crate::infrastructure::db::DbPool {
        crate::infrastructure::db::DbPool::new(init_db(":memory:").unwrap())
    }

    #[test]
    fn write_and_query_audit_via_ports() {
        let pool = make_pool();
        let sink = SqliteAuditSink::new(pool.clone());
        write_audit_via_sink(&sink, Some("wechat/conv1"), "auto_allowlist", "added");
        write_audit_via_sink(&sink, Some("wechat/conv1"), "send", "success");

        let q = SqliteAuditQuery::new(pool);
        let records = query_audit_logs(&q, Some("wechat/conv1"), 10).unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn query_all_audit_via_ports() {
        let pool = make_pool();
        let sink = SqliteAuditSink::new(pool.clone());
        write_audit_via_sink(&sink, None, "startup", "ok");

        let q = SqliteAuditQuery::new(pool);
        let records = query_audit_logs(&q, None, 10).unwrap();
        assert!(!records.is_empty());
    }
}
