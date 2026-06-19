use crate::domain::ports::dead_letter_repo::DeadLetterRepo;
use crate::domain::storage::dead_letter::DeadLetterEntry;

/// List dead letter entries.
pub fn list_dead_letters(dlq: &dyn DeadLetterRepo, limit: usize) -> Result<Vec<DeadLetterEntry>, String> {
    dlq.list(limit).map_err(|e| e.to_string())
}

/// Replay a dead letter entry back to the outbox.
pub fn replay_dead_letter(dlq: &dyn DeadLetterRepo, id: &str) -> Result<(), String> {
    let _entry = dlq.replay(id).map_err(|e| e.to_string())?;
    tracing::info!(dead_letter_id = %id, "replayed to outbox");
    Ok(())
}
