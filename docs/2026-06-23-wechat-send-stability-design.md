# WeChat Send Stability Design

**Date:** 2026-06-23
**Status:** Approved for implementation
**Capability level target:** `closed`

## Main Closed Loop

Entry points must converge on one recoverable send path:

```text
HTTP / MCP / CLI / Push
  -> Outbox.pending
  -> OutboxWorker claim
  -> ChannelRegistry
  -> WeChatChannel
  -> ilink sendmessage
  -> Outbox.sent / retrying / dead_letter
```

The runtime composition root remains `AppRuntime`. A send capability is not counted as closed unless it is wired into this chain and covered by an integration test.

## Context Token Ownership

`context_token` is runtime state for a WeChat peer, not an HTTP send side effect. The source of truth moves to SQLite:

```text
wechat_context_tokens(channel, account, peer_id, context_token, stale, updated_at)
```

Responsibilities:

- `WeChatChannel` long-poll receives inbound user messages and persists `(account, peer_id) -> context_token`.
- `WeChatChannel::send_message` loads the token by `peer_id` before calling ilink.
- If ilink returns a refreshed `context_token`, `WeChatChannel` writes it back.
- HTTP `/api/send` may upsert a request-provided token, then enqueue outbox. It does not direct-send.
- `context_tokens.json` remains a compatibility import/export file, not the primary runtime store.

## Failure Modes

| Failure | Handling |
| --- | --- |
| DB insert fails before send | request fails; no platform send happens |
| ilink transport/business failure | outbox transitions to retrying or dead_letter |
| process crashes while sending | startup recovery resets sending/retrying to pending |
| missing peer context token | send fails without direct platform call; outbox retries then DLQ |
| session expired | ilink business error surfaces to outbox retry/DLQ; long-poll stops after bounded retries |
| duplicate daemon workers | outbox claim must become atomic before multi-worker deployment |

## Rollback

The previous direct send path can be restored by reverting the HTTP/CLI entry changes. The token table is additive and does not alter existing message schemas. If needed, tokens can still be reconstructed from `context_tokens.json` or new inbound messages.

## Verification Gate

- HTTP `/api/send` returns queued status and creates an outbox row.
- WeChatChannel sends using a persisted peer token.
- Inbound long-poll persists peer tokens to SQLite.
- Failed sends follow retry/DLQ.
- Crash recovery returns in-flight messages to pending.
- Existing ilink contract tests pass.
