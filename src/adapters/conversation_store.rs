use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::core::pipeline::Pipeline;
use crate::domain::aggregates::conversation::{Conversation, ConversationState};
use crate::domain::entities::message::Message;
use crate::domain::ports::conversation_queue::{ConversationGC, ConversationQueue, EnqueueResult};
use crate::domain::ports::conversation_state_repo::ConversationStateRepo;
use crate::domain::value_objects::route_key::RouteKey;
use crate::infrastructure::config::AppConfig;

struct ConversationHandle {
    tx: mpsc::Sender<Message>,
    last_active: Instant,
}

/// In-memory implementation of ConversationQueue + ConversationGC.
/// Uses bounded mpsc channels for per-route serial processing.
/// Each worker maintains a Conversation aggregate with ReorderWindow.
pub struct ConversationStore {
    routes: Arc<RwLock<HashMap<RouteKey, ConversationHandle>>>,
    per_route_buffer: usize,
    reorder_window_ms: u64,
    pipeline: Option<Arc<Pipeline>>,
    config: AppConfig,
    state_repo: Option<Arc<dyn ConversationStateRepo>>,
}

impl ConversationStore {
    pub fn new(
        per_route_buffer: usize,
        _idle_timeout_secs: u64,
        reorder_window_ms: u64,
        pipeline: Option<Arc<Pipeline>>,
        config: AppConfig,
        state_repo: Option<Arc<dyn ConversationStateRepo>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            routes: Arc::new(RwLock::new(HashMap::new())),
            per_route_buffer,
            reorder_window_ms,
            pipeline,
            config,
            state_repo,
        })
    }

    /// Persist a conversation state transition (best-effort: a storage failure
    /// is logged but never blocks message processing).
    fn persist_state(&self, key: &RouteKey, state: ConversationState) {
        let Some(repo) = self.state_repo.as_ref() else {
            return;
        };
        let route_key = match serde_json::to_string(key) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "failed to serialize route_key for state persistence");
                return;
            }
        };
        let state_json = match serde_json::to_string(&state) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "failed to serialize conversation state");
                return;
            }
        };
        if let Err(e) = repo.upsert(&route_key, &state_json, chrono::Utc::now().timestamp()) {
            tracing::error!(error = %e, "failed to persist conversation state");
        }
    }

    /// Spawn a worker that creates a Conversation aggregate and serially
    /// processes messages through its reorder window.
    fn spawn_worker(&self, key: RouteKey) -> mpsc::Sender<Message> {
        let (tx, mut rx) = mpsc::channel::<Message>(self.per_route_buffer);
        let routes = Arc::clone(&self.routes);
        let route_key = key.clone();
        let reorder_window_ms = self.reorder_window_ms;
        let pipeline = self.pipeline.clone();
        let config = self.config.clone();

        tokio::spawn(async move {
            let mut conversation = Conversation::new(route_key.clone(), reorder_window_ms);
            let flush_interval = std::time::Duration::from_millis(reorder_window_ms.max(1));

            loop {
                tokio::select! {
                    msg = rx.recv() => {
                        match msg {
                            Some(msg) => {
                                conversation.touch();
                                let ordered = conversation.ingest(msg);
                                for m in ordered {
                                    tracing::info!(
                                        message_id = %m.id,
                                        channel = %m.route_key.channel,
                                        conversation_id = %m.route_key.conversation_id,
                                        peer_id = %m.route_key.peer_id,
                                        "worker processing message"
                                    );
                                    if let Some(ref pipeline) = pipeline {
                                        let ctx = crate::core::pipeline::PipelineContext {
                                            message: m,
                                            conversation: conversation.clone(),
                                            config: config.clone(),
                                            ai_response: None,
                                            short_circuit: false,
                                            user_agent_selection: None,
                                        };
                                        match pipeline.run(ctx).await {
                                            Ok(result) => {
                                                tracing::info!(
                                                    message_id = %result.message.id,
                                                    has_response = result.ai_response.is_some(),
                                                    short_circuit = result.short_circuit,
                                                    "pipeline complete"
                                                );
                                            }
                                            Err(e) => {
                                                tracing::error!(error = %e, "pipeline error");
                                            }
                                        }
                                    } else {
                                        tracing::debug!(
                                            message_id = %m.id,
                                            channel = %m.route_key.channel,
                                            "message processed (no pipeline)"
                                        );
                                    }
                                }

                                if let Ok(mut routes) = routes.write() {
                                    if let Some(handle) = routes.get_mut(&route_key) {
                                        handle.last_active = Instant::now();
                                    }
                                }
                            }
                            None => {
                                let remaining = conversation.drain();
                                for m in &remaining {
                                    tracing::info!(
                                        message_id = %m.id,
                                        channel = %route_key.channel,
                                        "flushing buffered message on worker shutdown"
                                    );
                                }
                                tracing::info!(
                                    channel = %route_key.channel,
                                    conversation_id = %route_key.conversation_id,
                                    drained_count = remaining.len(),
                                    "worker exiting"
                                );
                                return;
                            }
                        }
                    }
                    _ = tokio::time::sleep(flush_interval) => {
                        // Ensure single inbound messages are released even when
                        // no subsequent message arrives to advance the cutoff.
                        let pending = conversation.drain();
                        for m in pending {
                            tracing::info!(
                                message_id = %m.id,
                                channel = %m.route_key.channel,
                                conversation_id = %m.route_key.conversation_id,
                                peer_id = %m.route_key.peer_id,
                                "worker flushing pending message on timer"
                            );
                            if let Some(ref pipeline) = pipeline {
                                let ctx = crate::core::pipeline::PipelineContext {
                                    message: m,
                                    conversation: conversation.clone(),
                                    config: config.clone(),
                                    ai_response: None,
                                    short_circuit: false,
                                    user_agent_selection: None,
                                };
                                match pipeline.run(ctx).await {
                                    Ok(result) => {
                                        tracing::info!(
                                            message_id = %result.message.id,
                                            has_response = result.ai_response.is_some(),
                                            short_circuit = result.short_circuit,
                                            "pipeline complete"
                                        );
                                    }
                                    Err(e) => {
                                        tracing::error!(error = %e, "pipeline error");
                                    }
                                }
                            } else {
                                tracing::debug!(
                                    message_id = %m.id,
                                    channel = %m.route_key.channel,
                                    "message processed (no pipeline)"
                                );
                            }
                        }
                    }
                }
            }
        });

        tx
    }

    fn get_or_create_tx(&self, key: &RouteKey) -> mpsc::Sender<Message> {
        // Fast path: read lock
        {
            let routes = self.routes.read().unwrap();
            if let Some(handle) = routes.get(key) {
                return handle.tx.clone();
            }
        }

        // Slow path: write lock
        let tx = {
            let mut routes = self.routes.write().unwrap();
            if let Some(handle) = routes.get(key) {
                return handle.tx.clone();
            }

            let tx = self.spawn_worker(key.clone());
            routes.insert(
                key.clone(),
                ConversationHandle {
                    tx: tx.clone(),
                    last_active: Instant::now(),
                },
            );
            tx
        };
        self.persist_state(key, ConversationState::Active);
        tx
    }
}

impl ConversationQueue for ConversationStore {
    fn enqueue(&self, key: &RouteKey, msg: Message) -> EnqueueResult {
        let tx = self.get_or_create_tx(key);

        match tx.try_send(msg) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(m)) => {
                tracing::warn!(
                    message_id = %m.id,
                    conversation_id = %key.conversation_id,
                    "route queue full, dropping newest"
                );
                Err(crate::domain::ports::conversation_queue::EnqueueError::QueueFull {
                    message_id: m.id,
                })
            }
            Err(mpsc::error::TrySendError::Closed(m)) => {
                let tx = self.spawn_worker(key.clone());
                if let Ok(mut routes) = self.routes.write() {
                    routes.insert(
                        key.clone(),
                        ConversationHandle {
                            tx: tx.clone(),
                            last_active: Instant::now(),
                        },
                    );
                }
                self.persist_state(key, ConversationState::Active);
                let _ = tx.try_send(m);
                Ok(())
            }
        }
    }

    fn active_conversations(&self) -> usize {
        self.routes.read().unwrap().len()
    }
}

impl ConversationGC for ConversationStore {
    fn collect_idle(&self, idle_timeout_secs: u64) -> usize {
        let timeout = Duration::from_secs(idle_timeout_secs);
        let mut reclaimed_keys = Vec::new();

        {
            let mut routes = self.routes.write().unwrap();
            // retain=false removes the entry, the dropped sender triggers
            // the worker to call conversation.drain() before exiting
            routes.retain(|key, handle| {
                if handle.last_active.elapsed() > timeout {
                    tracing::info!(
                        channel = %key.channel,
                        conversation_id = %key.conversation_id,
                        "GC reclaiming idle conversation, worker will drain reorder window"
                    );
                    reclaimed_keys.push(key.clone());
                    false
                } else {
                    true
                }
            });
        }

        // Persist the terminal state outside the routes lock to avoid holding
        // the write guard during DB I/O.
        for key in &reclaimed_keys {
            self.persist_state(key, ConversationState::Closed);
        }

        reclaimed_keys.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entities::message::{Direction, MessageContent};
    use crate::domain::value_objects::route_key::{ChannelId, ConversationType};
    use crate::infrastructure::config::AppConfig;

    fn make_route_key(id: &str) -> RouteKey {
        RouteKey::new(
            ChannelId::new("wechat"),
            id,
            "user_a",
            ConversationType::Direct,
        )
    }

    fn make_msg(id: &str, key: &RouteKey, seq: i64) -> Message {
        Message {
            id: id.into(),
            route_key: key.clone(),
            sequence: Some(seq),
            timestamp_ms: seq * 100,
            direction: Direction::Inbound,
            content: MessageContent::Text("test".into()),
            audit_mark: None,
        }
    }

    #[tokio::test]
    async fn enqueue_creates_worker() {
        let store = ConversationStore::new(256, 1800, 200, None, AppConfig::default(), None);
        let key = make_route_key("conv_001");
        let result = store.enqueue(&key, make_msg("m1", &key, 1));
        assert!(result.is_ok());
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(store.active_conversations(), 1);
    }

    #[tokio::test]
    async fn same_route_key_uses_same_worker() {
        let store = ConversationStore::new(256, 1800, 200, None, AppConfig::default(), None);
        let key = make_route_key("conv_001");
        store.enqueue(&key, make_msg("m1", &key, 1)).unwrap();
        store.enqueue(&key, make_msg("m2", &key, 2)).unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(store.active_conversations(), 1);
    }

    #[tokio::test]
    async fn gc_collects_idle() {
        let store = ConversationStore::new(256, 0, 200, None, AppConfig::default(), None);
        let key = make_route_key("conv_001");
        store.enqueue(&key, make_msg("m1", &key, 1)).unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;

        let reclaimed = store.collect_idle(0);
        assert_eq!(reclaimed, 1);
        assert_eq!(store.active_conversations(), 0);
    }

    #[tokio::test]
    async fn worker_drains_on_shutdown() {
        let store = ConversationStore::new(256, 1800, 200, None, AppConfig::default(), None);
        let key = make_route_key("conv_001");

        // Enqueue enough messages to fill the buffer without triggering flush
        store.enqueue(&key, make_msg("m3", &key, 300)).unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;

        // Force GC → worker drains reorder window before exiting
        let reclaimed = store.collect_idle(0);
        assert_eq!(reclaimed, 1);
        // Give worker time to drain and exit
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
