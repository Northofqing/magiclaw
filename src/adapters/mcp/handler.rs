use serde_json::Value;

use crate::domain::ports::outbox_repo::OutboxRepo;

use super::protocol::{
    JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS,
    METHOD_INITIALIZE, METHOD_NOTIFICATIONS_INITIALIZED, METHOD_TOOLS_CALL,
    METHOD_TOOLS_LIST, METHOD_NOT_FOUND,
};
use super::tools::{self, ToolDispatcher};
use super::transport;

/// Handles MCP protocol messages: initialize, tools/list, tools/call.
pub struct ProtocolHandler {
    /// Server name reported in initialize response.
    server_name: String,
    /// Server version.
    server_version: String,
}

impl ProtocolHandler {
    pub fn new(server_name: impl Into<String>, server_version: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            server_version: server_version.into(),
        }
    }

    /// Process an incoming JSON-RPC request and write the response to stdout.
    pub fn handle_request(
        &self,
        request: JsonRpcRequest,
        outbox: &dyn OutboxRepo,
    ) {
        let response = match request.method.as_str() {
            METHOD_INITIALIZE => self.handle_initialize(request.id, &request.params),
            METHOD_TOOLS_LIST => self.handle_tools_list(request.id),
            METHOD_TOOLS_CALL => self.handle_tools_call(request.id, &request.params, outbox),
            _ => {
                tracing::warn!(method = %request.method, "unknown method");
                JsonRpcResponse::error(request.id, METHOD_NOT_FOUND, format!("unknown method: {}", request.method))
            }
        };

        transport::write_response(&response);
    }

    /// Handle an incoming notification (fire-and-forget, no response).
    pub fn handle_notification(&self, method: &str, _params: &Option<Value>) {
        match method {
            METHOD_NOTIFICATIONS_INITIALIZED => {
                tracing::info!("client initialized notification received");
            }
            _ => {
                tracing::debug!(method = %method, "unhandled notification");
            }
        }
    }

    // ── Method Handlers ──

    fn handle_initialize(&self, id: Value, _params: &Option<Value>) -> JsonRpcResponse {
        tracing::info!("handling initialize request");
        JsonRpcResponse::success(
            id,
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": self.server_name,
                    "version": self.server_version
                }
            }),
        )
    }

    fn handle_tools_list(&self, id: Value) -> JsonRpcResponse {
        let tools = tools::all_tools();
        JsonRpcResponse::success(
            id,
            serde_json::json!({
                "tools": tools
            }),
        )
    }

    fn handle_tools_call(
        &self,
        id: Value,
        params: &Option<Value>,
        outbox: &dyn OutboxRepo,
    ) -> JsonRpcResponse {
        let params = match params {
            Some(p) => p,
            None => {
                return JsonRpcResponse::error(id, INVALID_PARAMS, "missing params");
            }
        };

        let tool_name = match params.get("name").and_then(|n| n.as_str()) {
            Some(n) => n,
            None => {
                return JsonRpcResponse::error(id, INVALID_PARAMS, "missing tool name");
            }
        };

        let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);

        match ToolDispatcher::dispatch(tool_name, arguments, outbox) {
            Ok(result) => JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string(&result).unwrap_or_default()
                    }]
                }),
            ),
            Err(e) => {
                tracing::warn!(tool = %tool_name, error = %e, "tool call failed");
                JsonRpcResponse::success(
                    id,
                    serde_json::json!({
                        "content": [{
                            "type": "text",
                            "text": format!("error: {}", e)
                        }],
                        "isError": true
                    }),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::adapters::sqlite_outbox::SqliteOutboxRepo;
    use crate::infrastructure::db::{init_db, DbPool};

    fn make_outbox() -> Arc<SqliteOutboxRepo> {
        Arc::new(SqliteOutboxRepo::new(DbPool::new(init_db(":memory:").unwrap())))
    }

    #[test]
    fn initialize_returns_capabilities() {
        let handler = ProtocolHandler::new("aiclaw", "0.1.0");
        let resp = handler.handle_initialize(Value::Number(1.into()), &None);
        assert!(resp.result.is_some());
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["capabilities"]["tools"], serde_json::json!({}));
    }

    #[test]
    fn tools_list_returns_definitions() {
        let handler = ProtocolHandler::new("aiclaw", "0.1.0");
        let resp = handler.handle_tools_list(Value::Number(1.into()));
        assert!(resp.result.is_some());
        let tools = &resp.result.unwrap()["tools"];
        assert!(tools.is_array());
        assert!(tools.as_array().unwrap().len() >= 3);
    }

    #[tokio::test]
    async fn tools_call_send_succeeds() {
        let handler = ProtocolHandler::new("aiclaw", "0.1.0");
        let outbox = make_outbox();
        let params = serde_json::json!({
            "name": "send",
            "arguments": {
                "channel": "wechat",
                "conversation_id": "conv_001",
                "peer_id": "user_a",
                "conversation_type": "direct",
                "content": "hello"
            }
        });
        let resp = handler.handle_tools_call(Value::Number(1.into()), &Some(params), outbox.as_ref());
        assert!(resp.result.is_some());
    }

    #[tokio::test]
    async fn tools_call_unknown_tool_returns_error() {
        let handler = ProtocolHandler::new("aiclaw", "0.1.0");
        let outbox = make_outbox();
        let params = serde_json::json!({
            "name": "nonexistent",
            "arguments": {}
        });
        let resp = handler.handle_tools_call(Value::Number(1.into()), &Some(params), outbox.as_ref());
        assert!(resp.result.is_some());
        let content = &resp.result.unwrap()["content"][0]["text"];
        assert!(content.as_str().unwrap().contains("error"));
    }
}
