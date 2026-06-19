use crate::domain::ports::outbox_repo::OutboxRepo;
use crate::domain::storage::outbox::OutboxStatus;

/// Recover messages that were in-flight (sending/retrying) at crash time.
/// Resets them back to 'pending' so the outbox worker picks them up again.
pub fn recover_after_crash(outbox: &dyn OutboxRepo) -> Result<usize, String> {
    let entries = outbox.recover_after_crash().map_err(|e| e.to_string())?;
    let count = entries.len();

    for entry in &entries {
        tracing::info!(
            message_id = %entry.id,
            status = %entry.status.as_str(),
            "crash recovery: resetting in-flight message to pending"
        );
        outbox.mark_status(&entry.id, OutboxStatus::Pending, None)
            .map_err(|e| e.to_string())?;
    }

    if count > 0 {
        tracing::info!(recovered = count, "crash recovery complete");
    }

    Ok(count)
}
