# Phase 1.5 Architecture: MCP Adapter

**版本**: 1.0
**日期**: 2026-06-17
**状态**: Draft

## 1. 目标

在 Phase 1 稳定内核之上，通过 MCP (Model Context Protocol) stdio 传输暴露最小可用接口：
- `send` — 发送消息到指定会话
- `list_peers` — 列出可用联系人/群组
- `login` — 登录指定平台账号

**硬约束**: stdout 仅输出 JSON-RPC 协议，业务日志只走 stderr/文件。

## 2. 架构分层

```
┌──────────────────────────────────────────────────┐
│ MCP Client (Claude Desktop / Cursor / etc.)      │
└──────────────┬───────────────────────────────────┘
               │ stdin (JSON-RPC Request)
               │ stdout (JSON-RPC Response)
               ▼
┌──────────────────────────────────────────────────┐
│ Adapter Layer                                    │
│ ┌──────────────────────────────────────────────┐ │
│ │ MCP Adapter (src/adapters/mcp/)              │ │
│ │ - JsonRpcTransport (stdin/stdout)            │ │
│ │ - ToolDispatcher (send/list_peers/login)     │ │
│ │ - ProtocolHandler (initialize/notifications) │ │
│ └──────────────────────────────────────────────┘ │
│           │ stderr only: tracing logs            │
└───────────┼──────────────────────────────────────┘
            │ calls
┌───────────▼──────────────────────────────────────┐
│ Application Layer (existing)                     │
│ route_message / deduplicate / gc_janitor         │
└───────────┬──────────────────────────────────────┘
            │
┌───────────▼──────────────────────────────────────┐
│ Domain + Adapters (existing)                     │
│ ConversationQueue / DedupCache / SyncBufStore    │
└──────────────────────────────────────────────────┘
```

**依赖规则**:
- `adapters/mcp/` 只依赖 `application/` 和 `domain/ports/`
- `adapters/mcp/` 绝不依赖其他 adapter 实现
- Core 不知道 MCP 的存在

## 3. MCP 协议实现

### 3.1 JSON-RPC 2.0 消息模型

```rust
// adapters/mcp/protocol.rs

#[derive(Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    Request(JsonRpcRequest),
    Notification(JsonRpcNotification),
}

#[derive(Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,             // "2.0"
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}
```

### 3.2 MCP 生命周期

```
Client                          Server
  │                                │
  │── initialize (request) ───────→│  握手: 协商协议版本和能力
  │←── initialize (response) ─────│
  │                                │
  │── notifications/initialized ──→│  客户端就绪
  │                                │
  │── tools/list (request) ───────→│  发现可用工具
  │←── tools/list (response) ─────│
  │                                │
  │── tools/call: send ───────────→│  调用工具
  │←── tools/call: send ──────────│
  │                                │
  │── tools/call: list_peers ─────→│
  │←── tools/call: list_peers ────│
  │                                │
  │── tools/call: login ──────────→│
  │←── tools/call: login ─────────│
```

### 3.3 工具定义

```rust
// adapters/mcp/tools.rs

pub struct SendArgs {
    pub channel: String,       // "wechat" | "dingtalk" | "feishu"
    pub conversation_id: String,
    pub peer_id: String,
    pub content: String,       // text content
}

pub struct ListPeersArgs {
    pub channel: String,
}

pub struct LoginArgs {
    pub channel: String,
    pub account: String,
}
```

## 4. Stdout 零污染设计

**问题**: 业务日志和调试输出绝对不能进入 stdout，否则 MCP 客户端收到非法 JSON 会导致协议解析失败。

**设计**:

```
┌────────────┐     stdout     ┌──────────────────┐
│ MCP Core   │───────────────→│ JsonRpcTransport │
│ (handlers) │  只写 protocol │ (tokio::io::stdout)
└─────┬──────┘                └──────────────────┘
      │
      │ tracing::info!/warn!/error!
      ▼
┌────────────┐     stderr     ┌──────────────────┐
│ tracing    │───────────────→│ JSON-formatted    │
│ subscriber │  所有业务日志   │ log output        │
└────────────┘                └──────────────────┘
```

**强制执行**:
1. `JsonRpcTransport::write()` 是唯一写入 stdout 的代码路径
2. 所有 handler 通过 `tracing` 宏记录日志（已在 Phase 1 配置为 stderr）
3. `println!` / `eprintln!` 全局禁止（由 clippy lint `clippy::print_stdout` 强制执行）
4. panic hook 重定向到 stderr

**验证方式**:
- Contract test: 向 stdin 写入请求，读取 stdout，验证每一行都是合法 JSON
- 集成测试: 验证 stdout 中不含任何 tracing 输出

## 5. 数据流图

```
                    ┌──────────────────────┐
                    │   MCP Client         │
                    │   (Claude Desktop)   │
                    └──────────┬───────────┘
                               │ stdin: JSON-RPC
                               ▼
              ┌────────────────────────────────┐
              │     JsonRpcTransport           │
              │  - stdin reader (line-delimited)│
              │  - stdout writer (Serialize)    │
              │  - stderr → tracing subscriber  │
              └────────────────┬───────────────┘
                               │ parsed JsonRpcMessage
                               ▼
              ┌────────────────────────────────┐
              │     ProtocolHandler            │
              │  - initialize → negotiate caps │
              │  - tools/list → return schema   │
              │  - tools/call → dispatch        │
              │  - notifications → ack/noop     │
              └────────────────┬───────────────┘
                               │ tool name + args
                               ▼
              ┌────────────────────────────────┐
              │     ToolDispatcher             │
              │  ┌──────────────────────────┐  │
              │  │ send    → route_message() │  │
              │  │ list_peers → TODO(Phase3) │  │
              │  │ login   → channel.start() │  │
              │  └──────────────────────────┘  │
              └────────────────┬───────────────┘
                               │ calls
                               ▼
              ┌────────────────────────────────┐
              │  Application + Domain (Phase1) │
              │  route_message / ConversationQ │
              └────────────────────────────────┘
```

## 6. 模块结构

```
src/adapters/mcp/
├── mod.rs              # pub mod declarations
├── protocol.rs         # JsonRpcMessage, Request, Response, Error
├── transport.rs        # JsonRpcTransport (stdin/stdout line reader/writer)
├── handler.rs          # ProtocolHandler (initialize, tools/list, notifications)
├── tools.rs            # Tool definitions + ToolDispatcher (send/list_peers/login)
└── server.rs           # McpServer: wires transport + handler + dispatcher
```

## 7. 失败模式分析

| 组件 | 失败模式 | 处理策略 |
|------|---------|---------|
| stdin read | EOF (客户端断开) | Graceful shutdown |
| stdin read | 非法 JSON | 返回 JSON-RPC Parse Error (-32700) |
| tools/call | 未知 tool | 返回 Method Not Found (-32601) |
| tools/call | 参数解析失败 | 返回 Invalid Params (-32602) |
| tools/call | Core 层错误 | 返回 Internal Error (-32603), 错误详情仅 stderr |
| stdout write | Broken pipe | 进程退出 |
| initialize | 客户端版本不兼容 | 返回 Unsupported Version |
| send | 发送失败 | 返回错误 + audit log (stderr) |
| 业务日志 | 误入 stdout | clippy lint 阻断 + contract test 断言 |

## 8. 安全性

- **MUST** MCP 不暴露任何内部状态或配置
- **MUST** 错误响应仅包含错误码和消息，不含堆栈/内部路径
- **MUST** `send` 工具验证目标 channel 在白名单中
- **SHOULD** 敏感参数（密钥等）不出现在任何日志中

## 9. 回滚方案

- MCP Adapter 是独立的 `adapters/mcp/` 模块，删除此目录即可完全移除
- Core 层零改动，MCP 移除不影响其他功能
- 可通过 `main.rs` feature flag 禁用 MCP 编译

## 10. Phase 1.5 验收清单

- [ ] MCP `send` 工具可用 (端到端)
- [ ] MCP `list_peers` 工具可用
- [ ] MCP `login` 工具可用
- [ ] stdout 每行都是合法 JSON (contract test)
- [ ] stdout 不含业务日志 (集成测试断言)
- [ ] clippy lint `clippy::print_stdout` 开启
- [ ] panic hook 重定向到 stderr
- [ ] 单元测试覆盖率 ≥ 80%
