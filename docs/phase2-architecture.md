# Phase 2 Architecture: 可恢复投递 (Recoverable Delivery)

**版本**: 1.0
**日期**: 2026-06-17
**状态**: Draft

## 0. Review Gate 对齐

本阶段文档仅描述 Phase 2 的设计目标，不代表实现已 closed。只有满足下列条件才算完成：

- Send Request -> Outbox.pending -> Worker.dequeue -> Outbox.sending -> Channel.send -> sent/retrying/dead_letter -> restart recovery 闭环跑通。
- Inbox / Outbox / DLQ 不仅有数据结构，还必须接入运行时组合根和恢复路径。
- 任何 `stub` / `preview` / `draft` 说明都不计入阶段完成度。

## 1. 目标

在 Phase 1 稳定内核 + Phase 1.5 MCP 适配之上，实现消息的可靠投递：
- Inbox：入站幂等记录，防重复消费
- Outbox：出站状态机，可观测每个消息的发送状态
- DLQ：超过重试阈值的死信队列，支持查看和重放
- 崩溃恢复：进程重启后自动恢复未完成的发送

## 2. 核心模型

### 2.1 发送状态机

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

### 2.2 重试策略

```rust
pub struct RetryConfig {
    /// Max retry attempts before DLQ.
    pub max_retries: u32,             // default 5
    /// Base backoff in milliseconds.
    pub base_backoff_ms: u64,         // default 1000
    /// Max backoff in milliseconds.
    pub max_backoff_ms: u64,          // default 60000
    /// Jitter factor (0.0 - 1.0).
    pub jitter: f64,                  // default 0.1
}

// next_retry_delay = min(base * 2^attempt + jitter, max_backoff)
```

### 2.3 数据模型

```rust
// domain/storage/inbox.rs
pub struct InboxEntry {
    pub id: String,              // message_id
    pub channel: String,
    pub conversation_id: String,
    pub payload: String,         // serialized Message
    pub status: InboxStatus,
    pub created_at: i64,
    pub updated_at: i64,
}

pub enum InboxStatus {
    Pending,
    Processing,
    Processed,
}

// domain/storage/outbox.rs
pub struct OutboxEntry {
    pub id: String,              // message_id
    pub route_key: String,       // serialized RouteKey
    pub payload: String,         // serialized MessageContent
    pub status: OutboxStatus,
    pub retry_count: u32,
    pub next_retry_at: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub enum OutboxStatus {
    Pending,
    Sending,
    Sent,
    Retrying,
    DeadLetter,
}

// domain/storage/dead_letter.rs
pub struct DeadLetterEntry {
    pub id: String,
    pub source: String,          // "outbox"
    pub payload: String,
    pub reason: String,
    pub created_at: i64,
}
```

## 3. 数据流

```
Inbound flow:
  Channel Adapter → Dedup → Inbox.write(pending)
                         → Conversation Worker
                         → Inbox.mark(processing) → process → Inbox.mark(processed)

Outbound flow:
  Message enqueued → Outbox.write(pending)
                  → SendWorker dequeues → Outbox.mark(sending)
                  → Channel.send_message()
                  → success → Outbox.mark(sent)
                  → failure → Outbox.mark(retrying, next_retry_at)
                           → RetryWorker picks up at next_retry_at
                           → retries depleted → Outbox.mark(dead_letter)
                                              → DeadLetter.insert()

Crash Recovery (startup):
  1. Scan Outbox WHERE status IN ('sending', 'retrying')
  2. Re-enqueue to SendWorker
  3. Messages in 'pending' are picked up by normal dequeue
```

## 4. Port 接口

```rust
// domain/ports/
pub trait InboxRepo: Send + Sync {
    fn insert(&self, entry: &InboxEntry) -> Result<()>;
    fn mark_status(&self, id: &str, status: InboxStatus) -> Result<()>;
    fn exists(&self, id: &str) -> Result<bool>;
}

pub trait OutboxRepo: Send + Sync {
    fn insert(&self, entry: &OutboxEntry) -> Result<()>;
    fn mark_status(&self, id: &str, status: OutboxStatus, error: Option<&str>) -> Result<()>;
    fn mark_retrying(&self, id: &str, retry_count: u32, next_retry_at: i64, error: &str) -> Result<()>;
    fn fetch_pending(&self, limit: usize) -> Result<Vec<OutboxEntry>>;
    fn fetch_retryable(&self, now_ts: i64, limit: usize) -> Result<Vec<OutboxEntry>>;
    fn recover_after_crash(&self) -> Result<Vec<OutboxEntry>>; // status IN ('sending','retrying')
}

pub trait DeadLetterRepo: Send + Sync {
    fn insert(&self, entry: &DeadLetterEntry) -> Result<()>;
    fn list(&self, limit: usize) -> Result<Vec<DeadLetterEntry>>;
    fn replay(&self, id: &str) -> Result<OutboxEntry>; // move back to outbox
}
```

## 5. Application 用例

- `ProcessInbound` — 写入 Inbox + 更新状态
- `SendWorker` — 从 Outbox 取出 pending → 调用 Channel.send → 更新状态
- `RetryWorker` — 定期扫描 retryable → 重试发送
- `CrashRecovery` — 启动时扫描未完成发送 → 重新入队
- `DlqManager` — 查看死信 / 重放

## 6. 模块结构

```
src/
├── domain/storage/
│   ├── mod.rs
│   ├── inbox.rs           # InboxEntry, InboxStatus
│   ├── outbox.rs          # OutboxEntry, OutboxStatus, RetryConfig
│   └── dead_letter.rs     # DeadLetterEntry
├── domain/ports/
│   ├── inbox_repo.rs      # InboxRepo trait
│   ├── outbox_repo.rs     # OutboxRepo trait
│   └── dead_letter_repo.rs # DeadLetterRepo trait
├── adapters/
│   ├── sqlite_inbox.rs    # SqliteInboxRepo
│   ├── sqlite_outbox.rs   # SqliteOutboxRepo
│   └── sqlite_dead_letter.rs # SqliteDeadLetterRepo
├── application/
│   ├── inbox_processor.rs # ProcessInbound use case
│   ├── outbox_worker.rs   # SendWorker + RetryWorker
│   ├── crash_recovery.rs  # Crash recovery use case
│   └── dlq_manager.rs     # DLQ list/replay
└── infrastructure/
    └── db.rs              # (update: connection sharing)
```

## 7. 失败模式分析

| 场景 | 状态 | 恢复 |
|------|------|------|
| 发送中途 crash | Outbox: sending | 重启后 `recover_after_crash` 扫描 → 重新发送 |
| 重试中途 crash | Outbox: retrying | 同 crash recovery，基于 next_retry_at 重试 |
| 发送成功但 crash 前未标记 | Outbox: sending | 重新发送 → Channel 返回成功 → 逻辑幂等 |
| DB 写入失败 | 磁盘满 / 损坏 | 错误返回，不更新状态，上层告警 |
| 重试耗尽 | Outbox → DeadLetter | 进入 DLQ，等待手工重放 |
| DLQ 重放 | DeadLetter → Outbox(pending) | 重新走完整状态机 |

## 8. 回滚方案

- Inbox/Outbox/DLQ 都是新增模块，删除对应目录和表即可
- 不影响 Phase 1 核心路由能力
- 可降级为"无持久化"模式（跳过 Inbox/Outbox 写入）

## 9. Phase 2 验收清单

- [ ] Inbox 写入 + 状态更新可用
- [ ] Outbox 状态机 `pending → sending → sent` 完整
- [ ] Outbox `sending → retrying → sent` 完整
- [ ] Outbox `retrying → dead_letter` 阈值触发
- [ ] 指数退避 + jitter 正确计算
- [ ] 崩溃恢复：重启后 sending/retrying 消息重新投递
- [ ] DLQ 查看 + 重放可用
- [ ] 单元测试覆盖率 ≥ 80%
- [ ] 核心发送链路覆盖率 ≥ 95%
