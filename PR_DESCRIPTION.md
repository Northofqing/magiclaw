# PR: Fix WeChat login popup and stabilize daemon send flow

## Summary

Align WeChat QR-login state machine with reference implementation and fix daemon bootstrap to support token recovery from inbound messages. Adds macOS popup window for QR code display.

**Status:** ✅ All stages verified end-to-end (login → daemon → send)

---

## Changes

### 1. QR Login State Machine (Phase 1.5 Red Line)
- **File:** `src/main.rs`
- **Changes:**
  - Align status handling to `wait` → `scaned` → `confirmed` → `expired` state machine
  - Add fields extraction: `ilink_bot_id`, `ilink_user_id` from API response
  - Save complete `account.json` with `savedAt` timestamp upon `confirmed` state
  - Add macOS popup via `qlmanage -p` for QR display (auto-close on confirmed/expired/timeout)

### 2. iLink Protocol Alignment
- **File:** `src/channels/wechat/ilink.rs`
- **Changes:**
  - Add response fields: `ilink_bot_id`, `ilink_user_id` to `ILinkQrcodeStatusResponse`
  - Add `iLink-App-ClientVersion: 1` header to `get_qrcode_status` requests (contract alignment)

### 3. Daemon Bootstrap Fix
- **File:** `src/channels/wechat/ilink.rs`
- **Changes:**
  - Remove `context_token` non-empty requirement from `ILinkSendConfig::from_wechat_config()`
  - Allow channel to start without pre-existing token, enabling long-poll to acquire token on first inbound message

### 4. Documentation
- **New files:**
  - `docs/wechat-send-stability-design.md` — Architecture & recovery design
  - `docs/wechat-send-stability-plan.md` — Implementation breakdown
  - `docs/wechat-send-stability-log.md` — Acceptance test results

---

## Verification

### Phase 1.5 Acceptance Tests ✅

| Test | Result | Details |
|------|--------|---------|
| QR login flow | ✅ PASS | Confirmed state triggers file save, closes popup |
| Account.json persistence | ✅ PASS | Correct fields saved: token, baseUrl, accountId, userId, savedAt |
| Daemon bootstrap | ✅ PASS | WeChat channel shows "ilink enabled" not "skeleton" |
| Window status endpoint | ✅ PASS | `/api/window_status` returns active peer after inbound message |
| Send endpoint | ✅ PASS | `/api/send` returns `ok:true` with refreshed context_token |
| Stability (10 iterations) | ✅ PASS | 100% success rate, avg 250ms, min 171ms, max 284ms |

### Manual Testing
- QR popup displays on login, closes after confirmation
- Timeout and expiration paths close popup correctly
- No regressions to existing send/auth CLI commands

---

## Alignment with Red Lines

- ✅ **RouteKey includes conversation dimension** — Maintained in routing
- ✅ **Same RouteKey serial, different parallel** — Route worker pool unchanged
- ✅ **30-min idle route GC** — Maintained
- ✅ **Dedup uses TTL cache** — Maintained
- ✅ **Outbox state machine** — Maintained
- ✅ **MCP stdio zero-pollution** — No changes to MCP layer
- ✅ **ilink contract tests** — Added `iLink-App-ClientVersion` header

---

## Checklist

- [x] Code compiles without warnings
- [x] Unit/integration tests pass (all existing tests maintained)
- [x] Acceptance tests pass (see Verification section)
- [x] No mock residue in production paths
- [x] Explicit error handling per data source
- [x] Design doc written and committed
- [x] Old-module integration check completed (no deprecated modules touched)
- [x] Capability levels marked: send flow = `closed`, popup = `closed`

---

## Files Modified

- `src/main.rs` — QR login state machine + popup integration
- `src/channels/wechat/ilink.rs` — iLink response fields + header alignment + bootstrap fix
- `Cargo.toml`, `Cargo.lock` — Dependency lock update
- `docs/*.md` — Design, plan, and test results (3 new files)

---

## Next Steps

- [ ] Code review approval
- [ ] Merge to main
- [ ] Verify daemon stay stable under 24hr production load
