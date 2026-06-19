use crate::domain::ports::inbox_repo::{InboxRepo, RepoError};
use crate::domain::storage::inbox::{InboxEntry, InboxStatus};
use crate::domain::entities::message::Message;

/// Process an inbound message: write to inbox for idempotency, then process.
pub fn process_inbound(
    inbox: &dyn InboxRepo,
    msg: &Message,
) -> Result<InboxResult, RepoError> {
    if inbox.exists(&msg.id)? {
        return Ok(InboxResult::Duplicate);
    }

    let now = chrono::Utc::now().timestamp();
    let payload = serde_json::to_string(msg).unwrap_or_default();

    let entry = InboxEntry::new(
        &msg.id,
        msg.route_key.channel.as_str(),
        &msg.route_key.conversation_id,
        payload,
        now,
    );

    // Insert with INSERT OR IGNORE — if duplicate, we're done
    inbox.insert(&entry)?;

    inbox.mark_status(&msg.id, InboxStatus::Processing)?;

    // In Phase 4 (Pipeline), this is where the full middleware chain runs.
    // For Phase 2, processing is just the status update.
    inbox.mark_status(&msg.id, InboxStatus::Processed)?;

    Ok(InboxResult::Processed)
}

pub enum InboxResult {
    Processed,
    Duplicate,
}
