# WeChat Context Token 方案最终审核 - 基于官方实现

**Date**: 2026-06-23  
**基于**: https://github.com/corespeed-io/wechatbot (Rust/Go/Node.js/Python)  
**状态**: 关键漏洞已发现，修正方案已准备

---

## 🚨 第一轮审核的关键错误

### 错误 1：Token 缓存数据结构完全错误（P0）

**当前我们的代码**：
```rust
struct SessionState {
    context_token: String,  // ← WRONG!
}
```

**官方 Rust 实现**：
```rust
pub struct WeChatBot {
    context_tokens: RwLock<HashMap<String, String>>,  // ✅ userId → token
}
```

**官方 Node.js 实现**：
```typescript
class ContextStore {
    private readonly tokens = new Map<string, string>()  // ✅ userId → token
}
```

**官方 Go 实现**：
```go
contextTokens: &sync.Map{},  // ✅ userId → token
```

**问题**：
- 一个 WeChat 账户可能与多个用户聊天（群聊、多个直聊）
- 每个用户都需要自己的 context_token
- 当前代码只能存一个 token，发送给第二个用户时会用错 token

**影响范围**：
- 如果只与一个用户聊天，可以工作
- 如果有多个用户，第二个用户的消息发送会失败或路由到错误的用户

---

### 错误 2：Session Reset 的范围理解不完整

**官方 Node.js 的 clearAll() 实现**：
```typescript
async clearAll(): Promise<void> {
  await Promise.all([
    this.storage.delete(STORAGE_KEYS.CREDENTIALS),      // bot_token + baseurl
    this.storage.delete(STORAGE_KEYS.CURSOR),           // get_updates_buf
    this.storage.delete(STORAGE_KEYS.CONTEXT_TOKENS),   // 所有 per-user tokens
    this.storage.delete(STORAGE_KEYS.TYPING_TICKETS),   // 所有 typing tickets
  ])
}
```

**我的假设**：
- 只清除 context_token ❌
- 保留 bot_token ❌
- 保留 baseurl ❌

**正确理解**：
- Session 完全失效（errcode: -14）时，需要清除 **所有** 状态
- 包括 CREDENTIALS（bot_token + baseurl 都要清除）
- 原因：session 关联的所有 token 都失效了，需要重新扫码获得新的 bot_token

---

### 错误 3：缺少 Typing Service 的完整处理

**官方实现**（Node.js 示例）：
```typescript
class TypingService {
  private readonly ticketCache = new Map<string, { ticket: string; expiresAt: number }>()
  
  async startTyping(userId: string): Promise<void> {
    const ticket = await this.getTicket(userId)
    // POST /sendtyping { userId, ticket, status: 1 }
  }
  
  private async getTicket(userId: string): Promise<string | undefined> {
    // 1. 检查缓存（24h TTL）
    // 2. 需要 context_token 来调用 getConfig
    // 3. POST /getconfig { ilink_user_id, context_token }
    // 4. 返回 typing_ticket
  }
}
```

**问题**：
- `getConfig` 也需要 context_token
- `getConfig` 也可能返回 -14（session expired）
- 当前代码完全没有实现 typing 功能
- 这意味着"对方正在输入中"不会显示

---

### 错误 4：入站消息的用户 ID 抽取逻辑不够完整

**官方 Go 实现**：
```go
func (b *Bot) rememberContext(wire *WireMessage) {
    userID := wire.FromUserID              // 默认用 from_user_id
    if wire.MessageType == MessageTypeBot {
        userID = wire.ToUserID             // 但如果是 Bot 消息，用 to_user_id
    }
    if userID != "" && wire.ContextToken != "" {
        b.contextTokens.Store(userID, wire.ContextToken)
    }
}
```

**当前我们的代码**：
```rust
if let Some(ctx) = extract_latest_context_token(std::slice::from_ref(&msg), Some(&from_user_id)) {
    state.context_token = ctx;
}
```

**问题**：
- 直接用 `from_user_id`
- 没有检查消息类型（user vs bot）
- 如果是 bot 回复的消息，应该用 `to_user_id` 来索引 token

---

## ✅ 正确的修复方案（基于官方代码）

### 修复 1：将 Token 缓存改为 HashMap（P0 - MUST）

**File**: `src/channels/wechat/channel.rs`

**当前代码**：
```rust
struct SessionState {
    context_token: String,
}
```

**修复后**：
```rust
use std::collections::HashMap;

struct SessionState {
    // 改为 Map<userId, token>
    context_tokens: Arc<RwLock<HashMap<String, String>>>,
}

impl SessionState {
    pub async fn get_context_token(&self, user_id: &str) -> Option<String> {
        self.context_tokens.read().await.get(user_id).cloned()
    }
    
    pub async fn set_context_token(&self, user_id: String, token: String) {
        self.context_tokens.write().await.insert(user_id, token);
    }
    
    pub async fn clear_context_tokens(&self) {
        self.context_tokens.write().await.clear();
    }
}
```

**调用方改变**：
```rust
// 发送时
let token = state.get_context_token(&to_user_id).await
    .ok_or("no context_token for user")?;

// 接收时
let msg = /* ... */;
let user_id = if msg.message_type == Some(1) {
    msg.from_user_id.clone().unwrap_or_default()
} else {
    msg.to_user_id.clone().unwrap_or_default()
};
state.set_context_token(user_id, msg.context_token.clone()).await;
```

---

### 修复 2：Context Token 持久化到 SQLite（P1）

**新增表**：
```sql
CREATE TABLE IF NOT EXISTS wechat_context_tokens (
  account_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  token TEXT NOT NULL,
  updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (account_id, user_id)
);
```

**API**：
```rust
pub trait ContextTokenStore {
    async fn get(&self, account_id: &str, user_id: &str) -> Result<Option<String>>;
    async fn set(&self, account_id: &str, user_id: &str, token: &str) -> Result<()>;
    async fn delete_all(&self, account_id: &str) -> Result<()>;
}
```

---

### 修复 3：完整的 Session Reset 流程（P0）

**官方的 reset 步骤**（from Node.js clearAll）：
1. 清除 CREDENTIALS（bot_token, baseurl, account_id, user_id）
2. 清除 CURSOR（get_updates_buf）
3. 清除 CONTEXT_TOKENS（所有 per-user tokens）
4. 清除 TYPING_TICKETS（所有 typing tickets，如果实现了的话）

**当前代码应该改为**：
```rust
async fn reset_session_on_expiry(&self) {
    let mut state = self.session.lock().await;
    
    // 1. 清除所有 context_tokens
    state.context_tokens.write().await.clear();
    
    // 2. 清除 sync_buf（相当于 cursor）
    // （由应用层负责）
    
    // 3. 清除 credentials（由主进程负责）
    // TODO: 需要事件机制通知主进程重新登录
    
    tracing::warn!("WeChat session expired - cleared state, awaiting re-login");
}
```

---

### 修复 4：实现 Typing Service（P1）

**新增文件**：`src/channels/wechat/typing.rs`

```rust
use std::collections::HashMap;
use std::time::{SystemTime, Duration};

pub struct TypingTicketCache {
    // userId → (ticket, expiry_time)
    cache: Arc<RwLock<HashMap<String, (String, SystemTime)>>>,
}

impl TypingTicketCache {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    pub async fn get(&self, user_id: &str) -> Option<String> {
        let cache = self.cache.read().await;
        if let Some((ticket, expiry)) = cache.get(user_id) {
            if SystemTime::now() < *expiry {
                return Some(ticket.clone());
            }
        }
        None
    }
    
    pub async fn set(&self, user_id: String, ticket: String) {
        let expiry = SystemTime::now() + Duration::from_secs(24 * 3600);
        self.cache.write().await.insert(user_id, (ticket, expiry));
    }
    
    pub async fn clear(&self) {
        self.cache.write().await.clear();
    }
}

// 新增 API 调用
pub async fn get_config_via_ilink(
    client: &reqwest::Client,
    cfg: &ILinkSendConfig,
    user_id: &str,
    context_token: &str,
) -> Result<GetConfigResponse, SendMessageError> {
    let base = if cfg.base_url.ends_with('/') {
        cfg.base_url.clone()
    } else {
        format!("{}/", cfg.base_url)
    };
    let url = format!("{}ilink/bot/getconfig", base);
    let payload = serde_json::json!({
        "ilink_user_id": user_id,
        "context_token": context_token,
        "base_info": { "channel_version": cfg.channel_version }
    });
    
    let response = client
        .post(url)
        .headers(build_headers(&cfg.token))
        .json(&payload)
        .send()
        .await
        .map_err(|e| SendMessageError::Transport(format!("getconfig failed: {}", e)))?;
    
    let body = response.text().await
        .map_err(|e| SendMessageError::Transport(format!("failed to read response: {}", e)))?;
    
    let value: serde_json::Value = serde_json::from_str(body.trim())
        .map_err(|e| SendMessageError::Transport(format!("invalid JSON: {}", e)))?;
    
    // 检查 -14
    let errcode = value.get("errcode").and_then(|v| v.as_i64()).unwrap_or(0);
    if errcode == -14 {
        return Err(SendMessageError::SessionExpired {
            errmsg: value.get("errmsg").and_then(|v| v.as_str())
                .unwrap_or("session expired").to_string()
        });
    }
    
    if errcode != 0 {
        return Err(SendMessageError::Business {
            errcode,
            errmsg: value.get("errmsg").and_then(|v| v.as_str())
                .unwrap_or("unknown error").to_string()
        });
    }
    
    Ok(serde_json::from_value(value)?)
}
```

---

### 修复 5：改进入站消息的用户 ID 抽取（P1）

**File**: `src/channels/wechat/channel.rs`

```rust
// 改进的逻辑：根据消息类型选择 user_id
let user_id = if msg.message_type == Some(1) {  // USER message
    msg.from_user_id.clone().unwrap_or_default()
} else {  // BOT message
    msg.to_user_id.clone().unwrap_or_default()
};

if !user_id.is_empty() && let Some(ctx) = &msg.context_token {
    state.set_context_token(user_id, ctx.clone()).await;
}
```

---

## 📋 完整的修复清单

| # | 修正 | 文件 | P0/P1 | DoD | 依赖官方代码 |
|---|------|------|-------|-----|-----------|
| 1 | Token 缓存 HashMap | channel.rs | **P0** | 改为 `HashMap<String, String>` | Rust/Go/Node.js/Python |
| 2 | Session Reset 完整流程 | channel.rs | **P0** | 清除 credentials/cursor/tokens/tickets | Node.js |
| 3 | Session Expired 识别 | ilink.rs | **P0** | -14 返回 SessionExpired 错误 | 所有 |
| 4 | Token 持久化 | adapters/sqlite_context_tokens.rs (new) | P1 | 新增 SQLite 表 + Store trait | Node.js |
| 5 | Typing Service | channels/wechat/typing.rs (new) | P1 | 实现 getConfig + sendTyping | Node.js/Go |
| 6 | User ID 抽取逻辑 | channel.rs | P1 | 根据 message_type 选择 from/to | Go |
| 7 | 系统级测试 | tests/ | **P0** | 模拟 -14，验证完整恢复 | — |

---

## 🎯 与官方的对标

| 能力 | 官方 Rust | 官方 Node.js | 我们当前 | 修复后 |
|------|----------|------------|--------|------|
| Token 缓存类型 | HashMap | Map | ❌ String | ✅ HashMap |
| Token 持久化 | ❌ 无 | ✅ FileStorage | ❌ 无 | ✅ SQLite |
| Typing Service | ❌ 无 | ✅ TypingService | ❌ 无 | ✅ 实现 |
| -14 识别 | ✅ 有 | ✅ 有 | ❌ 无 | ✅ 有 |
| Session Reset | ✅ 有 | ✅ clearAll() | ❌ 不完整 | ✅ 完整 |
| Per-User Token | ✅ 有 | ✅ 有 | ❌ 无 | ✅ 有 |

---

## 💡 关键洞察

1. **主动推送只有在收到过消息后才能工作** — 这是设计的限制，不是 bug
2. **Token 必须是 Per-User 的 HashMap** — 一个账户通常对应多个用户
3. **Session 过期时需要完全清除状态** — 不只是 token，还包括 credentials
4. **Typing Service 是可选的** — 但如果实现，需要特殊处理 context_token

---

## 下一步

确认无误后，开始实施修复 1、2、3（P0），然后再做 4、5、6（P1）。

