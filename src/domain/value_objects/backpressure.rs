use serde::{Deserialize, Serialize};

/// Backpressure configuration for inbound and per-route channels.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackpressureConfig {
    /// Max messages in a per-conversation queue. Default 256.
    pub per_route_buffer: usize,
    /// Max messages in the global inbound channel. Default 4096.
    pub inbound_channel_capacity: usize,
}

impl Default for BackpressureConfig {
    fn default() -> Self {
        Self {
            per_route_buffer: 256,
            inbound_channel_capacity: 4096,
        }
    }
}

/// Action taken when a channel is full and backpressure triggers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackpressureAction {
    /// Drop the oldest message in the queue.
    DropOldest,
    /// Drop the message being enqueued right now.
    DropNewest,
    /// Block the producer until space is available (send path only).
    Block,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_backpressure_config() {
        let config = BackpressureConfig::default();
        assert_eq!(config.per_route_buffer, 256);
        assert_eq!(config.inbound_channel_capacity, 4096);
    }
}
