use std::sync::Arc;

use async_trait::async_trait;

use crate::domain::error::AiError;

use crate::core::resilience::gate::ResilienceGate;

use super::backend::AiBackend;

/// Resilience-wrapped AI backend (red line 2.5): every call to the inner
/// backend passes through a [`ResilienceGate`] (Circuit Breaker + Bulkhead),
/// so a failing or slow AI dependency trips the breaker and is isolated from
/// the send pool.
///
/// On rejection (open circuit) the call returns `Err`, which the AI middleware
/// already degrades gracefully (echoing the input back).
pub struct ResilientAiBackend {
    inner: Arc<dyn AiBackend>,
    gate: Arc<ResilienceGate>,
}

impl ResilientAiBackend {
    pub fn new(inner: Arc<dyn AiBackend>, gate: Arc<ResilienceGate>) -> Self {
        Self { inner, gate }
    }
}

#[async_trait]
impl AiBackend for ResilientAiBackend {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    async fn generate(&self, input: &str, context: Option<&str>) -> Result<String, AiError> {
        self.gate.execute(self.inner.generate(input, context)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::resilience::circuit_breaker::{BreakerConfig, CircuitState};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    struct CountingFailBackend {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl AiBackend for CountingFailBackend {
        fn name(&self) -> &'static str {
            "counting-fail"
        }
        async fn generate(&self, _input: &str, _context: Option<&str>) -> Result<String, AiError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(AiError::Transport("backend down".to_string()))
        }
    }

    #[tokio::test]
    async fn breaker_trips_and_stops_calling_inner_backend() {
        let inner = Arc::new(CountingFailBackend {
            calls: AtomicUsize::new(0),
        });
        let gate = Arc::new(ResilienceGate::new(
            BreakerConfig {
                failure_threshold: 3,
                timeout: Duration::from_secs(60),
                half_open_max: 1,
            },
            5,
        ));
        let backend = ResilientAiBackend::new(inner.clone(), gate.clone());

        // 3 failures trip the breaker.
        for _ in 0..3 {
            assert!(backend.generate("hi", None).await.is_err());
        }
        assert_eq!(gate.circuit_state(), CircuitState::Open);
        assert_eq!(inner.calls.load(Ordering::SeqCst), 3);

        // Further calls are rejected by the open circuit without hitting inner.
        assert!(backend.generate("hi", None).await.is_err());
        assert_eq!(
            inner.calls.load(Ordering::SeqCst),
            3,
            "inner backend must not be called once the circuit is open"
        );
    }
}
