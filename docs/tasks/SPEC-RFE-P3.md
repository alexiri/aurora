# SPEC-RFE-P3: Spec updates for the design-review items of phase 3

## Goal
Write the owner's decisions in `docs/80-design-review.md` into the normative spec for every item scheduled in **phase 3** (RFE-013, 014, 018, 019, 020, 022, 023, 024). The result is specification only. No code changes. Implementation is briefed later from these spec sections.

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
- **Decisions log**: add owner decisions D-59 onward in `docs/90-decisions.md`, one per RFE, referencing `80` and the phase.
- **Limits** (new constants such as rate-limit buckets, retention defaults and approval-timeout bounds): add them to the limits table in the doc that owns limits (`02 §Limits`), phase-tagged.

## Per-item notes (hints, not replacements for the Decision)
- RFE-013: optional per-instance and per-org maximum age for BYO agent tokens (`aug_`), default none; expiry shown in the dashboard; `last_used_at` surfacing of stale tokens; regenerate flow unchanged.
- RFE-014 (explicitly phase 3): gateway under Postgres unavailability: (a) serve resolution-cache entries within TTL, refuse on miss (fail closed for new state); (b) on LISTEN loss reconnect first, then flush; (c) a metric and a readyz distinction for "degraded, serving from cache" vs "down".
- RFE-018: optional client idempotency key on `POST /v1/conversations/{id}/messages`, reusing `source_event_id` and its partial unique index; same response on replay.
- RFE-019: spec default `agents.default_brief` becomes 2-3 neutral sentences; the Daneel persona moves to this deployment's `aurora.toml` (show it as an example); base image names a public base (AlmaLinux official) with the CERN registry as a configured override.
- RFE-020: workspace-tier skills (nullable `workspace_id` on `skills`), projection precedence agent > workspace > org, same shadowing rule; routes, permissions (`can_own` to edit workspace skills), UI.
- RFE-022: on activation of a plugin major upgrade, connections whose stored credential keys no longer satisfy the new required fields become `reauth_required` with a reason naming the version change; the admin approval screen warns "N connections will need reconnecting".
- RFE-023: one paragraph in 07 explaining the LLM-host carve-out of the credentialed Default Rule and pointing to 08 §11; note that explicit block rules naming an LLM host still win; dashboard Default Rule UI note.
- RFE-024: deprovision ops are self-contained (payload snapshots platform_user_ref and the credentials needed, sealed), committed in the same transaction as the delete before the cascade; or the item states where deprovision is skipped and orphaned bots are a documented residual.

## Deliverable
- One commit per RFE on your branch (prefix `SPEC-RFE-P3: RFE-nnn`), so the coordinator can review each.
- A final report listing every doc and section changed per RFE, the "Choices made", and any open questions.
- `bash scripts/guards/run-all.sh` and `cargo test -p aurora-db --test schema_matches_spec` still pass.

## Constraints
- You own `docs/0*.md`, `docs/1*.md`, `docs/20-mattermost-connector.md`, `docs/MASTER_SPEC.md` and `docs/90-decisions.md` (D-59+ only).
- Don't edit `docs/80-design-review.md` (the owner's document), `docs/API_CONTRACTS.md` (contracts are written per phase at implementation time), `docs/PROGRESS.md` or `docs/FEATURES.md`, or any code.
- AUDIT-001 runs in parallel and only edits `docs/operator/**` and `docs/audit/**`.

## Suggested agent/model
Sonnet (careful spec writing; long task); the coordinator (Opus) reviews every commit.
