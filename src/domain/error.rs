//! Domain error types for core traits.
//!
//! These replace `Result<_, String>` signatures on the `Channel`, `Middleware`,
//! and `AiBackend` traits so callers can branch on error variant (retry vs.
//! dead-letter, short-circuit vs. propagate) instead of string-matching.
//!
//! Design principles (from m13-domain-error):
//! - Categorize by audience (channel caller / pipeline caller / ai caller)
//! - Categorize by recoverability (transient vs. terminal)
//! - Carry structured context (errcode, retry_after) where the platform provides it
//! - `From<String>` and `From<&str>` so legacy `String` error sites can adopt gradually

use thiserror::Error;

/// Errors returned by channel implementations when sending messages or
/// performing lifecycle operations.
#[derive(Debug, Error)]
pub enum ChannelError {
    /// Underlying transport failure (network, DNS, timeout).
    /// Transient — eligible for retry.
    #[error("transport error: {0}")]
    Transport(String),

    /// Authentication expired or invalid (e.g. WeChat ret=-2, Feishu errcode 10003).
    /// Terminal for the current session — caller must refresh credentials before retry.
    #[error("authentication expired (errcode={errcode}): {detail}")]
    AuthExpired { errcode: i64, detail: String },

    /// Platform rate limit hit. `retry_after_secs` is the suggested wait.
    /// Transient — caller should back off.
    #[error("rate limited, retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    /// Recipient identifier is malformed or unknown to the platform.
    /// Terminal — no retry will succeed.
    #[error("invalid recipient: {0}")]
    InvalidRecipient(String),

    /// Content was rejected by the platform (e.g. blocked keyword, too long).
    /// Terminal — caller must modify the message before resending.
    #[error("content rejected: {0}")]
    ContentRejected(String),

    /// Operation not supported by this channel implementation.
    /// Terminal — caller must choose a different channel.
    #[error("unsupported operation: {0}")]
    Unsupported(String),

    /// Catch-all for unexpected internal failures (bug, invariant violation).
    /// Terminal — requires investigation.
    #[error("internal error: {0}")]
    Internal(String),
}

impl ChannelError {
    /// Whether the outbox should retry this error.
    /// Aligns with `FeishuErrorSemantics::should_retry` decision table.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Transport(_) | Self::RateLimited { .. }
        )
    }

    /// Whether the outbox should send this message to the dead-letter queue.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::AuthExpired { .. }
                | Self::InvalidRecipient(_)
                | Self::ContentRejected(_)
                | Self::Unsupported(_)
                | Self::Internal(_)
        )
    }
}

/// Errors returned by pipeline middleware.
#[derive(Debug, Error)]
pub enum PipelineError {
    /// A middleware returned an error and the pipeline must abort.
    #[error("middleware '{name}' failed: {source}")]
    MiddlewareFailed {
        name: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Pipeline context was invalid for the next stage
    /// (e.g. missing required config field, empty message id).
    #[error("invalid pipeline context: {0}")]
    InvalidContext(String),

    /// Backend dependency (AI, channel, storage) failed in a recoverable way.
    #[error("upstream unavailable: {0}")]
    UpstreamUnavailable(String),

    /// Pipeline is shutting down; caller should drop the message.
    #[error("pipeline shutting down")]
    Shutdown,
}

/// Errors returned by AI backend implementations.
#[derive(Debug, Error)]
pub enum AiError {
    /// Network or transport failure to the AI provider.
    /// Transient — eligible for retry.
    #[error("backend transport error: {0}")]
    Transport(String),

    /// AI provider returned an error response.
    /// May be transient (5xx) or terminal (4xx).
    #[error("backend error: status={status} body={body}")]
    Backend { status: u16, body: String },

    /// Request timed out waiting for AI response.
    /// Transient — caller may retry with shorter prompt.
    #[error("backend timeout after {0}ms")]
    Timeout(u64),

    /// AI provider rate limit hit.
    /// Transient — caller should back off.
    #[error("rate limited, retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    /// Selected AI backend refused the request (auth, quota, model not found).
    /// Terminal for this backend — caller must switch backends.
    #[error("backend refused: {0}")]
    Refused(String),

    /// No backend is configured.
    /// Terminal — caller must configure at least one backend.
    #[error("no AI backend configured")]
    NoBackend,

    /// Internal invariant violation (missing stdout pipe, unexpected None).
    /// Terminal — requires investigation.
    #[error("internal error: {0}")]
    Internal(String),
}

impl AiError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Transport(_) | Self::Timeout(_) | Self::RateLimited { .. }
        )
    }
}

// --- Gradual migration helpers ------------------------------------------------
//
// Existing implementations and call-sites still return `String` errors in many
// places. These `From` impls let them flow into typed errors via `?` while we
// incrementally migrate each site to a concrete variant. New code should
// construct the concrete variant directly.

impl From<String> for ChannelError {
    fn from(s: String) -> Self {
        Self::Internal(s)
    }
}

impl From<&str> for ChannelError {
    fn from(s: &str) -> Self {
        Self::Internal(s.to_string())
    }
}

impl From<String> for PipelineError {
    fn from(s: String) -> Self {
        Self::UpstreamUnavailable(s)
    }
}

impl From<&str> for PipelineError {
    fn from(s: &str) -> Self {
        Self::UpstreamUnavailable(s.to_string())
    }
}

impl From<String> for AiError {
    fn from(s: String) -> Self {
        Self::Backend { status: 0, body: s }
    }
}

impl From<&str> for AiError {
    fn from(s: &str) -> Self {
        Self::Backend { status: 0, body: s.to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_retry_classification() {
        assert!(ChannelError::Transport("net".into()).is_retryable());
        assert!(ChannelError::RateLimited { retry_after_secs: 60 }.is_retryable());
        assert!(!ChannelError::AuthExpired { errcode: 10003, detail: "x".into() }.is_retryable());
        assert!(ChannelError::AuthExpired { errcode: 10003, detail: "x".into() }.is_terminal());
        assert!(ChannelError::InvalidRecipient("bad".into()).is_terminal());
    }

    #[test]
    fn ai_retry_classification() {
        assert!(AiError::Transport("net".into()).is_retryable());
        assert!(AiError::Timeout(5000).is_retryable());
        assert!(!AiError::NoBackend.is_retryable());
    }

    #[test]
    fn string_conversion_lands_in_internal() {
        let e: ChannelError = "boom".into();
        assert!(matches!(e, ChannelError::Internal(_)));
        let e: AiError = "boom".into();
        assert!(matches!(e, AiError::Backend { status: 0, .. }));
    }
}