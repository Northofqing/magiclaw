# magiclaw 项目设计文档（综合版）

> **版本**: v6.0 (综合整合版)
> **最后更新**: 2026-06-28
> **范围**: 整合 RUST_MIGRATION_V5 + Phase 1-5 架构 + 各模块设计 + 实施报告 + 当前状态
> **状态**: 单一可信源（Single Source of Truth）
> **配套文档**: `2026-06-28-integration-index.md`、`2026-06-28-development-status.md`、`2026-06-28-optimization-backlog.md`、`2026-06-28-bugs.md`

---

## 目录

- [第 1 章 项目定位与核心原则](#第-1-章-项目定位与核心原则)
- [第 2 章 架构总览](#第-2-章-架构总览)
- [第 3 章 领域模型](#第-3-章-领域模型)
- [第 4 章 核心机制](#第-4-章-核心机制)
- [第 5 章 持久化与可恢复投递](#第-5-章-持久化与可恢复投递)
- [第 6 章 消息信道](#第-6-章-消息信道)
- [第 7 章 管道与中间件](#第-7-章-管道与中间件)
- [第 8 章 AI 后端系统](#第-8-章-ai-后端系统)
- [第 9 章 鉴权与安全](#第-9-章-鉴权与安全)
- [第 10 章 韧性与隔离](#第-10-章-韧性与隔离)
- [第 11 章 媒体流式上传](#第-11-章-媒体流式上传)
- [第 12 章 多项目推送](#第-12-章-多项目推送)
- [第 13 章 适配层（Adapter）](#第-13-章-适配层adapter)
- [第 14 章 可观测性与审计](#第-14-章-可观测性与审计)
- [第 15 章 部署与运行模式](#第-15-章-部署与运行模式)
- [第 16 章 数据持久化 Schema](#第-16-章-数据持久化-schema)
- [第 17 章 迁移阶段计划](#第-17-章-迁移阶段计划)
- [第 18 章 强制红线（MUST）](#第-18-章-强制红线must)
- [第 19 章 失败模式与恢复](#第-19-章-失败模式与恢复)
- [第 20 章 已知 Bug 与优化空间](#第-20-章-已知-bug-与优化空间)

---

## 第 1 章 项目定位与核心原则

### 1.1 项目定位

**magiclaw** — 信道中心架构系统，基于 Rust 的多平台消息中枢。

**核心目标**: 把 WeChat / Dingtalk / Feishu 等异构信道统一接入一个**可恢复、可审计、可扩展**的消息核心，并提供 CLI / MCP / HTTP / 推送四种接入方式。

### 1.2 核心原则

| 原则 | 实现 |
|------|------|
| **同 RouteKey 串行，跨 RouteKey 并行** | `ConversationStore` 用 `mpsc::channel` per-route |
| **AI 是可选能力，不依赖 Agent** | 默认 echo 后端，系统不依赖任何 agent |
| **消息可恢复投递** | Inbox → Outbox → DLQ 三段式，SQLite 持久化 + 崩溃恢复 |
| **审计不可篡改** | SHA-256 链式 hash，启动时校验完整性 |
| **MCP 是 Adapter，不是核心** | 不污染核心业务模型 |
| **路径级多账号隔离** | channel/account 维度命名空间 |
| **信道稳定性与顺序性优先** | 重于吞吐量 |
| **契约测试覆盖协议** | ilink / Feishu 等需要 contract test |
| **业务日志只走 stderr/file** | stdout 仅协议输出 |
| **失败优雅降级** | AI 失败 → echo 降级，主链路不中断 |

### 1.3 非目标

- 不实现完整后台管理 UI
- 不做外部项目主数据托管
- 不引入分布式消息队列
- 不实现多轮对话记忆/工具调用编排（独立阶段）
- 不实现桌面版 Claude 调用（仅 CLI）

---

## 第 2 章 架构总览

### 2.1 分层架构（DDD + Hexagonal）

```
┌─────────────────────────────────────────────────────────────────┐
│  Adapter Layer（适配层）                                          │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌────────────────┐    │
│  │ MCP stdio│  │ HTTP API │  │ CLI      │  │ Push / Binding │    │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────────┬───────┘    │
└───────┼─────────────┼─────────────┼────────────────┼────────────┘
        │             │             │                │
        ▼             ▼             ▼                ▼
┌─────────────────────────────────────────────────────────────────┐
│  Pipeline Layer（管道层，可插拔中间件）                              │
│  Normalize → Permission → RateLimit → AgentCommand → AI → Outbox │
│  (Chain of Responsibility + Decorator + short-circuit)           │
└─────────────────────────────────────────────────────────────────┘
        │
        ▼
┌─────────────────────────────────────────────────────────────────┐
│  Message Core（信道稳定核心）                                       │
│  ┌─────────────────┐  ┌──────────────┐  ┌──────────────────┐      │
│  │ ConversationStore│  │ InboxProcessor│  │ OutboxWorker     │      │
│  │  (RouteKey-routed│  │  (dedup Moka) │  │  (retry + DLQ)   │      │
│  │   per-worker)    │  │               │  │                  │      │
│  └─────────────────┘  └──────────────┘  └──────────────────┘      │
│                                                                  │
│  Domain: Message, Conversation, ConversationSnapshot,             │
│          ChannelError, PipelineError, AiError                    │
└─────────────────────────────────────────────────────────────────┘
        │             │             │
        ▼             ▼             ▼
┌─────────────────────────────────────────────────────────────────┐
│  Channels（Channel trait + port）                                 │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐                         │
│  │ WeChat   │  │ Feishu   │  │ Dingtalk │                         │
│  │  (ilink) │  │  (OpenAPI│  │  (OpenAPI│                         │
│  │          │  │  + webhook│  │  + token)│                         │
│  └──────────┘  └──────────┘  └──────────┘                         │
└─────────────────────────────────────────────────────────────────┘
        │
        ▼
┌─────────────────────────────────────────────────────────────────┐
│  Infrastructure                                                  │
│  SQLite (DbPool with N connections + WAL) + TaskSupervisor        │
│  + Circuit Breaker + Bulkhead (ResilienceGate) + Audit Chain     │
└─────────────────────────────────────────────────────────────────┘
```

### 2.2 模块结构

```
src/
├── adapters/              # 适配层（SQLite 实现、HTTP auth、ConversationStore 等）
│   ├── sqlite_*.rs        # SQLite 实现（inbox/outbox/dlq/audit/sync_buf/context_tokens/...）
│   ├── api_client_registry.rs # 动态 API 鉴权
│   ├── conversation_store.rs  # 内存 RouteKey 路由 + per-worker
│   ├── http_auth.rs       # Bearer auth 中间件
│   ├── moka_dedup.rs      # Moka TTL 去重
│   └── ...
├── application/           # 应用服务（编排、worker、recovery）
│   ├── outbox_worker.rs   # 可恢复投递 + DLQ 重放
│   ├── crash_recovery.rs  # 启动时重置 sending/retrying
│   ├── gc_janitor.rs      # 30 分钟空闲回收
│   ├── audit.rs           # AuditRecord + 查询 API
│   ├── agent_preferences.rs # per-user AI 偏好
│   ├── push.rs / binding.rs # 多项目推送 + 投递目标
│   └── ...
├── channels/              # 信道实现（trait: Channel）
│   ├── wechat/            # ilink 长轮询 + token 管理
│   ├── feishu/            # OpenAPI + webhook 验证 + 错误语义
│   └── dingtalk/          # access_token + 消息类型映射
├── core/                  # 核心能力
│   ├── pipeline/          # 中间件链
│   ├── ai/                # 后端抽象 + echo/claude_code/codex/copilot/custom
│   └── resilience/        # CircuitBreaker + Bulkhead + ResilienceGate
├── domain/                # 领域模型（无基础设施依赖）
│   ├── entities/          # Message
│   ├── aggregates/        # Conversation
│   ├── value_objects/     # RouteKey, ConversationSnapshot, MessageContent, MediaMeta
│   ├── services/          # ReorderWindow, audit_chain
│   ├── ports/             # trait 抽象（AuditQuery, AuditSink, UserPreferenceStore 等）
│   └── error.rs           # ChannelError, PipelineError, AiError
├── infrastructure/        # runtime / config / db / tracing
│   ├── runtime.rs         # AppRuntime 装配 + 启动后台任务
│   ├── db.rs              # DbPool（连接池 + Condvar）+ schema 初始化
│   ├── task_supervisor.rs # JoinSet 包装的后台任务管理
│   └── config.rs          # AppConfig
├── cli/                   # CLI 解析与命令
├── daemon/                # daemon 模式 + 单例锁
└── main.rs                # 入口
```

### 2.3 依赖方向

- `domain/` 不依赖任何外部 crate（除 std + 基础类型）
- `application/` 只依赖 `domain/` + port traits
- `adapters/` 依赖 `domain/` port traits + 具体实现库（moka, rusqlite）
- `infrastructure/` 负责组装、启动、配置

**Core 永不依赖 Agent 或 Adapter**（红线 2.1）。

---

## 第 3 章 领域模型

### 3.1 核心实体与值对象

#### 3.1.1 RouteKey（值对象，不可变）

```rust
pub enum ConversationType {
    Direct,
    Group,
    Thread,
    BotSession,
}

pub struct RouteKey {
    pub channel: ChannelId,              // wechat / dingtalk / feishu
    pub conversation_id: String,         // 平台会话 ID（主路由键）
    pub peer_id: String,                 // 用户或群标识
    pub conversation_type: ConversationType,
}
```

**升级说明**: RouteKey 从 `channel + peer_id` 升级为 `channel + conversation_id + peer_id + conversation_type`，因为多平台场景里真正稳定的处理单位是会话而非单 peer。

#### 3.1.2 Conversation（一等聚合根）

```rust
pub struct Conversation {
    pub route_key: RouteKey,              // 聚合标识
    pub participants: Vec<String>,
    pub state: ConversationState,
    pub reorder_window: ReorderWindow,    // 聚合拥有的乱序窗口
    pub last_active: Instant,
    pub created_at: DateTime,
}

pub enum ConversationState {
    Active,
    Idle,      // >30min 无消息 → GC 候选
    Closed,
}
```

**聚合不变量**:
- 同一 RouteKey 的所有 Message **必须**串行处理
- ReorderWindow 的生命周期 = Conversation 的生命周期
- Conversation 进入 Idle 状态后，由 GC Janitor 回收其运行时投影
- 聚合内的 Message 处理顺序由 ReorderWindow 保证

#### 3.1.3 Message（实体）

```rust
pub struct Message {
    pub id: String,                    // 平台消息 ID（全局唯一）
    pub route_key: RouteKey,
    pub sequence: Option<i64>,         // 平台提供时使用
    pub timestamp_ms: i64,
    pub direction: Direction,
    pub content: MessageContent,
    pub audit_mark: Option<AuditMark>, // 迟到/重复等审计标记
}

pub enum Direction { Inbound, Outbound }

pub enum AuditMark {
    LateArrival { delay_ms: u64 },
    Duplicate,
    OutOfOrder { expected_seq: i64, actual_seq: i64 },
}

pub enum MessageContent {
    Text(String),
    Image { url: String, media_id: Option<String> },
    File { url: String, name: String, size: u64 },
    Unknown,
}
```

#### 3.1.4 ConversationSnapshot（只读值对象，用于 Pipeline）

```rust
pub struct ConversationSnapshot {
    pub route_key: RouteKey,
    pub conversation_id: String,
    pub peer_id: String,
    pub conversation_type: ConversationType,
    pub message_count: u64,
    pub last_active_at: i64,
}
```

**用途**: 替代 PipelineContext 中的 Conversation clone（解决 B2 聚合边界破坏问题）。

### 3.2 领域服务

#### 3.2.1 ReorderWindow（由 Conversation 聚合拥有）

```rust
pub struct ReorderWindow {
    buffer: BTreeMap<i64, Message>,  // key = sequence or timestamp_ms
    window_ms: u64,                 // 默认 200ms
    last_flushed: Option<Instant>,
}

impl ReorderWindow {
    /// 插入消息，返回已排序可投递的批次
    pub fn insert(&mut self, msg: Message, now: Instant) -> Vec<Message>;
    /// 超窗口迟到消息标记 audit 后放行
    pub fn handle_late(&mut self, msg: Message) -> LateMessageAction;
    /// GC 时强制刷出所有 buffer 中的消息
    pub fn flush_all(&mut self) -> Vec<Message>;
}
```

**排序策略矩阵**:

| 平台 | 排序依据 | window_ms | 迟到处理 |
|------|---------|-----------|----------|
| WeChat (webhook) | timestamp | 200 | 幂等 + audit |
| WeChat (sync) | sync_buf seq | N/A | 严格按序 |
| Dingtalk | timestamp | 200 | 幂等 + audit |
| Feishu | timestamp | 200 | 幂等 + audit |

**截止时间**: 应使用挂钟时间（`Instant::now()`），而非消息排序键（修复 B1）。

### 3.3 Port 接口（领域边界）

```rust
/// 去重缓存（Phase 1: moka 实现）
pub trait DedupCache: Send + Sync {
    fn check_and_set(&self, channel: &str, msg_id: &str) -> bool;
}

/// sync_buf 持久化存储
pub trait SyncBufStore: Send + Sync {
    fn save(&self, channel: &str, account: &str, buf: &[u8]) -> Result<()>;
    fn load(&self, channel: &str, account: &str) -> Result<Vec<u8>>;
}

/// 会话处理队列
pub trait ConversationQueue: Send + Sync {
    fn enqueue(&self, key: &RouteKey, msg: Message) -> Result<(), BackpressureError>;
    fn active_conversations(&self) -> usize;
}

/// GC Janitor 抽象
pub trait ConversationGC: Send + Sync {
    fn collect_idle(&self, idle_timeout: Duration) -> Vec<RouteKey>;
}

/// 媒体字节来源
pub trait MediaSource: Send + Sync {
    fn meta(&self) -> &MediaMeta;
    async fn open(&self) -> Result<MediaByteStream, MediaError>;
}

/// 媒体上传
pub trait MediaUploader: Send + Sync {
    async fn upload(&self, source: &dyn MediaSource) -> Result<MediaRef, MediaError>;
}

/// Context Token 持久化
pub trait ContextTokenStore: Send + Sync {
    async fn get(&self, account_id: &str, user_id: &str) -> Result<Option<String>>;
    async fn set(&self, account_id: &str, user_id: &str, token: &str) -> Result<()>;
    async fn delete_all(&self, account_id: &str) -> Result<()>;
}

/// API Client 注册表（动态鉴权）
pub trait ApiClientRegistry: Send + Sync {
    fn lookup_token(&self, token: &str) -> Result<Option<ApiClientRecord>, String>;
    fn is_authorized(&self, token: &str, required_scope: &str) -> bool;
}
```

### 3.4 错误类型

```rust
// src/domain/error.rs
#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("auth expired (errcode={errcode})")]
    AuthExpired { errcode: i32 },
    #[error("rate limited, retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },
    #[error("invalid recipient: {0}")]
    InvalidRecipient(String),
    #[error("content rejected: {0}")]
    ContentRejected(String),
    #[error("session expired: {0}")]
    SessionExpired(String),
    #[error("media error: {0}")]
    Media(String),
}

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("rate limit exceeded")]
    RateLimitExceeded,
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("upstream error: {0}")]
    Upstream(String),
    #[error("terminated: {0}")]
    Terminated(String),
}

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("backend not available: {0}")]
    BackendUnavailable(String),
    #[error("execution timeout")]
    Timeout,
    #[error("circuit open")]
    CircuitOpen,
    #[error("bulkhead full")]
    BulkheadFull,
    #[error("output error: {0}")]
    Output(String),
}
```

> 注: 当前代码中仍多为 `Result<_, String>`，OP-P1-4 正在迁移到类型化错误。

---

## 第 4 章 核心机制

### 4.1 Route Worker 生命周期（Idle GC）

- 每个 RouteKey 仍保持串行 worker
- 维护 `last_active` 和 janitor 扫描任务
- `idle_timeout` 默认 30 分钟
- 空闲 route 自动回收：drop sender → worker 退出 → registry 删除

```rust
struct RouteWorkerHandle {
    tx: mpsc::Sender<Message>,
    last_active: Instant,
}

// janitor 每 60 秒扫描
if now.duration_since(handle.last_active) > Duration::from_secs(1800) {
    remove handle; tx dropped; worker exits on recv None
}
```

**GC 安全流程**:
1. Janitor 检测 `last_active > 30min`
2. 向 Worker 发送 `FlushAndExit` 命令
3. Worker 调用 `ReorderWindow::flush_all()` 处理剩余消息
4. Worker 写最后一条 sync_buf
5. Worker 退出，drop queue receiver
6. Janitor 从 ConversationStore 中删除条目

### 4.2 去重（TTL Cache）

使用 `moka` TTL 缓存（替代全量 retain）：

```rust
let dedup = moka::sync::Cache::builder()
    .time_to_live(Duration::from_secs(300))   // TTL 5 分钟
    .max_capacity(2_000_000)                  // 容量 200 万
    .build();

// key: channel + message_id
fn check_and_set(&self, channel: &str, msg_id: &str) -> bool {
    self.cache.insert(format!("{}:{}", channel, msg_id), ()).is_none()
}
```

**容量规划**: 每条目约 80 bytes，2M 条目 ≈ 160MB；TTL 5 分钟覆盖大多数平台重试窗口。

### 4.3 背压策略

```rust
pub struct BackpressureConfig {
    pub per_route_buffer: usize,         // 默认 256
    pub inbound_channel_capacity: usize, // 默认 4096
}

pub enum BackpressureAction {
    DropOldest,    // 丢弃最旧 + 审计
    DropNewest,    // 丢弃最新 + 审计
    Block,         // 阻塞生产者（仅发送路径）
}
```

**入站背压**（Channel → Router）: channel 满 → Drop Oldest + audit + 指标递增；webhook 场景下丢弃 + HTTP 200（避免无限重试）。

**会话队列背压**（Router → Worker）: 队列满 → Drop Newest + audit + 指标递增（保留旧消息上下文）。

### 4.4 限速（RateLimit）

- 替代单点 `Mutex<VecDeque<Instant>>` 竞争模型
- 推荐 `governor` 或分片滑窗
- 按 channel + route 可配置限速
- 默认策略: wechat 60s/30，其他 channel 可单独覆盖

### 4.5 错误重试与 DLQ

```rust
pub struct RetryConfig {
    pub max_retries: u32,             // default 5
    pub base_backoff_ms: u64,         // default 1000
    pub max_backoff_ms: u64,          // default 60000
    pub jitter: f64,                  // default 0.1
}
// next_retry_delay = min(base * 2^attempt + jitter, max_backoff)
```

---

## 第 5 章 持久化与可恢复投递

### 5.1 发送状态机

```
     ┌─────────┐
     │ pending │  消息写入 Outbox，等待投递
     └────┬────┘
          │ dequeue
          ▼
     ┌─────────┐
     │ sending │  已投递给 Worker，等待发送结果
     └────┬────┘
          │
    ┌─────┴─────┐
    ▼           ▼
┌──────┐   ┌──────────┐
│ sent │   │ retrying │  发送失败，等待重试
└──────┘   └────┬─────┘
                │
          ┌─────┴─────┐
          ▼           ▼
      ┌──────┐   ┌─────────────┐
      │ sent │   │ dead_letter │  重试次数超过阈值
      └──────┘   └──────┬──────┘
                        │ replay command
                        ▼
                   ┌─────────┐
                   │ pending │  手动重放回到队列
                   └─────────┘
```

### 5.2 Inbox / Outbox / DLQ 模型

```rust
pub struct InboxEntry {
    pub id: String,              // message_id
    pub channel: String,
    pub conversation_id: String,
    pub payload: String,         // serialized Message
    pub status: InboxStatus,     // pending/processing/processed
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct OutboxEntry {
    pub id: String,
    pub route_key: String,       // serialized RouteKey (B12 待修复)
    pub payload: String,
    pub status: OutboxStatus,     // pending/sending/sent/retrying/dead_letter
    pub retry_count: u32,
    pub next_retry_at: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct DeadLetterEntry {
    pub id: String,
    pub source: String,          // "outbox"
    pub payload: String,
    pub reason: String,
    pub created_at: i64,
}
```

### 5.3 崩溃恢复

启动时自动调用 `recover_after_crash`：
1. 重置所有 `outbox.status IN ('sending', 'retrying')` 的记录为 `pending`
2. 重新投递这些消息

### 5.4 审计 hash chain（红线 2.6）

```sql
audit_log:
  id INTEGER PRIMARY KEY
  source TEXT
  route_key TEXT
  action TEXT          -- ai_generate, send, project_bind, token_revoke, ...
  result TEXT          -- ok, failed, dropped, ...
  payload TEXT
  prev_hash TEXT       -- 前一条的 entry_hash
  entry_hash TEXT      -- SHA256(id + source + ... + prev_hash)
  created_at INTEGER
```

- 启动时校验 hash chain 完整性，失败则 daemon 拒绝启动
- 保留期 ≥ 5 年（红线 2.6）

### 5.5 WeChat sync_buf 持久化

```sql
sync_buf(
  channel TEXT,
  account TEXT,
  buf BLOB,
  updated_at INTEGER,
  PRIMARY KEY(channel, account)
)
```

- WAL 模式 + `PRAGMA synchronous=NORMAL`
- 每次更新 `INSERT OR REPLACE`
- 重启时从 DB 恢复断点继续

### 5.6 WeChat Context Token（多用户 HashMap）

**问题**: 单一 String token 无法支持多用户场景。

**官方 SDK 对标**: Rust/Node.js/Go/Python 全部使用 `HashMap<UserId, Token>`。

```rust
struct SessionState {
    context_tokens: HashMap<String, String>,  // user_id → token
    sync_buf: String,
}

impl SessionState {
    async fn get_context_token(&self, user_id: &str) -> Option<String>;
    async fn set_context_token(&self, user_id: String, token: String);
    async fn clear_context_tokens(&self);
}
```

**持久化**（新增 B25 待完善）:

```sql
wechat_context_tokens(
  account_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  token TEXT NOT NULL,
  updated_at INTEGER,
  PRIMARY KEY(account_id, user_id)
)
```

**User ID 抽取规则**: USER 消息（`message_type=1`）用 `from_user_id`；BOT 消息用 `to_user_id`。

---

## 第 6 章 消息信道

### 6.1 Channel Trait

```rust
#[async_trait]
pub trait Channel: Send + Sync {
    fn id(&self) -> ChannelId;

    /// 注入入站总线发送端，信道内部自行决定长轮询或 webhook
    async fn start(&self, inbound_tx: mpsc::Sender<Message>) -> Result<()>;

    async fn send_message(&self, to: &str, content: &MessageContent) -> Result<SendReceipt>;

    async fn stop(&self) -> Result<()>;

    async fn health_check(&self) -> Result<HealthStatus>;
}
```

### 6.2 WeChat 信道

**协议**: ilink 长轮询（35s hold）+ webhook

**关键契约**:
- `POST /getupdates`: 长轮询，携带 `get_updates_buf`，返回 `msgs[]` 含 `from_user_id` / `context_token` / `item_list`
- `POST /sendmessage`: 携带 `context_token` + `text`，返回 `{ret, errcode}`
- `-14` errcode = SessionExpired，需清除 credentials + cursor + tokens + tickets
- AES-128-ECB/PKCS7 加解密

**未完成**:
- B24 Typing Service（getConfig + sendTyping）
- B25 完整 Session Reset（仅清 tokens，未清 credentials/cursor）

### 6.3 Feishu 信道

**协议**: webhook + 签名验证（HMAC-SHA256） + OpenAPI

**错误语义** (`error_semantics.rs`):

| HTTP 状态码 | 类别 |
|-------------|------|
| 400, 401, 403, 404, 405 | Terminal（不重试） |
| 429, 5xx, 408 | Retryable（重试） |

| Feishu 错误码 | 类别 |
|---------------|------|
| 1001-1004, 2001-2002 | Terminal |
| 1008, 5001-5002 | Retryable |

**多账号隔离**: `channel_id = "feishu:<account_id>"`，按 channel 字段做路径级隔离。

**当前状态**:
- ✅ Phase A 闭环已落地（webhook 签名 + 多账号 + 错误语义 + 媒体上传）
- ⛔ Phase B long-connection adapter 待实现
- 🟡 Phase C 韧性硬化部分完成

### 6.4 Dingtalk 信道

**协议**: access_token + OpenAPI 消息类型映射

**当前状态**:
- ✅ 单账号 send/health 闭环测试通过
- 🟡 B17 多账号隔离未充分验证

---

## 第 7 章 管道与中间件

### 7.1 中间件链

```rust
pub trait Middleware: Send + Sync {
    fn name(&self) -> &'static str;
    fn is_terminal(&self) -> bool { false }
    async fn process(&self, ctx: PipelineContext) -> Result<PipelineContext, PipelineError>;
}
```

**链路顺序**:

```
Normalize → Permission → RateLimit → AgentCommand → AI → Outbox
```

### 7.2 各中间件职责

| 中间件 | 职责 | 状态 |
|--------|------|------|
| **Normalize** | 规范化消息内容（trim、清洗） | ✅ closed |
| **Permission** | 白名单门控 | 🟡 **占位放行**（B20） |
| **RateLimit** | 频率/成本控制 | ✅ closed（仅非 echo 后端启用） |
| **AgentCommand** | 解析 `cc` / `cx` / `oc` / `h` 等用户切换命令 | ✅ closed |
| **AI** | 调用 `AiBackend::generate()` | ✅ closed |
| **Outbox** | 写入 outbox（pending） | ✅ closed |

### 7.3 PipelineContext

```rust
pub struct PipelineContext {
    pub message: Message,
    pub conversation_snapshot: ConversationSnapshot,  // 替代 Conversation clone（B2）
    pub selected_agent: Option<String>,               // per-user preference
    pub ai_response: Option<String>,
    pub metadata: HashMap<String, String>,
}
```

### 7.4 Agent Command 解析

支持的命令格式：
- `cc` / `/cc` / `claude` / `claude code` → 切换到 `claude_code`
- `cx` / `/cx` / `codex` → 切换到 `codex`
- `oc` / `/oc` / `openclaw` → 切换到 `openclaw`
- `h` / `/h` / `hermes` → 切换到 `hermes`
- `当前 agent` / `/agent` → 查询当前选择

**SwitchAndProcess 模式**: `cc 帮我总结` → 先切换，再用 claude_code 处理。

**持久化**: `user_agent_preferences(channel, account_scope, peer_id)` 表。

---

## 第 8 章 AI 后端系统

### 8.1 后端抽象

```rust
#[async_trait]
pub trait AiBackend: Send + Sync {
    fn name(&self) -> &'static str;
    async fn generate(&self, input: &str, context: &AiContext) -> Result<String, AiError>;
}
```

### 8.2 内置后端

| 后端 | 类型 | 调用形态 | 状态 |
|------|------|---------|------|
| `echo` | 默认 | 直接回显原文 | ✅ closed |
| `claude_code` | 本机 CLI | `claude -p "<prompt>" --output-format json --permission-mode plan` | ✅ closed |
| `codex` | 本机 CLI | `codex exec --skip-git-repo-check --sandbox read-only -o <FILE>` | ✅ closed |
| `copilot` | 公版 CLI | `copilot -p "<prompt>"` | 🟡 experimental（公版 CLI 未在本机验证） |
| `claude` (Anthropic API) | HTTP | — | ⛔ **stub**（返回占位字符串，B21） |
| 自定义 (hermes/openclaw/任意) | 配置化 | 见 `ai.agents.<name>` | 🟡 experimental |

### 8.3 自定义 CLI Agent 配置

```jsonc
{
  "ai": {
    "backend": "hermes",
    "rate_limit_min_interval_ms": 3000,
    "agents": {
      "hermes": {
        "binary_path": "hermes",
        "args": ["chat", "{prompt}"],
        "timeout_secs": 120,
        "max_output_bytes": 16384,
        "result_json_pointer": null,
        "read_output_file": false
      }
    }
  }
}
```

**字段语义**:
- `args`: argv 模板，`{prompt}` 在单个 argv 参数内替换（**绝不经过 shell**）
- `result_json_pointer`: JSON 指针（如 `/reply`），从输出 JSON 提取
- `read_output_file`: 为 true 时从 `{output_file}` 读取回复
- `timeout_secs` / `max_output_bytes`: 硬超时 + 输出截断上限

### 8.4 安全保证（OWASP）

| 风险 | 防护 |
|------|------|
| **命令注入 (A03)** | `tokio::process::Command::new(binary).arg(prompt)` 显式 argv，**绝不** shell |
| **权限提升** | `--permission-mode plan`（只读，不写文件 / 不执行命令） |
| **资源耗尽** | timeout_secs 杀进程 + max_output_bytes 截断 + bulkhead + CB |
| **输出注入** | 输出消毒：长度上限 + 去控制字符 |
| **信息泄露** | stderr 仅入本地日志，不发回微信；降级文案不含路径/堆栈 |
| **默认开放** | `backend=echo` 默认；启用为显式 opt-in |
| **审计留痕** | 每次 AI 调用写 `audit_log`（source / RouteKey / backend / result） |
| **数据出境** | 文档明示消息正文会交给本机 claude 配置的模型 |

### 8.5 ResilientAiBackend

所有非 echo 后端都包裹 `ResilientAiBackend`：
- **Circuit Breaker**: 失败 ≥ 阈值 → Open → 60s 内快速失败
- **Bulkhead**: AI 池 5 并发，与 Send 池（50 并发）严格隔离
- **降级**: 任何 AI 失败自动回落到 echo，主链路不中断

### 8.6 触发策略与成本控制

> 来源: ClaudeCode 四角挑战 Round 1+2 收敛结论

- **默认关闭**: `backend=echo`；启用 `claude_code` / `codex` / 自定义为显式 opt-in
- **触发门控**:
  - Permission 中间件（白名单内 peer 才进 AI）— **B20 占位放行**，需补
  - RateLimit 中间件：每会话 `rate_limit_min_interval_ms`（默认 3000ms）
- **成本/延迟预期**: 每次 ~$0.14、~3.3s；same-RouteKey 串行下，同会话连发顺序排队、每条数秒
- **数据出境**: 消息正文会交给本机 agent 配置的模型（可能上行云端）
- **token 窗口叠加**: AI 延迟占用 ilink token 新鲜度窗口；Outbox 重试兜底

---

## 第 9 章 鉴权与安全

### 9.1 动态 API 鉴权（多项目 token）

**核心概念**:

| 字段 | 用途 |
|------|------|
| `project_id` | 调用方项目稳定标识 |
| `client_name` | 可读标签 |
| `token` | 不透明随机 bearer token |
| `token_hash` | 服务端存储的 hash，**绝不存原文** |
| `scopes` | 允许的能力列表（`send`、`window_status`） |
| `expires_at` / `revoked_at` / `rotated_from` | 生命周期管理 |

### 9.2 数据存储

```sql
api_clients (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    client_name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    scopes TEXT NOT NULL,           -- JSON array
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER NULL,
    rotated_from TEXT NULL
)
CREATE INDEX idx_api_clients_project ON api_clients (project_id, revoked_at, expires_at);
```

### 9.3 端点映射

| 端点 | 所需 scope | 公开 |
|------|-----------|------|
| `GET /api/health` | — | ✅ |
| `POST /api/send` | `send` | ❌ |
| `GET /api/window_status` | `window_status` | ❌ |
| `GET /api/token_status` | `window_status` | ❌（B14 待脱敏） |
| `POST /api/feishu/webhook` | — | ✅（HMAC-SHA256 验签） |

### 9.4 鉴权失败处理

| 失败 | 状态码 |
|------|--------|
| 缺少 token | 401 |
| 未知/过期/撤销 token | 401 |
| scope 不匹配 | 403 |

### 9.5 CLI 命令

```bash
magiclaw auth issue --project <id> --name <name> --scopes send,window_status --ttl-secs 86400
magiclaw auth list --project <id>
magiclaw auth revoke --token <raw_token>
```

`auth issue` 仅打印一次原始 token。

### 9.6 Token 轮换

1. 签发新 token
2. 旧 token 在短重叠窗口内仍有效
3. 更新调用方使用新 token
4. 重叠窗口后撤销旧 token

### 9.7 已知问题

- 🟡 **B6** `runtime.rs:1382-1383` 鉴权代码被注释，导致 `daemon_api_auth_closed_loop` 测试 FAIL
- 🟡 **B14** `/api/token_status` 缺脱敏（待 OP-P0-4 修复）

---

## 第 10 章 韧性与隔离

### 10.1 Circuit Breaker

```rust
pub enum CircuitState {
    Closed,    // 正常
    Open,      // 熔断（拒绝请求）
    HalfOpen,  // 半开（探测恢复）
}

pub struct CircuitBreaker {
    failure_threshold: u32,    // 默认 20
    half_open_max: u32,        // 半开探测上限
    open_timeout: Duration,    // Open → HalfOpen 等待时间，默认 60s
    state: AtomicCell<CircuitState>,
    failure_count: AtomicU32,
    opened_at: AtomicI64,
}
```

**状态转换**:
- Closed → Open: 失败 ≥ `failure_threshold`
- Open → HalfOpen: 等待 `open_timeout` 后
- HalfOpen → Closed: 探测成功
- HalfOpen → Open: 探测失败 ≥ `half_open_max`（**B3**: 当前 HalfOpen 卡死，缺超时转回 Open 机制）

### 10.2 Bulkhead 隔离舱

```rust
pub struct ResilienceGate {
    semaphore: Arc<Semaphore>,
    active: AtomicUsize,
    max_concurrent: usize,
}
```

**配置**:
- AI 池: 5 并发
- Send 池: 50 并发
- 媒体池: 4 并发（独立）

### 10.3 已知问题

- 🔴 **B3** 熔断器 HalfOpen 永久卡死 — 必须修复
- 🟡 **B11** ConversationStore `.unwrap()` 锁中毒风险 — OP-P2-3
- 🟡 **B16** Background tasks 无 JoinHandle 监控 — OP-P1-6

---

## 第 11 章 媒体流式上传

### 11.1 字节来源抽象

```rust
pub struct MediaMeta { pub filename: String, pub mime: String, pub size: u64 }

#[async_trait]
pub trait MediaSource: Send + Sync {
    fn meta(&self) -> &MediaMeta;
    /// 打开一个分块字节流。每次发送尝试调用一次 open()
    /// 失败重试时丢弃旧流并重新 open()，绝不复用已消费的流。
    async fn open(&self) -> Result<MediaByteStream, MediaError>;
}
// MediaByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, MediaError>> + Send>>
```

### 11.2 上传 Port

```rust
pub struct MediaRef { pub media_id: String, pub url: Option<String> }

#[async_trait]
pub trait MediaUploader: Send + Sync {
    async fn upload(&self, source: &dyn MediaSource) -> Result<MediaRef, MediaError>;
}
```

### 11.3 硬约束

1. **禁止整文件读入内存**: 用 `tokio::fs::File` + `ReaderStream` 逐块产出
2. **可重开**: 每次 `upload` 调用 `open()` 获取新流
3. **独立隔离舱**: 媒体上传使用 `BulkheadPools.media`（默认并发 4）
4. **硬超时**: `media_upload_timeout_ms`（默认 60s）
5. **错误诊断**: DLQ 必带结构化错误原因（source/upload/send 阶段）

### 11.4 URL 严格解析

```rust
pub enum MediaLocation {
    LocalFile(PathBuf),
    RemoteUrl(Url),
    Unsupported(String),  // 拒绝歧义 scheme
}
```

在 channel 边界显式判定，避免远程 URL 误当本地路径。

### 11.5 幂等与恢复

- 上传请求携带 **client 端内容指纹（hash）** 作为幂等键
- 崩溃恢复沿用 Outbox：重放即重新打开来源 → 重新上传
- 媒体二进制不入库

### 11.6 已知状态

- ✅ Port + FileMediaSource + fake-uploader 已落地（`closed`）
- 🟡 **B19** IlinkMediaUploader 真实契约未实现（待 ilink 端点确认）

---

## 第 12 章 多项目推送

### 12.1 目标

- 项目与平台目标端点绑定（多对多）
- 支持"扫码登录后命令绑定"与"绑定码/链接自动绑定"两种入口
- 项目维度的多人广播与项目内定向推送
- 外部系统先通过 CLI/文件导入（JSONL + CSV）对接
- 数据统一收拢到 `data/` 目录下
- 同一项目消息可同时扇出到不同平台（wechat / dingtalk / feishu）

### 12.2 数据模型

```sql
projects (project_key PK, project_name, source_system, metadata_json, ...)
delivery_targets (target_id PK, channel, peer_id, conversation_id, conversation_type,
                  account_scope, last_seen_at, status, ...)
project_bindings (id PK, project_key, target_id, bind_source, status, bound_at, ...)
binding_tokens (token PK, project_key, target_channel, target_peer_id, expires_at,
                consumed_at, consumed_by_target_id, status, ...)
push_jobs (job_id PK, source_format, source_path, status, total_items, ...)
push_job_items (item_id PK, job_id, project_key, message_text, mode,
                target_targets_json, status, error, ...)
```

### 12.3 推送流程

```
External System → CLI import (JSONL/CSV)
  → SQLite push_jobs / push_job_items
  → Push Job Runner
  → 按 channel 分组 → 写 Outbox.pending
  → OutboxWorker → Channel.send → sent/retrying/dead_letter
```

**关键点**: 推送任务不直接发送，统一落 outbox，保持可恢复投递语义。

### 12.4 两种绑定入口

1. **扫码后命令绑定**: 用户在微信发送 `绑定项目 <project_key>` 或 `bind <project_key>`
2. **绑定码/链接自动绑定**: 外部系统生成 token → 用户扫码进入绑定动作 → 消费 token

### 12.5 当前状态

- ⏳ **设计已落地**，实施未启动（设计文档: `2026-06-19-project-binding-and-multi-push-design.md`）

---

## 第 13 章 适配层（Adapter）

### 13.1 MCP stdio Adapter

**能力分级** (`2026-06-19-mcp-deployment.md`):

| 能力 | 状态 |
|------|------|
| 传输 stdio + JSON-RPC 2.0 | ✅ closed |
| `Content-Length` 帧 + 单行 JSON 输入兼容 | ✅ closed |
| stdout 零污染 | ✅ closed |
| `initialize` / `tools/list` | ✅ closed |
| `tools/call: send` | ✅ closed（异步入 Outbox） |
| `tools/call: list_peers` | 🟡 experimental（仅 wechat） |
| `tools/call: login` | 🟡 experimental |

**Claude Desktop 集成**:
```json
{
  "mcpServers": {
    "magiclaw": {
      "command": "/path/to/magiclaw/target/release/magiclaw",
      "args": ["--mcp"],
      "env": {
        "WECHAT_CHANNEL_DIR": "/Users/you/.claude/channels/wechat",
        "RUST_LOG": "info"
      }
    }
  }
}
```

### 13.2 HTTP API Adapter

**端点**:
| 路径 | 鉴权 | 作用 |
|------|------|------|
| `POST /api/send` | Bearer (`send` scope) | 发送消息 |
| `GET /api/window_status` | Bearer (`window_status` scope) | 查询发送窗口状态 |
| `GET /api/token_status` | Bearer | Token 状态（**B14 待脱敏**） |
| `GET /api/health` | 公开 | 健康检查 |
| `POST /api/feishu/webhook` | HMAC-SHA256 验签 | Feishu 事件入口 |

### 13.3 CLI Adapter

**命令清单**:
- `magiclaw send --message "..." --to "<peer>"` — 单次发送
- `magiclaw auth issue/list/revoke` — Token 管理
- `magiclaw --mcp` — 启动 MCP stdio server
- `magiclaw bind import / push import / push run` — 多项目推送（**待实现**）

**发送策略**:
1. 先尝试发到本地 daemon 的 `POST /api/send`
2. daemon 不可达时回退为本地 Outbox（**B5 待修复**）

### 13.4 Push / Binding Adapter

**当前状态**: ⏳ 设计已落地，实施未启动。

---

## 第 14 章 可观测性与审计

### 14.1 健康检查 `GET /api/health`

无需鉴权。返回结构：

```json
{
  "ok": true,
  "feishu": {
    "enabled": true,
    "accounts": [{"account_id": "...", "receive_id_type": "...", "auth_method": "..."}],
    "account_count": 1
  },
  "tasks": {
    "running": ["outbox_worker", "gc_janitor", "inbound_router"],
    "finished_count": 2,
    "finished": [{"name": "outbox_worker", "state": "running"}, ...]
  },
  "resilience": {
    "send_gate": {"circuit_state": "closed", "failure_count": 0, "failure_threshold": 20, "active": 3, "max_concurrent": 50},
    "ai_gate": {"circuit_state": "closed", "failure_count": 0, "failure_threshold": 20, "active": 0, "max_concurrent": 5},
    "outbox_pending": 0,
    "dead_letter_count": 0
  }
}
```

### 14.2 审计日志

**审计动作**:
- `ai_generate` — AI 调用结果
- `send` — 发送决策
- `project_bind` / `project_unbind` — 绑定/解绑
- `binding_token_create` / `binding_token_consume` — Token 消费
- `push_job_import` / `push_job_run` — 推送任务
- `wechat_login` — WeChat 登录
- `token_revoke` — Token 撤销
- `permission_denied` — 权限拒绝

**字段**:
```sql
audit_log (
    id, source, route_key, action, result,
    payload, prev_hash, entry_hash,  -- hash chain
    created_at
)
```

**完整性**: 启动时校验 hash chain 失败则拒绝启动：
```
[ERROR] audit log chain integrity check FAILED — refusing to start
```

### 14.3 结构化日志

- **stdout**: 仅 MCP JSON-RPC 协议输出
- **stderr**: `tracing` 结构化日志（JSON 格式）
- **文件**: 可选 `tracing-appender` 滚动文件输出

**关键指标**:
- `dedup_hit_total` / `dedup_miss_total`
- `inbound_dropped_total`（背压丢弃）
- `route_queue_dropped_total`（会话队列丢弃）
- `late_arrival_total`（ReorderWindow 迟到）
- `active_conversations`（gauge）
- `conversation_gc_total`（GC 回收计数）

---

## 第 15 章 部署与运行模式

### 15.1 三种运行模式

#### 15.1.1 Daemon（默认）

```bash
./target/release/magiclaw
```

启动后：
- 后台 runtime（GC janitor / inbound router / outbox worker / wechat token poller / HTTP API）
- 端口默认 `127.0.0.1:18011`
- 所有持久化状态写入 SQLite

> ⚠️ **不要写 `magiclaw daemon`** —— 没有该子命令

#### 15.1.2 MCP stdio Server

```bash
./target/release/magiclaw --mcp
# 或
./target/release/magiclaw mcp
```

- JSON-RPC 2.0 over stdio
- stdout 零污染
- stdin 到 EOF（Ctrl-D）时 Server 优雅退出

#### 15.1.3 CLI 单次发送

```bash
./target/release/magiclaw send --message "hello" --to "<peer_id>"
```

发送策略：daemon HTTP → 本地 Outbox 回退（**B5 待修复**）。

### 15.2 环境变量

| 变量 | 默认值 | 作用 |
|------|--------|------|
| `MAGICLAW_DB_PATH` | `data/magiclaw.db` | SQLite 路径 |
| `MAGICLAW_API_ADDR` | `127.0.0.1:18011` | HTTP API 监听地址 |
| `MAGICLAW_API_AUTH_ENABLED` | `true` | 是否要求 Bearer 鉴权（**B6 当前被注释**） |
| `MAGICLAW_WECHAT_SEND_MIN_INTERVAL_MS` | `500` | 同 peer 发送最小间隔 |
| `MAGICLAW_DB_POOL_SIZE` | `max(4, num_cpus)` | DB 连接池大小 |
| `MAGICLAW_AI_BACKEND` | `echo` | AI 后端（`echo` / `claude_code` / `codex` / `copilot` / 自定义） |
| `WECHAT_CHANNEL_DIR` | `~/.claude/channels/wechat` 或 `./.claude/channels/wechat` | WeChat 数据目录 |
| `FEISHU_ENABLED` / `FEISHU_*` | `false` | Feishu 通道启用与凭证 |
| `DINGTALK_*` | — | Dingtalk 通道凭证 |
| `RUST_LOG` | `info` | 日志级别（只走 stderr/file） |

### 15.3 一键启动

```bash
scripts/daemon-up.sh
# 自动签发 token + 启动 daemon
```

---

## 第 16 章 数据持久化 Schema

> 来源: `src/infrastructure/db.rs` 自动建表

### 16.1 核心消息表

```sql
-- 入站消息（幂等记录）
inbox (
    id TEXT PRIMARY KEY,
    channel TEXT, conversation_id TEXT,
    payload TEXT,
    status TEXT,           -- pending/processing/processed
    created_at INTEGER, updated_at INTEGER
)

-- 出站消息 + 重试元数据
outbox (
    id TEXT PRIMARY KEY,
    route_key TEXT,        -- serialized RouteKey (B12 待修复)
    payload TEXT,
    status TEXT,           -- pending/sending/sent/retrying/dead_letter
    retry_count INTEGER,
    next_retry_at INTEGER,
    last_error TEXT,
    created_at INTEGER, updated_at INTEGER
)

-- 死信
dead_letter (
    id TEXT PRIMARY KEY,
    source TEXT,
    payload TEXT,
    reason TEXT,
    created_at INTEGER
)

-- 会话状态（崩溃恢复）
conversation_state (
    route_key TEXT PRIMARY KEY,
    state_json TEXT,
    updated_at INTEGER
)
```

### 16.2 WeChat 表

```sql
-- 长轮询同步缓冲
sync_buf (channel TEXT, account TEXT, buf BLOB, updated_at INTEGER,
          PRIMARY KEY(channel, account))

-- Per-user context token
wechat_context_tokens (account_id TEXT, user_id TEXT, token TEXT,
                       updated_at INTEGER,
                       PRIMARY KEY(account_id, user_id))
```

### 16.3 AI 与偏好

```sql
-- Per-user AI agent 偏好
user_agent_preferences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel TEXT, account_scope TEXT, peer_id TEXT,
    agent_name TEXT,
    updated_at INTEGER,
    UNIQUE(channel, account_scope, peer_id)
)
```

### 16.4 鉴权

```sql
-- 动态 API token
api_clients (
    id TEXT PRIMARY KEY,
    project_id TEXT, client_name TEXT,
    token_hash TEXT UNIQUE,
    scopes TEXT,
    created_at INTEGER, expires_at INTEGER,
    revoked_at INTEGER, rotated_from TEXT
)
```

### 16.5 审计

```sql
-- 不可篡改审计日志 + hash chain
audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source TEXT, route_key TEXT,
    action TEXT,           -- ai_generate, send, project_bind, ...
    result TEXT,           -- ok, failed, dropped, ...
    payload TEXT,
    prev_hash TEXT,        -- hash chain 前一条
    entry_hash TEXT,       -- SHA256
    created_at INTEGER
)
-- 保留期 ≥ 5 年 (1827 天 + 缓冲)
```

### 16.6 多项目推送

```sql
projects (project_key TEXT PK, project_name TEXT, source_system TEXT, metadata_json TEXT, ...)
delivery_targets (target_id TEXT PK, channel TEXT, peer_id TEXT, conversation_id TEXT,
                  conversation_type TEXT, account_scope TEXT, last_seen_at INTEGER,
                  status TEXT, created_at INTEGER, updated_at INTEGER,
                  UNIQUE(channel, peer_id, conversation_id, conversation_type, account_scope))
project_bindings (id TEXT PK, project_key TEXT, target_id TEXT, bind_source TEXT,
                  status TEXT, bound_at INTEGER, unbound_at INTEGER, ...)
binding_tokens (token TEXT PK, project_key TEXT, target_channel TEXT, target_peer_id TEXT,
                expires_at INTEGER, consumed_at INTEGER, consumed_by_target_id TEXT,
                status TEXT, ...)
push_jobs (job_id TEXT PK, source_format TEXT, source_path TEXT, status TEXT,
           total_items INTEGER, success_items INTEGER, failed_items INTEGER, ...)
push_job_items (item_id TEXT PK, job_id TEXT, project_key TEXT, message_text TEXT,
                mode TEXT, target_targets_json TEXT, status TEXT, error TEXT, ...)
```

### 16.7 配置

```sql
config (key TEXT PRIMARY KEY, value TEXT, updated_at INTEGER)
allowlist (channel TEXT, peer_id TEXT, conversation_id TEXT, created_at INTEGER,
           PRIMARY KEY(channel, peer_id, conversation_id))
session (channel TEXT, account TEXT, key TEXT, value TEXT,
         PRIMARY KEY(channel, account, key))
```

### 16.8 索引建议

- `inbox(channel, status)` / `inbox(conversation_id, status)`
- `outbox(status, next_retry_at)` / `outbox(channel, status)`
- `idx_outbox_pending` / `idx_outbox_retry`（**WSS-005 已修复**）
- `audit_log(action, created_at)` / `audit_log(route_key, created_at)`
- `user_agent_preferences(channel, account_scope, peer_id)`
- `api_clients(token_hash)` UNIQUE / `api_clients(project_id, revoked_at, expires_at)`
- `project_bindings(project_key, status)` / `project_bindings(target_id, status)`
- `binding_tokens(project_key, status, expires_at)`
- `push_job_items(job_id, status)`
- `delivery_targets(channel, status)`

---

## 第 17 章 迁移阶段计划

来源: `2026-06-19-rust-migration-v5.md` §10

### 17.1 Phase 1（2 周）— 稳定内核

**目标**: 可长期运行的稳定内核。

**交付**:
1. ✅ RouteKey 升级（conversation 维度）
2. ✅ Worker Idle GC（30 分钟）
3. ✅ Dedup TTL Cache（moka）
4. 🟡 Reorder Window（**B1 截止时间计算错误需修复**）
5. ✅ WeChat `sync_buf` 持久化

### 17.2 Phase 1.5（0.5-1 周）— MCP 前置

**交付**:
1. ✅ MCP Adapter 最小可用（send closed）
2. 🟡 list_peers / login 仍为 experimental
3. ✅ stdout 零污染

### 17.3 Phase 2（1.5-2 周）— 可恢复投递

**交付**:
1. ✅ SQLite Inbox/Outbox/DLQ
2. ✅ 重试与死信重放
3. ✅ 崩溃恢复验证
4. 🟡 **B4** `/api/send` 绕过 Outbox（待修复）

### 17.4 Phase 3（1-2 周）— 多信道扩展

**交付**:
1. ✅ Dingtalk / Feishu 骨架
2. 🟡 **B17** Dingtalk 多账号隔离未验证
3. ✅ Feishu Phase A 闭环

### 17.5 Phase 4（1.5-2 周）— Pipeline + AI 可插拔

**交付**:
1. ✅ Middleware 链落地
2. ✅ Echo/ClaudeCode/Codex/Copilot/Hermes/Openclaw 接入
3. 🟡 **B20** Permission 中间件占位放行

### 17.6 Phase 5（2 周）— 生产强化

**交付**:
1. 🟡 Circuit Breaker（**B3 HalfOpen 卡死需修复**）
2. ✅ Bulkhead 隔离
3. 🟡 daemon / system service / health
4. ✅ 审计不可篡改（hash chain）
5. 🟡 **B15** 审计保留期清理策略未实现

### 17.7 当前 PR/已落地能力

**已合并**:
- ✅ Phase A Feishu Tasks 2-5
- ✅ WeChat Token P0 修复（commit 5ea6e95）
- ✅ WeChat QR 登录 + daemon bootstrap
- ✅ Claude Code AI 后端（`closed`）
- ✅ DB 连接池（commit 6a35152）
- ✅ `/api/health` 韧性增强（commit c080c9a）
- ✅ HTTP API 单元测试（commit 4269c22）

**待启动 / 部分完成**:
- ⛔ `2026-06-25-code-analysis-modification-plan.md` Phase A（API 鉴权 + CB HalfOpen + 审计 hash chain + token 脱敏）
- ⛔ `2026-06-25-code-analysis-modification-plan.md` Phase B（Conversation Snapshot + ReorderWindow + unwrap 消除 + outbox 错误处理）
- ⛔ `2026-06-25-code-analysis-modification-plan.md` Phase C（main.rs 拆分 + HTTP API 拆分 + 类型化错误）
- ⛔ Feishu long-connection adapter
- ⏳ 项目绑定 + 多人推送

---

## 第 18 章 强制红线（MUST）

> 来自 `AGENTS.md` 第二部分，违反即阻断合并与上线。

### 18.1 架构边界（红线 2.1）

- ✅ **MUST** Core 不依赖 Agent；AI 仅作为可插拔能力
- ✅ **MUST** MCP/REST/CLI 作为 Adapter，不侵入核心业务模型
- ✅ **MUST** Conversation 为一等对象；RouteKey 包含 channel、conversation_id、peer_id、conversation_type

### 18.2 信道稳定与顺序性（红线 2.2）

- ✅ **MUST** 同 RouteKey 串行，不同 RouteKey 并行
- ✅ **MUST** Route Worker 具备 Idle GC（默认 30 分钟）
- ✅ **MUST** Dedup 使用 TTL Cache（moka），禁止全量清理
- 🟡 **MUST** 有 sequence 的平台按 sequence 排序；无 sequence 用 `timestamp + reorder_window`（**B1 截止时间计算错误**）
- ✅ **MUST** 超窗口迟到消息走幂等处理 + 审计标记

### 18.3 可恢复投递与状态持久化（红线 2.3）

- ✅ **MUST** 落地 Inbox/Outbox/DLQ，具备重试与死信重放
- ✅ **MUST** 发送状态机 pending → sending → sent；失败 → retrying → dead_letter
- ✅ **MUST** 核心状态持久化（allowlist / session / conversation_state / inbox / outbox / audit_log）
- ✅ **MUST** WeChat `sync_buf` 持久化并支持重启恢复
- ✅ **MUST** 崩溃恢复后可继续未完成发送
- 🟡 **MUST** Session Expired 完整清除（**B25** 仅清 tokens）

### 18.4 协议与安全（红线 2.4）

- ✅ **MUST** MCP stdio 零污染
- ✅ **MUST** ilink 关键契约具备 contract test
- 🟡 **MUST** 多账号/多通道路径级隔离（**B17 Dingtalk 未验证**）
- ✅ **MUST** 媒体上传支持流式和分段
- 🔴 **MUST** REST Adapter 启用最小认证（**B6 当前被注释**）
- 🟡 **MUST** Typing Service（**B24 未实现**）

### 18.5 韧性与隔离（红线 2.5）

- 🔴 **MUST** 外部平台 API 与 AI API 启用 Circuit Breaker（**B3 HalfOpen 卡死**）
- ✅ **MUST** AI 执行池与发送执行池做 Bulkhead 隔离
- 🟡 **SHOULD** Pipeline/Middleware 化
- 🟡 **SHOULD** 发送统一为 `send_text_with_recovery` / `send_media_with_recovery`
- 🟡 **SHOULD** RateLimiter 升级为可扩展实现

### 18.6 审计与可追溯（红线 2.6）

- 🟡 **MUST** 关键数据流与发送决策留痕（**B15 保留期清理策略未实现**）
- 🟡 **MUST** 自动加白名单等高风险操作写入审计（**B20 Permission 占位放行**）
- ✅ **MUST** 审计日志不可篡改（hash chain）
- 🟡 **MUST** 审计日志保留期 ≥ 5 年（**B15 清理策略未实现**）

**红线违反统计**: 全部 6 类红线都有违反，详见 `2026-06-28-bugs.md`。

---

## 第 19 章 失败模式与恢复

### 19.1 入站失败

| 失败源 | 影响 | 处理 | 恢复 |
|--------|------|------|------|
| WeChat webhook 超时 | 入站消息丢失 | WeChat 侧有重试队列 | 幂等接收入站 + dedup |
| WeChat poll 断开 | 无法拉取新消息 | Channel 内部自动重连 + backoff | 重连后从 sync_buf 断点继续 |
| moka Dedup OOM | 老条目被 LRU 驱逐 | moka 内置驱逐策略 | 重复处理由 Inbox 幂等兜底 |
| moka Dedup 进程崩溃 | Cache 全量丢失 | 重启后冷启动 | Inbox 幂等兜底 |
| Reorder Window 进程崩溃 | Buffer 中未刷出消息丢失 | 接受（Phase 2 Inbox 补齐持久化） | — |
| 迟到消息（超 window） | 乱序进入处理 | 标记 `late_arrival` + audit | 审计可追溯 |
| sync_buf SQLite 磁盘满 | 写入失败 | 返回错误，上层重试 | 清理磁盘后恢复 |
| sync_buf DB 损坏 | sync_buf 丢失 | 备份 + 全量重拉 | 从 WeChat 服务端重新同步 |
| Inbound Channel 背压 | channel 满 → Drop Oldest | 审计 + 指标递增 | webhook 重试重新入站 |
| Route Queue 背压 | 单会话队列满 → Drop Newest | 审计 + 指标递增 | 旧消息保留，新消息由发送方重试 |
| Worker Idle GC 误回收 | 消息处理中断 | last_active 在 enqueue 前更新 | 下一条消息自动创建新 Conversation |

### 19.2 出站失败

| 场景 | 状态 | 恢复 |
|------|------|------|
| 发送中途 crash | Outbox: sending | 重启后 `recover_after_crash` → 重新发送 |
| 重试中途 crash | Outbox: retrying | 同 crash recovery，基于 next_retry_at 重试 |
| 发送成功但 crash 前未标记 | Outbox: sending | 重新发送 → 平台返回成功 → 逻辑幂等 |
| DB 写入失败 | 磁盘满 / 损坏 | 错误返回，不更新状态，上层告警 |
| 重试耗尽 | Outbox → DeadLetter | 进入 DLQ，等待手工重放 |
| DLQ 重放 | DeadLetter → Outbox(pending) | 重新走完整状态机 |
| Session Expired (-14) | send 失败 | 检测后清除 tokens（**B25 待完善**）+ Outbox 重试 |

### 19.3 AI 失败

| 失败源 | 处理 |
|--------|------|
| claude 二进制缺失 | 捕获 → Err → 中间件降级 echo + 熔断累计 |
| 未登录 / 认证失败 | 子进程非零退出 → Err → 降级 echo，不泄露 stderr |
| 子进程挂起 / 长耗时 | 超时 → `child.start_kill()` → Err → 降级 |
| 输出过大 | 截断 + 标注 `…(truncated)` |
| 非 UTF-8 输出 | `from_utf8_lossy` 有损返回 |
| 输出 JSON 解析失败 | 回退到读纯文本 stdout |
| 并发过载 | ResilienceGate 隔离舱拒绝 → Err → 降级 |
| 连续失败 | 熔断器 Open → 直接 Err → 降级 |

### 19.4 WeChat Session 失效

**完整 reset 步骤** (来源: 官方 Node.js `clearAll()`):

1. 删除 CREDENTIALS（bot_token + baseurl + account_id + user_id）
2. 删除 CURSOR（sync_buf / get_updates_buf）
3. 删除 CONTEXT_TOKENS（所有 per-user tokens）
4. 删除 TYPING_TICKETS（如实现了）

**当前实现** (`channel.rs` long-poll 错误处理):
- ✅ 仅调用 `state.clear_context_tokens()`（**B25 不完整**）
- ⛔ CREDENTIALS / CURSOR / TYPING_TICKETS 未清除

---

## 第 20 章 已知 Bug 与优化空间

> 完整列表见 `2026-06-28-bugs.md`（25 项）与 `2026-06-28-optimization-backlog.md`（33 项）。

### 20.1 关键 P0 红线违反（必修）

| Bug | 标题 | 工作量 |
|-----|------|--------|
| **B1** | ReorderWindow 截止时间用 sequence 推导（非挂钟） | 4h |
| **B2** | Conversation clone 泄漏到 Pipeline（DDD 聚合边界） | 5h |
| **B3** | 熔断器 HalfOpen 永久卡死 | 3h |
| **B4** | `/api/send` 绕过 Outbox 直接 ilink 发送 | 6h |
| **B6** | HTTP API 鉴权被注释（测试 FAIL） | 4h |
| **B14** | `/api/token_status` 无认证 + 泄露 token | 1h |

### 20.2 P1 健壮性

| Bug | 标题 |
|-----|------|
| B5 | CLI fallback 绕过 Outbox |
| B8 | Outbox `fetch_pending` + `mark_status(sending)` 非原子 |
| B10 | Outbox `.ok()` 静默吞错误 |
| B11 | ConversationStore `.unwrap()` 锁中毒风险 |
| B15 | Audit 保留期清理策略未实现 |
| B16 | Background tasks 无 JoinHandle 监控 |
| B17 | Dingtalk 多账号隔离未验证 |
| B18 | Feishu long-connection adapter 完全未实现 |
| B19 | IlinkMediaUploader 真实契约未实现 |
| B20 | Permission 中间件占位放行 |
| B21 | `claude.rs` Anthropic API stub |
| B24 | Typing Service 完全未实现 |
| B25 | Session Reset 不完整 |

### 20.3 故意保留的设计决策（不修复）

来源 `2026-06-25-code-analysis-modification-plan.md` §九.5：

| ID | 决策 | 理由 |
|----|------|------|
| KP-1 | `validate_feishu_config` panic 设计 | fail-fast |
| KP-2 | WeChat iLink string-based JSON | 格式不稳定，保留 `serde_json::Value` 灵活性 |
| KP-3 | PipelineContext 含 AppConfig clone | GC 回收时持有有效配置 |
| KP-4 | BackpressureConfig `DropNewest` 默认 | 当前正确的安全选择 |
| KP-5 | MessageContent `Image/File/Unknown` 变体 | 按 Phase 3-4 推进 |

---

## 附录 A：参考文档索引

> 历史设计文档保留于 `docs/` 与 `docs/superpowers/`，本文件作为整合入口。

### 架构与迁移
- `2026-06-19-rust-migration-v5.md` — 迁移方案 v5
- `2026-06-19-phase1-architecture.md` / `2026-06-19-phase1-plan.md` — 阶段 1
- `2026-06-19-phase1.5-architecture.md` — 阶段 1.5（MCP）
- `2026-06-19-phase2-architecture.md` — 阶段 2（可恢复投递）
- `2026-06-19-phase3-architecture.md` — 阶段 3（多信道）
- `2026-06-19-phase4-architecture.md` — 阶段 4（Pipeline + AI）
- `2026-06-19-implementation-gap-solution.md` / `2026-06-19-implementation-gap-task-breakdown.md` — 落地偏差分析

### 阶段报告与 PR
- `2026-06-23-phase-a-completion-report.md`
- `2026-06-23-phase-a-checklist.md`
- `2026-06-23-pr-description.md` / `2026-06-23-pr-phase-a-tasks-2-5.md`
- `2026-06-23-p0-fixes-implementation-report.md`

### 信道设计
- WeChat: `wechat-send-stability-{design,plan,log}` + `wechat-context-token-{robustness-design,review,final-review}`
- Feishu: `2026-06-23-feishu-push-{architecture,plan}`
- Dingtalk: `tests/dingtalk_closed_loop.rs`

### 特性设计
- MCP: `2026-06-19-mcp-deployment.md`
- AI 后端: `claude-code-backend-{design,challenge,plan}`
- 项目绑定: `2026-06-19-project-binding-and-multi-push-design.md`
- 用户偏好: `2026-06-19-user-agent-preferences-design.md`
- 鉴权: `2026-06-20-dynamic-api-auth-design.md` + `superpowers/plans/2026-06-20-dynamic-api-auth.md`
- 媒体: `media-streaming-upload-{design,plan}`

### superpowers
- `superpowers/2026-06-23-feishu-push-pr-checklist.md`
- `superpowers/plans/2026-06-25-code-analysis-modification-plan.md`（v3）

### 项目根
- `README.md`
- `CLAUDE.md`（红线与开发流程）
- `AGENTS.md`（7 步流程）

### 整合文档
- `2026-06-28-integration-index.md` — 文档地图
- `2026-06-28-development-status.md` — 完成度评估
- `2026-06-28-optimization-backlog.md` — 优化机会
- `2026-06-28-bugs.md` — Bug 清单
- `2026-06-28-project-design.md` — **本文档**

---

## 附录 B：术语表

| 术语 | 含义 |
|------|------|
| **RouteKey** | 路由键，包含 channel / conversation_id / peer_id / conversation_type |
| **Conversation** | 会话聚合根，拥有一等对象地位 |
| **ConversationSnapshot** | 只读值对象，用于 Pipeline（替代 Conversation clone） |
| **MPSC** | Multi-Producer Single-Consumer channel |
| **DLQ** | Dead Letter Queue，死信队列 |
| **CB** | Circuit Breaker，断路器 |
| **WAL** | Write-Ahead Logging，SQLite 预写日志 |
| **JWT / Bearer** | HTTP 认证方案 |
| **HMAC-SHA256** | Feishu webhook 验签算法 |
| **ilink** | WeChat 内部协议（getupdates / sendmessage / getconfig） |
| **sync_buf** | WeChat 长轮询的会话同步缓冲 |
| **context_token** | WeChat 用户级会话凭证 |
| **errcode -14** | WeChat 会话失效错误码 |
| **Pipeline** | 中间件链处理模型 |

---

*文档版本: v6.0 | 整合日期: 2026-06-28 | 维护者: 项目组 | 配套: INTEGRATION_INDEX / DEVELOPMENT_STATUS / OPTIMIZATION_BACKLOG / BUGS*