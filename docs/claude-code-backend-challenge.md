# Step 2:四角挑战记录 — ClaudeCodeBackend

> 关联设计:[claude-code-backend-design.md](claude-code-backend-design.md)
> 退出条件:无新增 Blocking 异议(单方案上限 3 轮)
> 结论:**Round 2 收敛,无新增 Blocking**。所有 Blocking 已闭环并反哺 Step 1 设计文档。

---

## Round 1:四角书面异议

### 角色 A · AI 产品经理
- **A1 [Blocking]** 每条入站微信消息无差别触发一次付费 AI(实测 ~$0.14/次)。无触发策略 = 成本不可控、易被刷量。必须有"何时才调用 AI"的门控。
- **A2 [Blocking]** ~3.3s 延迟 + per-RouteKey 串行 = 同会话连发消息会排队,用户体验差且观感像卡死。需要明确延迟预期与并发上限语义。
- **A3 [Non-blocking]** 后端名为 `claude_code` 但实际路由到 deepseek,文档/日志须如实说明,避免误导。

### 角色 B · 网络通信资深技术研发
- **B1 [Blocking]** 子进程生命周期:超时 kill 必须确保不留僵尸/孤儿进程。`tokio::process::Command` 默认 **不** 在 drop 时杀子进程,需显式 `kill_on_drop(true)` + 超时后 `child.kill()`。
- **B2 [Blocking]** stdout/stderr 必须分离读取并设上限,否则大输出/管道阻塞会挂死 worker。`.result` 从 stdout JSON 取,stderr 仅入本地日志。
- **B3 [Non-blocking]** AI 延迟会吃掉 ilink token 新鲜度窗口(已知 P1 token bug 叠加),可能导致发送时 token 失效。但 Outbox 重试可兜底,且 P1 修复是独立事项,降级为 NB + 备注。
- **B4 [Non-blocking]** 并发上限已由现有 `ResilienceGate` 隔离舱覆盖;跨 RouteKey 并行的总 claude 进程数受其约束,确认即可。

### 角色 C · 用户
- **C1 [Blocking]** 隐私/数据出境:微信消息正文会被送进本机 `claude`,进而上行到模型云端(deepseek)。必须默认关闭、仅授权联系人触发、并对"消息会离开本机交给模型"显式知情。
- **C2 [Non-blocking]** 不希望因别人给 bot 发消息而被动产生费用——与 A1 同源,由触发门控覆盖。

### 角色 D · 系统 DDD 架构
- **D1 [Blocking]** 红线 2.6:AI 是关键发送决策,必须审计留痕(来源、时间、RouteKey、backend、结果/错误)。当前设计未提 audit 接入。
- **D2 [Non-blocking]** "spawn 进程"是 I/O/基础设施关注点,放 `src/core/ai/` 是否破坏 core 纯净?——鉴于 `openai.rs`/`claude.rs`(同样要做 HTTP I/O)已先例置于 `core/ai/`,遵循既有约定,可接受;但需在文档注明这是沿用先例。
- **D3 [Non-blocking]** 后端不应自己承载"触发策略"(那是 pipeline 关注点)。触发门控应由 Permission(白名单)+ RateLimit 中间件承担,后端保持单一职责。

---

## Round 2:收敛与处置

| 异议 | 处置 | 反哺位置 |
|------|------|----------|
| **A1 / C1 / C2** 触发门控 + 成本 + 隐私 | ① 默认 `backend=echo`,启用 `claude_code` 为显式 opt-in;② 触发由现有 **Permission(白名单)** 中间件把关(非白名单 peer 根本不进 AI);③ 文档强烈建议启用时**同时接入 RateLimit 中间件**做频率/成本上限;④ 后端本身不含触发逻辑(满足 D3)。→ 解除 Blocking | 设计 §3.3 / §5 / 新增 §10 触发与成本 |
| **A2** 延迟/串行语义 | 明确:same-RouteKey 串行是既定并发模型(红线 2.2),AI 延迟只影响同一会话顺序处理,跨会话并行;隔离舱限制总并发;文档写明"同会话连发会顺序排队、每条数秒"为预期行为。→ 解除 Blocking | 设计 §4 注记 + §10 |
| **A3** 命名如实 | 文档/日志注明:`claude_code` 指本机 CLI,底层模型由本机 claude 配置决定(实测 deepseek)。→ NB 记录 | 设计 §3.1(已含) |
| **B1** 子进程不留僵尸 | 实现固定 `Command.kill_on_drop(true)`;超时分支显式 `child.start_kill()` 并 await 回收。→ 解除 Blocking | 设计 §4 + §5.3 |
| **B2** stdout/stderr 分离 + 上限 | 分别捕获;`.result` 仅取 stdout JSON;stderr 仅日志;读取设 `max_output_bytes` 上限并截断。→ 解除 Blocking | 设计 §3.1 / §4 |
| **B3** token 窗口叠加 | 备注:AI 延迟消耗 token 窗口,Outbox 重试兜底;P1 token 修复为独立事项,不在本特性范围。→ NB | 设计 §4 + §10 |
| **B4** 并发上限 | 确认由 `ResilienceGate` 隔离舱覆盖,无需新增。→ NB | 设计 §3.3(已含) |
| **D1** 审计留痕 | **新增**:AI 调用结果(成功/失败/降级)写 `audit_log`(source、RouteKey、backend、result/err 摘要),满足红线 2.6。→ 解除 Blocking | 设计 §5.7 + §6 + §8 |
| **D2** core 纯净 | 沿用 `core/ai/` 既有先例,文档注明。→ NB | 设计 §6 |
| **D3** 单一职责 | 后端不含触发策略,触发交 Permission/RateLimit。→ 已采纳 | 设计 §10 |

### Round 2 退出判定
- 所有 Round 1 的 **Blocking(A1/A2/B1/B2/C1/D1)** 均已闭环处置并反哺设计。
- 处置过程**未引入新的 Blocking 异议**。
- ⇒ 满足退出条件,**挑战收敛(2 轮,未触及 3 轮上限)**,可进入 Step 3 任务拆分。

---

## 反哺 Step 1 设计的增量(已同步)
1. 新增 §10「触发策略与成本控制」:默认 echo、显式 opt-in、Permission 白名单门控、建议接入 RateLimit、延迟/排队预期说明。
2. §4 失败模式补充:`kill_on_drop(true)` + 超时显式 kill;stdout/stderr 分离与上限。
3. §5 安全补充:§5.7 AI 调用审计留痕(红线 2.6)。
4. §6 旧模块表补充:`SqliteAuditSink` 复用接入;`core/ai/` 置放沿用先例的说明。
