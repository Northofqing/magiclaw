//! WeChat media helpers: strict location parsing, a re-openable file-backed
//! media source with chunked reads, and a resilience decorator that isolates
//! uploads in their own bulkhead with a per-upload timeout.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncReadExt;

use crate::core::resilience::bulkhead::Bulkhead;
use crate::domain::ports::media::{
    MediaByteStream, MediaError, MediaMeta, MediaRef, MediaSource, MediaUploader,
};

/// Default chunk size for streaming reads (64 KiB).
pub const DEFAULT_CHUNK_BYTES: usize = 64 * 1024;
/// Default per-upload timeout.
pub const DEFAULT_UPLOAD_TIMEOUT_MS: u64 = 60_000;
/// Default media bulkhead concurrency (isolated from the send pool).
pub const DEFAULT_MEDIA_CONCURRENCY: usize = 4;

/// Strictly-parsed media location. Never confuse a remote URL with a local
/// path (challenge B6) — ambiguous or unsupported schemes are rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaLocation {
    LocalFile(PathBuf),
    RemoteUrl(String),
}

impl MediaLocation {
    pub fn parse(raw: &str) -> Result<Self, MediaError> {
        let s = raw.trim();
        if s.is_empty() {
            return Err(MediaError::Source("empty media location".into()));
        }
        if let Some(path) = s.strip_prefix("file://") {
            if path.trim().is_empty() {
                return Err(MediaError::Source("file:// with empty path".into()));
            }
            return Ok(MediaLocation::LocalFile(PathBuf::from(path)));
        }
        if s.starts_with("http://") || s.starts_with("https://") {
            return Ok(MediaLocation::RemoteUrl(s.to_string()));
        }
        Err(MediaError::Source(format!(
            "unsupported or ambiguous media location: {s}"
        )))
    }
}

/// Best-effort MIME from a filename extension; defaults to octet-stream.
fn mime_from_filename(filename: &str) -> String {
    let ext = filename
        .rsplit('.')
        .next()
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        _ => "application/octet-stream",
    };
    mime.to_string()
}

/// A re-openable, chunked media source backed by a local file.
///
/// Size is validated once at construction (early reject for over-limit files);
/// `open` may be called repeatedly and always returns a fresh stream.
#[derive(Debug)]
pub struct FileMediaSource {
    path: PathBuf,
    meta: MediaMeta,
    chunk_bytes: usize,
}

impl FileMediaSource {
    /// Build a source from a local path. `size_limit` of 0 disables the limit.
    pub async fn new(
        path: PathBuf,
        chunk_bytes: usize,
        size_limit: u64,
    ) -> Result<Self, MediaError> {
        let md = tokio::fs::metadata(&path)
            .await
            .map_err(|e| MediaError::Source(format!("stat {}: {e}", path.display())))?;
        if !md.is_file() {
            return Err(MediaError::Source(format!(
                "not a regular file: {}",
                path.display()
            )));
        }
        let size = md.len();
        if size_limit > 0 && size > size_limit {
            return Err(MediaError::TooLarge {
                size,
                limit: size_limit,
            });
        }
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let mime = mime_from_filename(&filename);
        let chunk_bytes = chunk_bytes.max(1);
        Ok(Self {
            path,
            meta: MediaMeta {
                filename,
                mime,
                size,
            },
            chunk_bytes,
        })
    }
}

#[async_trait]
impl MediaSource for FileMediaSource {
    fn meta(&self) -> &MediaMeta {
        &self.meta
    }

    async fn open(&self) -> Result<Box<dyn MediaByteStream>, MediaError> {
        let file = tokio::fs::File::open(&self.path)
            .await
            .map_err(|e| MediaError::Source(format!("open {}: {e}", self.path.display())))?;
        Ok(Box::new(FileChunkStream {
            file,
            chunk_bytes: self.chunk_bytes,
        }))
    }
}

struct FileChunkStream {
    file: tokio::fs::File,
    chunk_bytes: usize,
}

#[async_trait]
impl MediaByteStream for FileChunkStream {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, MediaError> {
        let mut buf = vec![0u8; self.chunk_bytes];
        let n = self
            .file
            .read(&mut buf)
            .await
            .map_err(|e| MediaError::Source(format!("read: {e}")))?;
        if n == 0 {
            return Ok(None);
        }
        buf.truncate(n);
        Ok(Some(buf))
    }
}

/// Resilience decorator for any [`MediaUploader`]: bounds upload concurrency in
/// a dedicated bulkhead (so large uploads cannot starve text sends — challenge
/// B3) and enforces a per-upload timeout.
pub struct ResilientMediaUploader {
    inner: Arc<dyn MediaUploader>,
    bulkhead: Bulkhead,
    timeout: Duration,
}

impl ResilientMediaUploader {
    pub fn new(inner: Arc<dyn MediaUploader>, max_concurrent: usize, timeout: Duration) -> Self {
        Self {
            inner,
            bulkhead: Bulkhead::new(max_concurrent.max(1)),
            timeout,
        }
    }

    pub fn with_defaults(inner: Arc<dyn MediaUploader>) -> Self {
        Self::new(
            inner,
            DEFAULT_MEDIA_CONCURRENCY,
            Duration::from_millis(DEFAULT_UPLOAD_TIMEOUT_MS),
        )
    }

    /// In-flight uploads (observability).
    pub fn active_count(&self) -> usize {
        self.bulkhead.active_count()
    }
}

#[async_trait]
impl MediaUploader for ResilientMediaUploader {
    async fn upload(&self, source: &dyn MediaSource) -> Result<MediaRef, MediaError> {
        let _permit = self
            .bulkhead
            .acquire()
            .await
            .map_err(|e| MediaError::Upload(format!("media bulkhead: {e}")))?;

        match tokio::time::timeout(self.timeout, self.inner.upload(source)).await {
            Ok(result) => result,
            Err(_) => Err(MediaError::Upload(format!(
                "upload timed out after {}ms",
                self.timeout.as_millis()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Test uploader that drains the stream chunk-by-chunk, asserting it never
    /// receives the whole object at once.
    struct CountingUploader {
        chunks: AtomicUsize,
        bytes: AtomicUsize,
    }

    impl CountingUploader {
        fn new() -> Self {
            Self {
                chunks: AtomicUsize::new(0),
                bytes: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl MediaUploader for CountingUploader {
        async fn upload(&self, source: &dyn MediaSource) -> Result<MediaRef, MediaError> {
            let mut stream = source.open().await?;
            while let Some(chunk) = stream.next_chunk().await? {
                self.chunks.fetch_add(1, Ordering::SeqCst);
                self.bytes.fetch_add(chunk.len(), Ordering::SeqCst);
            }
            Ok(MediaRef {
                media_id: "media-test".into(),
                url: None,
            })
        }
    }

    fn temp_file(bytes: &[u8]) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("aiclaw_media_{}.bin", uuid::Uuid::new_v4()));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(bytes).unwrap();
        path
    }

    #[test]
    fn parse_local_remote_and_rejects_ambiguous() {
        assert_eq!(
            MediaLocation::parse("file:///tmp/x.png").unwrap(),
            MediaLocation::LocalFile(PathBuf::from("/tmp/x.png"))
        );
        assert_eq!(
            MediaLocation::parse("https://h/x.png").unwrap(),
            MediaLocation::RemoteUrl("https://h/x.png".into())
        );
        assert!(MediaLocation::parse("").is_err());
        assert!(MediaLocation::parse("   ").is_err());
        assert!(MediaLocation::parse("file://").is_err());
        assert!(MediaLocation::parse("/tmp/x.png").is_err()); // ambiguous, no scheme
        assert!(MediaLocation::parse("ftp://h/x").is_err());
    }

    #[tokio::test]
    async fn file_source_reopens_and_streams_in_chunks() {
        let data = vec![7u8; 10_000];
        let path = temp_file(&data);
        let src = FileMediaSource::new(path.clone(), 4096, 0).await.unwrap();
        assert_eq!(src.meta().size, 10_000);

        // Two independent opens each read the full content.
        for _ in 0..2 {
            let mut stream = src.open().await.unwrap();
            let mut chunks = 0usize;
            let mut total = 0usize;
            while let Some(c) = stream.next_chunk().await.unwrap() {
                assert!(c.len() <= 4096, "chunk exceeds chunk_bytes");
                chunks += 1;
                total += c.len();
            }
            assert_eq!(total, 10_000);
            assert!(chunks >= 3, "expected multiple chunks, got {chunks}");
        }
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn over_limit_file_is_rejected_early() {
        let path = temp_file(&[0u8; 2048]);
        let err = FileMediaSource::new(path.clone(), 512, 1024).await.unwrap_err();
        assert!(matches!(err, MediaError::TooLarge { size: 2048, limit: 1024 }));
        assert_eq!(err.stage(), "source");
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn missing_file_is_source_error() {
        let err = FileMediaSource::new(PathBuf::from("/no/such/aiclaw_x.bin"), 512, 0)
            .await
            .unwrap_err();
        assert!(matches!(err, MediaError::Source(_)));
        assert_eq!(err.stage(), "source");
    }

    #[tokio::test]
    async fn resilient_uploader_streams_through() {
        let data = vec![1u8; 5000];
        let path = temp_file(&data);
        let src = FileMediaSource::new(path.clone(), 1024, 0).await.unwrap();
        let counter = Arc::new(CountingUploader::new());
        let uploader = ResilientMediaUploader::with_defaults(counter.clone());

        let media_ref = uploader.upload(&src).await.unwrap();
        assert_eq!(media_ref.media_id, "media-test");
        assert_eq!(counter.bytes.load(Ordering::SeqCst), 5000);
        assert!(counter.chunks.load(Ordering::SeqCst) >= 4);
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn resilient_uploader_enforces_timeout() {
        struct SlowUploader;
        #[async_trait]
        impl MediaUploader for SlowUploader {
            async fn upload(&self, _s: &dyn MediaSource) -> Result<MediaRef, MediaError> {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(MediaRef { media_id: "x".into(), url: None })
            }
        }
        let path = temp_file(&[0u8; 16]);
        let src = FileMediaSource::new(path.clone(), 8, 0).await.unwrap();
        let uploader =
            ResilientMediaUploader::new(Arc::new(SlowUploader), 4, Duration::from_millis(20));
        let err = uploader.upload(&src).await.unwrap_err();
        assert!(matches!(err, MediaError::Upload(_)));
        assert!(err.to_string().contains("timed out"));
        std::fs::remove_file(&path).ok();
    }
}
