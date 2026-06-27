//! Closed-loop test for Dingtalk channel (Phase 4 skeleton → full).
//!
//! Exercises the stub path and error semantics without requiring a real
//! Dingtalk API connection.

use magiclaw::channels::channel_trait::{Channel, HealthStatus};
use magiclaw::channels::dingtalk::channel::DingtalkChannel;
use magiclaw::channels::dingtalk::error_semantics::DingtalkErrorSemantics;
use magiclaw::domain::entities::message::MessageContent;
use magiclaw::domain::error::ChannelError;

#[tokio::test]
async fn dingtalk_stub_health_and_send() {
    let ch = DingtalkChannel::new();
    let h: HealthStatus = ch.health().await.unwrap();
    assert!(h.healthy);
    assert!(h.detail.contains("skeleton"));

    let receipt = ch
        .send_message("manager123", &MessageContent::Text("hi".into()))
        .await
        .unwrap();
    assert!(receipt.platform_msg_id.unwrap().starts_with("dingtalk_stub_"));
    assert!(receipt.timestamp_ms > 0);
}

#[tokio::test]
async fn dingtalk_content_mapping_roundtrip() {
    let ch = DingtalkChannel::new();

    // Text
    let receipt = ch.send_message("u1", &MessageContent::Text("hello".into())).await.unwrap();
    assert!(!receipt.message_id.is_empty());

    // Image
    let receipt = ch.send_message("u1", &MessageContent::Image { url: "img_abc".into(), media_id: None }).await.unwrap();
    assert!(!receipt.message_id.is_empty());

    // File
    let receipt = ch.send_message("u1", &MessageContent::File { url: "file_xyz".into(), name: "doc.pdf".into(), size: 1024 }).await.unwrap();
    assert!(!receipt.message_id.is_empty());
}

#[tokio::test]
async fn dingtalk_stop_graceful() {
    let ch = DingtalkChannel::new();
    ch.stop().await.unwrap();
}

#[test]
fn error_semantics_classification() {
    // HTTP level
    assert!(DingtalkErrorSemantics::from_http_status(429).should_retry());
    assert!(!DingtalkErrorSemantics::from_http_status(429).is_terminal());

    assert!(!DingtalkErrorSemantics::from_http_status(400).should_retry());
    assert!(DingtalkErrorSemantics::from_http_status(400).is_terminal());

    // Dingtalk error codes
    assert!(DingtalkErrorSemantics::from_dingtalk_errcode(33001).should_retry());
    assert!(DingtalkErrorSemantics::from_dingtalk_errcode(-1).should_retry());
    assert!(DingtalkErrorSemantics::from_dingtalk_errcode(40014).is_terminal());
    assert!(DingtalkErrorSemantics::from_dingtalk_errcode(40001).is_terminal());
}

#[test]
fn channel_error_is_retryable() {
    // Transport errors are retryable.
    let e = ChannelError::Transport("timeout".into());
    assert!(e.is_retryable());

    // Rate limit errors are retryable.
    let e = ChannelError::RateLimited { retry_after_secs: 10 };
    assert!(e.is_retryable());

    // Auth expired is terminal.
    let e = ChannelError::AuthExpired { errcode: 40014, detail: "expired".into() };
    assert!(e.is_terminal());
}

#[tokio::test]
async fn dingtalk_from_config_empty_returns_stub() {
    // Empty credentials: stub with skeleton behavior
    let ch = DingtalkChannel::from_config("", "", "", "");
    let h = ch.health().await.unwrap();
    assert!(h.detail.contains("skeleton"));
}
