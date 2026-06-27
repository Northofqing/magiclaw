use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::State;
use axum::middleware::from_fn_with_state;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use serde::{Deserialize, Serialize};

use crate::adapters::conversation_store::ConversationStore;
use crate::adapters::api_client_registry::ApiClientRegistry;
use crate::adapters::http_auth::{require_bearer_auth, HttpAuth};
use crate::adapters::moka_dedup::MokaDedupCache;
use crate::adapters::sqlite_audit::SqliteAuditSink;
use crate::adapters::sqlite_context_tokens::SqliteContextTokenStore;
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
use crate::channels::feishu::channel::{
    parse_webhook_event, verify_webhook_signature, FeishuChannel, FeishuWebhookDispatch,
};
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
    get_updates_via_ilink, send_text_via_ilink, ILinkGetUpdatesError,
    ILinkSendConfig,
};
use crate::domain::entities::message::MessageContent;
use crate::domain::ports::conversation_queue::ConversationQueue;
use crate::domain::ports::conversation_state_repo::ConversationStateRepo;
use crate::domain::ports::inbox_repo::RepoError;
use crate::domain::ports::audit_sink::AuditSink;
use crate::domain::ports::context_token_store::ContextTokenStore;
use crate::domain::ports::sync_buf_store::SyncBufStore;
use crate::domain::storage::outbox::{OutboxEntry, RetryConfig};
use crate::domain::value_objects::route_key::RouteKey;
use crate::infrastructure::config::{AppConfig, FeishuConfig};
use crate::infrastructure::db::{self, DbPool};
use crate::infrastructure::daily_logger::DailyLogger;
use crate::infrastructure::task_supervisor::{TaskState, TaskSupervisor};

fn persist_context_token(
    store: &dyn ContextTokenStore,
    account_id: &str,
    peer_id: &str,
    token: &str,
) -> Result<(), String> {
    store
        .set(account_id, peer_id, token)
        .map_err(|e| format!("persist context token failed: {}", e))
}

fn load_persisted_context_tokens(
    store: &dyn ContextTokenStore,
    account_id: &str,
) -> Result<HashMap<String, String>, String> {
    store
        .get_all(account_id)
        .map_err(|e| format!("load persisted context tokens failed: {}", e))
}

fn clear_persisted_context_tokens(store: &dyn ContextTokenStore, account_id: &str) -> Result<(), String> {
    store
        .delete_all(account_id)
        .map_err(|e| format!("clear persisted context tokens failed: {}", e))
}

pub struct AppRuntime {
    pub config: AppConfig,
    pub conversation_store: Arc<ConversationStore>,
    pub inbox_repo: Arc<SqliteInboxRepo>,
    pub outbox_repo: Arc<SqliteOutboxRepo>,
    pub dead_letter_repo: Arc<SqliteDeadLetterRepo>,
    pub channel_registry: Arc<ChannelRegistry>,
    api_client_registry: Arc<ApiClientRegistry>,
    dedup_cache: Arc<MokaDedupCache>,
    sync_buf_store: Arc<SqliteSyncBufStore>,
    audit_sink: Arc<SqliteAuditSink>,
    conversation_state_repo: Arc<SqliteConversationStateRepo>,
    send_gate: Arc<ResilienceGate>,
    /// Supervised background tasks (GC janitor, inbound router, outbox worker,
    /// wechat token poller, HTTP API). Panics are observable instead of
    /// silently dropped. See `task_supervisor.rs` for the design.
    pub task_supervisor: Arc<TaskSupervisor>,
}

fn extract_feishu_verification_token(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("token")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
        .or_else(|| {
            payload
                .get("header")
                .and_then(|h| h.get("token"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| v.to_string())
        })
}

fn select_feishu_config<'a>(configs: &'a [FeishuConfig], payload: &serde_json::Value) -> Option<&'a FeishuConfig> {
    if configs.is_empty() {
        return None;
    }

    if let Some(token) = extract_feishu_verification_token(payload) {
        if let Some(cfg) = configs
            .iter()
            .find(|c| !c.verification_token.trim().is_empty() && c.verification_token.trim() == token)
        {
            return Some(cfg);
        }
    }

    if configs.len() == 1 {
        return configs.first();
    }

    configs
        .iter()
        .find(|c| c.verification_token.trim().is_empty())
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
            .map_err(|e| e.to_string())
    }
}

impl AppRuntime {
    pub fn new(config: AppConfig) -> Result<Self, Box<dyn std::error::Error>> {
        // Keep persistent state under the configured DB path and create the
        // parent directory on demand, so the default `data/magiclaw.db` works in
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
        let api_client_registry = Arc::new(ApiClientRegistry::new(db_pool.clone()));

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
        let context_token_store = Arc::new(
            SqliteContextTokenStore::open(&config.db_path)
                .map_err(|e| format!("failed to open context token store: {}", e))?,
        );

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
            Some(context_token_store.clone()),
        )));
        registry.register(Arc::new(DingtalkChannel::new()));
        registry.register(Arc::new(FeishuChannel::from_config(config.feishu.clone())));
        for feishu_cfg in &config.feishu_accounts {
            registry.register(Arc::new(FeishuChannel::from_config(feishu_cfg.clone())));
        }

        Ok(Self {
            config,
            conversation_store,
            inbox_repo,
            outbox_repo,
            dead_letter_repo,
            channel_registry: Arc::new(registry),
            api_client_registry,
            dedup_cache,
            sync_buf_store,
            audit_sink,
            conversation_state_repo,
            send_gate,
            task_supervisor: Arc::new(TaskSupervisor::new()),
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
        self.task_supervisor.spawn("gc_janitor", async move {
            gc_janitor::gc_janitor(gc_store.as_ref(), gc_timeout, gc_interval).await;
        });

        let mut inbound_rx = self.channel_registry.start_all().await?;
        let inbox = self.inbox_repo.clone();
        let dedup = self.dedup_cache.clone();
        let queue = self.conversation_store.clone();
        self.task_supervisor.spawn("inbound_router", async move {
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
        self.task_supervisor.spawn("outbox_worker", async move {
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
                // Context tokens are short-lived but not second-level. Using an
                // aggressive 25s cutoff causes false "window expired" prechecks.
                const MAX_TOKEN_AGE_SECS: u64 = 25 * 60;

                self.stale
                    || self.observed_at.elapsed() >= std::time::Duration::from_secs(MAX_TOKEN_AGE_SECS)
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

            fn observed_age_secs(&self) -> u64 {
                self.observed_at.elapsed().as_secs()
            }

            fn last_success_age_secs(&self) -> Option<u64> {
                self.last_success_at.map(|instant| instant.elapsed().as_secs())
            }
        }

        let context_token_store = Arc::new(
            SqliteContextTokenStore::open(&self.config.db_path)
                .map_err(|e| format!("failed to open context token store: {}", e))?,
        );

        let persisted_context_tokens = load_persisted_context_tokens(
            context_token_store.as_ref(),
            &self.config.wechat.account_id,
        )
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to load persisted context tokens");
            HashMap::new()
        });

        let mut initial_token_cache = HashMap::new();
        for (peer_id, token) in persisted_context_tokens {
            if !token.trim().is_empty() {
                initial_token_cache.insert(peer_id, PeerTokenState::new(token));
            }
        }

        #[derive(Clone)]
        struct HttpApiState {
            wechat: crate::infrastructure::config::WeChatConfig,
            feishu_configs: Vec<crate::infrastructure::config::FeishuConfig>,
            sync_buf_store: Arc<SqliteSyncBufStore>,
            context_token_store: Arc<dyn ContextTokenStore>,
            token_cache: Arc<tokio::sync::Mutex<std::collections::HashMap<String, PeerTokenState>>>,
            send_peer_locks: Arc<tokio::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
            send_min_interval_ms: u64,
            send_debug_enabled: bool,
            inbox_repo: Arc<SqliteInboxRepo>,
            dedup_cache: Arc<MokaDedupCache>,
            conversation_store: Arc<ConversationStore>,
            logger: Arc<DailyLogger>,
            audit_sink: Arc<SqliteAuditSink>,
            task_supervisor: Arc<TaskSupervisor>,
        }

        #[derive(Clone, Deserialize)]
        struct SendRequest {
            to: String,
            text: String,
            #[serde(default)]
            context_token: Option<String>,
        }

        #[derive(Serialize)]
        struct SendDiagnostics {
            token_cache_hit: bool,
            request_token_supplied: bool,
            lock_wait_ms: u128,
            peer_fingerprint: String,
            text_len: usize,
            #[serde(skip_serializing_if = "Option::is_none")]
            context_token_len: Option<usize>,
            #[serde(skip_serializing_if = "Option::is_none")]
            ret: Option<i64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            errcode: Option<i64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            server_id: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            returned_token_prefix: Option<String>,
        }

        #[derive(Serialize)]
        struct SendResponse {
            ok: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            context_token: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            error: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            diagnostics: Option<SendDiagnostics>,
        }

        #[derive(Serialize)]
        struct FeishuWebhookResponse {
            ok: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            challenge: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            duplicate: Option<bool>,
            #[serde(skip_serializing_if = "Option::is_none")]
            ignored: Option<bool>,
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

        #[derive(Serialize)]
        struct TokenStatusEntry {
            peer_id: String,
            token_age_secs: u64,
            last_update_ts_secs: i64,
            is_stale: bool,
        }

        #[derive(Serialize)]
        struct TokenStatusResponse {
            ok: bool,
            tokens: Vec<TokenStatusEntry>,
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

            fn env_flag_enabled(name: &str) -> bool {
                std::env::var(name)
                .ok()
                .map(|v| v.trim().to_ascii_lowercase())
                .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
                .unwrap_or(false)
            }

            fn env_u64(name: &str, default_value: u64) -> u64 {
                std::env::var(name)
                    .ok()
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .unwrap_or(default_value)
            }

            fn is_ret_minus_2_error(err: &str) -> bool {
                err.contains("ret=-2") || err.contains("ret=-2,") || err.contains("ret=-2 ")
            }

        let log_dir = std::path::Path::new(&self.config.db_path)
            .parent()
            .map(|p| p.join("logs"))
            .unwrap_or_else(|| std::path::PathBuf::from("logs"));
        let logger = Arc::new(DailyLogger::new(&log_dir)
            .unwrap_or_else(|_| DailyLogger::new("logs").unwrap()));

        let state = Arc::new(HttpApiState {
            wechat: self.config.wechat.clone(),
            feishu_configs: {
                let mut all = Vec::new();
                all.push(self.config.feishu.clone());
                all.extend(self.config.feishu_accounts.clone());
                all
            },
            sync_buf_store: self.sync_buf_store.clone(),
            context_token_store: context_token_store.clone(),
            token_cache: Arc::new(tokio::sync::Mutex::new(initial_token_cache)),
            send_peer_locks: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            send_min_interval_ms: env_u64("MAGICLAW_WECHAT_SEND_MIN_INTERVAL_MS", 500),
            send_debug_enabled: env_flag_enabled("MAGICLAW_API_SEND_DEBUG"),
            inbox_repo: self.inbox_repo.clone(),
            dedup_cache: self.dedup_cache.clone(),
            conversation_store: self.conversation_store.clone(),
            logger,
            audit_sink: self.audit_sink.clone(),
            task_supervisor: self.task_supervisor.clone(),
        });

        // Start a background long-poll loop to keep context_token hot in memory.
        // This continuously consumes getupdates and updates per-user token cache,
        // so /api/send can avoid token-expiry gaps during continuous sending.
        if state.wechat.enabled && !state.wechat.token.trim().is_empty() {
            let poll_state = state.clone();
            self.task_supervisor.spawn("wechat_token_poller", async move {
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
                                        drop(cache);
                                        if let Err(e) = persist_context_token(
                                            poll_state.context_token_store.as_ref(),
                                            &poll_state.wechat.account_id,
                                            uid,
                                            &token,
                                        ) {
                                            tracing::warn!(peer_id = %uid, error = %e, "failed to persist refreshed context token");
                                        }
                                        poll_state.logger.log_token_refresh(uid, 0, "long-poll");
                                        poll_state.audit_sink.record(None, "token_refresh", &format!(
                                            "source=long-poll,peer_id={},trigger=inbound",
                                            uid
                                        ));
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

                            {
                                let mut cache = poll_state.token_cache.lock().await;
                                cache.clear();
                            }
                            if let Err(e) = clear_persisted_context_tokens(
                                poll_state.context_token_store.as_ref(),
                                &poll_state.wechat.account_id,
                            ) {
                                tracing::warn!(error = %e, "failed to clear persisted context tokens on session expiry");
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

        let auth = Arc::new(HttpAuth::new(self.api_client_registry.clone()));

        let app: Router = Router::new()
            .route(
                "/api/send",
                post(
                    |State(state): State<Arc<HttpApiState>>, Json(req): Json<SendRequest>| async move {
                        // Same peer must be serialized to avoid parallel sends racing on one context_token.
                        let lock_wait_started = std::time::Instant::now();
                        let peer_lock = {
                            let mut locks = state.send_peer_locks.lock().await;
                            locks
                                .entry(req.to.clone())
                                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                                .clone()
                        };
                        let _peer_guard = peer_lock.lock().await;
                        let lock_wait_ms = lock_wait_started.elapsed().as_millis();

                        let request_token_supplied = req
                            .context_token
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .is_some();
                        let mut selected_ctx = req
                            .context_token
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string());
                        let cache_entry = {
                            let cache = state.token_cache.lock().await;
                            cache.get(&req.to).cloned()
                        };

                        if let Some(entry) = cache_entry.as_ref() {
                            selected_ctx = Some(entry.token.clone());
                        }
                        let token_cache_hit = cache_entry.is_some();

                        // Pace same-peer sends to reduce upstream ret=-2 bursts under high density.
                        if let Some(entry) = cache_entry.as_ref() {
                            if let Some(last_success_at) = entry.last_success_at {
                                let elapsed = last_success_at.elapsed();
                                let min_interval = std::time::Duration::from_millis(state.send_min_interval_ms);
                                if elapsed < min_interval {
                                    tokio::time::sleep(min_interval - elapsed).await;
                                }
                            }
                        }

                        let client = reqwest::Client::new();

                        if selected_ctx.is_none() {
                            selected_ctx = req
                                .context_token
                                .as_deref()
                                .map(str::trim)
                                .filter(|token| !token.is_empty())
                                .map(|token| token.to_string());
                        }

                        if let Some(ctx) = selected_ctx {
                            let text_len = req.text.chars().count();
                            let context_token_len = Some(ctx.chars().count());
                            let peer_fingerprint = {
                                let mut h = std::collections::hash_map::DefaultHasher::new();
                                req.to.hash(&mut h);
                                req.text.hash(&mut h);
                                context_token_len.hash(&mut h);
                                format!("{:016x}", h.finish())
                            };

                            let build_cfg = |context_token: String| ILinkSendConfig {
                                base_url: state.wechat.base_url.clone(),
                                token: state.wechat.token.clone(),
                                from_user_id: state.wechat.account_id.clone(),
                                context_token,
                                channel_version: state.wechat.channel_version.clone(),
                                timeout_ms: state.wechat.timeout_ms,
                                keepalive_timeout_ms: state.wechat.keepalive_timeout_ms,
                            };

                            // Single attempt with current token. If upstream rejects it with
                            // ret=-2, require a fresh inbound message to refresh token.
                            let send_outcome = send_text_via_ilink(&client, &build_cfg(ctx.clone()), &req.to, &req.text).await;

                            match send_outcome {
                                Ok(resp) => {
                                    let ret = resp.get("ret").and_then(|v| v.as_i64()).unwrap_or_default();
                                    let errcode = resp.get("errcode").and_then(|v| v.as_i64()).unwrap_or_default();
                                    let server_id = resp
                                        .get("msg")
                                        .and_then(|v| v.get("server_id"))
                                        .and_then(|v| v.as_str())
                                        .map(|v| v.to_string())
                                        .or_else(|| {
                                            resp.get("server_id")
                                                .and_then(|v| v.as_str())
                                                .map(|v| v.to_string())
                                        });
                                    let next_ctx = extract_context_token_from_send_response(&resp);
                                    let response_context_token =
                                        next_ctx.clone().unwrap_or_else(|| ctx.clone());
                                    let (observed_age_secs, send_count, stale, token_to_persist) = {
                                        let mut cache = state.token_cache.lock().await;
                                        let entry = cache
                                            .entry(req.to.clone())
                                            .or_insert_with(|| PeerTokenState::new(ctx.clone()));
                                        entry.mark_success(next_ctx);
                                        (
                                            entry.observed_age_secs(),
                                            entry.send_count,
                                            entry.stale,
                                            entry.token.clone(),
                                        )
                                    };
                                    if let Err(e) = persist_context_token(
                                        state.context_token_store.as_ref(),
                                        &state.wechat.account_id,
                                        &req.to,
                                        &token_to_persist,
                                    ) {
                                        tracing::warn!(peer_id = %req.to, error = %e, "failed to persist context token after send");
                                    }
                                    {
                                        let returned_token_len = response_context_token.chars().count();
                                        let returned_token_prefix = response_context_token.chars().take(16).collect::<String>();
                                        tracing::info!(
                                            peer_id = %req.to,
                                            token_cache_hit = token_cache_hit,
                                            request_token_supplied = request_token_supplied,
                                            peer_fingerprint = %peer_fingerprint,
                                            text_len = text_len,
                                            context_token_len = context_token_len,
                                            ret = ret,
                                            errcode = errcode,
                                            server_id = ?server_id,
                                            observed_age_secs = observed_age_secs,
                                            send_count = send_count,
                                            stale = stale,
                                            returned_token_len = returned_token_len,
                                            returned_token_prefix = %returned_token_prefix,
                                            "wechat send OK + returned context_token"
                                        );
                                    }
                                    return (
                                        StatusCode::OK,
                                        Json(SendResponse {
                                            ok: true,
                                            context_token: Some(response_context_token.clone()),
                                            error: None,
                                            diagnostics: state.send_debug_enabled.then_some(SendDiagnostics {
                                                token_cache_hit,
                                                request_token_supplied,
                                                lock_wait_ms,
                                                peer_fingerprint: peer_fingerprint.clone(),
                                                text_len,
                                                context_token_len,
                                                ret: Some(ret),
                                                errcode: Some(errcode),
                                                server_id,
                                                returned_token_prefix: Some(response_context_token.chars().take(16).collect::<String>()),
                                            }),
                                        }),
                                    );
                                }
                                Err(e) => {
                                    if is_ret_minus_2_error(&e) {
                                        let mut cache = state.token_cache.lock().await;
                                        cache.remove(&req.to);
                                        drop(cache);
                                        if let Err(del_err) = state
                                            .context_token_store
                                            .delete(&state.wechat.account_id, &req.to)
                                        {
                                            tracing::warn!(peer_id = %req.to, error = %del_err, "failed to delete invalid context token after ret=-2");
                                        }
                                        let err_text = "upstream rejected context_token (ret=-2); wait for a new inbound user message, then retry".to_string();
                                        tracing::warn!(
                                            peer_id = %req.to,
                                            token_cache_hit = token_cache_hit,
                                            request_token_supplied = request_token_supplied,
                                            peer_fingerprint = %peer_fingerprint,
                                            text_len = text_len,
                                            context_token_len = context_token_len,
                                            error = %e,
                                            "wechat send diagnostics"
                                        );
                                        return (
                                            StatusCode::PRECONDITION_FAILED,
                                            Json(SendResponse {
                                                ok: false,
                                                context_token: None,
                                                error: Some(err_text),
                                                diagnostics: state.send_debug_enabled.then_some(SendDiagnostics {
                                                    token_cache_hit,
                                                    request_token_supplied,
                                                    lock_wait_ms,
                                                    peer_fingerprint,
                                                    text_len,
                                                    context_token_len,
                                                    ret: Some(-2),
                                                    errcode: Some(0),
                                                    server_id: None,
                                                    returned_token_prefix: None,
                                                }),
                                            }),
                                        );
                                    }
                                    tracing::warn!(
                                        peer_id = %req.to,
                                        token_cache_hit = token_cache_hit,
                                        request_token_supplied = request_token_supplied,
                                        peer_fingerprint = %peer_fingerprint,
                                        text_len = text_len,
                                        context_token_len = context_token_len,
                                        error = %e,
                                        "wechat send diagnostics"
                                    );
                                    return (
                                        StatusCode::BAD_GATEWAY,
                                        Json(SendResponse {
                                            ok: false,
                                            context_token: None,
                                            error: Some(e),
                                            diagnostics: state.send_debug_enabled.then_some(SendDiagnostics {
                                                token_cache_hit,
                                                request_token_supplied,
                                                lock_wait_ms,
                                                peer_fingerprint,
                                                text_len,
                                                context_token_len,
                                                ret: None,
                                                errcode: None,
                                                server_id: None,
                                                returned_token_prefix: None,
                                            }),
                                        }),
                                    );
                                }
                            }
                        }

                        (
                            StatusCode::PRECONDITION_FAILED,
                            Json(SendResponse {
                                ok: false,
                                context_token: None,
                                error: Some(
                                    "no valid context_token for peer; wait for inbound user message then retry"
                                        .to_string(),
                                ),
                                diagnostics: state.send_debug_enabled.then_some(SendDiagnostics {
                                    token_cache_hit,
                                    request_token_supplied,
                                    lock_wait_ms,
                                    peer_fingerprint: {
                                        let mut h = std::collections::hash_map::DefaultHasher::new();
                                        req.to.hash(&mut h);
                                        req.text.hash(&mut h);
                                        None::<usize>.hash(&mut h);
                                        format!("{:016x}", h.finish())
                                    },
                                    text_len: req.text.chars().count(),
                                    context_token_len: None,
                                    ret: None,
                                    errcode: None,
                                    server_id: None,
                                    returned_token_prefix: None,
                                }),
                            }),
                        )
                    },
                ),
            )
            .route(
                "/api/token_status",
                get(|State(state): State<Arc<HttpApiState>>| async move {
                    let cache = state.token_cache.lock().await;
                    let now = chrono::Utc::now().timestamp();
                    let mut tokens: Vec<TokenStatusEntry> = cache
                        .iter()
                        .map(|(peer_id, entry)| TokenStatusEntry {
                            peer_id: peer_id.clone(),
                            token_age_secs: entry.observed_age_secs(),
                            last_update_ts_secs: now,
                            is_stale: entry.stale,
                        })
                        .collect();
                    tokens.sort_by(|left, right| left.peer_id.cmp(&right.peer_id));

                    (
                        StatusCode::OK,
                        Json(TokenStatusResponse {
                            ok: true,
                            tokens,
                        }),
                    )
                }),
            )
            .route(
                "/api/health",
                get(|State(state): State<Arc<HttpApiState>>| async move {
                    // Task 2: Health endpoint includes Feishu configuration and webhook verification status
                    let mut feishu_detail = serde_json::json!({
                        "enabled": false,
                        "accounts": []
                    });
                    
                    let mut enabled_count = 0;
                    let mut feishu_accounts = vec![];
                    
                    for cfg in &state.feishu_configs {
                        if cfg.enabled {
                            enabled_count += 1;
                            let has_webhook_verification = !cfg.verification_token.trim().is_empty() && !cfg.signing_secret.trim().is_empty();
                            let has_app_auth = !cfg.app_id.trim().is_empty() && !cfg.app_secret.trim().is_empty();
                            let has_preissued_token = !cfg.tenant_access_token.trim().is_empty();
                            
                            feishu_accounts.push(serde_json::json!({
                                "account_id": cfg.account_id,
                                "receive_id_type": cfg.receive_id_type,
                                "webhook_verified": has_webhook_verification,
                                "auth_method": if has_app_auth { "app_credentials" } else if has_preissued_token { "preissued_token" } else { "none" }
                            }));
                        }
                    }
                    
                    if enabled_count > 0 {
                        feishu_detail = serde_json::json!({
                            "enabled": true,
                            "accounts": feishu_accounts,
                            "account_count": enabled_count
                        });
                    }

                    // Task 9: include background task status from the supervisor
                    // so operators can detect panics in GC/outbox/HTTP/etc.
                    let finished = state.task_supervisor.poll_status();
                    let running = state.task_supervisor.running_names();
                    let tasks = serde_json::json!({
                        "running": running,
                        "finished_count": finished.len(),
                        "finished": finished.iter().map(|t| serde_json::json!({
                            "name": t.name,
                            "state": match t.state {
                                TaskState::Running => "running",
                                TaskState::Completed => "completed",
                                TaskState::Failed(_) => "failed",
                            }
                        })).collect::<Vec<_>>()
                    });

                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "ok": true,
                            "feishu": feishu_detail,
                            "tasks": tasks
                        }))
                    )
                }),
            )
            .route(
                "/api/feishu/webhook",
                post(
                    |State(state): State<Arc<HttpApiState>>,
                     headers: axum::http::HeaderMap,
                     body: axum::body::Bytes| async move {
                        let payload: serde_json::Value = match serde_json::from_slice(body.as_ref()) {
                            Ok(v) => v,
                            Err(e) => {
                                return (
                                    StatusCode::BAD_REQUEST,
                                    Json(FeishuWebhookResponse {
                                        ok: false,
                                        challenge: None,
                                        duplicate: None,
                                        ignored: None,
                                        error: Some(format!("invalid feishu webhook JSON: {}", e)),
                                    }),
                                );
                            }
                        };

                        let Some(feishu_cfg) = select_feishu_config(&state.feishu_configs, &payload) else {
                            return (
                                StatusCode::BAD_REQUEST,
                                Json(FeishuWebhookResponse {
                                    ok: false,
                                    challenge: None,
                                    duplicate: None,
                                    ignored: None,
                                    error: Some("no matching feishu account config for webhook".to_string()),
                                }),
                            );
                        };

                        if let Err(e) = verify_webhook_signature(&headers, body.as_ref(), feishu_cfg) {
                            return (
                                StatusCode::UNAUTHORIZED,
                                Json(FeishuWebhookResponse {
                                    ok: false,
                                    challenge: None,
                                    duplicate: None,
                                    ignored: None,
                                    error: Some(e),
                                }),
                            );
                        }

                        let dispatch = match parse_webhook_event(payload, feishu_cfg) {
                            Ok(v) => v,
                            Err(e) => {
                                return (
                                    StatusCode::UNAUTHORIZED,
                                    Json(FeishuWebhookResponse {
                                        ok: false,
                                        challenge: None,
                                        duplicate: None,
                                        ignored: None,
                                        error: Some(e),
                                    }),
                                );
                            }
                        };

                        match dispatch {
                            FeishuWebhookDispatch::UrlVerification { challenge } => (
                                StatusCode::OK,
                                Json(FeishuWebhookResponse {
                                    ok: true,
                                    challenge: Some(challenge),
                                    duplicate: None,
                                    ignored: None,
                                    error: None,
                                }),
                            ),
                            FeishuWebhookDispatch::Ignore => (
                                StatusCode::OK,
                                Json(FeishuWebhookResponse {
                                    ok: true,
                                    challenge: None,
                                    duplicate: None,
                                    ignored: Some(true),
                                    error: None,
                                }),
                            ),
                            FeishuWebhookDispatch::Message(msg) => {
                                let dedup_message_id = msg.id.clone();
                                match inbox_processor::process_inbound(state.inbox_repo.as_ref(), &msg) {
                                    Ok(inbox_processor::InboxResult::Duplicate) => (
                                        StatusCode::OK,
                                        Json(FeishuWebhookResponse {
                                            ok: true,
                                            challenge: None,
                                            duplicate: Some(true),
                                            ignored: None,
                                            error: None,
                                        }),
                                    ),
                                    Ok(inbox_processor::InboxResult::Processed) => {
                                        let route_outcome = route_message::route_message(
                                            state.dedup_cache.as_ref(),
                                            state.conversation_store.as_ref(),
                                            msg,
                                        );
                                        match route_outcome {
                                            RouteOutcome::Enqueued => (
                                                StatusCode::OK,
                                                Json(FeishuWebhookResponse {
                                                    ok: true,
                                                    challenge: None,
                                                    duplicate: Some(false),
                                                    ignored: None,
                                                    error: None,
                                                }),
                                            ),
                                            RouteOutcome::Duplicate => (
                                                StatusCode::OK,
                                                Json(FeishuWebhookResponse {
                                                    ok: true,
                                                    challenge: None,
                                                    duplicate: Some(true),
                                                    ignored: None,
                                                    error: None,
                                                }),
                                            ),
                                            RouteOutcome::Dropped(message_id) => (
                                                StatusCode::SERVICE_UNAVAILABLE,
                                                Json(FeishuWebhookResponse {
                                                    ok: false,
                                                    challenge: None,
                                                    duplicate: Some(false),
                                                    ignored: None,
                                                    error: Some(format!(
                                                        "feishu route queue full, dropped message_id={}",
                                                        message_id
                                                    )),
                                                }),
                                            ),
                                        }
                                    }
                                    Err(RepoError::Db(e)) => (
                                        StatusCode::INTERNAL_SERVER_ERROR,
                                        Json(FeishuWebhookResponse {
                                            ok: false,
                                            challenge: None,
                                            duplicate: None,
                                            ignored: None,
                                            error: Some(format!(
                                                "failed to persist feishu inbound message {}: {}",
                                                dedup_message_id, e
                                            )),
                                        }),
                                    ),
                                    Err(RepoError::NotFound(e)) => (
                                        StatusCode::NOT_FOUND,
                                        Json(FeishuWebhookResponse {
                                            ok: false,
                                            challenge: None,
                                            duplicate: None,
                                            ignored: None,
                                            error: Some(e),
                                        }),
                                    ),
                                }
                            }
                        }
                    },
                ),
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
            .with_state(state.clone())
            .layer(from_fn_with_state(
                auth.clone(),
                require_bearer_auth,
            ));

        let addr: std::net::SocketAddr = addr
            .parse()
            .map_err(|e| format!("invalid HTTP API address '{}': {}", addr, e))?;

        self.task_supervisor.spawn("http_api", async move {
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