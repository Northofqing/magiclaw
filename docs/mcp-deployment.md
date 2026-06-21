# MCP 部署与联调文档

> 适用版本:magiclaw(信道中心架构系统)阶段 1.5 MCP Adapter
> 目标:在本机把 magiclaw 作为 MCP Server 跑起来,供 MCP Client(Claude Desktop / 自研宿主 / 手工 JSON-RPC)联调,先验证 `initialize → tools/list → tools/call` 闭环。

---

## 1. 能力概览(当前实现)

| 项 | 实现状态 | 说明 |
|----|----------|------|
| 传输 | `closed` | stdio,JSON-RPC 2.0 |
| 协议版本 | `closed` | `2024-11-05` |
| 帧格式 | `closed` | 同时支持 `Content-Length` 头帧 **和** 单行 JSON(换行分隔);输出**始终**为 `Content-Length` 帧 |
| stdout 零污染 | `closed` | 业务日志只走 **stderr**(JSON),stdout 仅协议输出 |
| `initialize` | `closed` | 返回 capabilities + serverInfo |
| `tools/list` | `closed` | 返回 `send` / `list_peers` / `login` |
| `tools/call: send` | `closed` | 文本消息入 Outbox(返回 `pending` + `message_id`),由后台 worker 投递 |
| `tools/call: list_peers` | `experimental` | 仅 `wechat`,从本地通道目录读取 peers |
| `tools/call: login` | `experimental` | 检查 WeChat 账号配置与登录就绪度 |

> 说明:MCP 模式下会同时启动后台 runtime(`start_background`),因此 `send` 入 Outbox 后会被投递 worker 处理;崩溃恢复同样生效。

---

## 2. 前置条件

- Rust 工具链(stable,2021 edition)。
- macOS / Linux。
- 一个 MCP Client。最简单的联调方式是手工用 `printf` 发 JSON-RPC(见 §6)。

---

## 3. 构建

```bash
cd /path/to/magiclaw
cargo build --release
# 产物:target/release/magiclaw
```

冒烟测试(可选):

```bash
cargo test
```

---

## 4. 配置

magiclaw 启动时从 **WeChat 通道数据目录**加载账号配置(不是必须;缺失时以 skeleton 通道运行,`send` 仍可入 Outbox)。

数据目录解析顺序:

1. 环境变量 `WECHAT_CHANNEL_DIR`
2. 否则 `~/.claude/channels/wechat`
3. 否则 `./.claude/channels/wechat`

目录下可放置:

- `account.json`(WeChat 账号),示例:

  ```json
  {
    "token": "<channel-token>",
    "baseUrl": "https://<ilink-host>",
    "accountId": "<account-id>",
    "userId": "<optional-user-id>"
  }
  ```

- `context_tokens.json`(可选,`userId -> contextToken` 映射):

  ```json
  { "<user-id>": "<context-token>" }
  ```

其他相关环境变量:

| 变量 | 默认 | 作用 |
|------|------|------|
| `WECHAT_CHANNEL_DIR` | `~/.claude/channels/wechat` | WeChat 数据目录 |
| `RUST_LOG` | `info` | 日志级别(走 stderr)。联调建议 `info` 或 `debug` |
| `MAGICLAW_API_ADDR` | `127.0.0.1:18011` | 仅 daemon 模式的 HTTP API;MCP 模式不使用 |

> SQLite 落盘默认 `data/magiclaw.db`。程序会在启动时自动创建 `data/` 目录；在固定工作目录启动以保证 Outbox / 崩溃恢复状态连续。

---

## 5. 启动 MCP Server

```bash
# 方式 A:flag
./target/release/magiclaw --mcp

# 方式 B:子命令
./target/release/magiclaw mcp
```

注意:

- `--mcp` **不接受额外参数**,否则报错退出。
- 进程占用 **stdin/stdout** 作为协议通道。**不要**往 stdout 打印任何非协议内容。
- 日志全部走 **stderr**;联调时可 `2>magiclaw.log` 重定向以免干扰观察。
- stdin 到 EOF(`Ctrl-D` / 客户端关闭)时 Server 优雅退出。

---

## 6. 手工联调(无需 MCP Client)

传输层兼容**单行 JSON**,因此可直接用 shell 验证闭环。把日志重定向到文件,只看 stdout 协议输出:

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"send","arguments":{"channel":"wechat","conversation_id":"conv_001","peer_id":"user_a","conversation_type":"direct","content":"hello from mcp"}}}' \
  | ./target/release/magiclaw --mcp 2>magiclaw.log
```

预期 stdout(每条均为 `Content-Length` 帧 + JSON body):

1. `initialize` → `result.protocolVersion = "2024-11-05"`,含 `capabilities.tools` 与 `serverInfo`。
2. `tools/list` → `result.tools` 数组含 `send` / `list_peers` / `login`。
3. `tools/call send` → `result.content[0].text` 为 JSON 字符串,内含 `"status":"pending"` 与 `"message_id"`。

校验 stdout 零污染:

```bash
# 只应看到 Content-Length 帧与 JSON,绝不应出现日志行
./target/release/magiclaw --mcp 2>/dev/null <<< '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'
```

---

## 7. 接入 Claude Desktop(示例)

在 MCP Client 配置(如 Claude Desktop 的 `claude_desktop_config.json`)中加入 stdio server:

```json
{
  "mcpServers": {
    "magiclaw": {
      "command": "/path/to/magiclaw/target/release/magiclaw",
      "args": ["--mcp"],
      "env": {
        "WECHAT_CHANNEL_DIR": "/Users/you/.claude/channels/wechat",
        "RUST_LOG": "info"
      }
    }
  }
}
```

重启 Client 后应能发现 `send` / `list_peers` / `login` 三个工具。

---

## 8. 工具调用速查

### send(closed)

必填参数:

| 字段 | 类型 | 说明 |
|------|------|------|
| `channel` | string | `wechat` / `dingtalk` / `feishu` |
| `conversation_id` | string | 会话 ID |
| `peer_id` | string | 对端用户 / 群 ID |
| `conversation_type` | enum | `direct` / `group` / `thread` / `bot_session` |
| `content` | string | 文本内容 |

返回:`{ "status": "pending", "message_id": "...", "channel": "...", "conversation_id": "..." }`

### list_peers(experimental)

`{ "channel": "wechat" }` → 从本地通道目录枚举 peers(仅 wechat)。

### login(experimental)

`{ "channel": "wechat", "account": "<id>" }` → 返回账号配置与登录就绪度。

---

## 9. 排障

| 现象 | 排查 |
|------|------|
| 客户端发现不到工具 | 确认 `command`/`args` 绝对路径正确;`2>` 看 stderr 是否有 panic |
| stdout 里混入日志 | 不应发生;若有请反馈(违反零污染红线)。临时用 `2>/dev/null` 隔离 |
| `send` 返回 pending 但未实际送达 | 当前 wechat 真实投递依赖 ilink 配置;检查 `account.json` / `context_tokens.json` 是否齐备,看 stderr 投递日志 |
| `list_peers` / `login` 报 unsupported channel | 仅 `wechat` 受支持(experimental) |
| 进程立即退出 | `--mcp` 不能带额外参数;或 stdin 立即 EOF |

---

## 10. 当前边界(诚实披露)

- `send` 为**异步入 Outbox**语义:返回 `pending` 表示已入队,不代表已送达对端。真实送达取决于 WeChat ilink 配置与后台 worker。
- `list_peers` / `login` 为 `experimental`,仅覆盖 wechat,不计入阶段完成度。
- MCP 媒体发送尚未接入(媒体流式上传为独立 `closed` 增量,但 ilink 媒体契约未定,见 `docs/media-streaming-upload-design.md` §8)。
- 多账号 / 多通道隔离、审计、熔断等红线在 core 已具备,但 MCP 工具面当前只暴露文本 `send`。
- `Permission` 中间件目前仍是占位放行实现,白名单门控尚未真正闭环;因此启用非 `echo` AI 后端后,入站文本都会按现有主链路进入 AI 阶段(仅受 RateLimit 限流)。

---

## 11. AI 后端:启用本机 Claude Code(`claude_code`)

> 设计/挑战/计划见 `docs/claude-code-backend-{design,challenge,plan}.md`。默认关闭(`echo`),需显式启用。

让微信入站消息**自动调用本机 `claude` CLI** 生成回复:

```bash
# 默认 echo;设为 claude_code 即启用本机 claude(只读 plan 模式)
MAGICLAW_AI_BACKEND=claude_code ./target/release/magiclaw
```

也可写入配置文件:

```jsonc
{
  "ai": {
    "backend": "claude_code",
    "claude_code": {
      "binary_path": "claude",
      "timeout_secs": 60,
      "max_output_bytes": 16384,
      "extra_args": ["--permission-mode", "plan"]
    }
  }
}
```

行为与约束:
- 调用形态:`claude -p "<用户消息>" --output-format json --permission-mode plan`。prompt 以**单个 argv 参数**传入,绝不经过 shell(无命令注入)。
- `--permission-mode plan` 为**只读**模式,agent 不改文件 / 不执行命令。
- 任何失败(二进制缺失 / 未登录 / 超时 / 非零退出)都**降级为 echo**,主链路不中断、不 panic;超时会 kill 子进程,不留僵尸。
- 每次 AI 调用写 `audit_log`(红线 2.6)。
- **成本/延迟须知**:每条触发消息会调用一次本机 `claude`(实测约 `$0.14`、`~3.3s`,底层模型由本机 claude 配置决定,本机实测为 deepseek)。当前 `Permission` 仍为占位放行,因此实际效果不是“仅白名单触发”;如需收敛范围,请配合业务侧触发条件并按需接入 RateLimit。
- **数据出境须知**:消息正文会交给本机 `claude` 配置的模型(可能上行云端)。
- 当前实现:非 `echo` 后端启用后,所有入站文本都可能进入 AI 步骤;真正的白名单门控尚未落地。

能力分级:`closed`(已接入运行时主链路 + 闭环/降级/超时/安全单测与集成测试 + 满足红线 2.5/2.6)。

## 12. AI 后端:接入通用 CLI Agent(codex / copilot / hermes / openclaw / 自定义)

> 任何可**无头(headless)调用**的本机 agent 都可作为 AI 后端,无需改代码。codex / copilot 为内置 preset,其余(hermes / openclaw 等)通过 `ai.agents` 配置接入。

### 12.1 选择后端

```bash
# 内置 preset:codex(只读沙箱)、copilot(需公版 copilot CLI)
MAGICLAW_AI_BACKEND=codex ./target/release/magiclaw
```

`MAGICLAW_AI_BACKEND` / 配置 `ai.backend` 的取值与解析顺序:
1. `echo`(默认) / `claude_code` —— 专用后端;
2. 命中 `ai.agents` 中的同名键 —— 使用该自定义 agent(**优先级最高**,可覆盖内置 preset);
3. 命中内置 preset(`codex` / `copilot`);
4. 都不命中 —— 告警并降级为 `echo`。

### 12.2 内置 preset

| backend | 调用形态 | 回复来源 | 前置条件 |
|---------|----------|----------|----------|
| `codex` | `codex exec --skip-git-repo-check --sandbox read-only --color never -o <临时文件> "<prompt>"` | 临时文件(读取后即删) | 本机已装 OpenAI Codex CLI(`codex`) |
| `copilot` | `copilot -p "<prompt>"` | stdout(原文) | 本机已装**公版** GitHub Copilot CLI(`copilot`);VS Code 内置的同名 helper **不可用**,需用公版或在 `ai.agents.copilot` 覆盖 |

> hermes / openclaw 本机未安装,**不提供内置 preset**;按 10.3 自行配置即可接入(诚实标注:未在本机验证)。

### 12.3 配置任意自定义 agent

```jsonc
{
  "ai": {
    "backend": "hermes",
    "rate_limit_min_interval_ms": 3000,
    "agents": {
      "hermes": {
        "binary_path": "hermes",
        "args": ["chat", "{prompt}"],
        "timeout_secs": 120,
        "max_output_bytes": 16384,
        "result_json_pointer": null,
        "read_output_file": false
      },
      "openclaw": {
        "binary_path": "openclaw",
        "args": ["--json", "{prompt}"],
        "result_json_pointer": "/reply"
      }
    }
  }
}
```

字段语义:
- `args`:argv 模板。`{prompt}` 在**单个 argv 参数内**替换用户消息(绝不经过 shell,无命令注入);若模板中无 `{prompt}`,prompt 追加为最后一个参数。`{output_file}` 在 `read_output_file=true` 时替换为临时文件路径。
- `result_json_pointer`:设为 JSON 指针(如 `/result`、`/reply`)时,把输出按 JSON 解析并取该字段;为 `null` 时取原始输出(trim 后);兼容输出携带 `is_error=true` 时报错降级。
- `read_output_file`:为 `true` 时从 `{output_file}` 指向的临时文件读取回复(如 codex `-o`),读完即删;否则读 stdout。
- `timeout_secs` / `max_output_bytes`:硬超时(超时 kill 子进程,不留僵尸)与输出截断上限。

### 12.4 行为、安全与约束(与 claude_code 一致)

- 任何失败(二进制缺失 / 超时 / 非零退出 / 解析失败)都**降级为 echo**,主链路不中断、不 panic。
- 每次 AI 调用写 `audit_log`(红线 2.6),`backend` 记录为所选 agent 名。
- 外层包 `ResilientAiBackend`(熔断 + 隔离舱,红线 2.5)。
- **强制 RateLimit**:只要 `ai.backend` 非 `echo`(即启用了任何计费/本机 agent),Pipeline 会在 Permission 之后插入 `RateLimit` 中间件,按 `rate_limit_min_interval_ms`(默认 `3000ms`)对**每会话**限流——被限流的消息在进入 AI 步骤前 short-circuit,防止 agent 成本失控与回复风暴。
- 当前实现:非 `echo` 后端启用后,所有入站文本都可能进入 AI 步骤;真正的白名单门控尚未落地。
- **成本/数据出境须知**:每条触发消息会调用一次对应本机 agent,消息正文会交给该 agent 配置的模型(可能上行云端)。由于当前没有真正的白名单门控,生产使用前应先补触发范围控制。

能力分级:
- `codex`:`closed`(通用 CLI agent 主链路 + 输出文件/原始 stdout 双取值 + 降级/超时/安全单测 + 系统级集成测试 `tests/cli_agent_backend_closed_loop.rs`)。
- `copilot`:`experimental`(已接线并可配置,但公版 `copilot` CLI 未在本机验证;本机内置 helper 不可用)。
- `hermes` / `openclaw` / 其他自定义:`experimental`(机制已闭环并通过 stub 集成测试,但具体 agent 未在本机验证,需用户提供可用二进制)。


