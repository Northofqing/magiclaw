use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

use crate::domain::ports::sync_buf_store::{SyncBufError, SyncBufStore};

/// SQLite-backed sync_buf persistence.
pub struct SqliteSyncBufStore {
    conn: Mutex<Connection>,
}

impl SqliteSyncBufStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SyncBufError> {
        let conn = Connection::open(path).map_err(|e| SyncBufError::Db(e.to_string()))?;

        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS sync_buf (
                 channel TEXT NOT NULL,
                 account TEXT NOT NULL,
                 buf BLOB NOT NULL,
                 updated_at INTEGER NOT NULL,
                 PRIMARY KEY (channel, account)
             );",
        )
        .map_err(|e| SyncBufError::Db(e.to_string()))?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

impl SyncBufStore for SqliteSyncBufStore {
    fn save(&self, channel: &str, account: &str, buf: &[u8]) -> Result<(), SyncBufError> {
        let conn = self.conn.lock().map_err(|e| SyncBufError::Db(e.to_string()))?;
        conn.execute(
            "INSERT OR REPLACE INTO sync_buf (channel, account, buf, updated_at)
             VALUES (?1, ?2, ?3, unixepoch())",
            rusqlite::params![channel, account, buf],
        )
        .map_err(|e| SyncBufError::Db(e.to_string()))?;
        Ok(())
    }

    fn load(&self, channel: &str, account: &str) -> Result<Vec<u8>, SyncBufError> {
        let conn = self.conn.lock().map_err(|e| SyncBufError::Db(e.to_string()))?;
        let result: Result<Vec<u8>, _> = conn.query_row(
            "SELECT buf FROM sync_buf WHERE channel = ?1 AND account = ?2",
            rusqlite::params![channel, account],
            |row| row.get(0),
        );

        match result {
            Ok(buf) => Ok(buf),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Vec::new()),
            Err(e) => Err(SyncBufError::Db(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load_round_trip() {
        let store = SqliteSyncBufStore::open(":memory:").unwrap();
        let data = b"hello sync_buf";
        store.save("wechat", "account_001", data).unwrap();

        let loaded = store.load("wechat", "account_001").unwrap();
        assert_eq!(loaded, data);
    }

    #[test]
    fn load_nonexistent_returns_empty() {
        let store = SqliteSyncBufStore::open(":memory:").unwrap();
        let loaded = store.load("wechat", "nonexistent").unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn overwrite_existing() {
        let store = SqliteSyncBufStore::open(":memory:").unwrap();
        store.save("wechat", "acc", b"v1").unwrap();
        store.save("wechat", "acc", b"v2").unwrap();

        let loaded = store.load("wechat", "acc").unwrap();
        assert_eq!(loaded, b"v2");
    }
}
