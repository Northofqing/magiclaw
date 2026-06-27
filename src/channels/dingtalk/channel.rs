use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::channels::channel_trait::{Channel, HealthStatus, SendReceipt};
use crate::domain::entities::message::{Message, MessageContent};
use crate::domain::error::ChannelError;
use crate::domain::value_objects::route_key::ChannelId;

/// Dingtalk channel skeleton. Full implementation in Phase 4-5.
pub struct DingtalkChannel {
    channel_id: ChannelId,
}

impl Default for DingtalkChannel {
    fn default() -> Self { Self::new() }
}

impl DingtalkChannel {
    pub fn new() -> Self {
        Self { channel_id: ChannelId::new("dingtalk") }
    }
}

#[async_trait]
impl Channel for DingtalkChannel {
    fn id(&self) -> ChannelId {
        self.channel_id.clone()
    }

    async fn start(&self, _inbound_tx: mpsc::Sender<Message>) -> Result<(), ChannelError> {
        tracing::info!("Dingtalk channel started (skeleton)");
        // Phase 4: connect to Dingtalk webhook/long-poll
        Ok(())
    }

    async fn send_message(&self, to: &str, content: &MessageContent) -> Result<SendReceipt, ChannelError> {
        let body = match content {
            MessageContent::Text(t) => t.clone(),
            _ => format!("{:?}", content),
        };
        tracing::info!(to = %to, body = %body, "Dingtalk send_message (skeleton)");
        // Phase 4: call Dingtalk API
        Ok(SendReceipt {
            message_id: uuid::Uuid::new_v4().to_string(),
            platform_msg_id: Some(format!("dingtalk_stub_{}", chrono::Utc::now().timestamp_millis())),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
        })
    }

    async fn stop(&self) -> Result<(), ChannelError> {
        tracing::info!("Dingtalk channel stopped");
        Ok(())
    }

    async fn health(&self) -> Result<HealthStatus, ChannelError> {
        Ok(HealthStatus {
            channel: "dingtalk".into(),
            healthy: true,
            detail: "skeleton — not connected to platform".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entities::message::MessageContent;

    #[tokio::test]
    async fn dingtalk_health_returns_skeleton_status() {
        let ch = DingtalkChannel::new();
        let h = ch.health().await.unwrap();
        assert!(h.healthy);
        assert!(h.detail.contains("skeleton"));
    }

    #[tokio::test]
    async fn dingtalk_send_returns_stub_receipt() {
        let ch = DingtalkChannel::new();
        let receipt = ch.send_message("user1", &MessageContent::Text("hi".into())).await.unwrap();
        assert!(receipt.platform_msg_id.unwrap().starts_with("dingtalk_stub_"));
    }
}
