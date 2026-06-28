# WeChat Context Token 健壮性设计

**Date**: 2026-06-23  
**Status**: Design Review  
**Priority**: P0 - Blocking (causes send failures with hidden error)

---

## 问题陈述

用户反馈：
1. 推送返回成功，但微信收不到消息
2. 实际原因是 `context_token` 已失效
3. 接口错误被隐藏，返回错误的成功状态

根本原因：
- WeChat iLink 协议中 `context_token` 是临时凭证，来自入站消息
- 长时间无入站消息时，使用的 token 会过期（errcode: -14）
- 当前代码未能识别 -14，也没有恢复机制
- 发送错误被隐藏（errcode 检查逻辑有问题）

---

## 官方协议规则（来源：https://www.wechatbot.dev/en/protocol）

### 1. Context Token 源头
```
POST /getupdates (long-poll, 35s hold)
  ↓ 返回
{ ret: 0, msgs: [{ from_user_id, context_token, item_list }], get_updates_buf: "new_cursor" }
```
- **每条入站消息都包含 `context_token`**
- SDK 应缓存最新 token
- 跨重启持久化

### 2. 发送规则
```
POST /sendmessage { context_token, text, ... }
  ↓ 返回
{ ret: 0, errcode: 0 } 或 { ret: -2, errcode: -14, errmsg: "session timeout" }
```
- **发送必须包含有效的 `context_token`**
- 过期 token 返回 `errcode: -14`

### 3. Session 过期处理
- 任何 API 返回 `errcode: -14` 时，session 已失效
- 恢复步骤：
  1. 清除缓存的 context_tokens
  2. 清除轮询光标（get_updates_buf）
  3. 删除存储的凭证（但保留 bot_token？待确认）
  4. 获取新 QR 码并重新扫码

---

## 当前代码问题

### 问题 A：errcode 检查逻辑错误
**File**: `src/channels/wechat/ilink.rs:296-317`

```rust
let errcode = value.get("errcode").and_then(|v| v.as_i64()).unwrap_or_default();
if errcode != 0 {
    // 只有 errcode != 0 时才返回错误，否则 Ok
    return Err(...);
}
Ok(value)
```

**问题**：
- 当 errcode 不存在时，`unwrap_or_default()` 返回 0
- 只要没有 errcode 字段，就当作成功
- 实际上应该检查 `errcode` 和 `ret` 的组合
- 官方文档：ret: 0 + errcode: 0 = 真正成功

**错误举例**：
- 响应：`{ ret: 0, errcode: -14 }`
- 当前代码：errcode 被解析为 -14，errcode != 0 为真，所以返回错误 ✓（偶然正确）
- 但问题是：-14 是特殊错误，应该单独处理，不能用通用 Err

### 问题 B：没有识别 session expired（-14）
**File**: `src/channels/wechat/channel.rs:197-202`

```rust
let updates = match get_updates_via_ilink(...).await {
    Ok(updates) => updates,
    Err(e) => {
        tracing::warn!(..., error = %e, "wechat inbound poll failed");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        continue;  // ← 盲目重试
    }
};
```

**问题**：
- getupdates 返回 errcode: -14 时，错误被当作普通网络错误处理
- 只记日志后重试，永远无法恢复
- 应该立即清除状态、停止轮询、等待重新登录

### 问题 C：context_token 更新不及时
**File**: `src/channels/wechat/channel.rs:270` 和 `320-326`

```rust
// 更新（只在收到消息时）
if let Some(ctx) = extract_latest_context_token(...) {
    let mut state = poll_session.lock().await;
    state.context_token = ctx;
}

// 使用（在发送时）
let state = session.lock().await;
let mut send_cfg = config.clone();
send_cfg.context_token = state.context_token.clone();  // ← 可能很旧
```

**问题**：
- Token 只在收到入站消息时更新
- 长时间无入站消息时，发送用的是数小时前的 token
- Token 过期率很高

### 问题 D：context_token 没有持久化
**File**: `src/channels/wechat/channel.rs:15-16`

```rust
struct SessionState {
    context_token: String,  // ← 内存存储，重启丢失
    // ...
}
```

**问题**：
- 重启后需要重新登录才能获得 token
- 如果登录后 5 秒就有人发送消息，那 5 秒内发送会失败

### 问题 E：get_updates_via_ilink 返回通用错误类型
**File**: `src/channels/wechat/ilink.rs:145-165`

```rust
pub enum ILinkGetUpdatesError {
    Transport(String),
    Business { ret: i32, errcode: i32, errmsg: String },
}
```

**问题**：
- 无法区分"网络错误"与"session expired"
- Business 中的 errcode 可能是 -14，但上层无法识别

---

## 修复方案

### 方案 A：扩展 ILinkGetUpdatesError 以支持 Session Expired

**目标**：使上层能识别 -14 并执行恢复

**实现**：
```rust
pub enum ILinkGetUpdatesError {
    Transport(String),
    SessionExpired { errmsg: String },  // ← 新增
    Business { ret: i32, errcode: i32, errmsg: String },
}

// 在 get_updates_via_ilink 中
if value.ret != 0 {
    let errcode = value.errcode.unwrap_or_default();
    let errmsg = value.errmsg.clone().unwrap_or_else(|| "unknown error".into());
    
    if errcode == -14 {
        return Err(ILinkGetUpdatesError::SessionExpired { errmsg });  // ← 特殊处理
    }
    
    return Err(ILinkGetUpdatesError::Business { ret: value.ret, errcode, errmsg });
}
```

**上层处理**：
```rust
let updates = match get_updates_via_ilink(...).await {
    Ok(updates) => updates,
    Err(ILinkGetUpdatesError::SessionExpired { errmsg }) => {
        // 触发 session reset：清除 token、sync_buf、停止轮询
        tracing::error!("WeChat session expired: {}", errmsg);
        // TODO: 通知主线程执行重新登录
        return;  // ← 停止轮询
    }
    Err(e) => {
        tracing::warn!(error = %e, "wechat poll failed");
        tokio::time::sleep(...).await;
        continue;
    }
};
```

**改动**：
- File: `src/channels/wechat/ilink.rs` - 新增 `SessionExpired` 变体 + 检查逻辑
- File: `src/channels/wechat/channel.rs` - 长轮询循环中识别并处理

---

### 方案 B：扩展 send_text_via_ilink 以传播 Session Expired

**目标**：当发送时 token 过期，能识别错误并清除状态

**实现**：
```rust
pub enum SendMessageError {
    Transport(String),
    SessionExpired { errmsg: String },  // ← 新增
    Business { ret: i32, errcode: i32, errmsg: String },
}

pub async fn send_text_via_ilink(...) -> Result<serde_json::Value, SendMessageError> {
    // ... HTTP 请求 ...
    
    let errcode = value.get("errcode").and_then(|v| v.as_i64()).unwrap_or_default();
    
    if errcode == -14 {
        return Err(SendMessageError::SessionExpired {
            errmsg: value.get("errmsg").and_then(|v| v.as_str()).unwrap_or("session expired").to_string(),
        });
    }
    
    if errcode != 0 {
        return Err(SendMessageError::Business { ... });
    }
    
    Ok(value)
}
```

**上层处理**：
```rust
let resp = send_text_via_ilink(client, &send_cfg, to, &body).await?;
// 变成：
let resp = match send_text_via_ilink(...).await {
    Ok(v) => v,
    Err(SendMessageError::SessionExpired { errmsg }) => {
        tracing::error!("token expired during send: {}", errmsg);
        return Err(format!("WeChat token expired, please re-login"));  // ← 用户可见错误
    }
    Err(e) => return Err(format!("send failed: {}", e)),
};
```

**改动**：
- File: `src/channels/wechat/ilink.rs` - 新增 `SendMessageError::SessionExpired`
- File: `src/channels/wechat/channel.rs` - 在 `send_message` 中处理

---

### 方案 C：持久化 context_token

**目标**：重启后保留 token，减少重新登录的需要

**实现**：
```rust
// src/channels/wechat/session.rs
pub struct WeChatSession<S: SyncBufStore> {
    // ...
    context_token: String,  // ← 新增
}

pub fn set_context_token(&mut self, token: String) {
    self.context_token = token;
    // TODO: 持久化到 SQLite
}

pub fn get_context_token(&self) -> &str {
    &self.context_token
}

pub fn clear_context_token(&mut self) {
    self.context_token.clear();
}
```

**改动**：
- File: `src/channels/wechat/session.rs` - 添加 context_token 字段
- File: `src/adapters/` - 可能需要新增存储表或扩展现有表

**讨论**：
- 是否需要单独的 SQLite 表，还是放在现有 sync_buf 表中？
- Token 过期时间有多长？需要 TTL 机制吗？

---

### 方案 D：主动刷新 context_token（可选，P1）

**目标**：在发送前确保 token 新鲜

**实现选项 1**：发送前检查年龄，如需要则主动 getupdates
```rust
async fn send_message(&self, to: &str, content: &MessageContent) -> Result<SendReceipt, String> {
    // 如果 token 年龄 > 5 分钟，先做一次空的 getupdates 来刷新
    if state.last_context_token_update.elapsed() > Duration::from_secs(300) {
        // 触发一次 getupdates 来更新 token（即使没有消息）
        let _ = get_updates_via_ilink(&client, &cfg, &sync_buf).await;
        // 重新读取更新的 token
    }
    // 发送消息
}
```

**实现选项 2**：后台定期保活
```rust
// 在长轮询任务中，如果连续超过 N 分钟无消息，主动发一个空的 getupdates
```

**改动**：
- File: `src/channels/wechat/session.rs` - 添加 `last_context_token_update` 时间戳
- File: `src/channels/wechat/channel.rs` - 在发送前检查并刷新

---

### 方案 E：Session Expiry 恢复流程（P0）

**目标**：当检测到 -14 时，立即清除状态

**实现**：
```rust
fn reset_session(state: &mut SessionState) {
    state.context_token.clear();
    // sync_buf 由上层清除
    // 日志记录
    tracing::warn!("WeChat session reset due to expiry");
}

// 在长轮询中
Err(ILinkGetUpdatesError::SessionExpired { errmsg }) => {
    let mut state = poll_session.lock().await;
    reset_session(&mut state);
    // 清除 sync_buf（可选，也可以保留让服务器决定）
    drop(state);
    
    // 停止轮询或等待重新登录信号
    // 可选：向外部系统发送通知要求重新扫码
    
    return;  // 停止轮询线程
}
```

**改动**：
- File: `src/channels/wechat/channel.rs` - 长轮询中的错误处理

---

## 实现顺序（7 步流程）

| 步骤 | 任务 | DoD |
|------|------|-----|
| 1 | 扩展 ILinkGetUpdatesError（方案 A） | 新增 SessionExpired 变体，编译通过 |
| 2 | 扩展 send_text_via_ilink 返回类型（方案 B） | 新增 SendMessageError::SessionExpired |
| 3 | 长轮询中识别并处理 -14 | 日志验证 session expired 时正确响应 |
| 4 | 发送中识别并报告 -14 | 日志验证发送时 token 过期时正确报错 |
| 5 | 持久化 context_token（方案 C） | Token 重启后保留，集成测试通过 |
| 6 | Token 年龄检查与刷新（方案 D，可选） | 单元测试通过 |
| 7 | 系统级测试（Token 过期场景） | 模拟 -14 场景，验证完整恢复流程 |

---

## 失败模式分析

| 失败场景 | 当前行为 | 修复后行为 |
|---------|--------|-----------|
| 登录 1 小时后，token 过期 | 发送返回"成功"但微信收不到 | 发送返回 SessionExpired 错误，用户知道需要重新登录 |
| 长轮询过程中返回 -14 | 一直重试，永远无法恢复 | 停止轮询，清除状态，等待重新登录 |
| 进程重启，token 丢失 | 第一条消息发送失败 | 从 SQLite 恢复 token，发送成功（如 token 仍有效） |
| 发送与轮询并发竞争 | Token 被同时读写（race condition） | SessionState 加锁保护 |

---

## 跨模块影响检查

- **adapters/**: 是否需要新的存储层？
- **application/**: 出站/入站处理器是否需要调整？
- **domain/**: 是否需要新的聚合根或服务？
- **infrastructure/**: 是否需要扩展 SQLite schema？

建议：先实现方案 A、B、E（P0），再考虑方案 C、D（P1）。

---

## 与红线对齐

- ✅ 路由隔离（RouteKey 包含 conversation）- 不受影响
- ✅ 序列化处理（per-RouteKey 串行） - 不受影响
- ✅ 可恢复投递 - **改进**：token 过期时能正确识别并恢复
- ✅ 审计日志 - Token 过期应记入审计表

---

## 下一步

1. ✏️ 设计评审通过
2. 📋 拆分为 7 个独立任务
3. 🔧 按顺序实现并逐个测试
