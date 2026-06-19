use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Incoming (from client) ──

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    Request(JsonRpcRequest),
    Notification(JsonRpcNotification),
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

// ── Outgoing (to client) ──

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

// ── Standard JSON-RPC error codes ──

pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;

// ── MCP-specific method names ──

pub const METHOD_INITIALIZE: &str = "initialize";
pub const METHOD_NOTIFICATIONS_INITIALIZED: &str = "notifications/initialized";
pub const METHOD_TOOLS_LIST: &str = "tools/list";
pub const METHOD_TOOLS_CALL: &str = "tools/call";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_initialize_request() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#;
        let msg: JsonRpcMessage = serde_json::from_str(raw).unwrap();
        match msg {
            JsonRpcMessage::Request(req) => {
                assert_eq!(req.method, "initialize");
                assert_eq!(req.id, Value::Number(1.into()));
            }
            _ => panic!("expected request"),
        }
    }

    #[test]
    fn parse_notification() {
        let raw = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let msg: JsonRpcMessage = serde_json::from_str(raw).unwrap();
        match msg {
            JsonRpcMessage::Notification(n) => {
                assert_eq!(n.method, "notifications/initialized");
            }
            _ => panic!("expected notification"),
        }
    }

    #[test]
    fn response_success_serializes() {
        let resp = JsonRpcResponse::success(Value::Number(1.into()), serde_json::json!({"ok": true}));
        let raw = serde_json::to_string(&resp).unwrap();
        assert!(raw.contains(r#""result""#));
        assert!(!raw.contains(r#""error""#));
    }

    #[test]
    fn response_error_serializes() {
        let resp = JsonRpcResponse::error(Value::Number(1.into()), METHOD_NOT_FOUND, "unknown method");
        let raw = serde_json::to_string(&resp).unwrap();
        assert!(raw.contains(r#""error""#));
        assert!(raw.contains("-32601"));
    }
}
