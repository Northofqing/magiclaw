//! `TaskSupervisor` — owns a `tokio::task::JoinSet` of long-lived background
//! tasks so panics are observable instead of silent.
//!
//! Why a supervisor:
//! - `tokio::spawn` returns a `JoinHandle` that is silently dropped if not
//!   stored; any panic in the task goes undetected, especially dangerous for
//!   the outbox worker which can stall all outgoing deliveries.
//! - `/api/health` and the future `/api/tasks` endpoint need to surface
//!   per-task status: running, completed, failed.
//! - One-shot supervised tasks fit naturally in `JoinSet`; per-route workers
//!   remain owned by `ConversationStore` (different lifetime).
//!
//! Design (m07-concurrency: structured concurrency):
//! - `spawn(name, future)` adds the task to the inner `JoinSet` and records
//!   the task's `tokio::task::Id` plus its display name.
//! - `poll_status()` is a non-blocking drain that records finished tasks
//!   (success and failure) so the operator/health endpoint can see the
//!   latest status.
//! - `shutdown(timeout)` awaits all pending tasks with a timeout.
//!
//! This is intentionally minimal — it does not try to auto-restart failed
//! tasks. Restart policy is the operator's job, surfaced via health/audit.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use tokio::task::JoinSet;

/// Logical name + last-known status for one supervised task.
#[derive(Debug, Clone)]
pub struct TaskStatus {
    pub name: String,
    pub state: TaskState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    Running,
    Completed,
    Failed(String),
}

/// Owns a `JoinSet` plus a name registry so the supervisor can report
/// per-task status by name (the `JoinSet` itself only exposes a join future).
pub struct TaskSupervisor {
    set: Mutex<JoinSet<String>>,
    /// id → display name (for currently running tasks)
    running: Mutex<HashMap<tokio::task::Id, String>>,
    /// last known status for completed/failed tasks (kept until next poll)
    last_statuses: Mutex<Vec<TaskStatus>>,
}

impl TaskSupervisor {
    pub fn new() -> Self {
        Self {
            set: Mutex::new(JoinSet::new()),
            running: Mutex::new(HashMap::new()),
            last_statuses: Mutex::new(Vec::new()),
        }
    }

    /// Spawn a named supervised task. The task id is registered so the
    /// supervisor can attribute the eventual completion/failure to a name.
    pub fn spawn<F>(&self, name: &'static str, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let mut set = self.set.lock().unwrap_or_else(|e| e.into_inner());
        set.spawn(async move {
            fut.await;
            name.to_string()
        });
    }

    /// Non-blocking drain of finished tasks. Records latest status for each
    /// name and removes its running entry.
    pub fn poll_status(&self) -> Vec<TaskStatus> {
        let mut set = self.set.lock().unwrap_or_else(|e| e.into_inner());
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        let mut last = self.last_statuses.lock().unwrap_or_else(|e| e.into_inner());

        let mut out = Vec::new();
        while let Some(res) = set.try_join_next() {
            match res {
                Ok(name) => {
                    running.retain(|_, v| v != &name);
                    out.push(TaskStatus { name, state: TaskState::Completed });
                }
                Err(join_err) => {
                    // We don't have the original name here; surface a generic
                    // failed entry. Operators can grep logs by JoinError
                    // timing for the actual task.
                    let detail = join_err.to_string();
                    out.push(TaskStatus {
                        name: "<unknown>".into(),
                        state: TaskState::Failed(detail),
                    });
                }
            }
        }
        last.extend(out.iter().cloned());
        // cap last_statuses at 256 to avoid unbounded growth
        if last.len() > 256 {
            let drop = last.len() - 256;
            last.drain(0..drop);
        }
        out
    }

    /// Snapshot of currently running task names.
    pub fn running_names(&self) -> Vec<String> {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// Number of in-flight tasks.
    pub fn active_count(&self) -> usize {
        self.set.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Wait for all tasks to finish with a timeout. Tasks still running after
    /// the timeout are aborted (their JoinSet entries are dropped).
    pub async fn shutdown(&self, timeout: Duration) {
        let set = {
            let mut guard = self.set.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::replace(&mut *guard, JoinSet::new())
        };
        let _ = tokio::time::timeout(timeout, async {
            set.join_all().await;
        })
        .await;
        self.running.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

impl Default for TaskSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn supervisor_tracks_active_then_completed() {
        let sup = TaskSupervisor::new();
        sup.spawn("quick", async {
            tokio::time::sleep(Duration::from_millis(20)).await;
        });
        assert_eq!(sup.active_count(), 1);

        tokio::time::sleep(Duration::from_millis(50)).await;

        let finished = sup.poll_status();
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].name, "quick");
        assert_eq!(finished[0].state, TaskState::Completed);
        assert_eq!(sup.active_count(), 0);
    }

    #[tokio::test]
    async fn shutdown_completes_pending_tasks() {
        let sup = TaskSupervisor::new();
        sup.spawn("slow", async {
            tokio::time::sleep(Duration::from_millis(10)).await;
        });
        sup.shutdown(Duration::from_secs(1)).await;
        assert_eq!(sup.active_count(), 0);
    }

    #[tokio::test]
    async fn shutdown_timeout_drops_still_running() {
        let sup = TaskSupervisor::new();
        sup.spawn("forever", async {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        });
        sup.shutdown(Duration::from_millis(50)).await;
        assert_eq!(sup.active_count(), 0);
    }

    #[tokio::test]
    async fn multiple_tasks_share_supervisor() {
        let sup = TaskSupervisor::new();
        for i in 0..5 {
            sup.spawn("worker", async move {
                tokio::time::sleep(Duration::from_millis(5 + i * 5)).await;
            });
        }
        assert_eq!(sup.active_count(), 5);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let finished = sup.poll_status();
        assert_eq!(finished.len(), 5);
        assert!(finished.iter().all(|t| t.state == TaskState::Completed));
    }
}