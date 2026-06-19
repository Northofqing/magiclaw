use serde::Serialize;

use crate::channels::registry::ChannelRegistry;
use crate::domain::ports::conversation_queue::ConversationQueue;

#[derive(Debug, Serialize)]
pub struct SystemHealth {
    pub status: &'static str,
    pub uptime_secs: u64,
    pub active_conversations: usize,
    pub channels: Vec<crate::channels::channel_trait::HealthStatus>,
}

/// Collect system-wide health status.
pub async fn system_health(
    start_time: std::time::Instant,
    queue: &dyn ConversationQueue,
    registry: Option<&ChannelRegistry>,
) -> SystemHealth {
    let channels = match registry {
        Some(reg) => reg.health_check_all().await,
        None => Vec::new(),
    };

    let all_healthy = channels.iter().all(|h| h.healthy);

    SystemHealth {
        status: if all_healthy { "healthy" } else { "degraded" },
        uptime_secs: start_time.elapsed().as_secs(),
        active_conversations: queue.active_conversations(),
        channels,
    }
}
