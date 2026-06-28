# Feishu Push PR Checklist Template

Copy this checklist into your GitHub PR description for Feishu push implementation PRs (Tasks 2-8 from the plan).

---

## PR Title Format

`feat(feishu): [Task N] <short description>`

Example: `feat(feishu): Task 3 webhook ingress hardening and security tests`

---

## Scope And Capability Classification

- [ ] PR implements exactly one task from [2026-06-23-feishu-push-plan.md](../2026-06-23-feishu-push-plan.md)
- [ ] Capability classification in this PR is explicitly labeled:
  - [ ] Closes one or more items from the `closed` list (e.g., webhook signature verification)
  - [ ] Adds `experimental` capability (e.g., long-connection support in Phase B)
  - [ ] Adds `stub` capability with clear path to promotion
- [ ] No `stub` or `experimental` capabilities are written into "already completed" claims

---

## Architecture And Design Alignment

- [ ] All changes follow the closed-loop pattern from [2026-06-23-feishu-push-architecture.md](../2026-06-23-feishu-push-architecture.md)
- [ ] Core business model is unaffected; Feishu details remain in channel adapter or runtime layer
- [ ] RouteKey includes channel, conversation_id, peer_id, conversation_type
- [ ] No new dependencies on adapter or AI logic in core
- [ ] Multi-account isolation is enforced at path level (route key and persistence namespacing)

---

## Old Module Integration Checklist (AGENTS.md § 1.1)

List all existing modules that share Feishu capability scope and decide for each:

- [ ] **Feishu webhook ingress path**: 
  - Existing module(s) to check: `src/channels/feishu/channel.rs`, `src/infrastructure/runtime.rs`
  - Decision: ✓ already upgraded OR will be upgraded in a follow-up task OR explicitly not upgrading (with reason recorded)
  - Action recorded: _________________

- [ ] **Feishu outbound send path**:
  - Existing module(s): `src/channels/feishu/channel.rs`, `src/application/send_message.rs`
  - Decision: ✓ or upgrade plan or reason
  - Action recorded: _________________

- [ ] **Token and session state**:
  - Existing module(s): `src/adapters/sqlite_context_tokens.rs` or similar
  - Decision: ✓ or upgrade plan or reason
  - Action recorded: _________________

- [ ] **Audit and observability**:
  - Existing module(s): `src/adapters/sqlite_audit.rs`, `src/core/telemetry.rs`
  - Decision: ✓ or upgrade plan or reason
  - Action recorded: _________________

- [ ] No upgradeable existing module related to Feishu push was missed

---

## Implementation Completeness (AGENTS.md § 0.1 + 0.5)

- [ ] **Main closed loop exists and is documented**:
  - [ ] Entry point: _______________ (e.g., HTTP webhook or long-connection receiver)
  - [ ] Execution path through route/outbox/worker/send is clear
  - [ ] Exit point: _______________ (e.g., sent, retrying, or dead_letter)
  - [ ] Documented in code with inline comment or external doc reference

- [ ] **Runtime assembly is explicit**:
  - [ ] All new components are registered or composed in `AppRuntime::new` or delegated bootstrap module
  - [ ] No component is initialized but left unused (no underscore-prefixed orphan components)
  - [ ] Startup logs confirm all Feishu components are wired or explicitly disabled

- [ ] **Capability inventory is complete**:
  - [ ] New modules/functions in this PR are listed in a comment or table below
  - [ ] Each item is marked `closed`, `experimental`, or `stub`
  - [ ] Entry into "done" summary counts only `closed` items

---

## Model Change Synchronization (AGENTS.md § 0.5)

If this PR modifies core domain models or Feishu message structure:

- [ ] **Core model changes** (if any): _______________
- [ ] **Schema and adapter changes**:
  - [ ] SQLite schema updated (if inbox, outbox, or audit schema affected)
  - [ ] Serialization/deserialization in adapter layer matches new schema
- [ ] **Crash recovery and replay compatibility**:
  - [ ] Recovery process handles messages from prior schema version (if applicable)
  - [ ] DLQ replay uses compatible deserialization
- [ ] **Audit trail**:
  - [ ] Audit records capture message structure changes
- [ ] **Integration tests cover migration**:
  - [ ] At least one test exercises an older-version message through new code path

---

## Testing And Verification (AGENTS.md § 7)

- [ ] **Unit tests**: Line coverage ≥ 80% for new/modified code
  - Command: `cargo test --lib feishu`
  - Expected: All pass
  - Coverage: _______% (report screenshot or output)

- [ ] **Closed-loop integration test** (MUST have for Phase A):
  - [ ] Test exists and passes: _______________ (e.g., `tests/feishu_webhook_security_closed_loop.rs`)
  - [ ] Test proves main closed loop from entry to exit
  - [ ] Test is deterministic and repeatable (no flakes in 5 runs)

- [ ] **Regression tests**:
  - Command: `cargo test feishu_webhook_closed_loop`
  - Expected: Still passing ✓

- [ ] **Fallback and error path coverage**:
  - [ ] At least one test exercises failure scenario (e.g., 403 permission denied, signature mismatch, duplicate)
  - [ ] Test verifies failure path (e.g., outbox enters retrying or dead_letter)

- [ ] **Manual or staging verification** (if needed):
  - [ ] Tested against real Feishu platform (if applicable)
  - [ ] Tested with sample message payloads in docs/examples/
  - [ ] Staging checklist: _______________

---

## Process Gates (AGENTS.md § 1)

This PR progresses through the following gates:

### Step 4: Implementation (Code Quality)

- [ ] No `mock` or `#[ignore]` directives left in production code paths
- [ ] All external dependency failures have explicit handling (no `.unwrap()` on platform API responses)
- [ ] Lint passes: `cargo clippy --all-targets --all-features`
- [ ] Code follows existing style (imports, naming, module structure)

### Step 5: Review

- [ ] All review comments are recorded in this checklist or resolved GitHub comments
- [ ] Blocking review issues are resolved or escalated with explanation
- [ ] At least one reviewer approves the implementation

### Step 7: Fallback And Root-Cause Classification

If any test fails:
- [ ] Identify root cause (design, task split, implementation bug, or red-line violation)
- [ ] Roll back to appropriate step:
  - Design issue → architecture doc revision
  - Task split issue → plan revision
  - Implementation bug → this code iteration
  - Red-line violation → design review required before retry

---

## Feishu Production Red Lines (AGENTS.md § 2)

Specific to this PR, verify applicability:

- [ ] **Signature and token validation**:
  - Applicable: ✓ (if webhook ingress changes)
  - Implementation: `verify_webhook_signature` in `feishu/channel.rs`
  - Test: `tests/feishu_webhook_security_closed_loop.rs`

- [ ] **Dedup and ordering**:
  - Applicable: ✓ (if message ingress changes)
  - Policy: TTL cache (moka) if dedup scope, or explicit handling if sequence-based
  - Test coverage: _______________

- [ ] **Inbox and outbox state**:
  - Applicable: ✓ (if send changes)
  - Schema: SQLite `outbox` table with state machine (pending -> sending -> sent/retrying/dead_letter)
  - Test: `tests/feishu_media_retry_dlq_closed_loop.rs` or similar

- [ ] **Crash recovery**:
  - Applicable: ✓ (if outbox or storage changes)
  - Proof: Recovery process sets in-flight `sending` back to `pending`
  - Test: Add or reference recovery test

- [ ] **Multi-account isolation**:
  - Applicable: ✓ (if multi-account config is used)
  - Isolation: Route key and SQL queries scoped by channel and account_id
  - Test: `tests/feishu_multi_account_isolation_closed_loop.rs` or reference in this PR

- [ ] **Media streaming**:
  - Applicable: ✓ (if media upload is implemented)
  - Implementation: Streaming upload via `reqwest::multipart::Part::stream`
  - Test: Large file test (e.g., 10MB+) confirms no full-load into memory

---

## Security And Secrets (AGENTS.md § 2.4)

- [ ] **No hardcoded secrets or keys**:
  - [ ] App ID and secret are env-var only (no defaults in code)
  - [ ] Verification token and signing secret are env-var or config file only
  - [ ] No sensitive data in error messages or logs

- [ ] **Signature validation**:
  - [ ] X-Lark-Request-Timestamp, X-Lark-Request-Nonce, X-Lark-Signature headers are verified
  - [ ] Verification uses HMAC-SHA256 with signing secret
  - [ ] Test includes both valid and invalid signature cases

- [ ] **REST endpoint auth** (if applicable):
  - [ ] Feishu webhook endpoint has MAGICLAW_API_TOKEN check or IP allowlist
  - [ ] Unauthorized requests are rejected with 401 or 403 + audit record

---

## Observability And Audit (AGENTS.md § 2.6)

- [ ] **Audit logging**:
  - [ ] Ingress decision (webhook received, signature OK, parsed, deduplicated)
  - [ ] Send decision (outbox pending enqueued with reason)
  - [ ] Failure decision (rejected with code and description)
  - [ ] High-risk operations (token refresh, permission denied, retry threshold exceeded)

- [ ] **Audit records include**:
  - [ ] `route_key` (channel, conversation_id, peer_id)
  - [ ] `action` (webhook_ingress, send_queued, send_failed, retrying, dead_letter, replayed)
  - [ ] `detail` (error code, message count, attempt number)
  - [ ] Timestamp and no mutation guarantee

- [ ] **Telemetry**:
  - [ ] Counters: feishu_send_total, feishu_retry_total, feishu_dlq_total
  - [ ] Gauges: outbox_pending_count (by channel), token_refresh_failures (by account)
  - [ ] Latency: feishu_send_latency_ms (histogram)

---

## Phase Classification (Only For This PR)

- [ ] This PR closes one or more Phase A (Webhook-First) items: _______________
- [ ] This PR adds Phase B (Long-Connection) experimental: ___ (if applicable)
- [ ] This PR does NOT claim Phase C (Resilience) as closed unless resilience gates pass
- [ ] Phase A gate progression: (circle one)
  - [ ] Task 2 only (validation guardrails)
  - [ ] Tasks 2-3 (validation + ingress hardening)
  - [ ] Tasks 2-5 (validation + ingress + failure semantics + isolation)
  - [ ] Tasks 2-6 (validation + ingress + failure + isolation + audit)

---

## Commit Message Format

Use conventional commits:

```
feat(feishu): [Task N] <description>

Implements <task name> per plan 2026-06-23-feishu-push-plan.md.

Closes red line(s): <list or "none">
Capability changes: <closed | experimental | stub>
Audit compliance: <yes | needs follow-up task N>

Closes #<issue> (if applicable)
```

---

## Final Readiness Check

- [ ] This PR is a single coherent task (not combining unrelated changes)
- [ ] All checklist items above are marked ✓ or explicitly deferred to a follow-up task
- [ ] At least one reviewer has approved
- [ ] CI/CD pipeline passes
- [ ] Merge will not break Wechat or other channel functionality
- [ ] Documentation (code comments, architecture, plan) is up-to-date post-implementation

**Ready for merge**: ________ (date and reviewer name)

---

## Related Issues And References

- Design: [2026-06-23-feishu-push-architecture.md](../2026-06-23-feishu-push-architecture.md)
- Plan: [2026-06-23-feishu-push-plan.md](../2026-06-23-feishu-push-plan.md)
- Process rules: [AGENTS.md](../../AGENTS.md)
- Feishu setup guide: https://github.com/miaoxworld/OpenClawInstaller/blob/main/docs/feishu-setup.md
