use std::sync::Arc;

use crate::domain::ports::outbox_repo::OutboxRepo;

use super::handler::ProtocolHandler;
use super::protocol::JsonRpcMessage;
use super::transport::JsonRpcTransport;

/// MCP Server: wires transport → handler → domain.
pub struct McpServer {
    handler: ProtocolHandler,
    outbox: Arc<dyn OutboxRepo>,
}

impl McpServer {
    pub fn new(
        server_name: impl Into<String>,
        server_version: impl Into<String>,
        outbox: Arc<dyn OutboxRepo>,
    ) -> Self {
        Self {
            handler: ProtocolHandler::new(server_name, server_version),
            outbox,
        }
    }

    /// Start the MCP server on stdio. Must be awaited from within a tokio runtime.
    /// The transport reader runs on a dedicated blocking thread (stdin is blocking);
    /// this task drives the message loop on the ambient runtime.
    pub async fn run(self) {
        tracing::info!("MCP server starting on stdio");

        // Redirect panics to stderr so they don't corrupt stdout protocol.
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let msg = format!("{}", info);
            // Write to stderr only, never stdout
            eprintln!("PANIC: {}", msg);
            default_hook(info);
        }));

        let (tx, mut rx) = tokio::sync::mpsc::channel::<JsonRpcMessage>(256);

        // Spawn transport reader on a blocking thread (stdin is blocking)
        let transport = JsonRpcTransport::new(tx);
        std::thread::spawn(move || {
            transport.run_blocking();
        });

        // Process incoming messages on the ambient runtime.
        loop {
            match rx.recv().await {
                Some(JsonRpcMessage::Request(req)) => {
                    self.handler.handle_request(req, self.outbox.as_ref());
                }
                Some(JsonRpcMessage::Notification(notif)) => {
                    self.handler.handle_notification(&notif.method, &notif.params);
                }
                None => {
                    tracing::info!("stdin channel closed, MCP server shutting down");
                    break;
                }
            }
        }

        tracing::info!("MCP server stopped");
    }
}
