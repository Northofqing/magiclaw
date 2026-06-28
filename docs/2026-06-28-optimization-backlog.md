# 优化机会清单（Optimization Backlog）

> **最后更新**: 2026-06-28
> **主要来源**: `superpowers/plans/code-analysis-modification-plan.md` v3（22 项）+ `implementation-gap-solution.md` 根因 + `code-analysis` 三层元认知
> **优先级口径**: 🔴 P0（安全/正确性/红线违规）/ 🟡 P1（架构/可维护性）/ 🟢 P2（质量/性能）/ ⚪ P3（远期）

> **重要**: 所有 P0 项同时收录到 `2026-06-28-bugs.md`，因为它们违反了红线或正确性约束。本文件侧重**优化**视角（结构性、可维护性、性能）。

---

## 1. 🔴 P0 优化（安全 / 正确性 / 红线违规）

> 完整列表与修复细节见 `2026-06-28-bugs.md`。此处仅列优化路径。

### OP-P0-1 HTTP API 鉴权恢复

- **来源**: `code-analysis-modification-plan.md` §5.1 + `tests/daemon_api_auth_closed_loop.rs` 失败
- **影响**: REST API 无认证，违反红线 2.4
- **优化路径**:
  1. 恢复 `runtime.rs:1382-1383` 的 bearer auth（解开注释）
  2. `/api/send` / `/api/window_status` / `/api/token_status` 启用 token 校验
  3. `/api/health` 保持公开（运维探针）
  4. `/api/feishu/webhook` 仅用 HMAC-SHA256 验签（不叠加 bearer）
  5. 用环境变量 `MAGICLAW_API_AUTH_ENABLED` 控制（默认 true）
  6. 修复 clippy 警告：`unused import: from_fn_with_state`、`require_bearer_auth`、`unused variable: auth`
- **工作量**: 4h
- **关联**: 动态 API 鉴权设计 (`2026-06-20-dynamic-api-auth-design.md`) 已提供模型

### OP-P0-2 熔断器 HalfOpen 超时自恢复

- **来源**: `code-analysis-modification-plan.md` §5.2
- **位置**: `src/core/resilience/circuit_breaker.rs:73`
- **影响**: HalfOpen 状态永久卡死，违反云原生韧性约束
- **优化路径**: 在 `allow_request()` 的 HalfOpen 分支添加 `opened_at` 超时检查 → 超时后重置 `failure_count` → 转回 Open
- **工作量**: 3h
- **关联**: ResilienceGate 测试 (`tests/resilience_closed_loop.rs`)

### OP-P0-3 审计日志 hash chain + 保留策略

- **来源**: `code-analysis-modification-plan.md` §5.3 + CLAUDE.md 红线 2.6
- **影响**: 当前 audit_log 表结构已有 prev_hash，但保留期清理与启动完整性校验需补
- **优化路径**:
  1. 添加 `audit_log_retention_days` 配置（默认 1827 = 5 年 + 2 天）
  2. 在 GC Janitor 中加 audit 过期清理
  3. 启动时 hash chain 校验（README 已显示有"[ERROR] audit log chain integrity check FAILED"，需确认实现完整）
- **工作量**: 6h

### OP-P0-4 `/api/token_status` 脱敏

- **来源**: `code-analysis-modification-plan.md` §5.4
- **位置**: `src/infrastructure/runtime.rs:1117-1139`
- **影响**: 当前无认证保护，可枚举所有 peer token 年龄
- **优化路径**:
  1. 与 OP-P0-1 同步加 bearer auth
  2. token 前缀脱敏（前 4 + 后 4）
- **工作量**: 1h

---

## 2. 🟡 P1 优化（架构 / 可维护性）

### OP-P1-1 拆分 main.rs（1534 行 → <100 行）

- **来源**: `code-analysis-modification-plan.md` §6.1
- **现状**: `src/main.rs` 1097 行，包含 QR 登录 / CLI 解析 / 配置加载 / feishu 校验 / daemon 启动 / push / binding 等
- **优化路径**:
  ```
  src/
  ├── main.rs                     (~50 行)
  ├── cli/
  │   ├── parser.rs               CLI 参数解析
  │   ├── commands.rs             命令枚举
  │   ├── wechat_login.rs         QR 登录流程
  │   └── feishu_send.rs          CLI 发送
  └── daemon/
      ├── singleton.rs            进程锁
      └── config_loader.rs        配置加载
  ```
- **工作量**: 6h
- **预期收益**: 主入口可读性、单元测试覆盖度

### OP-P1-2 拆分 `start_http_api`（runtime.rs 含 ~900 行）

- **来源**: `code-analysis-modification-plan.md` §6.2
- **现状**: `src/infrastructure/runtime.rs` 1574 行
- **优化路径**:
  ```
  src/infrastructure/http_api/
  ├── mod.rs                 路由组装
  ├── state.rs               HttpApiState
  ├── handlers/
  │   ├── send.rs
  │   ├── health.rs
  │   ├── feishu_webhook.rs
  │   ├── token_status.rs
  │   └── window_status.rs
  └── long_poll.rs
  ```
- **工作量**: 6h

### OP-P1-3 `std::sync::RwLock` → `tokio::sync::RwLock`

- **来源**: `code-analysis-modification-plan.md` §6.3
- **位置**: `src/adapters/conversation_store.rs:41`
- **影响**: 在 async 上下文持同步锁 → 阻塞 runtime 线程
- **优化路径**: 全部 `.read()` → `.read().await`，`.write()` → `.write().await`
- **工作量**: 3h

### OP-P1-4 核心 trait 类型化错误

- **来源**: `code-analysis-modification-plan.md` §6.6
- **位置**: `src/channels/channel_trait.rs`、`src/core/pipeline/middleware.rs`、`src/core/ai/backend.rs`
- **影响**: 当前所有 trait 返回 `Result<_, String>`，调用方只能 `is_ret_minus_2_error()` 字符串匹配
- **优化路径**:
  ```rust
  // src/domain/error.rs
  pub enum ChannelError {
      Transport(String),
      AuthExpired { errcode: i32 },
      RateLimited { retry_after_secs: u64 },
      InvalidRecipient(String),
      ContentRejected(String),
  }
  // ...类似的 PipelineError / AiError
  ```
- **工作量**: 5h（约 40+ 函数签名变更）

### OP-P1-5 应用层通过端口访问 DB

- **来源**: `code-analysis-modification-plan.md` §6.10
- **位置**: `application/audit.rs`、`agent_preferences.rs`、`binding.rs`、`push.rs`
- **影响**: 这些模块直接接受 `DbPool`，绕过 `domain/ports/`，违反依赖反转
- **优化路径**: 为每类数据访问定义端口 trait，通过依赖注入传递
- **工作量**: 4h

### OP-P1-6 JoinSet 任务监控

- **来源**: `code-analysis-modification-plan.md` §5（隐性）+ §6.9
- **位置**: `src/infrastructure/runtime.rs` 369/378/404 行所有 `tokio::spawn`
- **影响**: 后台任务 panic 静默，尤其是 outbox_worker 崩溃会导致所有外发停止
- **优化路径**:
  ```rust
  pub struct TaskSupervisor {
      handles: tokio::task::JoinSet<()>,
  }
  // health endpoint 报告每个任务状态
  ```
- **工作量**: 4h
- **关联**: README 已提到 `/api/health` 返回 `tasks.running/finished_count/finished`，需确认覆盖全部任务

### OP-P1-7 Pipeline Middleware `is_terminal()` 抽象

- **来源**: `code-analysis-modification-plan.md` §7.4
- **位置**: `src/core/pipeline/middleware.rs:52-57`
- **影响**: 当前用字符串比较判断 short-circuit
- **优化路径**:
  ```rust
  pub trait Middleware: Send + Sync {
      fn name(&self) -> &'static str;
      fn is_terminal(&self) -> bool { false }
      async fn process(&self, ctx: PipelineContext) -> Result<PipelineContext, PipelineError>;
  }
  ```
- **工作量**: 1h

### OP-P1-8 WeChat SessionState 嵌套 RwLock 清理

- **来源**: `code-analysis-modification-plan.md` §7.8
- **位置**: `src/channels/wechat/channel.rs:19,24-35`
- **影响**: 外层 `Mutex<SessionState>` 已串行化，内层 `RwLock<HashMap>` 仅增加开销
- **优化路径**: 内层改为 `HashMap<String, String>`
- **工作量**: 1h

---

## 3. 🟢 P2 优化（质量 / 性能）

### OP-P2-1 Conversation 聚合边界修复（Snapshot）

- **来源**: `code-analysis-modification-plan.md` §5（隐性）+ §6.4
- **位置**: `src/adapters/conversation_store.rs:59-79`
- **影响**: Conversation clone 泄漏到 Pipeline，违反 DDD 聚合边界（P0 性质）
- **优化路径**:
  ```rust
  pub struct ConversationSnapshot {
      pub route_key: RouteKey,
      pub conversation_id: String,
      pub peer_id: String,
      pub conversation_type: ConversationType,
      pub message_count: u64,
      pub last_active_at: i64,
  }
  // PipelineContext.conversation: Conversation → conversation_snapshot: ConversationSnapshot
  ```
- **工作量**: 5h

### OP-P2-2 ReorderWindow 挂钟时间

- **来源**: `code-analysis-modification-plan.md` §6.5
- **位置**: `src/domain/services/reorder_window.rs:42`
- **影响**: 截止时间从 sequence 推导，高 sequence 跳跃导致中间消息提前刷新
- **优化路径**: 记录 `Instant::now()`，使用挂钟时间计算 cutoff
- **工作量**: 4h

### OP-P2-3 消除生产路径 `.unwrap()`

- **来源**: `code-analysis-modification-plan.md` §6.7
- **位置**: `src/adapters/conversation_store.rs:213,221,259,276,286` + `src/core/resilience/circuit_breaker.rs` 等
- **影响**: 锁中毒 → panic → 级联崩溃
- **优化路径**:
  - 短期：`.unwrap_or_else(|e| e.into_inner())`
  - 长期：迁移到 `parking_lot::Mutex` / `RwLock`
- **工作量**: 3h

### OP-P2-4 Outbox `.ok()` → 错误日志

- **来源**: `code-analysis-modification-plan.md` §6.8
- **位置**: `src/application/outbox_worker.rs` 多处 `.ok()`
- **影响**: 状态变更失败时静默 → 重复投递风险
- **优化路径**:
  ```rust
  if let Err(e) = outbox.mark_status(&id, OutboxStatus::Sending, None) {
      tracing::error!(message_id = %id, error = %e, "failed to mark sending");
      return;
  }
  ```
- **工作量**: 3h

### OP-P2-5 Dingtalk 完整闭环

- **来源**: `code-analysis-modification-plan.md` §7.2
- **现状**: `tests/dingtalk_closed_loop.rs` 通过但能力受限
- **优化路径**:
  1. access_token 获取（已有）
  2. 消息类型映射（文本/图片/文件/Markdown）
  3. 错误语义映射（参考 FeishuErrorSemantics）
  4. 多账号 / 隔离测试
- **工作量**: 6h

### OP-P2-6 HTTP API 处理器单元测试

- **来源**: `code-analysis-modification-plan.md` §7.5
- **现状**: 已存在 `tests/http_api_unit.rs`（10 用例），但 start_http_api 拆分后需补 handler 单元测试
- **优化路径**: 提取路由构建 → `axum::test` 辅助 → 覆盖正常 / token 缺失 / ret=-2 / webhook challenge
- **工作量**: 4h

### OP-P2-7 修复全部 Clippy 问题

- **来源**: `code-analysis-modification-plan.md` §3
- **当前**: 10 errors (lib) + 12 errors (tests)
- **优化路径**:
  - 自动修复 7 项（unused import / lazy_evaluations / manual_is_multiple_of 等）
  - 手动修复 8 项（large_enum_variant / too_many_arguments 等）
  - 测试 Bug 3 项（含 `wrong_result || true` 永远为 true 的逻辑 Bug）
- **工作量**: 4h

### OP-P2-8 `domain/storage/` → `infrastructure/storage/`

- **来源**: `code-analysis-modification-plan.md` §7.7
- **影响**: 当前 `domain/storage/inbox.rs` 等是 DB 行结构，不是领域概念
- **优化路径**: 移动 + 更新 import
- **工作量**: 2h

### OP-P2-9 `/api/health` 增强

- **来源**: `code-analysis-modification-plan.md` §7.3
- **现状**: README 已显示 `/api/health` 返回 CB / Bulkhead / Feishu 状态
- **可继续增强**:
  - Worker 统计（处理速率、平均延迟）
  - DB 状态（连接池利用率、WAL 模式）
  - Outbox 按状态分布
- **工作量**: 3h

---

## 4. ⚪ P3 优化（远期）

| ID | 项目 | 来源 |
|----|------|------|
| OP-P3-1 | 结构化并发：统一 `tokio::task::JoinSet` 管理后台任务 | code-analysis §8.1 |
| OP-P3-2 | 消息顺序强化：带 sequence 平台按 seq，无 sequence 平台用向量时钟 / Lamport | code-analysis §8.2 |
| OP-P3-3 | 数据库迁移框架：`refinery` 管理 schema 版本演进 | code-analysis §8.3 |
| OP-P3-4 | 配置热加载：非关键配置支持文件监听 + 热更新 | code-analysis §8.4 |
| OP-P3-5 | OutboxStatus 类型状态模式：使用类型状态消除非法转换 | code-analysis §8.5 |

---

## 5. 文档 / 流程优化

### OP-DOC-1 文档交叉引用

- **现状**: 各设计文档间偶有重复 / 矛盾
- **优化路径**:
  - `phase1-plan.md` 任务 ID（D1-U4）应映射到当前 `src/` 模块路径
  - `phase2-architecture.md` §3 端口 trait 应映射到 `src/domain/ports/` 实际文件
  - `2026-06-19-project-binding-and-multi-push-design.md` §6.0 提及 `get_bot_qrcode` 已在 ilink 层存在但未串成运行时闭环 — 应在 README 状态表标注

### OP-DOC-2 添加"能力分级"到所有 PR/设计

- **依据**: AGENTS.md §0.4 — 所有新增能力必须标 `closed` / `experimental` / `stub`
- **优化路径**:
  - 在每个 `docs/*-design.md` 顶部添加「能力分级目标 / 当前」字段
  - 在每个 PR 描述添加 `closed / experimental / stub` 清单
- **当前缺口**: `2026-06-19-user-agent-preferences-design.md`、`2026-06-19-project-binding-and-multi-push-design.md`、`2026-06-20-dynamic-api-auth-design.md` 缺少当前分级标注

### OP-DOC-3 整合文档体系

- **已完成（本轮）**:
  - `2026-06-28-integration-index.md` — 文档地图
  - `2026-06-28-development-status.md` — 完成度评估
  - `2026-06-28-optimization-backlog.md` — 优化清单（本文）
  - `2026-06-28-bugs.md` — Bug 清单
- **后续**:
  - `docs/old/` 真正落地历史归档（当前不存在）
  - 阶段完成后归档到 `docs/archive/`（避免主索引过载）

---

## 6. 性能 / 容量优化

### OP-PERF-1 DbPool 进一步调优

- **现状**: `code-analysis-modification-plan.md` 报告 DbPool 已从单连接升级为连接池（commit 6a35152），`MAGICLAW_DB_POOL_SIZE` 默认 `max(4, num_cpus)`
- **可继续**:
  - WAL checkpoint 调度（避免长事务阻塞）
  - 连接超时与重试策略
  - 池满时 backpressure 行为

### OP-PERF-2 ReorderWindow 边界 case 优化

- **边界**: 大幅 sequence 跳跃（已修复）+ 相同 timestamp + 负值
- **优化路径**: D6 单元测试扩展

### OP-PERF-3 媒体上传独立隔离舱

- **来源**: `media-streaming-upload-design.md` §3.2（挑战 B3）
- **现状**: 设计阶段，独立 BulkheadPools.media（默认并发 4）+ `media_upload_timeout_ms`（默认 60s）
- **优化路径**: 实现 + 压测

---

## 7. 安全 / 合规优化

### OP-SEC-1 WeChat App Secret 保护

- **来源**: `2026-06-23-feishu-push-architecture.md` §8
- **优化路径**:
  - App Secret 绝不入日志 / 错误消息
  - 定期轮换
  - 仅在内存中持有，不写 SQLite

### OP-SEC-2 Permission 中间件真实白名单

- **来源**: `mcp-deployment.md` §10
- **现状**: 当前 Permission 是占位放行
- **优化路径**: 真实白名单门控（白名单内 peer 才进入 AI 步骤）

### OP-SEC-3 Token 输出消毒

- **来源**: `claude-code-backend-design.md` §5.4
- **优化路径**:
  - Claude 输出：长度上限 + 去控制字符
  - 通用后端：相同约束
  - 通用审计：stderr 不外泄到 MCP 响应

---

## 8. 推荐实施顺序

1. **OP-P0-1 ~ OP-P0-4**（安全 / 红线，3-7 天）
2. **OP-P2-1 ~ OP-P2-2**（聚合边界 + ReorderWindow，2 天）— 与 P0 并行
3. **OP-P2-4**（outbox 错误处理，1 天）
4. **OP-P1-3 / OP-P1-8**（同步锁 + 嵌套锁，0.5 天）
5. **OP-P2-3**（消除 unwrap，1 天）
6. **OP-P1-6**（JoinSet 监控，1 天）
7. **OP-P1-1 / OP-P1-2**（main.rs + HTTP API 拆分，3 天）
8. **OP-P1-4**（类型化错误，2 天）
9. **OP-P2-5**（Dingtalk 闭环，2 天）
10. **OP-P2-7**（Clippy 清零，1 天）

剩余 P1/P2/P3 项可在不阻塞主线的情况下持续推进。

---

## 9. 与 2026-06-28-development-status.md / 2026-06-28-bugs.md 的对应关系

| 类型 | 去向 |
|------|------|
| 违反红线 / 正确性缺陷 | → `2026-06-28-bugs.md`（同时在本文件保留链接） |
| 已 `closed` 但代码结构差 | → 本文件（OP-P1-1 ~ OP-P1-2 拆分类） |
| 已实现但需补测试 / 性能 | → 本文件（OP-P2-* 性能 / Clippy） |
| 远期 / 预留 | → 本文件（OP-P3-*） |