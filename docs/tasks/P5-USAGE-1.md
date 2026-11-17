# P5-USAGE-1: Workspace usage — tokens and time (owner, 2026-11-17)

## Goal

Build `GET /v1/workspaces/{ws}/usage`, specified but never built (D-138 moved it to phase 5 with P4-LLM, which did not build it). It reports, per
workspace, agent, UTC day and model: turns, turn time, sandbox awake time and the four token
counters. No cost (owner: "let's forget cost for now and just track tokens and time").

## Linked requirements

- D-359, D-360 (and D-304 for the trust rule, D-276(b) for "no money").
- Contracts **C-1000** (workspace route), **C-1001** (`usage_daily` ledger and its write rule), **C-1002**
  (`runtime_daily.workspace_id`, the rollup job, `usage_rollup_state`), **C-1003** (instance route
  `GET /v1/admin/usage`, instance admins), **C-1006** (organization route `GET /v1/orgs/{org}/usage`,
  org owners and admins). Read them in full in
  `docs/API_CONTRACTS.md` §Phase 5 — P5-USAGE; they are normative and this brief does not repeat them.
- Normative: `10 §Usage and metering`; `05` (route row, `§Background jobs` "Runtime daily rollup",
  the invisible-direct-conversation note near the top: `…/usage` is aggregates only); `03`
  (`runtime_intervals`, `runtime_daily`, `usage_daily`, `usage_rollup_state`, `turns.usage`,
  `turns.model_basis`); `02 §Limits` (`USAGE_RANGE_MAX_DAYS`); `18` (*Workspace usage*).

## Context

- Turns end in exactly one place: `aurora_db::repo::turns::finish` (fenced on the turn being
  active; `rows_affected == 0` means a replay), called only from
  `services/aurora-api/src/domains/conversations/service.rs` `finish_turn`, which
  `domains/hosted/lifecycle.rs` `finish` wraps. Verify no other path sets a terminal status.
- `turns.usage` is reduced by the D-304 trust rule in `domains/hosted/model.rs`
  (`usage_for_turn`, `fold_usage`) when `turn.result` carries usage (`domains/hosted/reports.rs`
  ~490). A streaming `usage` report (`reports.rs` ~447, `hosted::set_usage`) stores the claim
  **unreduced**, and a turn finished without a terminal usage (abort, dead dispatch) keeps it.
  C-1001 requires that value to be reduced before it is counted.
- `runtime_intervals` is written by `crates/aurora-db/src/repo/sandboxes.rs` (insert ~1042, close
  ~1165 sets `ended_at` and `updated_at`). `runtime_daily` exists (migration
  `20261105000009_runners_sandboxes.sql`) and nothing writes it.
- Singleton jobs: `services/aurora-api/src/jobs.rs` with `aurora_db::jobs::try_singleton`.
- Access: copy `domains/activity/routes.rs` (`c.workspace(&s, ws)`), including the utoipa
  annotations and the session + API-key security.
- Deleted agents are hard-deleted (`agents` cascade); the ledger keeps their ids, the route returns
  `agent_name: null`.

## Scope

1. **Migration `20261201000247`** (take it and edit PROGRESS's next-free line if a lane has moved
   it): `usage_daily`, `usage_rollup_state` (with its row), `runtime_daily.workspace_id` + truncate +
   index (C-1001, C-1002), and the **backfill** of `usage_daily` from every terminal, started turn
   still present, with the same rules as the live write (C-1001). Forward-only, like the others.
2. **Ledger write** in the finish transaction (C-1001): the repo function in `aurora-db`, called
   where the fenced transition succeeded; workspace and agent from the turn's conversation. Turn
   count and time under the default model's key; tokens per `by_model` entry; nothing for a turn
   that never started.
3. **Reduce before counting** (C-1001, D-359(d)): when the finish carries no usage and the stored
   `usage` has not been through `usage_for_turn`, reduce it there, store the reduced value and count
   it. Decide how to tell "already reduced" (for example the presence of the `provider` stamp) and
   say which in the completion report.
4. **Runtime rollup job** (C-1002): hourly singleton, watermark in `usage_rollup_state`, UTC-day
   splitting, open intervals counted to the run's start, idempotent, catches up after missed runs.
5. **Route** (C-1000): query validation (`USAGE_RANGE_MAX_DAYS` in `aurora-core::limits`; remove
   its `doc-only` line from `scripts/guards/limits-gaps.allow`), the response shape exactly as
   specified, operation id `getWorkspaceUsage`, OpenAPI regenerated (`web/openapi.json`,
   `npm run gen:api` for `web/src/lib/api/schema.d.ts`). Workspace isolation: never read another
   workspace's rows, also for an `agent_id` from elsewhere.
6. **Instance route** (C-1003): `GET /v1/admin/usage`, instance admins only (copy the guard of
   `GET /v1/admin/audit`), the same range rules, `by_org`, the top `USAGE_TOP_MAX` (50, in
   `aurora-core::limits`; remove its `doc-only` line) workspaces plus `other_workspaces`, `by_model`;
   names null for deleted organizations and workspaces. Share the aggregation code with C-1000.
   Operation id `getInstanceUsage`; in the OpenAPI document.
7. **Organization route** (C-1006): `GET /v1/orgs/{org}/usage`, org owners and admins (copy the guard
   of `GET /v1/orgs/{org}/storage`), `workspace_id` filter, top `USAGE_TOP_MAX` workspaces plus
   `other_workspaces`, `by_model`; never reads another organization's rows. Same aggregation code.
   Operation id `getOrgUsage`.
8. Docs: mark the "specified, not built" notes in `05` (route row, job row) and `10` as built;
   the coordinator does PROGRESS and FEATURES.

## Constraints / file ownership

- Owns: the new migration; `crates/aurora-db/src/repo/` usage and rollup functions (new file) and
  the call in `turns::finish`'s caller; the new `domains/usage/` module in `aurora-api` and its
  registration; the job in `jobs.rs`; the usage branch of `domains/hosted/reports.rs`/`model.rs`
  only for scope item 3; `aurora-core` limits; `web/openapi.json`, `schema.d.ts`; the `05`/`10`
  sentences named above.
- No web pages: the views are P5-USAGE-2, which builds on this task's routes and generated types.
- No cost anywhere, and never read `costUSD`/`total_cost_usd` (D-276(b), D-304).
- The ledger only ever increments; no code path deletes from it (no FK, no cascade).

## Acceptance criteria (each test seen failing first)

1. Ledger (integration, `api/it/usage_ledger.rs` or similar): a finished turn with two `by_model`
   entries adds tokens to both keys and one turn plus its seconds to the default's; a replayed
   `turn.result` adds nothing; a never-started turn adds nothing; an aborted turn holding only a
   streaming claim with a foreign model is counted after folding (`unverified_models` stored);
   deleting the conversation and then the agent leaves the totals unchanged.
2. Backfill: the migration test seeds turns of each shape (with and without `by_model`, never
   started, active) and checks the ledger equals what the live rule would have written.
3. Rollup: an interval across UTC midnight splits correctly; an open interval counts up to the
   run; two runs are idempotent; a run after a missed day catches up; the watermark moves only on
   success.
4. Route: default range (30 days), `from_after_to` and `too_long` (422), malformed date (400),
   every day present with zeros, `agent_id` filter, deleted agent name `null`, another workspace's
   rows never appear, a non-member gets what `…/activity` gives, an API key works, `runtime_as_of`.
   Instance route: a non-admin gets what `GET /v1/admin/audit` gives; with 51 active workspaces the
   51st is in `other_workspaces` and the totals still add up; the `org_id` filter; a deleted
   organization's name is `null`. Organization route: a plain org member and an outsider get what
   `…/storage` gives them; a `workspace_id` of another organization returns zeros, never its rows;
   the top-50 cut adds up as above.
5. `cargo fmt --check`, clippy `-D warnings`, `scripts/guards/run-all.sh`, the OpenAPI test and
   `gen:api` (web typecheck) green; `upgrade.sh` applies the migration on a database with turns.

## Dependencies

- P5-MODELS-2 merged (39bf222c): `by_model`, `turns.model_basis`, the trust rule.

## Suggested agent/model and reason

Sonnet (`rust-developer`): a migration with a backfill, a change inside the turn-finish
transaction and a new job; data integrity matters more than size.

## Completion report (worker 33, 2026-11-17)

Commits: ca60b91a (migration `20261201000247`, ledger write, reduce, rollup job + tests), 00048abb (three routes, limits, OpenAPI/`schema.d.ts`, docs 05/10), 71e31fb9 (the `usage_per_model_migration` test now stops before every later migration), 7c032e2f (permission matrix rows).

- **Ledger**: `repo::usage::record_turn` (one `INSERT … SELECT … ON CONFLICT DO UPDATE` reading the finished turn row, conversation and agent) is called in `conversations::service::finish_turn` right after the fenced `turns::finish` succeeded (the only terminal path; `lifecycle::finish`, `fail_in` and the abort of a queued turn all go through it). The migration backfills with the same statement shape; `usage_ledger_migration::the_backfill_equals_what_the_live_rule_writes` truncates the ledger and replays `record_turn` over the same turns and compares.
- **"Already reduced" (scope 3)**: decided differently from the brief's sketch: the streaming `usage` report is reduced by `model::usage_for_turn` when it is stored (`hosted/reports.rs`, `one_event`), so every stored `turns.usage` has passed the trust rule and no stamp test is needed at finish. A terminal usage replaces it as before. Pre-migration unreduced claims are backfilled as stored (P5-MODELS-2 merged days ago).
- **Contract defect fixed**: `endpoint_id` is `text`, not `uuid` (endpoint ids are configured slugs, `03 agents.model_endpoint_id`); C-1000 and C-1001 and `03` say so now. The response field is `string|null`.
- **Rollup**: `repo::usage::rollup_runtime(conn, at)` + `jobs::rollup_runtime_daily` (hourly singleton, first tick at start-up, database clock). Watermark and upserts in one transaction.
- **Gate (batch)**: `cargo test -p aurora-core -p aurora-db` (79 + 8 + 43 it, incl. `migrated_schema_matches_03`) green; `cargo test -p aurora-api` full: 141 lib, 1418 it (one failure, `permission_matrix::coverage_every_openapi_route_is_listed`, fixed in 7c032e2f and re-run green), `managed_logs`, `ssh_logs`, `sec10_logs`, `db_spans`, `accounts_queries` green; workspace clippy `-D warnings`, fmt, guards (exit 0), `extract-schema --check`, OpenAPI drift (in the it-suite), `gen:api`, web typecheck, lint (0 errors, 5 old warnings), vitest 2279 tests, `pnpm build`; `bash scripts/e2e/upgrade.sh`: 36 passed, 0 failed. Playwright not run for this task (web changed only in the generated types).
- **Fail-first**: the ledger, route and rollup tests cannot exist without the tables and routes; the streaming-claim test asserts `unverified_models` on the stored usage, which fails without the `reports.rs` change.
- **Rule exceptions**: two `sed` one-liners in `API_CONTRACTS.md` (uuid → string) and one python replace folding the four repeated `CASE` expressions of the migration into `usage_has_breakdown()`; stated in the first commit.
