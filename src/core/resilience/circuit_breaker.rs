use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Circuit breaker state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

/// Circuit breaker for protecting external API calls.
pub struct CircuitBreaker {
    state: Mutex<CircuitState>,
    failure_count: AtomicU32,
    last_failure_time: AtomicU64,
    opened_at: Mutex<Option<Instant>>,
    config: BreakerConfig,
}

#[derive(Debug, Clone)]
pub struct BreakerConfig {
    /// Consecutive failures to trip Open.
    pub failure_threshold: u32,
    /// Duration to stay Open before transitioning to HalfOpen.
    pub timeout: Duration,
    /// Max requests allowed in HalfOpen state.
    pub half_open_max: u32,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 20,
            timeout: Duration::from_secs(300),
            half_open_max: 3,
        }
    }
}

impl CircuitBreaker {
    pub fn new(config: BreakerConfig) -> Self {
        Self {
            state: Mutex::new(CircuitState::Closed),
            failure_count: AtomicU32::new(0),
            last_failure_time: AtomicU64::new(0),
            opened_at: Mutex::new(None),
            config,
        }
    }

    /// Check if a request should be allowed through.
    pub fn allow_request(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match *state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if let Some(opened) = *self.opened_at.lock().unwrap_or_else(|e| e.into_inner()) {
                    if opened.elapsed() >= self.config.timeout {
                        *state = CircuitState::HalfOpen;
                        self.failure_count.store(0, Ordering::SeqCst);
                        tracing::info!("circuit breaker: Open → HalfOpen");
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => {
                // Check if too many failures have occurred in HalfOpen.
                // If so, revert to Open to await the full timeout again.
                if self.failure_count.load(Ordering::SeqCst) >= self.config.half_open_max {
                    let should_revert = {
                        if let Some(opened) = *self.opened_at.lock().unwrap_or_else(|e| e.into_inner()) {
                            opened.elapsed() >= self.config.timeout
                        } else {
                            false
                        }
                    };
                    if should_revert {
                        *state = CircuitState::Open;
                        *self.opened_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
                        self.failure_count.store(0, Ordering::SeqCst);
                        tracing::warn!(
                            "circuit breaker: HalfOpen → Open (probes exhausted)"
                        );
                    }
                    false
                } else {
                    true
                }
            }
        }
    }

    /// Record a successful request.
    pub fn record_success(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.failure_count.store(0, Ordering::SeqCst);
        if *state == CircuitState::HalfOpen {
            *state = CircuitState::Closed;
            tracing::info!("circuit breaker: HalfOpen → Closed");
        }
    }

    /// Record a failed request.
    pub fn record_failure(&self) {
        let count = self.failure_count.fetch_add(1, Ordering::SeqCst) + 1;
        self.last_failure_time.store(
            Instant::now().elapsed().as_millis() as u64,
            Ordering::SeqCst,
        );

        if count >= self.config.failure_threshold {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if *state == CircuitState::Closed {
                *state = CircuitState::Open;
                *self.opened_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
                tracing::warn!(
                    failures = count,
                    "circuit breaker: Closed → Open"
                );
            }
        }
    }

    /// Current state of the breaker.
    pub fn state(&self) -> CircuitState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaker_opens_after_threshold() {
        let config = BreakerConfig { failure_threshold: 3, ..Default::default() };
        let cb = CircuitBreaker::new(config);
        assert!(cb.allow_request());
        cb.record_failure();
        cb.record_failure();
        assert!(cb.allow_request()); // still closed
        cb.record_failure(); // 3rd failure
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.allow_request());
    }

    #[test]
    fn breaker_transitions_half_open() {
        let config = BreakerConfig { failure_threshold: 1, timeout: Duration::from_millis(1), half_open_max: 1 };
        let cb = CircuitBreaker::new(config);
        cb.record_failure(); // opens
        assert!(!cb.allow_request());
        std::thread::sleep(Duration::from_millis(2));
        assert!(cb.allow_request()); // half-open
    }

    #[test]
    fn half_open_reverts_to_open_when_probes_exhausted() {
        let config = BreakerConfig {
            failure_threshold: 1,
            timeout: Duration::from_millis(1),
            half_open_max: 2,
        };
        let cb = CircuitBreaker::new(config);
        // Trip to Open
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Wait for timeout, then enter HalfOpen
        std::thread::sleep(Duration::from_millis(2));
        assert!(cb.allow_request());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Exhaust all probes in HalfOpen
        cb.record_failure();
        cb.record_failure(); // failure_count = half_open_max

        // Now allow_request should block (probes exhausted, reverts to Open)
        assert!(!cb.allow_request());
        assert_eq!(cb.state(), CircuitState::Open);

        // Immediately after reverting to Open, still blocked
        assert!(!cb.allow_request());

        // After full timeout, can probe again (Open → HalfOpen)
        std::thread::sleep(Duration::from_millis(2));
        assert!(cb.allow_request());
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }
}
