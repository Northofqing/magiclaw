# 项目规则

> 适用范围:Rust 迁移中的信道中心架构系统
> 强制力分级:**MUST**(违反即阻断) / **SHOULD**(强烈建议) / **MAY**(可选)
> 本文档为开发、审查、上线活动的最高约束。信道稳定与可恢复性优先级高于流程效率。

---

## 0. 总则

- **MUST** 本规则与 `RUST_MIGRATION_V5.md` 保持一致,若冲突执行更严格条款。
- **MUST** 第二部分红线作为第一部分开发流程的阶段验收项,二者不可割裂。
- **MUST** 所有流程产物落地到 Git,以 PR 为载体,checklist 逐项勾选后方可合并。
- **MUST** 规则冲突时按优先级处理:信道稳定与数据正确性 > 可恢复投递 > 流程合规 > 开发效率。
- **MAY** 紧急情况走"受控例外通道"(见第四部分),但必须留痕并事后复盘。

### 0.1 落地闭环规则(MUST)

> 解决"模块存在但运行时未闭环"、"单测通过但阶段目标未真正落地"的问题。本节为流程与审查的强制补充规则。

- **MUST** 以"主链路闭环"作为阶段交付单位,禁止以"模块数量"或"单测数量"替代阶段完成度。
- **MUST** 每个阶段先打通一条最小可运行闭环,再补 repo / adapter / worker / stub 等外围实现。
- **MUST** 新增核心模块只有在进入运行时组合根(`main` 或明确 bootstrap/runtime 装配模块)后,才算真正完成。
- **MUST** 所有 P0 / P1 关键能力都能回答:① 谁在调用 ② 启动后何时生效 ③ 失败时如何恢复。
- **MUST NOT** 在主链路未闭环前,继续扩张 preview / skeleton / stub 能力面来制造完成度假象。

### 0.2 运行时装配规则(MUST)

- **MUST** `main` 或其委托的 bootstrap/runtime 模块作为唯一系统装配根,负责把阶段红线能力接入真实运行链路。
- **MUST** 禁止保留"已初始化但不参与业务链路"的核心依赖;下划线变量形式的核心组件初始化默认视为未落地。
- **MUST** 所有关键链路都能从入口追踪到执行点,不得存在"模块已实现但无运行时调用方"的核心能力。

### 0.3 验收与测试规则(MUST)

- **MUST** 单元测试仅证明模块局部逻辑成立,不得单独作为阶段完成依据。
- **MUST** 每个阶段至少有一条直接覆盖主闭环的系统级 / 集成级验收测试,作为合并 Gate。
- **MUST** 阶段验收先看主闭环是否跑通,再看外围扩展能力是否完整。

### 0.4 能力分级规则(MUST)

- **MUST** 所有新增能力在 PR 与文档中标记为 `closed` / `experimental` / `stub` 三类之一。
- **MUST** 仅 `closed` 能计入阶段完成度;其定义为:已接入主流程 + 具备闭环测试 + 满足当前阶段红线。
- **MUST** `experimental` 指已接线但未满足生产级约束,不得写入"已完成能力"清单。
- **MUST** `stub` / `preview` / `skeleton` 仅允许作为占位,不得写入阶段验收结论。

### 0.5 模型变更同步规则(MUST)

- **MUST** 核心模型变更必须同步落到:领域模型、持久化 schema、序列化边界、crash recovery、DLQ replay、audit 字段、集成测试。
- **MUST** 每次调整 `RouteKey`、`Conversation`、`Message`、`OutboxEntry` 等核心对象时,PR 中逐项回答上述影响面是否已同步更新。
- **MUST NOT** 允许只改领域对象而不改 schema / replay / recovery / audit 的"半升级"模型变更进入主分支。

---

## 一、开发流程

按序执行,**上一步未达成 Definition of Done(DoD)不进入下一步**。每步 DoD 即该步的合并 Gate。

### 流程步骤与完成判定

| 步骤 | 命令 / 动作 | Definition of Done(MUST 全部满足) |
|------|------------|------------------------------------|
| 1 | `/architecture-patterns` 设计方案文档落地 | 文档已提交且包含:① 数据流图 ② 失败模式分析(每个数据源失败如何处理)③ 回滚方案 ④ 与旧模块的关系说明 |
| 2 | AI产品经理、网络通信资深技术研发、用户、系统DDD架构四角挑战方案,反哺给 1 | 四个角色各留书面意见;所有 **Blocking 异议**已闭环;退出条件见下方"收敛规则" |
| 3 | `/project-planner` 拆分计划文档落地 | 每个任务可独立验收;依赖关系明确;每个任务标注涉及的迁移红线 |
| 4 | `/andrej-karpathy-skills:karpathy-guidelines` 按规则编码 | 通过 lint + 单测;**无 mock 残留于生产路径**;每个外部依赖失败路径有显式处理 |
| 5 | `/review` 审查代码 | ① review 意见全部记录 ② **旧模块接入检查表逐项勾选**(见下)③ 迁移红线检查表逐项勾选 |
| 6 | 修复 review 问题 | 所有问题 resolved,或显式标注 `wontfix` + 理由并经审查者确认 |
| 7 | 测试验证 | **单元测试行覆盖率 ≥ 80%**;核心信道 / 投递链路覆盖率 **≥ 95%**;回归通过;崩溃恢复与隔离验证通过;不通过按"根因回退"处理 |

### 收敛规则(第 2 步)

- **MUST** 四角挑战以"**无新增 Blocking 异议**"为退出条件。
- **MUST** 单一方案挑战轮次上限 **3 轮**;第 3 轮后仍有 Blocking 异议则升级决策,不得无限循环。

### 根因回退(第 7 步)

测试不通过时,**MUST 按根因回退到对应步骤**,而非一律回第 4 步:

| 失败根因 | 回退到 |
|----------|--------|
| 信道模型 / 架构边界设计错误 | 步骤 1 |
| 计划遗漏 / 任务拆分不当 | 步骤 3 |
| 实现 bug | 步骤 4 |
| 迁移红线违规 | 步骤 4,并复查步骤 1 的失败模式分析 |

### 旧模块接入检查表(第 5 步 · MUST)

> 新能力上线后,**必须对照旧模块**,逐个回答,不得跳过。

- [ ] 列出所有与新能力同类 / 相关的现有模块
- [ ] 对每个旧模块回答:**是否应升级接入新能力?**
  - 接入 → 记录接入计划 / PR
  - 不接入 → 记录明确理由
- [ ] 确认无"应接入却遗漏"的旧模块

### 流程载体(MUST)

- **MUST** 每步产出物作为 PR 的 checklist 项或关联文档。
- **MUST** 合并需指定审查者批准;前 7 步证据可追溯。
- **MUST** 设计 / 计划文档存于仓库固定目录 `docs/`,非聊天记录或临时文件。

### 落地偏差防控检查表(第 4/5/7 步 · MUST)

> 用于阻断"模块写了但没接线"、"stub 被误判为完成"、"模型升级只做一半"等问题。

- [ ] 本阶段存在且仅存在一条明确主闭环,并已在文档中写明入口、路径、出口
- [ ] 主闭环已接入运行时组合根,不是仅存在于单测或独立模块中
- [ ] PR 已列出:新增模块、已接线模块、未接线模块,以及未接线理由/后续计划
- [ ] PR 已标记每项新增能力属于 `closed` / `experimental` / `stub`
- [ ] 阶段验收结论未把 `stub` / `preview` / `skeleton` 计入已完成能力
- [ ] 核心模型变更已同步检查: schema、adapter、recovery、DLQ replay、audit、集成测试
- [ ] 阶段主闭环具备至少一条系统级 / 集成级测试,而非仅依赖单元测试

---

## 二、迁移红线(基于 RUST_MIGRATION_V5)

本部分每一条均为 **MUST** (除显式 SHOULD/MAY 标注),违反即阻断合并与上线。

### 2.1 架构边界

- **MUST** Core 不依赖 Agent;AI 仅作为可插拔能力。
- **MUST** MCP/REST/CLI 作为 Adapter,不得侵入核心业务模型。(`src/adapters/`)
- **MUST** Conversation 为一等对象;RouteKey 至少包含 channel、conversation_id、peer_id、conversation_type。(`src/core/types.rs`)

### 2.2 信道稳定与顺序性

- **MUST** 同 RouteKey 串行,不同 RouteKey 并行。(`src/core/router.rs`、`src/channels/registry.rs`)
- **MUST** Route Worker 具备 Idle GC,默认 30 分钟空闲回收。(`src/channels/registry.rs`)
- **MUST** Dedup 使用 TTL Cache(例如 moka),禁止每条消息全量清理。(`src/core/dedup.rs`)
- **MUST** 有 sequence 的平台按 sequence 排序;无 sequence 的平台按 `timestamp + reorder_window` 处理。(`src/core/reorder.rs`)
- **MUST** 超窗口迟到消息走幂等处理并记录审计标记。(`src/core/reorder.rs`)

### 2.3 可恢复投递与状态持久化

- **MUST** 落地 Inbox/Outbox/DLQ,具备重试与死信重放能力。(`src/core/storage/{inbox,outbox,dlq}.rs`)
- **MUST** 发送状态机遵循 pending -> sending -> sent,失败进入 retrying,超阈值进入 dead_letter。(`src/core/storage/outbox.rs`)
- **MUST** 核心状态持久化(allowlist、session、conversation_state、inbox、outbox、audit_log)。(`src/core/storage/`)
- **MUST** WeChat `sync_buf` 持久化并支持重启恢复。(`src/channels/wechat/session.rs`)
- **MUST** 崩溃恢复后可继续未完成发送。(`src/core/storage/outbox.rs`)

### 2.4 协议与安全

- **MUST** MCP stdio 零污染:stdout 仅协议输出,业务日志仅 stderr/文件。(`src/adapters/mcp.rs`)
- **MUST** ilink 关键契约具备 contract test(`ret`、`errcode`、`sync_buf`、`X-WECHAT-UIN`、AES-128-ECB/PKCS7)。(`src/channels/wechat/`)
- **MUST** 多账号/多通道路径级隔离(session、sync_buf、allowlist、inbox/outbox、audit)。(`src/channels/`、`src/core/storage/`)
- **MUST** 媒体上传支持流式和分段处理,禁止一次性全量读入大文件。(`src/channels/`)
- **MUST** REST Adapter 暴露时启用最小认证(token 或等效机制),禁止裸口默认开放。(`src/adapters/rest.rs`)

### 2.5 韧性与隔离

- **MUST** 外部平台 API 与 AI API 启用 Circuit Breaker。(`src/core/resilience.rs`)
- **MUST** AI 执行池与发送执行池做 Bulkhead 隔离,防止级联故障。(`src/core/resilience.rs`)
- **SHOULD** Processor Pipeline/Middleware 化,避免核心逻辑过胖。(`src/core/pipeline.rs`)
- **SHOULD** 发送统一为 `send_text_with_recovery` / `send_media_with_recovery`。(`src/core/processor.rs`)
- **SHOULD** RateLimiter 升级为可扩展实现(如 governor 或分片滑窗)。(`src/core/rate_limit.rs`)

### 2.6 审计与可追溯

- **MUST** 关键数据流与发送决策留痕:来源、时间、RouteKey、决策依据、执行结果。(`audit_log` 表)
- **MUST** 自动加白名单等高风险操作写入审计日志。(`audit_log` 表)
- **MUST** 审计日志不可篡改,保留期 >= 5 年。

---

## 三、迁移阶段验收 Gate

### 3.1 阶段 1（稳定内核）

- [ ] RouteKey 包含 conversation 维度。(`src/core/types.rs`)
- [ ] 同 RouteKey 串行,不同 RouteKey 并行。(`src/core/router.rs`、`src/channels/registry.rs`)
- [ ] 30 分钟 idle route 自动回收。(`src/channels/registry.rs`)
- [ ] Dedup 使用 TTL cache(无全表 retain)。(`src/core/dedup.rs`)
- [ ] 乱序场景按策略重排或幂等。(`src/core/reorder.rs`)
- [ ] `sync_buf` 持久化并重启可恢复。(`src/channels/wechat/session.rs`)

### 3.2 阶段 1.5（MCP 前置）

- [ ] MCP 最小接口可用(send/list_peers/login)。(`src/adapters/mcp.rs`)
- [ ] stdout 无业务日志污染。(`src/adapters/mcp.rs`、contract test)

### 3.3 阶段 2（可恢复投递）

- [ ] Outbox 状态机可观测。(`src/core/storage/outbox.rs`)
- [ ] 失败重试与 DLQ 可用。(`src/core/storage/{outbox,dlq}.rs`)
- [ ] 进程崩溃后可恢复未完成发送。(`src/core/storage/outbox.rs`、集成测试)

### 3.4 阶段 3（多信道扩展）

- [ ] 双信道并行运行且隔离。(`src/channels/`、隔离集成测试)
- [ ] 单信道故障不影响其他信道。(`src/channels/registry.rs`)

### 3.5 阶段 4（Pipeline + AI 可插拔）

- [ ] Middleware 可插拔(新增中间件不改内核)。(`src/core/pipeline.rs`)
- [ ] AI backend 可切换并可降级到 echo。(`src/core/pipeline.rs`、AI backend 适配层)

### 3.6 阶段 5（生产强化）

- [ ] 熔断、隔离舱、DLQ 运维闭环完整。(`src/core/resilience.rs`、`src/core/storage/dlq.rs`)
- [ ] 服务化、监控、审计可用于生产。(daemon/health、`src/core/telemetry.rs`、`audit_log` 表)

## 四、受控例外通道

- **MUST** 例外必须事先获得授权(指定审批人/角色)。
- **MUST** 例外操作全程留痕:谁、何时、做了什么、为什么。
- **MUST** 事后 24 小时内复盘,补齐对应流程或校验。
- **MUST NOT** 以"紧急"为由跳过第二部分 P0 红线。

---

## 附:强制力速查

| 分级 | 含义 | 违反后果 |
|------|------|----------|
| **MUST** | 强制 | 阻断,不可合并 / 上线 |
| **SHOULD** | 强烈建议 | 需记录偏离理由 |
| **MAY** | 可选 | 自行裁量 |
