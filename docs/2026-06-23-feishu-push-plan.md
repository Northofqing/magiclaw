# Feishu Push Implementation Plan (Webhook-First)

Date: 2026-06-23
Status: Ready for execution
Goal: Deliver a production-credible Feishu push closed loop using existing webhook path first, then add long-connection path safely.

## Scope

In scope:
- Phase A: webhook-first stabilization and closure
- Phase B: long-connection adapter behind feature switch
- Phase C: resilience and observability hardening

Out of scope for this iteration:
- Full replacement of webhook path
- New business workflows unrelated to message ingress and delivery

## Task 1: Phase A Baseline Freeze And Gap Audit

Files:
- Modify: docs/2026-06-23-feishu-push-architecture.md
- Modify: docs/2026-06-23-feishu-push-plan.md
- Optional note update: docs/implementation-gap-task-breakdown.md

Steps:
1. Mark current Feishu capabilities as closed or experimental or stub in architecture doc.
2. Record exact webhook-first closed loop entry and exit.
3. List red-line mapping and unresolved gaps.

Exit criteria:
- Capability classification is explicit and reviewable.
- One and only one main closed loop is identified for Phase A.

## Task 2: Feishu Permission And Config Validation Guardrails

Files:
- Modify: src/main.rs
- Modify: src/infrastructure/runtime.rs
- Add or modify tests under tests/

Steps:
1. Add startup validation logs for feishu enabled state and minimal required fields.
2. Validate receive_id_type against allowed set and fail fast on invalid values.
3. Add a health detail that reflects webhook verification config completeness.

Exit criteria:
- Invalid Feishu runtime config is rejected before traffic.
- Health output exposes Feishu readiness for operations.

## Task 3: Webhook Ingress Hardening

Files:
- Modify: src/channels/feishu/channel.rs
- Modify: src/infrastructure/runtime.rs
- Modify: tests/feishu_webhook_closed_loop.rs
- Add: tests/feishu_webhook_security_closed_loop.rs

Steps:
1. Tighten signature verification error classification for audit and observability.
2. Ensure duplicate detection response is explicit and side-effect free.
3. Add tests for token mismatch, signature mismatch, malformed payload, and duplicate event.

Exit criteria:
- Security and malformed ingress cases are covered by closed-loop tests.
- Duplicate events are acknowledged without duplicate downstream effects.

## Task 4: Outbox And Failure Semantics Reinforcement

Files:
- Modify: src/channels/feishu/channel.rs
- Modify: src/application/outbox_worker.rs
- Modify: src/adapters/sqlite_outbox.rs
- Modify: tests/feishu_media_retry_dlq_closed_loop.rs
- Add: tests/feishu_permission_denied_closed_loop.rs

Steps:
1. Normalize Feishu API rejection mapping to retryable and terminal classes.
2. Ensure permission-denied paths produce actionable dead-letter context.
3. Add test for 403 or code-based permission rejection into retry and dead-letter path.

Exit criteria:
- Retry and dead-letter behavior is deterministic and test-backed.
- Dead-letter records include reason codes useful for replay decisions.

## Task 5: Multi-Account Isolation Validation

Files:
- Modify: src/infrastructure/runtime.rs
- Add: tests/feishu_multi_account_isolation_closed_loop.rs

Steps:
1. Validate account selection for webhook events by verification token and headers.
2. Assert route key channel namespace separation per account_id.
3. Assert failures in account A do not affect sends for account B.

Exit criteria:
- Multi-account path-level isolation has integration evidence.

## Task 6: Audit Completeness For Feishu Decisions

Files:
- Modify: src/application/audit.rs
- Modify: src/adapters/sqlite_audit.rs
- Modify: src/infrastructure/runtime.rs
- Add: tests/audit_send_closed_loop.rs updates for Feishu cases

Steps:
1. Record ingress decision, dedup decision, and outbound decision with route key.
2. Record high-risk auto policy changes and permission rejection details.
3. Ensure audit rows are queryable by channel and account.

Exit criteria:
- Feishu critical decisions are audit-complete and searchable.

## Task 7: Phase B Long-Connection Adapter (Experimental)

Files:
- Add: src/channels/feishu/long_conn.rs
- Modify: src/channels/feishu/mod.rs
- Modify: src/infrastructure/config.rs
- Modify: src/infrastructure/runtime.rs
- Add: tests/feishu_long_connection_closed_loop.rs

Steps:
1. Introduce feature switch for long-connection receiver.
2. Implement event receive loop for im.message.receive_v1 and feed unified ingress path.
3. Keep webhook path active as fallback.

Exit criteria:
- Long-connection receive works under switch.
- Webhook fallback remains available and healthy.

## Task 8: Phase C Resilience And Operational Hardening

Files:
- Modify: src/core/resilience.rs
- Modify: src/channels/feishu/channel.rs
- Modify: src/core/telemetry.rs
- Add: tests/resilience_closed_loop.rs Feishu coverage
- Modify: docs/mcp-deployment.md or ops doc with Feishu runbook

Steps:
1. Apply circuit breaker around Feishu OpenAPI calls.
2. Ensure bulkhead separation between AI pool and send pool remains effective for Feishu traffic.
3. Add telemetry for send success rate, retry depth, DLQ size, and token exchange failures per account.

Exit criteria:
- Feishu path has resilience controls with test and telemetry evidence.

## Test Execution Matrix

Core commands:
- cargo test --test feishu_webhook_closed_loop
- cargo test --test feishu_media_retry_dlq_closed_loop
- cargo test --test audit_send_closed_loop
- cargo test --test resilience_closed_loop

Phase A additional commands to add:
- cargo test --test feishu_webhook_security_closed_loop
- cargo test --test feishu_permission_denied_closed_loop
- cargo test --test feishu_multi_account_isolation_closed_loop

## Release And Rollback Checklist

Release checklist:
1. Feishu app published with required scopes and available range confirmed.
2. FEISHU_APP_ID and FEISHU_APP_SECRET loaded in runtime env.
3. Webhook verification token and signing secret configured and validated.
4. Closed-loop tests pass in CI.

Rollback checklist:
1. Disable long-connection switch first.
2. Keep webhook-first path active.
3. Disable only affected Feishu account by account_id if partial incident.
4. Replay DLQ after fix and verify outbox sent transitions.

## Immediate Next Work Package (Recommended)

Start with Task 2, Task 3, Task 4, and Task 5 as one PR stack to reach a hard closed Phase A gate quickly.
