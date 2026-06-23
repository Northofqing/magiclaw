use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use crate::domain::ports::context_token_store::{ContextTokenError, ContextTokenStore};

/// SQLite-backed context_token persistence.
pub struct SqliteContextTokenStore {
    conn: Mutex<Connection>,
}

impl SqliteContextTokenStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ContextTokenError> {
        let conn = Connection::open(path).map_err(|e| ContextTokenError::Db(e.to_string()))?;

        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS context_tokens (
                 account_id TEXT NOT NULL,
                 user_id TEXT NOT NULL,
                 token TEXT NOT NULL,
                 updated_at INTEGER NOT NULL,
                 PRIMARY KEY (account_id, user_id)
             );",
        )
        .map_err(|e| ContextTokenError::Db(e.to_string()))?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

impl ContextTokenStore for SqliteContextTokenStore {
    fn set(&self, account_id: &str, user_id: &str, token: &str) -> Result<(), ContextTokenError> {
        let conn = self.conn.lock().map_err(|e| ContextTokenError::Db(e.to_string()))?;
        conn.execute(
            "INSERT OR REPLACE INTO context_tokens (account_id, user_id, token, updated_at)
             VALUES (?1, ?2, ?3, unixepoch())",
            rusqlite::params![account_id, user_id, token],
        )
        .map_err(|e| ContextTokenError::Db(e.to_string()))?;
        Ok(())
    }

    fn get(&self, account_id: &str, user_id: &str) -> Result<Option<String>, ContextTokenError> {
        let conn = self.conn.lock().map_err(|e| ContextTokenError::Db(e.to_string()))?;
        let result: Result<String, _> = conn.query_row(
            "SELECT token FROM context_tokens WHERE account_id = ?1 AND user_id = ?2",
            rusqlite::params![account_id, user_id],
            |row| row.get(0),
        );

        match result {
            Ok(token) => Ok(Some(token)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(ContextTokenError::Db(e.to_string())),
        }
    }

    fn get_all(&self, account_id: &str) -> Result<HashMap<String, String>, ContextTokenError> {
        let conn = self.conn.lock().map_err(|e| ContextTokenError::Db(e.to_string()))?;
        let mut stmt = conn
            .prepare("SELECT user_id, token FROM context_tokens WHERE account_id = ?1")
            .map_err(|e| ContextTokenError::Db(e.to_string()))?;

        let tokens = stmt
            .query_map(rusqlite::params![account_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| ContextTokenError::Db(e.to_string()))?
            .collect::<Result<HashMap<_, _>, _>>()
            .map_err(|e| ContextTokenError::Db(e.to_string()))?;

        Ok(tokens)
    }

    fn delete_all(&self, account_id: &str) -> Result<(), ContextTokenError> {
        let conn = self.conn.lock().map_err(|e| ContextTokenError::Db(e.to_string()))?;
        conn.execute(
            "DELETE FROM context_tokens WHERE account_id = ?1",
            rusqlite::params![account_id],
        )
        .map_err(|e| ContextTokenError::Db(e.to_string()))?;
        Ok(())
    }

    fn delete(&self, account_id: &str, user_id: &str) -> Result<(), ContextTokenError> {
        let conn = self.conn.lock().map_err(|e| ContextTokenError::Db(e.to_string()))?;
        conn.execute(
            "DELETE FROM context_tokens WHERE account_id = ?1 AND user_id = ?2",
            rusqlite::params![account_id, user_id],
        )
        .map_err(|e| ContextTokenError::Db(e.to_string()))?;
        Ok(())
    }
}
