# SPEC-P4b: Normative spec for groups, SCIM, and approval park-and-resume with Web Push

## Goal
Second phase-4 spec batch (owner, Q10, 2026-11-09: every phase-4 item, in the proposed order). Write the **normative spec** so implementation can be briefed from it. Specification only; no code. Same shape and rules as `docs/tasks/SPEC-P4a.md` (read it and how its items landed: `git log --oneline --grep 'SPEC-P4a'`).

Items (one commit each, prefix `SPEC-P4b: <item>`):
1. **P4-GRP-1 — groups (manual).** Directory groups per organization; membership; **group → workspace access bindings** (a group can hold the same workspace roles a user can, with the effective role being the strongest of direct and group grants); **group identities in policy** (`{ "kind": "group" }`; the evaluator's principal set gains the speaker's group ids, consistent with the `user` identity binding of RFE-005/SEC-44); **group → org-role mappings**. Specify the tables, routes, permission-matrix rows (who manages groups and bindings), how access is recomputed and invalidated (gateway caches, sessions, running turns), audit, the dashboard (org Groups page, workspace access showing group-derived members), limits, and tests. Source: `21 §Phase 4 — Groups and SCIM`, `04` (roles, access law), `07` (identities).
2. **P4-GRP-2 — SCIM 2.0** at `/scim/v2` (Users and Groups resources, the subset real IdPs use: Entra ID, Okta, Keycloak): bearer tokens stored hashed (new prefix, per org), provisioning semantics (create/patch/deactivate users; groups and membership feed P4-GRP-1), how SCIM users relate to OIDC sign-in (link by verified e-mail / `externalId`, never by unverified data; D-118 trusted domains), deprovisioning effects (sessions, tokens, running work, owned agents), pagination/filter subset, errors in SCIM format, rate limits, tests against a conformance client or recorded IdP traffic.
3. **RFE-011b — approval park-and-resume and Web Push** (`80 §RFE-011` Decision; `07` and `14` already mention it as phase 4). Park-and-resume: for deadlines longer than a gateway hold can reasonably keep a connection, the turn parks (the request is refused to the harness in a resumable way, or the turn is suspended) and resumes on approval; specify which approvals park (per-rule/grant timeout above a threshold), the harness-facing behaviour, what is persisted, sandbox lifecycle while parked, resume after approval/denial/expiry, interaction with RFE-011a deadlines, freezes (RFE-006), and channel cards. Web Push: VAPID keys (generation, storage, rotation), subscription routes, the service worker, what a push contains (no secrets, no request bodies), per-user opt-in, delivery when no SSE surface is live, and tests.

## Rules
As SPEC-P4a: phase-tag "(phase 4, P4-xxx / RFE-011b)"; update every owning doc (01, 02 §Limits, 03 §Planned changes with level-4 headings, 04 permission matrix, 05, 06, 07, 08, 10, 11/12, 14, 15, 16, 17 SEC + residual risks, 18, 21); no future columns in the machine-checked tables; the most secure reasonable default for open choices, listed under "Choices made"; ask through Orca `ask` for consequential choices (a security property change, required new infrastructure, a breaking change). New behaviour is opt-in or invisible until used; existing configs keep working.

## Numbers reserved
- Decisions **D-130..D-135**; SEC invariants **SEC-71..SEC-76** (in `17` and new `docs/tests/security-coverage.md` rows, status `pending`); contracts **C-460..C-499** are reserved for implementation (do not write `API_CONTRACTS.md`, but name routes and shapes in `05`). New token prefix: pick one not in `00`'s table and add it there.

## Constraints
- You own `docs/0*.md`, `docs/1*.md`, `docs/21-later-phases.md`, `docs/90-decisions.md` (D-130..D-135), `docs/tests/security-coverage.md` (new rows only). Don't edit `docs/80-design-review.md`, `docs/API_CONTRACTS.md`, `docs/PROGRESS.md`, `docs/FEATURES.md`, `docs/operator/**`, `docs/audit/**`, or code.
- Other lanes running: AUDIT-003 (e2e; owns `docs/audit/**`), P4-MCON-1 (code). Don't run `scripts/e2e/*` or `scripts/perf/*`.
- Change files only with the Edit/Write tools; never python, sed, awk, perl, `cat >>` or heredocs.

## Acceptance
- One commit per item; `bash scripts/guards/run-all.sh`, `python3 scripts/extract-schema.py --check`, `cargo test -p aurora-db --test it schema` pass.
- Report in the `worker_done` message itself (not a file): per item every doc and section changed, "Choices made", open questions.
