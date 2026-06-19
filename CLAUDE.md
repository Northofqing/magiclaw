# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

信道中心架构系统 (Channel-Centric Architecture System) — a Rust-based multi-platform messaging hub. The system routes and processes messages across chat platforms (WeChat, Dingtalk, Feishu) with channel stability and recoverable delivery as top priorities.

**Core principles:**
- AI is an optional/pluggable capability; the system runs independently without any agent dependency
- MCP is an adapter, not part of the core
- Message channel stability and ordering correctness take priority over throughput

## Architecture

```
Adapter Layer (MCP / REST / CLI)
        ↓
Pipeline Layer (Normalize → Dedup → Permission → RateLimit → ConversationLoad → AI/Rule → Formatter → Outbox)
        ↓
Message Core (Route Registry + Per-RouteQueue + Worker Idle GC + Reorder Window + Retry + DLQ)
        ↓
Channels (WeChat / Dingtalk / Feishu)
```

**Key design rules:**
- **Core never depends on Agent or Adapter.** Adapters (MCP/REST/CLI) must not leak into the core business model.
- **Conversation is a first-class aggregate root.** Message is an event. `RouteKey` includes `channel`, `conversation_id`, `peer_id`, and `conversation_type`.
- **Same RouteKey → serial processing; different RouteKeys → parallel.** This is the fundamental concurrency model.
- **Route Workers have Idle GC** — idle workers are recycled after 30 minutes by default.
- **Dedup uses Moka TTL cache** (not full-table retain). TTL 5 minutes, max 2M entries.
- **Reorder Window**: platforms with sequence numbers sort by sequence; those without use `timestamp + reorder_window_ms` (default 200ms). Late messages are idempotency-processed with audit markers.
- **Send state machine**: `pending → sending → sent`, failure → `retrying`, threshold exceeded → `dead_letter`.
- **MCP stdio zero-pollution**: stdout is protocol-only; business logs go to stderr/file only.
- **Multi-account/channel isolation is path-level**: session, sync_buf, allowlist, inbox/outbox, audit all namespaced by channel/account.
- **Media uploads must stream/chunk** — never read large files entirely into memory.
- **Circuit Breaker on external platform APIs and AI APIs.** AI execution pool and send execution pool use Bulkhead isolation.
- **All key data flows and send decisions are audit-logged** (source, time, RouteKey, decision basis, result). Audit logs immutable, retention ≥ 5 years.

## Planned Directory Structure

```
src/
├── core/           # types, router, pipeline, processor, reorder, dedup, rate_limit, resilience, event_bus, capabilities, telemetry
│   └── storage/    # inbox, outbox, dlq, conversation
├── channels/       # registry (route worker registry + idle janitor), wechat/session (sync_buf persistence)
└── adapters/       # mcp, rest, cli
```

## Persistence (SQLite)

Tables: `inbox`, `outbox`, `dead_letter`, `conversation_state`, `audit_log`. Plus: `config` (JSON), `allowlist`, `session`.

WeChat `sync_buf` must be persisted (every update written immediately) and recoverable on restart.

## Development Process (from AGENTS.md)

The project follows a 7-step gated process. Design docs and plans live in `docs/`, carried via PR checklists.

**Review gate rule**: only `closed` capabilities count toward completion. Unit tests are necessary but not sufficient; a stage is complete only when its closed-loop review gate is satisfied.

| Step | Command/Action | Key Gate |
|------|---------------|----------|
| 1 | `/architecture-patterns` — design doc | Must include data flow diagram, failure mode analysis, rollback plan, relationship to old modules, and stage closed-loop definition |
| 2 | 4-role challenge (PM, Sr Dev, Trading Analyst, DDD Architect) | Exit when no new blocking objections; max 3 rounds; include review-gate objections explicitly |
| 3 | `/project-planner` — task breakdown | Each task independently verifiable, dependencies clear, data redlines marked, and each stage has exactly one main closed loop |
| 4 | `/andrej-karpathy-skills:karpathy-guidelines` — code | Pass lint + unit tests; no mock residue in production paths; explicit failure handling per data source; no stub/preview counted as closed |
| 5 | `/review` | All review comments recorded; old-module integration checklist completed; data redline checklist completed; closed vs experimental vs stub clearly separated |
| 6 | Fix review issues | All resolved or explicitly `wontfix` with reviewer confirmation; unresolved P0 items block merge |
| 7 | Test verification | Unit test line coverage ≥ 80%; core trading/data paths ≥ 95%; regression + live data validation pass; closed-loop tests required for merge |

**Root-cause rollback**: test failures roll back to the causal step, not always step 4 (see AGENTS.md table).

**Old-module integration checklist** (step 5): for every new capability, list all existing related modules, decide whether each should upgrade to the new capability, and record plans or explicit reasons.

### Review Gate Checklist (summary)

- Phase 1 closed-loop: Inbound -> Dedup -> Route -> Queue -> Reorder -> Worker -> GC.
- Phase 1.5 closed-loop: MCP framed request -> handler -> domain/application entry -> framed response, with stdout zero-pollution.
- Phase 2 closed-loop: Send Request -> Outbox.pending -> Worker.dequeue -> Outbox.sending -> Channel.send -> sent/retrying/dead_letter -> restart recovery.
- Only capabilities that are `closed` and covered by a closed-loop test may be marked complete.

## Mandatory Red Lines (MUST — merge/launch blocking)

These are the P0 constraints from RUST_MIGRATION_V5.md Section 2:

- RouteKey includes conversation dimension
- Same-RouteKey serial, cross-RouteKey parallel
- 30-min idle route GC
- Dedup via TTL cache (moka)
- Out-of-order messages handled by reorder strategy or idempotency
- Inbox/Outbox/DLQ with retry and dead-letter replay
- Core state persisted (not in-memory)
- sync_buf persisted and restart-recoverable
- Crash recovery resumes incomplete sends
- MCP stdout zero-pollution
- ilink contract tests (ret, errcode, sync_buf, X-WECHAT-UIN, AES-128-ECB/PKCS7)
- Path-level multi-account/channel isolation
- Streamed media uploads
- REST adapter requires auth, no open ports by default
- AI and external APIs behind Circuit Breaker
- Bulkhead isolation between AI pool and send pool
- High-risk operations (auto-allowlist) write audit log
- Audit logs immutable, retention ≥ 5 years

**Completion rule**: a red line is not complete until it has (1) a design definition, (2) a runtime call site, (3) a closed-loop test, and (4) an explicit failure/recovery path.

## Migration Phases

1. **Phase 1** (2 weeks): Stable kernel — RouteKey upgrade, Idle GC, TTL dedup, reorder window, sync_buf persistence
2. **Phase 1.5** (0.5-1 week): MCP adapter (send/list_peers/login), stdout zero-pollution
3. **Phase 2** (1.5-2 weeks): Recoverable delivery — SQLite Inbox/Outbox/DLQ, retry, crash recovery
4. **Phase 3** (1-2 weeks): Multi-channel — Dingtalk/Feishu skeletons, isolation testing
5. **Phase 4** (1.5-2 weeks): Pipeline + pluggable AI — middleware chain, Echo/OpenAI/Claude/Local backends
6. **Phase 5** (2 weeks): Production hardening — Circuit Breaker, Bulkhead, daemon/health, audit tooling
