use crate::domain::ports::sync_buf_store::{SyncBufError, SyncBufStore};

/// WeChat session state with persistent sync_buf.
pub struct WeChatSession<S: SyncBufStore> {
    channel: String,
    account: String,
    store: S,
    /// The current sync buffer. Every update is immediately persisted.
    sync_buf: Vec<u8>,
    /// Whether the session has been restored from persistent storage.
    restored: bool,
}

impl<S: SyncBufStore> WeChatSession<S> {
    /// Create a new session. Attempts to restore from persistent storage first.
    pub fn new(channel: impl Into<String>, account: impl Into<String>, store: S) -> Result<Self, SyncBufError> {
        let channel = channel.into();
        let account = account.into();

        let sync_buf = store.load(&channel, &account)?;
        let restored = !sync_buf.is_empty();

        if restored {
            tracing::info!(
                channel = %channel,
                account = %account,
                buf_len = sync_buf.len(),
                "sync_buf restored from persistent storage"
            );
        }

        Ok(Self {
            channel,
            account,
            store,
            sync_buf,
            restored,
        })
    }

    /// Whether this session was restored from persistent storage.
    pub fn was_restored(&self) -> bool {
        self.restored
    }

    /// Get the current sync buffer.
    pub fn sync_buf(&self) -> &[u8] {
        &self.sync_buf
    }

    /// Update the sync buffer. This immediately persists to storage.
    pub fn update_sync_buf(&mut self, buf: Vec<u8>) -> Result<(), SyncBufError> {
        self.store.save(&self.channel, &self.account, &buf)?;
        self.sync_buf = buf;
        Ok(())
    }

    /// Append data to the sync buffer and persist.
    pub fn append_to_sync_buf(&mut self, data: &[u8]) -> Result<(), SyncBufError> {
        let mut new_buf = self.sync_buf.clone();
        new_buf.extend_from_slice(data);
        self.update_sync_buf(new_buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::sqlite_sync_buf::SqliteSyncBufStore;

    #[test]
    fn new_session_restores_from_store() {
        let store = SqliteSyncBufStore::open(":memory:").unwrap();
        store.save("wechat", "test_account", b"restored_buf").unwrap();

        let session = WeChatSession::new("wechat", "test_account", store).unwrap();
        assert!(session.was_restored());
        assert_eq!(session.sync_buf(), b"restored_buf");
    }

    #[test]
    fn new_session_starts_empty_when_no_saved_state() {
        let store = SqliteSyncBufStore::open(":memory:").unwrap();
        let session = WeChatSession::new("wechat", "empty_account", store).unwrap();
        assert!(!session.was_restored());
        assert!(session.sync_buf().is_empty());
    }

    #[test]
    fn update_sync_buf_persists() {
        let store = SqliteSyncBufStore::open(":memory:").unwrap();
        let mut session = WeChatSession::new("wechat", "acc", store).unwrap();

        session.update_sync_buf(b"new_buf".to_vec()).unwrap();
        assert_eq!(session.sync_buf(), b"new_buf");

        // Verify persistence by loading directly
        let loaded = session.store.load("wechat", "acc").unwrap();
        assert_eq!(loaded, b"new_buf");
    }

    #[test]
    fn append_to_sync_buf() {
        let store = SqliteSyncBufStore::open(":memory:").unwrap();
        let mut session = WeChatSession::new("wechat", "acc", store).unwrap();

        session.update_sync_buf(b"hello".to_vec()).unwrap();
        session.append_to_sync_buf(b" world").unwrap();
        assert_eq!(session.sync_buf(), b"hello world");
    }
}
