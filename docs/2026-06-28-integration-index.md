# magiclaw 文档整合索引

> **最后更新**: 2026-06-28
> **范围**: `docs/` (33 篇) + `docs/superpowers/` (1 篇 + 2 篇 plan) 共 36 篇文档
> **目的**: 按功能分类索引，便于快速定位
>
> ⚠️ 注：`docs/old` 目录不存在；用户提到的"old"文档已合并在 `docs/` 根目录与 `docs/superpowers/` 下。

---

## 0. 总览：文档地图

```
docs/
├── 项目根文档（CLAUDE.md / AGENTS.md / README.md）         — 不在本索引
├── 01-architecture/       架构与迁移蓝图
├── 02-plans/              阶段与任务计划
├── 03-phase-progress/     阶段完成度报告 / PR 描述
├── 04-channel-designs/    各信道（WeChat/Feishu/Dingtalk）设计
├── 05-feature-designs/    特性设计（AI 后端 / 鉴权 / 推送 / 媒体 / 偏好）
├── 06-reviews/            评审与挑战记录
├── 07-superpowers/        superpowers 子计划与 checklist
└── 整合文档（本目录）
    ├── 2026-06-28-integration-index.md        ← 本文件
    ├── 2026-06-28-development-status.md       开发完成度评估
    ├── 2026-06-28-optimization-backlog.md     优化机会清单
    └── 2026-06-28-bugs.md                     遗留 bug 清单
```

---

## 1. 项目根级文档（位于仓库根）

| 文件 | 作用 | 关键内容 |
|------|------|----------|
| `README.md` | 用户视角项目介绍 | 架构图、目录结构、运行模式、AI 后端、可观测性、测试 |
| `CLAUDE.md` | Claude Code 上下文 | 架构分层、关键设计规则、迁移阶段、强制红线 |
| `AGENTS.md` | 7 步开发流程 | 流程步骤 DoD、根因回退、旧模块接入检查表、阶段 Gate |

---

## 2. 架构与迁移蓝图（01-architecture）

迁移 V5 之后的整体方案文档与各阶段架构设计。

| 文件 | 作用 | 关联红线 |
|------|------|----------|
| `RUST_MIGRATION_V5.md` | 迁移总方案 v5，P0/P1/P2 风险识别 + 分阶段计划 | 全部 |
| `phase1-architecture.md` | 阶段 1：稳定内核（RouteKey / Idle GC / Dedup TTL / Reorder / sync_buf） | 2.1, 2.2, 2.3, 2.6 |
| `phase1-plan.md` | 阶段 1 任务拆分（M1-M4 里程碑，工时估算） | — |
| `phase1.5-architecture.md` | 阶段 1.5：MCP Adapter（stdout 零污染） | 2.4 |
| `phase2-architecture.md` | 阶段 2：可恢复投递（Inbox/Outbox/DLQ 状态机） | 2.3 |
| `phase3-architecture.md` | 阶段 3：多信道扩展（Dingtalk/Feishu 骨架） | 2.4 |
| `phase4-architecture.md` | 阶段 4：Pipeline + AI 可插拔（中间件链） | 2.5 |

---

## 3. 阶段与任务计划（02-plans）

待办拆解与执行路径。

| 文件 | 作用 |
|------|------|
| `implementation-gap-solution.md` | 落地偏差解决方案（5 大根因 + Formal Review Checklist 7.1-7.8） |
| `implementation-gap-task-breakdown.md` | 当前未闭环项任务拆解（P0-1 ~ P0-4 + P1/P2） |

---

## 4. 阶段完成度报告与 PR（03-phase-progress）

| 文件 | 状态 | 范围 |
|------|------|------|
| `PHASE_A_COMPLETION_REPORT.md` | ✅ READY FOR MERGE | Feishu Tasks 2-5（校验/Webhook/错误语义/隔离） |
| `phase-a-checklist.md` | ✅ 全部 closed | Phase A 闭环门限验收清单 |
| `PR_DESCRIPTION.md` | ✅ All stages verified | WeChat QR 登录修复 + daemon bootstrap |
| `PR_PHASE_A_TASKS_2_5.md` | ✅ Ready to merge | Phase A 4 任务的 PR 描述 |
| `p0-fixes-implementation-report.md` | ✅ 完成（commit 5ea6e95） | WeChat Token 多用户缓存 + SessionExpired 检测 |

---

## 5. 信道设计（04-channel-designs）

| 文件 | 信道 | 状态 |
|------|------|------|
| `wechat-context-token-robustness-design.md` | WeChat | 设计评审（5 方案 A-E） |
| `wechat-context-token-review.md` | WeChat | Token 评审 |
| `wechat-context-token-final-review.md` | WeChat | 最终评审（决定 HashMap 方案） |
| `wechat-send-stability-design.md` | WeChat | Send 主链路稳定（Outbox 统一 + Token 持久化） |
| `wechat-send-stability-plan.md` | WeChat | 6 任务实施计划（Task 1-6） |
| `wechat-send-stability-log.md` | WeChat | Bug 跟踪表（WSS-001 ~ WSS-006） |
| `2026-06-23-feishu-push-architecture.md` | Feishu | webhook-first 架构（Phase A/B/C 路线） |
| `2026-06-23-feishu-push-plan.md` | Feishu | 8 任务实施计划（Task 1-8） |

---

## 6. 特性设计（05-feature-designs）

| 文件 | 特性 | 能力分级 | 状态 |
|------|------|---------|------|
| `mcp-deployment.md` | MCP stdio 部署 | `closed`（传输/framing/zero-pollution） | 已落地 |
| `claude-code-backend-design.md` | Claude Code AI 后端 | `closed` | 已接入主链路 |
| `claude-code-backend-challenge.md` | Claude Code 四角挑战 | 2 轮收敛 | 已落地 |
| `claude-code-backend-plan.md` | Claude Code 6 任务计划 | 全部完成 | 已落地 |
| `2026-06-19-project-binding-and-multi-push-design.md` | 项目绑定 + 多人推送 | `planned`（Phase A schema + CLI 设计落地） | 设计阶段 |
| `2026-06-19-user-agent-preferences-design.md` | 按用户切换 AI agent（cc/cx/oc/h 别名 + 持久化） | `closed` | 已落地（`user_agent_preferences_closed_loop` 测试通过） |
| `2026-06-20-dynamic-api-auth-design.md` + `superpowers/plans/2026-06-20-dynamic-api-auth.md` | 多项目动态 API 鉴权（`auth issue/list/revoke` + SQLite `api_clients` 表） | `closed`（实现）/ 🟡 **API 鉴权被注释掉**（B6） | 实现落地但被临时注释 |
| `claude-code-backend-challenge.md` | ClaudeCode 四角挑战记录（Round 1+2 收敛） | 2 轮收敛 | 已闭环 |
| `wechat-context-token-review.md` + `wechat-context-token-final-review.md` | WeChat Token 方案三方对标 + 修复清单 | HashMap 方案已实施（P0 修复） | 已闭环 |
| `media-streaming-upload-design.md` | 媒体流式上传 | `closed`（port 层）/ `experimental`（真实 ilink adapter） | 部分落地 |
| `media-streaming-upload-plan.md` | 媒体上传计划 | 配套 | — |

---

## 7. 评审与挑战记录（06-reviews）

| 文件 | 范围 |
|------|------|
| `implementation-gap-solution.md` §7 | Formal Review Checklist（7.1 组合根 / 7.2 Phase 1 / 7.3 Phase 1.5 / 7.4 Phase 2 / 7.5 ilink 契约 / 7.6 可观测性 / 7.7 Stub / 7.8 通过标准） |
| `wechat-context-token-final-review.md` | Token 设计三方对标（Rust/Node.js/Go/Python SDK） |
| `claude-code-backend-challenge.md` | 四角挑战 2 轮收敛 |
| `media-streaming-upload-design.md` §9 | 四角挑战 B1-B6 |

---

## 8. superpowers 子计划（07-superpowers）

| 文件 | 作用 |
|------|------|
| `superpowers/feishu-push-pr-checklist.md` | Feishu 推送 PR 检查清单（Phase A closed ✅ / Phase B open / Phase C partial） |
| `superpowers/plans/2026-06-20-dynamic-api-auth.md` | 动态 API 鉴权实施计划（4 任务：registry 表 + bearer 查找 + CLI + 闭环测试） |
| `superpowers/plans/code-analysis-modification-plan.md` | 代码分析与修改方案 v3（**当前最重要的待办来源**）：P0×4 + P1×8 + P2×6 + P3×4 |

> `code-analysis-modification-plan.md` v3（2026-06-25）基于全项目 88 个 Rust 文件 + 18 个测试文件的扫描，是 2026-06-28-bugs.md / 2026-06-28-optimization-backlog.md 的源头。

---

## 9. 文档时间线

```
2026-06-17  RUST_MIGRATION_V5 + phase1-architecture/plan + phase1.5/2/3/4-architecture
            + implementation-gap-{solution,task-breakdown} (审计)
2026-06-19  claude-code-backend-{design,challenge,plan} + project-binding 设计
            + user-agent-preferences 设计
2026-06-20  dynamic-api-auth 设计
2026-06-21  README + CLAUDE 重写
2026-06-23  feishu-push architecture/plan + wechat-send-stability 系列
            + wechat-context-token 系列 + phase-a-checklist
2026-06-25  code-analysis-modification-plan v3（最重要的修复清单）
2026-06-28  本次整合（INTEGRATION_INDEX + DEVELOPMENT_STATUS + OPTIMIZATION_BACKLOG + BUGS）
```

---

## 10. 阅读路径建议

| 角色 | 推荐阅读顺序 |
|------|--------------|
| 新人入门 | `README.md` → `CLAUDE.md` → `RUST_MIGRATION_V5.md` → `phase1-architecture.md` |
| 评审 PR | `phase-a-checklist.md` / `PR_*.md` → `superpowers/feishu-push-pr-checklist.md` |
| 修复 Bug | `superpowers/plans/code-analysis-modification-plan.md` → `2026-06-28-bugs.md` → `2026-06-28-optimization-backlog.md` |
| 加新特性 | `AGENTS.md` 7 步流程 → 对应 `*-design.md` → `*-plan.md` |
| 查找红线 | `CLAUDE.md` 强制红线表 + `AGENTS.md` 第二部分 + `implementation-gap-solution.md` §7 |

---

## 11. 与整合文档的关系

| 整合文档 | 内容来源 |
|----------|----------|
| `2026-06-28-development-status.md` | 全部 `*-plan.md` / `*-completion-report.md` / `superpowers/plans/code-analysis-modification-plan.md` §九 |
| `2026-06-28-optimization-backlog.md` | `superpowers/plans/code-analysis-modification-plan.md` §二-§八 + `implementation-gap-solution.md` 根因 + §九.5 故意保留 |
| `2026-06-28-bugs.md` | `wechat-send-stability-log.md` WSS-001~006 + `wechat-context-token-final-review.md` §错误 1-4 + `code-analysis-modification-plan.md` P0 项 |

## 12. ClaudeCodeBackend 四角挑战关键结论

来源 `claude-code-backend-challenge.md`，6 项 Blocking 异议已闭环：

| 异议 | 来源 | 处置 |
|------|------|------|
| A1 / C1 / C2 触发门控 + 成本 + 隐私 | AI 产品经理 + 用户 | 默认 `backend=echo`，启用为 opt-in；Permission 白名单门控；建议接入 RateLimit |
| A2 延迟/串行语义 | AI 产品经理 | 文档明示 same-RouteKey 串行下"同会话连发会顺序排队、每条数秒" |
| B1 子进程不留僵尸 | 网络研发 | `Command.kill_on_drop(true)` + 超时显式 `child.start_kill()` |
| B2 stdout/stderr 分离 + 上限 | 网络研发 | 分通道读取；`.result` 仅取 stdout JSON；stderr 仅日志 |
| D1 审计留痕（红线 2.6） | DDD 架构 | AI 调用结果（成功/失败/降级）写 `audit_log` |

## 13. Dynamic API Auth 实施计划

来源 `superpowers/plans/2026-06-20-dynamic-api-auth.md`，4 任务 TDD 模式：

| 任务 | 内容 | 状态 |
|------|------|------|
| Task 1 | `api_clients` 表 + 索引 | ✅ 完成 |
| Task 2 | `ApiClientRegistry` + bearer 中间件 + 401/403 分流 | ✅ 完成但**被注释掉**（B6） |
| Task 3 | `auth issue/list/revoke` CLI 子命令 | ✅ 完成 |
| Task 4 | issue/expire/revoke/scopes 闭环测试 | ✅ 完成但 `daemon_api_auth_closed_loop` FAIL |

## 14. WeChat Context Token 修复清单（基于官方 SDK 对标）

来源 `wechat-context-token-final-review.md`，对标官方 Rust/Node.js/Go/Python 实现后识别的修正：

| # | 修正 | P0/P1 | 状态 |
|---|------|-------|------|
| 1 | Token 缓存 → `HashMap<user_id, token>` | P0 | ✅ 已修复（commit 5ea6e95） |
| 2 | Session Reset 完整流程（credentials + cursor + tokens + tickets） | P0 | 🟡 B25 部分实现（仅清 tokens，未清 credentials/cursor） |
| 3 | Session Expired (-14) 识别 | P0 | ✅ 已实现 |
| 4 | Token 持久化到 SQLite | P1 | ✅ 已实现（`sqlite_context_tokens.rs`） |
| 5 | Typing Service（getConfig + sendTyping） | P1 | ⛔ **未实现**（新增 B24） |
| 6 | User ID 抽取逻辑（按 message_type 选 from/to） | P1 | ✅ 已实现 |
| 7 | 系统级测试（模拟 -14） | P0 | ⏳ 部分（README 测试通过，但完整恢复路径未验证） |