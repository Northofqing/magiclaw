# Phase 1 项目计划: 稳定内核

**基于**: docs/phase1-architecture.md v1.1
**日期**: 2026-06-17
**预计工期**: 2 周 (10 工作日)

---

## Review Gate 对齐

本计划与 [implementation-gap-solution.md](implementation-gap-solution.md) 中的 Formal Review Checklist 采用同一套口径：

- 只有 `closed` 能力计入阶段完成度。
- 单测是必要条件，闭环测试才是合并条件。
- Phase 1 不能只看 Domain/Adapter 单点测试，必须看到运行时主闭环。
- Phase 1.5 / Phase 2 的目标必须覆盖 MCP stdout 零污染、ilink 契约、sync_buf 持久化、Outbox/DLQ 恢复等 P0 红线。

**Phase 1 Gate 定义**：

1. 主链路闭环跑通：Inbound -> Dedup -> Route -> Queue -> Reorder -> Worker -> GC。
2. 所有 P0 红线条目在 review checklist 中可被勾选为 closed。
3. 运行时装配由 main/bootstrap 完成，不存在核心能力挂空挡。
4. 任何 stub / preview / experimental 不计入完成度。

---

## 里程碑

| # | 里程碑 | 目标日期 | 验收标准 |
|---|--------|---------|---------|
| M1 | Domain 层完成 | Day 3 | RouteKey + Conversation + Message + ReorderWindow 编译通过, 单元测试通过, review gate 2.1 可勾选 |
| M2 | 内核运行时完成 | Day 6 | Dedup + Route + GC Janitor 集成可运行, 消息端到端流动, review gate 7.2 主闭环可勾选 |
| M3 | WeChat 信道接入 | Day 8 | sync_buf 持久化, ilink 协议解析, contract test 通过, review gate 7.5 可勾选 |
| M4 | Phase 1 验收 | Day 10 | 全部验收清单勾选, 覆盖率达标, Formal Review Checklist 的 Phase 1 P0 全部为 closed |

---

## 任务拆分

> 说明：以下任务中的“单元测试通过”不代表完成；只有对应闭环测试和 review gate 通过才算 closed。

### Phase 1a: Domain 层 (Day 1-3)

核心原则: **纯域模型, 零外部依赖, 先行建立**。

| ID | 任务 | 工时 | 依赖 | 验收标准 | 涉及数据红线 |
|----|------|------|------|---------|-------------|
| D1 | 创建 Cargo 项目骨架 + `lib.rs`/`main.rs` | 1h | - | `cargo build` 通过, 模块层次就位 | - |
| D2 | 实现 `RouteKey` 值对象 | 2h | D1 | `RouteKey` 含 channel/conversation_id/peer_id/conversation_type, Hash+Eq+Clone derive | **红线2.1**: Conversation 为一等对象, RouteKey 包含 conversation 维度 |
| D3 | 实现 `ConversationType` 枚举 + `ChannelId` 类型 | 1h | D2 | `Direct/Group/Thread/BotSession` 四变体, `ChannelId` 为 newtype | **红线2.1** |
| D4 | 实现 `Message` 实体 + `AuditMark` | 2h | D2 | Message 含 id/route_key/sequence/timestamp/direction/content/audit_mark | **红线2.6**: 审计标记可追溯 |
| D5 | 实现 `Conversation` 聚合根 | 3h | D2,D3 | 聚合拥有 ReorderWindow + last_active, `ConversationState` 状态机 | **红线2.1**: Conversation 为一等对象 |
| D6 | 实现 `ReorderWindow` 领域服务 | 4h | D4 | insert/flush_all/handle_late 逻辑, 有 seq 按 seq 排 / 无 seq 按 timestamp + window | **红线2.2**: 乱序按策略重排, 迟到幂等+审计 |
| D7 | 实现 `BackpressureConfig` + `BackpressureAction` | 1h | D2 | per_route_buffer/inbound_channel_capacity 配置, DropOldest/DropNewest/Block 枚举 | - |
| D8 | 定义 Port traits (`DedupCache`, `SyncBufStore`, `ConversationQueue`, `ConversationGC`) | 2h | D2,D4 | 四个 trait 纯抽象, 无任何实现依赖 | **红线2.6**: GC 抽象不泄露实现 |
| D9 | Domain 层单元测试 | 3h | D2-D8 | RouteKey 等值比较, ReorderWindow 排序正确性, Conversation 状态转换 | - |

**小计**: 19h (~2.5 天)

---

### Phase 1b: Adapter 实现 (Day 3-5)

| ID | 任务 | 工时 | 依赖 | 验收标准 | 涉及数据红线 |
|----|------|------|------|---------|-------------|
| A1 | 实现 `MokaDedupCache` | 2h | D8 | 集成 moka crate, TTL 5min, max 2M, `check_and_set` 正确返回 true/false | **红线2.2**: Dedup 使用 TTL cache, 无全量 retain |
| A2 | 实现 `SqliteSyncBufStore` | 3h | D8,D1 | `save/load` 正确, WAL 模式, `INSERT OR REPLACE` 语义 | **红线2.3**: sync_buf 持久化 |
| A3 | 实现 `ConversationStore` (含 bounded queue + GC) | 5h | D8,D5 | per-route bounded queue (256), `enqueue` 背压处理, GC Janitor 60s 扫描, 30min idle 回收 | **红线2.2**: 同 RouteKey 串行, 30min GC; **红线2.5**: 隔离 |
| A4 | Adapter 层单元测试 | 3h | A1-A3 | Dedup hit/miss, SyncBuf round-trip, ConversationStore spawn/GC 正确性 | - |

**小计**: 13h (~2 天)

---

### Phase 1c: Application 层 (Day 5-6)

| ID | 任务 | 工时 | 依赖 | 验收标准 | 涉及数据红线 |
|----|------|------|------|---------|-------------|
| U1 | 实现 `Deduplicate` 用例 | 1h | A1 | 调用 DedupCache, hit → 丢弃+audit, miss → 放行 | **红线2.2** |
| U2 | 实现 `RouteMessage` 用例 | 3h | A3,U1 | inbound channel bounded (4096), 背压 Drop Oldest, dedup → route → enqueue 完整链路 | **红线2.2**: 同 RouteKey 串行, 不同并行; **红线2.5** |
| U3 | 实现 `GCJanitor` 用例 | 2h | A3 | tokio interval 60s, 扫描+回收 idle Conversation, FlushAndExit 安全流程 | **红线2.2**: 30min idle GC |
| U4 | Application 层集成测试 | 2h | U1-U3 | 消息去重→路由→入队→串行处理完整链路, 背压触发+丢弃审计 | - |

**小计**: 8h (~1 天)

---

### Phase 1d: WeChat 信道 (Day 6-8)

| ID | 任务 | 工时 | 依赖 | 验收标准 | 涉及数据红线 |
|----|------|------|------|---------|-------------|
| W1 | 实现 ilink 协议解析 (`ret`, `errcode`, `X-WECHAT-UIN`) | 3h | D1 | 错误码矩阵全映射, header 提取, 未知 errcode 不 panic | **红线2.4**: ilink contract test |
| W2 | 实现 AES-128-ECB/PKCS7 加解密 | 3h | W1 | round-trip 正确, 错误密文不 panic, padding 边界 case | **红线2.4**: AES-128-ECB/PKCS7 |
| W3 | 实现 `WeChatSession` (sync_buf 持久化 + 恢复) | 4h | A2,W1 | sync_buf 每次更新落盘, 重启从 DB 恢复断点继续 | **红线2.3**: sync_buf 持久化并重启可恢复 |
| W4 | 实现 `WeChatChannel` (实现 Channel trait) | 4h | W3,U2 | start/stop/send_message/health_check, 入站推送到 inbound channel | **红线2.4**: MCP stdio 零污染(业务日志 stderr) |
| W5 | ilink Contract Tests | 3h | W2,W1 | ret/errcode/sync_buf/X-WECHAT-UIN/AES 每个契约项有覆盖 | **红线2.4**: contract test 全部通过 |
| W6 | sync_buf 崩溃恢复测试 | 2h | W3 | 模拟进程 kill → 重启 → sync_buf 恢复 → 从断点继续, SLA <30s | **红线2.3**: 崩溃恢复后可继续未完成发送 |

**小计**: 19h (~2.5 天)

---

### Phase 1e: 基础设施 + 验收 (Day 8-10)

| ID | 任务 | 工时 | 依赖 | 验收标准 | 涉及数据红线 |
|----|------|------|------|---------|-------------|
| I1 | SQLite 数据库初始化 (`infrastructure/db.rs`) | 2h | D1 | 建表 sync_buf, WAL 模式, 连接池 | **红线2.3**: 核心状态持久化 |
| I2 | 结构化日志初始化 (`infrastructure/tracing.rs`) | 2h | D1 | stdout 仅协议, stderr JSON 格式, 关键指标采集 | **红线2.4**: MCP stdio 零污染 |
| I3 | 配置加载 (`infrastructure/config.rs`) | 1h | D1 | backpressure 容量/Dedup TTL/GC timeout 可配置 | - |
| I4 | 端到端集成测试 | 4h | 全部 | 完整消息流: 入站→解密→dedup→路由→排序→sync_buf 持久化 | - |
| I5 | 压力测试 | 2h | I4 | 验证 bounded channel 背压触发, GC 正常回收 | - |
| I6 | 覆盖率报告 + 验收清单 | 2h | I4,I5 | 行覆盖率 ≥80%, 核心路径 ≥95%, Phase 1 验收清单全部勾选 | **全部红线** |

**小计**: 13h (~1.5 天)

---

## 总工时

| Phase | 工时 | 天数 |
|-------|------|------|
| 1a Domain | 19h | 2.5 |
| 1b Adapters | 13h | 2 |
| 1c Application | 8h | 1 |
| 1d WeChat | 19h | 2.5 |
| 1e 基础设施 | 13h | 1.5 |
| **合计** | **72h** | **~9.5 天** |

含 20% buffer ≈ **10 工作日 (2 周)**

---

## 依赖关系图

```
D1 (骨架)
 ├──> D2 (RouteKey) ──> D3 (ConversationType) ──> D5 (Conversation) ──> A3 (ConversationStore)
 │         │                    │                                            │
 │         ├──> D4 (Message) ───┤                                            │
 │         │        │           │                                            │
 │         │        └──> D6 (ReorderWindow) ──────────────────────────────┐  │
 │         │                                                              │  │
 │         └──> D7 (Backpressure) ─────────────────────────────────────┐  │  │
 │                                                                      │  │  │
 ├──> D8 (Port traits) ──> A1 (MokaDedup) ──> U1 (Deduplicate) ────┐   │  │  │
 │                     ├──> A2 (SqliteSyncBuf) ──> W3 (Session) ────┤   │  │  │
 │                     └──> A3 (ConversationStore) ──> U2 (Route) ──┤   │  │  │
 │                                                      │           │   │  │  │
 │                                                      └──> U3 (GC) ┤   │  │  │
 │                                                                   │   │  │  │
 ├──> I1 (DB) ──────────────────────────────────────────────────────┤   │  │  │
 ├──> I2 (Tracing) ─────────────────────────────────────────────────┤   │  │  │
 └──> I3 (Config) ──────────────────────────────────────────────────┤   │  │  │
                                                                      │   │  │  │
                      W1 (ilink parse) ──> W2 (AES) ──> W4 (Channel) ┤   │  │  │
                                                           │          │   │  │  │
                      W5 (contract tests) <────────────────┘          │   │  │  │
                      W6 (crash recovery) <───────────────────────────┘   │  │  │
                                                                           │  │  │
                      A4 (adapter tests) <─────────────────────────────────┘  │  │
                      U4 (app tests) <─────────────────────────────────────────┘  │
                      I4 (e2e) <────────────────────────────────────────────────────┘
                      I5 (stress) <── I4
                      I6 (coverage) <── I4, I5
```

**关键路径**: D1 → D2 → D4 → D6 → D8 → A3 → U2 → U3 → W4 → I4 → I6

---

## 数据红线映射

每个任务涉及的红线检查:

| 红线 (AGENTS.md 第二部分) | Phase 覆盖? | 对应任务 |
|---------------------------|-------------|---------|
| 2.1 Core 不依赖 Agent, Conversation 一等对象, RouteKey 含 conversation | Phase 1 | D2, D3, D5 |
| 2.2 同 RouteKey 串行, Idle GC, TTL Dedup, Reorder, 迟到处理 | Phase 1 | D6, A1, A3, U1, U2, U3 |
| 2.3 Inbox/Outbox/DLQ, sync_buf 持久化, 崩溃恢复 | Phase 1 (sync_buf); Phase 2 (Inbox/Outbox/DLQ) | A2, W3, W6 |
| 2.4 MCP stdio 零污染, ilink contract test, 路径隔离, 流式上传 | Phase 1 (ilink, stdio); Phase 1.5 (MCP); Phase 3 (隔离, 上传) | W1, W2, W5, I2 |
| 2.5 Circuit Breaker, Bulkhead | Phase 5 | Phase 1 预留背压基础 |
| 2.6 审计日志, 不可篡改, ≥5年保留 | Phase 1 (基础审计标记); Phase 2 (完整审计) | D4 (AuditMark) |

---

## 风险与缓解

| 风险 | 影响 | 概率 | 缓解 |
|------|------|------|------|
| moka Cache 行为与预期不符 | 中 | 低 | A1 有独立测试, 提前验证 TTL 驱逐行为 |
| SQLite WAL 性能不足 | 高 | 低 | sync_buf 写入频率可控 (每次更新), 提前 benchmark |
| ReorderWindow 边界 case (timestamp 回退等) | 中 | 中 | D6 单元测试覆盖边界: 相同 timestamp, 大幅跳跃, 负值 |
| ilink 协议未文档化的行为 | 高 | 中 | W5 contract test 基于旧 Python 实现的行为快照 |
| tokio 并发模型与预期不符 | 中 | 低 | 使用成熟的 mpsc channel + interval 模式, 风险可控 |
