//! Media upload ports (red line 2.4: streamed / chunked media transfer).
//!
//! These ports keep the core/channels decoupled from any concrete transport:
//!   - [`MediaSource`] is a *re-openable* source of bytes that yields data in
//!     bounded chunks — implementations MUST NOT read a whole object into
//!     memory at once.
//!   - [`MediaUploader`] consumes a source and performs the actual streamed
//!     upload, returning a platform-side [`MediaRef`].
//!
//! Failures are classified by stage ([`MediaError::stage`]) so that dead-letter
//! entries are diagnosable (source vs upload vs send).

use async_trait::async_trait;

/// Metadata describing a media object to be uploaded.
#[derive(Debug, Clone)]
pub struct MediaMeta {
    pub filename: String,
    pub mime: String,
    pub size: u64,
}

/// A pull-based, chunked byte stream over a media object.
///
/// Red line 2.4: implementations MUST read lazily. Each `next_chunk` reads at
/// most one chunk; returning `Ok(None)` signals end-of-stream.
#[async_trait]
pub trait MediaByteStream: Send {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, MediaError>;
}

/// A re-openable source of media bytes.
///
/// `open` MUST be callable multiple times — once per send attempt. Each call
/// returns a *fresh* stream so retries never reuse a consumed stream
/// (consumed streams cannot be rewound).
#[async_trait]
pub trait MediaSource: Send + Sync {
    fn meta(&self) -> &MediaMeta;
    async fn open(&self) -> Result<Box<dyn MediaByteStream>, MediaError>;
}

/// Platform-side reference returned after a successful upload.
#[derive(Debug, Clone)]
pub struct MediaRef {
    pub media_id: String,
    pub url: Option<String>,
}

/// Uploads media via streaming / chunked transfer.
#[async_trait]
pub trait MediaUploader: Send + Sync {
    async fn upload(&self, source: &dyn MediaSource) -> Result<MediaRef, MediaError>;
}

/// Errors across the media pipeline, classified by failing stage so dead-letter
/// reasons are diagnosable.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("media source error: {0}")]
    Source(String),
    #[error("media too large: {size} bytes exceeds limit {limit}")]
    TooLarge { size: u64, limit: u64 },
    #[error("media upload error: {0}")]
    Upload(String),
    #[error("media send error: {0}")]
    Send(String),
}

impl MediaError {
    /// Stage label used for audit / dead-letter reason.
    pub fn stage(&self) -> &'static str {
        match self {
            MediaError::Source(_) | MediaError::TooLarge { .. } => "source",
            MediaError::Upload(_) => "upload",
            MediaError::Send(_) => "send",
        }
    }

    /// Whether retrying could succeed. Source / TooLarge are terminal
    /// (re-uploading the same bad source will fail identically).
    pub fn is_retryable(&self) -> bool {
        matches!(self, MediaError::Upload(_) | MediaError::Send(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_labels_cover_all_variants() {
        assert_eq!(MediaError::Source("x".into()).stage(), "source");
        assert_eq!(MediaError::TooLarge { size: 2, limit: 1 }.stage(), "source");
        assert_eq!(MediaError::Upload("x".into()).stage(), "upload");
        assert_eq!(MediaError::Send("x".into()).stage(), "send");
    }

    #[test]
    fn retryable_only_for_upload_and_send() {
        assert!(!MediaError::Source("x".into()).is_retryable());
        assert!(!MediaError::TooLarge { size: 2, limit: 1 }.is_retryable());
        assert!(MediaError::Upload("x".into()).is_retryable());
        assert!(MediaError::Send("x".into()).is_retryable());
    }

    #[test]
    fn error_display_includes_context() {
        assert!(MediaError::TooLarge { size: 9, limit: 4 }
            .to_string()
            .contains("9 bytes exceeds limit 4"));
        assert!(MediaError::Upload("boom".into())
            .to_string()
            .contains("boom"));
    }
}

