//! Closed-loop test for streamed media upload (red line 2.4), exercised through
//! the live `ChannelRegistry → WeChatChannel → MediaUploader` path.
//!
//! Validates design §7: chunked upload, media_id reference in receipt, source
//! failure → error, over-limit → early reject, and that the uploader observes
//! the payload as a sequence of bounded chunks (never one full-file read).

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use aiclaw::channels::registry::ChannelRegistry;
use aiclaw::channels::wechat::channel::WeChatChannel;
use aiclaw::channels::wechat::media::ResilientMediaUploader;
use aiclaw::domain::entities::message::MessageContent;
use aiclaw::domain::ports::media::{MediaError, MediaRef, MediaSource, MediaUploader};
use aiclaw::domain::value_objects::route_key::ChannelId;
use async_trait::async_trait;

/// Fake uploader recording how the source was streamed.
struct RecordingUploader {
    chunks: AtomicUsize,
    bytes: AtomicUsize,
    max_chunk: AtomicUsize,
}

impl RecordingUploader {
    fn new() -> Self {
        Self {
            chunks: AtomicUsize::new(0),
            bytes: AtomicUsize::new(0),
            max_chunk: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl MediaUploader for RecordingUploader {
    async fn upload(&self, source: &dyn MediaSource) -> Result<MediaRef, MediaError> {
        let mut stream = source.open().await?;
        while let Some(chunk) = stream.next_chunk().await? {
            self.chunks.fetch_add(1, Ordering::SeqCst);
            self.bytes.fetch_add(chunk.len(), Ordering::SeqCst);
            self.max_chunk.fetch_max(chunk.len(), Ordering::SeqCst);
        }
        Ok(MediaRef {
            media_id: format!("uploaded-{}", source.meta().filename),
            url: None,
        })
    }
}

fn temp_file(bytes: &[u8]) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("aiclaw_cl_{}.bin", uuid::Uuid::new_v4()));
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(bytes).unwrap();
    path
}

fn registry_with(uploader: Arc<dyn MediaUploader>, chunk_bytes: usize, size_limit: u64) -> ChannelRegistry {
    let channel = WeChatChannel::new().with_media_uploader(uploader, chunk_bytes, size_limit);
    let mut registry = ChannelRegistry::new();
    registry.register(Arc::new(channel));
    registry
}

#[tokio::test]
async fn media_uploads_streamed_in_chunks_and_returns_reference() {
    let data = vec![9u8; 20_000];
    let path = temp_file(&data);
    let recorder = Arc::new(RecordingUploader::new());
    let uploader = Arc::new(ResilientMediaUploader::with_defaults(recorder.clone()));
    let registry = registry_with(uploader, 4096, 0);

    let receipt = registry
        .send_via(
            &ChannelId::new("wechat"),
            "peer_a",
            &MessageContent::File {
                url: format!("file://{}", path.display()),
                name: "doc.bin".into(),
                size: 20_000,
            },
        )
        .await
        .unwrap();

    // Receipt references the uploaded media id.
    assert!(receipt.platform_msg_id.unwrap().starts_with("wechat_media_uploaded-"));
    // Streamed in bounded chunks, never one full read.
    assert_eq!(recorder.bytes.load(Ordering::SeqCst), 20_000);
    assert!(recorder.chunks.load(Ordering::SeqCst) >= 4);
    assert!(recorder.max_chunk.load(Ordering::SeqCst) <= 4096);
    std::fs::remove_file(&path).ok();
}

#[tokio::test]
async fn missing_source_surfaces_stage_tagged_error() {
    let recorder = Arc::new(RecordingUploader::new());
    let uploader = Arc::new(ResilientMediaUploader::with_defaults(recorder));
    let registry = registry_with(uploader, 4096, 0);

    let err = registry
        .send_via(
            &ChannelId::new("wechat"),
            "peer_a",
            &MessageContent::File {
                url: "file:///no/such/aiclaw_missing.bin".into(),
                name: "x.bin".into(),
                size: 1,
            },
        )
        .await
        .unwrap_err();
    assert!(err.contains("media[source]"), "got: {err}");
}

#[tokio::test]
async fn over_limit_media_is_rejected_before_upload() {
    let path = temp_file(&[0u8; 4096]);
    let recorder = Arc::new(RecordingUploader::new());
    let uploader = Arc::new(ResilientMediaUploader::with_defaults(recorder.clone()));
    let registry = registry_with(uploader, 1024, 1024); // limit < file size

    let err = registry
        .send_via(
            &ChannelId::new("wechat"),
            "peer_a",
            &MessageContent::File {
                url: format!("file://{}", path.display()),
                name: "big.bin".into(),
                size: 4096,
            },
        )
        .await
        .unwrap_err();
    assert!(err.contains("media[source]"), "got: {err}");
    // Nothing was streamed to the uploader.
    assert_eq!(recorder.bytes.load(Ordering::SeqCst), 0);
    std::fs::remove_file(&path).ok();
}

#[tokio::test]
async fn remote_url_without_media_id_is_rejected() {
    let recorder = Arc::new(RecordingUploader::new());
    let uploader = Arc::new(ResilientMediaUploader::with_defaults(recorder));
    let registry = registry_with(uploader, 4096, 0);

    let err = registry
        .send_via(
            &ChannelId::new("wechat"),
            "peer_a",
            &MessageContent::Image {
                url: "https://example.invalid/x.png".into(),
                media_id: None,
            },
        )
        .await
        .unwrap_err();
    assert!(err.contains("remote URL"), "got: {err}");
}

#[tokio::test]
async fn existing_media_id_is_referenced_without_upload() {
    let recorder = Arc::new(RecordingUploader::new());
    let uploader = Arc::new(ResilientMediaUploader::with_defaults(recorder.clone()));
    let registry = registry_with(uploader, 4096, 0);

    let receipt = registry
        .send_via(
            &ChannelId::new("wechat"),
            "peer_a",
            &MessageContent::Image {
                url: "https://cdn/x.png".into(),
                media_id: Some("platform-123".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        receipt.platform_msg_id.unwrap(),
        "wechat_media_platform-123"
    );
    assert_eq!(recorder.bytes.load(Ordering::SeqCst), 0);
}
