use crate::domain::ports::dedup_cache::DedupCache;

/// Moka-based deduplication cache with TTL and max capacity.
pub struct MokaDedupCache {
    cache: moka::sync::Cache<String, ()>,
}

impl MokaDedupCache {
    pub fn new(ttl_secs: u64, max_capacity: u64) -> Self {
        Self {
            cache: moka::sync::Cache::builder()
                .time_to_live(std::time::Duration::from_secs(ttl_secs))
                .max_capacity(max_capacity)
                .build(),
        }
    }
}

impl DedupCache for MokaDedupCache {
    fn check_and_set(&self, channel: &str, msg_id: &str) -> bool {
        let key = format!("{}:{}", channel, msg_id);
        // Use get_with for atomic check-and-set.
        // The closure runs only on cache miss (atomic within moka's internal lock).
        // On cache hit the closure is skipped and the existing value is returned.
        let mut is_new = false;
        self.cache.get_with(key, || {
            is_new = true;
        });
        is_new
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_message_returns_true() {
        let cache = MokaDedupCache::new(300, 10_000);
        assert!(cache.check_and_set("wechat", "msg_001"));
    }

    #[test]
    fn duplicate_message_returns_false() {
        let cache = MokaDedupCache::new(300, 10_000);
        assert!(cache.check_and_set("wechat", "msg_001"));
        assert!(!cache.check_and_set("wechat", "msg_001"));
    }

    #[test]
    fn different_channels_independent() {
        let cache = MokaDedupCache::new(300, 10_000);
        assert!(cache.check_and_set("wechat", "msg_001"));
        assert!(cache.check_and_set("dingtalk", "msg_001"));
    }
}
