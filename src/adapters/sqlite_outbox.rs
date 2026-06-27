use crate::domain::ports::inbox_repo::{RepoError, RepoResult};
use crate::domain::ports::outbox_repo::OutboxRepo;
use crate::infrastructure::storage::outbox::{OutboxEntry, OutboxStatus};
use crate::infrastructure::db::DbPool;

pub struct SqliteOutboxRepo {
    db: DbPool,
}

impl SqliteOutboxRepo {
    pub fn new(db: DbPool) -> Self { Self { db } }
}

fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<OutboxEntry> {
    Ok(OutboxEntry {
        id: row.get(0)?,
        route_key: row.get(1)?,
        payload: row.get(2)?,
        status: OutboxStatus::parse(&row.get::<_, String>(3)?).unwrap_or(OutboxStatus::Pending),
        retry_count: row.get(4)?,
        next_retry_at: row.get(5)?,
        last_error: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn fetch_entries(conn: &rusqlite::Connection, sql: &str, params: &[Box<dyn rusqlite::types::ToSql>]) -> rusqlite::Result<Vec<OutboxEntry>> {
    let mut stmt = conn.prepare(sql)?;
    let rows: Result<Vec<_>, _> = stmt.query_map(
        rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
        row_to_entry,
    )?.collect();
    rows
}

impl OutboxRepo for SqliteOutboxRepo {
    fn insert(&self, entry: &OutboxEntry) -> RepoResult<()> {
        let entry = entry.clone();
        self.db.execute(move |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO outbox (id, route_key, payload, status, retry_count, next_retry_at, last_error, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![entry.id, entry.route_key, entry.payload, entry.status.as_str(), entry.retry_count, entry.next_retry_at, entry.last_error, entry.created_at, entry.updated_at],
            )?;
            Ok(())
        }).map_err(RepoError::Db)
    }

    fn mark_status(&self, id: &str, status: OutboxStatus, error: Option<&str>) -> RepoResult<()> {
        let id = id.to_string();
        let s = status.as_str().to_string();
        let err = error.map(|e| e.to_string());
        self.db.execute(move |conn| {
            conn.execute("UPDATE outbox SET status = ?1, last_error = ?2, updated_at = unixepoch() WHERE id = ?3", rusqlite::params![s, err, id])?;
            Ok(())
        }).map_err(RepoError::Db)
    }

    fn mark_retrying(&self, id: &str, retry_count: u32, next_retry_at: i64, error: &str) -> RepoResult<()> {
        let id = id.to_string();
        let err = error.to_string();
        self.db.execute(move |conn| {
            conn.execute("UPDATE outbox SET status='retrying', retry_count=?1, next_retry_at=?2, last_error=?3, updated_at=unixepoch() WHERE id=?4",
                rusqlite::params![retry_count, next_retry_at, err, id])?;
            Ok(())
        }).map_err(RepoError::Db)
    }

    fn fetch_pending(&self, limit: usize) -> RepoResult<Vec<OutboxEntry>> {
        let db = self.db.clone();
        db.query(move |conn| {
            fetch_entries(conn, "SELECT id, route_key, payload, status, retry_count, next_retry_at, last_error, created_at, updated_at FROM outbox WHERE status='pending' ORDER BY created_at ASC LIMIT ?1",
                &[Box::new(limit as i64)])
        }).map_err(RepoError::Db)
    }

    fn fetch_retryable(&self, now_ts: i64, limit: usize) -> RepoResult<Vec<OutboxEntry>> {
        let db = self.db.clone();
        db.query(move |conn| {
            fetch_entries(conn, "SELECT id, route_key, payload, status, retry_count, next_retry_at, last_error, created_at, updated_at FROM outbox WHERE status='retrying' AND next_retry_at <= ?1 ORDER BY next_retry_at ASC LIMIT ?2",
                &[Box::new(now_ts), Box::new(limit as i64)])
        }).map_err(RepoError::Db)
    }

    fn recover_after_crash(&self) -> RepoResult<Vec<OutboxEntry>> {
        let db = self.db.clone();
        db.query(move |conn| {
            fetch_entries(conn, "SELECT id, route_key, payload, status, retry_count, next_retry_at, last_error, created_at, updated_at FROM outbox WHERE status IN ('sending','retrying') ORDER BY created_at ASC",
                &[])
        }).map_err(RepoError::Db)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::db::{init_db, DbPool};

    fn make_pool() -> DbPool { DbPool::new(init_db(":memory:").unwrap()) }

    #[test]
    fn full_state_machine() {
        let repo = SqliteOutboxRepo::new(make_pool());
        repo.insert(&OutboxEntry::new("m1", "wc/c1", "hi", 1000)).unwrap();
        repo.mark_status("m1", OutboxStatus::Sending, None).unwrap();
        repo.mark_status("m1", OutboxStatus::Sent, None).unwrap();
    }

    #[test]
    fn retry_flow() {
        let repo = SqliteOutboxRepo::new(make_pool());
        repo.insert(&OutboxEntry::new("m2", "wc/c2", "hi", 1000)).unwrap();
        repo.mark_retrying("m2", 1, 2000, "timeout").unwrap();
        let retryable = repo.fetch_retryable(3000, 10).unwrap();
        assert_eq!(retryable.len(), 1);
    }

    #[test]
    fn crash_recovery_finds_inflight() {
        let repo = SqliteOutboxRepo::new(make_pool());
        repo.insert(&OutboxEntry::new("m1", "wc/c1", "a", 1000)).unwrap();
        repo.insert(&OutboxEntry::new("m2", "wc/c2", "b", 1000)).unwrap();
        repo.mark_status("m1", OutboxStatus::Sending, None).unwrap();
        repo.mark_status("m2", OutboxStatus::Retrying, None).unwrap();
        assert_eq!(repo.recover_after_crash().unwrap().len(), 2);
    }
}
