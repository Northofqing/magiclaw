use crate::domain::aggregates::conversation::Conversation;
use crate::domain::value_objects::route_key::{ConversationType, RouteKey};

/// Read-only snapshot of a Conversation aggregate for use in the pipeline.
/// Prevents the pipeline from modifying aggregate state through a clone.
#[derive(Debug, Clone)]
pub struct ConversationSnapshot {
    pub route_key: RouteKey,
    pub conversation_id: String,
    pub peer_id: String,
    pub conversation_type: ConversationType,
    pub participants: Vec<String>,
    pub message_count: u64,
    pub last_active_secs: u64,
}

impl ConversationSnapshot {
    pub fn from_conversation(conv: &Conversation, message_count: u64) -> Self {
        Self {
            route_key: conv.route_key.clone(),
            conversation_id: conv.route_key.conversation_id.clone(),
            peer_id: conv.route_key.peer_id.clone(),
            conversation_type: conv.route_key.conversation_type.clone(),
            participants: conv.participants.clone(),
            message_count,
            last_active_secs: conv.last_active.elapsed().as_secs(),
        }
    }
}
