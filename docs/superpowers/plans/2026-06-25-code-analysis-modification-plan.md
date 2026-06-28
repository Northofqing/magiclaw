# Magiclaw 代码分析与修改方案

**分析日期**: 2026-06-25 (v3 更新版)
**分析方法**: Rust Skills 三层元认知分析 + Clippy 语法检查 + Superpowers 架构审查 + 测试验证
**分析范围**: 全项目 88 个 Rust 源文件 (17,525 行) + 18 个测试文件 (2,501 行)
**当前状态**:
- `cargo check`: 通过 (3 warnings)
- `cargo test`: **21/22 通过** (1 FAIL: `daemon_api_auth_closed_loop` — 因 API 认证被注释)
- `cargo clippy --lib`: 10 errors (with `-D warnings`)
- `cargo clippy --tests`: 12 errors (with `-D warnings`)

---

## 一、总体评估

### 1.1 架构优势

| 优势 | 详情 |
|------|------|
| DDD + 六边形架构 | 依赖方向正确 (domain 零依赖)，端口-适配器分离干净 |
| Pipeline 模式 | Chain of Responsibility + Decorator，中间件可插拔，short-circuit 机制灵活 |
| RouteKey 并发模型 | 同 RouteKey 串行 (mpsc)，跨 RouteKey 并行，设计正确 |
| 韧性装饰器 | `ResilientAiBackend` / `ResilientOutboxSender` 包装业务逻辑，Phase 5 注入零侵入 |
| 闭环测试 | 18 个闭环测试覆盖 WeChat/Feishu/Resilience/MCP/Media/Pipeline 核心链路 |
| 崩溃恢复 | `recover_after_crash` 在启动时重置 `sending`/`retrying`，sync_buf 持久化 |
| iLink 加密 | AES-128-ECB/PKCS7 实现完备，有 known-vector 测试 |

### 1.2 关键风险矩阵

| 严重度 | 数量 | 描述 |
|--------|------|------|
| 🔴 P0 | 4 | 熔断器 Bug、聚合边界破坏、ReorderWindow 计算错误、API 无认证 |
| 🟡 P1 | 8 | String 错误类型、outbox .ok() 吞错、RwLock unwrap、无 JoinHandle、DbPool 直用等 |
| 🟢 P2 | 6 | main.rs 巨大、domain/storage 位置错误、clippy 警告、测试 Bug 等 |
| ⚪ P3 | 4 | 结构性并发、消息顺序强化、DB 迁移、配置热加载 |

---

## 二、三层元认知分析发现（新增）

### 2.1 Layer 1 — 并发/语法层面

#### 🔴 发现 1: 熔断器 HalfOpen 状态永久卡死

**位置**: `src/core/resilience/circuit_breaker.rs:73`

```rust
CircuitState::HalfOpen => {
    self.failure_count.load(Ordering::SeqCst) < self.config.half_open_max
}
```

**问题**: 当 `failure_count >= half_open_max` 时，`allow_request()` 永久返回 `false`，且 **没有任何超时机制从 HalfOpen 转回 Open**。电路永久卡死在 HalfOpen。

**推理链**:
```
+-- Layer 1: 并发状态机 Bug
|   Problem: HalfOpen 状态无超时转回，failure_count 累积后永久阻塞
|       ^
+-- Layer 3: 云原生韧性 (domain-cloud-native)
|   Constraint: Circuit Breaker 必须有自恢复路径
|   Rule: 所有状态都必须在有限时间内收敛到 Closed
|       v
+-- Layer 2: 设计决策
    Decision: HalfOpen 添加超时 → 超时后转回 Open 重新等待完整 timeout
```

**修复方案**: 在 `allow_request()` 的 HalfOpen 分支也检查 `opened_at` 超时，超时后重置 `failure_count` 并切换到 Open 状态。

#### 🔴 发现 2: 聚合边界被破坏 — Conversation 克隆泄漏

**位置**: `src/adapters/conversation_store.rs:59-79`

```rust
let ctx = PipelineContext {
    message: m,
    conversation: conversation.clone(),  // BUG: 克隆的聚合
    ...
};
```

**问题**: 工作者持有的真实 `Conversation` 聚合被克隆后传入 Pipeline 中间件。中间件对克隆的任何修改都 **不会反映回真实聚合**，且任何中间件都可以绕过聚合根直接读/写 Conversation。

**推理链**:
```
+-- Layer 1: 所有权/借用语义
|   Problem: Clone 创建独立副本，修改丢失
|       ^
+-- Layer 3: DDD 领域建模 (m09-domain)
|   Constraint: 聚合外部只能通过聚合根方法修改
|   Rule: "聚合边界被破坏" 是 Common Mistake
|       v
+-- Layer 2: 设计决策
    Decision: 定义 ConversationSnapshot 值对象（只读视图）替代 clone
```

**修复方案**: 创建 `ConversationSnapshot` 只读值对象，包含管道需要的 conversation_id、conversation_type 等信息，不暴露聚合内部状态。

#### 🔴 发现 3: ReorderWindow 截止时间计算错误

**位置**: `src/domain/services/reorder_window.rs:42`

```rust
let newest_key = *self.buffer.last_key_value().unwrap().0;
let cutoff = newest_key.saturating_sub(self.window_ms as i64);
```

**问题**: 截止时间从消息的排序键（sequence number）推导，而非挂钟时间。当高序列号消息到达时（如 seq=100 后 seq=999999），截止时间跳跃导致中间消息被错误地提前刷新。

**修复方案**: 记录消息实际插入时间戳 (`Instant::now()`)，使用挂钟时间计算截止窗口。

#### 🟡 发现 4: 无 JoinHandle 观察 — 后台任务静默崩溃

**位置**: `src/infrastructure/runtime.rs:369,378,404` 所有 `tokio::spawn` 位置

**问题**: 所有后台任务（GC janitor、inbound router、outbox_worker、long-poll、HTTP API）都没有存储 `JoinHandle`。如果任何一个 panic，系统不会检测到 — 尤其是 outbox_worker 崩溃会导致所有外发消息静默停止。

**修复方案**: 使用 `tokio::task::JoinSet` 统一管理，结合 health endpoint 暴露任务状态。

#### 🟡 发现 5: Outbox .ok() 静默吞错误 → 重复投递风险

**位置**: `src/application/outbox_worker.rs` (多行 `.ok()` 调用)

```rust
outbox.mark_status(&entry.id, OutboxStatus::Sending, None).ok();
// 如果此处失败，消息仍为 Pending → 下次轮询重发 → 重复投递
```

**修复方案**: 使用 `if let Err(e) = ... { tracing::error!(...); return; }` 模式。

#### 🟡 发现 6: WeChat SessionState 冗余嵌套 RwLock

**位置**: `src/channels/wechat/channel.rs:19,24-35`

```rust
struct SessionState {
    context_tokens: Arc<tokio::sync::RwLock<HashMap<String, String>>>, // 冗余
    sync_buf: String,
}
// 外层已有 Arc<tokio::sync::Mutex<SessionState>>
```

**问题**: 外层 `Mutex<SessionState>` 已经串行化所有访问，内层 `RwLock<HashMap>` 无额外线程安全收益，仅增加调度开销。

**修复方案**: 将 `context_tokens` 改为普通 `HashMap<String, String>`。

### 2.2 Layer 2 — 设计模式层面

#### 🟡 发现 7: 核心 Trait 全部使用 String 错误类型

**位置**: `channels/channel_trait.rs`, `core/pipeline/middleware.rs`, `core/ai/backend.rs`

```rust
async fn send_message(&self, ...) -> Result<SendReceipt, String>;
async fn process(&self, ...) -> Result<PipelineContext, String>;
async fn generate(&self, ...) -> Result<String, String>;
```

**影响**: 调用方无法按错误类型分支（只能靠 `is_ret_minus_2_error()` 字符串匹配），与已有的 `RepoError`、`SendError`、`BulkheadError` 不一致。

**修复方案**: 定义 `ChannelError`、`PipelineError`、`AiError` 枚举。

#### 🟡 发现 8: 应用层直接使用 DbPool 绕过端口

**位置**: `application/audit.rs`, `agent_preferences.rs`, `binding.rs`, `push.rs`

这些模块直接接受 `DbPool` 而非通过 `domain/ports/` 中的 trait。违反依赖反转原则。

#### 🟡 发现 9: ConversationStore 中 RwLock::unwrap() 级联 panic 风险

**位置**: `src/adapters/conversation_store.rs:213,221,259,276,286`

```rust
routes.read().unwrap()  // 锁中毒 → panic → 整个会话存储崩溃
```

**修复方案**: `.lock().unwrap_or_else(|e| e.into_inner())` 或迁移到 `parking_lot::RwLock`。

### 2.3 Layer 3 — 领域建模层面

#### 🟢 发现 10: domain/storage/ 属于 infrastructure/

`domain/storage/inbox.rs`, `outbox.rs`, `dead_letter.rs` 是数据库行结构体，包含 `created_at`、`updated_at` 等持久化细节，不是领域概念。应移至 `infrastructure/storage/`。

#### 🟢 发现 11: OutboxStatus 状态转换未在类型层强制

`OutboxStatus` 的转换（`Pending → Sending → Sent/Retrying/DeadLetter`）靠运行时约定和审计日志保证，未使用类型状态模式。类型状态可以消除非法转换。

#### 🟢 发现 12: OutboxEntry.route_key 存储为 JSON String

反序列化边界在运行时可能失败 (`RegistryOutboxSender::send` 中的 `serde_json::from_str`)，丢失了类型安全。

---

## 三、Clippy 语法检查结果

### 3.1 可自动修复 (6 warnings → `cargo clippy --fix`)

| # | 文件 | 行 | 警告 | 修复 |
|---|------|-----|------|------|
| 1 | `src/infrastructure/runtime.rs` | 7 | `unused import: from_fn_with_state` | 移除导入 (API 认证恢复时重新添加) |
| 2 | `src/infrastructure/runtime.rs` | 16 | `unused import: require_bearer_auth` | 移除导入 |
| 3 | `src/infrastructure/runtime.rs` | 839 | `unused variable: auth` | 改为 `_auth` 或在恢复认证时使用 |
| 4 | `src/channels/feishu/channel.rs` | 168 | `unnecessary_lazy_evaluations` | `unwrap_or_else` → `unwrap_or` |
| 5 | `src/channels/wechat/ilink.rs` | 605 | `manual_is_multiple_of` | `x % 16 != 0` → `!x.is_multiple_of(16)` |
| 6 | `src/channels/wechat/ilink.rs` | 639 | `manual_repeat_n` | `repeat().take()` → `repeat_n()` |
| 7 | `src/core/pipeline/agent_command.rs` | 55 | `unnecessary_sort_by` | `sort_by` → `sort_by_key` |

### 3.2 需手动处理

| # | 文件 | 行 | 警告 | 建议 |
|---|------|-----|------|------|
| 8 | `src/channels/feishu/channel.rs` | 304 | `large_enum_variant` (FeishuMode) | `Box<FeishuConfig>` |
| 9 | `src/channels/wechat/channel.rs` | 39 | `large_enum_variant` (WeChatMode) | `Box<ILinkSendConfig>` |
| 10 | `src/channels/feishu/channel.rs` | 371 | `too_many_arguments` (9/7) | 提取 `UploadMediaConfig` 结构体 |
| 11 | `src/main.rs` | 805-806 | `field_reassign_with_default` | 使用结构体更新语法 |
| 12 | `src/main.rs` | 960 | `suspicious_open_options` | 添加 `.truncate(true)` |
| 13 | `src/application/audit.rs` | 88 | `len_zero` | `>= 1` → `!is_empty()` |

### 3.3 测试 Bug

| # | 文件 | 行 | 类型 | 问题 |
|---|------|-----|------|------|
| 14 | `tests/feishu_webhook_security_closed_loop.rs` | 57 | **Logic Bug** | `wrong_result \|\| true` 永远为 true，断言失效 |
| 15 | `tests/feishu_webhook_security_closed_loop.rs` | 87 | unused variable | `result_2` 未使用 |
| 16 | `tests/daemon_api_auth_closed_loop.rs` | 108 | **Test Failure** | 期望 401 但得到 200（API 认证被注释） |

---

## 四、当前测试状态

```
总测试: 22
通过:   21 ✅
失败:   1  ❌ (daemon_api_auth_closed_loop — API 认证被注释导致)
```

---

## 五、P0 修改项（安全 / 正确性 / 红线违规）

### 5.1 【安全】恢复 HTTP API 认证

**位置**: `src/infrastructure/runtime.rs:1382-1383`

**方案**:
1. 对 `/api/send`、`/api/window_status`、`/api/token_status` 恢复 bearer auth
2. `/api/feishu/webhook` 保持签名验证作为唯一认证（不叠加 bearer）
3. `/api/health` 保持公开
4. 将 auth 开关从硬编码改为环境变量 `MAGICLAW_API_AUTH_ENABLED`（默认 true）
5. 同时修复 clippy: `unused import: from_fn_with_state`、`require_bearer_auth`、`unused variable: auth`
6. **恢复 daemon_api_auth_closed_loop 测试通过**

### 5.2 【正确性】修复熔断器 HalfOpen 永久卡死

**位置**: `src/core/resilience/circuit_breaker.rs:73`

**方案**:
1. 在 `allow_request()` 的 HalfOpen 分支中添加超时检查
2. 超时后重置 `failure_count` 并转回 `Open` 状态
3. 添加 HalfOpen 状态超时的单元测试

### 5.3 【红线】审计日志保留策略 + 不可变性

**位置**: `src/adapters/sqlite_audit.rs`、`src/infrastructure/db.rs`

**问题**: CLAUDE.md 红线要求 "Audit logs immutable, retention ≥ 5 years"

**方案**:
1. 为 `audit_log` 表添加 `prev_hash TEXT` 字段，实现链式 hash
2. 添加 `audit_log_retention_days` 配置 (默认 1827 = 5 年 + 2 天缓冲)
3. 在 GC Janitor 中增加审计日志过期清理
4. 启动时检查审计日志完整性（hash chain 验证）

### 5.4 【安全】`/api/token_status` 脱敏

**位置**: `src/infrastructure/runtime.rs:1117-1139`

**问题**: 返回所有 peer token 年龄，无认证保护时可被信息枚举。

**方案**:
1. 添加 bearer auth（与 5.1 同步）
2. 对 token 前缀脱敏（仅返回前 4 + 后 4 字符）

---

## 六、P1 修改项（架构 / 可维护性）

### 6.1 【架构】拆分 main.rs (1534 行 → ~50 行)

**目标结构**:
```
src/
├── main.rs                     (~50 行) 入口 + 模式分发
├── cli/
│   ├── mod.rs
│   ├── parser.rs               CLI 参数解析 (440 行)
│   ├── commands.rs             命令枚举定义
│   ├── wechat_login.rs         WeChat 登录流程 (140 行)
│   └── feishu_send.rs          飞书 CLI 发送
└── daemon/
    ├── mod.rs
    ├── singleton.rs            进程锁
    └── config_loader.rs        运行时配置加载 (140 行)
```

**验收**: main.rs < 100 行, 每个新模块有单元测试

### 6.2 【架构】拆分 start_http_api (900+ 行)

**目标结构**:
```
src/infrastructure/
├── http_api/
│   ├── mod.rs                 路由组装
│   ├── state.rs               HttpApiState + PeerTokenState
│   ├── handlers/
│   │   ├── send.rs            POST /api/send
│   │   ├── health.rs          GET /api/health
│   │   ├── feishu_webhook.rs  POST /api/feishu/webhook
│   │   ├── token_status.rs    GET /api/token_status
│   │   └── window_status.rs   GET /api/window_status
│   └── long_poll.rs           WeChat token 长轮询
```

### 6.3 【架构】std::sync::RwLock → tokio::sync::RwLock

**位置**: `src/adapters/conversation_store.rs:41`

```rust
// Before
routes: Arc<RwLock<HashMap<RouteKey, ConversationHandle>>>
// After
routes: Arc<tokio::sync::RwLock<HashMap<RouteKey, ConversationHandle>>>
```

所有 `.read().unwrap()` → `.read().await`, `.write().unwrap()` → `.write().await`

### 6.4 【正确性】修复聚合边界 — Conversation 克隆

**位置**: `src/adapters/conversation_store.rs:59-79`

**方案**: 创建 `ConversationSnapshot` 值对象:
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

PipelineContext 中 `conversation: Conversation` → `conversation_snapshot: ConversationSnapshot`

### 6.5 【正确性】修复 ReorderWindow 截止时间

**位置**: `src/domain/services/reorder_window.rs:42`

**方案**: 改用挂钟时间:
```rust
pub fn insert(&mut self, msg: Message, now: Instant) -> Vec<Message> {
    // 记录每条消息的到达时间
    // cutoff = now - window_ms
}
```

### 6.6 【错误处理】核心 Trait 类型化错误

**范围**: `Channel`、`Middleware`、`AiBackend` trait 签名，约 40+ 函数

**方案**:
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
}
```

### 6.7 【健壮性】消除生产路径 unwrap()

**范围**: `ConversationStore`, `CircuitBreaker`, 约 10 处

**方案**:
1. `Mutex::lock().unwrap()` → `.unwrap_or_else(|e| e.into_inner())`
2. 或迁移到 `parking_lot::Mutex` / `parking_lot::RwLock`（无 poison 语义）

### 6.8 【健壮性】Outbox .ok() 错误处理

**位置**: `src/application/outbox_worker.rs`

**方案**: 对 `mark_status` 和 `insert` 结果使用:
```rust
if let Err(e) = outbox.mark_status(&entry.id, OutboxStatus::Sending, None) {
    tracing::error!(message_id = %entry.id, error = %e, "failed to mark sending");
    return;
}
```

### 6.9 【健壮性】添加 JoinHandle 任务监控

**方案**:
```rust
pub struct TaskSupervisor {
    handles: tokio::task::JoinSet<()>,
}

// 启动时
supervisor.spawn(outbox_worker_loop);
supervisor.spawn(inbound_router);
supervisor.spawn(gc_janitor);

// health endpoint 报告每个任务状态
```

### 6.10 【架构】应用层通过端口访问 DB

**位置**: `application/audit.rs`, `agent_preferences.rs`, `binding.rs`, `push.rs`

**方案**: 为每类数据访问定义端口 trait，通过依赖注入传递。

---

## 七、P2 修改项（质量 / 性能）

### 7.1 【性能】DbPool 从单连接到连接池

**方案**: 迁移到 `r2d2-sqlite`:
```rust
pub struct DbPool {
    pool: r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>,
}
```

### 7.2 【功能】Dingtalk 频道骨架实现

**方案**: 参考 FeishuChannel 完成:
1. Dingtalk access_token 获取
2. 消息类型映射（文本/图片/文件/Markdown）
3. 错误语义映射（retryable vs terminal）

### 7.3 【可观测性】增强 /api/health 端点

增加信息: Circuit breaker 状态、Bulkhead 水位、Worker 统计、Outbox 统计、DB 状态

### 7.4 【Pipeline】消除硬编码的 short-circuit 中间件名

**位置**: `src/core/pipeline/middleware.rs:52-57`

```rust
pub trait Middleware: Send + Sync {
    fn name(&self) -> &'static str;
    fn is_terminal(&self) -> bool { false }  // 新增
    async fn process(&self, ctx: PipelineContext) -> Result<PipelineContext, String>;
}
```

### 7.5 【测试】HTTP API 处理器单元测试

**方案**: 提取路由构建 → 使用 `axum::test` 辅助工具 → 覆盖正常发送、token 缺失、ret=-2、webhook challenge

### 7.6 【编译】修复全部 Clippy 问题

| 优先级 | 文件 | 数量 |
|--------|------|------|
| 自动修复 | 多个 | 7 |
| 手动处理 | 多个 | 8 |
| 测试 Bug | 2 文件 | 3 |

### 7.7 【代码整洁】domain/storage/ → infrastructure/storage/

移动 `inbox.rs`、`outbox.rs`、`dead_letter.rs` 到基础设施层。

### 7.8 【冗余】清理 WeChat SessionState 嵌套 RwLock

移除内层 `RwLock<HashMap>`，使用普通 `HashMap`。

---

## 八、P3 修改项（远期优化）

### 8.1 结构化并发（Task Supervision）
统一 `tokio::task::JoinSet` 管理所有后台任务。

### 8.2 消息顺序保证强化
对带序列号的平台使用序列号排序，对无序列号的平台使用向量时钟或 Lamport 时间戳。

### 8.3 数据库迁移框架
引入 `refinery` 管理 schema 版本演进。

### 8.4 配置热加载
对非关键配置项支持文件监听 + 热更新。

### 8.5 OutboxStatus 类型状态模式
使用类型状态消除非法状态转换。

---

## 九、开发计划

### 9.1 阶段划分

```
Phase A (3天) ─ P0 安全修复
  ├── A1: 恢复 HTTP API 认证 + 修复 daemon_api_auth 测试
  ├── A2: 修复熔断器 HalfOpen 卡死
  ├── A3: 审计日志 hash chain + 保留策略
  └── A4: /api/token_status 脱敏

Phase B (4天) ─ P0/P1 正确性修复
  ├── B1: 修复 Conversation 聚合边界 (Snapshot)
  ├── B2: 修复 ReorderWindow 截止时间
  ├── B3: 消除生产路径 unwrap()
  └── B4: Outbox .ok() → 错误日志

Phase C (5天) ─ P1 架构重构
  ├── C1: 拆分 main.rs
  ├── C2: 拆分 start_http_api
  ├── C3: std::sync::RwLock → tokio::sync::RwLock
  ├── C4: 核心 trait 类型化错误
  └── C5: 应用层端口化 (DB 访问)

Phase D (3天) ─ P1/P2 健壮性
  ├── D1: JoinHandle 任务监控
  ├── D2: 消除 Short-circuit 硬编码
  ├── D3: SessionState 嵌套 RwLock 清理
  └── D4: domain/storage 迁移

Phase E (5天) ─ P2 质量提升
  ├── E1: DB 连接池
  ├── E2: Dingtalk 频道
  ├── E3: 增强 /api/health
  ├── E4: HTTP API 单元测试
  └── E5: 修复全部 Clippy 问题

Phase F (远期) ─ P3
  ├── F1: 结构化并发
  ├── F2: 消息顺序强化
  ├── F3: DB 迁移框架
  ├── F4: 配置热加载
  └── F5: OutboxStatus 类型状态
```

### 9.2 详细任务分解

#### Phase A: P0 安全修复 (预计 3 天)

| ID | 任务 | 文件 | 预计 | 验收标准 |
|----|------|------|------|----------|
| A1 | 恢复 HTTP API 认证 | `runtime.rs:1382-1383`, `http_auth.rs` | 4h | `daemon_api_auth_closed_loop` 测试通过; clippy 3 个 unused 警告消失 |
| A2 | 修复熔断器 HalfOpen 卡死 | `circuit_breaker.rs:73` | 3h | 添加 `test_halfopen_timeout_transition` 测试 |
| A3 | 审计日志 hash chain | `sqlite_audit.rs`, `db.rs`, `gc_janitor.rs` | 6h | 启动时验证 hash 链完整性; 过期日志自动清理 |
| A4 | token_status 脱敏 | `runtime.rs:1117-1139` | 1h | 返回 token 仅前 4 + 后 4 字符 |

#### Phase B: P0/P1 正确性修复 (预计 4 天)

| ID | 任务 | 文件 | 预计 | 验收标准 |
|----|------|------|------|----------|
| B1 | Conversation Snapshot | `domain/value_objects/`, `conversation_store.rs`, `middleware.rs` | 5h | Pipeline 不再持有 Conversation clone; 编译通过 |
| B2 | ReorderWindow 挂钟时间 | `reorder_window.rs:42` | 4h | 添加 `test_cutoff_based_on_wall_clock` 测试 |
| B3 | 消除 unwrap() | `conversation_store.rs` (5处), `circuit_breaker.rs` (3处) | 3h | 生产路径无 `.unwrap()` |
| B4 | Outbox 错误处理 | `outbox_worker.rs` (所有 `.ok()`) | 3h | 每个状态变更都有错误日志 |

#### Phase C: P1 架构重构 (预计 5 天)

| ID | 任务 | 文件 | 预计 | 验收标准 |
|----|------|------|------|----------|
| C1 | 拆分 main.rs | 新建 `src/cli/`, `src/daemon/` | 6h | main.rs < 100 行; CLI 解析有单元测试 |
| C2 | 拆分 start_http_api | 新建 `src/infrastructure/http_api/` | 6h | 每个 handler 独立文件; 编译通过 |
| C3 | RwLock 迁移 | `conversation_store.rs` | 3h | 全部 `.read()` → `.read().await` |
| C4 | 类型化错误 | 新建 `src/domain/error.rs` | 5h | Channel/Middleware/AiBackend 使用类型化错误 |
| C5 | 应用层端口化 | `audit.rs`, `binding.rs`, `push.rs` 等 | 4h | 新建对应 domain/ports trait |

#### Phase D: P1/P2 健壮性 (预计 3 天)

| ID | 任务 | 文件 | 预计 | 验收标准 |
|----|------|------|------|----------|
| D1 | JoinSet 任务监控 | `runtime.rs` | 4h | health endpoint 报告任务状态 |
| D2 | 消除 short-circuit 硬编码 | `middleware.rs` | 1h | `is_terminal()` 方法替代字符串比较 |
| D3 | SessionState 嵌套 RwLock | `wechat/channel.rs` | 1h | `context_tokens: HashMap` |
| D4 | domain/storage 迁移 | `domain/storage/` → `infrastructure/storage/` | 2h | 编译通过，import 路径更新 |

#### Phase E: P2 质量提升 (预计 5 天)

| ID | 任务 | 文件 | 预计 | 验收标准 |
|----|------|------|------|----------|
| E1 | DB 连接池 | `db.rs` | 4h | 压测: 10 并发 worker 正常 |
| E2 | Dingtalk 频道 | `channels/dingtalk/` | 6h | 闭环测试通过 |
| E3 | 增强 /api/health | `http_api/handlers/health.rs` | 3h | 返回 CB/Bulkhead/Outbox 统计 |
| E4 | HTTP API 单元测试 | 新建 `tests/http_api_*.rs` | 4h | 覆盖 send/webhook/health 主要路径 |
| E5 | 修复全部 Clippy | 多个文件 | 4h | `cargo clippy --all-targets -- -D warnings` 零错误 |

### 9.3 里程碑时间线

```
Week 1 ─────────────────────────────────────────────
Day 1-3:  Phase A (P0 安全修复)
          Milestone: 熔断器修复 + API 认证恢复 + 审计 hash chain
          验收: cargo test 22/22 通过

Day 4-7:  Phase B (P0/P1 正确性修复)
          Milestone: Conversation Snapshot + ReorderWindow 修复 + unwrap 消除
          验收: 生产路径无 unwrap

Week 2 ─────────────────────────────────────────────
Day 8-12: Phase C (P1 架构重构)
          Milestone: main.rs 拆分 + HTTP API 拆分 + 类型化错误
          验收: main.rs < 100 行；每个新模块有测试

Day 13-15: Phase D (P1/P2 健壮性)
          Milestone: JoinSet 监控 + domain/storage 迁移
          验收: health endpoint 可见任务状态

Week 3 ─────────────────────────────────────────────
Day 16-20: Phase E (P2 质量提升)
          Milestone: 连接池 + Dingtalk + Clippy 清零
          验收: `cargo clippy --all-targets -- -D warnings` 零错误

Ongoing:   Phase F (P3 远期优化)
```

### 9.4 每个阶段的回滚策略

| 阶段 | 回滚方式 | 风险 |
|------|---------|------|
| Phase A | Git revert + 测试验证 | 低 — 均为增量修改 |
| Phase B | 逐步提交 (每个任务独立 commit) | 中 — 涉及核心域逻辑 |
| Phase C | feature branch + PR review 后合并 | 高 — 文件拆分影响范围大 |
| Phase D | feature branch + 逐个合并 | 低 |
| Phase E | feature branch + 逐个合并 | 中 — 连接池变更需压测 |

### 9.5 有意保留的现状（不修改）

1. **`validate_feishu_config` panic 设计** — fail-fast 策略，配置错误应阻止启动
2. **WeChat iLink string-based JSON 响应** — iLink API 格式不稳定，保留 `serde_json::Value` 的灵活性
3. **PipelineContext 包含 AppConfig clone** — 保证 worker GC 回收时持有有效配置
4. **BackpressureConfig 未完全使用** — 当前 `DropNewest` 是正确选择，Block 和 DropOldest 保留为未来扩展点
5. **MessageContent 的 Image/File/Unknown 变体** — 接口已定义，实现按 Phase 3-4 计划推进

---

## 十、三层元认知总结

```
+-- Layer 1: 并发/语法 (Rust 编译期)
|   Findings: 熔断器状态机 Bug, RwLock unwrap panic 级联, .ok() 错误吞没
|   Status: 无编译错误，10 clippy warnings
|       ^
+-- Layer 3: 领域约束 (DDD + 云原生)
|   Constraints: RouteKey 隔离, 可恢复传递, 审计不可变, 弹性自愈
|   Status: 核心约束满足，聚合边界 + ReorderWindow 需修复
|       v
+-- Layer 2: 设计模式 (架构)
    Decisions: 六边形架构, Pipeline, Decorator, Registry, State Machine
    Status: 架构正确，main.rs/start_http_api 过大需重构
```

---

*最后更新: 2026-06-25 | 分析工具: Rust Skills v2.1.0 + Clippy + cargo test*
