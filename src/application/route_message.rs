use crate::domain::entities::message::Message;
use crate::domain::ports::conversation_queue::{ConversationQueue, EnqueueError};
use crate::domain::ports::dedup_cache::DedupCache;

/// Result of routing a message.
pub enum RouteOutcome {
    Enqueued,
    Duplicate,
    Dropped(String),
}

/// Route an inbound message: dedup then enqueue to the appropriate conversation.
pub fn route_message(
    dedup: &dyn DedupCache,
    queue: &dyn ConversationQueue,
    msg: Message,
) -> RouteOutcome {
    if !dedup.check_and_set(msg.route_key.channel.as_str(), &msg.id) {
        tracing::debug!(message_id = %msg.id, "duplicate message dropped");
        return RouteOutcome::Duplicate;
    }

    let key = msg.route_key.clone();
    match queue.enqueue(&key, msg) {
        Ok(()) => RouteOutcome::Enqueued,
        Err(EnqueueError::QueueFull { message_id }) => {
            tracing::warn!(
                message_id = %message_id,
                conversation_id = %key.conversation_id,
                "route queue full, message dropped"
            );
            RouteOutcome::Dropped(message_id)
        }
    }
}
