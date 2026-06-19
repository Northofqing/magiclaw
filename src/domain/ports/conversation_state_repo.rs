use super::inbox_repo::RepoResult;

/// A persisted conversation state row (red line 2.3: conversation_state 持久化).
///
/// Mirrors the `conversation_state(route_key, state_json, updated_at)` table.
/// `route_key` is the serialized `RouteKey` (JSON); `state_json` is the
/// serialized `ConversationState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedConversationState {
    pub route_key: String,
    pub state_json: String,
    pub updated_at: i64,
}

/// Repository for durable conversation state. Persisting state allows the
/// system to observe and recover prior conversation lifecycle on restart.
pub trait ConversationStateRepo: Send + Sync {
    /// Insert or update the state for a conversation (keyed by route_key).
    fn upsert(&self, route_key: &str, state_json: &str, updated_at: i64) -> RepoResult<()>;
    /// Load all persisted conversation states (used for restart recovery).
    fn load_all(&self) -> RepoResult<Vec<PersistedConversationState>>;
}
