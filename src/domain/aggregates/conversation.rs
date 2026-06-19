use std::time::Instant;

use crate::domain::entities::message::Message;
use crate::domain::services::reorder_window::ReorderWindow;
use crate::domain::value_objects::route_key::RouteKey;

/// Conversation is the aggregate root. It owns the ReorderWindow and
/// tracks its own activity for idle GC.
#[derive(Debug, Clone)]
pub struct Conversation {
    pub route_key: RouteKey,
    pub participants: Vec<String>,
    pub state: ConversationState,
    pub reorder_window: ReorderWindow,
    pub last_active: Instant,
}

impl Conversation {
    pub fn new(route_key: RouteKey, reorder_window_ms: u64) -> Self {
        Self {
            route_key,
            participants: Vec::new(),
            state: ConversationState::Active,
            reorder_window: ReorderWindow::new(reorder_window_ms),
            last_active: Instant::now(),
        }
    }

    /// Record activity and update state to Active if idle.
    pub fn touch(&mut self) {
        self.last_active = Instant::now();
        if self.state == ConversationState::Idle {
            self.state = ConversationState::Active;
        }
    }

    /// Insert a message into the reorder window and return ready messages.
    pub fn ingest(&mut self, msg: Message) -> Vec<Message> {
        self.touch();
        self.reorder_window.insert(msg)
    }

    /// Check if this conversation has been idle longer than the given duration.
    pub fn is_idle(&self, timeout: std::time::Duration) -> bool {
        self.last_active.elapsed() > timeout
    }

    /// Transition to idle state.
    pub fn mark_idle(&mut self) {
        self.state = ConversationState::Idle;
    }

    /// Flush all remaining messages before GC.
    pub fn drain(&mut self) -> Vec<Message> {
        self.reorder_window.flush_all()
    }

    /// Whether the reorder window still has buffered messages.
    pub fn has_pending(&self) -> bool {
        !self.reorder_window.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversationState {
    Active,
    Idle,
    Closed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entities::message::{Direction, MessageContent};
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
    fn new_conversation_is_active() {
        let conv = Conversation::new(make_route_key(), 200);
        assert_eq!(conv.state, ConversationState::Active);
        assert!(conv.reorder_window.is_empty());
    }

    #[test]
    fn touch_updates_last_active() {
        let mut conv = Conversation::new(make_route_key(), 200);
        // Can't easily test Instant, but we can verify it doesn't panic
        conv.ingest(Message {
            id: "m1".into(),
            route_key: make_route_key(),
            sequence: Some(1),
            timestamp_ms: 100,
            direction: Direction::Inbound,
            content: MessageContent::Text("hello".into()),
            audit_mark: None,
        });
        assert!(!conv.is_idle(std::time::Duration::from_secs(1)));
    }

    #[test]
    fn mark_idle_transitions_state() {
        let mut conv = Conversation::new(make_route_key(), 200);
        conv.mark_idle();
        assert_eq!(conv.state, ConversationState::Idle);
    }

    #[test]
    fn idle_to_active_on_touch() {
        let mut conv = Conversation::new(make_route_key(), 200);
        conv.mark_idle();
        conv.touch();
        assert_eq!(conv.state, ConversationState::Active);
    }
}
