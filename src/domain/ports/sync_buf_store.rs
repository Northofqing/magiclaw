use thiserror::Error;

#[derive(Debug, Error)]
pub enum SyncBufError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Db(String),
}

/// Persistent storage for WeChat sync_buf.
pub trait SyncBufStore: Send + Sync {
    /// Save the sync buffer for a given channel and account.
    fn save(&self, channel: &str, account: &str, buf: &[u8]) -> Result<(), SyncBufError>;

    /// Load the sync buffer for a given channel and account.
    fn load(&self, channel: &str, account: &str) -> Result<Vec<u8>, SyncBufError>;
}
