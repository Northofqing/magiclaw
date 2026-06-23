# Feishu Push Architecture (Webhook-First)

Date: 2026-06-23
Status: Proposed
Capability target: closed (Phase A), experimental (Phase B transition), closed (Phase C)

## 1. Background And Goal

This repository already has Feishu send, webhook verification, inbound parse, and outbox retry and DLQ tests. The immediate goal is to make Feishu push production-stable with one explicit closed loop before introducing additional transport modes.

Chosen strategy:
- Phase A: Webhook-first hardening (recommended fast path)
- Phase B: Long-connection adapter added behind switch, keep webhook as fallback
- Phase C: production hardening with resilience and observability reinforcement

Why webhook-first:
- Existing runtime path and tests are already present
- Lower migration risk than transport replacement
- Fastest path to a closed capability that meets current red lines

## 2. Main Closed Loop (Phase A)

Single closed loop for inbound to outbound:

Inbound Feishu event (HTTP webhook)
-> signature and token validation
-> parse to domain message
-> inbox dedup and persistence
-> route enqueue (same RouteKey serial, cross RouteKey parallel)
-> pipeline processing
-> outbox pending
-> outbox worker send via Feishu channel
-> sent or retrying or dead_letter

Runtime composition root remains AppRuntime. No adapter logic leaks into core model.

## 3. Runtime Composition

Current key wiring points:
- Feishu env loading in main
- Feishu channel registration in runtime
- HTTP endpoint at /api/feishu/webhook
- Signature verification and payload parse in Feishu channel module

Required composition constraints:
- Core does not depend on MCP or REST details
- Channel/account namespace remains path-level isolated
- Outbox state machine remains single source of delivery truth

## 4. Data And Isolation Model

Route and identity:
- Channel id: feishu or feishu:<account_id>
- RouteKey includes channel, conversation_id, peer_id, conversation_type

Isolation:
- Multi-account Feishu config uses account_id namespace
- inbox, outbox, dead_letter, audit, and conversation flow remain channel-account isolated by route key and persistence metadata

Message mapping:
- text: content.text -> MessageContent::Text
- image: content.image_key
- file: content.file_key and metadata

## 5. Delivery And Media Strategy

Token:
- Prefer tenant token exchange from app_id and app_secret
- Optional static tenant_access_token for operational override

Media:
- Upload image and file by stream, never full-file load
- If media upload fails, fail send attempt and move through retrying and dead_letter according to outbox policy

Receive id type:
- Controlled by FEISHU_RECEIVE_ID_TYPE
- Must be explicit in rollout checklist (open_id or chat_id depends on traffic pattern)

## 6. Failure Modes And Recovery

Failure handling policy:
- Invalid signature or token mismatch: reject and audit
- Parse failure: reject and audit with payload fingerprint
- Duplicate message id: acknowledge duplicate and skip side effects
- Feishu API non-zero code or HTTP non-success: outbox retrying then dead_letter
- Crash during sending: recovery process returns in-flight entries to pending and resumes

No direct send outside recoverable path for production flows.

## 7. Capability Classification

Phase A classification:
- closed:
  - text send with outbox state machine
  - image and file send with streaming upload
  - webhook signature and token verification
  - inbound to outbox closed-loop tests
  - retry and dead letter replay tests
- experimental:
  - advanced event types beyond im.message.receive_v1
  - rich card interactions and menu workflows
- stub:
  - long-connection event receiver in this phase

Phase B target:
- long-connection event receiver moves from stub to experimental, webhook remains closed fallback

Phase C target:
- long-connection plus fallback policy reaches closed after resiliency and integration gates pass

## 8. Security And Compliance Requirements

Must-haves:
- Keep app_secret out of logs and error messages
- Keep API token auth enabled for protected HTTP endpoints
- Audit high-risk operations and send decision outcomes with route key and reason
- Document retention and immutability policy for audit records

Operational action required:
- Rotate exposed Feishu app_secret before production rollout

## 9. Verification Gates

Gate A (webhook-first closed):
- Feishu webhook ingress test passes
- Feishu media failure to retry to DLQ and replay test passes
- Multi-account route isolation test passes
- 403 permission-denied send path enters retrying and dead_letter with audit record

Gate B (long-connection experimental):
- Long-connection receiver can consume im.message.receive_v1 without public webhook
- Fault in long-connection path does not break webhook fallback

Gate C (production closed):
- Circuit breaker and bulkhead evidence for Feishu API path
- Recovery test proves unfinished sends resume after restart
- Observability dashboard includes send success, retry depth, DLQ count, token refresh failures, and per-account error rate

## 10. Rollback Strategy

Rollback order:
1. Disable long-connection receiver switch (if enabled)
2. Keep webhook-first closed loop as primary path
3. If needed, disable Feishu account entry by account_id while preserving other channels
4. Replay DLQ after fix and verify sent transitions

This rollback keeps message core and outbox model unchanged and minimizes blast radius.
