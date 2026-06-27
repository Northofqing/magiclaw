//! Dingtalk channel — full implementation with access_token management,
//! message type mapping, and error semantics.
//!
//! API reference: https://open.dingtalk.com/document/orgapp
//! - Token: POST /gettoken?appkey={}&appsecret={}
//! - Send (corpconversation): POST /topapi/message/corpconversation/asyncsend_v2

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};
use tokio::time::{Duration, Instant};

use crate::channels::channel_trait::{Channel, HealthStatus, SendReceipt};
use crate::domain::entities::message::{Message, MessageContent};
use crate::domain::error::ChannelError;
use crate::domain::value_objects::route_key::ChannelId;

use super::error_semantics::DingtalkErrorSemantics;

// ── API data types ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TokenResponse {
    errcode: i64,
    errmsg: Option<String>,
    access_token: Option<String>,
    expires_in: Option<i64>,
}

#[derive(Debug, Serialize)]
struct SendRequest<'a> {
    agent_id: &'a str,
    userid_list: &'a str,
    msg: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct SendResponse {
    #[serde(default)]
    errcode: i64,
    #[serde(default)]
    errmsg: String,
    task_id: Option<i64>,
}

// ── Cached token ────────────────────────────────────────────────────────

struct TokenCache {
    token: String,
    expires_at: Instant,
}

impl TokenCache {
    fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

// ── Channel ─────────────────────────────────────────────────────────────

enum DingtalkMode {
    Stub,
    OpenApi {
        client: reqwest::Client,
        config: DingtalkConfig,
        token_cache: Mutex<Option<TokenCache>>,
    },
}

#[derive(Clone)]
struct DingtalkConfig {
    app_key: String,
    app_secret: String,
    agent_id: String,
    base_url: String,
    timeout_secs: u64,
}

pub struct DingtalkChannel {
    channel_id: ChannelId,
    mode: DingtalkMode,
}

impl Default for DingtalkChannel {
    fn default() -> Self { Self::new() }
}

impl DingtalkChannel {
    pub fn new() -> Self {
        Self { channel_id: ChannelId::new("dingtalk"), mode: DingtalkMode::Stub }
    }

    /// Build from config. `agent_id` is the corp app's agent ID.
    pub fn from_config(app_key: &str, app_secret: &str, agent_id: &str, base_url: &str) -> Self {
        if app_key.is_empty() || app_secret.is_empty() || agent_id.is_empty() {
            return Self::new();
        }
        Self {
            channel_id: ChannelId::new("dingtalk"),
            mode: DingtalkMode::OpenApi {
                client: reqwest::Client::new(),
                config: DingtalkConfig {
                    app_key: app_key.to_string(),
                    app_secret: app_secret.to_string(),
                    agent_id: agent_id.to_string(),
                    base_url: if base_url.is_empty() {
                        "https://oapi.dingtalk.com".into()
                    } else {
                        base_url.to_string()
                    },
                    timeout_secs: 30,
                },
                token_cache: Mutex::new(None),
            },
        }
    }
}

/// Fetch or refresh the access_token. Thread-safe — callers race on cache
/// under a tokio::sync::Mutex.
async fn ensure_access_token(
    client: &reqwest::Client,
    config: &DingtalkConfig,
    cache: &Mutex<Option<TokenCache>>,
) -> Result<String, ChannelError> {
    {
        let guard = cache.lock().await;
        if let Some(ref c) = *guard {
            if !c.is_expired() {
                return Ok(c.token.clone());
            }
        }
    }

    let url = format!(
        "{}/gettoken?appkey={}&appsecret={}",
        config.base_url, config.app_key, config.app_secret
    );
    let resp = client
        .get(&url)
        .timeout(Duration::from_secs(config.timeout_secs))
        .send()
        .await
        .map_err(|e| ChannelError::Transport(format!("dingtalk gettoken: {}", e)))?;

    let _status = resp.status();
    let body: TokenResponse = resp
        .json()
        .await
        .map_err(|e| ChannelError::Transport(format!("dingtalk gettoken parse: {}", e)))?;

    if body.errcode != 0 {
        return Err(match DingtalkErrorSemantics::from_dingtalk_errcode(body.errcode) {
            DingtalkErrorSemantics::Retryable => ChannelError::RateLimited {
                retry_after_secs: 60,
            },
            _ => ChannelError::AuthExpired {
                errcode: body.errcode,
                detail: body.errmsg.unwrap_or_default(),
            },
        });
    }

    let token = body
        .access_token
        .ok_or_else(|| ChannelError::Internal("dingtalk: no access_token in response".into()))?;
    let expires_in = body.expires_in.unwrap_or(7200) as u64;
    // Expire 60s early to avoid edge-case expiry mid-request.
    let expires_at = Instant::now() + Duration::from_secs(expires_in.saturating_sub(60));

    let mut guard = cache.lock().await;
    *guard = Some(TokenCache {
        token: token.clone(),
        expires_at,
    });
    Ok(token)
}

/// Map MessageContent to Dingtalk message JSON body.
fn map_content(content: &MessageContent) -> Result<serde_json::Value, ChannelError> {
    match content {
        MessageContent::Text(text) => Ok(serde_json::json!({
            "msgtype": "text",
            "text": { "content": text }
        })),
        MessageContent::Image { url, .. } => Ok(serde_json::json!({
            "msgtype": "image",
            "image": { "media_id": url }
        })),
        MessageContent::File { url, name, .. } => Ok(serde_json::json!({
            "msgtype": "file",
            "file": { "media_id": url, "file_name": name.as_str() }
        })),
        _ => Err(ChannelError::Unsupported(
            "dingtalk: unsupported message content type".to_string(),
        )),
    }
}

#[async_trait]
impl Channel for DingtalkChannel {
    fn id(&self) -> ChannelId {
        self.channel_id.clone()
    }

    async fn start(&self, _inbound_tx: mpsc::Sender<Message>) -> Result<(), ChannelError> {
        match &self.mode {
            DingtalkMode::Stub => tracing::info!("Dingtalk channel started (skeleton)"),
            DingtalkMode::OpenApi { .. } => tracing::info!("Dingtalk channel started (openapi)"),
        }
        Ok(())
    }

    async fn send_message(
        &self,
        to: &str,
        content: &MessageContent,
    ) -> Result<SendReceipt, ChannelError> {
        match &self.mode {
            DingtalkMode::Stub => {
                let _msg = map_content(content)?;
                tracing::info!(to = %to, "Dingtalk send_message (skeleton)");
                Ok(SendReceipt {
                    message_id: uuid::Uuid::new_v4().to_string(),
                    platform_msg_id: Some(format!("dingtalk_stub_{}", chrono::Utc::now().timestamp_millis())),
                    timestamp_ms: chrono::Utc::now().timestamp_millis(),
                })
            }
            DingtalkMode::OpenApi {
                client,
                config,
                token_cache,
            } => {
                let token = ensure_access_token(client, config, token_cache).await?;
                let msg_body = map_content(content)?;

                let url = format!(
                    "{}/topapi/message/corpconversation/asyncsend_v2?access_token={}",
                    config.base_url, token
                );

                let req = SendRequest {
                    agent_id: &config.agent_id,
                    userid_list: to,
                    msg: msg_body,
                };

                let resp = client
                    .post(&url)
                    .json(&req)
                    .timeout(Duration::from_secs(config.timeout_secs))
                    .send()
                    .await
                    .map_err(|e| ChannelError::Transport(format!("dingtalk send: {}", e)))?;

                let status = resp.status();
                let body: SendResponse = resp
                    .json()
                    .await
                    .map_err(|e| ChannelError::Transport(format!("dingtalk send parse: {}", e)))?;

                if body.errcode != 0 {
                    let sem = DingtalkErrorSemantics::from_dingtalk_errcode(body.errcode);
                    let sem = if matches!(sem, DingtalkErrorSemantics::Unknown) {
                        DingtalkErrorSemantics::from_http_status(status.as_u16())
                    } else {
                        sem
                    };
                    return Err(match sem {
                        DingtalkErrorSemantics::Retryable => ChannelError::RateLimited {
                            retry_after_secs: 10,
                        },
                        _ => ChannelError::ContentRejected(format!(
                            "dingtalk send rejected: errcode={} msg={}",
                            body.errcode, body.errmsg
                        )),
                    });
                }

                Ok(SendReceipt {
                    message_id: uuid::Uuid::new_v4().to_string(),
                    platform_msg_id: body.task_id.map(|id| id.to_string()),
                    timestamp_ms: chrono::Utc::now().timestamp_millis(),
                })
            }
        }
    }

    async fn stop(&self) -> Result<(), ChannelError> {
        tracing::info!("Dingtalk channel stopped");
        Ok(())
    }

    async fn health(&self) -> Result<HealthStatus, ChannelError> {
        Ok(HealthStatus {
            channel: "dingtalk".into(),
            healthy: true,
            detail: match self.mode {
                DingtalkMode::Stub => "skeleton — not connected to platform".into(),
                DingtalkMode::OpenApi { .. } => "openapi configured".into(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entities::message::MessageContent;

    #[tokio::test]
    async fn dingtalk_health_returns_status() {
        let ch = DingtalkChannel::new();
        let h = ch.health().await.unwrap();
        assert!(h.healthy);
        assert!(h.detail.contains("skeleton"));
    }

    #[tokio::test]
    async fn dingtalk_send_stub_returns_receipt() {
        let ch = DingtalkChannel::new();
        let receipt = ch
            .send_message("user1", &MessageContent::Text("hi".into()))
            .await
            .unwrap();
        assert!(receipt.platform_msg_id.unwrap().starts_with("dingtalk_stub_"));
    }

    #[tokio::test]
    async fn dingtalk_send_text_maps_msgtype() {
        let ch = DingtalkChannel::new();
        // Stub mode verifies the content map doesn't panic.
        let receipt = ch
            .send_message("user1", &MessageContent::Text("test".into()))
            .await
            .unwrap();
        assert!(!receipt.message_id.is_empty());
    }

    #[test]
    fn map_text_content_produces_json() {
        let body = map_content(&MessageContent::Text("hello".into())).unwrap();
        assert_eq!(body["msgtype"], "text");
        assert_eq!(body["text"]["content"], "hello");
    }

}
