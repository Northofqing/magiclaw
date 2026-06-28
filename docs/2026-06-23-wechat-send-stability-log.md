# WeChat Send Stability Log

**Started:** 2026-06-23

| ID | Status | Issue | Evidence | Root Cause | Fix / Next Step | Verification |
| --- | --- | --- | --- | --- | --- | --- |
| WSS-001 | open | HTTP `/api/send` bypasses outbox | `src/infrastructure/runtime.rs` calls `send_text_via_ilink` directly | HTTP adapter owns send side effects | Change handler to enqueue outbox and return queued status | pending |
| WSS-002 | open | CLI fallback bypasses outbox | `src/main.rs` direct ilink fallback | CLI owns platform send when daemon unreachable | Enqueue local outbox instead of direct send | pending |
| WSS-003 | open | Context token persisted outside channel ownership | HTTP handler writes `context_tokens.json`; `WeChatChannel` keeps one in-memory token | Token state split across adapter and channel | Add SQLite context token store and inject into channel | pending |
| WSS-004 | open | Duplicate long-poll can race on `sync_buf` | Runtime HTTP API starts token long-poll while `WeChatChannel::start` also polls | Two owners for the same session cursor | Keep session/token refresh in WeChatChannel; HTTP no direct poll | pending |
| WSS-005 | fixed | Outbox pending scan lacks indexes | `outbox` schema had no status/time indexes | Full scans under higher volume | Added `idx_outbox_pending` and `idx_outbox_retry` in DB init | `cargo test context_token --lib` |
| WSS-006 | open | Outbox fetch then mark is not atomic | `fetch_pending` then `mark_status(sending)` | Multi-worker duplicate-send risk | Record as deployment risk or add claim API | pending |
