# Phase 1 Architecture: 稳定内核 (Stable Kernel)

**版本**: 1.1
**日期**: 2026-06-17
**状态**: Round 2 修订 (已解决 Blocking #1 背压策略, Blocking #2 聚合边界)

## 1. 架构分层 (Clean Architecture + DDD)

```
┌─────────────────────────────────────────────────────────────┐
│ Infrastructure (最外层)                                     │
│ SQLite │ moka Cache │ tokio runtime │ tracing              │
└───────────────────────────────┬─────────────────────────────┘
                                │ 实现接口
┌───────────────────────────────▼─────────────────────────────┐
│ Adapters                                                    │
│ SqliteInboxRepo │ MokaDedupCache │ WeChatSessionStore       │
└───────────────────────────────┬─────────────────────────────┘
                                │ 实现 Port
┌───────────────────────────────▼─────────────────────────────┐
│ Application (Use Cases)                                     │
│ RouteMessage │ ReorderMessages │ GarbageCollectIdleWorkers  │
│ PersistSyncBuf │ Deduplicate                                │
└───────────────────────────────┬─────────────────────────────┘
                                │ 操作
┌───────────────────────────────▼─────────────────────────────┐
│ Domain (核心, 零外部依赖)                                   │
│ RouteKey │ Conversation │ Message │ ReorderWindow           │
│ BackpressureConfig │ AuditMark │ DedupCache              │
└─────────────────────────────────────────────────────────────┘
```

**依赖方向(向内)**:
- `domain/` 不依赖任何外部 crate(除 std + 基础类型)
- `application/` 只依赖 `domain/` + port traits
- `adapters/` 依赖 `domain/` port traits + 具体实现库(moka, rusqlite)
- `infrastructure/` 负责组装、启动、配置

## 1.1 Review Gate 对齐

本架构文档与 [implementation-gap-solution.md](implementation-gap-solution.md) 中的 Formal Review Checklist 采用同一套口径：

- 只有 `closed` 能力计入阶段完成度。
- 单测是必要条件，闭环测试才是合并条件。
- Phase 1 的完成定义不是“模块都有实现”，而是“主链路闭环真正跑通”。
- Phase 1.5 / Phase 2 的完成定义必须覆盖 MCP stdout 零污染、ilink 契约、sync_buf 持久化、Outbox/DLQ 恢复等 P0 红线。

**Phase 1 Gate**：

1. Inbound -> Dedup -> Route Resolution -> Per-Route Queue -> Reorder Window -> Worker Process -> Idle GC 主闭环跑通。
2. RouteKey / Conversation / Message / ReorderWindow 的领域模型和持久化边界一致。
3. `main` / bootstrap 是唯一系统装配根，没有核心能力挂空挡。
4. 任何 skeleton / preview / stub 不计入完成度。

## 2. 领域模型 (DDD) — 已修订(Round 2)

### 2.1 聚合根: Conversation

Conversation 是聚合根，**拥有**其 ReorderWindow 和 last_active 状态。Worker 是 Conversation 的运行时投影，生命周期绑定在 Conversation 上，而非 RouteKey 值对象。

```rust
// domain/aggregates/conversation.rs

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
- 同一 RouteKey 的所有 Message **必须**串行处理 (由聚合的运行时投影 Worker 保证)
- ReorderWindow 的生命周期 = Conversation 的生命周期
- Conversation 进入 Idle 状态后，由 GC Janitor 回收其运行时投影
- 聚合内的 Message 处理顺序由 ReorderWindow 保证

**路由键与聚合的关系**:
- `RouteKey` 是**值对象** — 不可变、无生命周期、仅作为查找键
- `Conversation` 是**聚合根** — 有生命周期、拥有状态、可被 GC
- Worker（运行时投影）— 一对一绑定到 Conversation 实例，负责串行消费消息
- 只有被运行时入口或闭环测试覆盖到的能力，才算真正闭环；仅有结构存在不算完成

### 2.2 实体: Message

```rust
// domain/entities/message.rs

pub struct Message {
    pub id: String,                    // 平台消息 ID (WeChat 内全局唯一)
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
```

### 2.3 值对象: RouteKey

```rust
// domain/value_objects/route_key.rs

#[derive(Hash, Eq, PartialEq, Clone)]
pub struct RouteKey {
    pub channel: ChannelId,
    pub conversation_id: String,
    pub peer_id: String,
    pub conversation_type: ConversationType,
}

pub enum ConversationType {
    Direct,
    Group,
    Thread,
    BotSession,
}
```

### 2.4 领域服务: ReorderWindow (由 Conversation 聚合拥有)

```rust
// domain/services/reorder_window.rs

pub struct ReorderWindow {
    buffer: BTreeMap<i64, Message>,
    window_ms: u64,                    // 默认 200ms
    last_flushed: Option<Instant>,
}

impl ReorderWindow {
    /// 插入消息，返回已排序可投递的批次
    pub fn insert(&mut self, msg: Message) -> Vec<Message>;
    /// 超窗口迟到消息标记 audit 后放行
    pub fn handle_late(&mut self, msg: Message) -> LateMessageAction;
    /// GC 时强制刷出所有 buffer 中的消息
    pub fn flush_all(&mut self) -> Vec<Message>;
}
```

### 2.5 Port 接口(领域边界) — 已修订

```rust
// domain/ports/

/// 去重缓存 (Phase 1: moka 实现)
pub trait DedupCache: Send + Sync {
    /// 返回 true = 新消息 (未命中缓存)
    fn check_and_set(&self, channel: &str, msg_id: &str) -> bool;
}

/// sync_buf 持久化存储
pub trait SyncBufStore: Send + Sync {
    fn save(&self, channel: &str, account: &str, buf: &[u8]) -> Result<()>;
    fn load(&self, channel: &str, account: &str) -> Result<Vec<u8>>;
}

/// 会话处理队列 — Conversation 运行时投影的抽象
/// 不暴露 mpsc 细节到 domain 层
pub trait ConversationQueue: Send + Sync {
    /// 投递消息到指定会话的处理队列。满时返回 BackpressureAction
    fn enqueue(&self, key: &RouteKey, msg: Message) -> Result<(), BackpressureError>;
    /// 获取活跃会话数量
    fn active_conversations(&self) -> usize;
}

/// GC Janitor 的抽象 — 定期扫描并回收空闲会话
pub trait ConversationGC: Send + Sync {
    /// 扫描所有会话，回收 idle 超过 timeout 的
    fn collect_idle(&self, idle_timeout: Duration) -> Vec<RouteKey>;
}

// Review gate 约束：上述 Port 只有被 application/infrastructure 真实接线后，
// 才可计入 Phase 1 完成度；单元测试通过不代表 closed。
```

### 2.6 背压策略 (Blocking #1 修订)

```rust
// domain/value_objects/backpressure.rs

pub struct BackpressureConfig {
    /// 每个会话的队列容量
    pub per_route_buffer: usize,        // 默认 256
    /// 全局入站 channel 容量
    pub inbound_channel_capacity: usize, // 默认 4096
}

pub enum BackpressureAction {
    /// 丢弃最旧消息并记录审计日志
    DropOldest,
    /// 丢弃当前消息并记录审计日志
    DropNewest,
    /// 阻塞生产者(仅发送路径使用，入站路径禁用)
    Block,
}

pub struct BackpressureError {
    pub action_taken: BackpressureAction,
    pub dropped_message_id: String,
    pub route_key: RouteKey,
}
```

**入站背压策略**(Channel → Router):
- 使用 `mpsc::channel(4096)` bounded channel
- Channel 满时：`try_send` 失败 → Drop Oldest + 写审计日志 + 递增 `dropped_inbound` 指标
- Webhook 场景：丢弃 + HTTP 200 响应(避免 WeChat 无限重试)
- Poll 场景：丢弃 + 继续 polling(消息可在下次 poll 补拉)

**会话队列背压**(Router → Worker):
- 每会话队列 bounded (默认 256)
- 满时：Drop Newest + 写审计日志 + 递增 `dropped_route` 指标
- 理由：同一会话队列满意味着 Worker 严重滞后，丢弃最新的比丢弃最旧的安全(旧消息可能有关键上下文)


## 3. 数据流图 (含背压控制)

```
                    ┌──────────────────────┐
                    │   WeChat Platform    │
                    │  (webhook / poll)    │
                    └──────────┬───────────┘
                               │ raw inbound
                               ▼
              ┌────────────────────────────────┐
              │       Channel Adapter          │
              │  - 解密 (AES-128-ECB/PKCS7)    │
              │  - 协议解析 (ret, errcode)     │
              │  - 构造 Message               │
              └────────────────┬───────────────┘
                               │ Message
                               ▼
              ┌────────────────────────────────┐
              │         Dedup (Step 1)         │
              │  moka Cache TTL 5min           │
              │  key: channel + message_id     │
              │  hit → discard + audit         │
              └────────────────┬───────────────┘
                               │ miss → new
                               ▼
         ┌─────────────────────────────────────────┐
         │   Inbound Channel (bounded: 4096)       │
         │   ┌─────────────────────────────────┐   │
         │   │ BACKPRESSURE: full → drop oldest │   │
         │   │ + audit log + metric increment   │   │
         │   └─────────────────────────────────┘   │
         └────────────────┬────────────────────────┘
                           │
                           ▼
              ┌────────────────────────────────┐
              │     Route Resolution (Step 2)  │
              │  根据 RouteKey 查 Conversation │
              │  Queue.get_or_create(key)      │
              └────────────────┬───────────────┘
                               │ enqueue
         ┌─────────────────────────────────────────┐
         │  Per-RouteKey Queue (bounded: 256)      │
         │  ┌─────────────────────────────────┐    │
         │  │ BACKPRESSURE: full → drop newest │    │
         │  │ + audit log + metric increment   │    │
         │  └─────────────────────────────────┘    │
         └────────────────┬────────────────────────┘
                           │ dequeue
                           ▼
              ┌────────────────────────────────┐
              │  Conversation Worker (Step 3)  │
              │  (Conversation 聚合运行时投影)  │
              │  串行处理该会话的所有消息       │
              │  ┌──────────────────────────┐  │
              │  │ Reorder Window Buffer     │  │
              │  │ - 有 seq: 按 seq 排序     │  │
              │  │ - 无 seq: timestamp+200ms │  │
              │  │ - 迟到: 幂等+审计标记     │  │
              │  └──────────┬───────────────┘  │
              │             │ ordered batch     │
              │             ▼                   │
              │  ┌──────────────────────────┐  │
              │  │ Persistence              │  │
              │  │ - Inbox write (pending)   │  │
              │  │ - sync_buf persist        │  │
              │  └──────────────────────────┘  │
              │  last_active 更新              │
              └────────────────┬───────────────┘
                               │
              ┌────────────────▼───────────────┐
              │     Idle GC Janitor            │
              │  每分钟扫描 Conversation 集合   │
              │  last_active > 30min →          │
              │  flush_all() → drop queue       │
              │  → worker 退出 → 聚合回收       │
              └────────────────────────────────┘
```

**关键路径延迟预算**:
- Dedup 查重: <1ms (moka 内存)
- Route Resolution: <0.1ms (HashMap 查找)
- Reorder Window: 最多等待 200ms (可配置)
- sync_buf Write: <5ms (SQLite WAL)

**崩溃恢复 SLA 目标**: 进程重启后 **30 秒内**恢复服务 (从 SQLite 恢复 sync_buf + 重建 moka Cache 冷启动)

## 4. 组件详细设计

### 4.1 RouteKey 升级

**变更**: RouteKey 从 `channel + peer_id` 升级为 `channel + conversation_id + peer_id + conversation_type`

**影响范围**:
- `domain/value_objects/route_key.rs` — 新增定义
- `application/router.rs` — Worker 索引键变更
- `channels/registry.rs` — HashMap key 类型变更
- 所有依赖 RouteKey 的序列化/反序列化

**数据迁移**: 由于是 greenfield, 无迁移需求。Phase 3 多信道时 conversation_type 枚举可扩展。

### 4.2 Conversation 生命周期与 Idle GC

Conversation 聚合拥有 `ReorderWindow` 和 `last_active`。Worker 是 Conversation 的运行时投影，生命周期一对一绑定。

```
┌──────────────────────────────────────────────────┐
│           ConversationStore (Adapter)            │
│  HashMap<RouteKey, ConversationHandle>           │
│                                                  │
│  ConversationHandle {                            │
│      queue: BoundedMpsc<Message>,  // cap: 256   │
│      last_active: Instant,                       │
│  }                                               │
│                                                  │
│  Janitor (tokio interval 60s):                   │
│    for each (key, handle):                       │
│      if idle > 30min:                            │
│        send FlushAndExit → worker flushes        │
│        reorder_window → drops queue → GC entry   │
└──────────────────────────────────────────────────┘
```

**聚合 GC 安全流程**:
1. Janitor 检测 `last_active > 30min`
2. 向 Worker 发送 `FlushAndExit` 命令
3. Worker 调用 `ReorderWindow::flush_all()` 处理剩余消息
4. Worker 写最后一条 sync_buf
5. Worker 退出，drop queue receiver
6. Janitor 从 ConversationStore 中删除条目

**防误回收保护**:
- `last_active` 在每次 `enqueue` 时更新(先更新再入队，时序安全)
- GC 检查 last_active 前先检查 queue 是否为空(非空说明有消息在处理，跳过)
- 如果恰好被 GC 的会话收到新消息 → `get_or_create` 发现条目不存在 → 自动创建新的 Conversation 聚合

### 4.3 Dedup (moka TTL Cache)

```rust
pub struct MokaDedupCache {
    cache: moka::sync::Cache<String, ()>,
}

impl MokaDedupCache {
    pub fn new(ttl_secs: u64, max_capacity: u64) -> Self {
        Self {
            cache: moka::sync::Cache::builder()
                .time_to_live(Duration::from_secs(ttl_secs))   // 默认 300
                .max_capacity(max_capacity)                     // 默认 2_000_000
                .build(),
        }
    }
}

impl DedupCache for MokaDedupCache {
    fn check_and_set(&self, channel: &str, msg_id: &str) -> bool {
        let key = format!("{}:{}", channel, msg_id);
        self.cache.insert(key, ()).is_none()  // None = 新消息
    }
}
```

**容量规划**:
- 每条目约 80 bytes (key + overhead)
- 2M 条目 ≈ 160MB
- 5 分钟 TTL 覆盖大多数平台重试窗口
- 超过 max_capacity 时 moka 自动 LRU 驱逐

### 4.4 Reorder Window

**策略矩阵**:

| 平台 | 排序依据 | window_ms | 迟到处理 |
|------|---------|-----------|----------|
| WeChat (webhook) | timestamp | 200 | 幂等 + audit |
| WeChat (sync) | sync_buf seq | N/A | 严格按序 |
| Dingtalk | timestamp | 200 | 幂等 + audit |
| Feishu | timestamp | 200 | 幂等 + audit |

**实现**:
```rust
impl ReorderWindow {
    pub fn insert(&mut self, msg: Message) -> Vec<Message> {
        let key = msg.sequence.unwrap_or(msg.timestamp_ms);
        self.buffer.insert(key, msg);

        // 检查是否可刷出
        let cutoff = now_ms() - self.window_ms;
        let ready: Vec<_> = self.buffer
            .range(..=cutoff)
            .map(|(_, m)| m.clone())
            .collect();

        for k in ready.iter().map(|m| m.sequence.unwrap_or(m.timestamp_ms)) {
            self.buffer.remove(&k);
        }
        ready
    }

    pub fn flush_all(&mut self) -> Vec<Message> {
        self.buffer.drain().map(|(_, m)| m).collect()
    }
}
```

### 4.5 WeChat sync_buf 持久化

**问题**: sync_buf 是 WeChat ilink 协议的会话同步缓冲区, 之前全在内存, 重启丢失导致需要全量重拉。

**方案**:
- SQLite 表: `sync_buf(channel TEXT, account TEXT, buf BLOB, updated_at INTEGER, PRIMARY KEY(channel, account))`
- 每次 sync_buf 更新立即写入(同步或 WAL 模式下异步 batch)
- 重启时从 DB 恢复 sync_buf, 从上次断点继续

**写策略**: WAL 模式 + `PRAGMA synchronous=NORMAL`, 每次更新调用 `INSERT OR REPLACE`

### 4.6 ilink 契约测试列表

以下为 WeChat ilink 协议关键契约, Phase 1 必须覆盖 contract test:

| 契约项 | 测试内容 | 失败影响 |
|--------|---------|---------|
| `ret` 字段 | 正常值 0, 异常值 -1/-2/-3 各语义 | 错误码误判导致消息静默丢失 |
| `errcode` 字段 | 错误码矩阵: 0(成功), 1(参数错), 2(鉴权失败), 3(限频), 4(会话过期) | 错误处理分支走错 |
| `sync_buf` | 增量更新、全量同步、空 buf 三种场景 | 会话状态不一致 |
| `X-WECHAT-UIN` | Header 存在性、格式校验 | 账号路由错误 |
| AES-128-ECB/PKCS7 | 加解密 round-trip、错误密文处理、padding 边界 | 消息无法解密或解密错 |
| 消息 ID 唯一性 | 验证 WeChat message_id 全局唯一(非 per-conversation) | Dedup key 冲突 |

### 4.7 结构化日志与可观测性

Phase 1 使用 `tracing` crate 实现结构化日志, 不打乱 stdout(MCP 零污染):

- **stdout**: 仅 MCP JSON-RPC 协议输出
- **stderr**: `tracing` 结构化日志 (JSON format)
- **文件**: 可选 `tracing-appender` 滚动文件输出

**Phase 1 必采指标**:
- `dedup_hit_total` / `dedup_miss_total`
- `inbound_dropped_total` (背压丢弃)
- `route_queue_dropped_total` (会话队列背压丢弃)
- `late_arrival_total` (ReorderWindow 迟到消息)
- `active_conversations` (gauge)
- `conversation_gc_total` (GC 回收计数)

## 5. 失败模式分析

| 数据源/组件 | 失败模式 | 影响 | 处理策略 | 恢复方式 |
|------------|---------|------|---------|---------|
| **WeChat webhook** | 网络超时/平台不可达 | 入站消息丢失 | Webhook 天然重试, WeChat 侧有重试队列 | 幂等接收入站, 重复消息 dedup 过滤 |
| **WeChat poll** | poll 连接断开 | 无法拉取新消息 | Channel 内部自动重连 + backoff | 重连后从 sync_buf 断点继续 |
| **moka Dedup Cache** | OOM (超过 max_capacity) | 老条目被 LRU 驱逐 | moka 内置驱逐策略, 不会 OOM | 驱逐后可能重复处理, 由 Inbox 幂等兜底 |
| **moka Dedup Cache** | 进程崩溃 | Cache 全量丢失 | 重启后 Cold start, 部分消息可能重复 | Phase 2 Inbox 幂等写保证 at-most-once |
| **Reorder Window** | 消息在 window 期内进程崩溃 | Buffer 中未刷出消息丢失 | 当前阶段接受(Phase 2 Inbox 补齐持久化 window) | Phase 2 将 window buffer 落地 |
| **Reorder Window** | 迟到消息(超 window) | 乱序进入处理 | 标记 `late_arrival` + audit log | 审计日志可追溯, 业务方可查 |
| **sync_buf SQLite** | 磁盘满 | 写入失败 | 返回错误, 上层重试 | 清理磁盘后恢复 |
| **sync_buf SQLite** | DB 损坏 | sync_buf 丢失 | 备份 + 全量重拉 | 从 WeChat 服务端重新同步 |
| **Inbound Channel 背压** | 入站速率 > 处理速率, channel 满 | 旧消息被丢弃 | Drop Oldest + audit log | 被丢弃消息因 webhook 重试重新入站, dedup 处理 |
| **Route Queue 背压** | 单会话队列满 (Worker 滞后) | 该会话最新消息被丢弃 | Drop Newest + audit log | 旧消息保留, 新消息由发送方重试 |
| **Worker Idle GC** | 误回收活跃 worker | 消息处理中断 | last_active 在 enqueue 前更新, queue 非空跳过 GC | 下一条消息到达时自动创建新 Conversation |
| **ConversationStore** | HashMap 膨胀(内存泄漏) | 内存持续增长 | GC Janitor 兜底 + active_conversations gauge 监控 | 告警 → 排查是否有 Conversation 未正常 GC |

## 6. 回滚方案

### 6.1 整体回滚

Phase 1 作为 Greenfield Rust 实现, 无旧版本可回滚到。但可回退到"不启动 Rust 系统"的状态。

### 6.2 组件级回滚

| 组件 | 回滚操作 | 数据影响 |
|------|---------|---------|
| RouteKey 升级 | 模型定义是 foundation, 无法单独回滚 | 需要全量重新设计, 风险最高 |
| Dedup TTL | 可降级为 HashMap + 定时清理(性能退化但可用) | Dedup 行为一致 |
| Reorder Window | 可临时关闭(直接放行, 不排序) | 消息可能乱序, 但不丢 |
| Idle GC | 可临时禁用(timeout 设极大值) | Worker 不回收, 内存增长 |
| sync_buf 持久化 | 可降级为文件 append log | 恢复逻辑切换回文件读取 |

### 6.3 数据回滚

- SQLite 文件是新增的, 删除即可清空所有持久化状态
- moka Cache 是纯内存, 重启即清空

## 7. 与旧模块的关系

### 7.1 迁移来源分析

当前系统为 Python/TypeScript 混合实现, Rust 迁移是**重写(rewrite)**而非渐进替换。

| 旧模块 | 新模块 | 处理方式 |
|--------|--------|---------|
| Python WeChat bot (ilink) | `channels/wechat/` | 功能等价重写, 增加持久化 |
| Python 消息路由 (内存 dict) | `core/router.rs` + `channels/registry.rs` | 升级为 RouteKey + Worker 模型 |
| Python Dedup (set in memory) | `core/dedup.rs` | 升级为 moka TTL Cache |
| TypeScript 排序逻辑 | `core/reorder.rs` | 升级为 ReorderWindow |
| Python sync_buf (内存变量) | `channels/wechat/session.rs` | 升级为 SQLite 持久化 |

### 7.2 旧模块接入检查表

- [x] 列出所有与新能力同类/相关的现有模块 (见上表)
- [x] 对每个旧模块回答是否应升级接入新能力 — **全部接入**(Rewrite 策略)
- [x] 确认无"应接入却遗漏"的旧模块

### 7.3 兼容性

- **协议层面**: 与 WeChat ilink 协议保持兼容(contract test 覆盖)
- **数据层面**: SQLite schema 是新增的, 不影响旧系统数据
- **运行层面**: Rust 和 Python 系统可并行运行(不同端口/进程), 切换通过反向代理

## 8. Phase 1 验收清单

对照 RUST_MIGRATION_V5.md 第 11 节:

- [ ] RouteKey 包含 conversation 维度
- [ ] 同 RouteKey 串行, 不同 RouteKey 并行
- [ ] 30 分钟 idle route 自动回收
- [ ] Dedup 使用 TTL cache (无全表 retain)
- [ ] 乱序场景按策略重排/幂等
- [ ] sync_buf 持久化并重启可恢复

## 9. 目录结构(Phase 1 范围)

```
src/
├── domain/
│   ├── mod.rs
│   ├── aggregates/
│   │   └── conversation.rs       # 聚合根, 拥有 ReorderWindow + last_active
│   ├── entities/
│   │   └── message.rs            # Message 实体 + AuditMark
│   ├── value_objects/
│   │   ├── route_key.rs          # RouteKey, ConversationType
│   │   └── backpressure.rs       # BackpressureConfig, BackpressureAction
│   ├── services/
│   │   └── reorder_window.rs     # 乱序窗口领域服务
│   └── ports/
│       ├── mod.rs
│       ├── dedup_cache.rs        # DedupCache trait
│       ├── sync_buf_store.rs     # SyncBufStore trait
│       └── conversation_queue.rs # ConversationQueue + ConversationGC traits
├── application/
│   ├── mod.rs
│   ├── route_message.rs          # 消息去重 + 路由到 Conversation
│   ├── deduplicate.rs            # Dedup 用例
│   └── gc_janitor.rs             # 空闲 Conversation GC
├── adapters/
│   ├── mod.rs
│   ├── moka_dedup.rs             # MokaDedupCache 实现
│   ├── sqlite_sync_buf.rs        # SqliteSyncBufStore 实现
│   └── conversation_store.rs     # ConversationStore 实现 (bounded queues + GC)
├── channels/
│   ├── mod.rs
│   └── wechat/
│       ├── mod.rs
│       ├── session.rs            # sync_buf 持久化 + 会话恢复
│       └── ilink.rs              # ilink 协议: 加解密, ret/errcode 解析
├── infrastructure/
│   ├── mod.rs
│   ├── config.rs
│   ├── db.rs                     # SQLite 连接池
│   └── tracing.rs                # 结构化日志初始化
├── main.rs
└── lib.rs
```
