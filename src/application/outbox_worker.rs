use std::time::Duration;

use async_trait::async_trait;

use crate::domain::ports::audit_sink::AuditSink;
use crate::domain::ports::dead_letter_repo::DeadLetterRepo;
use crate::domain::ports::outbox_repo::OutboxRepo;
use crate::infrastructure::storage::dead_letter::DeadLetterEntry;
use crate::infrastructure::storage::outbox::{OutboxEntry, OutboxStatus, RetryConfig};

#[async_trait]
pub trait OutboxMessageSender: Send + Sync {
    async fn send(&self, entry: &OutboxEntry) -> Result<(), String>;
}

/// Process pending outbox messages: pick up, mark sending, send, mark sent/retrying/dead.
pub async fn process_pending(
    outbox: &dyn OutboxRepo,
    dlq: &dyn DeadLetterRepo,
    retry: &RetryConfig,
    sender: &dyn OutboxMessageSender,
    audit: &dyn AuditSink,
    batch_size: usize,
) {
    let entries = match outbox.fetch_pending(batch_size) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(error = %e, "failed to fetch pending outbox entries");
            return;
        }
    };

    for entry in entries {
        if let Err(e) = outbox.mark_status(&entry.id, OutboxStatus::Sending, None) {
            tracing::error!(message_id = %entry.id, error = %e, "failed to mark sending, skipping");
            continue;
        }

        match sender.send(&entry).await {
            Ok(()) => {
                if let Err(e) = outbox.mark_status(&entry.id, OutboxStatus::Sent, None) {
                    tracing::error!(message_id = %entry.id, error = %e, "failed to mark sent (message already sent)");
                }
                audit.record(Some(&entry.route_key), "send", "sent");
                tracing::debug!(message_id = %entry.id, "outbox message sent");
            }
            Err(err) => {
                let new_count = entry.retry_count + 1;
                if new_count >= retry.max_retries {
                    // Move to dead letter
                    if let Err(e) = outbox.mark_status(&entry.id, OutboxStatus::DeadLetter, Some(&err)) {
                        tracing::error!(message_id = %entry.id, error = %e, "failed to mark dead letter");
                    }
                    let dl_entry = DeadLetterEntry::new(
                        &entry.id,
                        "outbox",
                        &entry.payload,
                        format!("max retries ({}): {}", new_count, err),
                        chrono::Utc::now().timestamp(),
                    );
                    if let Err(e) = dlq.insert(&dl_entry) {
                        tracing::error!(message_id = %entry.id, error = %e, "failed to insert dead letter entry");
                    }
                    audit.record(
                        Some(&entry.route_key),
                        "dead_letter",
                        &format!("max retries ({}): {}", new_count, err),
                    );
                    tracing::warn!(message_id = %entry.id, retry_count = new_count, "moved to dead letter queue");
                } else {
                    let next_at = chrono::Utc::now().timestamp_millis() + retry.next_delay_ms(new_count) as i64;
                    if let Err(e) = outbox.mark_retrying(&entry.id, new_count, next_at, &err) {
                        tracing::error!(message_id = %entry.id, error = %e, "failed to mark retrying");
                    }
                    audit.record(
                        Some(&entry.route_key),
                        "send",
                        &format!("retrying ({}): {}", new_count, err),
                    );
                    tracing::debug!(message_id = %entry.id, retry_count = new_count, next_retry_at = next_at, "outbox retry scheduled");
                }
            }
        }
    }
}

/// Process retryable messages whose next_retry_at has passed.
pub async fn process_retries(
    outbox: &dyn OutboxRepo,
    dlq: &dyn DeadLetterRepo,
    retry: &RetryConfig,
    sender: &dyn OutboxMessageSender,
    audit: &dyn AuditSink,
    batch_size: usize,
) {
    let now_ts = chrono::Utc::now().timestamp_millis();
    let entries = match outbox.fetch_retryable(now_ts, batch_size) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(error = %e, "failed to fetch retryable outbox entries");
            return;
        }
    };

    for entry in entries {
        if let Err(e) = outbox.mark_status(&entry.id, OutboxStatus::Sending, None) {
            tracing::error!(message_id = %entry.id, error = %e, "failed to mark retry sending, skipping");
            continue;
        }

        match sender.send(&entry).await {
            Ok(()) => {
                if let Err(e) = outbox.mark_status(&entry.id, OutboxStatus::Sent, None) {
                    tracing::error!(message_id = %entry.id, error = %e, "failed to mark sent (message already sent on retry)");
                }
                audit.record(Some(&entry.route_key), "send", "sent");
                tracing::info!(message_id = %entry.id, "outbox retry succeeded");
            }
            Err(err) => {
                let new_count = entry.retry_count + 1;
                if new_count >= retry.max_retries {
                    if let Err(e) = outbox.mark_status(&entry.id, OutboxStatus::DeadLetter, Some(&err)) {
                        tracing::error!(message_id = %entry.id, error = %e, "failed to mark dead letter on retry");
                    }
                    let dl_entry = DeadLetterEntry::new(
                        &entry.id,
                        "outbox",
                        &entry.payload,
                        format!("max retries ({}): {}", new_count, err),
                        chrono::Utc::now().timestamp(),
                    );
                    if let Err(e) = dlq.insert(&dl_entry) {
                        tracing::error!(message_id = %entry.id, error = %e, "failed to insert dead letter entry on retry");
                    }
                    audit.record(
                        Some(&entry.route_key),
                        "dead_letter",
                        &format!("max retries ({}): {}", new_count, err),
                    );
                    tracing::warn!(message_id = %entry.id, "moved to dead letter after {} retries", new_count);
                } else {
                    let next_at = chrono::Utc::now().timestamp_millis() + retry.next_delay_ms(new_count) as i64;
                    if let Err(e) = outbox.mark_retrying(&entry.id, new_count, next_at, &err) {
                        tracing::error!(message_id = %entry.id, error = %e, "failed to mark retrying on retry");
                    }
                    audit.record(
                        Some(&entry.route_key),
                        "send",
                        &format!("retrying ({}): {}", new_count, err),
                    );
                }
            }
        }
    }
}

/// Background worker: periodically polls pending and retryable messages.
pub async fn outbox_worker_loop(
    outbox: &dyn OutboxRepo,
    dlq: &dyn DeadLetterRepo,
    retry: &RetryConfig,
    sender: &dyn OutboxMessageSender,
    audit: &dyn AuditSink,
    interval_ms: u64,
    batch_size: usize,
) {
    let mut ticker = tokio::time::interval(Duration::from_millis(interval_ms));
    loop {
        ticker.tick().await;
        process_pending(outbox, dlq, retry, sender, audit, batch_size).await;
        process_retries(outbox, dlq, retry, sender, audit, batch_size).await;
    }
}
