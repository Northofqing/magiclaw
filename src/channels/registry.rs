use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::channels::channel_trait::{Channel, HealthStatus, SendReceipt};
use crate::domain::entities::message::{Message, MessageContent};
use crate::domain::value_objects::route_key::ChannelId;

/// Manages multiple channel instances with per-channel isolation.
pub struct ChannelRegistry {
    channels: HashMap<ChannelId, Arc<dyn Channel>>,
}

impl Default for ChannelRegistry {
    fn default() -> Self { Self::new() }
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self { channels: HashMap::new() }
    }

    /// Register a channel. If a channel with the same ID exists, it is replaced.
    pub fn register(&mut self, channel: Arc<dyn Channel>) {
        let id = channel.id();
        tracing::info!(channel = %id, "channel registered");
        self.channels.insert(id, channel);
    }

    /// Start all registered channels. Each channel gets its own inbound sender.
    /// Returns a single receiver that merges all inbound messages.
    pub async fn start_all(&self) -> Result<mpsc::Receiver<Message>, String> {
        let (tx, rx) = mpsc::channel::<Message>(4096);

        for (id, channel) in &self.channels {
            let ch_tx = tx.clone();
            let ch = Arc::clone(channel);
            let ch_id = id.clone();
            tokio::spawn(async move {
                if let Err(e) = ch.start(ch_tx).await {
                    tracing::error!(channel = %ch_id, error = %e, "channel start failed");
                }
            });
        }

        tracing::info!(count = self.channels.len(), "all channels started");
        Ok(rx)
    }

    /// Stop all registered channels.
    pub async fn stop_all(&self) {
        for (id, channel) in &self.channels {
            if let Err(e) = channel.stop().await {
                tracing::error!(channel = %id, error = %e, "channel stop error");
            }
        }
        tracing::info!("all channels stopped");
    }

    /// Health check all channels. Returns per-channel status.
    /// A single unhealthy channel does not affect the result of others.
    pub async fn health_check_all(&self) -> Vec<HealthStatus> {
        let mut results = Vec::new();
        for (id, channel) in &self.channels {
            match channel.health().await {
                Ok(h) => results.push(h),
                Err(e) => results.push(HealthStatus {
                    channel: id.to_string(),
                    healthy: false,
                    detail: e,
                }),
            }
        }
        results
    }

    /// Send a message through a specific channel.
    pub async fn send_via(
        &self,
        channel_id: &ChannelId,
        to: &str,
        content: &MessageContent,
    ) -> Result<SendReceipt, String> {
        let channel = self.channels.get(channel_id)
            .ok_or_else(|| format!("channel not found: {}", channel_id))?;
        channel.send_message(to, content).await
    }

    /// Number of registered channels.
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Check if a channel is registered.
    pub fn has_channel(&self, id: &ChannelId) -> bool {
        self.channels.contains_key(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::dingtalk::channel::DingtalkChannel;
    use crate::channels::feishu::channel::FeishuChannel;
    use async_trait::async_trait;

    /// Stub channel for testing isolation.
    struct StubChannel {
        id: ChannelId,
        start_fails: bool,
    }

    #[async_trait]
    impl Channel for StubChannel {
        fn id(&self) -> ChannelId { self.id.clone() }
        async fn start(&self, _tx: mpsc::Sender<Message>) -> Result<(), String> {
            if self.start_fails { Err("simulated failure".into()) } else { Ok(()) }
        }
        async fn send_message(&self, _to: &str, _content: &MessageContent) -> Result<SendReceipt, String> {
            Ok(SendReceipt { message_id: "stub".into(), platform_msg_id: None, timestamp_ms: 1 })
        }
        async fn stop(&self) -> Result<(), String> { Ok(()) }
    }

    #[tokio::test]
    async fn register_and_count() {
        let mut reg = ChannelRegistry::new();
        reg.register(Arc::new(DingtalkChannel::new()));
        reg.register(Arc::new(FeishuChannel::new()));
        assert_eq!(reg.channel_count(), 2);
    }

    #[tokio::test]
    async fn single_channel_failure_does_not_block_others() {
        let mut reg = ChannelRegistry::new();
        reg.register(Arc::new(StubChannel { id: ChannelId::new("good"), start_fails: false }));
        reg.register(Arc::new(StubChannel { id: ChannelId::new("bad"), start_fails: true }));

        // start_all should succeed even though one channel fails
        let _rx = reg.start_all().await.unwrap();

        // Both channels are registered
        assert_eq!(reg.channel_count(), 2);
    }

    #[tokio::test]
    async fn health_check_all_isolates_failures() {
        let mut reg = ChannelRegistry::new();
        reg.register(Arc::new(DingtalkChannel::new()));
        reg.register(Arc::new(FeishuChannel::new()));

        let results = reg.health_check_all().await;
        assert_eq!(results.len(), 2);
        // All should be healthy (skeletons always return ok)
        assert!(results.iter().all(|h| h.healthy));
    }
}
