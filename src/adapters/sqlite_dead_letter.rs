use crate::domain::ports::dead_letter_repo::DeadLetterRepo;
use crate::domain::ports::inbox_repo::{RepoError, RepoResult};
use crate::infrastructure::storage::dead_letter::DeadLetterEntry;
use crate::infrastructure::storage::outbox::OutboxEntry;
use crate::infrastructure::db::DbPool;
use rusqlite::OptionalExtension;

pub struct SqliteDeadLetterRepo {
    db: DbPool,
}

impl SqliteDeadLetterRepo {
    pub fn new(db: DbPool) -> Self { Self { db } }
}

impl DeadLetterRepo for SqliteDeadLetterRepo {
    fn insert(&self, entry: &DeadLetterEntry) -> RepoResult<()> {
        let entry = entry.clone();
        self.db.execute(move |conn| {
            conn.execute(
                "INSERT INTO dead_letter (id, source, payload, reason, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![entry.id, entry.source, entry.payload, entry.reason, entry.created_at],
            )?;
            Ok(())
        }).map_err(RepoError::Db)
    }

    fn list(&self, limit: usize) -> RepoResult<Vec<DeadLetterEntry>> {
        let db = self.db.clone();
        db.query(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, source, payload, reason, created_at FROM dead_letter ORDER BY created_at DESC LIMIT ?1"
            )?;
            let entries: Result<Vec<_>, _> = stmt.query_map(rusqlite::params![limit as i64], |row| {
                Ok(DeadLetterEntry {
                    id: row.get(0)?, source: row.get(1)?, payload: row.get(2)?,
                    reason: row.get(3)?, created_at: row.get(4)?,
                })
            })?.collect();
            entries
        }).map_err(RepoError::Db)
    }

    fn replay(&self, id: &str) -> RepoResult<OutboxEntry> {
        let id = id.to_string();
        let db = self.db.clone();
        db.query(move |conn| {
            let dl: DeadLetterEntry = conn.query_row(
                "SELECT id, source, payload, reason, created_at FROM dead_letter WHERE id = ?1",
                rusqlite::params![id],
                |row| Ok(DeadLetterEntry {
                    id: row.get(0)?, source: row.get(1)?, payload: row.get(2)?,
                    reason: row.get(3)?, created_at: row.get(4)?,
                }),
            )?;

            conn.execute("DELETE FROM dead_letter WHERE id = ?1", rusqlite::params![id])?;

            let now = chrono::Utc::now().timestamp();

            let existing_route_key: Option<String> = conn
                .query_row(
                    "SELECT route_key FROM outbox WHERE id = ?1",
                    rusqlite::params![id],
                    |row| row.get(0),
                )
                .optional()?;

            if existing_route_key.is_some() {
                conn.execute(
                    "UPDATE outbox SET status='pending', retry_count=0, next_retry_at=NULL, last_error=NULL, payload=?2, updated_at=?3 WHERE id=?1",
                    rusqlite::params![id, dl.payload, now],
                )?;
            } else {
                conn.execute(
                    "INSERT INTO outbox (id, route_key, payload, status, retry_count, created_at, updated_at) VALUES (?1, 'replayed', ?2, 'pending', 0, ?3, ?3)",
                    rusqlite::params![id, dl.payload, now],
                )?;
            }

            Ok(OutboxEntry::new(
                id,
                existing_route_key.unwrap_or_else(|| "replayed".to_string()),
                dl.payload,
                now,
            ))
        }).map_err(RepoError::Db)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::db::{init_db, DbPool};

    fn make_pool() -> DbPool { DbPool::new(init_db(":memory:").unwrap()) }

    #[test]
    fn insert_and_list() {
        let repo = SqliteDeadLetterRepo::new(make_pool());
        repo.insert(&DeadLetterEntry::new("m1", "outbox", "{}", "max retries", 1000)).unwrap();
        assert_eq!(repo.list(10).unwrap().len(), 1);
    }

    #[test]
    fn replay_moves_back_to_outbox() {
        let repo = SqliteDeadLetterRepo::new(make_pool());
        repo.insert(&DeadLetterEntry::new("m1", "outbox", "hello", "max retries", 1000)).unwrap();
        repo.replay("m1").unwrap();
        assert!(repo.list(10).unwrap().is_empty());
    }
}
