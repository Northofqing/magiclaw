//! User agent preference port — read/write per-user AI agent selections.
//!
//! Replaces direct DbPool usage in `application/agent_preferences.rs` so the
//! application layer only sees this trait. Mirrors the `AuditQuery` pattern
//! applied earlier.

use crate::application::agent_preferences::UserAgentPreferences;

/// Port for the per-user agent preference store.
pub trait UserPreferenceStore: Send + Sync {
    /// Get the agent preference for a user, or None if not set.
    fn get(
        &self,
        channel: &str,
        account_scope: &str,
        peer_id: &str,
    ) -> Result<Option<String>, String>;

    /// Set (insert-or-replace) the agent preference for a user.
    fn set(
        &self,
        channel: &str,
        account_scope: &str,
        peer_id: &str,
        agent_name: &str,
    ) -> Result<(), String>;

    /// List preferences, optionally filtered by channel.
    fn list(
        &self,
        channel: Option<&str>,
    ) -> Result<Vec<UserAgentPreferences>, String>;
}

/// In-memory preference store for tests and contexts where persistence is not
/// required. Cheap to clone and shared via `Arc`.
pub struct InMemoryPreferenceStore {
    inner: std::sync::Mutex<Vec<UserAgentPreferences>>,
}

impl InMemoryPreferenceStore {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl Default for InMemoryPreferenceStore {
    fn default() -> Self {
        Self::new()
    }
}

impl UserPreferenceStore for InMemoryPreferenceStore {
    fn get(
        &self,
        channel: &str,
        account_scope: &str,
        peer_id: &str,
    ) -> Result<Option<String>, String> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Ok(guard
            .iter()
            .find(|p| p.channel == channel && p.account_scope == account_scope && p.peer_id == peer_id)
            .map(|p| p.agent_name.clone()))
    }

    fn set(
        &self,
        channel: &str,
        account_scope: &str,
        peer_id: &str,
        agent_name: &str,
    ) -> Result<(), String> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = guard.iter_mut().find(|p| {
            p.channel == channel && p.account_scope == account_scope && p.peer_id == peer_id
        }) {
            existing.agent_name = agent_name.to_string();
        } else {
            guard.push(UserAgentPreferences {
                channel: channel.to_string(),
                account_scope: account_scope.to_string(),
                peer_id: peer_id.to_string(),
                agent_name: agent_name.to_string(),
            });
        }
        Ok(())
    }

    fn list(&self, channel: Option<&str>) -> Result<Vec<UserAgentPreferences>, String> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Ok(guard
            .iter()
            .filter(|p| channel.is_none_or(|c| p.channel == c))
            .cloned()
            .collect())
    }
}