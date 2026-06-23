use async_trait::async_trait;
use tokio::sync::mpsc;
use std::collections::HashMap;

use crate::channels::channel_trait::{Channel, HealthStatus, SendReceipt};
use crate::domain::entities::message::{Message, MessageContent};
use crate::domain::ports::media::{MediaRef, MediaUploader};
use crate::domain::ports::sync_buf_store::SyncBufStore;
use crate::domain::value_objects::route_key::ChannelId;
use crate::infrastructure::config::WeChatConfig;

use super::ilink::{extract_latest_context_token, get_updates_via_ilink, send_text_via_ilink, ILinkSendConfig};
use super::media::{FileMediaSource, MediaLocation, DEFAULT_CHUNK_BYTES};

#[derive(Debug)]
struct SessionState {
    /// Map from user_id to context_token (per-user token cache)
    context_tokens: Arc<tokio::sync::RwLock<HashMap<String, String>>>,
    sync_buf: String,
}

impl SessionState {
    pub async fn get_context_token(&self, user_id: &str) -> Option<String> {
        self.context_tokens.read().await.get(user_id).cloned()
    }

    pub async fn set_context_token(&self, user_id: String, token: String) {
        self.context_tokens.write().await.insert(user_id, token);
    }

    pub async fn clear_context_tokens(&self) {
        self.context_tokens.write().await.clear();
    }
}

use std::sync::Arc;

enum WeChatMode {
    Stub,
    ILink {
        config: ILinkSendConfig,
        client: reqwest::Client,
        session: std::sync::Arc<tokio::sync::Mutex<SessionState>>,
        sync_store: Option<std::sync::Arc<dyn SyncBufStore>>,
        account_id: String,
    },
}

/// WeChat channel skeleton. Full ilink integration remains a later step,
/// but runtime send path needs a concrete channel implementation now.
pub struct WeChatChannel {
    channel_id: ChannelId,
    mode: WeChatMode,
    /// Optional injected media uploader (red line 2.4). When absent, media
    /// sends are rejected, preserving the prior text-only behavior.
    media_uploader: Option<std::sync::Arc<dyn MediaUploader>>,
    media_chunk_bytes: usize,
    media_size_limit: u64,
}

impl Default for WeChatChannel {
    fn default() -> Self { Self::new() }
}

impl WeChatChannel {
    pub fn new() -> Self {
        Self {
            channel_id: ChannelId::new("wechat"),
            mode: WeChatMode::Stub,
            media_uploader: None,
            media_chunk_bytes: DEFAULT_CHUNK_BYTES,
            media_size_limit: 0,
        }
    }

    /// Inject a media uploader, enabling media sends. `size_limit` of 0 disables
    /// the limit.
    pub fn with_media_uploader(
        mut self,
        uploader: std::sync::Arc<dyn MediaUploader>,
        chunk_bytes: usize,
        size_limit: u64,
    ) -> Self {
        self.media_uploader = Some(uploader);
        self.media_chunk_bytes = chunk_bytes.max(1);
        self.media_size_limit = size_limit;
        self
    }

    pub fn from_config(cfg: WeChatConfig) -> Self {
        Self::from_config_with_store(cfg, None)
    }

    pub fn from_config_with_store(
        cfg: WeChatConfig,
        sync_store: Option<std::sync::Arc<dyn SyncBufStore>>,
    ) -> Self {
        if let Some(ilink_cfg) = ILinkSendConfig::from_wechat_config(&cfg) {
            let sync_buf = sync_store
                .as_ref()
                .and_then(|store| store.load("wechat", &cfg.account_id).ok())
                .map(|b| String::from_utf8_lossy(&b).to_string())
                .unwrap_or_default();

            return Self {
                channel_id: ChannelId::new("wechat"),
                mode: WeChatMode::ILink {
                    config: ilink_cfg,
                    client: reqwest::Client::new(),
                    session: std::sync::Arc::new(tokio::sync::Mutex::new(SessionState {
                        context_tokens: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
                        sync_buf,
                    })),
                    sync_store,
                    account_id: cfg.account_id,
                },
                media_uploader: None,
                media_chunk_bytes: DEFAULT_CHUNK_BYTES,
                media_size_limit: 0,
            };
        }

        Self::new()
    }

    /// Resolve a media reference (uploading if necessary) and send it.
    async fn send_media(
        &self,
        to: &str,
        url: &str,
        media_id: Option<&str>,
    ) -> Result<SendReceipt, String> {
        let Some(uploader) = &self.media_uploader else {
            return Err("wechat media upload not enabled (no uploader configured)".into());
        };

        let media_ref = if let Some(id) = media_id.filter(|s| !s.trim().is_empty()) {
            // Already uploaded on the platform; reference directly.
            MediaRef { media_id: id.to_string(), url: Some(url.to_string()) }
        } else {
            match MediaLocation::parse(url).map_err(|e| format!("media[{}]: {e}", e.stage()))? {
                MediaLocation::LocalFile(path) => {
                    let source = FileMediaSource::new(
                        path,
                        self.media_chunk_bytes,
                        self.media_size_limit,
                    )
                    .await
                    .map_err(|e| format!("media[{}]: {e}", e.stage()))?;
                    uploader
                        .upload(&source)
                        .await
                        .map_err(|e| format!("media[{}]: {e}", e.stage()))?
                }
                MediaLocation::RemoteUrl(_) => {
                    return Err(
                        "media[source]: remote URL upload requires a pre-uploaded media_id".into(),
                    );
                }
            }
        };

        match &self.mode {
            WeChatMode::Stub => {
                tracing::info!(to = %to, media_id = %media_ref.media_id, "WeChat send_media (skeleton)");
                Ok(SendReceipt {
                    message_id: uuid::Uuid::new_v4().to_string(),
                    platform_msg_id: Some(format!("wechat_media_{}", media_ref.media_id)),
                    timestamp_ms: chrono::Utc::now().timestamp_millis(),
                })
            }
            WeChatMode::ILink { .. } => {
                // Real ilink media-reference send is pending the media contract
                // (plan T9, experimental).
                Err("media[send]: ilink media send not yet implemented (pending media contract)".into())
            }
        }
    }
}

#[async_trait]
impl Channel for WeChatChannel {
    fn id(&self) -> ChannelId {
        self.channel_id.clone()
    }

    async fn start(&self, inbound_tx: mpsc::Sender<Message>) -> Result<(), String> {
        let channel_id = self.channel_id.clone();
        match &self.mode {
            WeChatMode::Stub => tracing::info!("WeChat channel started (skeleton)"),
            WeChatMode::ILink {
                config,
                client,
                session,
                sync_store,
                account_id,
            } => {
                tracing::info!(account_id = %account_id, "WeChat channel started (ilink enabled)");

                let poll_cfg = config.clone();
                let poll_client = client.clone();
                let poll_session = std::sync::Arc::clone(session);
                let poll_inbound_tx = inbound_tx.clone();
                let poll_account_id = account_id.clone();
                let poll_sync_store = sync_store.clone();
                let poll_channel_id = channel_id.clone();

                tokio::spawn(async move {
                    let mut sync_buf = {
                        let state = poll_session.lock().await;
                        state.sync_buf.clone()
                    };

                    loop {
                        let updates = match get_updates_via_ilink(&poll_client, &poll_cfg, &sync_buf).await {
                            Ok(updates) => updates,
                            Err(super::ilink::ILinkGetUpdatesError::SessionExpired { errcode, errmsg }) => {
                                // Session expired (-14): clear all state and wait for re-login
                                tracing::error!(
                                    account_id = %poll_account_id,
                                    errcode = errcode,
                                    errmsg = %errmsg,
                                    "wechat session expired (-14): clearing all context tokens"
                                );
                                let state = poll_session.lock().await;
                                state.clear_context_tokens().await;
                                drop(state);
                                // Wait before retrying (session may recover or user needs to re-login)
                                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                                continue;
                            }
                            Err(e) => {
                                tracing::warn!(account_id = %poll_account_id, error = %e, "wechat inbound poll failed");
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                                continue;
                            }
                        };

                        if let Some(buf) = updates.get_updates_buf.as_deref() {
                            sync_buf = buf.to_string();
                            {
                                let mut state = poll_session.lock().await;
                                state.sync_buf = sync_buf.clone();
                            }
                            if let Some(store) = &poll_sync_store {
                                let _ = store.save("wechat", &poll_account_id, sync_buf.as_bytes());
                            }
                        }

                        for msg in updates.msgs {
                            if msg.message_type != Some(1) {
                                continue;
                            }

                            let Some(from_user_id) = msg.from_user_id.clone() else {
                                continue;
                            };

                            let conversation_id = msg
                                .group_id
                                .clone()
                                .unwrap_or_else(|| from_user_id.clone());
                            let conversation_type = if msg.group_id.is_some() {
                                crate::domain::value_objects::route_key::ConversationType::Group
                            } else {
                                crate::domain::value_objects::route_key::ConversationType::Direct
                            };

                            let text = msg
                                .item_list
                                .iter()
                                .find_map(|item| {
                                    item.text_item
                                        .as_ref()
                                        .and_then(|text_item| text_item.text.clone())
                                        .filter(|text| !text.trim().is_empty())
                                })
                                .unwrap_or_default();

                            let content = if text.is_empty() {
                                MessageContent::Unknown
                            } else {
                                MessageContent::Text(text)
                            };

                            let route_key = crate::domain::value_objects::route_key::RouteKey::new(
                                poll_channel_id.clone(),
                                conversation_id,
                                from_user_id.clone(),
                                conversation_type,
                            );

                            let message = Message::new_inbound(
                                msg.context_token.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                                route_key,
                                None,
                                chrono::Utc::now().timestamp_millis(),
                                content,
                            );

                            // Extract context_token and cache it per-user (following official SDK pattern)
                            // For user messages, use from_user_id; for bot messages, use to_user_id
                            let token_user_id = if msg.message_type == Some(1) {
                                msg.from_user_id.clone()
                            } else {
                                msg.to_user_id.clone()
                            };
                            
                            if let Some(user_id) = token_user_id {
                                if let Some(ctx) = extract_latest_context_token(std::slice::from_ref(&msg), Some(&user_id)) {
                                    let state = poll_session.lock().await;
                                    state.set_context_token(user_id, ctx).await;
                                }
                            }

                            if poll_inbound_tx.send(message).await.is_err() {
                                tracing::warn!(account_id = %poll_account_id, "wechat inbound receiver closed");
                                return;
                            }
                        }
                    }
                });
            }
        }
        Ok(())
    }

    async fn send_message(&self, to: &str, content: &MessageContent) -> Result<SendReceipt, String> {
        let body = match content {
            MessageContent::Text(t) => t.clone(),
            MessageContent::Image { url, media_id } => {
                return self.send_media(to, url, media_id.as_deref()).await;
            }
            MessageContent::File { url, .. } => {
                return self.send_media(to, url, None).await;
            }
            MessageContent::Unknown => {
                return Err("wechat cannot send Unknown content".into());
            }
        };

        match &self.mode {
            WeChatMode::Stub => {
                tracing::info!(to = %to, body = %body, "WeChat send_message (skeleton)");
                Ok(SendReceipt {
                    message_id: uuid::Uuid::new_v4().to_string(),
                    platform_msg_id: Some(format!("wechat_stub_{}", chrono::Utc::now().timestamp_millis())),
                    timestamp_ms: chrono::Utc::now().timestamp_millis(),
                })
            }
            WeChatMode::ILink {
                config,
                client,
                session,
                sync_store: _,
                account_id: _,
            } => {
                // context_token is granted by the ilink server only when the user sends a
                // message. Calling getupdates before/after send does NOT refresh the token
                // (it is a 35s long-poll, not a keepalive ping). The correct pattern is:
                //   background poller updates context_token → stored context_token used here.
                let state = session.lock().await;
                let context_token = state.get_context_token(to).await
                    .ok_or_else(|| format!("wechat: no context_token cached for user {}", to))?;
                let mut send_cfg = config.clone();
                send_cfg.context_token = context_token;
                drop(state);

                let resp = send_text_via_ilink(client, &send_cfg, to, &body).await?;

                let platform_msg_id = resp
                    .get("msg")
                    .and_then(|v| v.get("server_id"))
                    .and_then(|v| v.as_str())
                    .map(|v| v.to_string())
                    .or_else(|| {
                        resp.get("server_id")
                            .and_then(|v| v.as_str())
                            .map(|v| v.to_string())
                    });

                Ok(SendReceipt {
                    message_id: uuid::Uuid::new_v4().to_string(),
                    platform_msg_id,
                    timestamp_ms: chrono::Utc::now().timestamp_millis(),
                })
            }
        }
    }

    async fn stop(&self) -> Result<(), String> {
        tracing::info!("WeChat channel stopped");
        Ok(())
    }

    async fn health(&self) -> Result<HealthStatus, String> {
        Ok(HealthStatus {
            channel: "wechat".into(),
            healthy: true,
            detail: match self.mode {
                WeChatMode::Stub => "skeleton — not connected to platform".into(),
                WeChatMode::ILink { .. } => "ilink configured (context refresh enabled)".into(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entities::message::MessageContent;

    #[tokio::test]
    async fn wechat_health_returns_skeleton_status() {
        let ch = WeChatChannel::new();
        let h = ch.health().await.unwrap();
        assert!(h.healthy);
        assert!(h.detail.contains("skeleton"));
    }

    #[tokio::test]
    async fn wechat_send_returns_stub_receipt() {
        let ch = WeChatChannel::new();
        let receipt = ch.send_message("user1", &MessageContent::Text("hi".into())).await.unwrap();
        assert!(receipt.platform_msg_id.unwrap().starts_with("wechat_stub_"));
    }

    #[tokio::test]
    async fn wechat_rejects_media_send_without_uploader() {
        let ch = WeChatChannel::new();
        let err = ch
            .send_message(
                "user1",
                &MessageContent::Image {
                    url: "https://example.invalid/image.png".into(),
                    media_id: None,
                },
            )
            .await
            .unwrap_err();
        assert!(err.contains("not enabled"));
    }

    #[tokio::test]
    async fn wechat_from_config_reports_ilink_mode() {
        let ch = WeChatChannel::from_config(WeChatConfig {
            enabled: true,
            base_url: "https://ilinkai.weixin.qq.com".into(),
            token: "token1".into(),
            context_token: "ctx1".into(),
            ..WeChatConfig::default()
        });

        let health = ch.health().await.unwrap();
        assert!(health.detail.contains("ilink"));
    }
}