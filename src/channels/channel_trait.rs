use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::domain::entities::message::{Message, MessageContent};
use crate::domain::error::ChannelError;
use crate::domain::value_objects::route_key::ChannelId;

/// Receipt returned after a successful send.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendReceipt {
    pub message_id: String,
    pub platform_msg_id: Option<String>,
    pub timestamp_ms: i64,
}

/// Health status of a channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub channel: String,
    pub healthy: bool,
    pub detail: String,
}

/// Common interface for all messaging channel implementations.
#[async_trait]
pub trait Channel: Send + Sync {
    /// Unique channel identifier.
    fn id(&self) -> ChannelId;

    /// Start the channel. It pushes inbound messages to `inbound_tx`.
    async fn start(&self, inbound_tx: mpsc::Sender<Message>) -> Result<(), ChannelError>;

    /// Send a message to a recipient.
    async fn send_message(&self, to: &str, content: &MessageContent) -> Result<SendReceipt, ChannelError>;

    /// Gracefully stop the channel.
    async fn stop(&self) -> Result<(), ChannelError>;

    /// Health check.
    async fn health(&self) -> Result<HealthStatus, ChannelError> {
        Ok(HealthStatus {
            channel: self.id().to_string(),
            healthy: true,
            detail: "ok".into(),
        })
    }
}
