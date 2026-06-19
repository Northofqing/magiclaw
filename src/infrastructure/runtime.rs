use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use serde::{Deserialize, Serialize};

use crate::adapters::conversation_store::ConversationStore;
use crate::adapters::moka_dedup::MokaDedupCache;
use crate::adapters::sqlite_audit::SqliteAuditSink;
use crate::adapters::sqlite_conversation_state::SqliteConversationStateRepo;
use crate::adapters::sqlite_dead_letter::SqliteDeadLetterRepo;
use crate::adapters::sqlite_inbox::SqliteInboxRepo;
use crate::adapters::sqlite_outbox::SqliteOutboxRepo;
use crate::adapters::sqlite_sync_buf::SqliteSyncBufStore;
use crate::application::crash_recovery;
use crate::application::gc_janitor;
use crate::application::inbox_processor;
use crate::application::outbox_worker::{self, OutboxMessageSender};
use crate::application::resilient_sender::ResilientOutboxSender;
use crate::application::route_message::{self, RouteOutcome};
use crate::channels::dingtalk::channel::DingtalkChannel;
use crate::channels::feishu::channel::FeishuChannel;
use crate::channels::registry::ChannelRegistry;
use crate::channels::wechat::channel::WeChatChannel;
use crate::core::ai::backend::AiBackend;
use crate::core::ai::claude_code::ClaudeCodeBackend;
use crate::core::ai::cli_agent::{self, CliAgentBackend};
use crate::core::ai::echo::EchoBackend;
use crate::core::ai::resilient::ResilientAiBackend;
use crate::core::pipeline::ai::AiMiddleware;
use crate::core::pipeline::agent_command_middleware::AgentCommandMiddleware;
use crate::core::pipeline::formatter::Formatter;
use crate::core::pipeline::normalize::Normalize;
use crate::core::pipeline::outbox::OutboxStage;
use crate::core::pipeline::permission::Permission;
use crate::core::pipeline::rate_limit::RateLimit;
use crate::core::pipeline::Pipeline;
use crate::core::resilience::gate::ResilienceGate;
use crate::channels::wechat::ilink::{
    extract_latest_context_token, get_updates_via_ilink, send_text_via_ilink, ILinkGetUpdatesError,
    ILinkSendConfig,
};
use crate::domain::entities::message::MessageContent;
use crate::domain::ports::conversation_queue::ConversationQueue;
use crate::domain::ports::conversation_state_repo::ConversationStateRepo;
use crate::domain::ports::sync_buf_store::SyncBufStore;
use crate::domain::storage::outbox::{OutboxEntry, RetryConfig};
use crate::domain::value_objects::route_key::RouteKey;
use crate::infrastructure::config::AppConfig;
use crate::infrastructure::db::{self, DbPool};

pub struct AppRuntime {
    pub config: AppConfig,
    pub conversation_store: Arc<ConversationStore>,
    pub inbox_repo: Arc<SqliteInboxRepo>,
    pub outbox_repo: Arc<SqliteOutboxRepo>,
    pub dead_letter_repo: Arc<SqliteDeadLetterRepo>,
    pub channel_registry: Arc<ChannelRegistry>,
    dedup_cache: Arc<MokaDedupCache>,
    sync_buf_store: Arc<SqliteSyncBufStore>,
    audit_sink: Arc<SqliteAuditSink>,
    conversation_state_repo: Arc<SqliteConversationStateRepo>,
    send_gate: Arc<ResilienceGate>,
}

struct RegistryOutboxSender {
    registry: Arc<ChannelRegistry>,
}

impl RegistryOutboxSender {
    fn new(registry: Arc<ChannelRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl OutboxMessageSender for RegistryOutboxSender {
    async fn send(&self, entry: &OutboxEntry) -> Result<(), String> {
        let route_key: RouteKey = serde_json::from_str(&entry.route_key)
            .map_err(|e| format!("invalid route_key payload: {}", e))?;
        let payload: MessageContent = serde_json::from_str(&entry.payload)
            .map_err(|e| format!("invalid message payload: {}", e))?;

        self.registry
            .send_via(&route_key.channel, &route_key.peer_id, &payload)
            .await
            .map(|_| ())
    }
}

impl AppRuntime {
    pub fn new(config: AppConfig) -> Result<Self, Box<dyn std::error::Error>> {
        // Keep persistent state under the configured DB path and create the
        // parent directory on demand, so the default `data/aiclaw.db` works in
        // a fresh checkout without any manual mkdir step.
        if config.db_path != ":memory:" {
            let db_path = std::path::Path::new(&config.db_path);
            if let Some(parent) = db_path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
        }

        let conn = db::init_db(&config.db_path)?;
        let db_pool = DbPool::new(conn);

        let dedup_cache = Arc::new(MokaDedupCache::new(
            config.dedup_ttl_secs,
            config.dedup_max_capacity,
        ));
        let sync_buf_store = Arc::new(SqliteSyncBufStore::open(&config.db_path)?);
        let inbox_repo = Arc::new(SqliteInboxRepo::new(db_pool.clone()));
        let outbox_repo = Arc::new(SqliteOutboxRepo::new(db_pool.clone()));
        let dead_letter_repo = Arc::new(SqliteDeadLetterRepo::new(db_pool.clone()));
        let conversation_state_repo = Arc::new(SqliteConversationStateRepo::new(db_pool.clone()));
        let audit_sink = Arc::new(SqliteAuditSink::new(db_pool.clone()));

        // Phase 4: assemble the processing pipeline. Inbound messages flow
        // through Normalize -> Permission -> AI (echo by default, pluggable) ->
        // Formatter -> OutboxStage, where the formatted reply is persisted for
        // recoverable delivery. The AI backend is swappable and degrades to echo.
        //
        // Phase 5 (red line 2.5): the AI backend and the send path each run
        // behind a dedicated ResilienceGate (Circuit Breaker + Bulkhead), so a
        // failing AI dependency or platform API trips its breaker and the two
        // pools are concurrency-isolated from each other.
        let ai_gate = Arc::new(ResilienceGate::ai_default());
        let send_gate = Arc::new(ResilienceGate::send_default());
        // Phase 4 (red line: pluggable AI): select the backend from config and
        // wrap it in the resilience gate. Unknown/"echo" falls back to the echo
        // backend, so the system degrades safely and rollback is config-only.
        //
        // Helper function to create a backend by name
        let create_backend = |name: &str| -> Arc<dyn AiBackend> {
            match name {
                "echo" => Arc::new(EchoBackend),
                "claude_code" => {
                    tracing::info!("AI backend: claude_code (local claude CLI, restricted)");
                    Arc::new(ClaudeCodeBackend::new(config.ai.claude_code.clone()))
                }
                other => {
                    if let Some(custom) = config.ai.agents.get(other) {
                        tracing::info!(
                            backend = other,
                            binary = %custom.binary_path,
                            "AI backend: custom CLI agent"
                        );
                        Arc::new(CliAgentBackend::new(other, custom.clone()))
                    } else if let Some(preset) = cli_agent::preset(other) {
                        tracing::info!(
                            backend = other,
                            binary = %preset.binary_path,
                            "AI backend: preset CLI agent"
                        );
                        Arc::new(CliAgentBackend::new(other, preset))
                    } else {
                        tracing::warn!(backend = other, "unknown AI backend, falling back to echo");
                        Arc::new(EchoBackend)
                    }
                }
            }
        };

        // Create the default backend
        let inner_backend = create_backend(config.ai.backend.as_str());
        let ai_backend: Arc<dyn AiBackend> =
            Arc::new(ResilientAiBackend::new(inner_backend, ai_gate.clone()));

        // Phase A+: Create backend map for user agent preference switching.
        // Include all known agent names so users can switch to any of them.
        let mut backend_map: std::collections::HashMap<String, Arc<dyn AiBackend>> =
            std::collections::HashMap::new();
        
        // Add built-in agents
        for agent_name in &["echo", "claude_code", "codex", "openclaw", "hermes"] {
            let backend = create_backend(agent_name);
            let resilient_backend = Arc::new(ResilientAiBackend::new(backend, ai_gate.clone()));
            backend_map.insert(agent_name.to_string(), resilient_backend);
        }

        // Add custom agents from config
        for agent_name in config.ai.agents.keys() {
            let backend = create_backend(agent_name);
            let resilient_backend = Arc::new(ResilientAiBackend::new(backend, ai_gate.clone()));
            backend_map.insert(agent_name.clone(), resilient_backend);
        }
        // Build the chain. When a non-echo (cost-bearing) AI backend is active,
        // insert the RateLimit middleware after Permission so a per-conversation
        // minimum interval is enforced — guarding against runaway agent cost and
        // reply loops. A rate-limited message short-circuits before the AI stage.
        //
        // Phase A+: Insert AgentCommandMiddleware at the very start to intercept
        // and process agent switching commands before any other pipeline stages.
        let mut pipeline = Pipeline::new()
            .with(Box::new(AgentCommandMiddleware::new(
                db_pool.clone(),
                config.agent.clone(),
            )))
            .with(Box::new(Normalize))
            .with(Box::new(Permission));
        if config.ai.backend != "echo" {
            tracing::info!(
                min_interval_ms = config.ai.rate_limit_min_interval_ms,
                "rate limit enforced for AI agent backend"
            );
            pipeline =
                pipeline.with(Box::new(RateLimit::new(config.ai.rate_limit_min_interval_ms)));
        }
        let pipeline = Arc::new(
            pipeline
                .with(Box::new(AiMiddleware::with_backends(
                    ai_backend,
                    backend_map,
                    audit_sink.clone() as Arc<dyn crate::domain::ports::audit_sink::AuditSink>,
                )))
                .with(Box::new(Formatter))
                .with(Box::new(OutboxStage::new(
                    outbox_repo.clone() as Arc<dyn crate::domain::ports::outbox_repo::OutboxRepo>,
                ))),
        );

        let conversation_store = ConversationStore::new(
            config.backpressure.per_route_buffer,
            config.idle_timeout_secs,
            config.reorder_window_ms,
            Some(pipeline),
            config.clone(),
            Some(conversation_state_repo.clone()
                as Arc<dyn crate::domain::ports::conversation_state_repo::ConversationStateRepo>),
        );

        let mut registry = ChannelRegistry::new();
        registry.register(Arc::new(WeChatChannel::from_config_with_store(
            config.wechat.clone(),
            Some(sync_buf_store.clone()),
        )));
        registry.register(Arc::new(DingtalkChannel::new()));
        registry.register(Arc::new(FeishuChannel::new()));

        Ok(Self {
            config,
            conversation_store,
            inbox_repo,
            outbox_repo,
            dead_letter_repo,
            channel_registry: Arc::new(registry),
            dedup_cache,
            sync_buf_store,
            audit_sink,
            conversation_state_repo,
            send_gate,
        })
    }

    pub async fn start_background(&self) -> Result<(), String> {
        crash_recovery::recover_after_crash(self.outbox_repo.as_ref())?;

        match self.conversation_state_repo.load_all() {
            Ok(states) => {
                tracing::info!(
                    recovered_conversation_states = states.len(),
                    "loaded persisted conversation states on startup"
                );
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to load persisted conversation states");
            }
        }

        let gc_store = self.conversation_store.clone();
        let gc_timeout = self.config.idle_timeout_secs;
        let gc_interval = self.config.gc_scan_interval_secs;
        tokio::spawn(async move {
            gc_janitor::gc_janitor(gc_store.as_ref(), gc_timeout, gc_interval).await;
        });

        let mut inbound_rx = self.channel_registry.start_all().await?;
        let inbox = self.inbox_repo.clone();
        let dedup = self.dedup_cache.clone();
        let queue = self.conversation_store.clone();
        tokio::spawn(async move {
            while let Some(msg) = inbound_rx.recv().await {
                match inbox_processor::process_inbound(inbox.as_ref(), &msg) {
                    Ok(inbox_processor::InboxResult::Duplicate) => {
                        tracing::debug!(message_id = %msg.id, "inbound duplicate dropped by inbox");
                        continue;
                    }
                    Ok(inbox_processor::InboxResult::Processed) => {}
                    Err(e) => {
                        tracing::error!(message_id = %msg.id, error = %e, "failed to persist inbound message");
                        continue;
                    }
                }

                match route_message::route_message(dedup.as_ref(), queue.as_ref(), msg) {
                    RouteOutcome::Enqueued | RouteOutcome::Duplicate => {}
                    RouteOutcome::Dropped(message_id) => {
                        tracing::warn!(message_id = %message_id, "inbound route queue dropped message");
                    }
                }
            }
        });

        let outbox = self.outbox_repo.clone();
        let dlq = self.dead_letter_repo.clone();
        let registry = self.channel_registry.clone();
        let audit = self.audit_sink.clone();
        let send_gate = self.send_gate.clone();
        tokio::spawn(async move {
            let retry = RetryConfig::default();
            let inner = Arc::new(RegistryOutboxSender::new(registry)) as Arc<dyn OutboxMessageSender>;
            let sender = ResilientOutboxSender::new(inner, send_gate);
            outbox_worker::outbox_worker_loop(outbox.as_ref(), dlq.as_ref(), &retry, &sender, audit.as_ref(), 250, 64).await;
        });

        Ok(())
    }

    pub async fn process_outbox_once(&self) {
        let retry = RetryConfig::default();
        let inner = Arc::new(RegistryOutboxSender::new(self.channel_registry.clone()))
            as Arc<dyn OutboxMessageSender>;
        let sender = ResilientOutboxSender::new(inner, self.send_gate.clone());
        outbox_worker::process_pending(
            self.outbox_repo.as_ref(),
            self.dead_letter_repo.as_ref(),
            &retry,
            &sender,
            self.audit_sink.as_ref(),
            64,
        )
        .await;
        outbox_worker::process_retries(
            self.outbox_repo.as_ref(),
            self.dead_letter_repo.as_ref(),
            &retry,
            &sender,
            self.audit_sink.as_ref(),
            64,
        )
        .await;
    }

    pub fn active_conversations(&self) -> usize {
        self.conversation_store.active_conversations()
    }

    /// Start a minimal HTTP API server for in-process communication.
    ///
    /// Endpoints:
    ///   POST /api/send   — send a WeChat message via the live channel registry
    ///   GET  /api/health — liveness probe (returns 200)
    ///
    /// Binds to `addr` (default: 127.0.0.1:18011 matching weclaw convention).
    /// Returns immediately; server runs in a background tokio task.
    pub fn start_http_api(&self, addr: &str) -> Result<(), String> {
        #[derive(Clone)]
        struct PeerTokenState {
            token: String,
            observed_at: std::time::Instant,
            last_success_at: Option<std::time::Instant>,
            send_count: u32,
            stale: bool,
        }

        impl PeerTokenState {
            fn new(token: String) -> Self {
                Self {
                    token,
                    observed_at: std::time::Instant::now(),
                    last_success_at: None,
                    send_count: 0,
                    stale: false,
                }
            }

            fn should_refresh(&self) -> bool {
                const MAX_TOKEN_AGE_SECS: u64 = 25;
                const MAX_SENDS_PER_TOKEN: u32 = 8;

                self.stale
                    || self.observed_at.elapsed() >= std::time::Duration::from_secs(MAX_TOKEN_AGE_SECS)
                    || self.send_count >= MAX_SENDS_PER_TOKEN
            }

            fn mark_success(&mut self, next_token: Option<String>) {
                let now = std::time::Instant::now();
                if let Some(token) = next_token.filter(|token| !token.trim().is_empty()) {
                    if token != self.token {
                        self.token = token;
                        self.send_count = 0;
                        self.observed_at = now;
                    }
                }

                self.last_success_at = Some(now);
                self.send_count = self.send_count.saturating_add(1);
                self.stale = false;
            }

            fn mark_stale(&mut self) {
                self.stale = true;
            }

            fn observed_age_secs(&self) -> u64 {
                self.observed_at.elapsed().as_secs()
            }

            fn last_success_age_secs(&self) -> Option<u64> {
                self.last_success_at.map(|instant| instant.elapsed().as_secs())
            }
        }

        #[derive(Clone)]
        struct HttpApiState {
            wechat: crate::infrastructure::config::WeChatConfig,
            sync_buf_store: Arc<SqliteSyncBufStore>,
            token_cache: Arc<tokio::sync::Mutex<std::collections::HashMap<String, PeerTokenState>>>,
        }

        #[derive(Clone, Deserialize)]
        struct SendRequest {
            to: String,
            text: String,
            #[serde(default)]
            context_token: Option<String>,
        }

        #[derive(Serialize)]
        struct SendResponse {
            ok: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            error: Option<String>,
        }

        #[derive(Serialize)]
        struct WindowStatusEntry {
            peer_id: String,
            observed_age_secs: u64,
            last_success_age_secs: Option<u64>,
            send_count: u32,
            stale: bool,
            should_refresh: bool,
        }

        #[derive(Serialize)]
        struct WindowStatusResponse {
            ok: bool,
            peers: Vec<WindowStatusEntry>,
        }

        fn is_context_expired_error(err: &str) -> bool {
            let lower = err.to_ascii_lowercase();
            lower.contains("ret=-2") || lower.contains("context_token") || lower.contains("session")
        }

        fn extract_context_token_from_send_response(value: &serde_json::Value) -> Option<String> {
            value
                .get("msg")
                .and_then(|v| v.get("context_token"))
                .and_then(|v| v.as_str())
                .map(|v| v.to_string())
                .or_else(|| {
                    value
                        .get("context_token")
                        .and_then(|v| v.as_str())
                        .map(|v| v.to_string())
                })
                .filter(|v| !v.trim().is_empty())
        }

        async fn fetch_context_token_for_peer(
            state: &Arc<HttpApiState>,
            client: &reqwest::Client,
            peer_id: &str,
        ) -> Option<PeerTokenState> {
            let cfg = ILinkSendConfig {
                base_url: state.wechat.base_url.clone(),
                token: state.wechat.token.clone(),
                from_user_id: state.wechat.account_id.clone(),
                context_token: String::new(),
                channel_version: state.wechat.channel_version.clone(),
                timeout_ms: state.wechat.timeout_ms,
                keepalive_timeout_ms: state.wechat.keepalive_timeout_ms.max(40_000),
            };
            let sync_buf = state
                .sync_buf_store
                .load("wechat", &state.wechat.account_id)
                .ok()
                .map(|b| String::from_utf8_lossy(&b).to_string())
                .unwrap_or_default();

            let updates = get_updates_via_ilink(client, &cfg, &sync_buf).await.ok()?;
            if let Some(buf) = updates.get_updates_buf.as_deref() {
                let _ = state
                    .sync_buf_store
                    .save("wechat", &state.wechat.account_id, buf.as_bytes());
            }

            let token = extract_latest_context_token(&updates.msgs, Some(peer_id))?;
            let mut cache = state.token_cache.lock().await;
                let entry = PeerTokenState::new(token);
                tracing::info!(peer_id = %peer_id, "wechat peer token refreshed from getupdates");
                cache.insert(peer_id.to_string(), entry.clone());
                Some(entry)
        }

        let state = Arc::new(HttpApiState {
            wechat: self.config.wechat.clone(),
            sync_buf_store: self.sync_buf_store.clone(),
            token_cache: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        });

        // Short-interval token probe: runs every 25 seconds with a fast getupdates
        // (2-second timeout). Complements the 40-second long-poll by proactively
        // refreshing per-peer token cache so sends after a quiet period don't block.
        // Uses short timeout so it never delays itself — if no messages arrive within
        // 2 seconds it moves on and retries 25 seconds later.
        if state.wechat.enabled && !state.wechat.token.trim().is_empty() {
            let probe_state = state.clone();
            tokio::spawn(async move {
                const PROBE_INTERVAL_SECS: u64 = 25;
                const PROBE_TIMEOUT_MS: u64 = 2_000;

                let probe_cfg = ILinkSendConfig {
                    base_url: probe_state.wechat.base_url.clone(),
                    token: probe_state.wechat.token.clone(),
                    from_user_id: probe_state.wechat.account_id.clone(),
                    context_token: String::new(),
                    channel_version: probe_state.wechat.channel_version.clone(),
                    timeout_ms: probe_state.wechat.timeout_ms,
                    keepalive_timeout_ms: PROBE_TIMEOUT_MS,
                };
                let client = reqwest::Client::new();

                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(PROBE_INTERVAL_SECS)).await;

                    let sync_buf = probe_state
                        .sync_buf_store
                        .load("wechat", &probe_state.wechat.account_id)
                        .ok()
                        .map(|b| String::from_utf8_lossy(&b).to_string())
                        .unwrap_or_default();

                    match get_updates_via_ilink(&client, &probe_cfg, &sync_buf).await {
                        Ok(updates) => {
                            if let Some(buf) = updates.get_updates_buf.as_deref() {
                                let _ = probe_state
                                    .sync_buf_store
                                    .save("wechat", &probe_state.wechat.account_id, buf.as_bytes());
                            }

                            let mut refreshed = 0usize;
                            for msg in &updates.msgs {
                                if msg.message_type != Some(1) {
                                    continue;
                                }
                                let Some(token) = &msg.context_token else { continue };
                                if token.trim().is_empty() { continue }
                                let Some(uid) = msg.from_user_id.as_deref() else { continue };
                                if uid.trim().is_empty() || uid == probe_state.wechat.account_id {
                                    continue;
                                }
                                let mut cache = probe_state.token_cache.lock().await;
                                cache.insert(uid.to_string(), PeerTokenState::new(token.clone()));
                                refreshed += 1;
                            }

                            if refreshed > 0 {
                                tracing::info!(
                                    refreshed = refreshed,
                                    "wechat token probe: refreshed peer tokens"
                                );
                            } else {
                                // No inbound messages in probe window; mark all stale peers so
                                // next /api/send will do a proper long-poll refresh instead of
                                // using an expired token.
                                let stale_count = {
                                    let cache = probe_state.token_cache.lock().await;
                                    cache.values().filter(|e| e.should_refresh()).count()
                                };
                                if stale_count > 0 {
                                    tracing::debug!(
                                        stale_count = stale_count,
                                        "wechat token probe: no new messages, stale peers detected"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            tracing::debug!(error = %e, "wechat token probe: getupdates returned error (benign)");
                        }
                    }
                }
            });
        }

        // Start a background long-poll loop to keep context_token hot in memory.
        // This continuously consumes getupdates and updates per-user token cache,
        // so /api/send can avoid token-expiry gaps during continuous sending.
        if state.wechat.enabled && !state.wechat.token.trim().is_empty() {
            let poll_state = state.clone();
            tokio::spawn(async move {
                const INITIAL_RETRY_DELAY_MS: u64 = 2_000;
                const MAX_RETRY_DELAY_MS: u64 = 30_000;
                const MAX_CONSECUTIVE_FAILURES: u32 = 5;
                const SESSION_PAUSE_MS: u64 = 300_000;
                const MAX_SESSION_RETRIES: u32 = 3;
                const SESSION_EXPIRED_ERRCODE: i32 = -14;

                let mut poll_cfg = ILinkSendConfig {
                    base_url: poll_state.wechat.base_url.clone(),
                    token: poll_state.wechat.token.clone(),
                    from_user_id: poll_state.wechat.account_id.clone(),
                    context_token: String::new(),
                    channel_version: poll_state.wechat.channel_version.clone(),
                    timeout_ms: poll_state.wechat.timeout_ms,
                    // long-poll window: slightly above server 35s default.
                    keepalive_timeout_ms: poll_state
                        .wechat
                        .keepalive_timeout_ms
                        .max(40_000),
                };
                let client = reqwest::Client::new();
                let mut sync_buf = poll_state
                    .sync_buf_store
                    .load("wechat", &poll_state.wechat.account_id)
                    .ok()
                    .map(|b| String::from_utf8_lossy(&b).to_string())
                    .unwrap_or_default();
                let mut consecutive_failures: u32 = 0;
                let mut session_retries: u32 = 0;
                let mut retry_delay_ms: u64 = INITIAL_RETRY_DELAY_MS;

                loop {
                    match get_updates_via_ilink(&client, &poll_cfg, &sync_buf).await {
                        Ok(updates) => {
                            consecutive_failures = 0;
                            session_retries = 0;
                            retry_delay_ms = INITIAL_RETRY_DELAY_MS;

                            if let Some(buf) = updates.get_updates_buf {
                                sync_buf = buf;
                                let _ = poll_state
                                    .sync_buf_store
                                    .save("wechat", &poll_state.wechat.account_id, sync_buf.as_bytes());
                            }

                            let mut updated = 0usize;
                            for msg in updates.msgs {
                                // Official flow: context_token comes from inbound user messages.
                                if msg.message_type != Some(1) {
                                    continue;
                                }
                                let Some(token) = msg.context_token else {
                                    continue;
                                };
                                if token.trim().is_empty() {
                                    continue;
                                }

                                if let Some(uid) = msg.from_user_id.as_deref() {
                                    if !uid.trim().is_empty() && uid != poll_state.wechat.account_id {
                                        let mut cache = poll_state.token_cache.lock().await;
                                        cache.insert(uid.to_string(), PeerTokenState::new(token.clone()));
                                        tracing::debug!(peer_id = %uid, "wechat peer token refreshed from long-poll");
                                        updated += 1;
                                    }
                                }
                            }

                            if updated > 0 {
                                tracing::debug!(updated = updated, "wechat token cache refreshed from long-poll");
                            }
                        }
                        Err(ILinkGetUpdatesError::Business { errcode, .. })
                            if errcode == SESSION_EXPIRED_ERRCODE =>
                        {
                            session_retries += 1;
                            tracing::warn!(
                                session_retries = session_retries,
                                max_session_retries = MAX_SESSION_RETRIES,
                                "wechat session expired in long-poll"
                            );

                            if session_retries >= MAX_SESSION_RETRIES {
                                tracing::error!(
                                    "wechat long-poll stopped: session expired too many times; login required"
                                );
                                break;
                            }

                            tokio::time::sleep(std::time::Duration::from_millis(SESSION_PAUSE_MS)).await;
                        }
                        Err(e) => {
                            consecutive_failures += 1;
                            tracing::warn!(
                                error = %e,
                                consecutive_failures = consecutive_failures,
                                retry_delay_ms = retry_delay_ms,
                                "wechat token long-poll failed"
                            );

                            if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                                tracing::warn!(
                                    pause_ms = SESSION_PAUSE_MS,
                                    "wechat long-poll too many failures; pausing before resume"
                                );
                                tokio::time::sleep(std::time::Duration::from_millis(SESSION_PAUSE_MS)).await;
                                consecutive_failures = 0;
                                retry_delay_ms = INITIAL_RETRY_DELAY_MS;
                                continue;
                            }

                            tokio::time::sleep(std::time::Duration::from_millis(retry_delay_ms)).await;
                            retry_delay_ms = (retry_delay_ms.saturating_mul(2)).min(MAX_RETRY_DELAY_MS);

                            // keep long-poll timeout sane in case external config is invalid.
                            poll_cfg.keepalive_timeout_ms = poll_cfg.keepalive_timeout_ms.max(40_000);
                        }
                    }
                }
            });
        }

        let app: Router = Router::new()
            .route(
                "/api/send",
                post(
                    |State(state): State<Arc<HttpApiState>>, Json(req): Json<SendRequest>| async move {
                        let mut selected_ctx = req
                            .context_token
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string());
                        let mut cache_entry = None;

                        if selected_ctx.is_none() {
                            cache_entry = {
                                let cache = state.token_cache.lock().await;
                                cache.get(&req.to).cloned()
                            };
                            selected_ctx = cache_entry.as_ref().map(|entry| entry.token.clone());
                        }

                        let client = reqwest::Client::new();
                        let should_refresh = selected_ctx.is_none()
                            && cache_entry
                                .as_ref()
                                .map(|entry| entry.should_refresh())
                                .unwrap_or(true);
                        if should_refresh {
                            cache_entry = fetch_context_token_for_peer(&state, &client, &req.to).await;
                            selected_ctx = cache_entry.as_ref().map(|entry| entry.token.clone());
                        }

                        if selected_ctx.is_none() {
                            selected_ctx = req
                                .context_token
                                .as_deref()
                                .map(str::trim)
                                .filter(|token| !token.is_empty())
                                .map(|token| token.to_string());
                        }

                        if let Some(ctx) = selected_ctx {
                            let base_cfg = ILinkSendConfig {
                                base_url: state.wechat.base_url.clone(),
                                token: state.wechat.token.clone(),
                                from_user_id: state.wechat.account_id.clone(),
                                context_token: ctx.clone(),
                                channel_version: state.wechat.channel_version.clone(),
                                timeout_ms: state.wechat.timeout_ms,
                                keepalive_timeout_ms: state.wechat.keepalive_timeout_ms,
                            };
                            match send_text_via_ilink(&client, &base_cfg, &req.to, &req.text).await {
                                Ok(resp) => {
                                    let next_ctx = extract_context_token_from_send_response(&resp);
                                    if let Ok(mut cache) = state.token_cache.try_lock() {
                                        let entry = cache
                                            .entry(req.to.clone())
                                            .or_insert_with(|| PeerTokenState::new(ctx.clone()));
                                        entry.mark_success(next_ctx);
                                        tracing::debug!(
                                            peer_id = %req.to,
                                            observed_age_secs = entry.observed_age_secs(),
                                            send_count = entry.send_count,
                                            stale = entry.stale,
                                            "wechat peer token send succeeded"
                                        );
                                    }
                                    return (
                                        StatusCode::OK,
                                        Json(SendResponse { ok: true, error: None }),
                                    );
                                }
                                Err(e) => {
                                    if is_context_expired_error(&e) {
                                        {
                                            let mut cache = state.token_cache.lock().await;
                                            if let Some(entry) = cache.get_mut(&req.to) {
                                                entry.mark_stale();
                                                tracing::warn!(
                                                    peer_id = %req.to,
                                                    observed_age_secs = entry.observed_age_secs(),
                                                    send_count = entry.send_count,
                                                    "wechat peer token marked stale after ret=-2"
                                                );
                                            }
                                        }

                                        if let Some(refreshed) = fetch_context_token_for_peer(&state, &client, &req.to).await {
                                            let mut retry_cfg = base_cfg.clone();
                                            retry_cfg.context_token = refreshed.token.clone();
                                            match send_text_via_ilink(&client, &retry_cfg, &req.to, &req.text).await {
                                                Ok(resp) => {
                                                    let next_ctx = extract_context_token_from_send_response(&resp);
                                                    let mut cache = state.token_cache.lock().await;
                                                    let entry = cache
                                                        .entry(req.to.clone())
                                                        .or_insert(refreshed);
                                                    entry.mark_success(next_ctx);
                                                    return (
                                                        StatusCode::OK,
                                                        Json(SendResponse { ok: true, error: None }),
                                                    );
                                                }
                                                Err(retry_err) => {
                                                    let mut cache = state.token_cache.lock().await;
                                                    if let Some(entry) = cache.get_mut(&req.to) {
                                                        entry.mark_stale();
                                                        tracing::warn!(
                                                            peer_id = %req.to,
                                                            observed_age_secs = entry.observed_age_secs(),
                                                            send_count = entry.send_count,
                                                            "wechat peer token retry failed and remains stale"
                                                        );
                                                    }
                                                    return (
                                                        StatusCode::BAD_GATEWAY,
                                                        Json(SendResponse {
                                                            ok: false,
                                                            error: Some(retry_err),
                                                        }),
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    return (
                                        StatusCode::BAD_GATEWAY,
                                        Json(SendResponse { ok: false, error: Some(e) }),
                                    );
                                }
                            }
                        }

                        (
                            StatusCode::PRECONDITION_FAILED,
                            Json(SendResponse {
                                ok: false,
                                error: Some(
                                    "no valid context_token for peer; wait for inbound user message then retry"
                                        .to_string(),
                                ),
                            }),
                        )
                    },
                ),
            )
            .route(
                "/api/health",
                get(|| async { (StatusCode::OK, Json(serde_json::json!({"ok": true}))) }),
            )
            .route(
                "/api/window_status",
                get(|State(state): State<Arc<HttpApiState>>| async move {
                    let cache = state.token_cache.lock().await;
                    let mut peers: Vec<WindowStatusEntry> = cache
                        .iter()
                        .map(|(peer_id, entry)| WindowStatusEntry {
                            peer_id: peer_id.clone(),
                            observed_age_secs: entry.observed_age_secs(),
                            last_success_age_secs: entry.last_success_age_secs(),
                            send_count: entry.send_count,
                            stale: entry.stale,
                            should_refresh: entry.should_refresh(),
                        })
                        .collect();
                    peers.sort_by(|left, right| left.peer_id.cmp(&right.peer_id));

                    (
                        StatusCode::OK,
                        Json(WindowStatusResponse {
                            ok: true,
                            peers,
                        }),
                    )
                }),
            )
            .with_state(state);

        let addr: std::net::SocketAddr = addr
            .parse()
            .map_err(|e| format!("invalid HTTP API address '{}': {}", addr, e))?;

        tokio::spawn(async move {
            let listener = match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!(error = %e, %addr, "failed to bind HTTP API");
                    return;
                }
            };
            tracing::info!(%addr, "HTTP API listening");
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!(error = %e, "HTTP API server error");
            }
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::send_message;
    use crate::domain::ports::dead_letter_repo::DeadLetterRepo;
    use crate::domain::ports::outbox_repo::OutboxRepo;
    use crate::domain::value_objects::route_key::ConversationType;

    #[tokio::test]
    async fn process_outbox_once_moves_pending_to_sent() {
        let runtime = AppRuntime::new(AppConfig {
            db_path: ":memory:".into(),
            ..AppConfig::default()
        })
        .unwrap();

        let message_id = send_message::submit_text_for_delivery(
            runtime.outbox_repo.as_ref(),
            "wechat",
            "conv_001",
            "user_a",
            ConversationType::Direct,
            "hello runtime",
        )
        .unwrap();

        runtime.process_outbox_once().await;

        let recovered = runtime.outbox_repo.recover_after_crash().unwrap();
        assert!(recovered.is_empty());

        let pending = runtime.outbox_repo.fetch_pending(10).unwrap();
        assert!(pending.is_empty());

        let retryable = runtime.outbox_repo.fetch_retryable(i64::MAX, 10).unwrap();
        assert!(retryable.is_empty());

        let dlq = runtime.dead_letter_repo.list(10).unwrap();
        assert!(dlq.is_empty());

        let inflight = runtime.outbox_repo.recover_after_crash().unwrap();
        assert!(inflight.is_empty());

        let entries = runtime.outbox_repo.fetch_pending(10).unwrap();
        assert!(entries.iter().all(|entry| entry.id != message_id));
    }
}