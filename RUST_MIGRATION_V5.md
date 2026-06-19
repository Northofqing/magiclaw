# Rust 迁移方案 v5：信道中心架构（整合评审修订）

**更新时间**：2026-06-17  
**本次修订目标**：把会话中识别出的生产风险并入方案，避免阶段 1 启动后发生结构性返工。

**核心诉求（保持不变）**：
- ✅ 不依赖任何 Agent（系统可独立运行，AI 可选）
- ✅ 支持多信道、可扩展多平台
- ✅ 支持 MCP（作为 Adapter，不是核心）
- ✅ 消息信道稳定，多消息不串流
- ✅ 重点追求对接平台丰富 + 信道稳定

## 1. 本次评审结论（已并入）

以下问题被判定为有效风险，已进入修订方案：

### P0（开工前必须定稿）
1. Per-RouteKey worker 无回收，长期膨胀。
2. Dedup 每条消息全量清理，吞吐高时 CPU 风险。
3. 缺少 Inbox/Outbox/DLQ，可恢复投递能力不足。
4. RouteKey 维度不够（仅 channel+peer）。
5. webhook/重试下可能乱序（入队顺序不等于业务顺序）。
6. 核心状态全内存，重启丢失。

### P1（尽早落地，避免技术债）
1. Processor 过胖，需要 Pipeline/Middleware 化。
2. RateLimiter 竞争模型需要升级。
3. 熔断和隔离舱缺失，存在级联故障风险。
4. MCP 接入时点偏后，反馈闭环太慢。
5. Conversation 尚非一等对象。

### P2（预留接口）
1. EventBus/Broker 抽象可后置，但需先留 extension point。

## 1.1 补充问题清单（已识别但此前未单列）

### P0（必须显式约束）
1. MCP stdio 零污染：任何业务日志、调试输出都不能进入 stdout，只能走 stderr / 文件。
2. ilink 协议契约不清：`ret`、`errcode`、`sync_buf`、`X-WECHAT-UIN`、AES-128-ECB / PKCS7 需要 contract test。
3. 多账号 / 多通道隔离必须是路径级别的：`session`、`sync_buf`、`allowlist`、`inbox/outbox`、`audit` 都要按 channel / account 命名空间拆分。
4. 媒体上传不能一次性把大文件全读入内存，必须支持流式读取和分段处理。
5. REST Adapter 一旦开放，本地口也要有最小认证或 token 保护，不能默认裸口暴露。

### P1（需要尽早落地）
1. 发送链路要统一为 `send_text_with_recovery` / `send_media_with_recovery`，避免多条发送路径各自实现重试逻辑。
2. typing indicator 需要作为独立能力，不要和文本回复耦合。
3. 允许自动加白名单的行为必须记录审计日志，避免误放行无追踪。
4. 心跳调度与会话活动追踪要分开，避免互相污染。
5. 配置写入必须具备原子替换和回滚，不能只做覆盖写。

### P2（建议预留）
1. 统一的 platform capability registry（平台能力表）用于标记每个信道支持的消息类型、是否支持 sequence、是否支持 typing、是否支持 webhook。
2. 统一的 metrics / tracing 注入点，后续可接 Prometheus / OpenTelemetry。

---

## 2. 架构总览（v5）

```
┌───────────────────────────────────────────────────────────┐
│ Adapter Layer（可选接入）                                 │
│ MCP / REST / CLI                                          │
└───────────────────────────────┬───────────────────────────┘
                                │
┌───────────────────────────────▼───────────────────────────┐
│ Pipeline Layer（可插拔中间件）                            │
│ Normalize -> Dedup -> Permission -> RateLimit ->          │
│ ConversationLoad -> AI(or Rule) -> Formatter -> Outbox    │
└───────────────────────────────┬───────────────────────────┘
                                │
┌───────────────────────────────▼───────────────────────────┐
│ Message Core（信道稳定核心）                              │
│ Route Registry + Per-RouteQueue + Worker Idle GC +        │
│ Reorder Window + Retry + DLQ                              │
└───────────────────────────────┬───────────────────────────┘
                                │
         ┌──────────────────────┼──────────────────────┐
         ▼                      ▼                      ▼
   WeChat Channel         Dingtalk Channel        Feishu Channel
```

---

## 3. 核心模型升级

### 3.1 RouteKey（从 peer 级提升到会话级）

```rust
pub enum ConversationType {
    Direct,
    Group,
    Thread,
    BotSession,
}

pub struct RouteKey {
    channel: ChannelId,              // wechat / dingtalk / feishu
    conversation_id: String,         // 平台会话 ID（主路由键）
    peer_id: String,                 // 用户或群标识
    conversation_type: ConversationType,
}
```

**原因**：多平台场景里，真正稳定的处理单位是会话，不是单 peer。

### 3.2 Conversation 作为一等对象

```rust
pub struct Conversation {
    route: RouteKey,
    participants: Vec<String>,
    state: ConversationState,
    metadata: ConversationMetadata,
    updated_at: DateTime,
}

pub struct Message {
    id: String,
    route: RouteKey,
    sequence: Option<i64>,          // 平台提供时使用
    timestamp_ms: i64,
    direction: Direction,
    content: MessageContent,
}
```

**原则**：Message 是事件；Conversation 是聚合根。

---

## 4. 通道接口调整（避免 poll 阻塞耦合）

将外部拉取改为“信道自主驱动入站”，由信道把消息推给 router：

```rust
#[async_trait]
pub trait Channel: Send + Sync {
    fn id(&self) -> ChannelId;

    // 注入入站总线发送端，信道内部自行决定长轮询或 webhook
    async fn start(&self, inbound_tx: mpsc::Sender<Message>) -> Result<()>;

    async fn send_message(&self, to: &str, content: &MessageContent) -> Result<SendReceipt>;

    async fn stop(&self) -> Result<()>;

    async fn health_check(&self) -> Result<HealthStatus>;
}
```

**收益**：
- 避免 `poll_messages()` 长阻塞导致无法优雅停机。
- webhook 和 long-poll 信道可统一接入。

---

## 5. Message Core 关键机制（v5）

### 5.1 Route Worker 生命周期（Idle GC）

- 每个 RouteKey 仍保持串行 worker。
- 增加 `last_active` 和 janitor 扫描任务。
- `idle_timeout` 默认 30 分钟。
- 空闲 route 自动回收：drop sender -> worker 退出 -> registry 删除。

```rust
struct RouteWorkerHandle {
    tx: mpsc::Sender<Message>,
    last_active: Instant,
}
```

```rust
// janitor 每分钟扫描
if now.duration_since(handle.last_active) > Duration::from_secs(1800) {
    // remove handle; tx dropped; worker exits on recv None
}
```

### 5.2 去重改为 TTL Cache（替代全量 retain）

- 使用 `moka` TTL 缓存。
- key: `channel + message_id`。
- TTL: 5 分钟，可配置。
- 设置最大容量防止爆内存。

```rust
let dedup = moka::sync::Cache::builder()
    .time_to_live(Duration::from_secs(300))
    .max_capacity(2_000_000)
    .build();
```

### 5.3 限速实现升级

- 不再使用单点 `Mutex<VecDeque<Instant>>` 作为最终方案。
- 推荐 `governor` 或分片滑窗。
- 按 channel + route 可配置限速。

```rust
// 默认策略
wechat: 60s / 30
other channel: 可单独覆盖
```

### 5.4 乱序治理（Reorder Window）

- 有 sequence 的平台：按 sequence 严格排序。
- 无 sequence：使用 `timestamp + reorder_window_ms`（默认 200ms）。
- 超窗口迟到消息走幂等处理和审计标记。

---

## 6. 可恢复投递（Inbox / Outbox / DLQ）

### 6.1 最小可靠投递模型

- Inbox：记录入站消息及处理状态（幂等、防重复消费）。
- Outbox：记录待发送、已发送、失败待重试。
- DLQ：超过重试阈值后入死信。

### 6.2 SQLite 表（最小集）

- `inbox(id, channel, conversation_id, payload, status, created_at, updated_at)`
- `outbox(id, route_key, payload, status, retry_count, next_retry_at, last_error, created_at, updated_at)`
- `dead_letter(id, source, payload, reason, created_at)`
- `conversation_state(route_key, state_json, updated_at)`
- `audit_log(id, route_key, action, result, created_at)`

### 6.3 发送状态机

```text
pending -> sending -> sent
                 └-> failed -> retrying -> sent
                               └-> dead_letter
```

**规则**：
- 先写 outbox `pending`，再调用平台发送。
- 发送成功再标记 `sent`。
- 失败按指数退避重试，超过阈值入 DLQ。

---

## 7. 风控与稳定性（生产级补齐）

### 7.1 Circuit Breaker

- 对 AI API、外部平台 API 做断路保护。
- 例：连续失败 20 次，熔断 5 分钟，半开探测恢复。

### 7.2 Bulkhead

- AI 执行池与发送执行池隔离。
- 避免 AI 卡顿拖垮发送链路。

### 7.3 Dead Letter Queue

- 重试 N 次失败后进入 DLQ。
- 提供后台排障/重放命令。

### 7.4 EventBus 预留（P2）

- 首发不强制引入外部 broker，但先固定扩展接口。
- 作为后续“定时任务、风控、人审接管、批处理”统一事件入口。

```rust
pub trait EventPublisher {
    fn publish(&self, event: DomainEvent) -> Result<()>;
}

pub trait EventSubscriber {
    fn subscribe(&self, topic: &str) -> Result<EventStream>;
}
```

---

## 8. 状态存储规划（替代“全内存”）

| 数据 | 存储 |
| --- | --- |
| Config | JSON |
| Allowlist | SQLite |
| Session | SQLite |
| Conversation State | SQLite |
| Inbox | SQLite |
| Outbox | SQLite |
| Audit Log | SQLite |
| Dedup Cache | Moka (TTL 内存) |

**附加要求**：
- WeChat `sync_buf` 必须持久化（每次更新即落盘/落库）。
- 重启后从持久状态恢复而非全量重拉。

---

## 9. 目录结构修订（增量）

```
src/
├─ core/
│  ├─ types.rs
│  ├─ router.rs
│  ├─ pipeline.rs          # Middleware 链
│  ├─ processor.rs
│  ├─ reorder.rs           # 乱序窗口
│  ├─ dedup.rs             # moka TTL 去重
│  ├─ rate_limit.rs        # governor/滑窗实现
│  ├─ resilience.rs        # circuit breaker / bulkhead
│  ├─ event_bus.rs         # 事件发布/订阅扩展点
│  ├─ capabilities.rs      # 平台能力表（sequence / typing / webhook）
│  ├─ telemetry.rs         # metrics / tracing 注入点
│  └─ storage/
│     ├─ mod.rs
│     ├─ inbox.rs
│     ├─ outbox.rs
│     ├─ dlq.rs
│     └─ conversation.rs
├─ channels/
│  ├─ registry.rs          # route worker registry + idle janitor
│  └─ wechat/
│     └─ session.rs        # sync_buf 持久化 + 会话恢复
└─ adapters/
   ├─ mcp.rs
   ├─ rest.rs
   └─ cli.rs
```

---

## 10. 分阶段计划修订（v5）

### 阶段 1（2 周）
目标：稳定内核落地（可长期运行）

交付：
1. RouteKey 升级（conversation 维度）。
2. Worker Idle GC。
3. Dedup TTL Cache（moka）。
4. Reorder Window。
5. WeChat `sync_buf` 持久化。

### 阶段 1.5（0.5-1 周）
目标：提前接 MCP 做真实链路压测

交付：
1. MCP Adapter 最小可用（send/list_peers/login）。
2. stdout 零污染约束（日志仅 stderr/file）。

### 阶段 2（1.5-2 周）
目标：可恢复投递

交付：
1. SQLite Inbox/Outbox/DLQ。
2. 重试与死信重放。
3. 崩溃恢复验证。

### 阶段 3（1-2 周）
目标：多信道扩展

交付：
1. Dingtalk/Feishu 信道骨架。
2. 多信道隔离测试。

### 阶段 4（1.5-2 周）
目标：Pipeline + AI 可插拔

交付：
1. Middleware 链正式落地。
2. Echo/OpenAI/Claude/Local backend 接入。

### 阶段 5（2 周）
目标：生产强化

交付：
1. Circuit Breaker + Bulkhead。
2. daemon/system service/health。
3. 审计与运维工具。

---

## 11. 阶段验收清单（更新）

### 阶段 1
- [ ] RouteKey 包含 conversation 维度
- [ ] 同 RouteKey 串行，不同 RouteKey 并行
- [ ] 30 分钟 idle route 自动回收
- [ ] Dedup 使用 TTL cache（无全表 retain）
- [ ] 乱序场景按策略重排/幂等
- [ ] sync_buf 持久化并重启可恢复

### 阶段 1.5
- [ ] MCP 最小接口可用
- [ ] stdout 无业务日志污染

### 阶段 2
- [ ] Outbox 状态机可观测
- [ ] 失败重试与 DLQ 可用
- [ ] 进程崩溃后可恢复未完成发送

### 阶段 3
- [ ] 双信道并行运行且隔离
- [ ] 单信道故障不影响其他信道

### 阶段 4
- [ ] Middleware 可插拔（新增中间件不改内核）
- [ ] AI backend 可切换并可降级到 echo

### 阶段 5
- [ ] 熔断、隔离舱、DLQ 运维闭环完整
- [ ] 服务化、监控、审计可用于生产

---

## 12. 结论

v5 在保留“信道中心、去 Agent 依赖、MCP 适配化”主线的同时，已经把会话中识别出的关键生产风险纳入主方案，避免后续出现以下典型返工：

- worker/route 膨胀导致内存长期上升
- 去重清理导致 CPU 退化
- 崩溃后消息丢失且不可重放
- 多平台会话模型不兼容
- webhook 场景乱序导致对话错位

该版本可直接进入阶段 1 开发。
