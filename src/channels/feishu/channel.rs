use async_trait::async_trait;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::sync::mpsc;
use tokio_util::io::ReaderStream;

use crate::channels::channel_trait::{Channel, HealthStatus, SendReceipt};
use crate::domain::entities::message::{Message, MessageContent};
use crate::domain::error::ChannelError;
use crate::domain::value_objects::route_key::{ChannelId, ConversationType, RouteKey};
use crate::infrastructure::config::FeishuConfig;

fn feishu_channel_id(account_id: &str) -> ChannelId {
    let acc = account_id.trim();
    if acc.is_empty() || acc == "default" {
        ChannelId::new("feishu")
    } else {
        ChannelId::new(format!("feishu:{}", acc))
    }
}

#[derive(Debug)]
pub enum FeishuWebhookDispatch {
    UrlVerification { challenge: String },
    Message(Message),
    Ignore,
}

#[derive(Debug, Deserialize)]
struct FeishuWebhookEnvelope {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    challenge: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    header: Option<FeishuEventHeader>,
    #[serde(default)]
    event: Option<FeishuEventBody>,
}

#[derive(Debug, Deserialize)]
struct FeishuEventHeader {
    #[serde(default)]
    event_id: String,
    #[serde(default)]
    event_type: String,
    #[serde(default)]
    create_time: String,
    #[serde(default)]
    token: String,
}

#[derive(Debug, Deserialize)]
struct FeishuEventBody {
    #[serde(default)]
    sender: FeishuSender,
    #[serde(default)]
    message: FeishuInboundMessage,
}

#[derive(Debug, Default, Deserialize)]
struct FeishuSender {
    #[serde(default)]
    sender_id: FeishuSenderId,
}

#[derive(Debug, Default, Deserialize)]
struct FeishuSenderId {
    #[serde(default)]
    open_id: String,
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    union_id: String,
}

#[derive(Debug, Default, Deserialize)]
struct FeishuInboundMessage {
    #[serde(default)]
    message_id: String,
    #[serde(default)]
    chat_id: String,
    #[serde(default)]
    chat_type: String,
    #[serde(default)]
    message_type: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    create_time: String,
}

pub fn parse_webhook_event(
    payload: serde_json::Value,
    cfg: &FeishuConfig,
) -> Result<FeishuWebhookDispatch, String> {
    let envelope: FeishuWebhookEnvelope = serde_json::from_value(payload)
        .map_err(|e| format!("invalid feishu webhook payload: {}", e))?;

    let expected_token = cfg.verification_token.trim();
    if !expected_token.is_empty() {
        let candidate = if !envelope.token.trim().is_empty() {
            envelope.token.trim()
        } else {
            envelope
                .header
                .as_ref()
                .map(|h| h.token.trim())
                .unwrap_or_default()
        };
        if candidate != expected_token {
            return Err("feishu webhook token mismatch".into());
        }
    }

    if envelope.r#type == "url_verification" {
        return Ok(FeishuWebhookDispatch::UrlVerification {
            challenge: envelope.challenge,
        });
    }

    let Some(header) = envelope.header else {
        return Ok(FeishuWebhookDispatch::Ignore);
    };
    if header.event_type != "im.message.receive_v1" {
        return Ok(FeishuWebhookDispatch::Ignore);
    }

    let Some(event) = envelope.event else {
        return Ok(FeishuWebhookDispatch::Ignore);
    };

    let message_id = if !event.message.message_id.trim().is_empty() {
        event.message.message_id.trim().to_string()
    } else if !header.event_id.trim().is_empty() {
        header.event_id.trim().to_string()
    } else {
        uuid::Uuid::new_v4().to_string()
    };

    let peer_id = if !event.sender.sender_id.open_id.trim().is_empty() {
        event.sender.sender_id.open_id.trim().to_string()
    } else if !event.sender.sender_id.user_id.trim().is_empty() {
        event.sender.sender_id.user_id.trim().to_string()
    } else if !event.sender.sender_id.union_id.trim().is_empty() {
        event.sender.sender_id.union_id.trim().to_string()
    } else {
        String::new()
    };

    let conversation_id = if !event.message.chat_id.trim().is_empty() {
        event.message.chat_id.trim().to_string()
    } else if !peer_id.trim().is_empty() {
        peer_id.clone()
    } else {
        "unknown".to_string()
    };

    let conversation_type = if event.message.chat_type == "group" {
        ConversationType::Group
    } else {
        ConversationType::Direct
    };

    let content_json: serde_json::Value = serde_json::from_str(&event.message.content)
        .unwrap_or(serde_json::Value::Null);
    let content = match event.message.message_type.as_str() {
        "text" => content_json
            .get("text")
            .and_then(|v| v.as_str())
            .map(|v| MessageContent::Text(v.to_string()))
            .unwrap_or(MessageContent::Unknown),
        "image" => {
            let image_key = content_json
                .get("image_key")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if image_key.is_empty() {
                MessageContent::Unknown
            } else {
                MessageContent::Image {
                    url: String::new(),
                    media_id: Some(image_key),
                }
            }
        }
        "file" => {
            let file_key = content_json
                .get("file_key")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let name = content_json
                .get("file_name")
                .and_then(|v| v.as_str())
                .unwrap_or("file")
                .to_string();
            let size = content_json
                .get("file_size")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if file_key.is_empty() {
                MessageContent::Unknown
            } else {
                MessageContent::File {
                    url: file_key,
                    name,
                    size,
                }
            }
        }
        _ => MessageContent::Unknown,
    };

    let timestamp_ms = event
        .message
        .create_time
        .parse::<i64>()
        .ok()
        .or_else(|| header.create_time.parse::<i64>().ok())
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

    let route_key = RouteKey::new(
        feishu_channel_id(&cfg.account_id),
        conversation_id,
        peer_id,
        conversation_type,
    );

    Ok(FeishuWebhookDispatch::Message(Message::new_inbound(
        message_id,
        route_key,
        None,
        timestamp_ms,
        content,
    )))
}

pub fn verify_webhook_signature(
    headers: &axum::http::HeaderMap,
    body: &[u8],
    cfg: &FeishuConfig,
) -> Result<(), String> {
    let secret = cfg.signing_secret.trim();
    if secret.is_empty() {
        return Ok(());
    }

    let ts = headers
        .get("x-lark-request-timestamp")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| "missing x-lark-request-timestamp".to_string())?;
    let nonce = headers
        .get("x-lark-request-nonce")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| "missing x-lark-request-nonce".to_string())?;
    let signature = headers
        .get("x-lark-signature")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| "missing x-lark-signature".to_string())?;

    let payload = format!(
        "{}{}{}{}",
        ts,
        nonce,
        secret,
        String::from_utf8_lossy(body)
    );

    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|e| format!("invalid secret: {}", e))?;
    mac.update(payload.as_bytes());
    let digest = mac.finalize().into_bytes();

    let provided = signature
        .strip_prefix("sha256=")
        .unwrap_or(signature)
        .trim();
    let expected_b64 = base64::engine::general_purpose::STANDARD.encode(digest);
    if expected_b64 == provided {
        return Ok(());
    }

    let expected_hex: String = digest.iter().map(|b| format!("{:02x}", b)).collect();
    if expected_hex.eq_ignore_ascii_case(provided) {
        return Ok(());
    }

    Err("invalid feishu webhook signature".into())
}

#[derive(Debug)]
struct SessionState {
    tenant_access_token: String,
}

enum FeishuMode {
    Stub,
    OpenApi {
        config: Box<FeishuConfig>,
        client: reqwest::Client,
        session: std::sync::Arc<tokio::sync::Mutex<SessionState>>,
    },
}

#[derive(Debug, Deserialize)]
struct FeishuTokenResponse {
    code: i32,
    msg: String,
    #[serde(default)]
    tenant_access_token: String,
}

#[derive(Debug, Deserialize)]
struct FeishuSendEnvelope {
    code: i32,
    msg: String,
    #[serde(default)]
    data: Option<FeishuSendData>,
}

#[derive(Debug, Deserialize)]
struct FeishuSendData {
    #[serde(default)]
    message_id: String,
}

#[derive(Debug, Deserialize)]
struct FeishuUploadEnvelope {
    code: i32,
    msg: String,
    #[serde(default)]
    data: Option<FeishuUploadData>,
}

#[derive(Debug, Deserialize)]
struct FeishuUploadData {
    #[serde(default)]
    image_key: String,
    #[serde(default)]
    file_key: String,
}

#[derive(Debug, Serialize)]
struct FeishuSendRequest<'a> {
    receive_id: &'a str,
    msg_type: &'a str,
    content: String,
}

fn file_name_from_url(url: &str, fallback: &str) -> String {
    if let Ok(parsed) = reqwest::Url::parse(url) {
        if let Some(last) = parsed
            .path_segments()
            .and_then(|mut s| s.next_back())
            .filter(|v| !v.is_empty())
        {
            return last.to_string();
        }
    }
    fallback.to_string()
}

struct UploadMediaParams<'a> {
    client: &'a reqwest::Client,
    cfg: &'a FeishuConfig,
    token: &'a str,
    endpoint: &'a str,
    media_field: &'a str,
    type_field: &'a str,
    type_value: &'a str,
    source: &'a str,
    fallback_name: &'a str,
}

async fn upload_media(params: &UploadMediaParams<'_>) -> Result<FeishuUploadData, String> {
    let &UploadMediaParams { client, cfg, token, endpoint, media_field, type_field, type_value, source, fallback_name } = params;
    let (part, filename) = if let Some(path_text) = source.strip_prefix("file://") {
        let path = std::path::Path::new(path_text);
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|e| format!("open media file failed: {}", e))?;
        let meta = file
            .metadata()
            .await
            .map_err(|e| format!("read media metadata failed: {}", e))?;
        let name = path
            .file_name()
            .and_then(|v| v.to_str())
            .filter(|v| !v.is_empty())
            .unwrap_or(fallback_name)
            .to_string();
        let stream = ReaderStream::new(file);
        (
            reqwest::multipart::Part::stream_with_length(reqwest::Body::wrap_stream(stream), meta.len())
                .file_name(name.clone()),
            name,
        )
    } else if source.starts_with("http://") || source.starts_with("https://") {
        let resp = client
            .get(source)
            .send()
            .await
            .map_err(|e| format!("download media failed: {}", e))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("download media failed: HTTP {}", status));
        }
        let name = file_name_from_url(source, fallback_name);
        let stream = resp.bytes_stream();
        (
            reqwest::multipart::Part::stream(reqwest::Body::wrap_stream(stream)).file_name(name.clone()),
            name,
        )
    } else {
        let path = std::path::Path::new(source);
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|e| format!("open media file failed: {}", e))?;
        let meta = file
            .metadata()
            .await
            .map_err(|e| format!("read media metadata failed: {}", e))?;
        let name = path
            .file_name()
            .and_then(|v| v.to_str())
            .filter(|v| !v.is_empty())
            .unwrap_or(fallback_name)
            .to_string();
        let stream = ReaderStream::new(file);
        (
            reqwest::multipart::Part::stream_with_length(reqwest::Body::wrap_stream(stream), meta.len())
                .file_name(name.clone()),
            name,
        )
    };

    let url = format!(
        "{}/{}",
        cfg.base_url.trim_end_matches('/'),
        endpoint.trim_start_matches('/')
    );
    let form = reqwest::multipart::Form::new()
        .text(type_field.to_string(), type_value.to_string())
        .part(media_field.to_string(), part);

    let resp = client
        .post(url)
        .bearer_auth(token)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("upload {} failed: {}", filename, e))?;

    let status = resp.status();
    let parsed: FeishuUploadEnvelope = resp
        .json()
        .await
        .map_err(|e| format!("upload parse failed: {}", e))?;

    if !status.is_success() || parsed.code != 0 {
        return Err(format!(
            "upload rejected: status={} code={} msg={}",
            status, parsed.code, parsed.msg
        ));
    }

    parsed.data.ok_or_else(|| "upload response missing data".into())
}

fn looks_like_upload_source(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.starts_with("file://")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
    {
        return true;
    }
    std::path::Path::new(trimmed).exists()
}

async fn ensure_tenant_token(
    client: &reqwest::Client,
    cfg: &FeishuConfig,
    session: &std::sync::Arc<tokio::sync::Mutex<SessionState>>,
) -> Result<String, String> {
    {
        let state = session.lock().await;
        if !state.tenant_access_token.trim().is_empty() {
            return Ok(state.tenant_access_token.clone());
        }
    }

    if cfg.app_id.trim().is_empty() || cfg.app_secret.trim().is_empty() {
        return Err(
            "feishu token missing: set tenant_access_token or app_id/app_secret".into(),
        );
    }

    let endpoint = format!(
        "{}/open-apis/auth/v3/tenant_access_token/internal",
        cfg.base_url.trim_end_matches('/')
    );
    let body = serde_json::json!({
        "app_id": cfg.app_id,
        "app_secret": cfg.app_secret,
    });

    let resp = client
        .post(endpoint)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("feishu token request failed: {}", e))?;

    let status = resp.status();
    let parsed: FeishuTokenResponse = resp
        .json()
        .await
        .map_err(|e| format!("feishu token parse failed: {}", e))?;

    if !status.is_success() || parsed.code != 0 || parsed.tenant_access_token.trim().is_empty() {
        return Err(format!(
            "feishu token exchange failed: status={} code={} msg={}",
            status, parsed.code, parsed.msg
        ));
    }

    let mut state = session.lock().await;
    state.tenant_access_token = parsed.tenant_access_token.clone();
    Ok(state.tenant_access_token.clone())
}

fn map_content(content: &MessageContent) -> Result<(&'static str, String), String> {
    match content {
        MessageContent::Text(t) => Ok((
            "text",
            serde_json::json!({ "text": t }).to_string(),
        )),
        MessageContent::Image { media_id, .. } => {
            let Some(key) = media_id.as_ref().filter(|v| !v.trim().is_empty()) else {
                return Err("feishu image send requires media_id as image_key".into());
            };
            Ok(("image", serde_json::json!({ "image_key": key }).to_string()))
        }
        MessageContent::File { url, .. } => {
            // For parity with current wechat implementation, require a pre-uploaded key
            // and pass it through the url field for now.
            if url.trim().is_empty() {
                return Err("feishu file send requires pre-uploaded file_key in url".into());
            }
            Ok(("file", serde_json::json!({ "file_key": url }).to_string()))
        }
        MessageContent::Unknown => Err("feishu cannot send Unknown content".into()),
    }
}

/// Feishu (Lark) channel skeleton. Full implementation in Phase 4-5.
pub struct FeishuChannel {
    channel_id: ChannelId,
    mode: FeishuMode,
}

impl Default for FeishuChannel {
    fn default() -> Self { Self::new() }
}

impl FeishuChannel {
    pub fn new() -> Self {
        Self {
            channel_id: ChannelId::new("feishu"),
            mode: FeishuMode::Stub,
        }
    }

    pub fn from_config(cfg: FeishuConfig) -> Self {
        let channel_id = feishu_channel_id(&cfg.account_id);
        if cfg.enabled {
            return Self {
                channel_id,
                mode: FeishuMode::OpenApi {
                    client: reqwest::Client::builder()
                        .timeout(std::time::Duration::from_millis(cfg.timeout_ms.max(1000)))
                        .build()
                        .unwrap_or_else(|_| reqwest::Client::new()),
                    session: std::sync::Arc::new(tokio::sync::Mutex::new(SessionState {
                        tenant_access_token: cfg.tenant_access_token.clone(),
                    })),
                    config: Box::new(cfg.clone()),
                },
            };
        }
        Self {
            channel_id,
            mode: FeishuMode::Stub,
        }
    }
}

#[async_trait]
impl Channel for FeishuChannel {
    fn id(&self) -> ChannelId {
        self.channel_id.clone()
    }

    async fn start(&self, _inbound_tx: mpsc::Sender<Message>) -> Result<(), ChannelError> {
        match self.mode {
            FeishuMode::Stub => tracing::info!("Feishu channel started (skeleton)"),
            FeishuMode::OpenApi { .. } => tracing::info!("Feishu channel started (openapi enabled)"),
        }
        Ok(())
    }

    async fn send_message(&self, to: &str, content: &MessageContent) -> Result<SendReceipt, ChannelError> {
        match &self.mode {
            FeishuMode::Stub => {
                let (msg_type, _) = map_content(content)?;
                tracing::info!(to = %to, msg_type = %msg_type, "Feishu send_message (skeleton)");
                Ok(SendReceipt {
                    message_id: uuid::Uuid::new_v4().to_string(),
                    platform_msg_id: Some(format!("feishu_stub_{}", chrono::Utc::now().timestamp_millis())),
                    timestamp_ms: chrono::Utc::now().timestamp_millis(),
                })
            }
            FeishuMode::OpenApi {
                config,
                client,
                session,
            } => {
                let token = ensure_tenant_token(client, config, session).await?;
                let (msg_type, body_content) = match content {
                    MessageContent::Image { url, media_id } if media_id.as_deref().unwrap_or_default().trim().is_empty() => {
                        if url.trim().is_empty() {
                            return Err("feishu image send requires media_id or uploadable url/path".into());
                        }
                        if !looks_like_upload_source(url) {
                            (
                                "image",
                                serde_json::json!({ "image_key": url }).to_string(),
                            )
                        } else {
                            let uploaded = upload_media(&UploadMediaParams {
                                client,
                                cfg: config,
                                token: &token,
                                endpoint: "/open-apis/im/v1/images",
                                media_field: "image",
                                type_field: "image_type",
                                type_value: "message",
                                source: url,
                                fallback_name: "image.bin",
                            })
                            .await?;
                            if uploaded.image_key.trim().is_empty() {
                                return Err("feishu image upload returned empty image_key".into());
                            }
                            (
                                "image",
                                serde_json::json!({ "image_key": uploaded.image_key }).to_string(),
                            )
                        }
                    }
                    MessageContent::File { url, name, .. } => {
                        if url.trim().is_empty() {
                            return Err("feishu file send requires uploadable url/path or file_key".into());
                        }
                        if !looks_like_upload_source(url) {
                            (
                                "file",
                                serde_json::json!({ "file_key": url }).to_string(),
                            )
                        } else {
                        let uploaded = upload_media(&UploadMediaParams {
                            client,
                            cfg: config,
                            token: &token,
                            endpoint: "/open-apis/im/v1/files",
                            media_field: "file",
                            type_field: "file_type",
                            type_value: "stream",
                            source: url,
                            fallback_name: if name.trim().is_empty() { "file.bin" } else { name },
                        })
                        .await?;
                        let file_key = if uploaded.file_key.trim().is_empty() {
                            // Backward compatible path: treat url as a pre-uploaded file_key.
                            url.clone()
                        } else {
                            uploaded.file_key
                        };
                        ("file", serde_json::json!({ "file_key": file_key }).to_string())
                        }
                    }
                    _ => map_content(content)?,
                };

                let endpoint = format!(
                    "{}/open-apis/im/v1/messages",
                    config.base_url.trim_end_matches('/')
                );

                let req = FeishuSendRequest {
                    receive_id: to,
                    msg_type,
                    content: body_content,
                };

                let resp = client
                    .post(endpoint)
                    .query(&[("receive_id_type", config.receive_id_type.as_str())])
                    .bearer_auth(token)
                    .json(&req)
                    .send()
                    .await
                    .map_err(|e| format!("feishu send failed: {}", e))?;

                let status = resp.status();
                let parsed: FeishuSendEnvelope = resp
                    .json()
                    .await
                    .map_err(|e| format!("feishu send parse failed: {}", e))?;

                if !status.is_success() || parsed.code != 0 {
                    return Err(ChannelError::ContentRejected(format!(
                        "feishu send rejected: status={} code={} msg={}",
                        status, parsed.code, parsed.msg
                    )));
                }

                Ok(SendReceipt {
                    message_id: uuid::Uuid::new_v4().to_string(),
                    platform_msg_id: parsed
                        .data
                        .as_ref()
                        .map(|d| d.message_id.clone())
                        .filter(|v| !v.is_empty()),
                    timestamp_ms: chrono::Utc::now().timestamp_millis(),
                })
            }
        }
    }

    async fn stop(&self) -> Result<(), ChannelError> {
        tracing::info!("Feishu channel stopped");
        Ok(())
    }

    async fn health(&self) -> Result<HealthStatus, ChannelError> {
        Ok(HealthStatus {
            channel: "feishu".into(),
            healthy: true,
            detail: match self.mode {
                FeishuMode::Stub => "skeleton — not connected to platform".into(),
                FeishuMode::OpenApi { .. } => "openapi configured".into(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entities::message::MessageContent;

    #[tokio::test]
    async fn feishu_health_returns_skeleton_status() {
        let ch = FeishuChannel::new();
        let h = ch.health().await.unwrap();
        assert!(h.healthy);
        assert!(h.detail.contains("skeleton"));
    }

    #[tokio::test]
    async fn feishu_send_returns_stub_receipt() {
        let ch = FeishuChannel::new();
        let receipt = ch.send_message("user1", &MessageContent::Text("hi".into())).await.unwrap();
        assert!(receipt.platform_msg_id.unwrap().starts_with("feishu_stub_"));
    }

    #[test]
    fn parse_webhook_url_verification() {
        let cfg = FeishuConfig {
            verification_token: "verify-token".into(),
            ..FeishuConfig::default()
        };
        let payload = serde_json::json!({
            "type": "url_verification",
            "challenge": "abc123",
            "token": "verify-token"
        });

        let out = parse_webhook_event(payload, &cfg).unwrap();
        match out {
            FeishuWebhookDispatch::UrlVerification { challenge } => {
                assert_eq!(challenge, "abc123");
            }
            _ => panic!("expected UrlVerification"),
        }
    }

    #[test]
    fn parse_webhook_message_event() {
        let cfg = FeishuConfig {
            verification_token: "verify-token".into(),
            ..FeishuConfig::default()
        };
        let payload = serde_json::json!({
            "header": {
                "event_id": "evt_1",
                "event_type": "im.message.receive_v1",
                "create_time": "1718000000000",
                "token": "verify-token"
            },
            "event": {
                "sender": {
                    "sender_id": {
                        "open_id": "ou_user_1"
                    }
                },
                "message": {
                    "message_id": "om_1",
                    "chat_id": "oc_1",
                    "chat_type": "group",
                    "message_type": "text",
                    "content": "{\"text\":\"hello from feishu\"}",
                    "create_time": "1718000000001"
                }
            }
        });

        let out = parse_webhook_event(payload, &cfg).unwrap();
        match out {
            FeishuWebhookDispatch::Message(msg) => {
                assert_eq!(msg.id, "om_1");
                assert_eq!(msg.route_key.channel.as_str(), "feishu");
                assert_eq!(msg.route_key.conversation_id, "oc_1");
                assert_eq!(msg.route_key.peer_id, "ou_user_1");
                assert!(matches!(msg.route_key.conversation_type, ConversationType::Group));
                assert!(matches!(msg.content, MessageContent::Text(ref v) if v == "hello from feishu"));
            }
            _ => panic!("expected Message"),
        }
    }

    #[test]
    fn verify_webhook_signature_accepts_valid_base64() {
        let cfg = FeishuConfig {
            signing_secret: "secret-key".into(),
            ..FeishuConfig::default()
        };
        let body = b"{\"type\":\"url_verification\",\"challenge\":\"x\"}";
        let ts = "1718000000";
        let nonce = "nonce-1";
        let payload = format!("{}{}{}{}", ts, nonce, cfg.signing_secret, String::from_utf8_lossy(body));

        let mut mac = Hmac::<Sha256>::new_from_slice(cfg.signing_secret.as_bytes()).unwrap();
        mac.update(payload.as_bytes());
        let signature = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());

        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-lark-request-timestamp", ts.parse().unwrap());
        headers.insert("x-lark-request-nonce", nonce.parse().unwrap());
        headers.insert("x-lark-signature", signature.parse().unwrap());

        assert!(verify_webhook_signature(&headers, body, &cfg).is_ok());
    }
}
