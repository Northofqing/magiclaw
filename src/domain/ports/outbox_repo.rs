use crate::domain::storage::outbox::{OutboxEntry, OutboxStatus};

use super::inbox_repo::RepoResult;

pub trait OutboxRepo: Send + Sync {
    fn insert(&self, entry: &OutboxEntry) -> RepoResult<()>;
    fn mark_status(&self, id: &str, status: OutboxStatus, error: Option<&str>) -> RepoResult<()>;
    fn mark_retrying(&self, id: &str, retry_count: u32, next_retry_at: i64, error: &str) -> RepoResult<()>;
    fn fetch_pending(&self, limit: usize) -> RepoResult<Vec<OutboxEntry>>;
    fn fetch_retryable(&self, now_ts: i64, limit: usize) -> RepoResult<Vec<OutboxEntry>>;
    /// Recover messages that were in sending/retrying state at crash time.
    fn recover_after_crash(&self) -> RepoResult<Vec<OutboxEntry>>;
}
