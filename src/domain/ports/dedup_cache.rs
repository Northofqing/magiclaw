/// Deduplication cache. Used to detect and filter duplicate messages.
pub trait DedupCache: Send + Sync {
    /// Check if a message is new, and mark it as seen.
    /// Returns `true` if the message is new (cache miss), `false` if duplicate (cache hit).
    fn check_and_set(&self, channel: &str, msg_id: &str) -> bool;
}
