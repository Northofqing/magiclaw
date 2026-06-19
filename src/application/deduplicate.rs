use crate::domain::ports::dedup_cache::DedupCache;

/// Application use case: deduplicate an inbound message.
/// Returns true if the message should be processed (not a duplicate).
pub fn deduplicate(
    cache: &dyn DedupCache,
    channel: &str,
    msg_id: &str,
) -> bool {
    cache.check_and_set(channel, msg_id)
}
