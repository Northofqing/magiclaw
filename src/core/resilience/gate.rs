use super::bulkhead::Bulkhead;
use super::circuit_breaker::{BreakerConfig, CircuitBreaker, CircuitState};

/// A resilience gate combining a Circuit Breaker with a Bulkhead, protecting a
/// single failure domain (red line 2.5).
///
/// - The **Circuit Breaker** trips after consecutive failures, fast-failing
///   subsequent calls so a struggling dependency is not hammered.
/// - The **Bulkhead** bounds concurrency, isolating this domain so a backlog in
///   one pool (e.g. AI) cannot exhaust resources needed by another (e.g. send).
///
/// Use one gate per isolated pool: one for AI calls, one for channel sends.
pub struct ResilienceGate {
    breaker: CircuitBreaker,
    bulkhead: Bulkhead,
}

impl ResilienceGate {
    pub fn new(breaker_config: BreakerConfig, max_concurrent: usize) -> Self {
        Self {
            breaker: CircuitBreaker::new(breaker_config),
            bulkhead: Bulkhead::new(max_concurrent),
        }
    }

    /// Gate for the AI execution pool (small concurrency, isolated from send).
    pub fn ai_default() -> Self {
        Self::new(BreakerConfig::default(), 5)
    }

    /// Gate for the send execution pool (larger concurrency).
    pub fn send_default() -> Self {
        Self::new(BreakerConfig::default(), 50)
    }

    /// Execute a fallible async operation under the gate.
    ///
    /// Returns `Err` immediately if the circuit is open (without running the
    /// operation). Otherwise acquires a bulkhead permit (waiting if the pool is
    /// saturated, which bounds concurrency), runs the operation, and records the
    /// outcome with the breaker. The permit is released when the call returns.
    ///
    /// Generic over the error type `E` so callers can preserve structured error
    /// variants (e.g. `ChannelError`, `AiError`) without losing type information
    /// at the resilience boundary.
    pub async fn execute<T, E, Fut>(&self, op: Fut) -> Result<T, E>
    where
        Fut: std::future::Future<Output = Result<T, E>>,
        E: From<String>,
    {
        if !self.breaker.allow_request() {
            return Err(E::from("circuit breaker open".to_string()));
        }

        let _guard = self
            .bulkhead
            .acquire()
            .await
            .map_err(|e| E::from(e.to_string()))?;

        match op.await {
            Ok(value) => {
                self.breaker.record_success();
                Ok(value)
            }
            Err(e) => {
                self.breaker.record_failure();
                Err(e)
            }
        }
    }

    /// Current circuit breaker state (for observability/health).
    pub fn circuit_state(&self) -> CircuitState {
        self.breaker.state()
    }

    /// Current number of in-flight operations through the bulkhead.
    pub fn active_count(&self) -> usize {
        self.bulkhead.active_count()
    }

    /// Maximum concurrent operations allowed by the bulkhead.
    pub fn max_concurrent(&self) -> usize {
        self.bulkhead.max_concurrent()
    }

    /// Whether the circuit is open (rejecting all requests).
    pub fn is_open(&self) -> bool {
        matches!(self.breaker.state(), CircuitState::Open)
    }

    /// Failure count in the breaker.
    pub fn failure_count(&self) -> u32 {
        self.breaker.failure_count()
    }

    /// Configured failure threshold (for health reporting).
    pub fn failure_threshold(&self) -> u32 {
        self.breaker.config().failure_threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn passes_through_successful_calls() {
        let gate = ResilienceGate::new(BreakerConfig::default(), 4);
        let r: Result<i32, String> = gate.execute(async { Ok(7) }).await;
        assert_eq!(r.unwrap(), 7);
        assert_eq!(gate.circuit_state(), CircuitState::Closed);
    }

    #[tokio::test]
    async fn opens_after_threshold_and_rejects() {
        let gate = ResilienceGate::new(
            BreakerConfig {
                failure_threshold: 2,
                timeout: Duration::from_secs(60),
                half_open_max: 1,
            },
            4,
        );

        let _ = gate.execute(async { Err::<(), String>("boom".into()) }).await;
        let _ = gate.execute(async { Err::<(), String>("boom".into()) }).await;
        assert_eq!(gate.circuit_state(), CircuitState::Open);

        // Once open, the operation must not run — rejected immediately.
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ran2 = ran.clone();
        let result = gate
            .execute(async move {
                ran2.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok::<(), String>(())
            })
            .await;
        assert!(result.is_err());
        assert!(
            !ran.load(std::sync::atomic::Ordering::SeqCst),
            "open circuit must short-circuit before running the operation"
        );
    }
}
