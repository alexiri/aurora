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

## Completion report (worker 16)

**Commits:** `d188f4d3` (adapter, wire), `90cb406c` (API, gateway, migration), `f0dfe2e5` (migration test); batch-gate fixes under P5-MODELS-1 (`52f2f258`, `9dc15b02`).

**What was built**
- **Adapter.** `claude-code` reads the result's `modelUsage` (key verified in the pinned 2.1.319 binary: `modelUsage`, entries `inputTokens`/`outputTokens`/`cacheReadInputTokens`/`cacheCreationInputTokens`/`costUSD`/…), maps it to `by_model`, totals = sum, `costUSD`/`total_cost_usd` never read; fallback to the main-loop `usage` when the report is absent. **Finding not in the brief:** the SDK schema says `modelUsage` is *cumulative* for the process ("read the latest result rather than summing across results; a resumed session starts fresh") and one child serves every turn of a session, so the adapter keeps a per-child baseline (`Live.usage_base`, shared by the turn mappers) and reports each turn's share (a counter that went down = fresh child, taken whole). The main-loop `usage` fallback is unchanged (whether it is per turn or cumulative was not established).
- **Fixture.** `fixtures/subagent_turn.jsonl` is **built from the pinned schema, not captured** (a capture needs a model that actually calls a subagent; `llm.sh` fronts a scripted VidaiMock with no such scenario). Same precedent as the other fixtures there.
- **codex.** Unchanged: its `Done` carries no breakdown, and the API's rule (a claim with no `by_model` becomes one entry for the default) gives the brief's "one entry for the default model" without plumbing the model into the codex mapper.
- **Wire.** `usage` of `turn.result` is open JSON; round-trip test `turn_result_usage_carries_by_model_unchanged`. The runner forwards frames untouched.
- **API.** The turn's choice is stored at dispatch (`turns.model_basis` = `{provider, endpoint_id, model, roles?}`, migration `20261201000246`, together with `request_log.llm_model`); at turn end `model::usage_for_turn` applies `fold_usage` (default, resolved roles, effective allow-list *now*, any pattern-valid id when no allow-list; foreign and past-`USAGE_BY_MODEL_MAX` entries folded into the default's entry with `unverified_models`; totals = sum of kept). `provider`/`endpoint_id`/`model` stamped from the basis (the stamping the docs credited to P4-LLM did not exist in code). A turn with no recorded basis keeps its totals and loses `by_model`. Turn views carry `by_model`, `cache_write_tokens`, `provider`, `endpoint_id`, `model`, `unverified_models` (OpenAPI + `gen:api`). `USAGE_BY_MODEL_MAX = 16` (allow line removed). Streaming `usage` reports are stored as before (they carry no model).
- **Usage aggregate.** `GET /v1/workspaces/{ws}/usage` does not exist: nothing added. PROGRESS note for the coordinator: the route's builder implements C-879's aggregate rule (sum `by_model` per `(provider, endpoint_id, model)`; a turn without `by_model` counts under its `model`).
- **Gateway.** `Governance::llm_model` runs `model_from_request` for every request with an injected LLM key on a permitted governed endpoint (allow-list or not), with the same body-prefix bound (`needs_body` no longer needs an allow-list; every such request is now buffered up to the condition limit before 6a, as allow-listed ones already were). `Some(id)` is logged, `NoModel`/`Unknown` give null and are never a refusal; a request refused for its model is logged with the model it asked for. Reads (`LogRow`, `RequestLogEntry`, activity route) return `llm_model`.

**Tests (each seen failing with its rule broken):** `usage_by_model` IT ×5 (kept; foreign folded; >16 folded; no allow-list valid kept / invalid folded; no breakdown), stream fixtures ×3 (old reading under-counts 100 vs 400; fallback; cumulative reduction), gateway `request_log_records_*` ×3 (body with/without allow-list incl. truncated/unparsable/no-model; Vertex path; Bedrock path), `activity_returns_the_requested_llm_model`, migration test on a partitioned `request_log` with rows (up; old and new partitions; reverse statements by hand — migrations are forward-only here, so no down file). SEC-123's three integration cases are covered by the gateway refusal row, the foreign-model/provider claim test, and a role model that is also allow-listed.

**Gates (batch, both tasks).** `aurora-core/config/protocol/llm/db/supervisor/runner/gateway/api` full `cargo test -p` (all targets): all green after two gate fixes — the supervisor wrappers golden test used a model id with shell metacharacters that SEC-122 now refuses (fixed, plus a test pinning the refusal), and web test fixtures lacked `subagent_model`/readiness roles (typecheck red since MODELS-1's regenerated types). api: 1256 passed, 1 failed under load (`channels_inbox::a_channel_event_turn_belongs_to_the_inbox_job_trace`, the known flake; passes alone). Workspace clippy `-D warnings`, guards (`run-all.sh`), `extract-schema.py --check`, OpenAPI test + `gen:api`: green. Web: typecheck and lint green; vitest 1980/1981 (`connect-dialog` OAuth failure text, load flake, passes alone). e2e: `FRONTDOOR=1 gateway.sh` green (the `h2_mint…` test did not fail this time), `llm.sh` 4 passed (hosted part is env-gated), `hosted.sh` 19 passed (own prefix `p5agent`, FULL_IMAGE), `upgrade.sh` (baseline `8ba42973` → HEAD, running sandbox): 36 passed, 0 failed, 0 defects (all 83 migrations incl. 000245/000246 applied; 000246 on a partitioned table with rows is also pinned by `usage_per_model_migration`).

**Left open:** live conformance for MODELS-1 and a captured subagent fixture (both need a scenario where a model calls a subagent); the per-turn reduction of the CLI's cumulative `modelUsage` is verified only against the schema text and a synthetic fixture, not a real two-turn run.
