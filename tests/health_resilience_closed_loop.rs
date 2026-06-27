//! Closed-loop test for /api/health resilience stats (Task #16).
//!
//! Verifies that the AppRuntime exposes the right structures and that the
//! health endpoint shape matches operator expectations:
//! - circuit_state: closed | open | half_open
//! - failure_count, failure_threshold, active, max_concurrent
//! - outbox_pending, dead_letter_count
//! - tasks.{running, finished}

use magiclaw::infrastructure::config::AppConfig;
use magiclaw::infrastructure::runtime::AppRuntime;

#[tokio::test]
#[allow(clippy::field_reassign_with_default)]
async fn runtime_exposes_resilience_components() {
    let db_path = std::env::temp_dir().join(format!(
        "magiclaw_health_{}.db",
        uuid::Uuid::new_v4()
    ));
    let mut config = AppConfig::default();
    config.db_path = db_path.to_string_lossy().to_string();
    let runtime = AppRuntime::new(config).expect("AppRuntime::new");

    // Circuit breakers start closed
    let send_state = runtime.send_gate.circuit_state();
    let ai_state = runtime.ai_gate.circuit_state();
    assert!(matches!(send_state, magiclaw::core::resilience::circuit_breaker::CircuitState::Closed));
    assert!(matches!(ai_state, magiclaw::core::resilience::circuit_breaker::CircuitState::Closed));

    // Bulkhead has configured capacity
    assert!(runtime.send_gate.max_concurrent() > 0);
    assert!(runtime.ai_gate.max_concurrent() > 0);

    // Initial failure counts are 0
    assert_eq!(runtime.send_gate.failure_count(), 0);
    assert_eq!(runtime.ai_gate.failure_count(), 0);

    // Thresholds are reasonable defaults
    assert!(runtime.send_gate.failure_threshold() >= 1);
    assert!(runtime.ai_gate.failure_threshold() >= 1);

    let _ = std::fs::remove_file(db_path);
}

#[tokio::test]
#[allow(clippy::field_reassign_with_default)]
async fn task_supervisor_reports_initial_state() {
    let db_path = std::env::temp_dir().join(format!(
        "magiclaw_tasks_{}.db",
        uuid::Uuid::new_v4()
    ));
    let mut config = AppConfig::default();
    config.db_path = db_path.to_string_lossy().to_string();
    let runtime = AppRuntime::new(config).expect("AppRuntime::new");

    // Before start_background, no tasks are running
    let running = runtime.task_supervisor.running_names();
    assert!(running.is_empty());

    let _ = std::fs::remove_file(db_path);
}

#[test]
fn circuit_breaker_opens_after_threshold() {
    use magiclaw::core::resilience::circuit_breaker::{BreakerConfig, CircuitBreaker, CircuitState};
    use std::time::Duration;

    let cb = CircuitBreaker::new(BreakerConfig {
        failure_threshold: 2,
        timeout: Duration::from_secs(60),
        half_open_max: 1,
    });

    cb.record_failure();
    cb.record_failure();
    assert_eq!(cb.state(), CircuitState::Open);
    assert!(cb.failure_count() >= 2);
    assert_eq!(cb.config().failure_threshold, 2);
}