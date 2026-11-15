# Implementation progress

Last updated: 2026-11-15, master after `35606a00` (P4-LLM-3 merged). **Phase 5 is running**: wave 1 is merged; wave 2 runs in the same three lanes (`p5-apps`: SET-2 + SET-3 at batch gate → SET-4 → SET-5 → P4-LLM-6 → P5-MODELS-3; `p5-agent`: P4-LLM-4 → P4-LLM-5 → P5-MODELS-1 → P5-MODELS-2; `p5-platform`: FIX-146 Parts C–E → P5-FILES-1..4 (owner D-316) → P4-SSH-1/2); the wave-3 queue follows (§Execution order). Phase 6 stays closed until the owner's all-clear (D-278). Phases 0–4 are closed; their record is in `docs/history/`. **Phase 6 is proposed only** (P6-ORGTREE, P6-LIMITS; owner answers so far: D-276; P6-MODELS moved to phase 5 as P5-MODELS, D-302) and starts on the owner's word. **P5-WEBHOOK (outgoing webhook app)** specified 2026-11-15 after the owner's answers (D-320): briefs P5-WEBHOOK-1..4 ready, waiting for lane placement by the Orchestrator.

## How this file is kept

- This file holds only the **current phase, the next phase and what is still open**. When a phase closes, the coordinator moves its task table, execution order, exit criteria, answered questions and log **verbatim** into `docs/history/phase-<n>.md`, together with that phase's `done` rows from `FEATURES.md`, and leaves one summary row here. Closed phases so far: `docs/history/phase-0.md`, `phase-1-2.md`, `phase-3.md`, `phase-4.md`. `docs/history/phase-5.md` already holds the parts of phase 5 that are history (answered questions, the log, the task table's full status notes up to 2026-11-15).
- Cells stay short: **Scope** is one line; **Status** is `state (lane; sha)`. Acceptance detail, gate results and review notes go in the brief, the lane's completion report and the merge commit message, not in this table.
- Owner answers are recorded as D-numbers in `docs/90-decisions.md`; this file lists only **open** questions.
- Numbers: one "next free" line in §Number allocation, **updated in place** (no running "Update …" log). The log up to 2026-11-15 is `docs/history/number-allocations.md`.
- Per-feature status and evidence: `docs/FEATURES.md` (open and current-phase rows; done rows of closed phases are in the history files). Update both files in the same commit at every lane commit, merge, spec merge and owner decision.

## Summary by phase

One row per task id, counted from the task tables (closed phases: from their history file). **In progress** = committed on a lane branch or started, not merged; **Ready** = brief written (including queued in a running lane); Done = merged to `master`.

| Phase | Tasks | Done | In progress | Ready | Not started | Moved / deferred | Table |
|---|---|---|---|---|---|---|---|
| 0 (M0–M7) | 112 | 107 | 0 | 0 | 0 | 5 (FUP-M2..M7 follow-up buckets) | `docs/history/phase-0.md` (closed 2026-11-08, AUDIT-001) |
| 1 | 15 | 15 | 0 | 0 | 0 | 0 | `docs/history/phase-1-2.md` (closed with phase 2) |
| 2 | 52 | 51 | 1 (FIX-017: the owner's real-cluster install) | 0 | 0 | 0 | `docs/history/phase-1-2.md` (closed 2026-11-09, AUDIT-002) |
| 3 | 15 | 15 | 0 | 0 | 0 | 0 | `docs/history/phase-3.md` (closed 2026-11-10, AUDIT-003) |
| 4 | 124 | 123 | 0 | 0 | 0 | 1 (FIX-131 closed without a build, D-195) | `docs/history/phase-4.md` (closed 2026-11-14, AUDIT-004) |
| 5 | 120 | 51 | 4 (P4-LLM-4, P4-LLM-5, SET-4, P5-FILES-1) | 61 (incl. P5-WEBHOOK-1..4; P5-UX-1..15, P5-IA-1..5 and FIX-159, spec on branch `alexiri/p5-ux`) | 0 (every phase-5 item has a brief) | 4 (OPS-022 cancelled, D-318; FIX-049 moved to phase 4; P5-SLACK moved to phase 6, D-285; P4-S3 moved to phase 6, D-318) | §Phase 5 tasks (TEST-021 and FIX-057, done, are counted here but their rows are in `docs/history/phase-0.md`'s task graph) |
| 6 (proposed) | 4 | 0 | 0 | 1 (P4-S3: specified and briefed, moved from phase 5, D-318) | 3 (P6-ORGTREE, P6-LIMITS: proposals awaiting the owner; P6-SLACK, moved from phase 5, D-285) | 0 | §Phase 6 tasks (proposed) |
| **All** | **442** | **361** | **6** | **62** | **3** | **10** | (column sums; P5-SLACK/P6-SLACK and P4-S3 are counted in both phases) |

## Environment

- **Phases**: 0–4 closed. Phase 5 authorized by the owner 2026-11-14 and running (lanes branch from `master` in worktrees and merge straight back; no phase branches). Phase 6: proposals only, starts on the owner's word.
- **Concurrency**: owner set **3 concurrent implementation lanes** (2026-11-06 evening; was 4). Lanes already running finish; no new lane starts while 3 are active. Lanes are run by the Orchestrator session (`docs/tasks/PHASE5-PLAN.md`); other sessions add specs and briefs.
- **Worktrees** (2026-11-06, owner): implementation lanes that touch a crate another lane is editing (esp. `services/aurora-api`) run in their own git worktree (Orca worker `--worktree new-top-level --repo id:706ffb9e-ae8b-4f11-a841-518970480f40`, or Agent `isolation: worktree`) with a per-worktree `target/` (never a shared `CARGO_TARGET_DIR`: e2e scripts run `target/debug/<bin>`). The Orchestrator merges each branch into `master` after review. Disk budget: ~15 GB per worktree build, so at most 3 building worktrees while free space is under 100 GB; check `df -h /home` before starting one.
- Rust 1.99 stable (system, no rustup); `wasm32-wasip2` installed (`rust-wasm`, I-02a).
- Node 20.20, pnpm 10 (`~/.local/bin`), podman 5 (compose), psql client.
- Dev Postgres 16: container `aurora-dev-pg`, `postgres://aurora:aurora@127.0.0.1:55432/aurora` (`.env`, I-05).
- sqlx-cli 0.8.6 at `~/.cargo/bin/sqlx` (not on PATH; use full path).
- Disk: **74 GB free on `/home` (2026-11-15 08:30, 3 lane worktrees + 2 owner worktrees)** after pruning incremental dirs, below the 80 GB clean-up mark: prune `target/*/incremental` between batches and remove merged worktrees (`scripts/dev/clean-lane.sh`) before starting another build.
- CI: GitHub Actions runs again since 2026-11-09 (it was stopped by billing 2026-11-08). Local verification stays the merge gate; CI is checked after each push.

## Current phase: 5

Columns: **Size** S (≤ 1 day of one lane) / M (a few days) / L (a week or more, several crates or both stacks). **Lane** Rust / Next.js / both / docs / ops / package (`plugins/builtin/**` only) / tests. **Refs**: decisions, contracts, SEC invariants and migrations the task owns or implements. **Brief**: `docs/tasks/<file>`, or "needed".

Shared files that no lane owns (each lane appends or regenerates them, and the coordinator re-generates at merge): `docs/API_CONTRACTS.md` (one new section per lane), `web/openapi.json` + `web/src/lib/api/schema.d.ts` (regenerated), `crates/aurora-db/tests/fixtures/spec_schema.json` and the machine-checked tables of `docs/03` (each migration lane moves its own `§Planned changes` rows), `docs/tests/security-coverage.md` (append rows), `services/aurora-api/tests/it/permission_matrix.rs` (append rows), `Cargo.lock` (only lanes adding a dependency; serialize those merges).

### Phase 5 tasks

Lane order, waves and number blocks: `docs/tasks/PHASE5-PLAN.md`. Lanes `p5-apps`, `p5-agent`, `p5-platform`. Scope, ownership and acceptance live in each brief; the full status notes written before 2026-11-15 are kept in `docs/history/phase-5.md`. Status values: `done (merged <sha>)` · `in-progress (lane <lane>; committed <sha> on alexiri/<lane>, not merged)` · `in-progress (lane <lane>; started)` · `ready` (brief written; "queued in lane X" when it is in a running lane's chain) · `not started` (no brief yet) · `deferred` / `moved` / `cancelled`.

| Task | Scope (one line) | Refs | Size | Lane | Depends on | Brief | Status |
|---|---|---|---|---|---|---|---|
| OPS-022 | Multi-arch images (amd64 + arm64) | `15 §Images` | M | ops | OPS-020 | `OPS-022.md` | cancelled (owner 2026-11-15, D-318: not needed; was deferred 2026-11-07) |
| OPS-021 | Dependabot, grouped, with ignore rules and a review policy | owner | S | ops | OPS-019, OPS-020 | `OPS-021.md` | done (merged 3c983224) |
| P5-SLACK | Slack connector | `21`, D-78 | L | Rust | connector contract (`13`) | — | moved to phase 6 as P6-SLACK (owner 2026-11-15, D-285: not a priority) |
| P5-ASRC | Addon sources: third-party package repositories (signed repo manifest, TOFU key, polling, update badges) | D-250, C-590..C-609, SEC-86, migration 000180 | L | both | — | `P5-ASRC.md` | done (merged 3c983224) |
| P5-CERN-ADDONS | `aurora-addons-cern` repo (release script, CI, signed releases) with `cern-phonebook` (LDAP through the gateway LDAP lane) and `cern-maps` | D-235(d), D-251 | M–L | package + Rust | P5-ASRC | `P5-CERN-ADDONS.md` | done (merged 555f6058 (gateway LDAP lane), 26e91910 (aurora side); external repo `aurora-addons-cern` releases v0.1.0, v0.1.1; kind e2e k7 7/7) |
| OPS-024 | `helm upgrade` covered by CI (kind upgrade legs) + hook NetworkPolicy guard | owner 2026-11-11 | M | ops | — | `OPS-024.md` | done (merged d0819942) |
| FIX-045 | Personal-workspace names default to the OIDC `{sub}` | D-162, C-520..C-523 | M | both | — | `FIX-045.md` | done (merged 9ae49a47) |
| FIX-046 | Shell alignment: workspace selector not indented, phone drawer from the right | D-163 | S | Next.js | — | `FIX-046.md` | done (merged 34a65d3f) |
| FIX-047 | Load-flaky groups long-poll test made deterministic | — | S | Rust tests | — | `FIX-047.md` | done (merged b653734b) |
| FIX-048 | "All organizations" link 404 on `/admin` | C-459 | S | Next.js | — | — | done (merged 28e15781; follow-up: Admin Overview page) |
| VEL-001 | Build and test velocity pass (sccache in CI, parallel Playwright) | owner 2026-11-12 | M | CI/web | OPS-016 | — | done (merged 2bcfb4ba) |
| FIX-051 | Runner stream ITs answer WebSocket Pings | — | S | Rust tests | — | — | done (merged 7e77f62d) |
| FIX-053 | Terminal Helm switch set once (`terminal.enabled`) | — | S | Helm | — | — | done (merged 3e58fcc5) |
| FIX-054 | Terminal typing dead: Binary frames from the web, a real controlling terminal in the supervisor | owner incident 2026-11-12 | M | Rust + web | P4-TERM, FIX-051 | — | done (merged bb8beb73) |
| FIX-056 | Previews Helm switch set once (`relay.enabled`) | — | S | Helm | FIX-053 | — | done (merged cf44c722) |
| VEL-002 | Local quick gate (`scripts/dev/quick-gate.sh`, `just quick`) | owner 2026-11-12 | M | scripts | VEL-001 | — | done (merged 6396e32a) |
| VEL-003 | Template-database `TestDb` (2.6× on the api it-suite) | owner 2026-11-12 | M | Rust testkit | — | — | done (merged fc265788) |
| FIX-055 | Preview-stream test bind readiness | — | S | Rust tests | — | — | done (merged dfa4d8e3) |
| FIX-049 | Sandbox starts fail on a transient PVC provisioning race; Retry/Restart recovery | D-164, C-610, C-626 | S–M | Rust + web | — | `FIX-049.md` | moved to phase 4 (done there; `docs/history/phase-4.md`) |
| FIX-050 | OIDC-claim-created org born ownerless; gateway refusals made visible | D-165, C-630 | S–M | Rust + web | — | `FIX-050.md` | done (merged 98c0627b) |
| FIX-052 | Gateway sent `Host` next to `:authority` on HTTP/2 upstreams | D-166, C-633 | S | Rust | — | `FIX-052.md` | done (merged fffb4301) |
| P4-S3 | S3-compatible `BlobStore` beside Postgres, `aurora-api blobs migrate` | D-120, SEC-63 | M–L | Rust | — | `P4-S3.md` | moved to phase 6 (owner 2026-11-15, D-318; was queued in wave 3, lane p5-platform, never started) |
| P4-NARR | Live narration cards (one editable step card per turn on chat platforms; per-agent opt-in) | D-279, D-280, C-846..C-850, SEC-111, migration 000231 | M | both | — | `P4-NARR.md` | ready (spec merged 2026-11-15; owner answers D-286 (collapse), D-288 (rest accepted); wave 3: queued in lane p5-agent) |
| P4-UTOK | User tokens: `send_as_me` with a person's Mattermost token, each send approved by its owner | D-281..D-284, C-851..C-859, SEC-112, SEC-113, migrations 000232/000233 | L | both | — | `P4-UTOK.md` | ready (spec merged 2026-11-15; owner answers D-288 (recommendations accepted); Opus review before merge; wave 3: queued in lane p5-agent) |
| P4-EVAL-1 | Evals: questions, agent eval settings (judge key), configuration fingerprint + preview | D-289..D-294, C-860.., migrations 000236/000239 | M | Rust | — | `P4-EVAL-1.md` | ready (spec merged 2026-11-15; owner answers D-300: recommendations accepted; order 1 → {2 ∥ 4} → 3; wave 3: queued in lane p5-agent) |
| P4-EVAL-2 | Eval run engine: one `eval` conversation per question, numeric/text scoring, side-effect refusal (`eval_read_only`), cancel, schedules, regression notice, retention | D-290.., SEC-114..SEC-116, migrations 000237/000238/000240 | L | Rust | P4-EVAL-1 | `P4-EVAL-2.md` | ready (Opus reviews the refusal code; wave 3: queued in lane p5-agent) |
| P4-EVAL-3 | `PlatformLlm` with the `eval_judge` purpose; judged scoring, fail-closed judge | D-289.., `08` | M | Rust | P4-EVAL-2 | `P4-EVAL-3.md` | ready (Opus reviews `platform.rs`; wave 3: queued in lane p5-agent) |
| P4-EVAL-4 | Dashboard: agent Behaviour › Evals page | C-860.., `14` | M | Next.js | P4-EVAL-1 | `P4-EVAL-4.md` | ready (wave 3: queued in lane p5-agent) |
| P4-LLM-1 | LLM provider framework; `openai`, `openai_compatible`; `anthropic` inference path set | D-260, D-268(d), C-800.. | L | Rust | — | `P4-LLM-1.md` | done (merged 88faf124; D-248) |
| P4-LLM-2 | Gateway LLM mint service + Google Vertex AI | D-261, SEC-97 | L | Rust | P4-LLM-1 | `P4-LLM-2.md` | done (merged 7909fb9c; D-249) |
| P4-LLM-3 | `azure_openai` (keys and Entra) | D-261, D-268(c) | M | Rust | P4-LLM-2 | `P4-LLM-3.md` | done (merged 35606a00; D-311) |
| P4-LLM-4 | `bedrock` (assume-role, IAM keys, API keys; SigV4 in the gateway) | D-261, D-268(c) | L | Rust | P4-LLM-2 | `P4-LLM-4.md` | in-progress (lane p5-agent; done and merged with master on alexiri/p5-agent (0a17730b), merges together with P4-LLM-5 after its batch gate; D-312, D-313) |
| P4-LLM-5 | Harnesses on the new providers + hosted e2e (no key reaches the sandbox) | D-260, SEC-64, SEC-66 | M | Rust | P4-LLM-1 (e2e after 2–4) | `P4-LLM-5.md` | in-progress (lane p5-agent; code 0bb7fc77 committed, hosted e2e + batch gate next) |
| P4-LLM-6 | Endpoint destinations + dashboard fields for providers and auth modes | D-262, SEC-98 | M–L | both | SET-3, P4-LLM-1 | `P4-LLM-6.md` | ready (queued in lane p5-apps) |
| P5-MODELS-1 | The harness chooses models (owner D-302): the agent's model is the default; model roles (endpoint and agent), subagent default, model metadata, `ModelChoice.roles`, `claude-code` alias variables (turns and `harness.env`), readiness warnings, instructions line, eval fingerprint; live suite on LiteLLM | D-302, D-303, SEC-122, SEC-123, C-875..C-878, C-881..C-884 | M | Rust | P4-LLM-5 (same adapter code); C-779 (SET-3) for settings writes | `P5-MODELS-1.md` | ready (queued in lane p5-agent after P4-LLM-5) |
| P5-MODELS-2 | Usage per model including subagents (fixes `claude-code` counting the main loop only), trust rule against the allow-list, `turns.usage.by_model`, `request_log.llm_model` | D-304, SEC-123, C-879, C-880 | M | Rust | P5-MODELS-1 | `P5-MODELS-2.md` | ready (queued in lane p5-agent after P5-MODELS-1) |
| P5-MODELS-3 | Dashboard: Default model, roles and subagent default, readiness roles, Admin › LLM endpoints roles and model details, usage and request-log models; operator recipe (LiteLLM roles, router fallback) | D-302..D-304, C-875, C-876, C-878..C-880, C-882 | M | Next.js | P5-MODELS-1 (P5-MODELS-2 for usage), SET-3, P4-LLM-6 | `P5-MODELS-3.md` | ready (queued in lane p5-apps after P4-LLM-6) |
| P4-011B-1 | Gateway parking and single-use approval grants | D-264, D-269, SEC-74 | L | Rust | — | `P4-011B-1.md` | ready (order 1 → {2 ∥ 3} → 4 → 5; wave 3: queued in lane p5-platform) |
| P4-011B-2 | Deciding parked approvals; resume follow-up; `approvals.park.*` settings | D-264, D-265, C-822.. | L | Rust | P4-011B-1 | `P4-011B-2.md` | ready (wave 3: queued in lane p5-platform) |
| P4-011B-3 | Web Push backend (VAPID, subscriptions, encrypted pushes) | D-266, D-269(c), SEC-75/76 | L | Rust/ops | P4-011B-1 | `P4-011B-3.md` | ready (wave 3: queued in lane p5-platform) |
| P4-011B-4 | Dashboard: service worker, push opt-in, parked-approval states | D-266, D-267, SEC-102 | M–L | Next.js | P4-011B-2, -3 | `P4-011B-4.md` | ready (wave 3: queued in lane p5-platform) |
| P4-011B-5 | Park-and-resume and Web Push end to end, operator pages | D-133, D-134, D-269(d) | M | tests/ops | P4-011B-1..4 | `P4-011B-5.md` | ready (wave 3: queued in lane p5-platform) |
| P5-FILES-1 | Files backend: `exec.*` leg 2 + supervisor `sftp` exec, `auf_` legs, API SFTP pool and `/files*` routes | D-316, C-898..C-903, C-906, SEC-125/126 | L | Rust | — | `P5-FILES-1.md` | in-progress (lane p5-platform; Parts A + B committed 69fb3f31, cfa1df92 on alexiri/p5-platform, not merged; Part C next) |
| P5-FILES-2 | Files page: browse, upload (files, folders, drag and drop), download, rename, delete | D-316, C-904 | M | Next.js | P5-FILES-1 | `P5-FILES-2.md` | ready (queued in lane p5-platform) |
| P5-FILES-3 | Files viewer: CodeMirror editor with conflict dialog, raster image preview | D-316, C-905, SEC-126 | M | Next.js | P5-FILES-2 | `P5-FILES-3.md` | ready (queued in lane p5-platform) |
| P5-FILES-4 | Files end to end: `scripts/e2e/files.sh` + Playwright journey | D-316, C-907 | S–M | tests | P5-FILES-1..3 | `P5-FILES-4.md` | ready (queued in lane p5-platform) |
| P5-UX-1 | Role-aware nav, refusals in words (never "Could not reach the server" for a 403), empty states ask only non-actors, Welcome links that exist, first-run fixes | D-323, C-915, C-916, C-927 | M | Next.js | P5-IA-1 | `P5-UX-1.md` | ready (spec on branch `alexiri/p5-ux`, not merged; not placed) |
| P5-UX-2 | Connect plan (method readiness, who unblocks), manifest `audience`/`limit`/`scope_presets`, connect with a grant (sealed through OAuth), probe causes, journey links | D-321, D-322, D-337, SEC-130, C-955..C-959 | M | Rust | — | `P5-UX-2.md` | ready (spec on branch, not merged; not placed) |
| P5-UX-3 | "Give <Agent> <App>" connect journey: agent Access › Add an app, Welcome, refusals and Apps tab; C-842 controls under Advanced | D-337, D-321, C-914, C-927 | M–L | Next.js | P5-UX-2; P5-UX-4 (soft) | `P5-UX-3.md` | ready (spec on branch, not merged; not placed) |
| P5-UX-4 | Give-access preselection, "No agent can use this" (`grant_count`), effective access in words (`selected_by_details`) | D-324, C-917, C-918 | S–M | both | — | `P5-UX-4.md` | ready (spec on branch, not merged; not placed) |
| P5-UX-5 | Workspace owners add people by e-mail; pending access applied at verified first sign-in; never lets a non-member into the org (D-328) | D-325, D-328, SEC-128, C-919, C-920, migration (next free when placed) | M | both | P5-UX-1 | `P5-UX-5.md` | ready (spec on branch, not merged; not placed) |
| P5-UX-6 | Admin defaults: starter grant on by default + backfill, display-name template, wizard Apps step (apps without an OAuth client), builtins in the kind dev image, stale docs | D-326, C-922..C-924 | M | both | P5-IA-4; P5-UX-2 (soft) | `P5-UX-6.md` | ready (spec on branch, not merged; not placed) |
| P5-UX-7 | Credentials › Overview and a plain-language Policy summary with Common changes; operator labels renamed | D-327, C-925, C-926 | M | Next.js | P5-UX-3, P5-UX-4 | `P5-UX-7.md` | ready (spec on branch, not merged; not placed) |
| FIX-159 | **High priority.** Simulate decides app requests like the gateway: shared app/tool identification in `aurora-registry`, connection candidates and winner rule in the simulator, simulate == gateway table test (today every connection grant is `target_mismatch` and a blocked request can show `allow`) | D-329, C-930 | M | Rust | — | `FIX-159.md` | ready (spec on branch `alexiri/p5-ux`, not merged; not placed) |
| P5-UX-8 | App setup status `GET /v1/apps/{app}/setup` (methods ready or missing what, clients by level, instances, availability, connections, callback URL, external id, what the caller can do); the connect plan becomes a projection | D-330, C-931 | M | Rust | P5-UX-2 (same function, soft) | `P5-UX-8.md` | ready (spec on branch, not merged; not placed) |
| P5-UX-9 | One setup page per app (workspace and org views) with inline actions at the viewer's level; indexes link to it; posture on Org › Apps | D-330, C-932, C-933 | M | Next.js | P5-UX-8; P5-UX-3, P5-UX-13 (soft) | `P5-UX-9.md` | ready (spec on branch, not merged; not placed) |
| P5-UX-10 | "What can <Agent> do in <App>": per-tool decision with the deciding layer (`GET /v1/agents/{agent}/apps/{app}/access`), unreachable grant settings called out; Policy lists grant-derived rules read-only | D-331, C-934, C-935 | M | both | FIX-159, P5-IA-2 | `P5-UX-10.md` | ready (spec on branch, not merged; not placed) |
| P5-UX-11 | Connect an app for the whole organization from the dashboard (journey org mode, starter or give-now), org connection actions, chooser leads with the org | D-332, C-936, C-937, C-947 (with P5-UX-15) | M | Next.js | P5-UX-3 (soft) | `P5-UX-11.md` | ready (spec on branch, not merged; not placed) |
| P5-UX-12 | Access editor starts from the connection (read-only: reads Allow, writes folded; read-write: writes Ask), read-only note everywhere, tool descriptions instead of HTTP patterns, builtin descriptions + `ec2_call` rename | D-333, C-938, C-939 | M | both | — | `P5-UX-12.md` | ready (spec on branch, not merged; not placed) |
| P5-UX-13 | Setup guides with `{{aurora.*}}` placeholders filled for the viewer, guide checks, builtin guides rewritten (no "Admin, Apps," pages), external id shown on AWS and Bedrock assume-role forms (`show_context`) | D-334, C-940..C-942 | M | both | — | `P5-UX-13.md` | ready (spec on branch, not merged; not placed) |
| P5-UX-14 | Test connection (probe, gateway database login, hook mint) and connection health on every row; post-connect test for probe-less methods | D-335, SEC-129, C-943..C-945, migration (next free when placed) | M | both | — | `P5-UX-14.md` | ready (spec on branch, not merged; not placed) |
| P5-UX-15 | Names and labels: multi-workspace pickers lead with the workspace, agent name on New workspace, OAuth client picker by method (`used_by_methods`), GitLab method labels, `personal_of` for the Connect chooser | D-336, C-946..C-949 | S–M | both | — | `P5-UX-15.md` | ready (spec on branch, not merged; not placed) |
| P5-IA-1 | One structure for every level (owner, D-338): topic navigation per level, `routes.ts`, every old route redirected, role visibility, Administration entry and level badge, tour; page contents moved unchanged | D-338, D-342, C-960..C-962, C-975 | M–L | Next.js (+ gateway link paths) | — (first of P5-IA and the P5-UX screen briefs) | `P5-IA-1.md` | ready (spec on branch `alexiri/p5-ux`, not merged; not placed) |
| P5-IA-2 | Scope ladder primitive; Internet access as one page at each level (Governance › Egress + Policy merged), workspace overrides route | D-339, C-963..C-965 | M | both | P5-IA-1 | `P5-IA-2.md` | ready (spec on branch, not merged; not placed) |
| P5-IA-3 | AI models and Agents topics at each level (model access, LLM keys, keys to fix; BYO ladder, defaults, approvals, terminal, previews, skills) | D-338..D-340, C-966, C-967 | M | Next.js | P5-IA-1, P5-IA-2 | `P5-IA-3.md` | ready (spec on branch, not merged; not placed) |
| P5-IA-4 | People, Data (one retention form), Platform, Chat platforms and Settings topics; settings sections rendered inside topics; All settings index; Administration Overview | D-338..D-340, C-968..C-971 | M–L | Next.js | P5-IA-1 (P5-IA-2 for the ladder); coordinate with SET-4/5 | `P5-IA-4.md` | ready (spec on branch, not merged; not placed) |
| P5-IA-5 | Settings in words: field metadata registry (labels, help, choices, defaults, docs), read-only current values for not-yet-editable sections, UI text test | D-340, D-341, C-972..C-974 | M | both | P5-IA-4 | `P5-IA-5.md` | ready (spec on branch, not merged; not placed) |
| P4-SSH-1 | User SSH keys, SSH CA, certificate mint, `aurora ssh` | D-239, SEC-93 | M | both | — | `P4-SSH-1.md` | ready (queued in lane p5-platform after P5-FILES-4) |
| P4-SSH-2 | `aurora-terminator` (certificate-only SSH into the sandbox; no port forwarding) | D-239, D-268(a)(b), SEC-92/94..96 | L | Rust/ops | P4-SSH-1, P5-FILES-1 | `P4-SSH-2.md` | ready (queued in lane p5-platform) |
| SPEC-SETTINGS | Instance settings move from Helm into the app (Helm-set values locked) — spec batch | D-195, D-236..D-238, SEC-89/90 | L | docs | — | `SPEC-SETTINGS.md` | done (merged 76bb566a) |
| SET-1 | Settings framework: registry, Helm locks, storage, live reload, audit, export/import, CLI, Admin › Settings | D-236 | L | both | — | `SET-1.md` | done (merged 602b90db; security review fixed: D-307) |
| SET-2 | Governance and BYO floors as live settings | D-237 | M | both | SET-1, FIX-124 | `SET-2.md` | done (merged 2c55c538) |
| SET-3 | LLM endpoints as live settings | D-237 | M | both | SET-2 | `SET-3.md` | done (merged 2c55c538; D-308, C-749) |
| SET-4 | Sign-in, OIDC providers and Email editable | D-235(b), D-237 | M–L | both | SET-1 | `SET-4.md` | in-progress (lane p5-apps; backend committed a31d244b on alexiri/p5-apps, not merged (D-309; no re-authentication per D-317); dashboard editors next) |
| SET-5 | Agents, approvals, retention, skills, SCIM, previews, terminal and limits as settings | D-235(a), D-237 | L | both | SET-1 | `SET-5.md` | ready (queued in lane p5-apps) |
| FIX-124 | Admin › Secret stores editable (unless set in Helm), value migration job | D-195 | M–L | both | SET-1 | `FIX-124.md` | done (merged 602b90db; D-244, D-307) |
| FIX-133 | Read-only connections; GitHub App tokens narrowed; fine-grained GitHub auth | D-240, C-740 | M–L | both | FIX-123, FIX-107 | `FIX-133.md` | done (merged 787d5c02) |
| FIX-134 | One Sentry app with preset region instances | D-93, D-195(a) | M | both | FIX-123, FIX-132 | `FIX-134.md` | done (merged ada98f25) |
| FIX-135 | App families: one catalogue tile per product (GitHub/GHES, Atlassian Cloud/DC) | D-91..D-93 | M | both | FIX-134 (same lane) | `FIX-135.md` | done (merged ada98f25) |
| FIX-136 | Token-aware permissions (`tools[].scopes`, `tool_scope_missing`) | D-241, C-741 | M | both | FIX-118, FIX-133 | `FIX-136.md` | done (merged 787d5c02) |
| FIX-137 | One chat Markdown renderer that renders like GitHub | D-227, D-245 | M–L | Next.js | FIX-099, FIX-117 | `FIX-137.md` | done (merged 05a5753f) |
| FIX-138 | Terminal: bash/zsh, completion, bounded pane, own window, harness wrappers | D-246, D-247, C-750 | M–L | both | FIX-098, FIX-129 | `FIX-138.md` | done (merged 05a5753f; owner follow-ups on master: alternate-screen pane 6906cefe, terminal open carries the agent's models a7998616 (D-247(e), C-750(4))) |
| P5-AGENT-1 | `agent_id` filter on the workspace approvals list | D-201, C-700 | S | Rust | — | `P5-AGENT-1.md` | done (merged 05a5753f) |
| P5-AGENT-2 | Agent page in five sections with a collapsible status rail | D-200, D-201 | L | Next.js | P5-AGENT-1 | `P5-AGENT-2.md` | done (merged 05a5753f) |
| FIX-141 | Indico find-and-book recipe for agents | D-84, D-123 | S–M | package | FIX-136 | `FIX-141.md` | done (merged 7b46ca3d) |
| P5-CHAT-0 | Chat as a messaging client: design boards + `14` text | D-228 | M | design/docs | — | `P5-CHAT-0.md` | done (boards approved by the owner, cdea8832) |
| P5-CHAT-1 | Quote-reply and files-only messages, API | D-228, C-725, C-726, migration 000171 | M | Rust | — | `P5-CHAT-1.md` | done (merged ed41c9ed) |
| P5-CHAT-2 | Web: reply action, reply bar, quote strip, permalinks, jump to latest | D-228, C-725 | M | Next.js | P5-CHAT-1, P5-AGENT-2, FIX-137 | `P5-CHAT-2.md` | done (merged ed41c9ed) |
| P5-CHAT-3 | Web composer: formatting, preview, smart paste, files, drafts, emoji | D-228, C-726 | M–L | Next.js | P5-CHAT-2 | `P5-CHAT-3.md` | done (merged 154d635a) |
| P5-CHAT-4 | Message search: API (A), chat-panel search (B), agent conversation tools (C) | D-229, D-230, C-727, C-728, migration 000172 | M–L | Rust then Next.js | A after P5-CHAT-1; B after P5-CHAT-2 | `P5-CHAT-4.md` | done (merged ed41c9ed (A + C), ece633ed (B)) |
| FIX-142 | Agent messages keep their Markdown on chat platforms; every Mattermost ping accounted for | D-226, D-227, D-270, D-271 | M | Rust | — | `FIX-142.md` | done (merged 3c983224) |
| FIX-144 | Self-hosted host check says why it failed; agents told each server's address | C-730 | S–M | both | — | `FIX-144.md` | done (merged d1933319) |
| FIX-145 | Self-hosted apps served under a path (`base_path`) | D-231, C-731, migration 000140 | M–L | both | FIX-144 | `FIX-145.md` | done (merged d1933319) |
| FIX-146 | Request tracing end to end, browser to database | D-232, C-732, SEC-88, migration 000150 | L | both | spec done (c8aa4cb5) | `FIX-146.md` | done (merged 76c922d5; D-253; gateway perf not measured (busy host); the kind --tracing check is the owner's) |
| FIX-147 | One permissions editor (`GrantForm`, `PermissionGroups`) | D-233 | M | Next.js | — | `FIX-147.md` | done (merged 787d5c02) |
| FIX-149 | Connect/Reconnect as real dialogs; OAuth result over `BroadcastChannel` | D-272, C-842 | M | both | — | `FIX-149.md` | done (merged b9c3046f) |
| P5-ADMDEL | Instance admins delete any organization or workspace | D-234, C-733 | M | both | — | `P5-ADMDEL.md` | done (merged 3c983224) |
| P5-ICS | Generic "Calendar link (ICS)" app: secret-link credential, gateway fetches the stored URL | D-273, C-843, SEC-107 | M–L | Rust + package | p5-apps hot files | `P5-ICS.md` | ready (wave 3: queued in lane p5-apps) |
| P5-CALINV | Agents add calendar events by e-mailing the person an invitation | D-274, C-844, SEC-108, migration 000230 | M | Rust | instance mailer | `P5-CALINV.md` | ready (wave 3: queued in lane p5-platform) |
| P5-CALDAV | Read and write any CalDAV server with the person's own app password | D-275, C-845, SEC-109 | L | Rust + package | P5-ICS (same lane) | `P5-CALDAV.md` | ready (wave 3: queued in lane p5-apps) |
| P5-WEBHOOK-1 | Outgoing webhook: pure crate `aurora-webhook` (config validation, template render, slot checks, signing), registry `secret_link.mode: send` + presets, package `webhook` (9 presets, setup guide) | D-320, C-909, C-912, SEC-127 | M | Rust + package | P5-ICS (extends `secret_link`) | `crates/aurora-webhook`, `aurora-registry`, `limits.rs` (constants), `plugins/builtin/webhook`, `09` | `P5-WEBHOOK-1.md` | ready (spec merged 2026-11-15; not placed — proposed: `p5-apps` after P5-ICS) |
| P5-WEBHOOK-2 | Outgoing webhook: migration `…243` (`app_connections.config`), connect/edit/preview/presets/send-test routes, `outgoing_webhooks` settings + org settings, gateway send path (approval, rate, render, sign, redirects, echo), `slots` decoder | D-320, C-909, C-910, C-912, SEC-127, migration `20261201000243` | L | Rust | P5-WEBHOOK-1; P5-CALINV (org-settings store) | `aurora-api`, `aurora-gateway` `link` send path, decoder, `03`/`05`/`06`/`07`/`15` | `P5-WEBHOOK-2.md` | ready (spec merged 2026-11-15; not placed — proposed: `p5-apps` after -1) |
| P5-WEBHOOK-3 | Outgoing webhook: each webhook an account-bound `aurora-agent` operation (`api.json`, `tools.md`, `json` field type, re-render on edit); no `webhooks.json` | D-320(g), C-911, SEC-127 | M | Rust | P5-WEBHOOK-2, P5-GUIDE-6 | `agent_guide.rs`, `agent_cli/`, home-sync render inputs, `24` | `P5-WEBHOOK-3.md` | ready (spec merged 2026-11-15; not placed — proposed: `p5-apps` after P5-GUIDE-6 and -2) |
| P5-WEBHOOK-4 | Outgoing webhook dashboard: 3-step connect, presets, example picker, raw editor, preview, Send test, card, settings and org switch | D-320(f), C-913, C-909, C-912 | M–L | Next.js | contracts (now); Playwright needs P5-WEBHOOK-2; SET-5 for the Admin section | `web/` only, `14` | `P5-WEBHOOK-4.md` | ready (spec merged 2026-11-15; not placed — any lane with web capacity) |
| FIX-150 | `send_message` resolves an exact handle by itself; refusals list aliases to retry with; messaging-tool fields described | D-287, `13 §5.10` | S–M | Rust | — | `FIX-150.md` | ready (brief written 2026-11-15; wave 3: queued in lane p5-agent) |
| FIX-151 | Platform-tool calls tied to the calling harness process (`SO_PEERCRED` + process ancestry; another conversation's or a non-harness process's call refused `session_not_caller`) | D-301, SEC-121, S-09, `12 §Path` | S–M | Rust | — | `FIX-151.md` | ready (brief written 2026-11-15; wave 3: queued in lane p5-apps before P5-GUIDE-6) |
| FIX-152 | A failed re-authentication returns to its page with `reauth_error` (no longer `/auth/login`); the `reauth_stale` denial records the `auth_time` age or that the claim was missing (CERN SSO org delete refused `reauth_stale`; root cause follows from that evidence) | D-305, C-890, D-234, FIX-108 | S–M | both | — | `FIX-152.md` | done (merged 11f60818; oidc ITs 55/55, web tests 47/47, build, guards; next: owner retries on CERN SSO, the audit row's `auth_time_age_s`/`auth_time_missing` decides the root-cause follow-up) |
| FIX-153 | No dead ends between an app and an agent: one gateway link builder (the `apps/<app>` and `secrets` links had no page), `ldap_no_connection` reasons with links + `ldap_https_required`, `access_restricted` → Access tab, agent page by id, org Apps lists apps with Connect… (owner could not find where to connect CERN Phonebook) | D-306, C-891..C-894, D-251 | M | both | — | `FIX-153.md` | done (merged b79dffeb) |
| FIX-154 | Credential-free apps (CERN Phonebook, CERN Maps) in one step: `credential_free`, `PUT /v1/agents/{agent}/grants/apps/{app}` creates or reuses the connection inside the grant, Access tab **Give access**, form-less connect dialog, refusals and `request_app_connection` point at the Access tab | D-306, C-895..C-897, D-251, SEC-110 | M | both | FIX-153 | `FIX-154.md` | done (merged b79dffeb; cern-phonebook 0.1.2 released) |
| FIX-155 | Terminal hygiene: the wrappers parse `harness.env` instead of sourcing it (any other line → refuse to start), call `mkdir` by absolute path, the writer's "read-only to the agent" comment is corrected, and the terminal pane says it runs inside the agent's sandbox (red-team F12; the terminal is accepted as part of the agent's environment, `17` residual) | D-314 (withdrawn), `12 §Terminal`, `17` residual, `14 §Visual design`; SEC-124 released (unused) | S | both | — | `FIX-155.md` | ready (brief written 2026-11-15, cut to hygiene when D-314 was withdrawn; queued in lane p5-platform after FIX-156) |
| FIX-156 | **High priority (owner 2026-11-15).** Copy and paste work in the web terminal: Ctrl+Shift+V pastes (today wterm sends ^V, because it matches only a lowercase `v`), Ctrl+Shift+C copies or does nothing (never ^C), right-click offers Paste, the OSC 52 confirmation falls back outside a secure context; bracketed paste, ESC stripping and 16 KiB chunks are kept; fixed in `TerminalPane`, not in `@wterm/*` | C-622 (copy sentence amended by the lane), `14 §Terminal tab`, FIX-054 | S–M | Next.js | — (not beside FIX-155: both edit `terminal-tab.tsx`; FIX-156 first) | `FIX-156.md` | ready (brief written 2026-11-15; queued in lane p5-platform after P5-FILES-4, before FIX-155) |
| FIX-157 | No operation requires an OIDC re-authentication: the 600 s `reauth_at` check is removed from account deletion, admin org/workspace delete, SCIM enable/token create, instance settings writes/import and secret stores (session-only rules, confirmations and critical audit kept); dashboard "Sign in again" steps removed; the `reauth=1` flow stays unused (CERN SSO never sends `auth_time`) | D-317 (supersedes D-305(a)), C-152, C-471, C-733, C-746, C-773, SEC-90 | M | both | — | `FIX-157.md` | done (merged 8c431259; lane: api+db suites 1265/1265, clippy, guards, vitest 1681, build; post-merge targeted ITs, OpenAPI drift, vitest 1695 green) |
| FIX-158 | Chat platforms show the agent is working until the answer posts: the 👀 receipt stays until the thread's answer or failure line (follow-on work keeps it, orphan sweep, never removed before added), plus a typing indicator while a turn runs (Mattermost declares `native_status`, refreshes `/users/{id}/typing`); Part A investigates the early removal (deployment query, receipt log lines) | D-319, C-908, `13 §4.5/§5.1/§5.2`, `20` | M | Rust | — (not beside P4-NARR: `mirror.rs`, Mattermost `ops.rs`) | `FIX-158.md` | ready (brief written 2026-11-15; queued in lane p5-agent after FIX-150, before P4-NARR) |
| P5-GUIDE-1 | Agent guides: package format (summary, guide files, recipes), linter, blobs, snapshot, renderer, `aurora plugin guide`, `docs/22` paragraph | D-170, D-171, D-295, C-640..C-642, C-646, C-647, C-649, SEC-117, SEC-91, one migration | L | Rust | — | `P5-GUIDE-1.md` | ready (spec merged 2026-11-15; wave 3: queued in lane p5-apps) |
| P5-GUIDE-2 | Agent guides: home sync `apps` part, `.aurora/apps/` projection, always-index instruction file, admin API, advisory guard | D-171, D-295, C-643..C-645, C-648, SEC-117, SEC-91 | L | Rust | P5-GUIDE-1 | `P5-GUIDE-2.md` | ready (spec merged 2026-11-15; wave 3: queued in lane p5-apps) |
| P5-GUIDE-3 | Dashboard Agent guide section (plugin page and review; recipes marked; plain-text viewer) | C-648, SEC-117 | S–M | Next.js | P5-GUIDE-2 (C-648) | `P5-GUIDE-3.md` | ready (spec merged 2026-11-15; wave 3: queued in lane p5-apps) |
| P5-GUIDE-4 | Built-in packages adopt summaries, essentials, guide files and recipes (Indico free-room booking first) | D-295 | M | package | P5-GUIDE-1 | `P5-GUIDE-4.md` | ready (spec merged 2026-11-15; wave 3: queued in lane p5-apps) |
| P5-GUIDE-5 | `aurora plugin import-openapi` (reference files, catalogue skeleton) | D-172, C-650 | M | Rust (CLI) | P5-GUIDE-1 | `P5-GUIDE-5.md` | ready (spec merged 2026-11-15; wave 3: queued in lane p5-apps) |
| P5-GUIDE-6 | `aurora-agent`: operations file, `api.json`, one in-sandbox command (app targets; reserved app id `aurora`), image wrapper, import writes `operations.json` | D-296, C-651..C-653, C-656, SEC-118, SEC-120 | L | Rust + package | P5-GUIDE-2, P5-GUIDE-5 | `P5-GUIDE-6.md` | ready (spec merged 2026-11-15; wave 3: queued in lane p5-apps) |
| P5-GUIDE-7 | Platform tools on demand: exposure/family, bridge lists core only, family index, target `aurora` of `aurora-agent`, `AURORA_SESSION`, `hosted.platform_tools` | D-297, C-654, C-655, C-657, SEC-119, SEC-120 | M–L | Rust | P5-GUIDE-6; not beside lanes editing `domains/tools/` | `P5-GUIDE-7.md` | ready (spec merged 2026-11-15; wave 3: queued in lane p5-apps) |

### Execution order (phase 5)

At most three implementation lanes at a time, disjoint ownership; a lane starts when a slot frees **and** its dependencies have merged. Coordinator docs work takes no lane. Full plan: `docs/tasks/PHASE5-PLAN.md`.

- **Wave 1** (2026-11-14..15), all merged: `p5-apps` FIX-147, FIX-133, FIX-136, FIX-141, FIX-144, FIX-145, FIX-149, FIX-134, FIX-135; `p5-agent` P5-AGENT-1, FIX-137, P5-AGENT-2, FIX-138, P5-CHAT-1, P5-CHAT-2, P5-CHAT-3, P5-CHAT-4 A+C; `p5-platform` P5-ADMDEL, OPS-021, FIX-142, P5-ASRC, P5-CERN-ADDONS (in-repo part).
- **Wave 2** (since 2026-11-15; `docs/tasks/PHASE5-PLAN.md` §Wave 2 lanes): `p5-apps` SET-1 → FIX-124 ‖ SET-2 → SET-3 ‖ SET-4 → SET-5 ‖ P4-LLM-6; `p5-agent` P5-CHAT-4 B ‖ P4-LLM-1 → 2 ‖ 3 → 4 → 5; `p5-platform` P5-CERN-ADDONS (external repo) ‖ FIX-153 → FIX-154 (owner 2026-11-15) ‖ FIX-146 ‖ P5-FILES-1 → 2 → 3 → 4 (owner 2026-11-15, D-316: before P4-SSH; FILES-1 builds the `exec.*` engine P4-SSH-2 extends) ‖ P4-SSH-1 → P4-SSH-2. Then, as a lane frees: P4-011B-1 → {2 ∥ 3} → 4 → 5.
- **Wave 3 queue** (placed 2026-11-15; `docs/tasks/PHASE5-PLAN.md` §Wave 3 queue): `p5-apps` P5-GUIDE-1 → {2 ∥ 4 ∥ 5} → 3 → FIX-151 → 6 → 7 (7 only after FIX-150, P4-UTOK and P4-EVAL-2 are merged: `domains/tools/`) ‖ P5-ICS → P5-CALDAV; `p5-agent` FIX-150 → FIX-158 → P4-NARR → P4-UTOK ‖ P4-EVAL-1 → {2 ∥ 4} → 3; `p5-platform` P4-011B-1 → {2 ∥ 3} → 4 → 5 ‖ P5-CALINV (P4-S3 moved to phase 6, D-318).
- **Not placed yet**: P5-UX-1..15, P5-IA-1..5 and FIX-159 (spec on branch `alexiri/p5-ux`, awaiting the owner's merge; order in §Open questions). Meanwhile SET-4/5 add no sidebar entries (D-340).
- **No spec yet**: none in phase 5 (P4-NARR, P4-UTOK and P4-EVAL specified 2026-11-15; P5-SLACK moved to phase 6, D-285).

### Exit criteria (phase 5)

In the spirit of `docs/audit/phase0-exit.md`: one commit, a fresh `target/`, every command's exit code and result recorded, defects fixed as separate tasks and re-run. The phase-5 exit audit's scope is not defined yet; at least the items 4–11 of the earlier audits (`docs/history/phase-1-2.md` §Phase exit criteria) on the phase-5 commit, and every SEC invariant added for phase 5 (SEC-86 and SEC-88 onward, for what was built) proven. There is no multi-arch criterion: OPS-022 is cancelled (D-318).

## Next phase: 6 (proposed)

### Phase 6 tasks (proposed)

Owner, 2026-11-14: "It's time to start thinking about phase 6. We won't start working on this just yet, but it's good to have some ideas waiting in the wings." Two proposal drafts (merged 942a241c; a third, P6-MODELS, was moved to phase 5 by the owner, D-302(f), and specified as P5-MODELS-1..3); the owner's answers so far are D-276; no C-, SEC- or migration numbers taken (the spec batches take them from §Number allocation when phase 6 starts). Summary in `21 §Phase 6`, `19 §Phase 6`. The implementation briefs named below are **proposed splits only** (no brief files, no rows counted) until the spec batches are approved.

| Task | Scope (one line) | Spec sources | Size | Lane | Depends on | Owns | Brief | Status |
|---|---|---|---|---|---|---|---|---|
| P6-ORGTREE | Nested organizations (owner: "an org per department, with suborgs per group"): `parent_org_id` + closure table, depth cap 4; upward-closed membership with cascading removal (owner, D-276(a)), ancestor owners/admins implicit admins below, membership not flowing down; governance, org policy (strictest verdict over every org on the path), retention, availability, freeze and limits inherited tighten-only; credentials/libraries/registrations shared down only by a flag; SCIM at roots; create/move (dry-run impact)/delete children; tree switcher, breadcrumbs, "Inherited from" labels. Proposed briefs P6-ORG-1..5 | owner 2026-11-14; `04`, `07 §Evaluation`, `08 §11` | spec batch M–L (Opus), then ~5 briefs (2 L Rust, 1 M–L Rust, 1 L Next.js, 1 M both) | coordinator | owner answers Q-P6-ORG-2..8 (Q-P6-ORG-1 answered, D-276); phase-6 start | `04`, `03`, `05`, `07`, `08 §11`, `10`, `13`, `14`, `15`, `17`, `18`, `API_CONTRACTS` (at spec time) | `docs/tasks/P6-ORGTREE.md` (proposal) | not started (proposal; waiting for the owner) |
| P6-LIMITS | Usage limits and load knobs (owner: "runtime of sandboxes, number of schedules, rate limiting of expensive endpoints … knobs to turn when load is too high"): one quota framework `(resource, scope, window, value)` with instance defaults (settings section `quotas`), limit profiles `standard`/`trusted`/`restricted` for orgs and users, org admins tighten-only; abuse limits only, no billing (D-276(b)); **core (D-276(c), "long jobs on free compute")**: background vs interactive sandbox time, concurrent background sandboxes, per-profile background cap and background-hour budgets, only a person's engagement restarts the cap (proposed, Q-P6-LIM-10); an awake-hours hard ceiling from `runtime_intervals`, per-profile sandbox deadlines/sizes; schedule/webhook/watch counts per workspace/user/org, a profile cron floor applied at fire time, fires and turns per day; gateway request/egress-byte/LLM-request caps; API classes `sandbox`/`probe`/`invite` + per-replica concurrency caps; instance pressure level `normal`/`elevated`/`critical` (automations and background starts deferred before chat; live + CLI break-glass); Admin/Org/My usage; abuse signals report-only. Proposed briefs P6-LIM-1..7 | owner 2026-11-14; `02 §Limits registry`, `05 §Conventions`, `10`, `11`, `15 §Instance settings` | spec batch L (Opus), then ~7 briefs (2 L Rust, 1 M Rust, 1 M–L Rust, 1 M both, 1 L Next.js, 1 M both) | coordinator | owner answers Q-P6-LIM-1, 3..8, 10 (Q-P6-LIM-2 and 9 answered, D-276); production usage snapshot (Q-P6-LIM-5); phase-6 start | `02`, `03`, `04`, `05`, `06`, `10`, `11`, `14`, `15`, `17`, `18`, `API_CONTRACTS` (at spec time) | `docs/tasks/P6-LIMITS.md` (proposal) | not started (proposal; waiting for the owner) |
| P4-S3 | S3-compatible `BlobStore` beside Postgres for attachment bytes, `aurora-api blobs migrate` (moved from phase 5 by the owner 2026-11-15, D-318; already specified and briefed) | D-120, SEC-63, `10 §Attachments`, `15 §S3-compatible blob store` | M–L | Rust | phase-6 start | `crates/aurora-blobstore`, per the brief | `P4-S3.md` | ready (brief written 2026-11-15; waits for the owner's phase-6 all-clear) |
| P6-SLACK | Slack connector (was P5-SLACK; owner 2026-11-15, D-285: not a priority): events and OAuth endpoints, the shared app, manifest-API provisioning, configuration-token rotation through `POST /v1/connector/credentials`, Block Kit rendering; nothing in the core changes (`21 §Other later items`) | `21`, D-78, D-285 | spec batch, then L | Rust | connector contract (`13`); phase-6 start | `13`, `20`-style connector doc (at spec time) | — | not started (no spec; waits for the owner's phase-6 all-clear) |

Proposed order (Q-P6-LIM-8): the limits framework (P6-LIM-1) together with the sandbox compute limits (P6-LIM-2, the core per D-276(c)) first — it is the more urgent protection — then the org tree, whose P6-ORG-1 adds ancestor counting to org-scoped limits.

## Number allocation

**Next free: FIX-160, D-343, C-980, SEC-131, migration `20261201000244`.** Edit this line in place when you take a number (check `git log -3`, `ls docs/tasks`, and this line right before committing; several sessions write briefs). Task briefs are `docs/tasks/<ID>.md`, named after the task id (`P<phase>-<NAME>` for phase features, `FIX-nnn` for fixes).

Live reservations (numbers inside a block that a lane has already used are recorded in its commits; unused blocks are released when the lane or task closes):

| Holder | Decisions | Contracts | Migrations | Other |
|---|---|---|---|---|
| lane `p5-apps` | D-242..D-244 (used), D-308..D-310 | C-742..C-749 | `20261201000161..169` | |
| lane `p5-agent` | D-248, D-249 (used), D-311..D-313 | C-751..C-759 | `20261201000173..179` | |
| lane `p5-platform` | D-251..D-254 | C-760..C-769 | `20261201000181..189` | |
| FIX-146 (request tracing) | D-255..D-259 | C-732 (used) | `20261201000150` | SEC-88 (used) |
| SET-1..SET-5 (spec merged) | D-236..D-238 (used) | C-770..C-784 (used) | `20261201000190..199` | SEC-89, SEC-90 (used) |
| P4-SSH-1/2 (spec merged) | D-239 (used) | C-790..C-799 (used) | `20261201000200..209` | SEC-92..SEC-96 (used) |
| P4-011B-1..5 (spec merged) | D-264..D-267 (used) | C-820..C-839 (used) | `20261201000220..223` | SEC-102 (used) |
| P4-EVAL-1..4 (spec merged) | D-289..D-294 (used) | C-860..C-874 (used) | `20261201000236..240` | SEC-114..SEC-116 (used) |
| P4-NARR / P4-UTOK (spec merged) | D-279..D-284 (used) | C-846..C-859 (used) | `20261201000231..233` (`234`, `235` released) | SEC-111..SEC-113 (used) |
| P5-CALINV | D-274 (used) | C-844 (used) | `20261201000230` | SEC-108 (used) |
| P5-WEBHOOK-1..4 (spec merged 2026-11-15; outgoing webhooks, owner D-320) | D-320 (used) | C-909..C-913 (used) | `20261201000243` | SEC-127 (used) |
| agent-page follow-ups (coordinator) | D-202..D-209 | C-701..C-709 | | |
| P5-GUIDE-1..7 (spec merged 2026-11-15; always-index, recipes, `aurora-agent`, platform tools on demand) | D-170..D-172, D-295..D-298 (used; D-299 released) | C-640..C-657 (used), C-658, C-659 | migration for P5-GUIDE-1 takes the next free number when placed | SEC-91, SEC-117..SEC-120 (used; SEC-117 replaces the branch's colliding SEC-90) |
| P5-FILES-1..4 (spec merged 2026-11-15; Files page over SFTP, owner D-316) | D-316 (used) | C-898..C-907 (used) | `20261201000241`, `…000242` | SEC-125, SEC-126 (used) |
| P5-UX-1..7 (spec on branch `alexiri/p5-ux`, 2026-11-15; first run and connecting, owner walkthrough) | D-321..D-328, D-337 (used) | C-914..C-927, C-955..C-959 (used; C-921 not built, D-328), C-928, C-929 | P5-UX-5's migration takes the next free number when placed | SEC-130, SEC-128 (used) |
| P5-UX-8..15 + FIX-159 (same branch, 2026-11-15; app setup, owner's second walkthrough) | D-329..D-336 (used) | C-930..C-949 (used), C-950..C-954 | P5-UX-14's migration takes the next free number when placed | SEC-129 (used) |
| P5-IA-1..5 (same branch, 2026-11-15; one structure for every level, owner D-338) | D-338..D-342 (used) | C-960..C-975 (used), C-976..C-979 | none | none |
| P5-MODELS-1..3 (spec merged 2026-11-15; the harness chooses models, owner D-302) | D-302..D-304 (used) | C-875..C-884 (used), C-885..C-889 | migrations for P5-MODELS-1 and -2 take the next free numbers when placed | SEC-122, SEC-123 (used) |

## Open questions for the owner

- **Phase 5:** review of the coordinator defaults recorded in D-268.
- **Phase 5, P5-UX (spec on branch `alexiri/p5-ux`):** merge the spec batch? (Q-UX-1 and Q-UX-2 answered, D-328.) Suggested order once merged (one coherent design, D-342):
1. **FIX-159 first** (high priority: Simulate can show a false allow).
2. **Structure:** **P5-IA-1** (shell and routes) before every brief that touches pages or navigation (P5-UX-1 included). Then P5-IA-2 (ladder, Internet access) → P5-IA-3 → P5-IA-4 → P5-IA-5, sequential because they share the settings form components. P5-IA-4/5 are coordinated with SET-4/5 on the settings form files.
3. **Backend, independent:** P5-UX-2 → P5-UX-8, P5-UX-4, P5-UX-14, P5-UX-12/-13/-15. These are serialized where they share builtin `plugin.yaml` files (12, 13, 15) and the probe code (14 with 2).
4. **Screens after P5-IA-1:**
   - P5-UX-1 (refusals and empty states; navigation is P5-IA-1's);
   - P5-UX-3 after P5-UX-2;
   - P5-UX-7 after P5-UX-3/-4 and with P5-IA-2;
   - P5-UX-5 after P5-UX-1;
   - P5-UX-6 after P5-UX-2 and P5-IA-4;
   - P5-UX-9 after P5-UX-8;
   - P5-UX-10 after FIX-159 and P5-IA-2;
   - P5-UX-11 after P5-UX-3.

Shared files to serialize:
- `connections-tab.tsx`: P5-UX-4, -11, -14.
- `getting-started.tsx`: P5-UX-1 → -3 → -5.
- `apps-tab.tsx`: P5-UX-3 → -9.
- Builtin `plugin.yaml` files: P5-UX-2, -12, -13, -15.
- Settings form components: P5-IA-3 → -4 → -5 and SET-4/5.

The Orchestrator places the lanes.
- **Phase 6 (proposals):** Q-P6-ORG-2..8 in `docs/tasks/P6-ORGTREE.md` (membership flowing down? credentials shared down by flag? SCIM at roots? parent told of a child's critical events? subtree delete? placement by claim? depth 4?) and Q-P6-LIM-1, 3..8, 10 in `docs/tasks/P6-LIMITS.md` (limit profiles? interactive after the budget? LLM request counts only? defaults from production usage? manual pressure? report-only abuse signals? limits before the tree? only a person's engagement restarts the background cap?). (The P6-MODELS questions were answered 2026-11-15, D-302.) Each has a recommendation; nothing blocks until phase 6 starts. Answered so far: D-276.

## Open security findings

None open. Closed findings are in the history files (REVIEW-5's in `docs/history/phase-4.md`).

## Blockers

None. FIX-017 (phase 2: the real-cluster Helm install and real-key signing) stays with the owner.

## Standing notes (not blockers)

- Workspace `Cargo.toml` licenses all first-party crates `GPL-3.0-or-later` (owner's choice — do not change). `cargo deny`'s permissive-only allowlist applies to third-party deps only; our crates are `publish = false` and skipped via `private = { ignore = true }`, so the check still passes.
- Perf budgets run only on an idle machine; after AUDIT-004 they are still to be run (recorded "not yet measured").
