# Implementation progress

Last updated: 2026-11-15, master after `1a4088ed`. **Phase 5 is running**: wave 1 is merged; wave 2 started 2026-11-15 in the same three lanes (`p5-apps`: SET-1 → FIX-124 → SET-2..5 → P4-LLM-6; `p5-agent`: P5-CHAT-4 B → P4-LLM-1..5; `p5-platform`: P5-CERN-ADDONS external repo (owner's GitHub side done, D-278) → FIX-146 → P4-SSH-1/2; §Execution order). Phase 6 stays closed until the owner's all-clear (D-278). Phases 0–4 are closed; their record is in `docs/history/`. **Phase 6 is proposed only** (P6-ORGTREE, P6-LIMITS; owner answers so far: D-276) and starts on the owner's word.

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
| 5 | 69 | 37 | 3 (P5-CERN-ADDONS (external repo), P5-CHAT-4 B, SET-1) | 23 | 3 (no spec yet: P4-EVAL, P4-NARR, P4-UTOK) | 3 (OPS-022 deferred; FIX-049 moved to phase 4; P5-SLACK moved to phase 6, D-285) | §Phase 5 tasks (TEST-021 and FIX-057, done, are counted here but their rows are in `docs/history/phase-0.md`'s task graph) |
| 6 (proposed) | 3 | 0 | 0 | 0 | 3 (P6-ORGTREE, P6-LIMITS: proposals awaiting the owner; P6-SLACK, moved from phase 5, D-285) | 0 | §Phase 6 tasks (proposed) |
| **All** | **390** | **348** | **4** | **23** | **6** | **9** | |

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

Lane order, waves and number blocks: `docs/tasks/PHASE5-PLAN.md`. Lanes `p5-apps`, `p5-agent`, `p5-platform`. Scope, ownership and acceptance live in each brief; the full status notes written before 2026-11-15 are kept in `docs/history/phase-5.md`. Status values: `done (merged <sha>)` · `in-progress (lane <lane>; committed <sha> on alexiri/<lane>, not merged)` · `in-progress (lane <lane>; started)` · `ready` (brief written; "queued in lane X" when it is in a running lane's chain) · `not started` (no brief yet) · `deferred` / `moved`.

| Task | Scope (one line) | Refs | Size | Lane | Depends on | Brief | Status |
|---|---|---|---|---|---|---|---|
| OPS-022 | Multi-arch images (amd64 + arm64) | `15 §Images` | M | ops | OPS-020 | `OPS-022.md` | deferred (owner 2026-11-07) |
| OPS-021 | Dependabot, grouped, with ignore rules and a review policy | owner | S | ops | OPS-019, OPS-020 | `OPS-021.md` | done (merged 3c983224) |
| P5-SLACK | Slack connector | `21`, D-78 | L | Rust | connector contract (`13`) | — | moved to phase 6 as P6-SLACK (owner 2026-11-15, D-285: not a priority) |
| P5-ASRC | Addon sources: third-party package repositories (signed repo manifest, TOFU key, polling, update badges) | D-250, C-590..C-609, SEC-86, migration 000180 | L | both | — | `P5-ASRC.md` | done (merged 3c983224) |
| P5-CERN-ADDONS | `aurora-addons-cern` repo (release script, CI, signed releases) with `cern-phonebook` (LDAP through the gateway LDAP lane) and `cern-maps` | D-235(d), D-251 | M–L | package + Rust | P5-ASRC | `P5-CERN-ADDONS.md` | in-progress (lane p5-platform; started: external repo content, cern-maps, live xldap check, kind e2e, first `v*` release; in-repo part merged 555f6058; D-277, D-278) |
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
| P4-S3 | S3-compatible `BlobStore` beside Postgres, `aurora-api blobs migrate` | D-120, SEC-63 | M–L | Rust | — | `P4-S3.md` | ready (brief written 2026-11-15; not yet placed: the next free lane) |
| P4-NARR | Live narration cards | `21`, D-225 | M | Rust | a spec task | needed | not started (spec needed) |
| P4-UTOK | User-token features (`user_tokens` capability) | `21`, D-225 | M | Rust | a spec task | needed | not started (spec needed) |
| P4-EVAL | Evals | `21 §Phase 4 — Evals` | L | both | a spec task | needed | not started (spec needed) |
| P4-LLM-1 | LLM provider framework; `openai`, `openai_compatible`; `anthropic` inference path set | D-260, D-268(d), C-800.. | L | Rust | — | `P4-LLM-1.md` | ready (queued in lane p5-agent after P5-CHAT-4 B; order 1 → 2 → 3 → 4 → 5; 6 in p5-apps after SET-3) |
| P4-LLM-2 | Gateway LLM mint service + Google Vertex AI | D-261, SEC-97 | L | Rust | P4-LLM-1 | `P4-LLM-2.md` | ready (queued in lane p5-agent) |
| P4-LLM-3 | `azure_openai` (keys and Entra) | D-261, D-268(c) | M | Rust | P4-LLM-2 | `P4-LLM-3.md` | ready (queued in lane p5-agent) |
| P4-LLM-4 | `bedrock` (assume-role, IAM keys, API keys; SigV4 in the gateway) | D-261, D-268(c) | L | Rust | P4-LLM-2 | `P4-LLM-4.md` | ready (queued in lane p5-agent) |
| P4-LLM-5 | Harnesses on the new providers + hosted e2e (no key reaches the sandbox) | D-260, SEC-64, SEC-66 | M | Rust | P4-LLM-1 (e2e after 2–4) | `P4-LLM-5.md` | ready (queued in lane p5-agent) |
| P4-LLM-6 | Endpoint destinations + dashboard fields for providers and auth modes | D-262, SEC-98 | M–L | both | SET-3, P4-LLM-1 | `P4-LLM-6.md` | ready (queued in lane p5-apps) |
| P4-011B-1 | Gateway parking and single-use approval grants | D-264, D-269, SEC-74 | L | Rust | — | `P4-011B-1.md` | ready (not yet placed: the next free lane; order 1 → {2 ∥ 3} → 4 → 5) |
| P4-011B-2 | Deciding parked approvals; resume follow-up; `approvals.park.*` settings | D-264, D-265, C-822.. | L | Rust | P4-011B-1 | `P4-011B-2.md` | ready |
| P4-011B-3 | Web Push backend (VAPID, subscriptions, encrypted pushes) | D-266, D-269(c), SEC-75/76 | L | Rust/ops | P4-011B-1 | `P4-011B-3.md` | ready |
| P4-011B-4 | Dashboard: service worker, push opt-in, parked-approval states | D-266, D-267, SEC-102 | M–L | Next.js | P4-011B-2, -3 | `P4-011B-4.md` | ready |
| P4-011B-5 | Park-and-resume and Web Push end to end, operator pages | D-133, D-134, D-269(d) | M | tests/ops | P4-011B-1..4 | `P4-011B-5.md` | ready |
| P4-SSH-1 | User SSH keys, SSH CA, certificate mint, `aurora ssh` | D-239, SEC-93 | M | both | — | `P4-SSH-1.md` | ready (queued in lane p5-platform after FIX-146) |
| P4-SSH-2 | `aurora-terminator` (certificate-only SSH into the sandbox; no port forwarding) | D-239, D-268(a)(b), SEC-92/94..96 | L | Rust/ops | P4-SSH-1 | `P4-SSH-2.md` | ready (queued in lane p5-platform) |
| SPEC-SETTINGS | Instance settings move from Helm into the app (Helm-set values locked) — spec batch | D-195, D-236..D-238, SEC-89/90 | L | docs | — | `SPEC-SETTINGS.md` | done (merged 76bb566a) |
| SET-1 | Settings framework: registry, Helm locks, storage, live reload, audit, export/import, CLI, Admin › Settings | D-236 | L | both | — | `SET-1.md` | in-progress (lane p5-apps; started; then FIX-124 → SET-2..SET-5) |
| SET-2 | Governance and BYO floors as live settings | D-237 | M | both | SET-1, FIX-124 | `SET-2.md` | ready (queued in lane p5-apps) |
| SET-3 | LLM endpoints as live settings | D-237 | M | both | SET-2 | `SET-3.md` | ready (queued in lane p5-apps) |
| SET-4 | Sign-in, OIDC providers and Email editable | D-235(b), D-237 | M–L | both | SET-1 | `SET-4.md` | ready (queued in lane p5-apps) |
| SET-5 | Agents, approvals, retention, skills, SCIM, previews, terminal and limits as settings | D-235(a), D-237 | L | both | SET-1 | `SET-5.md` | ready (queued in lane p5-apps) |
| FIX-124 | Admin › Secret stores editable (unless set in Helm), value migration job | D-195 | M–L | both | SET-1 | `FIX-124.md` | ready (queued in lane p5-apps) |
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
| P5-CHAT-4 | Message search: API (A), chat-panel search (B), agent conversation tools (C) | D-229, D-230, C-727, C-728, migration 000172 | M–L | Rust then Next.js | A after P5-CHAT-1; B after P5-CHAT-2 | `P5-CHAT-4.md` | in-progress (lane p5-agent; started B; A + C merged ed41c9ed) |
| FIX-142 | Agent messages keep their Markdown on chat platforms; every Mattermost ping accounted for | D-226, D-227, D-270, D-271 | M | Rust | — | `FIX-142.md` | done (merged 3c983224) |
| FIX-144 | Self-hosted host check says why it failed; agents told each server's address | C-730 | S–M | both | — | `FIX-144.md` | done (merged d1933319) |
| FIX-145 | Self-hosted apps served under a path (`base_path`) | D-231, C-731, migration 000140 | M–L | both | FIX-144 | `FIX-145.md` | done (merged d1933319) |
| FIX-146 | Request tracing end to end, browser to database | D-232, C-732, SEC-88, migration 000150 | L | both | spec done (c8aa4cb5) | `FIX-146.md` | ready (queued in lane p5-platform after P5-CERN-ADDONS) |
| FIX-147 | One permissions editor (`GrantForm`, `PermissionGroups`) | D-233 | M | Next.js | — | `FIX-147.md` | done (merged 787d5c02) |
| FIX-149 | Connect/Reconnect as real dialogs; OAuth result over `BroadcastChannel` | D-272, C-842 | M | both | — | `FIX-149.md` | done (merged b9c3046f) |
| P5-ADMDEL | Instance admins delete any organization or workspace | D-234, C-733 | M | both | — | `P5-ADMDEL.md` | done (merged 3c983224) |
| P5-ICS | Generic "Calendar link (ICS)" app: secret-link credential, gateway fetches the stored URL | D-273, C-843, SEC-107 | M–L | Rust + package | p5-apps hot files | `P5-ICS.md` | ready (not yet placed) |
| P5-CALINV | Agents add calendar events by e-mailing the person an invitation | D-274, C-844, SEC-108, migration 000230 | M | Rust | instance mailer | `P5-CALINV.md` | ready (not yet placed) |
| P5-CALDAV | Read and write any CalDAV server with the person's own app password | D-275, C-845, SEC-109 | L | Rust + package | P5-ICS (same lane) | `P5-CALDAV.md` | ready (not yet placed) |

### Execution order (phase 5)

At most three implementation lanes at a time, disjoint ownership; a lane starts when a slot frees **and** its dependencies have merged. Coordinator docs work takes no lane. Full plan: `docs/tasks/PHASE5-PLAN.md`.

- **Wave 1** (2026-11-14..15), all merged: `p5-apps` FIX-147, FIX-133, FIX-136, FIX-141, FIX-144, FIX-145, FIX-149, FIX-134, FIX-135; `p5-agent` P5-AGENT-1, FIX-137, P5-AGENT-2, FIX-138, P5-CHAT-1, P5-CHAT-2, P5-CHAT-3, P5-CHAT-4 A+C; `p5-platform` P5-ADMDEL, OPS-021, FIX-142, P5-ASRC, P5-CERN-ADDONS (in-repo part).
- **Wave 2** (since 2026-11-15; `docs/tasks/PHASE5-PLAN.md` §Wave 2 lanes): `p5-apps` SET-1 → FIX-124 ‖ SET-2 → SET-3 ‖ SET-4 → SET-5 ‖ P4-LLM-6; `p5-agent` P5-CHAT-4 B ‖ P4-LLM-1 → 2 ‖ 3 → 4 → 5; `p5-platform` P5-CERN-ADDONS (external repo) ‖ FIX-146 ‖ P4-SSH-1 → P4-SSH-2. Then, as a lane frees: P4-011B-1 → {2 ∥ 3} → 4 → 5.
- **Not yet placed**: P4-011B-1..5 (the next free lane); P4-S3 (brief written 2026-11-15); P5-ICS → P5-CALDAV (same lane: registry, gateway decision, home sync), P5-CALINV (disjoint from the `p5-apps` hot files).
- **No spec yet**: P4-EVAL, P4-NARR, P4-UTOK (P5-SLACK moved to phase 6, D-285) — after the P4-LLM/P4-SSH/P4-011B batches (D-235(c)); their order is an open owner question. OPS-022 deferred.

### Exit criteria (phase 5)

In the spirit of `docs/audit/phase0-exit.md`: one commit, a fresh `target/`, every command's exit code and result recorded, defects fixed as separate tasks and re-run. The phase-5 exit audit's scope is not defined yet; at least the items 4–11 of the earlier audits (`docs/history/phase-1-2.md` §Phase exit criteria) on the phase-5 commit, and every SEC invariant added for phase 5 (SEC-86 and SEC-88 onward, for what was built) proven. OPS-022's multi-arch criterion is moot while OPS-022 is deferred.

## Next phase: 6 (proposed)

### Phase 6 tasks (proposed)

Owner, 2026-11-14: "It's time to start thinking about phase 6. We won't start working on this just yet, but it's good to have some ideas waiting in the wings." Two proposal drafts (merged 942a241c); the owner's answers so far are D-276; no C-, SEC- or migration numbers taken (the spec batches take them from §Number allocation when phase 6 starts). Summary in `21 §Phase 6`, `19 §Phase 6`. The implementation briefs named below are **proposed splits only** (no brief files, no rows counted) until the spec batches are approved.

| Task | Scope (one line) | Spec sources | Size | Lane | Depends on | Owns | Brief | Status |
|---|---|---|---|---|---|---|---|---|
| P6-ORGTREE | Nested organizations (owner: "an org per department, with suborgs per group"): `parent_org_id` + closure table, depth cap 4; upward-closed membership with cascading removal (owner, D-276(a)), ancestor owners/admins implicit admins below, membership not flowing down; governance, org policy (strictest verdict over every org on the path), retention, availability, freeze and limits inherited tighten-only; credentials/libraries/registrations shared down only by a flag; SCIM at roots; create/move (dry-run impact)/delete children; tree switcher, breadcrumbs, "Inherited from" labels. Proposed briefs P6-ORG-1..5 | owner 2026-11-14; `04`, `07 §Evaluation`, `08 §11` | spec batch M–L (Opus), then ~5 briefs (2 L Rust, 1 M–L Rust, 1 L Next.js, 1 M both) | coordinator | owner answers Q-P6-ORG-2..8 (Q-P6-ORG-1 answered, D-276); phase-6 start | `04`, `03`, `05`, `07`, `08 §11`, `10`, `13`, `14`, `15`, `17`, `18`, `API_CONTRACTS` (at spec time) | `docs/tasks/P6-ORGTREE.md` (proposal) | not started (proposal; waiting for the owner) |
| P6-LIMITS | Usage limits and load knobs (owner: "runtime of sandboxes, number of schedules, rate limiting of expensive endpoints … knobs to turn when load is too high"): one quota framework `(resource, scope, window, value)` with instance defaults (settings section `quotas`), limit profiles `standard`/`trusted`/`restricted` for orgs and users, org admins tighten-only; abuse limits only, no billing (D-276(b)); **core (D-276(c), "long jobs on free compute")**: background vs interactive sandbox time, concurrent background sandboxes, per-profile background cap and background-hour budgets, only a person's engagement restarts the cap (proposed, Q-P6-LIM-10); an awake-hours hard ceiling from `runtime_intervals`, per-profile sandbox deadlines/sizes; schedule/webhook/watch counts per workspace/user/org, a profile cron floor applied at fire time, fires and turns per day; gateway request/egress-byte/LLM-request caps; API classes `sandbox`/`probe`/`invite` + per-replica concurrency caps; instance pressure level `normal`/`elevated`/`critical` (automations and background starts deferred before chat; live + CLI break-glass); Admin/Org/My usage; abuse signals report-only. Proposed briefs P6-LIM-1..7 | owner 2026-11-14; `02 §Limits registry`, `05 §Conventions`, `10`, `11`, `15 §Instance settings` | spec batch L (Opus), then ~7 briefs (2 L Rust, 1 M Rust, 1 M–L Rust, 1 M both, 1 L Next.js, 1 M both) | coordinator | owner answers Q-P6-LIM-1, 3..8, 10 (Q-P6-LIM-2 and 9 answered, D-276); production usage snapshot (Q-P6-LIM-5); phase-6 start | `02`, `03`, `04`, `05`, `06`, `10`, `11`, `14`, `15`, `17`, `18`, `API_CONTRACTS` (at spec time) | `docs/tasks/P6-LIMITS.md` (proposal) | not started (proposal; waiting for the owner) |

| P6-SLACK | Slack connector (was P5-SLACK; owner 2026-11-15, D-285: not a priority): events and OAuth endpoints, the shared app, manifest-API provisioning, configuration-token rotation through `POST /v1/connector/credentials`, Block Kit rendering; nothing in the core changes (`21 §Other later items`) | `21`, D-78, D-285 | spec batch, then L | Rust | connector contract (`13`); phase-6 start | `13`, `20`-style connector doc (at spec time) | — | not started (no spec; waits for the owner's phase-6 all-clear) |

Proposed order (Q-P6-LIM-8): the limits framework (P6-LIM-1) together with the sandbox compute limits (P6-LIM-2, the core per D-276(c)) first — it is the more urgent protection — then the org tree, whose P6-ORG-1 adds ancestor counting to org-scoped limits.

## Number allocation

**Next free: FIX-150, D-286, C-860, SEC-114, migration `20261201000236`.** Edit this line in place when you take a number (check `git log -3`, `ls docs/tasks`, and this line right before committing; several sessions write briefs). Task briefs are `docs/tasks/<ID>.md`, named after the task id (`P<phase>-<NAME>` for phase features, `FIX-nnn` for fixes).

Live reservations (numbers inside a block that a lane has already used are recorded in its commits; unused blocks are released when the lane or task closes):

| Holder | Decisions | Contracts | Migrations | Other |
|---|---|---|---|---|
| lane `p5-apps` | D-242..D-244 | C-742..C-749 | `20261201000161..169` | |
| lane `p5-agent` | D-248, D-249 | C-751..C-759 | `20261201000173..179` | |
| lane `p5-platform` | D-251..D-254 | C-760..C-769 | `20261201000181..189` | |
| FIX-146 (request tracing) | D-255..D-259 | C-732 (used) | `20261201000150` | SEC-88 (used) |
| SET-1..SET-5 (spec merged) | D-236..D-238 (used) | C-770..C-784 (used) | `20261201000190..199` | SEC-89, SEC-90 (used) |
| P4-SSH-1/2 (spec merged) | D-239 (used) | C-790..C-799 (used) | `20261201000200..209` | SEC-92..SEC-96 (used) |
| P4-011B-1..5 (spec merged) | D-264..D-267 (used) | C-820..C-839 (used) | `20261201000220..223` | SEC-102 (used) |
| SPEC-NARR-UTOK (P4-NARR + P4-UTOK spec batch, coordinator) | D-279..D-284 | C-846..C-859 | `20261201000231..235` | SEC-111..SEC-113 |
| P5-CALINV | D-274 (used) | C-844 (used) | `20261201000230` | SEC-108 (used) |
| agent-page follow-ups (coordinator) | D-202..D-209 | C-701..C-709 | | |
| branch `alexiri/p5-guide` (unmerged) | D-170..D-172 | C-640..C-659 | | SEC-91 (its SEC-90 collides with SPEC-SETTINGS: renumber at merge) |

## Open questions for the owner

- **Phase 5:** the order of the specs for P4-EVAL, P4-NARR and P4-UTOK (D-235(c) puts them after the three spec batches without ordering them). **Coordinator default while the owner has not answered** (owner 2026-11-15: keep going through every pending phase-5 task): P4-NARR + P4-UTOK as one spec batch (both connector-side, `13`/`20`), then P4-EVAL; each batch's owner questions are asked before its briefs are placed. Also: review of the coordinator defaults recorded in D-268.
- **Phase 6 (proposals):** Q-P6-ORG-2..8 in `docs/tasks/P6-ORGTREE.md` (membership flowing down? credentials shared down by flag? SCIM at roots? parent told of a child's critical events? subtree delete? placement by claim? depth 4?) and Q-P6-LIM-1, 3..8, 10 in `docs/tasks/P6-LIMITS.md` (limit profiles? interactive after the budget? LLM request counts only? defaults from production usage? manual pressure? report-only abuse signals? limits before the tree? only a person's engagement restarts the background cap?). Each has a recommendation; nothing blocks until phase 6 starts. Answered so far: D-276.

## Open security findings

None open. Closed findings are in the history files (REVIEW-5's in `docs/history/phase-4.md`).

## Blockers

None. FIX-017 (phase 2: the real-cluster Helm install and real-key signing) stays with the owner.

## Standing notes (not blockers)

- Workspace `Cargo.toml` licenses all first-party crates `GPL-3.0-or-later` (owner's choice — do not change). `cargo deny`'s permissive-only allowlist applies to third-party deps only; our crates are `publish = false` and skipped via `private = { ignore = true }`, so the check still passes.
- Perf budgets run only on an idle machine; after AUDIT-004 they are still to be run (recorded "not yet measured").
