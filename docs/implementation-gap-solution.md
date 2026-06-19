# 落地偏差问题的解决方案

**日期**: 2026-06-17  
**目的**: 解决当前仓库中“模块存在但运行时未闭环”“单测通过但阶段目标未真正落地”的问题。  
**适用范围**: Phase 1 / 1.5 / 2 当前整改。

---

## 1. 问题定义

当前代码的主要偏差不是“完全没实现”，而是出现了以下结构性问题：

1. 模块级实现存在，但未接入主运行链路。
2. 单元测试通过，但系统级验收项没有形成闭环。
3. 发送、恢复、审计、协议契约等红线能力停留在局部模块，而非系统能力。
4. skeleton / preview / stub 数量较多，掩盖了真实完成度。
5. 模型边界升级不完整，导致多账号隔离、重放恢复等能力无法自然成立。

这类问题的本质不是单点 bug，而是**交付方式和验收方式不匹配**。

---

## 2. 根因与对应解决方案

### 2.1 根因一: 开发顺序以“模块完成”为目标，而不是“主链路闭环”为目标

当前实现模式更像：

```text
先写 repo / worker / adapter / trait / unit test
再等待未来某个时点接入 main
```

这种方式的问题是：

- 每个模块局部看起来都正确。
- 代码量快速增长。
- 但直到很后面才会暴露“没有被运行时调用”的事实。

**解决方案**:

1. 后续开发改为“主链路闭环优先”。
2. 每个阶段必须先打通最小可运行路径，再补模块细节。
3. 所有新增模块只有在进入组合根后，才算真正完成。

**执行规则**:

- Phase 1 功能完成的定义，不是 `ConversationStore`、`DedupCache`、`ReorderWindow` 各自有测试。
- Phase 1 完成的定义是：真实入站消息进入运行时后，经过 dedup -> route -> reorder -> worker -> GC 的完整链路。
- Phase 2 功能完成的定义，不是 outbox/dlq repo 可单测。
- Phase 2 完成的定义是：真实发送请求进入系统后，经过 pending -> sending -> sent/retrying/dead_letter 的完整链路，并可重启恢复。

---

### 2.2 根因二: 组合根过弱，`main` 只做初始化，没有做系统装配

当前 `main` 更像“初始化了一些对象”，而不是“组装一套系统”。

这会导致：

- 模块即使存在，也可能没有任何调用方。
- 编译器不会报错。
- 单测覆盖不到“有没有接线”这个问题。

**解决方案**:

1. 把 `main` 当作唯一系统装配根。
2. 所有阶段红线能力必须在 `main` 或其明确委托的 bootstrap 模块里完成装配。
3. 每条关键链路都要能从入口一路追到执行点。

**执行规则**:

- 建立 `bootstrap` / `app_runtime` 组装模块，集中初始化：
  - DB pool
  - dedup cache
  - inbox/outbox/dlq repo
  - crash recovery
  - outbox worker
  - channel registry
  - MCP server / REST adapter / CLI adapter
  - audit writer
- `main` 中禁止保留下划线形式的“初始化但不参与业务”的核心依赖。
- 所有红线能力都必须回答两个问题：
  - 谁在调用它？
  - 启动后它何时开始生效？

---

### 2.3 根因三: 验收方式偏单元测试，缺少阶段闭环测试

当前测试说明“模块逻辑成立”，但无法证明“阶段目标成立”。

例如：

- outbox worker 单测通过，不等于运行时发送链路可靠。
- MCP handler 单测通过，不等于 stdio framing 真实兼容。
- sync_buf store 单测通过，不等于真实 WeChat 会话能重启恢复。

**解决方案**:

1. 为每个阶段增加“闭环验收测试”。
2. 测试目标从“函数行为”提升到“系统行为”。
3. 阶段 gate 不再只看 `cargo test` 是否为绿。

**执行规则**:

- Phase 1 必须新增集成测试：
  - 同 RouteKey 串行，不同 RouteKey 并行
  - dedup 生效
  - reorder window 生效
  - idle GC 回收 worker
- Phase 1.5 必须新增协议测试：
  - MCP `Content-Length` framing 读写
  - stdout 零污染
- Phase 2 必须新增恢复测试：
  - send request 写入 outbox
  - worker 推进状态机
  - 失败重试进入 retrying
  - 超阈值进入 DLQ
  - restart 后 sending/retrying 可恢复

**阶段验收原则**:

- 单测是必要条件。
- 闭环测试是合并条件。

---

### 2.4 根因四: preview / skeleton / stub 过早扩张，稀释了完成定义

当前代码中存在较多：

- skeleton channel
- preview MCP tool
- stub crypto / protocol / login / list_peers

这些代码本身不是错误，但如果在主链路未闭环前扩张，会造成两类误判：

1. 功能数量很多，容易误以为阶段完成度很高。
2. 审查时注意力被新接口分散，反而忽略核心红线没真正落地。

**解决方案**:

1. 把能力分为 `closed` / `experimental` / `stub` 三类。
2. 只有 `closed` 能计入阶段完成度。
3. 所有 `stub` 和 `preview` 必须在代码和文档中显式隔离。

**执行规则**:

- `closed`: 已接入主流程，具备闭环测试，可作为阶段交付。
- `experimental`: 已接线但未满足生产级约束，不计入阶段验收。
- `stub`: 仅占位，不得写入“已完成能力”清单。

**管理规则**:

- 在 PR 描述里单列：
  - 本次真正完成的闭环能力
  - 本次新增但仍为 stub 的能力
- skeleton channel 和 preview tool 在主链路闭环前不再扩面。

---

### 2.5 根因五: 模型升级只做了一半，没有同步进入存储边界和恢复边界

典型表现：

- `RouteKey` 已升级到 conversation 维度。
- 但 account 维度没有同步进入 `RouteKey`、持久化 schema、审计模型、DLQ replay 模型。

这种“半升级”会导致：

- 多账号隔离做不实。
- replay 丢失原始路由。
- audit 无法反推真实上下文。

**解决方案**:

1. 核心模型变更必须同时落到三层：
  - 领域模型
  - 持久化 schema
  - 恢复 / 审计 / replay 模型
2. 引入“模型变更影响清单”。

**执行规则**:

每次调整 `RouteKey`、`Conversation`、`Message`、`OutboxEntry` 等核心模型时，PR 必须逐项回答：

1. 领域对象是否已更新？
2. DB schema 是否已更新？
3. adapter 序列化 / 反序列化是否已更新？
4. crash recovery 是否已更新？
5. DLQ replay 是否已更新？
6. audit 字段是否已更新？
7. 集成测试是否覆盖？

---

## 3. 具体整改策略

### 3.1 先修“交付方式”，再修“具体代码”

如果只逐个修 bug，不改变交付方式，后面还会继续出现同类问题。

因此整改顺序应该是：

1. 明确每个阶段的唯一主闭环。
2. 调整 `main` / bootstrap，让闭环真正可运行。
3. 为闭环增加系统级测试。
4. 再去补 stub / preview / skeleton。

---

### 3.2 建立“阶段闭环清单”

后续每个阶段只允许有一个主交付闭环：

#### Phase 1 主闭环

```text
Inbound Message
-> Dedup
-> Route Resolution
-> Per-Route Queue
-> Reorder Window
-> Worker Process
-> Idle GC
```

#### Phase 1.5 主闭环

```text
MCP Client
-> stdio framed request
-> MCP handler
-> domain/application entry
-> framed response
-> stdout zero-pollution
```

#### Phase 2 主闭环

```text
Send Request
-> Outbox.pending
-> Worker.dequeue
-> Outbox.sending
-> Channel.send
-> sent / retrying / dead_letter
-> restart recovery
```

任何一个阶段，只要主闭环没跑通，就不允许继续把精力投入更多扩展能力。

---

### 3.3 建立“未接线能力清单”制度

为避免再次出现“代码存在但没生效”，每次 PR 必须显式列出：

1. 新增模块清单
2. 已接线模块清单
3. 尚未接线模块清单
4. 接线计划或不接线理由

**判断标准**:

- 仅存在 `pub fn` / trait impl / repo impl / unit test，不算已落地。
- 只有被运行时入口或闭环测试覆盖到，才算已落地。

---

### 3.4 建立“红线能力不得挂空挡”规则

所有 P0 红线能力必须满足以下四个条件：

1. 有设计定义。
2. 有运行时调用点。
3. 有闭环测试。
4. 有故障恢复或失败路径定义。

只满足前两项不够，只满足前三项也不够。

---

## 4. 后续执行建议

### 第一阶段: 修交付结构

目标：把“模块存在”改成“运行时闭环存在”。

动作：

1. 提取 `bootstrap/runtime` 组装层。
2. 把 dedup / inbox / outbox / dlq / recovery / audit 全部接进运行时。
3. 删除或收敛未使用的核心依赖初始化。

### 第二阶段: 修核心闭环

目标：让 Phase 1 和 Phase 2 真正成立。

动作：

1. 打通 inbound 闭环。
2. 打通 outbound recoverable delivery 闭环。
3. 修正 DLQ replay 保真问题。

### 第三阶段: 修协议与隔离

目标：让系统满足迁移红线而不是只满足本地测试。

动作：

1. 修 MCP framing。
2. 修 WeChat ilink contract。
3. 引入 account 维度到核心模型和持久化。

### 第四阶段: 修生产级约束

目标：让骨架代码变成可上线能力。

动作：

1. 接入 audit 主链路。
2. 接入 circuit breaker / bulkhead。
3. 最后再补 skeleton channel 和 preview tool。

---

## 5. Definition of Done

本问题被认为“解决”，必须同时满足：

1. 主运行时中不存在核心能力初始化但不参与业务链路的情况。
2. 每个阶段至少有一条闭环测试直接覆盖其主目标。
3. 所有 P0 红线能力都能回答“谁调用、何时生效、失败如何恢复”。
4. `stub / preview / skeleton` 不再被误计入阶段完成度。
5. 模型升级会同步落到 schema、replay、audit、recovery 边界。

---

## 6. 一句话结论

这次问题出现的根因，不是“没人写代码”，而是**系统在用模块化的方式开发，却在用功能数量的方式验收**。  
解决方案也不是继续补零散模块，而是把交付和验收方式改成“主链路闭环 + 组合根装配 + 阶段系统测试”。

---

## 7. Formal Review Checklist

> 用途：作为 PR / 阶段评审的统一清单。只有满足“已接线 + 已闭环 + 已测试”才可勾选为闭环项。

### 7.1 Composition Root / 运行时装配

- [ ] `main` / `bootstrap` 是唯一系统装配根，没有旁路初始化。
- [ ] DB pool、dedup cache、inbox/outbox/dlq repo、crash recovery、outbox worker、channel registry、audit writer 都已接入主运行链路。
- [ ] 所有关键能力都能回答“谁在调用它、何时开始生效”。
- [ ] 未使用的核心依赖初始化已删除或明确标记为 experimental。

### 7.2 Phase 1 主闭环

- [ ] Inbound Message -> Dedup -> Route Resolution -> Per-Route Queue -> Reorder Window -> Worker Process -> Idle GC 已跑通。
- [ ] `RouteKey` 已包含 `channel / conversation_id / peer_id / conversation_type`。
- [ ] Conversation 已作为一等聚合根，Message 仅作为事件。
- [ ] 同 RouteKey 串行处理，不同 RouteKey 并行处理。
- [ ] 30 分钟 idle route 会自动回收，不产生 worker / queue 长期膨胀。
- [ ] Dedup 使用 TTL cache（moka），没有全量 retain 清理。
- [ ] 乱序消息按 sequence 或 reorder window 处理，迟到消息具备幂等和审计标记。
- [ ] `sync_buf` 每次更新都持久化，重启后可恢复。

### 7.3 Phase 1.5 主闭环（MCP）

- [ ] MCP Adapter 仅作为适配层存在，不污染 core。
- [ ] `send / list_peers / login` 工具定义清晰，preview / stub 与 closed 能力隔离。
- [ ] MCP stdio 只有协议输出，业务日志绝不进入 stdout。
- [ ] Content-Length framing 读写有真实协议测试覆盖。

### 7.4 Phase 2 主闭环（可靠投递）

- [ ] Outbox 状态机完整：pending -> sending -> sent / retrying / dead_letter。
- [ ] Inbox 用于幂等消费和重复消息屏蔽。
- [ ] 失败重试和 DLQ replay 可工作。
- [ ] 进程崩溃后，sending/retrying 状态可恢复。
- [ ] SQLite schema 与领域模型、replay、audit、recovery 同步。

### 7.5 WeChat / ilink 协议契约

- [ ] `ret` / `errcode` 映射清晰，未知错误码不会 panic。
- [ ] `X-WECHAT-UIN`、`context_token`、`sync_buf` 的处理已契约化。
- [ ] AES-128-ECB / PKCS7 加解密通过 contract test。
- [ ] WeChat 信道发送路径是统一的 `send_text_with_recovery` / `send_media_with_recovery`。
- [ ] 多账号 / 多通道隔离是路径级别的，而不是仅靠内存对象隔离。

### 7.6 可观测性 / 安全 / 风控

- [ ] 业务日志只走 stderr / 文件，不污染 stdout。
- [ ] 审计日志覆盖高风险操作（如自动加白名单、发送决策、恢复动作）。
- [ ] 审计日志为不可篡改存储，保留周期满足要求。
- [ ] REST adapter 不会默认暴露裸端口，必须有最小认证或 token 保护。
- [ ] AI / 外部平台 API 已纳入 Circuit Breaker。
- [ ] AI 执行池和发送执行池使用 Bulkhead 隔离。
- [ ] 媒体上传必须是流式/分段，不允许一次性读入大文件。

### 7.7 Stub / Preview 管理

- [ ] `stub` / `preview` / `experimental` 已显式区分。
- [ ] `stub` 能力不计入阶段完成度。
- [ ] skeleton channel 和 preview MCP tool 没有被误写成已完成能力。
- [ ] 每个 PR 都列出“新增模块 / 已接线模块 / 尚未接线模块 / 接线计划或理由”。

### 7.8 通过标准

- [ ] 上述 P0 项全部通过，才能进入下一阶段。
- [ ] 单测通过不是完成条件，闭环测试通过才算完成。
- [ ] 任何模型变更都必须同步检查领域模型、DB schema、adapter、recovery、DLQ replay、audit、测试。
