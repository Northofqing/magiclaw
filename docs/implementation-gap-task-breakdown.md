# 当前未闭环项任务拆解

**日期**: 2026-06-17  
**用途**: 基于当前代码状态和 `RUST_MIGRATION_V5.md` / `CLAUDE.md` 的差距，拆解还未完成的闭环任务，供后续按任务逐项开发。

---

## 1. 结论

当前仓库已经完成了部分可运行能力，尤其是：
- `cargo run` daemon 常驻启动
- `cargo run -- send --message "..."` 的微信发送路径
- `GET /api/window_status` 的 token 窗口观测
- MCP stdio 基础协议骨架

但按方案和 review gate 口径，仍有一批能力 **未闭环**，不能算 `closed`：
- 微信入站消息还没有真正接到统一主链路
- MCP 的 `list_peers` / `login` 仍是 preview/stub
- iLink 的 AES-128-ECB / PKCS7 仍是 stub
- Feishu / Dingtalk 仍是 skeleton
- 多信道隔离、媒体上传流式、REST 鉴权、完整审计、Outbox/DLQ 仍未完全闭环

---

## 2. 当前状态分级

### 2.1 已接近 `closed`

| 能力 | 当前状态 | 说明 |
|------|----------|------|
| daemon 常驻启动 | closed | `cargo run` 可起 daemon |
| CLI 发送到微信 | closed | `cargo run -- send --message` 可实际发送 |
| daemon send API | closed | `/api/send` 可用，支持 token 观测和窗口状态 |
| 窗口观测 | closed | `/api/window_status` 可查看 peer token 状态 |
| MCP stdio 基础骨架 | experimental | 具备协议入口，但工具能力未全闭环 |

### 2.2 仍是 `experimental`

| 能力 | 当前状态 | 说明 |
|------|----------|------|
| 微信长轮询刷新 token | experimental | 能刷新，但不能保证无新 inbound 时持续可用 |
| 30 秒节流压测稳定性 | experimental | 仅证明策略有效，不代表协议闭环完成 |
| MCP `send` | experimental | send 可工作，但 MCP 整体最小接口未完成 |

### 2.3 仍是 `stub` / `skeleton`

| 能力 | 当前状态 | 说明 |
|------|----------|------|
| MCP `list_peers` | stub | 仍返回 preview |
| MCP `login` | stub | 仍返回 preview |
| WeChat 入站到主链路 | stub | start 未把消息推入 inbound_tx |
| AES-128-ECB / PKCS7 | stub | 仍是 pass-through |
| Feishu 通道 | skeleton | 仅 send/health 占位 |
| Dingtalk 通道 | skeleton | 仅 send/health 占位 |

---

## 3. 任务拆解总览

### 3.1 优先级定义

- **P0**: 不完成就不能说主链路闭环成立
- **P1**: 影响可用性、稳定性、可维护性，但可在 P0 之后收敛
- **P2**: 预留能力、文档化、可观测性增强

---

## 4. P0 任务拆解

### P0-1 微信入站接入主链路

**目标**: 让微信收到的 inbound 消息真正进入统一处理链路，而不是只用于 token 刷新。

**现状**:
- `WeChatChannel::start` 接收了 `inbound_tx`，但当前未使用。
- 运行时主循环在消费 `inbound_rx`，说明主链路已经准备好，但微信没有接上。

**子任务**:
1. 在微信信道中添加 long-poll/webhook 到 `inbound_tx` 的推送逻辑。
2. 将 raw iLink 消息映射为统一 `Message`。
3. 为 inbound 消息补齐 `route_key` / `conversation_id` / `conversation_type`。
4. 确保入站消息写入 inbox、dedup、route、queue 流程。
5. 给 `start` 增加可停机的后台任务句柄。

**依赖**:
- 现有 `runtime::start_background` 的 inbound_rx 主循环
- 现有 `Message` / `RouteKey` / `Conversation` 类型

**验收标准**:
- 微信收到一条消息后，能在主链路里看到 inbox 写入和 route 投递。
- `cargo test` 的单测不算完成，必须补一条集成级闭环测试。
- 断言至少覆盖一条 inbound 消息从信道进入 `route_message`。

**闭环定义**:
- WeChat long-poll → inbound_tx → runtime inbound_rx → inbox → dedup → route → queue

---

### P0-2 MCP 最小接口闭环

**目标**: 把 MCP 从“可启动骨架”变成“最小可用工具集”。

**现状**:
- `send` 已工作。
- `list_peers` / `login` 仍是 preview stub。

**子任务**:
1. 将 `list_peers` 对接真实联系人/群组来源。
2. 将 `login` 对接微信/其他平台登录流程。
3. 确保 `tools/list` 返回的 schema 与实现一致。
4. 增加 MCP 级闭环测试，验证 JSON-RPC request/response。
5. 增加 stdout 零污染测试，确保协议流不被日志污染。

**依赖**:
- `ProtocolHandler`
- `ToolDispatcher`
- 平台侧联系人/登录能力

**验收标准**:
- `send/list_peers/login` 均可通过 MCP 客户端调用。
- stdout 只包含 JSON-RPC。
- stderr 承载所有 tracing 日志。

**闭环定义**:
- MCP framed request → handler → tool dispatch → domain/app entry → framed response

---

### P0-3 iLink 协议契约测试闭环

**目标**: 把 iLink 的关键协议行为变成可回归的 contract test。

**现状**:
- `ret` / `errcode` / `X-WECHAT-UIN` / `sync_buf` 相关解析已局部实现。
- AES-128-ECB / PKCS7 仍是 stub。
- 目前缺少覆盖协议契约的完整测试集合。

**子任务**:
1. 为 `ret` / `errcode` 建立 JSON fixture 测试。
2. 为 `X-WECHAT-UIN` 增加 header 断言。
3. 为 `getupdates` 的 `sync_buf` 增量行为增加回归测试。
4. 把 AES-128-ECB / PKCS7 改成真实实现并补 vector 测试。
5. 把 `sendmessage` / `getupdates` 的关键请求体/响应体变成固定 contract fixture。

**依赖**:
- `src/channels/wechat/ilink.rs`
- 现有 send / getupdates 实现

**验收标准**:
- 每个契约点至少一条测试用例。
- AES 向量测试通过。
- 真实协议字段变更会导致测试失败而不是静默漂移。

**闭环定义**:
- 协议输入 → 解析 → 断言 → 失败可定位

---

### P0-4 发送链路统一化

**目标**: 把文本发送、媒体发送、失败恢复统一成一致入口。

**现状**:
- 现在以文本发送为主，恢复逻辑在多个地方分散。
- 媒体发送路径尚未统一成 `send_media_with_recovery`。

**子任务**:
1. 抽出统一的 `send_text_with_recovery`。
2. 预留 `send_media_with_recovery` 接口。
3. 把 token 刷新 / stale 标记 / retry 逻辑从 CLI 侧收拢到一个策略层。
4. 增加发送状态机的可观测日志。

**验收标准**:
- 发送恢复只从一个入口实现。
- CLI / daemon / MCP 不重复实现重试分支。

---

## 5. P1 任务拆解

### P1-1 多信道从 skeleton 变成接线完成

**目标**: 至少把 Dingtalk / Feishu 从 skeleton 提升到“可接线、可隔离、可观测”。

**子任务**:
1. 明确每个信道的 capability registry。
2. 为 Feishu / Dingtalk 补充最小入站或发送闭环。
3. 把不同信道的配置、日志、队列路径做命名空间隔离。

**验收标准**:
- 多信道不会共用 session / sync_buf / inbox / outbox 路径。
- 单一信道故障不拖垮其他信道。

---

### P1-2 媒体上传流式处理

**目标**: 支持大文件流式/分段上传，避免一次性全量读入内存。

**子任务**:
1. 抽象媒体输入源为 reader/stream。
2. 实现分块上传和失败重试。
3. 增加大文件内存占用测试。

**验收标准**:
- 大文件发送不触发全量读入。
- 分块上传失败可恢复。

---

### P1-3 REST Adapter 最小认证

**目标**: 如果开放 REST 口，必须带最小认证，不裸口开放。

**子任务**:
1. 加 token 鉴权中间件。
2. 默认关闭外网监听。
3. 增加未授权访问测试。

---

### P1-4 审计日志闭环

**目标**: 高风险操作、发送决策、自动放行等必须留痕。

**子任务**:
1. 定义 audit event schema。
2. 每个关键决策写入审计。
3. 增加审计不可篡改策略和保留期说明。

---

## 6. P2 任务拆解

### P2-1 统一 capability registry

**目标**: 用能力表描述每个信道支持什么。

**内容**:
- 是否支持 sequence
- 是否支持 typing
- 是否支持 webhook
- 是否支持 media

---

### P2-2 指标与追踪

**目标**: 统一 metrics / tracing 注入点。

**内容**:
- inbound / outbound 计数
- retry / DLQ 指标
- token window 状态指标

---

## 7. 推荐执行顺序

1. 先做 **P0-1 微信入站接入主链路**
2. 同步补 **P0-2 MCP 最小接口闭环**
3. 再做 **P0-3 iLink 契约测试闭环**
4. 接着收敛 **P0-4 发送链路统一化**
5. 最后再展开 **P1 / P2** 的多信道、媒体、REST、审计能力

---

## 8. 当前不建议立即做的事

- 不建议先继续扩展 token probing 策略，除非主闭环已经补齐。
- 不建议先做更多 skeleton 信道能力，而忽略微信入站主链路。
- 不建议把 `list_peers` / `login` 继续作为“已完成能力”宣传，它们现在只是 preview。

---

## 9. 任务落地判定规则

一个任务只有同时满足以下条件，才算真正完成：

1. 代码已接入运行时组合根
2. 关键失败路径有明确处理
3. 有对应闭环测试
4. review checklist 中能标成 `closed`

单元测试通过、stub 可运行、日志有输出，都不算完成。
