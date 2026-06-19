use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::channels::channel_trait::{Channel, HealthStatus, SendReceipt};
use crate::domain::entities::message::{Message, MessageContent};
use crate::domain::value_objects::route_key::ChannelId;

/// Feishu (Lark) channel skeleton. Full implementation in Phase 4-5.
pub struct FeishuChannel {
    channel_id: ChannelId,
}

impl Default for FeishuChannel {
    fn default() -> Self { Self::new() }
}

impl FeishuChannel {
    pub fn new() -> Self {
        Self { channel_id: ChannelId::new("feishu") }
    }
}

#[async_trait]
impl Channel for FeishuChannel {
    fn id(&self) -> ChannelId {
        self.channel_id.clone()
    }

    async fn start(&self, _inbound_tx: mpsc::Sender<Message>) -> Result<(), String> {
        tracing::info!("Feishu channel started (skeleton)");
        Ok(())
    }

    async fn send_message(&self, to: &str, content: &MessageContent) -> Result<SendReceipt, String> {
        let body = match content {
            MessageContent::Text(t) => t.clone(),
            _ => format!("{:?}", content),
        };
        tracing::info!(to = %to, body = %body, "Feishu send_message (skeleton)");
        Ok(SendReceipt {
            message_id: uuid::Uuid::new_v4().to_string(),
            platform_msg_id: Some(format!("feishu_stub_{}", chrono::Utc::now().timestamp_millis())),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
        })
    }

    async fn stop(&self) -> Result<(), String> {
        tracing::info!("Feishu channel stopped");
        Ok(())
    }

    async fn health(&self) -> Result<HealthStatus, String> {
        Ok(HealthStatus {
            channel: "feishu".into(),
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
    async fn feishu_health_returns_skeleton_status() {
        let ch = FeishuChannel::new();
        let h = ch.health().await.unwrap();
        assert!(h.healthy);
        assert!(h.detail.contains("skeleton"));
    }

    #[tokio::test]
    async fn feishu_send_returns_stub_receipt() {
        let ch = FeishuChannel::new();
        let receipt = ch.send_message("user1", &MessageContent::Text("hi".into())).await.unwrap();
        assert!(receipt.platform_msg_id.unwrap().starts_with("feishu_stub_"));
    }
}
