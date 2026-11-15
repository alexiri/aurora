# P5-MODELS-2: Usage per model, including subagents, and the request log's model (owner, 2026-11-15)

## Goal

Record token usage per model for every turn. This includes **subagent tokens**, which
`claude-code` turns leave out today. Use a trust rule that keeps the harness from misattributing
cost. The gateway records which model each LLM request asked for. This task also fixes an existing
under-count (D-302(f): built in phase 5).

## Linked requirements

- D-304; SEC-123.
- Contracts **C-879, C-880**.
- Normative: `10 §Usage and metering`; `08` (*Error classification and usage*, `§11.5`); `12 §Adapter
  claude-code` (*Usage*), `§Adapter codex` (*Models*); `16` (`turn.result`); `03` (`turns.usage`,
  `request_log.llm_model`); `14` (Activity); `02 §Limits` (`USAGE_BY_MODEL_MAX`); `18 §Model roles and
  usage per model` (*Usage*, *Gateway*).

## Context

- The adapter builds `Done { usage }` from the result's `usage`
  (`services/aurora-supervisor/src/adapters/claude_code/stream.rs`). According to Claude Code's docs,
  that field covers **the main loop only**. The result's per-model report (`modelUsage` in the SDK; the
  key in the raw stream-json output is to be confirmed against the pinned 2.1.319) is keyed by model id
  and includes subagents. Its entries carry `inputTokens`, `outputTokens`, `cacheReadInputTokens`,
  `cacheCreationInputTokens` and `costUSD`.
- The API stores usage through `hosted::set_usage` (`services/aurora-api/src/domains/hosted/`, called
  from `reports.rs`) and stamps `provider`, `endpoint_id` and `model` from `ModelChoice` (P4-LLM).
- The gateway already computes `model_from_request` when an allow-list applies
  (`crates/aurora-llm/src/model.rs`, `services/aurora-gateway/src/decide.rs`), and writes request-log
  rows in `services/aurora-gateway/src/reqlog.rs`.

## Scope

1. **Adapter, `claude-code`**: parse the per-model report from the `result` event. Pin the key, and
   add a fixture captured from the pinned binary with a subagent turn to
   `adapters/claude_code/fixtures/`. Map it to `by_model` (C-879), make the totals the sum, and drop
   `costUSD` and `total_cost_usd`. Fall back to `usage` when the report is absent. Streaming `Usage`
   events are unchanged.
2. **Adapter, `codex`**: one `by_model` entry for the default model, unless the pinned version reports
   per-model counts.
3. **Wire**: the `usage` of `turn.result` may carry `by_model`. It is already JSON, so check the
   runner passthrough and add a test.
4. **API** (`set_usage` and its caller): the trust rule of `08`:
   - The allowed set is the turn's default model, the resolved roles of its `ModelChoice` (stored with
     the turn, or recomputable from the stored `ModelChoice`), and the effective allow-list at turn
     end. Any pattern-valid id is allowed when no allow-list applies.
   - Fold foreign entries, and entries beyond `USAGE_BY_MODEL_MAX`, into the default model's entry, and
     set `unverified_models`.
   - Keep stamping `provider` and `endpoint_id`.
   - Totals are the sum of the kept entries.

   Add `USAGE_BY_MODEL_MAX` to `aurora-core::limits` and remove its `doc-only` allow line. Turn views
   that return `usage` carry the new fields (update the OpenAPI schema).
5. **Usage aggregate**: if `GET /v1/workspaces/{ws}/usage` exists when you start, make its `by_model`
   sum the entries, with a turn that has no `by_model` counting under its `model`. If the route does
   not exist yet (it is specified, not built, per `10`), add nothing. Note in PROGRESS that the route's
   builder implements C-879's aggregate rule.
6. **Gateway and migration**:
   - Add `request_log.llm_model text null` on the partitioned parent (the pattern of
     `20261201000221_request_log_approval.sql`). Take the migration number from the next-free line and
     edit it.
   - Evaluate `model_from_request` for every request with an injected key on a governed LLM endpoint,
     using the same prefix bound as the allow-list check. Write the result: `Some(id)`, or null for
     `None`/`Unknown`. `Unknown` is never a refusal when no allow-list applies.
   - Add the field to the request-log reads (C-880). Its writes go only through the gateway role's
     existing grant.

## Constraints / file ownership

- This task owns the usage parsing in `stream.rs` and `codex`, `hosted::set_usage` and usage
  serialization, `reqlog.rs`, the `decide.rs` model step, and the request-log migration.
- It does not own selection or the environment, which are P5-MODELS-1's (same lane, merged first).
- Never parse an LLM **response** in the gateway: `08` rules it out.
- Measure performance only when the machine is idle. Re-run the existing streaming passthrough test for
  first-byte timing; it is not a budget run.

## Acceptance criteria (each test seen failing first)

1. Fixture tests:
   - a subagent turn whose totals include the subagent's tokens, where the same fixture parsed the old
     way under-counts (keep that assertion as a regression test);
   - fallback when the per-model report is absent;
   - `costUSD` is not stored.
2. Trust rule (`api/it/usage_by_model.rs`): kept (default, role, allow-listed); folded (a foreign
   model, more than 16 entries) with `unverified_models`; with no allow-list, a pattern-valid id is
   kept and an invalid one is folded. Provider and endpoint always come from `ModelChoice`.
3. Gateway: `llm_model` is set with and without an allow-list for the body, Bedrock path and Vertex
   path forms; null for `Unknown` with no refusal; the request-log reads return it; the streaming
   passthrough test is green.
4. SEC-123: the integration tests named in `17`.
5. Migration up and down on a partitioned table that already has rows. `cargo fmt --check`, clippy and
   the limits guard are green.

## Dependencies

- **P5-MODELS-1** merged: it provides roles in `ModelChoice` and `validate_model_id`.
- P4-LLM-5 merged: provider stamping and the adapters on the new providers.

## Suggested agent/model and reason

Sonnet (`rust-developer`). It crosses three services and has a security-relevant trust rule.
