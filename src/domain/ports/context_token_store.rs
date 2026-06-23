use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContextTokenError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Db(String),
}

/// Persistent storage for WeChat context_tokens (per-user, per-account).
pub trait ContextTokenStore: Send + Sync {
    /// Save a context_token for a user within an account.
    fn set(&self, account_id: &str, user_id: &str, token: &str) -> Result<(), ContextTokenError>;

    /// Load a context_token for a user within an account.
    fn get(&self, account_id: &str, user_id: &str) -> Result<Option<String>, ContextTokenError>;

    /// Load all tokens for an account (used for startup recovery).
    fn get_all(&self, account_id: &str) -> Result<std::collections::HashMap<String, String>, ContextTokenError>;

    /// Clear all tokens for an account (called on session expiry or re-login).
    fn delete_all(&self, account_id: &str) -> Result<(), ContextTokenError>;

    /// Clear a specific token (rarely used, but included for completeness).
    fn delete(&self, account_id: &str, user_id: &str) -> Result<(), ContextTokenError>;
}
