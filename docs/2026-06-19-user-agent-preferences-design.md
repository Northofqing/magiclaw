# 按用户切换 AI Agent 设计文档

**日期**：2026-06-19  
**范围**：Phase A+（微信用户可独立选择 claude_code / codex / openclaw / hermes，动态切换，支持别名简写）

---

## 1. 需求总结

- 支持的 agent：`claude_code`、`codex`、`openclaw`、`hermes`
- 切换粒度：按微信用户（`peer_id`）维度
- 默认行为：用户未选过 agent 时，不进 AI，先提示他选择
- 命令形式：短别名（`cc`、`cx`、`oc`、`h`）和标准形式都支持，可配置
- 识别规则：
  - 单独发命令就切换（如 `cc` → 切到 claude_code，不生成回复）
  - 命令 + 内容则同时切换 + 用新 agent 处理（如 `cc 帮我总结一下` → 先切到 claude_code，再用它回复）
- 持久化：仅对当前微信账号生效，重启保留选择

---

## 2. 数据模型

### 2.1 新增表：`user_agent_preferences`

```sql
CREATE TABLE IF NOT EXISTS user_agent_preferences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel TEXT NOT NULL,
    account_scope TEXT NOT NULL,
    peer_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (channel, account_scope, peer_id)
);

CREATE INDEX IF NOT EXISTS idx_user_agent_preferences 
    ON user_agent_preferences (channel, account_scope, peer_id);
```

**字段说明**：
- `channel`：wechat、dingtalk 等（目前只用 wechat）
- `account_scope`：当前微信账号标识，满足"仅对当前微信账号生效"
- `peer_id`：微信用户 ID
- `agent_name`：当前选择的 agent（claude_code、codex、openclaw、hermes）
- `updated_at`：最后更新时间

### 2.2 配置：agent 别名表

在 `AppConfig` 中新增：

```rust
pub struct AgentConfig {
    /// 可用的 agent 及其别名
    /// 默认：
    /// - claude_code: ["cc", "claude", "claude code"]
    /// - codex: ["cx", "codex"]
    /// - openclaw: ["oc", "openclaw"]
    /// - hermes: ["h", "hermes"]
    pub aliases: HashMap<String, Vec<String>>,
    
    /// 是否启用按用户偏好切换（Phase A+ 特性）
    pub enable_user_preferences: bool,
}
```

---

## 3. 运行时流程

### 3.1 入站消息处理流程（修改）

当入站文本消息到达时，在进入 `AiMiddleware` 前新增"agent 命令解析层"：

```
入站文本消息
    ↓
AgentCommandParser::parse()
    ↓
┌─ 是 agent 命令吗？
│  └─ 是：处理切换/查询，立即返回，不进 AI
│  └─ 否：继续
│
└─ 查用户偏好
   ├─ 有偏好 → 记下用户的 agent 选择，继续流程
   └─ 无偏好 → 生成"请先选择 agent"提示，返回，不进 AI
        ↓
   入站消息进 Pipeline（此时 backend 用用户选择而非全局 MAGICLAW_AI_BACKEND）
```

### 3.2 命令识别与执行

#### 3.2.1 命令格式

**单独切换**（只发命令，不发内容）：

```
cc
/cc
claude
claude code
```

响应：`已切换到 claude_code`

**命令 + 内容**（同时切换和处理）：

```
cc 帮我总结这个
/cx 这段代码有 bug 吗
openclaw 怎么部署
```

响应：
1. 先切换用户偏好到该 agent
2. 把"帮我总结这个"作为实际消息，用新 agent 生成回复

#### 3.2.2 查询当前 agent

```
当前 agent
/agent
```

响应：
- 已选：`当前使用 claude_code`
- 未选：`还未选择 agent，请发送 cc/cx/oc/h 或完整名称进行切换`

#### 3.2.3 别名对应表（默认配置）

| Agent | 短别名 | 完整形式 |
|-------|--------|----------|
| claude_code | `cc` / `/cc` | `claude` / `/claude` / `claude code` / `/claude code` |
| codex | `cx` / `/cx` | `codex` / `/codex` |
| openclaw | `oc` / `/oc` | `openclaw` / `/openclaw` |
| hermes | `h` / `/h` | `hermes` / `/hermes` |

#### 3.2.4 配置覆盖

用户可在 `magiclaw.config.json` 中改别名：

```json
{
  "agent": {
    "enable_user_preferences": true,
    "aliases": {
      "claude_code": ["cc", "claude", "claude code"],
      "codex": ["cx", "codex"],
      "openclaw": ["oc", "openclaw"],
      "hermes": ["h", "hermes"]
    }
  }
}
```

### 3.3 错误处理

| 场景 | 行为 |
|------|------|
| 用户发"xx"，不是已知 agent | 返回"不认识这个 agent，支持: cc/cx/oc/h 或完整名称" |
| 选中的 agent 对应的 CLI 不可用 | 保持现有降级逻辑，返回 echo 回复 + 审计 warn |
| 用户连续发两个命令 | 第二个覆盖第一个 |

---

## 4. 新增模块

### 4.1 `application/agent_preferences.rs`

```rust
pub struct UserAgentPreferences {
    pub channel: String,
    pub account_scope: String,
    pub peer_id: String,
    pub agent_name: String,
}

pub fn get_user_agent(db: &DbPool, channel: &str, account_scope: &str, peer_id: &str) 
    -> Result<Option<String>, String>;

pub fn set_user_agent(db: &DbPool, channel: &str, account_scope: &str, peer_id: &str, agent_name: &str) 
    -> Result<(), String>;
```

### 4.2 `core/pipeline/agent_command.rs`

命令解析和切换逻辑：

```rust
pub struct AgentCommandParser {
    pub aliases: HashMap<String, Vec<String>>,
}

pub enum AgentCommand {
    Switch(String),              // 切换到某 agent
    Query,                       // 查询当前 agent
    SwitchAndProcess(String, String), // 切换 + 处理内容
    NotCommand,                  // 不是命令
}

impl AgentCommandParser {
    pub fn parse(&self, text: &str) -> AgentCommand;
}
```

### 4.3 修改 `core/pipeline/mod.rs`

在主 Pipeline 前面加一层"agent 命令拦截"。

---

## 5. 运行时配置

### 5.1 环境变量（可选）

如果需要运行时控制，补充：

```bash
MAGICLAW_USER_AGENT_PREFS_ENABLED=true  # 启用用户偏好（默认 true）
```

### 5.2 配置文件示例

```json
{
  "agent": {
    "enable_user_preferences": true,
    "aliases": {
      "claude_code": ["cc", "claude", "claude code"],
      "codex": ["cx", "codex"],
      "openclaw": ["oc", "openclaw"],
      "hermes": ["h", "hermes"]
    }
  }
}
```

---

## 6. 与现有架构的关系

| 组件 | 变更 |
|------|------|
| `MAGICLAW_AI_BACKEND` | 退化成系统保底后端；优先级变成：用户选择 > 全局默认 > echo |
| `AiMiddleware` | 不变；从 Pipeline 上下文读取"当前用户的 agent 选择" |
| `Permission` | 保持占位实现；后续可在这层加"某 agent 只特定人用"的规则 |
| `RateLimit` | 不变 |
| 其他 middleware | 不变 |

---

## 7. 验收标准

### 7.1 功能验收

- [ ] 用户 A 发 `cc` → 切到 claude_code，后续普通文本用 claude_code 回复
- [ ] 用户 A 发 `cx` → 切到 codex，后续普通文本用 codex 回复
- [ ] 用户 B 同时发 `h` → 用户 B 切到 hermes，不影响用户 A
- [ ] 用户发 `cc 帮我总结` → 同时完成切换和处理，返回总结结果
- [ ] 用户发"当前 agent"或"/agent" → 返回当前使用的 agent
- [ ] 未选择的新用户发普通文本 → 返回"请先选择 agent"提示
- [ ] daemon 重启后，之前的选择仍然生效

### 7.2 配置覆盖验收

- [ ] 修改 `magiclaw.config.json` 中的别名后，新别名立即生效
- [ ] 不在 aliases 里的 agent 无法被切换

### 7.3 集成验收

- [ ] 跨平台：同一用户在 wechat 上的偏好不影响 dingtalk（后续使用）
- [ ] 审计：每次切换都记录到 audit_log
- [ ] 降级：当所选 agent 不可用时，自动回退到 echo，不卡主链路

---

## 8. Phase A+ 闭环定义

本功能属"closed"等级，满足：

- **已接线**：新增 AgentCommandParser，修改 pipeline 入口
- **闭环测试**：上述验收标准全部自动化覆盖
- **满足阶段范围**：支持用户动态切换 agent，不改动 outbox / recovery 等红线

---

## 9. 依赖与后续

### 9.1 本 phase 依赖

- 现有 `application/audit.rs` 审计能力（记录切换动作）
- 现有 RouteKey + Conversation 状态管理

### 9.2 后续可扩展

- **Permission 增强**：某 agent 只特定人/项目用
- **agent 预热**：用户切换时提前加载对应 CLI
- **统计**：每个用户偏好 agent 的使用频率
- **跨平台迁移**：用户在 WeChat 的选择可同步到 Dingtalk

---

## 10. 文件变更清单

| 文件 | 变更类型 | 说明 |
|------|----------|------|
| `src/infrastructure/db.rs` | 修改 | 新增 user_agent_preferences 表 |
| `src/infrastructure/config.rs` | 修改 | 新增 AgentConfig |
| `src/application/agent_preferences.rs` | 新增 | 偏好查询/设置 |
| `src/core/pipeline/agent_command.rs` | 新增 | 命令解析逻辑 |
| `src/core/pipeline/mod.rs` | 修改 | 在 Pipeline 前加命令拦截层 |
| `src/application/mod.rs` | 修改 | 注册 agent_preferences 模块 |
| `src/main.rs` | 修改 | 加载 agent aliases 配置 |
| `tests/user_agent_preferences_closed_loop.rs` | 新增 | 用户切换闭环集成测试 |

---

## 问题检查清单

- [ ] 别名可配置吗？✓ 通过 AppConfig
- [ ] 单独发命令不回复文本吗？✓ AgentCommand::NotCommand 处理
- [ ] 命令 + 内容能同时切换和处理吗？✓ AgentCommand::SwitchAndProcess 处理
- [ ] 未选择用户会被提示吗？✓ 返回选择提示，不进 AI
- [ ] 重启后选择保留吗？✓ 持久化到表
- [ ] 不同用户互不影响吗？✓ primary key 包含 peer_id

---

这份设计可以开始实现吗？如果有改动需求，先列出来。
