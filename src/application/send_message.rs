use crate::domain::entities::message::{Direction, Message, MessageContent};
use crate::domain::ports::conversation_queue::{ConversationQueue, EnqueueError};
use crate::domain::ports::outbox_repo::OutboxRepo;
use crate::domain::storage::outbox::OutboxEntry;
use crate::domain::value_objects::route_key::{ChannelId, ConversationType, RouteKey};

/// Use case: construct and route an outbound text message.
pub fn send_text(
    queue: &dyn ConversationQueue,
    channel: &str,
    conversation_id: &str,
    peer_id: &str,
    conversation_type: ConversationType,
    content: &str,
) -> Result<(), SendError> {
    let route_key = RouteKey::new(
        ChannelId::new(channel),
        conversation_id,
        peer_id,
        conversation_type,
    );

    let msg = Message {
        id: uuid::Uuid::new_v4().to_string(),
        route_key: route_key.clone(),
        sequence: None,
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        direction: Direction::Outbound,
        content: MessageContent::Text(content.to_string()),
        audit_mark: None,
    };

    match queue.enqueue(&route_key, msg) {
        Ok(()) => Ok(()),
        Err(EnqueueError::QueueFull { message_id }) => Err(SendError::QueueFull {
            message_id,
            conversation_id: conversation_id.to_string(),
        }),
    }
}

/// Use case: persist an outbound message into Outbox for recoverable delivery.
pub fn submit_outbound_for_delivery(
    outbox: &dyn OutboxRepo,
    channel: &str,
    conversation_id: &str,
    peer_id: &str,
    conversation_type: ConversationType,
    content: MessageContent,
) -> Result<String, SendError> {
    let route_key = RouteKey::new(
        ChannelId::new(channel),
        conversation_id,
        peer_id,
        conversation_type,
    );

    let message_id = uuid::Uuid::new_v4().to_string();
    let route_key_json = serde_json::to_string(&route_key)
        .map_err(|e| SendError::Serialization(e.to_string()))?;
    let payload_json = serde_json::to_string(&content)
        .map_err(|e| SendError::Serialization(e.to_string()))?;
    let now = chrono::Utc::now().timestamp();
    let entry = OutboxEntry::new(&message_id, route_key_json, payload_json, now);

    outbox.insert(&entry).map_err(|e| SendError::Persist(e.to_string()))?;
    Ok(message_id)
}

/// Use case: persist an outbound text message into Outbox for recoverable delivery.
pub fn submit_text_for_delivery(
    outbox: &dyn OutboxRepo,
    channel: &str,
    conversation_id: &str,
    peer_id: &str,
    conversation_type: ConversationType,
    content: &str,
) -> Result<String, SendError> {
    submit_outbound_for_delivery(
        outbox,
        channel,
        conversation_id,
        peer_id,
        conversation_type,
        MessageContent::Text(content.to_string()),
    )
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("queue full: message {message_id} to {conversation_id}")]
    QueueFull {
        message_id: String,
        conversation_id: String,
    },
    #[error("failed to persist outbound message: {0}")]
    Persist(String),
    #[error("failed to serialize outbound message: {0}")]
    Serialization(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::conversation_store::ConversationStore;
    use crate::adapters::sqlite_outbox::SqliteOutboxRepo;
    use crate::infrastructure::config::AppConfig;
    use crate::infrastructure::db::{init_db, DbPool};

    #[tokio::test]
    async fn send_text_constructs_correct_route_key() {
        let store = ConversationStore::new(256, 1800, 200, None, AppConfig::default(), None);
        let result = send_text(
            store.as_ref(),
            "wechat",
            "conv_001",
            "user_a",
            ConversationType::Direct,
            "hello from MCP",
        );
        assert!(result.is_ok());
        assert_eq!(store.active_conversations(), 1);
    }

    #[test]
    fn submit_text_for_delivery_persists_pending_outbox_entry() {
        let repo = SqliteOutboxRepo::new(DbPool::new(init_db(":memory:").unwrap()));

        let message_id = submit_text_for_delivery(
            &repo,
            "wechat",
            "conv_001",
            "user_a",
            ConversationType::Direct,
            "hello from MCP",
        )
        .unwrap();

        let pending = repo.fetch_pending(10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, message_id);

        let route_key: RouteKey = serde_json::from_str(&pending[0].route_key).unwrap();
        assert_eq!(route_key.channel.as_str(), "wechat");
        assert_eq!(route_key.conversation_id, "conv_001");
        assert_eq!(route_key.peer_id, "user_a");

        let payload: MessageContent = serde_json::from_str(&pending[0].payload).unwrap();
        assert!(matches!(payload, MessageContent::Text(ref body) if body == "hello from MCP"));
    }

    #[test]
    fn submit_outbound_for_delivery_persists_media_payloads() {
        let repo = SqliteOutboxRepo::new(DbPool::new(init_db(":memory:").unwrap()));

        let message_id = submit_outbound_for_delivery(
            &repo,
            "wechat",
            "conv_001",
            "user_a",
            ConversationType::Direct,
            MessageContent::Image {
                url: "https://example.invalid/image.png".into(),
                media_id: Some("media-1".into()),
            },
        )
        .unwrap();

        let pending = repo.fetch_pending(10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, message_id);

        let payload: MessageContent = serde_json::from_str(&pending[0].payload).unwrap();
        assert!(matches!(payload, MessageContent::Image { ref url, ref media_id } if url == "https://example.invalid/image.png" && media_id.as_deref() == Some("media-1")));
    }
}
