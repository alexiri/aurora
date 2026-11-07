# SPEC-RFE-P12: Spec updates for the design-review items of phases 1 and 2

## Goal
Write the owner's decisions in `docs/80-design-review.md` into the normative spec for every item scheduled in **phase 1** (RFE-001, 002, 003, 004, 009, 016) and **phase 2** (RFE-005, 006, 007, 008, 010, 011a, 012, 015, 017, 021). The result is specification only. No code changes. Implementation is briefed later from these spec sections.

## Rules
- **Phase-tag every change**: start each changed or new rule with "(phase N, RFE-nnn)". Where a phase-0 rule is replaced, keep it readable as phase-0 behaviour and state the new rule as superseding it from phase N. Example: "Phase 0: email+password and OIDC. From phase 1 (RFE-004): OIDC only …".
- **Respect the Decision text exactly.** Where the Decision is terse ("Implement option a"), specify that option fully, with concrete behaviour, data, routes, errors, limits, UI and tests in the relevant doc sections.
  - Where a design choice is still open after reading the option text, pick the most secure reasonable default, write it down, and list it under "Choices made" in your report.
  - If a choice is consequential, e.g. it changes a security property beyond the decision, ask through Orca `ask` instead.
- **Update every owning doc**, not just one:
  - 01 trust boundaries, 03 data model, 04 identity/authorization (incl. permission matrix), 05 routes/jobs, 06 gateway, 07 policy/approvals
  - 08 secrets/credentials/keys, 10 agents/conversations, 11/12 runner/supervisor, 13 channels, 14 dashboard, 15 deployment/config (config keys table)
  - 16 wire protocols, 17 security (adjust or add SEC invariants and the residual-risks section), 18 testing (what proves each change)
  - 20 Mattermost where RFE-001 touches it; MASTER_SPEC summary lines if any contradict
- **Data model (`docs/03`)**: phase-0 tables are machine-checked against the migrations by `scripts/extract-schema.py` (level-3 headings ``### `table` `` with a `| Column | Type | Notes |` header).
  - Do **not** add future columns or tables to those tables.
  - Add a section `## Planned changes (later phases)` at the end of 03, using **level-4** headings and a `| Planned column | Type | Notes | Phase |` header, so the extractor ignores it.
  - Verify with `python3 scripts/extract-schema.py docs/03-data-model.md` and `cargo test -p aurora-db --test schema_matches_spec` that the extract is unchanged.
- **Decisions log**: add owner decisions D-43 onward in `docs/90-decisions.md`, one per RFE, referencing `80` and the phase.
- **Limits** (new constants such as rate-limit buckets, retention defaults and approval-timeout bounds): add them to the limits table in the doc that owns limits (`02 §Limits`), phase-tagged.

## Per-item notes (hints, not replacements for the Decision)
- RFE-001: an org-admin integration setting (e.g. `channel_integrations.config.verified_email_autolink`, default false). When it is false, the sign-in link flow (option b) applies, with the platform email only pre-filling. Update 13 §5.1 step 4, §5.9, SEC-19/20, 17 §Residual risks and 20.
- RFE-002: TLS-only proxy listener on 4102 for in-cluster and external clients alike.
  - Specify the server certificate source. Recommended: issued by the existing gateway CA for the advertised proxy host(s) and in-cluster names, since sandboxes and BYO container config already trust that CA (`ca.pem`); an operator-supplied cert is optional.
  - `HTTPS_PROXY=https://…` everywhere; the supervisor loopback forwarder connects over TLS.
  - Client caveats for BYO tooling.
  - Remove plaintext from every doc (06 §Listeners, 08 §10, 11 boot env, 12 loopback proxies, 15 networking/Helm/compose).
- RFE-003:
  - (a) an open-lane posture `enabled | allowlist_only | disabled` in governance (instance/org) and workspace policy, tighten-only
  - (b) CONNECT-time evaluation: restrictive rules treat unknowable parts (path/method/body) as matched; allow rules need a full match on what is knowable
  - (c) the supervisor starts 3129 only when the sandbox has the browser capability (define where that capability comes from)
- RFE-004: OIDC-only sign-in from phase 1:
  - remove password sign-up/sign-in/reset/verify from the user-facing spec (phase-tagged)
  - `bootstrap-admin` and `auth.instance_admins` work through OIDC identities
  - API keys and CLI device login stay
  - specify the development and e2e story: a dev IdP, the existing mock IdP in compose/dev and tests
  - `signup_mode` semantics under OIDC (JIT)
- RFE-009: direct conversations visible only to their user and workspace owners (`can_own`); everything that names them follows suit (approval cards, activity, request log rows, search, SSE). Update the 04 permission matrix and 05 routes.
- RFE-016: a `DeltaBus` trait seam (phase-1 implementation: NOTIFY) and a back-of-envelope ceiling in 15 §Sizing.
- RFE-005: `user` identities bind to the turn's speaker when a context credential is present. With boot/agent-token traffic (no speaker), `user` rules **never match**, which is fail-closed; say so.
- RFE-006: freeze for agent, workspace and org:
  - state columns, routes, effects (gateway refuses credentials, due-work skips, webhooks 410, schedules skip, presences reply "paused", sandbox parked)
  - permissions: agent → `can_own`, workspace → `can_own`/org admin, org → instance admin or org owner; pick and justify
  - audited and reversible
- RFE-007: keyring-style lists (`key_id:value`, active key) for `AURORA_SIGNING_KEY` and `AURORA_INTERNAL_SECRET`, with a rotation procedure.
- RFE-008: name the security-critical audit set in 04 (at least: keys, grants, policy publish/rollback, plugin/connector/trusted-key/host-claim decisions, approval decisions, role/membership changes, freeze, governance changes). Those rows are written in the same transaction as the change.
- RFE-010:
  - conversation archive and delete (authorization rules)
  - transcript event redaction, mirroring memory redaction (audited)
  - optional per-org retention for conversations and attachments (daily job)
  - JSON export
- RFE-011a:
  - the stall-exemption fix: read `80` and the current approval-hold/stall rules in 06/07/10 to define precisely what "stall exemption" fixes; ask if unclear
  - per-rule/per-grant `approval_timeout` bounded by an instance max
  - email to workspace owners/approvers when no live delivery surface exists
  - SSE-driven browser notification in the dashboard
- RFE-012: workspace role `approver` between `member` and `owner`.
  - Plain members chat but don't decide approvals or action approvals.
  - Approvers decide them; owners can too.
  - Update the access law in 04 (`can_approve`), the permission matrix, cards/clicker rules in 13 §5.5, and invitations/bindings.
- RFE-015: per-org attachment retention window and storage quota (422 when over), an operator storage breakdown, and a sweep job. Joined with RFE-010.
- RFE-017: a per-principal token bucket on the API, with stricter buckets for simulate, uploads and search; 429 + `retry-after` in 05 §Conventions.
- RFE-021:
  - (a) `can_own` may reassign `creator_user_id` (audited)
  - (b) an offboarding view listing the automations a removal would disable
  - (c) workspace-anchored automations (authority = "an active owner exists")

## Deliverable
- One commit per RFE on your branch (prefix `SPEC-RFE-P12: RFE-nnn`), so the coordinator can review each.
- A final report listing every doc and section changed per RFE, the "Choices made", and any open questions.
- `bash scripts/guards/run-all.sh` and `cargo test -p aurora-db --test schema_matches_spec` still pass.

## Constraints
- You own `docs/0*.md`, `docs/1*.md`, `docs/20-mattermost-connector.md`, `docs/MASTER_SPEC.md` and `docs/90-decisions.md` (D-43+ only).
- Don't edit `docs/80-design-review.md` (the owner's document), `docs/API_CONTRACTS.md` (contracts are written per phase at implementation time), `docs/PROGRESS.md` or `docs/FEATURES.md`, or any code.
- AUDIT-001 runs in parallel and only edits `docs/operator/**` and `docs/audit/**`.

## Suggested agent/model
Sonnet (careful spec writing; long task); the coordinator (Opus) reviews every commit.
