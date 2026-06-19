use serde::{Deserialize, Serialize};

use crate::domain::value_objects::route_key::RouteKey;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub route_key: RouteKey,
    pub sequence: Option<i64>,
    pub timestamp_ms: i64,
    pub direction: Direction,
    pub content: MessageContent,
    pub audit_mark: Option<AuditMark>,
}

impl Message {
    pub fn new_inbound(
        id: impl Into<String>,
        route_key: RouteKey,
        sequence: Option<i64>,
        timestamp_ms: i64,
        content: MessageContent,
    ) -> Self {
        Self {
            id: id.into(),
            route_key,
            sequence,
            timestamp_ms,
            direction: Direction::Inbound,
            content,
            audit_mark: None,
        }
    }

    /// The sort key used by ReorderWindow: prefer sequence, fall back to timestamp.
    pub fn sort_key(&self) -> i64 {
        self.sequence.unwrap_or(self.timestamp_ms)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Inbound,
    Outbound,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum MessageContent {
    Text(String),
    #[allow(dead_code)]
    Image { url: String, media_id: Option<String> },
    #[allow(dead_code)]
    File { url: String, name: String, size: u64 },
    #[allow(dead_code)]
    Unknown,
}

/// Audit markers for messages that were handled in a non-standard way.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AuditMark {
    /// Message arrived after the reorder window had closed.
    LateArrival { delay_ms: u64 },
    /// Message was identified as a duplicate.
    Duplicate,
    /// Message arrived out of the expected sequence order.
    OutOfOrder { expected_seq: i64, actual_seq: i64 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::value_objects::route_key::{ChannelId, ConversationType};

    fn make_route_key() -> RouteKey {
        RouteKey::new(
            ChannelId::new("wechat"),
            "conv_001",
            "user_a",
            ConversationType::Direct,
        )
    }

    #[test]
    fn sort_key_prefers_sequence() {
        let msg = Message::new_inbound(
            "msg_1",
            make_route_key(),
            Some(42),
            1000,
            MessageContent::Text("hello".into()),
        );
        assert_eq!(msg.sort_key(), 42);
    }

    #[test]
    fn sort_key_falls_back_to_timestamp() {
        let msg = Message::new_inbound(
            "msg_1",
            make_route_key(),
            None,
            1000,
            MessageContent::Text("hello".into()),
        );
        assert_eq!(msg.sort_key(), 1000);
    }
}
