# Candidate validation and integration checklist

Status: experimental isolated producer candidate; not a merge, activation, delivery adjudication, or client-receipt claim.

The candidate is based on committed magiclaw HEAD `35519bd`, in branch `codex/feishu-pre-send-evidence-20261009`. The original working directory's user change to `src/domain/services/audit_chain.rs` is outside this candidate. No production binary, account, bot, environment, database, daemon, or existing notification was changed.

## Actual checks

|Check|Evidence|
|---|---|
|Library CLI checks|10 passed, including nonce version/variant/canonical form and result phase/receipt invariants|
|Library Feishu checks|6 passed, including refusal of a stub receipt on the evidence route|
|Actual CLI subprocess loopback closed loop|6 passed on the final code, with synthetic credentials and empty isolated environment|
|Existing Feishu media retry/DLQ regression|1 passed|
|Existing multi-account isolation regression|5 passed|
|Legacy binary parser checks|3 passed before the final receipt/nonce validation change; that change does not affect their legacy path|
|Targeted lint|`cargo clippy --lib --bin magiclaw --test feishu_cli_delivery_evidence_closed_loop` passed without warnings|
|Formatting and patch whitespace|New result/fixture files passed `rustfmt --check`; edited existing sections were formatted without rewriting unrelated baseline formatting; `git diff --check` passed|

The real subprocess tests verify auth transport/exchange/parse/307-redirect failures with one auth request and zero messages requests; message transport/parse/business/missing-ID failures with one messages request and Unknown; accepted remote ID and actual body/target/type/account/app binding; exact legacy stdout; preflight Unknown with zero requests; isolated test configuration; pure JSON stdout and no credential/body/target leakage. The kill test holds the provider response during auth or messages, kills the real child, observes no terminal JSON, then verifies an independent fresh invocation can succeed. This preserves Unknown semantics after interruption; it does not create a recovery/replay owner.

The final noninstrumented debug candidate binary SHA-256 is `fcae4c785305482603726437c0e9fe17ad5438c255bbc9b9f94979100f2219fa`. This identifies the offline pairing fixture only; production still runs its original binary.

## Measured coverage and its limits

LLVM instrumentation includes the actual CLI subprocess using only `LLVM_PROFILE_FILE` forwarded from the test harness. The instrumented runs cover the same six subprocess scenarios, ten CLI library tests, and six Feishu library tests. Raw JSON, LCOV and a range summary are retained outside the repository under the private investigation directory; no coverage count is inferred from test counts.

|Explicit affected production scope|Covered instrumented lines|Percent|
|---|---|---|
|New result module, excluding its tests (`delivery_result.rs:1..117`)|69/69|100%|
|New auth evidence method, messages marker and legacy delegate (`channel.rs:687..689,725..767,816..822`)|37/38|97.37%|
|New flags/nonce validation and main binding/output (`parser.rs:75..88,146..159,167..168`; `main.rs:727..758`)|50/50|100%|

The one uncovered line is `channel.rs:745`, the HTTP client build failure returning Unavailable/Unknown. No production fault injection was added to force that platform setup failure. The preexisting whole files under these scoped runs are `channel.rs` 56.12%, `parser.rs` 60.04%, and `main.rs` 16.02%; unrelated media/webhook/other CLI/platform paths were not instrumented exhaustively. The affected new line metrics exceed 80%/95%, but they are **not** a claim that the entire core/platform gate passes. Branch coverage, arbitrary panic timing and real external provider availability were not measured.

## Gates and ownership

- [x] One explicit main loop from CLI parse through resolved config, first authentication, existing messages send and terminal JSON.
- [x] Adapter contract stays outside RouteKey/Message/Outbox and existing durable state models.
- [x] Failure, rollback, original modules and migration-redline impact documented in the design and plan.
- [x] Four agent perspectives recorded, with human authorization distinguished from proxy user review.
- [x] No production mock or fault injection, no implicit fallback, retry or historical rewrite.
- [x] Legacy Feishu/daemon/media paths continue using the existing send implementation without evidence flags.
- [x] Relevant unit, actual subprocess, regression, kill and isolation checks passed.
- [x] Actual affected coverage and unmeasured platform scope recorded.
- [ ] Independent final producer/source review recorded and all findings resolved.
- [ ] Producer/Stock consumer actual-process loopback fixture and strict schema alignment accepted.
- [ ] Final PR checklist and designated reviewer approval before any merge.
- [ ] Separately authorized production activation with reviewed executable hashes and expected nonsecret identity configuration.

Root owns consumer integration and any subsequent deployment. A source commit preserves a reviewable candidate; it is not approval to merge or activate. New evidence applies only to a new invocation. The existing 79 Uncertain results remain untouched.

## Final source and paired artifact review

Designated independent reviewer: `/root/business_closeout_feishu_source_review`. The frozen narrow source was approved with zero unresolved P1/P2. Review covers both Standards and Spec and the existing physical families, not client delivery or historical adjudication. Review JSON SHA-256: `7f40d4520ccea9f6fe94044c13ef72d3580440311248824730002c3dc9aef6bc`.

The normal release producer SHA-256 is `eb57e1991357f6cdfb4c865cefd872917f7d9fbe02b51fe5333087b5176ef24e`. It was executed against loopback with synthetic credentials and the unchanged Stock production parser (SHA-256 `af5b3bc05d97f357fcd8c84c5eb82672b68e1a4f3ee1f676235b8f3fedcf9200`), not a hand-written schema approximation. All three scenarios passed: first-auth rejection with zero messages requests, accepted with one messages request, and post-message Unknown with one messages request. Each invocation had a fresh UUIDv4. The complete monitor helper mapping is separately exercised by its 16 consumer and 8 retained-family tests; the standalone pairing harness directly includes the production parser and argument builder rather than linking the entire monitor. No real platform request or production database mutation occurred. Pair receipt SHA-256: `9f3abd875ddffae1e5c4820a38c799d4d7d2f10830719fcc1a617e05c1274e5a`.

- [x] Independent final source review, all feedback resolved.
- [x] Actual normal producer/process and Stock consumer parser pairing.
- [x] Narrow source approval by the designated reviewer recorded.
- [ ] PR created and exact checklist linked.
- [ ] Merge and paired production activation, recorded separately after execution.

This closes the reviewed CLI contract loop only. It does not claim whole-platform coverage, old uncertain outcomes, natural client receipt, or healthy market/account inputs. Root performed an additional Standards check of the unchanged core models, legacy delegate, nonsecret config binding and deployment rollback boundaries.
