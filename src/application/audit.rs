use serde::{Deserialize, Serialize};

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

/// Audit tooling: query audit logs for a given route or action.
pub fn query_audit_logs(db: &DbPool, route_key: Option<&str>, limit: usize) -> Result<Vec<AuditRecord>, String> {
    let rk = route_key.map(|s| s.to_string());
    let limit = limit as i64;
    db.query(move |conn| {
        let mut stmt = if rk.is_some() {
            conn.prepare(
                "SELECT id, route_key, action, result, created_at FROM audit_log WHERE route_key = ?1 ORDER BY created_at DESC LIMIT ?2"
            )?
        } else {
            conn.prepare(
                "SELECT id, route_key, action, result, created_at FROM audit_log ORDER BY created_at DESC LIMIT ?1"
            )?
        };

        let records: Result<Vec<_>, _> = if let Some(ref key) = rk {
            stmt.query_map(rusqlite::params![key, limit], |row| {
                Ok(AuditRecord {
                    id: row.get(0)?, route_key: row.get(1)?, action: row.get(2)?,
                    result: row.get(3)?, created_at: row.get(4)?,
                })
            })?.collect()
        } else {
            stmt.query_map(rusqlite::params![limit], |row| {
                Ok(AuditRecord {
                    id: row.get(0)?, route_key: row.get(1)?, action: row.get(2)?,
                    result: row.get(3)?, created_at: row.get(4)?,
                })
            })?.collect()
        };

        records
    }).map_err(|e| format!("audit query error: {}", e))
}

/// Write an audit log entry for a high-risk operation.
pub fn write_audit(db: &DbPool, route_key: Option<&str>, action: &str, result: &str) -> Result<(), String> {
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
    use crate::infrastructure::db::init_db;

    fn make_pool() -> DbPool {
        DbPool::new(init_db(":memory:").unwrap())
    }

    #[test]
    fn write_and_query_audit() {
        let db = make_pool();
        write_audit(&db, Some("wechat/conv1"), "auto_allowlist", "added").unwrap();
        write_audit(&db, Some("wechat/conv1"), "send", "success").unwrap();

        let records = query_audit_logs(&db, Some("wechat/conv1"), 10).unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn query_all_audit() {
        let db = make_pool();
        write_audit(&db, None, "startup", "ok").unwrap();
        let records = query_audit_logs(&db, None, 10).unwrap();
        assert!(!records.is_empty());
    }
}
