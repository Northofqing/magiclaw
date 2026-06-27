use crate::infrastructure::storage::inbox::{InboxEntry, InboxStatus};

pub type RepoResult<T> = Result<T, RepoError>;

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("database error: {0}")]
    Db(String),
    #[error("not found: {0}")]
    NotFound(String),
}

pub trait InboxRepo: Send + Sync {
    fn insert(&self, entry: &InboxEntry) -> RepoResult<()>;
    fn mark_status(&self, id: &str, status: InboxStatus) -> RepoResult<()>;
    fn exists(&self, id: &str) -> RepoResult<bool>;
}
