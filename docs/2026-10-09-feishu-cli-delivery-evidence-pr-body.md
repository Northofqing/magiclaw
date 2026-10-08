Feishu authentication errors previously reached Stock as a generic nonzero CLI result, leaving a new notification Uncertain even when the message endpoint had never been called. The opt-in JSON v1 route reports typed first-auth rejection and binds each result to a fresh invocation, resolved account/app identity, target, receive-id type and exact text. Message-stage failures remain Uncertain, and the legacy CLI/daemon/media route keeps its behavior.

Validation: normal release producer/process plus the unchanged Stock production parser passed auth rejection, accepted receipt and message-stage Unknown loopback scenarios. Relevant CLI/channel/subprocess/media/isolation tests passed; measured new critical lines are 37/38 (97.37%), the new result and CLI binding lines are 100%. These scoped figures do not claim whole-platform coverage. No real platform message, production configuration or retained delivery decision was changed by validation.

Design, plan, four-role challenge, old-module/redline mapping and raw-check evidence are linked in `docs/2026-10-09-feishu-cli-delivery-evidence-{design,plan,challenge,validation}.md`.

- [x] One main loop: CLI parse/config → first auth → existing send → terminal JSON.
- [x] Capability classified: reviewed narrow CLI contract closed; activation pending.
- [x] Runtime wiring and system-level real subprocess test present.
- [x] Standards/Spec review: designated `/root/business_closeout_feishu_source_review` approved exact frozen source; zero unresolved P1/P2.
- [x] Existing module mapping checked; non-counted/daemon/media legacy remains unchanged for its documented scope.
- [x] Core model, schema, replay, recovery and audit fields unchanged; adapter evidence only.
- [x] No production mock, implicit fallback retry or historical adjudication.
- [x] Relevant regression, kill and account/channel isolation checks passed.
- [x] Actual affected unit coverage ≥80% and critical path ≥95%; unrelated platform scope explicitly unmeasured.
- [x] Rollback and paired opt-in deployment identity configuration documented.
- [ ] Paired activation recorded separately after install.
