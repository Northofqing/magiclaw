use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use async_trait::async_trait;

use super::middleware::{Middleware, PipelineContext};

/// RateLimit middleware: per-conversation rate limiting.
pub struct RateLimit {
    /// Track last access time per conversation.
    windows: Mutex<HashMap<String, Instant>>,
    /// Minimum interval between messages in milliseconds.
    min_interval_ms: u64,
}

impl RateLimit {
    pub fn new(min_interval_ms: u64) -> Self {
        Self { windows: Mutex::new(HashMap::new()), min_interval_ms }
    }
}

#[async_trait]
impl Middleware for RateLimit {
    fn name(&self) -> &'static str { "rate_limit" }

    async fn process(&self, mut ctx: PipelineContext) -> Result<PipelineContext, String> {
        // Some platforms (e.g. WeChat group inbound) may leave conversation_id empty.
        // Fall back to peer_id to avoid collapsing all traffic into one global bucket.
        let scope = if ctx.message.route_key.conversation_id.is_empty() {
            ctx.message.route_key.peer_id.as_str()
        } else {
            ctx.message.route_key.conversation_id.as_str()
        };
        let key = format!("{}:{}", ctx.message.route_key.channel, scope);
        let mut windows = self.windows.lock().map_err(|e| e.to_string())?;

        if let Some(last) = windows.get(&key) {
            let elapsed = last.elapsed().as_millis() as u64;
            if elapsed < self.min_interval_ms {
                tracing::warn!(
                    conversation = %key,
                    channel = %ctx.message.route_key.channel,
                    conversation_id = %ctx.message.route_key.conversation_id,
                    peer_id = %ctx.message.route_key.peer_id,
                    elapsed_ms = elapsed,
                    min_interval_ms = self.min_interval_ms,
                    "rate limited"
                );
                ctx.short_circuit = true;
            }
        }

        windows.insert(key, Instant::now());
        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entities::message::{Direction, MessageContent};
    use crate::domain::value_objects::route_key::{ChannelId, ConversationType, RouteKey};
    use crate::domain::aggregates::conversation::Conversation;
    use crate::infrastructure::config::AppConfig;

    fn make_ctx(id: &str) -> PipelineContext {
        let rk = RouteKey::new(ChannelId::new("wechat"), "c1", "p1", ConversationType::Direct);
        PipelineContext {
            message: crate::domain::entities::message::Message {
                id: id.into(), route_key: rk.clone(), sequence: None, timestamp_ms: 1,
                direction: Direction::Inbound, content: MessageContent::Text("hi".into()), audit_mark: None,
            },
            conversation: Conversation::new(rk, 200),
            config: AppConfig::default(),
            user_agent_selection: None,
            ai_response: None,
            short_circuit: false,
        }
    }

    #[tokio::test]
    async fn first_message_passes() {
        let rl = RateLimit::new(1000);
        let result = rl.process(make_ctx("m1")).await.unwrap();
        assert!(!result.short_circuit);
    }

    #[tokio::test]
    async fn rapid_second_message_is_limited() {
        let rl = RateLimit::new(1000);
        let _ = rl.process(make_ctx("m1")).await.unwrap();
        let result = rl.process(make_ctx("m2")).await.unwrap();
        assert!(result.short_circuit);
    }
}
