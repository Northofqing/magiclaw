use crate::domain::entities::message::Message;
use crate::domain::value_objects::route_key::RouteKey;

/// Result of a queue enqueue operation.
pub type EnqueueResult = Result<(), EnqueueError>;

#[derive(Debug, Clone, thiserror::Error)]
pub enum EnqueueError {
    #[error("queue full: dropped message {message_id}")]
    QueueFull { message_id: String },
}

/// Conversation processing queue — abstraction over the runtime projection
/// of a Conversation aggregate. Does not expose mpsc details.
pub trait ConversationQueue: Send + Sync {
    /// Enqueue a message to the conversation's processing queue.
    /// Returns an error with the dropped message ID if the queue is full.
    fn enqueue(&self, key: &RouteKey, msg: Message) -> EnqueueResult;

    /// Number of active conversations.
    fn active_conversations(&self) -> usize;
}

/// GC Janitor abstraction — periodically scans and reclaims idle conversations.
pub trait ConversationGC: Send + Sync {
    /// Scan all conversations and reclaim those idle longer than `idle_timeout`.
    /// Returns the count of reclaimed conversations.
    fn collect_idle(&self, idle_timeout_secs: u64) -> usize;
}
