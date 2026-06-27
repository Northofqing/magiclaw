//! SQLite implementation of the `AuditQuery` port.
//!
//! Decoupled from `application/audit.rs` so the application layer never
//! imports `rusqlite` or `DbPool` directly — it only sees the trait.

use crate::application::audit::AuditRecord;
use crate::domain::ports::audit_query::AuditQuery;
use crate::infrastructure::db::DbPool;

/// SQLite-backed audit query adapter. Stateless and cheap to clone/share.
pub struct SqliteAuditQuery {
    pool: DbPool,
}

impl SqliteAuditQuery {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }
}

impl AuditQuery for SqliteAuditQuery {
    fn query_by_route(
        &self,
        route_key: &str,
        limit: usize,
    ) -> Result<Vec<AuditRecord>, String> {
        let key = route_key.to_string();
        let lim = limit as i64;
        self.pool
            .query(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, route_key, action, result, created_at FROM audit_log \
                     WHERE route_key = ?1 ORDER BY created_at DESC LIMIT ?2",
                )?;
                let records: Result<Vec<_>, _> = stmt
                    .query_map(rusqlite::params![key, lim], |row| {
                        Ok(AuditRecord {
                            id: row.get(0)?,
                            route_key: row.get(1)?,
                            action: row.get(2)?,
                            result: row.get(3)?,
                            created_at: row.get(4)?,
                        })
                    })?
                    .collect();
                records
            })
            .map_err(|e| format!("audit query error: {}", e))
    }

    fn query_all(&self, limit: usize) -> Result<Vec<AuditRecord>, String> {
        let lim = limit as i64;
        self.pool
            .query(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, route_key, action, result, created_at FROM audit_log \
                     ORDER BY created_at DESC LIMIT ?1",
                )?;
                let records: Result<Vec<_>, _> = stmt
                    .query_map(rusqlite::params![lim], |row| {
                        Ok(AuditRecord {
                            id: row.get(0)?,
                            route_key: row.get(1)?,
                            action: row.get(2)?,
                            result: row.get(3)?,
                            created_at: row.get(4)?,
                        })
                    })?
                    .collect();
                records
            })
            .map_err(|e| format!("audit query error: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::sqlite_audit::SqliteAuditSink;
    use crate::domain::ports::audit_sink::AuditSink;
    use crate::infrastructure::db::init_db;

    #[test]
    fn query_by_route_returns_matching_records() {
        let pool = DbPool::new(init_db(":memory:").unwrap());
        let sink = SqliteAuditSink::new(pool.clone());
        sink.record(Some("wechat/conv1"), "send", "ok");
        sink.record(Some("wechat/conv2"), "send", "ok");
        sink.record(Some("wechat/conv1"), "auto_allowlist", "added");

        let q = SqliteAuditQuery::new(pool);
        let recs = q.query_by_route("wechat/conv1", 10).unwrap();
        assert_eq!(recs.len(), 2);
        assert!(recs.iter().all(|r| r.route_key.as_deref() == Some("wechat/conv1")));
    }

    #[test]
    fn query_all_returns_recent_records() {
        let pool = DbPool::new(init_db(":memory:").unwrap());
        let sink = SqliteAuditSink::new(pool.clone());
        sink.record(Some("a"), "send", "ok");
        sink.record(Some("b"), "send", "ok");
        sink.record(None, "startup", "ok");

        let q = SqliteAuditQuery::new(pool);
        let recs = q.query_all(10).unwrap();
        assert_eq!(recs.len(), 3);
    }
}