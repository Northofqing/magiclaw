# WeChat Send Stability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans or execute these steps task-by-task with review checkpoints.

**Goal:** Stabilize WeChat sending by making every send recoverable and by persisting per-peer context tokens in SQLite.

**Architecture:** Keep Clean/Hexagonal boundaries: domain ports define token storage, SQLite implements the port, application send use cases write outbox, and WeChatChannel owns ilink session state. Runtime remains the only composition root.

**Tech Stack:** Rust, Tokio, rusqlite, axum, reqwest, SQLite WAL, existing outbox/DLQ worker.

---

## Task 1: Persistent Context Token Port

**Files:**
- Create: `src/domain/ports/context_token_store.rs`
- Create: `src/adapters/sqlite_context_tokens.rs`
- Modify: `src/domain/ports/mod.rs`
- Modify: `src/adapters/mod.rs`
- Modify: `src/infrastructure/db.rs`

Steps:
- Add a `ContextTokenStore` trait with `save`, `load`, `mark_stale`, and `list`.
- Add `SqliteContextTokenStore` using table `wechat_context_tokens`.
- Add schema and indexes in `init_db`.
- Unit test save/load, stale mark, and missing token behavior.

## Task 2: WeChatChannel Token Ownership

**Files:**
- Modify: `src/channels/wechat/channel.rs`
- Modify: `src/infrastructure/runtime.rs`

Steps:
- Inject `Arc<dyn ContextTokenStore>` into `WeChatChannel`.
- Persist inbound context tokens during long-poll.
- Load token by `to` before send.
- Persist refreshed token from ilink send response.
- Wire the store in `AppRuntime::new`.

## Task 3: HTTP Send Uses Outbox

**Files:**
- Modify: `src/infrastructure/runtime.rs`
- Modify or add tests under `tests/`

Steps:
- Add outbox repo and token store to HTTP API state.
- `/api/send` upserts request context token when provided.
- `/api/send` calls `submit_text_for_delivery` and returns queued `message_id`.
- Remove direct ilink send from HTTP handler.

## Task 4: CLI Fallback Safety

**Files:**
- Modify: `src/main.rs`

Steps:
- Keep daemon HTTP submission as preferred path.
- If daemon is unreachable, enqueue directly into local SQLite outbox instead of direct ilink.
- Print queued message id instead of direct-send receipt.

## Task 5: Outbox Stability

**Files:**
- Modify: `src/infrastructure/db.rs`
- Modify: `src/adapters/sqlite_outbox.rs` if atomic claim is implemented in this pass

Steps:
- Add indexes for pending and retryable scans.
- Record any remaining non-atomic claim risk in the stability log if not fully solved in this pass.

## Task 6: Verification

Commands:
- `cargo test outbox --lib`
- `cargo test wechat --lib`
- `cargo test rest_auth_closed_loop --test rest_auth_closed_loop`
- `cargo test mcp_stdio_closed_loop --test mcp_stdio_closed_loop`
- `cargo test feishu_media_failure_goes_retry_then_dlq_and_can_replay --test feishu_media_retry_dlq_closed_loop`

Update `docs/wechat-send-stability-log.md` after each failing or fixed item.
