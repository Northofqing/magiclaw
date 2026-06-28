# 开发完成度评估

> **最后更新**: 2026-06-28
> **评估依据**: 各阶段计划文档（`phase1-plan.md`、`phase2-architecture.md`、`code-analysis-modification-plan.md`）+ `README.md` + `superpowers/feishu-push-pr-checklist.md` + 代码扫描
> **判定标准**: AGENTS.md §0.4 — `closed` 才能计入完成度，`experimental` / `stub` 不计入

---

## 0. 状态图例

| 标记 | 含义 |
|------|------|
| ✅ `closed` | 已接入主链路 + 具备闭环测试 + 满足阶段红线 |
| 🟡 `experimental` | 已接线但未满足生产级约束，**不计入**完成度 |
| ⛔ `stub` | 仅占位 / 报错 / 返回 preview，**不计入**完成度 |
| ⏳ `planned` | 设计已评审，未进入实施 |
| ❌ `not-started` | 尚未启动 |

---

## 1. 阶段完成度总览

| 阶段 | 目标 | 当前状态 | 完成度 |
|------|------|----------|--------|
| Phase 1 | 稳定内核（RouteKey/Idle GC/Dedup/Reorder/sync_buf） | 已落地，Post-Phase-A 已演进到 DDD 模块化 | ~95% |
| Phase 1.5 | MCP Adapter（send/list_peers/login + stdout 零污染） | `send` closed；`list_peers` / `login` experimental | ~75% |
| Phase 2 | 可恢复投递（Inbox/Outbox/DLQ/恢复） | 主闭环已落地，SQLite + crash_recovery 已运行 | ~90% |
| Phase 3 | 多信道扩展（Feishu/Dingtalk） | Feishu Phase A closed；Dingtalk 完整闭环 | ~70% |
| Phase 4 | Pipeline + AI 可插拔 | Pipeline/AI 已实现，6 个后端 | ~85% |
| Phase 5 | 生产强化（Circuit Breaker/Bulkhead/审计工具） | CB/Bulkhead 已实现，部分可观测性已落地 | ~70% |
| Phase 6 (新增) | P0 安全/正确性修复 | `code-analysis-modification-plan.md` Phase A/B | **未启动** |
| Phase 7 (新增) | 项目绑定 + 推送 | 设计已落地，`experimental` | 设计 |

> 注：参考 `code-analysis-modification-plan.md` 把修复工作重命名为 Phase A-F（注意它与 7 步流程阶段 A/B/C 不同）。

---

## 2. 各阶段详细状态

### 2.1 Phase 1 — 稳定内核（✅ ~95%）

| 能力 | 计划位置 | 实际状态 | 证据 |
|------|---------|---------|------|
| RouteKey 含 conversation 维度 | `phase1-architecture.md` §2.3 | ✅ closed | `src/domain/value_objects/route_key.rs` |
| Conversation 一等聚合根 | `phase1-architecture.md` §2.1 | ✅ closed | `src/domain/aggregates/conversation.rs` |
| 同 RouteKey 串行 | `phase1-architecture.md` §3 | ✅ closed | `src/adapters/conversation_store.rs` |
| 30min Idle GC | `phase1-architecture.md` §4.2 | ✅ closed | `src/application/gc_janitor.rs` |
| Dedup TTL（moka） | `phase1-architecture.md` §4.3 | ✅ closed | `src/adapters/moka_dedup.rs` |
| Reorder Window | `phase1-architecture.md` §4.4 | 🟡 存在缺陷 | `src/domain/services/reorder_window.rs:42` — **截止时间从 sequence 推导，非挂钟**，有 Bug（见 2026-06-28-bugs.md #B1） |
| WeChat sync_buf 持久化 | `phase1-architecture.md` §4.5 | ✅ closed | `src/adapters/sqlite_sync_buf.rs` |
| AES-128-ECB/PKCS7 | `phase1-architecture.md` §4.6 | ✅ closed | `src/channels/wechat/ilink.rs`（contract test 通过） |
| ilink 契约测试 | `phase1-architecture.md` §4.6 | ✅ closed | 已知向量测试 |
| 60s Janitor 间隔 | `phase1-architecture.md` §4.2 | ✅ closed | `gc_janitor.rs` |

**遗留**: ReorderWindow 计算错误（P1）+ Conversation 聚合边界被破坏（clone，P0 #B2）。

---

### 2.2 Phase 1.5 — MCP Adapter（🟡 ~75%）

| 能力 | 状态 | 证据 |
|------|------|------|
| `transport: stdio + JSON-RPC 2.0` | ✅ closed | `src/infrastructure/runtime.rs` MCP 模式 |
| `Content-Length` framing（输入兼容，输出统一） | ✅ closed | `mcp-deployment.md` §1 |
| `stdout` 零污染 | ✅ closed | 业务日志全部走 stderr/file |
| `initialize` | ✅ closed | `mcp-deployment.md` §6 |
| `tools/list` | ✅ closed | 返回 send/list_peers/login |
| `tools/call: send` | ✅ closed | 入 Outbox + 后台投递 |
| `tools/call: list_peers` | 🟡 experimental | 仅 wechat，从本地通道目录读取 |
| `tools/call: login` | 🟡 experimental | 检查 WeChat 账号配置就绪度 |

**测试**: `tests/mcp_stdio_closed_loop.rs` ✅

---

### 2.3 Phase 2 — 可恢复投递（✅ ~90%）

| 能力 | 状态 | 证据 |
|------|------|------|
| SQLite `inbox` / `outbox` / `dead_letter` 表 | ✅ closed | `src/infrastructure/db.rs` 自动建表 |
| Outbox 状态机 `pending → sending → sent` | ✅ closed | `src/application/outbox_worker.rs` |
| `sending → retrying → sent` | ✅ closed | 同上 |
| `retrying → dead_letter` | ✅ closed | 超阈值入 DLQ |
| 指数退避 + jitter | ✅ closed | `phase2-architecture.md` §2.2 |
| 崩溃恢复 `recover_after_crash` | ✅ closed | `src/application/crash_recovery.rs` |
| DLQ 重放 | ✅ closed | `src/application/dlq_manager.rs` |
| Audit hash chain | ✅ closed | `src/adapters/sqlite_audit.rs` |
| Audit 启动时完整性校验 | ✅ closed | `README.md` 提到 "[ERROR] audit log chain integrity check FAILED" |

**遗留**:
- `outbox_worker.rs` 多处 `.ok()` 静默吞错误（P1 #B4 → 2026-06-28-bugs.md #B5）
- `OutboxEntry.route_key` 存 JSON String（P2 #B12）

---

### 2.4 Phase 3 — 多信道扩展（🟡 ~70%）

| 信道 | 能力 | 状态 | 证据 |
|------|------|------|------|
| **WeChat** | send（文本） | ✅ closed | `src/channels/wechat/` |
| | send（媒体） | 🟡 experimental | port 落地，真实 ilink adapter 待契约 |
| | long-poll → 主链路 | ✅ closed | `src/channels/wechat/channel.rs` |
| | context_token 多用户 HashMap | ✅ closed（commit 5ea6e95） | `p0-fixes-implementation-report.md` |
| | SessionExpired (-14) 检测 | ✅ closed | `src/channels/wechat/ilink.rs` |
| | token 持久化 | ✅ closed | `src/adapters/sqlite_context_tokens.rs` |
| **Feishu** | webhook ingress + 签名验证 | ✅ closed | Phase A Task 3 |
| | send（文本） | ✅ closed | `feishu_webhook_closed_loop.rs` |
| | send（媒体 + 流式） | ✅ closed | `feishu_media_retry_dlq_closed_loop.rs` |
| | 多账号隔离 | ✅ closed | `feishu_multi_account_isolation_closed_loop.rs` |
| | 错误语义（retry vs terminal） | ✅ closed | `src/channels/feishu/error_semantics.rs` |
| | 启动配置校验 | ✅ closed | Phase A Task 2 |
| | 健康检查 | ✅ closed | `/api/health` 含 feishu 块 |
| | long-connection adapter | ⛔ stub | Phase B 待实现 |
| **Dingtalk** | send + health | ✅ closed | `tests/dingtalk_closed_loop.rs` |

**遗留**:
- Feishu long-connection adapter 未实现（Phase B Task 7）
- Dingtalk 多账号 / 隔离验证缺失
- WeChat ilink 媒体契约未确定，IlinkMediaUploader 待升 `closed`

---

### 2.5 Phase 4 — Pipeline + AI 可插拔（✅ ~85%）

| 能力 | 状态 | 证据 |
|------|------|------|
| Middleware Chain（Normalize→Permission→RateLimit→AgentCommand→AI→Outbox） | ✅ closed | `src/core/pipeline/` |
| PipelineContext 含 Conversation | ⚠️ 聚合边界破坏 | clone 而非 snapshot（P0 #B2） |
| AI Backend 抽象 | ✅ closed | `src/core/ai/backend.rs` |
| `echo` 后端 | ✅ closed | 默认 |
| `claude_code` 后端 | ✅ closed | `src/core/ai/claude_code.rs` |
| `codex` 后端 | ✅ closed | `tests/cli_agent_backend_closed_loop.rs` |
| `copilot` 后端 | 🟡 experimental | 公版 CLI 未在本机验证 |
| `claude` (Anthropic API) 后端 | ⛔ stub | `src/core/ai/claude.rs` 仅返回占位 |
| 自定义 CLI agent（hermes/openclaw/任意） | 🟡 experimental | 框架就绪但本机未验证 |
| 失败降级 echo | ✅ closed | `claude_code.rs` + `cli_agent.rs` |
| Circuit Breaker 包裹 | ✅ closed | `ResilientAiBackend` |
| Bulkhead 隔离 | ✅ closed | AI 池 5 并发 / Send 池 50 并发 |
| AgentCommand 别名（cc/cx/oc/h） | ✅ closed | `src/core/pipeline/agent_command.rs` |
| UserAgentPreferences 持久化 | ✅ closed | `src/application/agent_preferences.rs` |
| RateLimit（仅非 echo 后端启用） | ✅ closed | `src/core/pipeline/rate_limit.rs` |

**遗留**:
- `claude.rs` API stub 始终未实现
- `Permission` 中间件仍为占位放行（`mcp-deployment.md` §10 明确披露）

---

### 2.6 Phase 5 — 生产强化（🟡 ~70%）

| 能力 | 状态 | 证据 |
|------|------|------|
| Circuit Breaker | ⚠️ HalfOpen 永久卡死 | `src/core/resilience/circuit_breaker.rs:73`（P0 #B3 → 2026-06-28-bugs.md #B3） |
| Bulkhead | ✅ closed | `ResilienceGate` |
| SQLite 连接池 | ✅ closed（recent commit） | `tests/db_pool_closed_loop.rs` |
| daemon 模式 + 单例锁 | ✅ closed | `src/daemon/singleton.rs` |
| HTTP API | ✅ closed | `src/infrastructure/runtime.rs` |
| `/api/health`（含 CB / Bulkhead / Feishu 状态） | ✅ closed | `tests/health_resilience_closed_loop.rs` |
| `/api/send` / `/api/window_status` / `/api/token_status` | ⚠️ 鉴权被注释 | `daemon_api_auth_closed_loop.rs` 失败（P0 #B6 → 2026-06-28-bugs.md #B6） |
| `auth issue/list/revoke` CLI | ✅ closed | 多项目 token 注册 |
| Audit 不可篡改（hash chain） | ✅ closed | `tests/audit_chain_closed_loop.rs` |
| Audit 保留期 ≥ 5 年 | 🟡 部分 | 表结构就绪，保留期清理策略未实现 |
| 业务日志走 stderr/file | ✅ closed | MCP 零污染已验证 |
| 媒体流式上传（port 层） | ✅ closed | `src/domain/ports/media_*.rs` |
| 真实 ilink 媒体上传 | 🟡 experimental | 契约待确认 |
| 高风险操作写 audit_log | ✅ closed | `audit_log` action 覆盖 send/ai_generate/auth 等 |
| `/api/feishu/webhook` HMAC-SHA256 验签 | ✅ closed | `feishu_webhook_security_closed_loop.rs` |

---

### 2.7 阶段 6/7 — 修复 + 新业务能力

| 能力 | 状态 | 来源 |
|------|------|------|
| 项目绑定中心（projects / delivery_targets / project_bindings） | ⏳ planned | `2026-06-19-project-binding-and-multi-push-design.md` Phase A |
| JSONL/CSV 绑定导入 | ⏳ planned | 同上 §8.1 |
| JSONL/CSV 推送导入 | ⏳ planned | 同上 |
| 广播 / 定向推送（写 outbox） | ⏳ planned | 同上 §7 |
| 扫码登录闭环 | ⏳ planned | 同上 §6.0（依赖 ilink 登录） |
| 绑定码自动绑定 | ⏳ planned | 同上 §6.2 Phase B |

---

## 3. 测试覆盖快照

| 类别 | 位置 | 数量 |
|------|------|------|
| 单元测试 | `src/**/*.rs` 的 `#[cfg(test)] mod tests` | ~50 |
| 闭环测试 | `tests/*_closed_loop.rs` | 22 个文件 |
| HTTP API 测试 | `tests/http_api_unit.rs` | ✅ |
| DB 连接池并发 | `tests/db_pool_closed_loop.rs` | ✅ |
| 审计 hash chain | `tests/audit_chain_closed_loop.rs` | ✅ |
| Dingtalk 完整路径 | `tests/dingtalk_closed_loop.rs` | ✅ |
| 健康检查韧性 | `tests/health_resilience_closed_loop.rs` | ✅ |
| Feishu 6 闭环测试 | feishu_*_closed_loop.rs | ✅ |
| CLI Agent | `tests/cli_agent_backend_closed_loop.rs` | ✅ |
| Claude Code | `tests/claude_code_backend_closed_loop.rs` | ✅ |
| **失败测试** | `tests/daemon_api_auth_closed_loop.rs` | ❌ **FAIL — API 认证被注释** |

**README 声称** 285 passing / 0 clippy warnings；**code-analysis** 报告 21/22 passing / 10 clippy errors（lib）/ 12 errors（tests）。两者口径不一致，需以 `cargo test` 实际运行为准。

---

## 4. 与 v5 阶段验收清单的对照

来源: `RUST_MIGRATION_V5.md` §11

| 红线 | 计划阶段 | 当前状态 |
|------|---------|---------|
| RouteKey 含 conversation 维度 | Phase 1 | ✅ |
| 同 RouteKey 串行 / 跨 RouteKey 并行 | Phase 1 | ✅ |
| 30 分钟 idle route 自动回收 | Phase 1 | ✅ |
| Dedup TTL cache（无全表 retain） | Phase 1 | ✅ |
| 乱序场景按策略重排/幂等 | Phase 1 | ⚠️ **ReorderWindow 计算错误**，需修复 |
| sync_buf 持久化并重启可恢复 | Phase 1 | ✅ |
| MCP 最小接口可用 | Phase 1.5 | 🟡 send closed，list_peers/login experimental |
| stdout 无业务日志污染 | Phase 1.5 | ✅ |
| Outbox 状态机可观测 | Phase 2 | ✅ |
| 失败重试与 DLQ 可用 | Phase 2 | ✅ |
| 进程崩溃后可恢复未完成发送 | Phase 2 | ✅ |
| 双信道并行运行且隔离 | Phase 3 | 🟡 Feishu ✅ / Dingtalk 隔离未充分验证 |
| 单信道故障不影响其他信道 | Phase 3 | ✅（ChannelRegistry 隔离） |
| Middleware 可插拔 | Phase 4 | ✅ |
| AI backend 可切换并可降级到 echo | Phase 4 | ✅ |
| 熔断、隔离舱、DLQ 运维闭环完整 | Phase 5 | ⚠️ **CB HalfOpen 卡死**，需修复 |
| 服务化、监控、审计可用于生产 | Phase 5 | 🟡 部分 |

**结论**: 17 条 P0 红线 / 12 ✅ / 3 ⚠️（ReorderWindow、CB HalfOpen、MCP list_peers/login）/ 2 🟡。

---

## 5. 与 superpowers feishu-push-pr-checklist 的对照

| Gate | 状态 |
|------|------|
| Gate A — webhook-first closed（签名 + 多账号隔离 + 错误语义 + 闭环测试） | ✅ 全部通过 |
| Gate B — long-connection experimental | ⛔ 未启动 |
| Gate C — production closed（CB + 恢复 + 观测） | 🟡 CB 有 Bug，恢复已实现 |

---

## 6. 当前 PR/已落地能力清单（按 `closed` 排序）

### 已合并（commit 历史可见）

1. ✅ Phase A Feishu Tasks 2-5（PR_PHASE_A_TASKS_2_5）
2. ✅ WeChat Token P0 修复（commit 5ea6e95，p0-fixes-implementation-report.md）
3. ✅ WeChat QR 登录 + daemon bootstrap（PR_DESCRIPTION.md）
4. ✅ Claude Code AI 后端（claude-code-backend-design.md，已 closed）
5. ✅ DB 连接池（commit 6a35152，db_pool_closed_loop.rs）
6. ✅ `/api/health` 韧性增强（commit c080c9a）
7. ✅ HTTP API 单元测试（commit 4269c22）

### 待启动 / 部分完成

1. ⛔ Phase A 安全修复（code-analysis-modification-plan Phase A：API 鉴权恢复 + CB HalfOpen + 审计 hash chain + token 脱敏）
2. ⛔ Phase B 正确性修复（Conversation Snapshot + ReorderWindow 修复 + 消除 unwrap + outbox 错误处理）
3. ⛔ Phase C 架构重构（main.rs 拆分 + HTTP API 拆分 + 类型化错误）
4. ⛔ Feishu long-connection adapter（Phase B Task 7）
5. ⏳ 项目绑定 + 多人推送（2026-06-19 设计已落地，实施未启动）

---

## 7. 关键差距

1. **API 鉴权被注释** — `daemon_api_auth_closed_loop.rs` 失败，`runtime.rs:1382-1383` 注释掉了 bearer auth。违反红线 2.4「REST Adapter 必须启用最小认证」。
2. **熔断器 HalfOpen 永久卡死** — 违反红线 2.5「Circuit Breaker 必须有自恢复路径」。
3. **ReorderWindow 截止时间计算错误** — 违反红线 2.2「乱序场景按策略重排」中的"挂钟窗口"语义。
4. **聚合边界被破坏** — Conversation clone 泄漏到 Pipeline，违反 DDD「聚合外部只能通过聚合根方法修改」。
5. **无 JoinHandle 监控** — 后台任务 panic 静默，尤其是 outbox_worker 崩溃会导致所有外发停止。
6. **Dingtalk 隔离 / 多账号** 闭环测试不足。
7. **Feishu long-connection adapter** 完全未实现。
8. **媒体上传真实 ilink adapter** 待契约确认才能标 `closed`。
9. **`Permission` 中间件仍是占位放行** — 启用非 echo AI 后端时入站消息全部进入 AI。
10. **Audit 保留期清理策略** 缺失。