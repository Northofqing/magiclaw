use crate::domain::storage::dead_letter::DeadLetterEntry;
use crate::domain::storage::outbox::OutboxEntry;

use super::inbox_repo::RepoResult;

pub trait DeadLetterRepo: Send + Sync {
    fn insert(&self, entry: &DeadLetterEntry) -> RepoResult<()>;
    fn list(&self, limit: usize) -> RepoResult<Vec<DeadLetterEntry>>;
    /// Move an entry back to outbox for replay.
    fn replay(&self, id: &str) -> RepoResult<OutboxEntry>;
}
