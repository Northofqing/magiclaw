use crate::domain::ports::inbox_repo::{InboxRepo, RepoError, RepoResult};
use crate::infrastructure::storage::inbox::{InboxEntry, InboxStatus};
use crate::infrastructure::db::DbPool;

pub struct SqliteInboxRepo {
    db: DbPool,
}

impl SqliteInboxRepo {
    pub fn new(db: DbPool) -> Self { Self { db } }
}

impl InboxRepo for SqliteInboxRepo {
    fn insert(&self, entry: &InboxEntry) -> RepoResult<()> {
        let entry = entry.clone();
        self.db.execute(move |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO inbox (id, channel, conversation_id, payload, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![entry.id, entry.channel, entry.conversation_id, entry.payload, entry.status.as_str(), entry.created_at, entry.updated_at],
            )?;
            Ok(())
        }).map_err(RepoError::Db)
    }

    fn mark_status(&self, id: &str, status: InboxStatus) -> RepoResult<()> {
        let id = id.to_string();
        let s = status.as_str().to_string();
        self.db.execute(move |conn| {
            conn.execute("UPDATE inbox SET status = ?1, updated_at = unixepoch() WHERE id = ?2", rusqlite::params![s, id])?;
            Ok(())
        }).map_err(RepoError::Db)
    }

    fn exists(&self, id: &str) -> RepoResult<bool> {
        let id = id.to_string();
        self.db.query(move |conn| {
            conn.query_row("SELECT COUNT(*) > 0 FROM inbox WHERE id = ?1", rusqlite::params![id], |row| row.get(0))
        }).map_err(RepoError::Db)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::db::{init_db, DbPool};

    fn make_pool() -> DbPool {
        DbPool::new(init_db(":memory:").unwrap())
    }

    #[test]
    fn insert_and_check_exists() {
        let repo = SqliteInboxRepo::new(make_pool());
        repo.insert(&InboxEntry::new("m1", "wechat", "c1", "{}", 1000)).unwrap();
        assert!(repo.exists("m1").unwrap());
        assert!(!repo.exists("m2").unwrap());
    }

    #[test]
    fn insert_duplicate_is_ignored() {
        let repo = SqliteInboxRepo::new(make_pool());
        let e = InboxEntry::new("m1", "wechat", "c1", "{}", 1000);
        repo.insert(&e).unwrap();
        repo.insert(&e).unwrap();
    }

    #[test]
    fn mark_status_updates() {
        let repo = SqliteInboxRepo::new(make_pool());
        repo.insert(&InboxEntry::new("m1", "wechat", "c1", "{}", 1000)).unwrap();
        repo.mark_status("m1", InboxStatus::Processed).unwrap();
    }
}
