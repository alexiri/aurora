# P4-NARR: live narration cards in chat

## Goal
On a chat platform whose connector can edit messages, an agent with `narration_cards` on shows its turn's progress as one card: the tool steps and their status, updated as they run and removed when the answer posts. The card is built only from durable `tool.started` and `tool.finished` events, shows no arguments, secrets or answer text, never notifies or links, and stays within the connector's write budget. The dashboard chat is unchanged.

## Linked requirements
- D-225 (moved to phase 5, needs a spec task); D-279 (card as the turn's receipt, events, no new op kind, dashboard unchanged); D-280 (shared-space names only, bounds and cadence); D-286 (owner: the card is collapsed at completion, not deleted).
- Contracts C-846 (agent setting), C-847 (`narration` content), C-848 (card lifecycle on existing ops), C-849 (cadence and budget), C-850 (events and privacy), C-851 (Mattermost rendering), C-852 (dashboard switch) in `docs/API_CONTRACTS.md` §Phase 5 — P4-NARR / P4-UTOK.
- SEC-111 (`docs/17-security.md`): no argument, secret, file content or answer text in a card; no ping or link; a failed card write never fails a turn.
- Spec sections: `docs/10-agents-conversations.md` §Live narration cards; `docs/13-channels-connectors.md` §4.5, §5.2, §5.10, §5.12; `docs/20-mattermost-connector.md` §Operations, §Rendering; `docs/18-testing.md` §Live narration cards; `docs/02-engineering-conventions.md` §Limits (`NARR_*`); `docs/03-data-model.md` §Planned changes (`agents.narration_cards`, migration `20261201000231`).
- FIX-142 (D-226, D-227, D-270, D-271): every ping and link rule applies to card text.

## Context
- Receipt and answer path: `services/aurora-api/src/domains/channels/mirror.rs` (`on_turn_finished`, `mirror`, `post_answer`, `clear_receipt`, receipt op key `receipt:<event_id>`). The receipt the inbox posts is in `services/aurora-api/src/domains/channels/inbox/{send,message,events}.rs`.
- Tool events: their producer is the turn-event ingest; `services/aurora-api/src/domains/hosted/reports.rs` is where `tool.started`/`tool.finished` enter. Confirm the exact path before coding.
- Ops: `crates/aurora-db/src/repo/channel_ops.rs` (`insert`, `by_dedupe_key`); the op kinds `message.post`, `message.update`, `message.delete` exist (`docs/13 §4.2`).
- Capabilities: `crates/aurora-channels/src/capability.rs` (`edits`); the connector's fallbacks in `docs/13 §4.5`.
- Mattermost connector: `connectors/mattermost/src/{ops.rs, render.rs, convert.rs, mentions.rs}`; the update path `PUT …/posts/{id}/patch` and delete are already implemented for the answer's `message.update`/`delete` ops (verify in `ops.rs`).
- Agent settings: `services/aurora-api/src/domains/agents/{dto.rs, service.rs, routes.rs}`, repo `crates/aurora-db/src/repo/agents.rs`, migrations under `crates/aurora-db/migrations/` (last existing `20261201000180`; this task takes `20261201000231`).
- Limits: `aurora-core::limits` (`crates/aurora-core`), registry rows added to `docs/02`; `scripts/guards/limits-gaps.allow` lists the `NARR_*` names as `doc-only` until this task removes them.
- Dashboard: the agent's Channels page (Triggers section) in `web/src/…` (locate by the existing Channels route; verify before editing). Shared primitives only (`docs/14` §Visual design).

## Scope
### In scope
**Part A — API (Rust, Sonnet)**
- Migration `20261201000231_agent_narration_cards.sql`: `agents.narration_cards boolean not null default false`; repo, DTO, `PATCH /v1/agents/{agent}` (C-846).
- Projector (new module under `services/aurora-api/src/domains/channels/`, e.g. `narration.rs`): reads only the four event kinds of C-850, keeps per-turn state, coalesces, emits `message.post` / `message.update` (the last one the collapse, D-286; no `message.delete`) with `narr:<turn_id>` / `narr:<turn_id>:<seq>` / `narr:<turn_id>:end` dedupe keys (C-848). Replaces the receipt when the setting is on and the connector declares `edits`; otherwise the receipt path is untouched.
- Cadence and budget (C-849): `NARR_MIN_INTERVAL`, `NARR_SPACE_UPDATES_PER_MINUTE`, `NARR_CARDS_PER_SPACE`; per-space counter in the existing shared rate-limit primitive if it fits (`docs/03` and `docs/05` §Shared rate-limit counters, P4-CNT, D-119), otherwise a bounded in-process counter with the same cap per replica (document the choice).
- Limits `NARR_MAX_STEPS` (12) and `NARR_STEP_CHARS` (120) in `aurora-core::limits`; remove their `doc-only` lines from `scripts/guards/limits-gaps.allow`.
- `narration` content variant (C-847) in the neutral content type used by the connector contract (`crates/aurora-connector-sdk` or the wire type that carries `Content`; verify).

**Part B — connector (Rust, Sonnet)**
- `connectors/mattermost/src/render.rs`: card rendering per C-851 (header, one line per step, code spans, `N earlier`, bare-URL wrapping, defuse per D-271).
- Conformance: `tests/conformance/connector/` scenario for a card's create, update, collapse.

**Part C — dashboard (Next.js, nextjs-developer)**
- Channels page switch "Show progress in chat" bound to `narration_cards` (C-852), help text as in `docs/14`, off by default, inline error on failure. Vitest + Playwright at 390 px in both themes.

**Part D — docs**: keep `docs/10`, `13`, `20`, `14`, `02`, `03`, `05`, `API_CONTRACTS`, `18` matching what lands; regenerate OpenAPI if the repo does so.

### Out of scope
Streaming the answer as edits; narration in the dashboard chat; tool arguments or result previews of any kind; per-tool configuration; non-chat connectors; Slack; a card for `send_message` or approvals; any change to `reactions`-based receipts when narration is off.

## Constraints / file ownership
- Owns: `services/aurora-api/src/domains/channels/narration.rs` (new), `services/aurora-api/src/domains/channels/mirror.rs` (receipt replacement branch only), `services/aurora-api/src/domains/agents/**` (the setting only), `crates/aurora-db/src/repo/agents.rs`, migration `20261201000231`, `crates/aurora-core` (limits), `connectors/mattermost/src/{render.rs, ops.rs}` (card path only), `tests/conformance/connector/**`, the Channels settings component, `scripts/guards/limits-gaps.allow` (only the `NARR_*` lines), `docs/02`, `docs/03`, `docs/05`, `docs/API_CONTRACTS.md`, `docs/18` (the NARR sections only).
- Does not touch: `aurora-channels` capability semantics, the inbox receipt for turns with narration off, the answer rule, approvals, user tokens (P4-UTOK owns `send_as_me`, `user_tokens`, `user_credential`).
- The card never carries `detail` outside a `direct` conversation (C-850); the check lives in the projector, not in the connector.
- No new op kind; no new capability.
- Lanes: `docs/tasks/LANE_RULES.md` gates per task. Opus reviews the privacy filter in `narration.rs` before merge (security-adjacent).

## Acceptance criteria (failing-first: each new test fails before its code)
1. API ITs (`services/aurora-api/tests/it/narration.rs`): one card per turn in place of the receipt; steps added and updated from `tool.started`/`tool.finished`; `NARR_MAX_STEPS` cut with "N earlier"; collapse (D-286) at answer post, failed turn and stalled turn, the closing update retried after `rate_limited`; a restart mid-turn resumes the same card (no second `narr:<turn_id>` post).
2. Privacy IT: no `activity`, `thinking.delta` or `text.delta` content in any card write; a shared-space card carries names only; a `direct` card carries the sanitized summary; a canary argument and a canary secret appear in no card write and in no connector request log.
3. Budget IT: bursts coalesce to at most one update per `NARR_MIN_INTERVAL`; two concurrent turns in one space cap updates at `NARR_SPACE_UPDATES_PER_MINUTE`; a third turn in a full space gets no card and its answer is unchanged; `rate_limited` drops the update and is not retried.
4. Connector test (Mattermost, mock server then the pinned dev server, `docs/20`): card with `@here`, `@all`, `<@user:…>`, a group mention and a bare URL in a label produces no notification and no preview; the card fits `message_max_chars` at the cap.
5. Degradation IT: a connector without `edits` gets no card and the existing receipt; `forbidden` on a card write leaves the turn running and the answer posted.
6. Migration IT: existing agents read `narration_cards = false`; the column is in `docs/03` (`scripts/extract-schema.py --check` passes after the table moves from §Planned changes).
7. Web: vitest for the switch (default off, PATCH on change, inline error); Playwright at 390 px, both themes, no horizontal scroll.
8. `bash scripts/guards/run-all.sh` passes, with every `NARR_*` line removed from `limits-gaps.allow`.

## Verification
Per-task gates of `docs/tasks/LANE_RULES.md`. Rust: `cargo fmt --check`, `cargo nextest run -p aurora-api -p aurora-connector-mattermost` (targeted), `cargo clippy --workspace --all-targets --all-features -- -D warnings` at the batch gate. Web: lint, typecheck, vitest, production build. Guards: `bash scripts/guards/run-all.sh`. Live Mattermost check (C-851) runs when the machine is idle (perf and live suites are not run during other builds).

## Dependencies
- Blocked by: none technical (FIX-142 merged). Schedule with the phase-5 lane plan (`PHASE5-PLAN.md`), since Part A touches `channels/mirror.rs`.
- Blocked on the owner: Q-NARR-1 to Q-NARR-6 (the recommended answers are the spec as written; the lane may start Parts A and B on them, and the owner can flip any answer before merge).
- Unblocks: none.

## Suggested agent/model and reason
`rust-developer` (Sonnet) for Parts A and B (multi-file, cross-module: channels, agents, connector, core limits), `nextjs-developer` (Sonnet) for Part C. Opus reviews `narration.rs`'s privacy filter and the cadence logic before merge (D-280). Size **M**.

## Open questions for the owner
Each has the recommended answer; the spec text follows it, marked "(pending owner answer Q-NARR-n)".

- **Q-NARR-1 — Default and level of the setting.** Recommended: off by default, set per agent on the agent's Channels page. Alternative: on for every agent of a workspace or of the instance.
- **Q-NARR-2 — What a shared space sees.** Recommended: step names only in any conversation that is not `direct`; the sanitized summary only in a `direct` conversation. Alternative: sanitized summaries everywhere (more useful, puts argument-bearing text in shared channels).
- **Q-NARR-3 — Cadence and budget.** Recommended: one update per turn at least every 5 s; at most 10 narration writes per space per minute; at most 3 cards per space at a time. Alternative: faster updates, a larger share of the connector's 30 writes per minute.
- **Q-NARR-4 — After completion. Answered (owner, 2026-11-15, D-286): collapsed, not deleted** — the card's last update shrinks it to one line (outcome, step count, elapsed time). Original recommendation: the card is deleted when the answer posts, as the receipt is today. Alternative: the card stays as a settled summary ("Done, 4 steps").
- **Q-NARR-5 — Platforms without message edits.** Recommended: no card; the receipt stays as today. Alternative: one "Working on it" post with no updates and no step list.
- **Q-NARR-6 — Which events feed the card.** Recommended: tool steps only (`tool.started`, `tool.finished`). Alternative: also the sanitized `activity` lines, which are free-text status lines and would widen what reaches shared spaces (not recommended).

## Completion report
- Files changed:
- Tests/checks (exact commands and results):
- Decisions taken on open questions:
- Remaining risks:
