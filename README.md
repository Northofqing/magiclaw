# magiclaw

> 信道中心架构系统 — 基于 Rust 的多平台消息中枢。统一接入 WeChat / Dingtalk / Feishu，**优先保证信道稳定性、消息顺序正确性和可恢复投递**。AI 只是可插拔能力，主链路不依赖任何 Agent。

[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org/)
[![tests](https://img.shields.io/badge/tests-285%20passing-brightgreen.svg)](#testing)
[![clippy](https://img.shields.io/badge/clippy-clean-blue.svg)](#testing)

---

## 目录

- [项目定位](#项目定位)
- [架构概览](#架构概览)
- [关键设计规则](#关键设计规则)
- [目录结构](#目录结构)
- [快速开始](#快速开始)
- [运行模式](#运行模式)
- [AI 后端](#ai-后端)
- [可观测性](#可观测性)
- [数据持久化](#数据持久化)
- [测试](#测试)
- [常见命令](#常见命令)
- [相关文档](#相关文档)

---

## 项目定位

**核心目标**：把 WeChat / Dingtalk / Feishu 等异构信道统一接入一个**可恢复、可审计、可扩展**的消息核心，并提供 CLI / MCP / HTTP 三种接入方式。

**核心原则**：

| 原则 | 实现 |
|------|------|
| **同 RouteKey 串行，跨 RouteKey 并行** | `ConversationStore` 用 `mpsc::channel` per-route |
| **消息可恢复投递** | Inbox → Outbox → DLQ 三段式，SQLite 持久化 + 崩溃恢复 |
| **审计不可篡改** | SHA-256 链式 hash，启动时校验完整性 |
| **Core 不依赖 Agent** | AI 后端故障时降级为 echo，主链路不中断 |
| **路径级多账号隔离** | channel/account 维度命名空间 |

---

## 架构概览

```
┌──────────────────────────────────────────────────────────────────┐
│  Adapter Layer (MCP / HTTP API / CLI / Push / Binding)           │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌─────────────────┐     │
│  │ MCP stdio│  │ HTTP API │  │ CLI send │  │ Push / Binding  │     │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────────┬────────┘     │
└───────┼─────────────┼─────────────┼───────────────┼──────────────┘
        │             │             │               │
        ▼             ▼             ▼               ▼
┌──────────────────────────────────────────────────────────────────┐
│  Pipeline Layer (Chain of Responsibility)                        │
│  Normalize → Permission → RateLimit → AgentCommand → AI → Outbox  │
│  (each stage is `Arc<dyn Middleware>` with short-circuit)        │
└──────────────────────────────────────────────────────────────────┘
        │
        ▼
┌──────────────────────────────────────────────────────────────────┐
│  Message Core (domain-driven)                                    │
│  ┌─────────────────┐  ┌──────────────┐  ┌─────────────────────┐   │
│  │ ConversationStore│  │ InboxProcessor│  │ OutboxWorker        │   │
│  │  (RouteKey-routed│  │  (dedup Moka) │  │  (retry + DLQ replay)│  │
│  │   per-worker)    │  │               │  │                     │   │
│  └─────────────────┘  └──────────────┘  └─────────────────────┘   │
│                                                                  │
│  Domain Types: Message, Conversation, ConversationSnapshot,    │
│                ChannelError, PipelineError, AiError             │
└──────────────────────────────────────────────────────────────────┘
        │             │             │
        ▼             ▼             ▼
┌──────────────────────────────────────────────────────────────────┐
│  Channels (Channel trait + port)                                 │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐                          │
│  │ WeChat   │  │ Feishu   │  │ Dingtalk │                          │
│  │  (ilink) │  │  (OpenAPI│  │  (OpenAPI│                          │
│  │          │  │  + webhook│  │  + token)│                          │
│  └──────────┘  └──────────┘  └──────────┘                          │
└──────────────────────────────────────────────────────────────────┘
        │
        ▼
┌──────────────────────────────────────────────────────────────────┐
│  Infrastructure                                                  │
│  SQLite (DbPool with N connections) + TaskSupervisor (JoinSet)   │
│  + Circuit Breaker + Bulkhead (ResilienceGate) + Audit Chain     │
└──────────────────────────────────────────────────────────────────┘
```

---

## 关键设计规则

来自 [CLAUDE.md](CLAUDE.md) 红线 + DDD 实践：

1. **`RouteKey`** 包含 `channel` / `conversation_id` / `peer_id` / `conversation_type`，`Conversation` 是一等聚合根
2. **同 RouteKey 串行，跨 RouteKey 并行** — 通过 `mpsc::channel` per-route 实现
3. **空闲 Route Worker GC** — 默认 30 分钟回收，由 `gc_janitor` 任务周期性扫描
4. **Dedup via TTL Cache (moka)** — 默认 TTL 5 分钟，容量 200 万
5. **乱序消息处理** — 有 sequence 按 sequence；无 sequence 用 `timestamp + reorder_window_ms`
6. **发送状态机** — `pending → sending → sent`；失败 → `retrying`；超阈值 → `dead_letter`
7. **MCP stdio 零污染** — 协议只走 stdout，日志只走 stderr / 文件
8. **路径级多账号/通道隔离** — session、sync_buf、allowlist、inbox/outbox、audit 都按 channel/account 命名
9. **流式媒体上传** — 永不整文件加载到内存
10. **Circuit Breaker + Bulkhead** — 外部平台 API 和 AI API 都隔离
11. **AI 池 vs Send 池分离**（red line 2.5）
12. **高风险操作写 audit_log**（自动-allowlist 等）
13. **审计日志不可变，保留 ≥ 5 年** — 通过 hash chain 实现防篡改

---

## 目录结构

```
src/
├── adapters/               # 适配层（DB、HTTP auth、conversation store、MCP 等）
│   ├── sqlite_*.rs         # SQLite 实现（inbox/outbox/dlq/audit/sync_buf/...）
│   ├── http_auth.rs        # Bearer auth 中间件
│   ├── conversation_store.rs # 内存 RouteKey 路由 + per-worker
│   └── ...
├── application/            # 应用服务（编排、worker、recovery）
│   ├── outbox_worker.rs    # 可恢复投递 + DLQ 重放
│   ├── crash_recovery.rs   # 启动时重置 sending/retrying 状态
│   ├── gc_janitor.rs       # 30 分钟空闲回收
│   ├── audit.rs            # AuditRecord + 通过端口的查询 API
│   ├── agent_preferences.rs # per-user AI 偏好
│   ├── push.rs / binding.rs # 多项目推送 + 投递目标
│   └── ...
├── channels/               # 信道实现（trait: Channel）
│   ├── wechat/             # ilink 长轮询 + token 管理
│   ├── feishu/             # OpenAPI + webhook 验证 + 错误语义
│   └── dingtalk/           # access_token + 消息类型映射 + 错误语义
├── core/                   # 核心能力
│   ├── pipeline/           # 中间件链
│   ├── ai/                 # 后端抽象 + echo/claude_code/codex/copilot
│   └── resilience/         # CircuitBreaker + Bulkhead + ResilienceGate
├── domain/                 # 领域模型（无基础设施依赖）
│   ├── entities/           # Message
│   ├── aggregates/         # Conversation
│   ├── value_objects/      # RouteKey, ConversationSnapshot, MessageContent
│   ├── services/           # ReorderWindow, audit_chain
│   ├── ports/              # trait 抽象（AuditQuery, AuditSink, UserPreferenceStore 等）
│   └── error.rs            # ChannelError, PipelineError, AiError
├── infrastructure/         # runtime / config / db / tracing
│   ├── runtime.rs          # AppRuntime 装配 + 启动后台任务
│   ├── db.rs               # DbPool（连接池 + Condvar）+ schema 初始化
│   ├── task_supervisor.rs  # JoinSet 包装的后台任务管理
│   └── config.rs           # AppConfig（WeChatConfig, FeishuConfig, ...）
├── cli/                    # CLI 解析与命令
├── daemon/                 # daemon 模式 + 单例锁
└── main.rs                 # 入口
```

设计文档在 [`docs/`](docs/) 下：
- [`docs/mcp-deployment.md`](docs/mcp-deployment.md) — MCP stdio 部署
- [`docs/phase1-architecture.md`](docs/phase1-architecture.md) … [`docs/phase4-architecture.md`](docs/phase4-architecture.md) — 各阶段架构决策
- [`docs/2026-06-23-feishu-push-architecture.md`](docs/2026-06-23-feishu-push-architecture.md) — Feishu 多账号推送

---

## 快速开始

### 前置条件

- Rust stable（edition 2021, 1.85+）
- macOS / Linux
- SQLite 通过 `rusqlite/bundled` 内置，无需系统 SQLite

### 最小可跑通路径

```bash
# 1. 准备 WeChat 数据目录
mkdir -p ~/.claude/channels/wechat
cat > ~/.claude/channels/wechat/account.json <<'EOF'
{
  "token": "<your-channel-token>",
  "baseUrl": "https://<ilink-host>",
  "accountId": "<your-account-id>",
  "userId": "<optional-user-id>"
}
EOF

# 2. 启动 daemon
cargo run --release

# 3. 命令行直发验证
./target/release/magiclaw send --message "hello" --to "<peer_id>"

# 4. 启用本地 AI 自动回复
MAGICLAW_AI_BACKEND=claude_code ./target/release/magiclaw
```

**Or use the one-liner**: `scripts/daemon-up.sh` 自动签发 token 并启动 daemon。

---

## 运行模式

### 1. Daemon（默认，无参数）

```bash
./target/release/magiclaw
```

启动后：

- 后台 runtime（GC janitor / inbound router / outbox worker / wechat token poller / HTTP API）
- 端口默认 `127.0.0.1:18011`
- 所有持久化状态写入 SQLite

> ⚠️ **不要写 `magiclaw daemon`** —— 没有该子命令，会导致 daemon 没起来。

### 2. MCP stdio Server

```bash
./target/release/magiclaw --mcp
```

- JSON-RPC 2.0 over stdio
- stdout 零污染（协议只走 stdout，日志只走 stderr）
- 工具：`send` / `list_peers` / `login`

Claude Desktop 配置：

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

### 3. CLI 单次发送

```bash
./target/release/magiclaw send --message "hello" --to "<peer_id>"
```

发送策略：

1. 先尝试发到本地 daemon 的 `POST /api/send`
2. daemon 不可达时回退为直接 ilink 发送

### 4. 鉴权管理

```bash
# 签发 token（30 天，仅允许 send + window_status）
./target/release/magiclaw auth issue \
  --project my-project \
  --name cicd \
  --scopes send,window_status \
  --ttl-secs 2592000

# 列出项目下的 token（仅元数据，无明文）
./target/release/magiclaw auth list --project my-project

# 撤销 token
./target/release/magiclaw auth revoke --token <raw_token>
```

### 环境变量

| 变量 | 默认值 | 作用 |
|------|--------|------|
| `MAGICLAW_DB_PATH` | `data/magiclaw.db` | SQLite 路径（daemon / auth issue / send 必须一致） |
| `MAGICLAW_API_ADDR` | `127.0.0.1:18011` | HTTP API 监听地址 |
| `MAGICLAW_API_AUTH_ENABLED` | `true` | 是否要求 Bearer 鉴权 |
| `MAGICLAW_WECHAT_SEND_MIN_INTERVAL_MS` | `500` | 同 peer 发送最小间隔 |
| `MAGICLAW_DB_POOL_SIZE` | `max(4, num_cpus)` | DB 连接池大小（`1` 强制单连接） |
| `WECHAT_CHANNEL_DIR` | `~/.claude/channels/wechat` 或 `./.claude/channels/wechat` | WeChat 数据目录 |
| `MAGICLAW_AI_BACKEND` | `echo` | AI 后端：`echo` / `claude_code` / `codex` / `copilot` / `hermes` / `openclaw` |
| `FEISHU_ENABLED` / `FEISHU_*` | `false` | Feishu 通道启用与凭证 |
| `DINGTALK_*` | — | Dingtalk 通道凭证 |
| `RUST_LOG` | `info` | 日志级别（只走 stderr / 文件） |

---

## AI 后端

AI 通过 `AiBackend` trait 接入，**默认 `echo`（不调用任何 LLM，仅回显原文）**。切换后端只需设置 `MAGICLAW_AI_BACKEND`：

| 后端 | 二进制 | 备注 |
|------|--------|------|
| `echo` | — | 默认，回显原文（CI 与测试用） |
| `claude_code` | `claude` | 本机 Claude Code CLI，`--permission-mode plan` 只读 |
| `codex` | `codex` | Codex CLI，`--sandbox read-only`，输出写到临时文件 |
| `copilot` | `copilot` | GitHub Copilot CLI（`-p` headless 模式） |
| 自定义 CLI | `agents.<name>` 配置 | 任意 headless CLI |

### 自定义 CLI Agent 示例

```json
{
  "ai": {
    "backend": "hermes",
    "agents": {
      "hermes": {
        "binary_path": "hermes",
        "args": ["chat", "{prompt}"],
        "timeout_secs": 120,
        "max_output_bytes": 16384,
        "read_output_file": false
      }
    }
  }
}
```

模板语义：

- `{prompt}` — 替换为用户消息（始终在单个 argv token，**不经过 shell**）
- `{output_file}` — codex `-o <FILE>` 这类输出文件模式

### 韧性保证

- **Circuit Breaker**：AI 失败 ≥ 阈值 → Open → 60s 内快速失败
- **Bulkhead**：AI 池 5 并发 / Send 池 50 并发，**严格隔离**
- **优雅降级**：任何 AI 失败自动回落到 echo，**主链路不中断**
- **审计**：每次 AI 调用写 `audit_log`（action=`ai_generate`）

---

## 可观测性

### `GET /api/health`

无需鉴权。返回结构：

```json
{
  "ok": true,
  "feishu": {
    "enabled": true,
    "accounts": [{ "account_id": "...", "receive_id_type": "...", "auth_method": "app_credentials" }],
    "account_count": 1
  },
  "tasks": {
    "running": ["outbox_worker", "gc_janitor", "inbound_router"],
    "finished_count": 2,
    "finished": [{ "name": "outbox_worker", "state": "running" }, ...]
  },
  "resilience": {
    "send_gate": {
      "circuit_state": "closed",
      "failure_count": 0,
      "failure_threshold": 20,
      "active": 3,
      "max_concurrent": 50
    },
    "ai_gate": {
      "circuit_state": "closed",
      "failure_count": 0,
      "failure_threshold": 20,
      "active": 0,
      "max_concurrent": 5
    },
    "outbox_pending": 0,
    "dead_letter_count": 0
  }
}
```

### 其他端点

| 路径 | 鉴权 | 作用 |
|------|------|------|
| `POST /api/send` | Bearer (`send` scope) | 发送消息 |
| `GET /api/window_status` | Bearer (`window_status` scope) | 查询发送窗口状态 |
| `GET /api/health` | 公开 | 健康检查 |
| `POST /api/feishu/webhook` | 公开（用 HMAC-SHA256 验签） | Feishu 事件入口 |

---

## 数据持久化

SQLite 数据库位于 `data/magiclaw.db`（可通过 `MAGICLAW_DB_PATH` 覆盖）。所有表通过 `IF NOT EXISTS` 自动创建：

| 表 | 作用 |
|----|------|
| `inbox` | 入站消息（含 status: pending/processing/processed） |
| `outbox` | 待发送消息 + 重试元数据 |
| `dead_letter` | 超过最大重试次数的消息 |
| `conversation_state` | 每个 RouteKey 的持久化状态（用于崩溃恢复） |
| `audit_log` | 关键操作 + AI/发送决策（含 `prev_hash`/`entry_hash` 链式 hash） |
| `sync_buf` | WeChat 长轮询的 sync_buf 状态 |
| `user_agent_preferences` | per-user AI agent 偏好 |
| `api_clients` | 项目级 bearer token |
| `projects` / `delivery_targets` / `project_bindings` | 多项目推送 |
| `push_jobs` / `push_job_items` | 推送任务 |

### 崩溃恢复

启动时自动调用 `recover_after_crash`：

- 重置所有 `outbox.status IN ('sending', 'retrying')` 的记录为 `pending`
- 重新投递这些消息

### 审计链完整性

```bash
# 启动时自动校验；如果 audit_log 被篡改，daemon 拒绝启动
[ERROR] audit log chain integrity check FAILED — refusing to start
```

---

## 测试

```bash
# 单元 + 集成测试（285 个，全部通过）
cargo test

# 严格 clippy（0 errors / 0 warnings with -D warnings）
cargo clippy --all-targets -- -D warnings

# 构建发布版
cargo build --release
```

### 测试组织

| 类别 | 位置 | 数量 |
|------|------|------|
| 单元测试 | `src/**/*.rs` 的 `#[cfg(test)] mod tests` | ~50 |
| 闭环测试 | `tests/*_closed_loop.rs` | ~230 |
| HTTP API 测试 | `tests/http_api_unit.rs`（用 `axum::test` + `tower::ServiceExt`） | 10 |
| DB 连接池并发 | `tests/db_pool_closed_loop.rs` | 6 |
| 审计 hash chain | `tests/audit_chain_closed_loop.rs` | 6 |
| Dingtalk 完整路径 | `tests/dingtalk_closed_loop.rs` | 6 |
| 健康检查韧性 | `tests/health_resilience_closed_loop.rs` | 3 |

---

## 常见命令

```bash
# 构建
cargo build --release

# 测试
cargo test
cargo clippy --all-targets -- -D warnings

# 启动 daemon
./target/release/magiclaw

# 启动 MCP server
./target/release/magiclaw --mcp

# CLI 单发
./target/release/magiclaw send --message "hello" --to "<peer_id>"

# Token 管理
./target/release/magiclaw auth issue --project my-proj --scopes send,window_status
./target/release/magiclaw auth list --project my-proj
./target/release/magiclaw auth revoke --token <raw_token>

# 本地 Claude Code 自动回复
MAGICLAW_AI_BACKEND=claude_code ./target/release/magiclaw

# 本地 Codex 自动回复
MAGICLAW_AI_BACKEND=codex ./target/release/magiclaw

# 数据库位置
ls -lh data/magiclaw.db

# 一键启动 daemon（自动签发 token + 写 .env + 启动）
scripts/daemon-up.sh

# 复用已有 token 启动
REUSE_TOKEN=1 scripts/daemon-up.sh
```

---

## 相关文档

- [CLAUDE.md](CLAUDE.md) — 项目红线与开发流程
- [AGENTS.md](AGENTS.md) — 7 步开发流程
- [RUST_MIGRATION_V5.md](RUST_MIGRATION_V5.md) — Rust 迁移约束
- [`docs/`](docs/) — 各阶段架构与设计文档
- [`docs/mcp-deployment.md`](docs/mcp-deployment.md) — MCP 部署详情

---

## License

（按项目实际情况填写）