# WeChat Context Token 方案审核与问题发现

**Date**: 2026-06-23  
**审核内容**: 与官方协议对照

---

## 🚨 发现的漏洞

### 漏洞 1：主动发送（Proactive Send）无解

**官方协议规则**：
```
context_token 只来自入站消息：
{ ret: 0, msgs: [{ from_user_id, context_token, item_list }], get_updates_buf: "..." }

发送必须包含 context_token：
POST /sendmessage { msg: { to_user_id, context_token, item_list: [...] } }
```

**问题**：
- Token 只通过 getupdates 获得（从消息中提取）
- 如果从未收到某用户的消息，就没有 context_token
- 那么无法向该用户发送任何消息（包括 typing indicator）

**当前代码行为**：
```rust
// 如果 context_token 为空字符串，会发送空 token？
send_cfg.context_token = state.context_token.clone();  // ← 可能为 ""
```

**修复**：
- ✅ 应该在 send_message 中检查：如果 token 为空，返回错误（而非发送）
- ✅ 文档化：系统仅支持"回复"模式，不支持"主动推送"

---

### 漏洞 2：Token Per User ID 的理解错误

**官方协议**：
```
Cache per (userId) — 每个用户ID维度缓存最新token

但消息结构是：
{ msgs: [
    { from_user_id: "user1", context_token: "ctx_1", ... },
    { from_user_id: "user2", context_token: "ctx_2", ... }
]}
```

**问题**：
- 一个 getupdates 响应可能包含来自**多个用户**的消息
- 当前代码用 `extract_latest_context_token(..., Some(&from_user_id))` 提取**单个**消息的 token
- 这是对的，但逻辑有改进空间

**当前代码**：
```rust
if let Some(ctx) = extract_latest_context_token(std::slice::from_ref(&msg), Some(&from_user_id)) {
    let mut state = poll_session.lock().await;
    state.context_token = ctx;  // ← 这只适用于 Direct Message
}
```

**问题**：
- SessionState 只有**一个** `context_token` 字段
- 如果系统处理多用户场景（群聊或多个直聊），这个设计不对
- 应该是 `Map<UserId, ContextToken>`

**修复**：
```rust
struct SessionState {
    // context_tokens: HashMap<String, String>,  // userId → token
    context_token: String,  // 临时：仅支持单用户
}
```

**建议**：
- 当前实现假设"一个 WeChat 账户对应一个会话"，这是合理的初始设计
- 但应该在代码注释中明确说明这个限制

---

### 漏洞 3：Session 过期时的"Delete Stored Credentials"未定义

**官方文档的恢复步骤**：
```
1. Clear cached context_tokens  ← 清除 token
2. Clear poll cursor            ← 清除 sync_buf / get_updates_buf
3. Delete stored credentials    ← ？？？
4. Get new QR code
5. Resume polling with fresh state
```

**问题**：
- "Delete stored credentials" 具体指什么？
  - bot_token？（可能不需要删除，用来验证身份）
  - baseurl？（可能不需要删除，用来定位服务）
  - account_id / ilink_bot_id / ilink_user_id？（可能不需要删除）

**官方文档说**：
> "All SDKs handle this automatically — the user just needs to scan a new QR code."

这暗示：
- session 完全失效（token 不再有效）
- 需要重新登录获得新的 bot_token 吗？还是仅清除 context_token？

**当前方案**：
- 只清除 context_token 和 sync_buf
- 保留 bot_token 和 baseurl

**可能是错的**！应该保留什么、删除什么？

---

### 漏洞 4：Typing Indicator 也需要 Context Token

**官方文档**：
```
POST /getconfig { ilink_user_id, context_token }
{ typing_ticket: "base64..." }

POST /sendtyping { ilink_user_id, typing_ticket, status: 1 }
```

**问题**：
- getconfig 也需要 context_token
- 当前代码中**没有实现** getconfig/sendtyping
- 如果实现了，也需要处理 -14 错误

**当前代码**：
- `send_text_via_ilink` 中有 context_token
- 但没有 `getconfig` 和 `sendtyping` 函数

**修复**：
- 添加 `get_config_via_ilink(context_token)` 和 `send_typing_via_ilink(context_token, ticket, status)`
- 这两个函数也应该能返回 -14 错误

---

### 漏洞 5：Context Token 的生命周期和 TTL

**官方文档**：
```
typing_ticket: "valid ~24h"
```

但 context_token 的有效期？**没有说明**。

**推论**：
- Token 可能绑定到 session
- Session 过期时 token 一起失效
- 但没有说 token 是否可能在 session 有效期内过期

**当前设计**：
- 假设 token 一直有效（直到 session 失效）
- 没有 TTL 机制

**风险**：
- 如果系统运行 24 小时后，token 过期但 session 未发送 -14
- 那么后续发送会失败，且无法识别原因

---

### 漏洞 6：Concurrent Access 和原子性

**官方文档**：
```
Each response returns a new get_updates_buf — treat it as an opaque cursor
```

**场景**：
- getupdates 长轮询线程返回 -14（session expired）
- 同时主线程正在调用 send_message，读取 context_token
- Race condition！

**当前代码**：
```rust
let state = session.lock().await;
let mut send_cfg = config.clone();
send_cfg.context_token = state.context_token.clone();
drop(state);  // ← 解锁

let resp = send_text_via_ilink(...).await?;  // ← 同时长轮询线程可能修改 state

if let Some(ctx) = extract_latest_context_token(...) {
    let mut state = poll_session.lock().await;
    state.context_token = ctx;  // ← 覆盖
}
```

**问题**：
- 发送成功后，收到新消息的 token 覆盖了旧的 token
- 这通常是对的（新 token 更新鲜）
- 但如果发送返回 -14，应该**立即**清除 token，防止后续继续用过期 token

**当前代码对 -14 的处理**：
```rust
let resp = send_text_via_ilink(...).await?;  // ← 如果返回 -14，这里是 Err，不会继续
```

这是对的（返回 Err，不会继续）。但上层（应用层）可能没有正确处理 -14。

---

### 漏洞 7：Session Reset 流程缺失

**当检测到 -14 时应该做什么**？

**官方说法**：
> "All SDKs handle this automatically — the user just needs to scan a new QR code."

**这意味着**：
- SDK 应该发出通知，告诉应用：需要重新扫码
- 应用（或用户）扫新二维码
- SDK 重新调用登录流程

**当前代码**：
```rust
Err(ILinkGetUpdatesError::SessionExpired { errmsg }) => {
    // 只有日志，没有实际动作
    tracing::error!("WeChat session expired: {}", errmsg);
    return;  // 停止轮询线程
}
```

**问题**：
- 谁来通知用户需要重新扫码？
- 主线程？应用程序？
- 当前代码没有机制来处理这个通知

**修复思路**：
- 需要一个"会话事件总线"
- 当 session 失效时，发出事件
- 应用程序监听事件，触发重新登录流程
- 或者返回特定错误，让 HTTP API 的调用方知道需要重新登录

---

## 📋 方案修正

### 修正 1：检查 Context Token 非空（P0）

**File**: `src/channels/wechat/channel.rs`

```rust
async fn send_message(&self, to: &str, content: &MessageContent) -> Result<SendReceipt, String> {
    // ...
    WeChatMode::ILink { config, client, session, ... } => {
        let state = session.lock().await;
        
        // ✅ 新增：检查 token 是否为空
        if state.context_token.trim().is_empty() {
            return Err(
                "Cannot send: no context_token yet. \
                 System requires at least one incoming message to establish routing context."
                .into()
            );
        }
        
        let mut send_cfg = config.clone();
        send_cfg.context_token = state.context_token.clone();
        drop(state);
        
        let resp = send_text_via_ilink(client, &send_cfg, to, &body).await?;
        // ...
    }
}
```

---

### 修正 2：澄清 Per-User Token 缓存的限制（P0）

**File**: `src/channels/wechat/channel.rs`

```rust
struct SessionState {
    // 当前实现假设：一个 WeChat 账户对应一个会话（单用户模式）
    // 实际 iLink 协议支持多用户（群聊），但这需要：
    // 1. 将 context_token 从 String 改为 HashMap<UserId, Token>
    // 2. 修改发送时根据 to_user_id 查询对应的 token
    // 
    // TODO (Phase 3): 升级为多用户支持
    context_token: String,
}
```

---

### 修正 3：定义 Session Reset 的具体行为（P0）

**官方协议**："Delete stored credentials" 需要确认范围。

**建议的解释**：
- ✅ 清除 context_token（必须）
- ✅ 清除 sync_buf / get_updates_buf（必须）
- ❓ 清除 bot_token（待确认） — 本方案假设保留
- ❓ 清除 baseurl（待确认） — 本方案假设保留
- ❓ 清除 ilink_bot_id / ilink_user_id（待确认） — 本方案假设保留

**建议**：
- 添加注释说明假设
- 如果官方提供更多信息，再调整

```rust
fn reset_session_on_expiry(state: &mut SessionState) {
    // 官方 -14 处理步骤：
    // 1. Clear cached context_tokens
    state.context_token.clear();
    
    // 2. Clear poll cursor
    // （由上层负责清除 sync_buf）
    
    // 3. Delete stored credentials
    // 官方文档未明确指出具体范围。当前假设：
    // - bot_token 保留（用于重新扫码后的验证）
    // - baseurl 保留（用于重新扫码后的连接）
    // - 仅清除临时会话状态（context_token）
    //
    // 如果需要完全重置，调用方应显式调用 `full_logout()`
}
```

---

### 修正 4：Typing Indicator 函数缺失（P1）

**当前**：只实现了 sendmessage  
**应该有**：getconfig + sendtyping

```rust
// 添加到 src/channels/wechat/ilink.rs

pub async fn get_config_via_ilink(
    client: &reqwest::Client,
    cfg: &ILinkSendConfig,
) -> Result<GetConfigResponse, ILinkGetUpdatesError> {
    // POST /getconfig { ilink_user_id, context_token }
    // 返回 typing_ticket
    // 也可能返回 -14
}

pub async fn send_typing_via_ilink(
    client: &reqwest::Client,
    cfg: &ILinkSendConfig,
    typing_ticket: &str,
    status: i32,  // 1 = start, 2 = stop
) -> Result<SendTypingResponse, ILinkGetUpdatesError> {
    // POST /sendtyping { ilink_user_id, typing_ticket, status }
    // 也可能返回 -14
}
```

---

### 修正 5：Session 失效通知机制（P0 但架构问题）

**问题**：当 -14 返回时，谁来通知应用程序需要重新扫码？

**当前代码**：
```rust
Err(ILinkGetUpdatesError::SessionExpired { errmsg }) => {
    tracing::error!(...);
    return;  // ← 轮询线程自行停止，但没有通知机制
}
```

**修复思路**：
需要一个事件通知系统。可选方案：

**方案 A：内部事件通道**
```rust
pub enum ChannelEvent {
    SessionExpired { reason: String },
    // ...
}

// 在 Channel trait 中添加
pub async fn start(&self, inbound_tx: mpsc::Sender<Message>, event_tx: mpsc::Sender<ChannelEvent>) -> Result<(), String>
```

**方案 B：返回特殊错误给 HTTP API**
```rust
// 在 /api/send 中
if resp contains SessionExpired {
    return { ok: false, error: "session_expired", action: "please_rescan_qr" }
}
```

**方案 C：将 session 状态暴露给应用程序查询**
```rust
pub async fn get_channel_status(&self, channel_id: &str) -> ChannelStatus {
    // 应用程序定期查询状态，发现失效后主动重新登录
}
```

**建议**：当前阶段选择**方案 B**（最简单），留出接口支持后续升级。

---

## ✅ 修正后的实施清单

| 优先级 | 修正 | 位置 | DoD |
|------|------|------|-----|
| **P0** | 1️⃣ 检查 Token 非空 | channel.rs:send_message | 空 token 返回明确错误 |
| **P0** | 2️⃣ 澄清 Per-User 缓存限制 | channel.rs:SessionState | 代码注释清晰 |
| **P0** | 3️⃣ 定义 Session Reset 行为 | ilink.rs:reset_session_on_expiry | 注释解释每一步 |
| **P0** | 4️⃣ Session Expired 通知机制 | 架构设计 | 定出选择（A/B/C）并实施 |
| **P1** | 5️⃣ Typing Indicator 函数 | ilink.rs | get_config / send_typing 实现 |
| **P1** | 6️⃣ Token TTL / 生命周期 | session.rs | 待官方信息更新 |

---

## 🎯 结论

### 关键发现
1. **主动推送不可行**（当前设计只能回复）
2. **多用户场景需要升级**（当前假设单账户）
3. **Session Reset 的确切范围需要确认**（官方文档未完全明确）
4. **缺少通知机制**（应用程序不知道何时需要重新登录）

### 建议的下一步
1. ✅ 实施 P0 四个修正
2. ✅ 在代码注释中明确限制和假设
3. ❓ 与官方/参考实现确认 "Delete stored credentials" 的范围
4. 📝 文档化：系统支持模式（被动回复），不支持主动推送

