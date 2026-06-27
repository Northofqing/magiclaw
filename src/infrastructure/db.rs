use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Shared database connection pool (single connection with mutex).
#[derive(Clone)]
pub struct DbPool {
    conn: Arc<Mutex<Connection>>,
}

impl DbPool {
    pub fn new(conn: Connection) -> Self {
        Self { conn: Arc::new(Mutex::new(conn)) }
    }

    /// Execute a write operation. The closure should return Ok(()) on success.
    pub fn execute<F>(&self, f: F) -> Result<(), String>
    where
        F: FnOnce(&Connection) -> Result<(), rusqlite::Error>,
    {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        f(&conn).map_err(|e| e.to_string())
    }

    /// Execute a read operation and return a value.
    pub fn query<F, T>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error>,
    {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        f(&conn).map_err(|e| e.to_string())
    }
}

/// Initialize the SQLite database with the required schema.
pub fn init_db(path: impl AsRef<Path>) -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open(path)?;

    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;

         CREATE TABLE IF NOT EXISTS sync_buf (
             channel TEXT NOT NULL,
             account TEXT NOT NULL,
             buf BLOB NOT NULL,
             updated_at INTEGER NOT NULL,
             PRIMARY KEY (channel, account)
         );

         CREATE TABLE IF NOT EXISTS inbox (
             id TEXT PRIMARY KEY,
             channel TEXT NOT NULL,
             conversation_id TEXT NOT NULL,
             payload TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'pending',
             created_at INTEGER NOT NULL DEFAULT (unixepoch()),
             updated_at INTEGER NOT NULL DEFAULT (unixepoch())
         );

         CREATE TABLE IF NOT EXISTS outbox (
             id TEXT PRIMARY KEY,
             route_key TEXT NOT NULL,
             payload TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'pending',
             retry_count INTEGER NOT NULL DEFAULT 0,
             next_retry_at INTEGER,
             last_error TEXT,
             created_at INTEGER NOT NULL DEFAULT (unixepoch()),
             updated_at INTEGER NOT NULL DEFAULT (unixepoch())
         );

         CREATE TABLE IF NOT EXISTS dead_letter (
             id TEXT PRIMARY KEY,
             source TEXT NOT NULL,
             payload TEXT NOT NULL,
             reason TEXT NOT NULL,
             created_at INTEGER NOT NULL DEFAULT (unixepoch())
         );

         CREATE TABLE IF NOT EXISTS conversation_state (
             route_key TEXT PRIMARY KEY,
             state_json TEXT NOT NULL,
             updated_at INTEGER NOT NULL DEFAULT (unixepoch())
         );

         CREATE TABLE IF NOT EXISTS audit_log (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             route_key TEXT,
             action TEXT NOT NULL,
             result TEXT NOT NULL,
             created_at INTEGER NOT NULL DEFAULT (unixepoch()),
             -- Red line #5.3: chain hash for tamper detection.
             -- prev_hash is the previous entry's entry_hash (NULL only for the
             -- genesis entry); entry_hash is sha256(prev_hash || route_key ||
             -- action || result || created_at). Verification at startup walks
             -- the chain and aborts on any mismatch.
             prev_hash TEXT,
             entry_hash TEXT
         );

         CREATE TABLE IF NOT EXISTS api_clients (
             id TEXT PRIMARY KEY,
             project_id TEXT NOT NULL,
             client_name TEXT NOT NULL,
             token_hash TEXT NOT NULL UNIQUE,
             scopes TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             expires_at INTEGER NOT NULL,
             revoked_at INTEGER,
             rotated_from TEXT
         );

         CREATE TABLE IF NOT EXISTS projects (
             project_key TEXT PRIMARY KEY,
             project_name TEXT NOT NULL,
             source_system TEXT,
             metadata_json TEXT,
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         );

         CREATE TABLE IF NOT EXISTS delivery_targets (
             target_id TEXT PRIMARY KEY,
             channel TEXT NOT NULL,
             peer_id TEXT NOT NULL,
             conversation_id TEXT NOT NULL,
             conversation_type TEXT NOT NULL,
             account_scope TEXT NOT NULL DEFAULT '',
             last_seen_at INTEGER,
             status TEXT NOT NULL DEFAULT 'active',
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL,
             UNIQUE (channel, peer_id, conversation_id, conversation_type, account_scope)
         );

         CREATE TABLE IF NOT EXISTS project_bindings (
             id TEXT PRIMARY KEY,
             project_key TEXT NOT NULL,
             target_id TEXT NOT NULL,
             bind_source TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'active',
             bound_at INTEGER NOT NULL,
             unbound_at INTEGER,
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL,
             UNIQUE (project_key, target_id)
         );

         CREATE TABLE IF NOT EXISTS binding_tokens (
             token TEXT PRIMARY KEY,
             project_key TEXT NOT NULL,
             target_channel TEXT,
             target_peer_id TEXT,
             expires_at INTEGER NOT NULL,
             consumed_at INTEGER,
             consumed_by_target_id TEXT,
             status TEXT NOT NULL DEFAULT 'active',
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         );

         CREATE TABLE IF NOT EXISTS push_jobs (
             job_id TEXT PRIMARY KEY,
             source_format TEXT NOT NULL,
             source_path TEXT NOT NULL,
             status TEXT NOT NULL,
             total_items INTEGER NOT NULL DEFAULT 0,
             success_items INTEGER NOT NULL DEFAULT 0,
             failed_items INTEGER NOT NULL DEFAULT 0,
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         );

         CREATE TABLE IF NOT EXISTS push_job_items (
             item_id TEXT PRIMARY KEY,
             job_id TEXT NOT NULL,
             project_key TEXT NOT NULL,
             message_text TEXT NOT NULL,
             mode TEXT NOT NULL,
             target_targets_json TEXT,
             status TEXT NOT NULL,
             error TEXT,
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         );

         CREATE TABLE IF NOT EXISTS user_agent_preferences (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             channel TEXT NOT NULL,
             account_scope TEXT NOT NULL,
             peer_id TEXT NOT NULL,
             agent_name TEXT NOT NULL,
             updated_at INTEGER NOT NULL,
             UNIQUE (channel, account_scope, peer_id)
         );

         CREATE INDEX IF NOT EXISTS idx_project_bindings_project ON project_bindings (project_key, status);
         CREATE INDEX IF NOT EXISTS idx_project_bindings_target ON project_bindings (target_id, status);
         CREATE INDEX IF NOT EXISTS idx_binding_tokens_project ON binding_tokens (project_key, status, expires_at);
         CREATE INDEX IF NOT EXISTS idx_push_job_items_job ON push_job_items (job_id, status);
         CREATE INDEX IF NOT EXISTS idx_delivery_targets_channel ON delivery_targets (channel, status);
         CREATE INDEX IF NOT EXISTS idx_user_agent_preferences 
             ON user_agent_preferences (channel, account_scope, peer_id);
         CREATE INDEX IF NOT EXISTS idx_api_clients_project ON api_clients (project_id, revoked_at, expires_at);",
    )?;

    // Schema migrations for existing DBs (idempotent: errors are tolerated if
    // the column already exists). Adding hash chain columns to audit_log
    // (CLAUDE.md red line #5.3) requires backfilling prev_hash/entry_hash for
    // pre-existing rows or accepting that the chain starts from "now".
    let _ = conn.execute(
        "ALTER TABLE audit_log ADD COLUMN prev_hash TEXT",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE audit_log ADD COLUMN entry_hash TEXT",
        [],
    );

    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_db_creates_tables() {
        let conn = init_db(":memory:").unwrap();
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(tables.contains(&"sync_buf".to_string()));
        assert!(tables.contains(&"inbox".to_string()));
        assert!(tables.contains(&"outbox".to_string()));
        assert!(tables.contains(&"dead_letter".to_string()));
        assert!(tables.contains(&"conversation_state".to_string()));
        assert!(tables.contains(&"audit_log".to_string()));
        assert!(tables.contains(&"api_clients".to_string()));
        assert!(tables.contains(&"projects".to_string()));
        assert!(tables.contains(&"delivery_targets".to_string()));
        assert!(tables.contains(&"project_bindings".to_string()));
        assert!(tables.contains(&"binding_tokens".to_string()));
        assert!(tables.contains(&"push_jobs".to_string()));
        assert!(tables.contains(&"push_job_items".to_string()));
        assert!(tables.contains(&"user_agent_preferences".to_string()));
    }
}
