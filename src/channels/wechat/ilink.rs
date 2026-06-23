use base64::Engine;
use aes::cipher::{generic_array::GenericArray, BlockDecrypt, BlockEncrypt, KeyInit};
use rand::Rng;
use serde::Deserialize;
use serde::Serialize;

use crate::infrastructure::config::WeChatConfig;

/// ilink protocol response envelope.
/// `ret` field semantics:
/// - 0: success
/// - -1: system error
/// - -2: parameter error
/// - -3: rate limited
#[derive(Debug, Deserialize)]
pub struct ILinkResponse {
    pub ret: i32,
    #[serde(default)]
    pub errcode: Option<i32>,
    #[serde(default)]
    pub errmsg: Option<String>,
}

/// Error code matrix for the `errcode` field.
#[derive(Debug, PartialEq, Eq)]
pub enum ILinkErrorCode {
    Success,
    BadParam,
    AuthFailed,
    RateLimited,
    SessionExpired,
    Unknown(i32),
}

#[derive(Clone, Debug)]
pub struct ILinkSendConfig {
    pub base_url: String,
    pub token: String,
    pub from_user_id: String,
    pub context_token: String,
    pub channel_version: String,
    pub timeout_ms: u64,
    pub keepalive_timeout_ms: u64,
}

impl ILinkSendConfig {
    pub fn from_wechat_config(cfg: &WeChatConfig) -> Option<Self> {
        if !cfg.enabled {
            return None;
        }
        if cfg.base_url.trim().is_empty() || cfg.token.trim().is_empty() {
            return None;
        }

        Some(Self {
            base_url: cfg.base_url.trim().to_string(),
            token: cfg.token.trim().to_string(),
            from_user_id: cfg.account_id.trim().to_string(),
            context_token: cfg.context_token.trim().to_string(),
            channel_version: cfg.channel_version.trim().to_string(),
            timeout_ms: cfg.timeout_ms,
            keepalive_timeout_ms: cfg.keepalive_timeout_ms,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct ILinkMessage {
    #[serde(default)]
    pub message_type: Option<i32>,
    #[serde(default)]
    pub from_user_id: Option<String>,
    #[serde(default)]
    pub to_user_id: Option<String>,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub context_token: Option<String>,
    #[serde(default)]
    pub item_list: Vec<ILinkItem>,
}

#[derive(Debug, Deserialize)]
pub struct ILinkItem {
    #[serde(default, rename = "type")]
    pub item_type: Option<i32>,
    #[serde(default)]
    pub text_item: Option<ILinkTextItem>,
}

#[derive(Debug, Deserialize)]
pub struct ILinkTextItem {
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ILinkQrcodeResponse {
    #[serde(default)]
    pub ret: i32,
    #[serde(default)]
    pub errcode: Option<i32>,
    #[serde(default)]
    pub errmsg: Option<String>,
    pub qrcode: String,
    #[serde(default)]
    pub qrcode_img_content: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ILinkQrcodeStatusResponse {
    #[serde(default)]
    pub ret: i32,
    #[serde(default)]
    pub errcode: Option<i32>,
    #[serde(default)]
    pub errmsg: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub bot_token: Option<String>,
    #[serde(default)]
    pub baseurl: Option<String>,
    #[serde(default)]
    pub ilink_bot_id: Option<String>,
    #[serde(default)]
    pub ilink_user_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ILinkGetUpdatesResponse {
    #[serde(default)]
    pub ret: i32,
    #[serde(default)]
    pub errcode: Option<i32>,
    #[serde(default)]
    pub errmsg: Option<String>,
    #[serde(default)]
    pub msgs: Vec<ILinkMessage>,
    #[serde(default)]
    pub get_updates_buf: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ILinkGetUpdatesError {
    Transport(String),
    SessionExpired {
        errcode: i32,
        errmsg: String,
    },
    Business {
        ret: i32,
        errcode: i32,
        errmsg: String,
    },
}

impl std::fmt::Display for ILinkGetUpdatesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(msg) => write!(f, "{}", msg),
            Self::SessionExpired { errcode, errmsg } => {
                write!(
                    f,
                    "ilink session expired: errcode={}, errmsg={}",
                    errcode, errmsg
                )
            }
            Self::Business {
                ret,
                errcode,
                errmsg,
            } => write!(
                f,
                "ilink getupdates business error: ret={}, errcode={}, errmsg={}",
                ret, errcode, errmsg
            ),
        }
    }
}

#[derive(Debug, Serialize)]
struct BaseInfo {
    #[serde(skip_serializing_if = "String::is_empty")]
    channel_version: String,
}

#[derive(Debug, Serialize)]
struct TextItemInner {
    text: String,
}

#[derive(Debug, Serialize)]
struct MessageItem {
    #[serde(rename = "type")]
    item_type: i32,
    text_item: TextItemInner,
}

#[derive(Debug, Serialize)]
struct SendMsg {
    from_user_id: String,
    to_user_id: String,
    client_id: String,
    message_type: i32,
    message_state: i32,
    item_list: Vec<MessageItem>,
    context_token: String,
}

#[derive(Debug, Serialize)]
struct SendMessageRequest {
    msg: SendMsg,
    base_info: BaseInfo,
}

#[derive(Debug, Serialize)]
struct GetUpdatesRequest {
    get_updates_buf: String,
    base_info: BaseInfo,
}

pub fn random_wechat_uin() -> String {
    let value: u32 = rand::thread_rng().gen();
    base64::engine::general_purpose::STANDARD.encode(value.to_string().as_bytes())
}

pub fn build_headers(token: &str) -> reqwest::header::HeaderMap {
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        HeaderName::from_static("authorizationtype"),
        HeaderValue::from_static("ilink_bot_token"),
    );

    if let Ok(uin) = HeaderValue::from_str(&random_wechat_uin()) {
        headers.insert(HeaderName::from_static("x-wechat-uin"), uin);
    }
    if let Ok(auth) = HeaderValue::from_str(&format!("Bearer {}", token.trim())) {
        headers.insert(AUTHORIZATION, auth);
    }

    headers
}

fn build_send_request_payload(cfg: &ILinkSendConfig, to: &str, text: &str) -> SendMessageRequest {
    SendMessageRequest {
        msg: SendMsg {
            from_user_id: cfg.from_user_id.clone(),
            to_user_id: to.to_string(),
            client_id: format!(
                "wechat-channel:{}-{}",
                chrono::Utc::now().timestamp_millis(),
                uuid::Uuid::new_v4().simple()
            ),
            message_type: 2,
            message_state: 2,
            item_list: vec![MessageItem {
                item_type: 1,
                text_item: TextItemInner {
                    text: text.to_string(),
                },
            }],
            context_token: cfg.context_token.clone(),
        },
        base_info: BaseInfo {
            channel_version: cfg.channel_version.clone(),
        },
    }
}

pub async fn send_text_via_ilink(
    client: &reqwest::Client,
    cfg: &ILinkSendConfig,
    to: &str,
    text: &str,
) -> Result<serde_json::Value, String> {
    let base = if cfg.base_url.ends_with('/') {
        cfg.base_url.clone()
    } else {
        format!("{}/", cfg.base_url)
    };
    let url = format!("{}ilink/bot/sendmessage", base);
    let payload = build_send_request_payload(cfg, to, text);

    let response = client
        .post(url)
        .headers(build_headers(&cfg.token))
        .timeout(std::time::Duration::from_millis(cfg.timeout_ms))
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("ilink request failed: {}", e))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("failed to read ilink response: {}", e))?;

    if !status.is_success() {
        return Err(format!("ilink HTTP {}: {}", status, body));
    }

    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err("ilink empty response body (session may be expired)".into());
    }

    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| format!("invalid ilink JSON response: {}", e))?;

    // Check for session expired (-14) FIRST before generic errcode check
    let errcode = value.get("errcode").and_then(|v| v.as_i64()).unwrap_or_default() as i32;
    if errcode == -14 {
        let errmsg = value
            .get("errmsg")
            .and_then(|v| v.as_str())
            .unwrap_or("session expired")
            .to_string();
        return Err(format!("wechat session expired (errcode -14): {}", errmsg));
    }

    // Aligned with the upstream reference bot (corespeed-io/wechatbot): the
    // sendmessage path only treats a non-zero `errcode` as a real failure
    // (notably errcode=-14 "session expired"). `ret=-2` is observed on
    // proactive sends but is not a reliable delivery-failure signal here.
    let ret = value.get("ret").and_then(|v| v.as_i64()).unwrap_or_default();
    if errcode != 0 {
        let errmsg = value
            .get("errmsg")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(format!("ilink business error: ret={}, errcode={}, errmsg={}", ret, errcode, errmsg));
    }

    Ok(value)
}

pub async fn get_updates_via_ilink(
    client: &reqwest::Client,
    cfg: &ILinkSendConfig,
    sync_buf: &str,
) -> Result<ILinkGetUpdatesResponse, ILinkGetUpdatesError> {
    let base = if cfg.base_url.ends_with('/') {
        cfg.base_url.clone()
    } else {
        format!("{}/", cfg.base_url)
    };
    let url = format!("{}ilink/bot/getupdates", base);
    let payload = GetUpdatesRequest {
        get_updates_buf: sync_buf.to_string(),
        base_info: BaseInfo {
            channel_version: cfg.channel_version.clone(),
        },
    };

    // getupdates is a long-poll (server holds connection up to 35s waiting for messages).
    // A timeout is NOT an error — it just means no messages arrived. Return empty response
    // and preserve the existing sync_buf so the caller can continue.
    let send_result = client
        .post(url)
        .headers(build_headers(&cfg.token))
        .timeout(std::time::Duration::from_millis(cfg.keepalive_timeout_ms))
        .json(&payload)
        .send()
        .await;

    let response = match send_result {
        Ok(r) => r,
        Err(e) if e.is_timeout() || e.is_connect() => {
            // Long-poll timed out or connection was reset — treat as "no new messages".
            return Ok(ILinkGetUpdatesResponse {
                ret: 0,
                errcode: None,
                errmsg: None,
                msgs: Vec::new(),
                get_updates_buf: Some(sync_buf.to_string()),
            });
        }
        Err(e) => return Err(ILinkGetUpdatesError::Transport(format!("ilink getupdates failed: {}", e))),
    };

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| ILinkGetUpdatesError::Transport(format!("failed to read ilink getupdates response: {}", e)))?;

    if !status.is_success() {
        return Err(ILinkGetUpdatesError::Transport(format!("ilink getupdates HTTP {}: {}", status, body)));
    }

    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Ok(ILinkGetUpdatesResponse {
            ret: 0,
            errcode: None,
            errmsg: None,
            msgs: Vec::new(),
            get_updates_buf: Some(sync_buf.to_string()),
        });
    }

    let value: ILinkGetUpdatesResponse = serde_json::from_str(trimmed)
        .map_err(|e| ILinkGetUpdatesError::Transport(format!("invalid ilink getupdates JSON response: {}", e)))?;

    // Check for session expired (-14) FIRST
    if let Some(errcode) = value.errcode {
        if errcode == -14 {
            let errmsg = value.errmsg.clone().unwrap_or_else(|| "session expired".into());
            return Err(ILinkGetUpdatesError::SessionExpired { errcode, errmsg });
        }
    }

    if value.ret != 0 {
        let errcode = value.errcode.unwrap_or_default();
        let errmsg = value.errmsg.clone().unwrap_or_else(|| "unknown error".into());
        return Err(ILinkGetUpdatesError::Business {
            ret: value.ret,
            errcode,
            errmsg,
        });
    }

    Ok(value)
}

pub async fn get_bot_qrcode(
    client: &reqwest::Client,
    base_url: &str,
) -> Result<ILinkQrcodeResponse, String> {
    let base = if base_url.ends_with('/') {
        base_url.to_string()
    } else {
        format!("{}/", base_url)
    };
    let url = format!("{}ilink/bot/get_bot_qrcode?bot_type=3", base);

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("ilink get_bot_qrcode request failed: {}", e))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("failed to read ilink get_bot_qrcode response: {}", e))?;

    if !status.is_success() {
        return Err(format!("ilink get_bot_qrcode HTTP {}: {}", status, body));
    }

    let parsed: ILinkQrcodeResponse = serde_json::from_str(body.trim())
        .map_err(|e| format!("invalid ilink get_bot_qrcode JSON response: {}", e))?;
    if parsed.ret != 0 {
        return Err(format!(
            "ilink get_bot_qrcode business error: ret={}, errcode={}, errmsg={}",
            parsed.ret,
            parsed.errcode.unwrap_or_default(),
            parsed.errmsg.clone().unwrap_or_else(|| "unknown error".into())
        ));
    }

    Ok(parsed)
}

pub async fn get_qrcode_status(
    client: &reqwest::Client,
    base_url: &str,
    qrcode: &str,
) -> Result<ILinkQrcodeStatusResponse, String> {
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

    let base = if base_url.ends_with('/') {
        base_url.to_string()
    } else {
        format!("{}/", base_url)
    };
    let url = format!("{}ilink/bot/get_qrcode_status?qrcode={}", base, qrcode);

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("ilink-app-clientversion"),
        HeaderValue::from_static("1"),
    );

    let response = client
        .get(url)
        .headers(headers)
        .send()
        .await
        .map_err(|e| format!("ilink get_qrcode_status request failed: {}", e))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("failed to read ilink get_qrcode_status response: {}", e))?;

    if !status.is_success() {
        return Err(format!("ilink get_qrcode_status HTTP {}: {}", status, body));
    }

    let parsed: ILinkQrcodeStatusResponse = serde_json::from_str(body.trim())
        .map_err(|e| format!("invalid ilink get_qrcode_status JSON response: {}", e))?;
    if parsed.ret != 0 {
        return Err(format!(
            "ilink get_qrcode_status business error: ret={}, errcode={}, errmsg={}",
            parsed.ret,
            parsed.errcode.unwrap_or_default(),
            parsed.errmsg.clone().unwrap_or_else(|| "unknown error".into())
        ));
    }

    Ok(parsed)
}

pub fn extract_latest_context_token(
    msgs: &[ILinkMessage],
    peer_id: Option<&str>,
) -> Option<String> {
    for msg in msgs.iter().rev() {
        let Some(token) = &msg.context_token else {
            continue;
        };
        if let Some(peer) = peer_id {
            let same_peer = msg.from_user_id.as_deref() == Some(peer)
                || msg.to_user_id.as_deref() == Some(peer);
            if !same_peer {
                continue;
            }
        }
        if !token.trim().is_empty() {
            return Some(token.clone());
        }
    }
    None
}

impl From<i32> for ILinkErrorCode {
    fn from(code: i32) -> Self {
        match code {
            0 => Self::Success,
            1 => Self::BadParam,
            2 => Self::AuthFailed,
            3 => Self::RateLimited,
            4 => Self::SessionExpired,
            n => Self::Unknown(n),
        }
    }
}

impl ILinkResponse {
    /// Interpret the `ret` field.
    pub fn is_success(&self) -> bool {
        self.ret == 0
    }

    /// Classify the `errcode` if present.
    pub fn error_code(&self) -> Option<ILinkErrorCode> {
        self.errcode.map(ILinkErrorCode::from)
    }

    /// Message suitable for logging (does not expose secrets).
    pub fn summary(&self) -> String {
        format!(
            "ret={} errcode={:?} errmsg={:?}",
            self.ret, self.errcode, self.errmsg
        )
    }
}

/// AES-128-ECB/PKCS7 decryption.
pub fn decrypt_aes_128_ecb_pkcs7(encrypted: &[u8], key: &[u8; 16]) -> Result<Vec<u8>, DecryptError> {
    if encrypted.is_empty() {
        return Err(DecryptError::EmptyInput);
    }
    if encrypted.len() % 16 != 0 {
        return Err(DecryptError::InvalidPadding);
    }

    let cipher = aes::Aes128::new(GenericArray::from_slice(key));
    let mut decrypted = encrypted.to_vec();
    for chunk in decrypted.chunks_exact_mut(16) {
        cipher.decrypt_block(GenericArray::from_mut_slice(chunk));
    }

    let Some(&pad_len) = decrypted.last() else {
        return Err(DecryptError::EmptyInput);
    };
    let pad_len = pad_len as usize;
    if pad_len == 0 || pad_len > 16 || pad_len > decrypted.len() {
        return Err(DecryptError::InvalidPadding);
    }

    let start = decrypted.len() - pad_len;
    if !decrypted[start..].iter().all(|byte| *byte as usize == pad_len) {
        return Err(DecryptError::InvalidPadding);
    }

    decrypted.truncate(start);
    Ok(decrypted)
}

/// Encrypt with AES-128-ECB/PKCS7.
pub fn encrypt_aes_128_ecb_pkcs7(plaintext: &[u8], key: &[u8; 16]) -> Vec<u8> {
    let pad_len = 16 - (plaintext.len() % 16);
    let pad_len = if pad_len == 0 { 16 } else { pad_len };

    let mut padded = Vec::with_capacity(plaintext.len() + pad_len);
    padded.extend_from_slice(plaintext);
    padded.extend(std::iter::repeat(pad_len as u8).take(pad_len));

    let cipher = aes::Aes128::new(GenericArray::from_slice(key));
    for chunk in padded.chunks_exact_mut(16) {
        cipher.encrypt_block(GenericArray::from_mut_slice(chunk));
    }

    padded
}

#[derive(Debug, thiserror::Error)]
pub enum DecryptError {
    #[error("empty input")]
    EmptyInput,
    #[error("invalid padding")]
    InvalidPadding,
    #[error("key error: {0}")]
    KeyError(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use axum::{Json, Router};
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    #[derive(Clone, Default)]
    struct ContractState {
        send_headers: Arc<Mutex<Option<HeaderMap>>>,
        send_body: Arc<Mutex<Option<serde_json::Value>>>,
        updates_headers: Arc<Mutex<Option<HeaderMap>>>,
        updates_body: Arc<Mutex<Option<serde_json::Value>>>,
    }

    async fn spawn_contract_server() -> (String, ContractState, tokio::task::JoinHandle<()>) {
        async fn send_handler(
            State(state): State<ContractState>,
            headers: HeaderMap,
            Json(body): Json<serde_json::Value>,
        ) -> (StatusCode, Json<serde_json::Value>) {
            *state.send_headers.lock().await = Some(headers);
            *state.send_body.lock().await = Some(body);
            (
                StatusCode::OK,
                Json(serde_json::json!({"ret": 0, "errcode": 0, "errmsg": "ok"})),
            )
        }

        async fn updates_handler(
            State(state): State<ContractState>,
            headers: HeaderMap,
            Json(body): Json<serde_json::Value>,
        ) -> (StatusCode, Json<serde_json::Value>) {
            *state.updates_headers.lock().await = Some(headers);
            *state.updates_body.lock().await = Some(body);
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "ret": 0,
                    "errcode": 0,
                    "errmsg": "ok",
                    "msgs": [{
                        "message_type": 1,
                        "from_user_id": "peer_a",
                        "to_user_id": "bot",
                        "group_id": null,
                        "context_token": "ctx-updates",
                        "item_list": [{"type": 1, "text_item": {"text": "hello"}}]
                    }],
                    "get_updates_buf": "buf-next"
                })),
            )
        }

        let state = ContractState::default();
        let app = Router::new()
            .route("/ilink/bot/sendmessage", post(send_handler))
            .route("/ilink/bot/getupdates", post(updates_handler))
            .with_state(state.clone());

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        (format!("http://{}", addr), state, handle)
    }

    #[test]
    fn ilink_response_success() {
        let resp: ILinkResponse = serde_json::from_str(r#"{"ret":0}"#).unwrap();
        assert!(resp.is_success());
        assert_eq!(resp.error_code(), None);
    }

    #[test]
    fn ilink_response_error_with_errcode() {
        let resp: ILinkResponse =
            serde_json::from_str(r#"{"ret":-1,"errcode":2,"errmsg":"auth failed"}"#).unwrap();
        assert!(!resp.is_success());
        assert_eq!(resp.error_code(), Some(ILinkErrorCode::AuthFailed));
    }

    #[test]
    fn ilink_error_code_unknown() {
        let resp: ILinkResponse =
            serde_json::from_str(r#"{"ret":-1,"errcode":99}"#).unwrap();
        assert_eq!(resp.error_code(), Some(ILinkErrorCode::Unknown(99)));
    }

    #[test]
    fn build_headers_includes_required_contract_fields() {
        let headers = build_headers(" token-1 ");
        assert_eq!(headers.get(reqwest::header::CONTENT_TYPE).unwrap(), "application/json");
        assert_eq!(headers.get("authorizationtype").unwrap(), "ilink_bot_token");
        assert_eq!(headers.get(reqwest::header::AUTHORIZATION).unwrap(), "Bearer token-1");
        assert!(headers.get("x-wechat-uin").is_some());
    }

    #[tokio::test]
    async fn send_text_sets_headers_and_serializes_contract_payload() {
        let (base_url, state, handle) = spawn_contract_server().await;
        let client = reqwest::Client::new();
        let cfg = ILinkSendConfig {
            base_url,
            token: "token-1".into(),
            from_user_id: "bot-user".into(),
            context_token: "ctx-1".into(),
            channel_version: "0.1.0".into(),
            timeout_ms: 2_000,
            keepalive_timeout_ms: 2_000,
        };

        let response = send_text_via_ilink(&client, &cfg, "peer_a", "hello world")
            .await
            .unwrap();
        assert_eq!(response["ret"], 0);

        let headers = state.send_headers.lock().await.clone().unwrap();
        assert_eq!(headers.get(reqwest::header::AUTHORIZATION).unwrap(), "Bearer token-1");
        assert!(headers.get("x-wechat-uin").is_some());

        let body = state.send_body.lock().await.clone().unwrap();
        assert_eq!(body["msg"]["from_user_id"], "bot-user");
        assert_eq!(body["msg"]["to_user_id"], "peer_a");
        assert_eq!(body["msg"]["context_token"], "ctx-1");
        assert_eq!(body["msg"]["item_list"][0]["text_item"]["text"], "hello world");
        assert_eq!(body["base_info"]["channel_version"], "0.1.0");

        handle.abort();
    }

    #[tokio::test]
    async fn send_text_accepts_ret_minus_2_as_success() {
        // Aligned with upstream corespeed-io/wechatbot: `ret=-2` without a
        // non-zero `errcode` must not be treated as a hard send failure.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let app = Router::new().route(
                "/ilink/bot/sendmessage",
                post(|| async {
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "ret": -2,
                            "errmsg": "parameter error"
                        })),
                    )
                }),
            );
            let _ = axum::serve(listener, app).await;
        });

        let client = reqwest::Client::new();
        let cfg = ILinkSendConfig {
            base_url: format!("http://{}", addr),
            token: "token-1".into(),
            from_user_id: "bot-user".into(),
            context_token: "ctx-1".into(),
            channel_version: "0.1.0".into(),
            timeout_ms: 2_000,
            keepalive_timeout_ms: 2_000,
        };

        let response = send_text_via_ilink(&client, &cfg, "peer_a", "hello")
            .await
            .expect("ret=-2 must be treated as a successful send");
        assert_eq!(response["ret"], -2);

        handle.abort();
    }

    #[tokio::test]
    async fn send_text_fails_on_session_expired_errcode() {
        // errcode=-14 ("session expired") is the only real failure signal on the send
        // path; it must surface as an error so the caller's outbox/DLQ can retry/relogin.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let app = Router::new().route(
                "/ilink/bot/sendmessage",
                post(|| async {
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "ret": 0,
                            "errcode": -14,
                            "errmsg": "session expired"
                        })),
                    )
                }),
            );
            let _ = axum::serve(listener, app).await;
        });

        let client = reqwest::Client::new();
        let cfg = ILinkSendConfig {
            base_url: format!("http://{}", addr),
            token: "token-1".into(),
            from_user_id: "bot-user".into(),
            context_token: "ctx-1".into(),
            channel_version: "0.1.0".into(),
            timeout_ms: 2_000,
            keepalive_timeout_ms: 2_000,
        };

        let err = send_text_via_ilink(&client, &cfg, "peer_a", "hello")
            .await
            .unwrap_err();
        assert!(err.contains("errcode=-14"), "unexpected error: {err}");

        handle.abort();
    }

    #[tokio::test]
    async fn get_updates_preserves_sync_buf_and_parses_messages() {
        let (base_url, state, handle) = spawn_contract_server().await;
        let client = reqwest::Client::new();
        let cfg = ILinkSendConfig {
            base_url,
            token: "token-1".into(),
            from_user_id: "bot-user".into(),
            context_token: "ctx-1".into(),
            channel_version: "0.1.0".into(),
            timeout_ms: 2_000,
            keepalive_timeout_ms: 2_000,
        };

        let response = get_updates_via_ilink(&client, &cfg, "buf-123").await.unwrap();
        assert_eq!(response.ret, 0);
        assert_eq!(response.get_updates_buf.as_deref(), Some("buf-next"));
        assert_eq!(response.msgs.len(), 1);
        assert_eq!(response.msgs[0].from_user_id.as_deref(), Some("peer_a"));
        assert_eq!(response.msgs[0].context_token.as_deref(), Some("ctx-updates"));
        assert_eq!(response.msgs[0].item_list[0].text_item.as_ref().and_then(|item| item.text.as_deref()), Some("hello"));

        let headers = state.updates_headers.lock().await.clone().unwrap();
        assert_eq!(headers.get(reqwest::header::AUTHORIZATION).unwrap(), "Bearer token-1");

        let body = state.updates_body.lock().await.clone().unwrap();
        assert_eq!(body["get_updates_buf"], "buf-123");
        assert_eq!(body["base_info"]["channel_version"], "0.1.0");

        handle.abort();
    }

    #[tokio::test]
    async fn get_updates_business_error_reports_ret_and_errcode() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let app = Router::new().route(
                "/ilink/bot/getupdates",
                post(|| async {
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "ret": -2,
                            "errcode": 4,
                            "errmsg": "session expired"
                        })),
                    )
                }),
            );
            let _ = axum::serve(listener, app).await;
        });

        let client = reqwest::Client::new();
        let cfg = ILinkSendConfig {
            base_url: format!("http://{}", addr),
            token: "token-1".into(),
            from_user_id: "bot-user".into(),
            context_token: "ctx-1".into(),
            channel_version: "0.1.0".into(),
            timeout_ms: 2_000,
            keepalive_timeout_ms: 2_000,
        };

        let err = get_updates_via_ilink(&client, &cfg, "buf-123").await.unwrap_err();
        let text = err.to_string();
        assert!(text.contains("ret=-2"));
        assert!(text.contains("errcode=4"));

        handle.abort();
    }

    #[test]
    fn decrypt_round_trip() {
        let key: [u8; 16] = [0; 16];
        let data = b"hello world";
        let encrypted = encrypt_aes_128_ecb_pkcs7(data, &key);
        let decrypted = decrypt_aes_128_ecb_pkcs7(&encrypted, &key).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn aes_128_ecb_pkcs7_matches_known_vector() {
        let key: [u8; 16] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
            0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        ];
        let plaintext = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77,
            0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
        ];
        let encrypted = encrypt_aes_128_ecb_pkcs7(&plaintext, &key);
        assert_eq!(encrypted.len(), 32);
        assert_eq!(
            &encrypted[..16],
            &[
                0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30,
                0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4, 0xc5, 0x5a,
            ]
        );
        let decrypted = decrypt_aes_128_ecb_pkcs7(&encrypted, &key).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn decrypt_empty_input_errors() {
        let key: [u8; 16] = [0; 16];
        let result = decrypt_aes_128_ecb_pkcs7(&[], &key);
        assert!(result.is_err());
    }

    #[test]
    fn random_wechat_uin_is_base64_text() {
        let uin = random_wechat_uin();
        assert!(!uin.is_empty());
        assert!(uin
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='));
    }

    #[test]
    fn from_wechat_config_requires_complete_fields() {
        let mut cfg = WeChatConfig::default();
        cfg.enabled = true;
        assert!(ILinkSendConfig::from_wechat_config(&cfg).is_none());

        cfg.base_url = "https://ilinkai.weixin.qq.com".into();
        cfg.token = "token-1".into();
        cfg.context_token = "ctx-1".into();
        assert!(ILinkSendConfig::from_wechat_config(&cfg).is_some());
    }
}
