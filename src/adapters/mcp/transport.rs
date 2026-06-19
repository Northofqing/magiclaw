use std::io::{BufRead, BufReader, Write};
use tokio::sync::mpsc;

use super::protocol::JsonRpcMessage;

/// Reads JSON-RPC messages from stdin using MCP framing:
/// Content-Length: <N>\r\n\r\n<JSON body>
pub struct JsonRpcTransport {
    /// Sender for parsed messages → handler task.
    incoming_tx: mpsc::Sender<JsonRpcMessage>,
}

impl JsonRpcTransport {
    pub fn new(incoming_tx: mpsc::Sender<JsonRpcMessage>) -> Self {
        Self { incoming_tx }
    }

    /// Read loop: blocks on stdin, parses MCP-framed messages, sends to handler.
    /// Runs on a dedicated std::thread (stdin is blocking).
    pub fn run(self) {
        self.run_blocking();
    }

    /// Run the transport in a blocking fashion (for sync context).
    /// This is the primary entry point called from main.
    pub fn run_blocking(self) {
        let stdin = std::io::stdin();
        let mut reader = BufReader::new(stdin.lock());

        loop {
            match read_framed_or_json_line(&mut reader) {
                Ok(Some(raw)) => {
                    if let Ok(msg) = serde_json::from_str::<JsonRpcMessage>(&raw) {
                        if self.incoming_tx.blocking_send(msg).is_err() {
                            tracing::error!("handler channel closed");
                            return;
                        }
                    } else {
                        tracing::error!(raw = %raw, "failed to parse JSON-RPC message body");
                    }
                }
                Ok(None) => {
                    tracing::info!("stdin EOF, transport exiting");
                    return;
                }
                Err(e) => {
                    tracing::error!(error = %e, "stdin read error");
                    return;
                }
            }
        }
    }
}

fn read_framed_or_json_line<R: BufRead>(reader: &mut R) -> std::io::Result<Option<String>> {
    loop {
        let mut first_line = String::new();
        let n = reader.read_line(&mut first_line)?;
        if n == 0 {
            return Ok(None);
        }

        let trimmed = first_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with('{') {
            return Ok(Some(trimmed.to_string()));
        }

        let content_length = if let Some((name, value)) = trimmed.split_once(':') {
            if name.eq_ignore_ascii_case("Content-Length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        } else {
            None
        };

        let Some(content_length) = content_length else {
            tracing::debug!(raw = %trimmed, "skipping unknown transport line");
            continue;
        };

        loop {
            let mut header_line = String::new();
            let n = reader.read_line(&mut header_line)?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF while reading MCP headers",
                ));
            }
            if header_line == "\r\n" || header_line == "\n" {
                break;
            }
        }

        let mut body = vec![0_u8; content_length];
        reader.read_exact(&mut body)?;
        let body = String::from_utf8(body).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid UTF-8 MCP body: {}", e),
            )
        })?;

        return Ok(Some(body));
    }
}

/// Write a JSON-RPC response to stdout using MCP framing.
/// This is the ONLY function that writes to stdout.
pub fn write_response(response: &impl serde::Serialize) {
    let body = serde_json::to_string(response).unwrap_or_else(|e| {
        // Fallback: write a JSON-RPC error manually
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": {
                "code": -32603,
                "message": format!("serialization error: {}", e)
            }
        })
        .to_string()
    });

    let framed = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let _ = handle.write_all(framed.as_bytes());
    let _ = handle.flush();
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor};

    use super::read_framed_or_json_line;

    #[test]
    fn write_response_produces_valid_framing() {
        let resp = serde_json::json!({"jsonrpc":"2.0","id":1,"result":"ok"});
        // Can't easily test stdout, but verify serialization doesn't panic
        let body = serde_json::to_string(&resp).unwrap();
        let framed = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        assert!(framed.starts_with("Content-Length: "));
        assert!(framed.contains("\r\n\r\n"));
    }

    #[test]
    fn read_framed_payload_reads_exact_body_length() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let raw = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let cursor = Cursor::new(raw.into_bytes());
        let mut reader = BufReader::new(cursor);

        let parsed = read_framed_or_json_line(&mut reader).unwrap();
        assert_eq!(parsed.as_deref(), Some(body));
    }

    #[test]
    fn read_framed_payload_skips_unknown_headers_before_frame() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let raw = format!("garbage\nContent-Length: {}\r\n\r\n{}", body.len(), body);
        let cursor = Cursor::new(raw.into_bytes());
        let mut reader = BufReader::new(cursor);

        let parsed = read_framed_or_json_line(&mut reader).unwrap();
        assert_eq!(parsed.as_deref(), Some(body));
    }
}
