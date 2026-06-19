use std::sync::Arc;

use async_trait::async_trait;

use crate::core::resilience::gate::ResilienceGate;
use crate::domain::storage::outbox::OutboxEntry;

use super::outbox_worker::OutboxMessageSender;

/// Resilience-wrapped outbound sender (red line 2.5): every channel send passes
/// through a [`ResilienceGate`] (Circuit Breaker + Bulkhead) dedicated to the
/// send pool, isolating it from the AI pool and tripping the breaker when the
/// external platform API keeps failing.
///
/// On rejection (open circuit / saturated pool) the send returns `Err`, which
/// the outbox worker treats like any send failure: the message is scheduled for
/// retry or moved to the dead-letter queue once retries are exhausted.
pub struct ResilientOutboxSender {
    inner: Arc<dyn OutboxMessageSender>,
    gate: Arc<ResilienceGate>,
}

impl ResilientOutboxSender {
    pub fn new(inner: Arc<dyn OutboxMessageSender>, gate: Arc<ResilienceGate>) -> Self {
        Self { inner, gate }
    }
}

#[async_trait]
impl OutboxMessageSender for ResilientOutboxSender {
    async fn send(&self, entry: &OutboxEntry) -> Result<(), String> {
        self.gate.execute(self.inner.send(entry)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::resilience::circuit_breaker::{BreakerConfig, CircuitState};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    struct CountingFailSender {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl OutboxMessageSender for CountingFailSender {
        async fn send(&self, _entry: &OutboxEntry) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err("channel down".to_string())
        }
    }

    #[tokio::test]
    async fn breaker_trips_and_stops_calling_inner_sender() {
        let inner = Arc::new(CountingFailSender {
            calls: AtomicUsize::new(0),
        });
        let gate = Arc::new(ResilienceGate::new(
            BreakerConfig {
                failure_threshold: 2,
                timeout: Duration::from_secs(60),
                half_open_max: 1,
            },
            50,
        ));
        let sender = ResilientOutboxSender::new(inner.clone(), gate.clone());

        let entry = OutboxEntry::new("m1", "rk", "{}", 1000);

        for _ in 0..2 {
            assert!(sender.send(&entry).await.is_err());
        }
        assert_eq!(gate.circuit_state(), CircuitState::Open);
        assert_eq!(inner.calls.load(Ordering::SeqCst), 2);

        // Open circuit rejects further sends without touching the channel.
        assert!(sender.send(&entry).await.is_err());
        assert_eq!(
            inner.calls.load(Ordering::SeqCst),
            2,
            "inner sender must not be called once the circuit is open"
        );
    }
}
