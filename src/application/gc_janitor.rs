use std::time::Duration;

use tokio::time::interval;

use crate::domain::ports::conversation_queue::ConversationGC;

/// Background task that periodically scans and reclaims idle conversations.
pub async fn gc_janitor(gc: &dyn ConversationGC, idle_timeout_secs: u64, scan_interval_secs: u64) {
    let mut ticker = interval(Duration::from_secs(scan_interval_secs));
    loop {
        ticker.tick().await;
        let reclaimed = gc.collect_idle(idle_timeout_secs);
        if reclaimed > 0 {
            tracing::info!(reclaimed, "gc janitor reclaimed idle conversations");
        }
    }
}
