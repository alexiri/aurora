# Implementation progress

Last updated: 2026-11-06 (evening, orchestrator; reconciled with git through e65ec66d)

Scope: **Phase 0 (M0–M7)**. Phases 1–4 are out of scope until phase 0 exits.
Ids per decision I-01. Task briefs live in `docs/tasks/<ID>.md`. Milestones
beyond the active one are tracked as epics and decomposed into briefs when
their dependencies near completion (adaptive planning).

## Environment

- **Phase sequencing** (owner, 2026-11-06): phase 0 must land fully and be **merged to `master`** first. After that, phases **1 and 2 may overlap**. Phases **3 and 4** start once 1 and 2 are done.

- **Concurrency**: owner set **3 concurrent implementation lanes** (2026-11-06 evening; was 4). Lanes already running finish; no new lane starts while 3 are active.
- **Worktrees** (2026-11-06, owner): implementation lanes that touch a crate another lane is editing (esp. `services/aurora-api`) run in their own git worktree (Orca worker `--worktree new-top-level --repo id:706ffb9e-ae8b-4f11-a841-518970480f40`, or Agent `isolation: worktree`) with a per-worktree `target/` (never a shared `CARGO_TARGET_DIR`: e2e scripts run `target/debug/<bin>`). Orchestrator merges each branch into `phase0` after review. Disk budget: ~15 GB per worktree build, so at most 3 worktrees while free space is under 100 GB; check `df -h /home` before starting one.

- Rust 1.99 stable (system, no rustup); `wasm32-wasip2` installed (`rust-wasm`, I-02a).
- Node 20.20, pnpm 10 (`~/.local/bin`), podman 5 (compose), psql client.
- Dev Postgres 16: container `aurora-dev-pg`, `postgres://aurora:aurora@127.0.0.1:55432/aurora` (`.env`, I-05).
- sqlx-cli 0.8.6 at `~/.cargo/bin/sqlx` (not on PATH; use full path).
- Disk: ~130 GB free on `/home` (2026-11-06 evening, after `cargo clean` of the main tree). Each worktree has its own `target/`; remove worktrees after merge.

## Milestone status

| Milestone | Status | Acceptance evidence |
|---|---|---|
| M0 | done (CI never executed on GitHub — no remote) | schema guard, compose up, guards, cargo-deny |
| M1 | done | permission matrix enforced on reads (SEC-FIX-03, I-27 + grant-candidate exception); 0 inconclusive rows; `scripts/e2e/m1.sh` Playwright sign-up → workspace (Daneel) → invite → accept: pass; OIDC (BE-015) done; permission_matrix.rs |
| M2 | done (core) — follow-ups FUP-M2 | `scripts/e2e/gateway.sh` 11 scenarios pass; governance SEC-35..39 service tests (GW-005, TEST-020); hold tests incl. two replicas |
| M3 | done (core) — follow-ups FUP-M3 | plugin lifecycle/ingestion, apps/connect/grants, hook host + WIT + SDK, CLI plugin, fixtures 33/33 (`scripts/fixtures/build.sh`), e2e-gateway 15/15 incl. hooks |
| M4 | done (core) — follow-ups FUP-M4; sandbox env gap (FUP-M7 item from RUN-004) | `AURORA_TEST_PODMAN=1 bash scripts/e2e/hosted.sh` 9/9 (scenarios 1–8 + 8b SEC probes) after SEC-FIX-01/02 |
| M5 | in progress: all backend features done (memory, skills, schedules, webhooks, processes/watches, attachments, action approvals, links, peer tasks, app-turn cap) and their UI (FE-050/051); sandbox env/stubs (FIX-002) in progress; apps tools, TEST-014 todo; live conformance blocked (no LiteLLM endpoint) | schedules 5, webhooks 11, memory 7, skills 5 |
| M6 | in progress: aurora-channels (CHAN-001), connector contract + SDK + fake connector (CON-001), API-006 contract done; BE-070 done, BE-071 in progress, TEST-015 conformance done; BE-072..075, FE-060, TEST-016 todo | — |
| M7 | in progress: Helm chart (OPS-010), kubernetes backend (RUN-004), kind e2e (OPS-011: `bash scripts/e2e/kind.sh --fresh` 4/4 incl. SEC-02/SEC-09 with enforced NetworkPolicy), operator docs (OPS-012/013) done; perf budgets + upgrade test (OPS-014), FIX-003 todo | k8s_mock 15 |

## Requirement coverage

Per-requirement status and evidence live in `docs/FEATURES.md` (reconciled 2026-11-06 evening). Final audit maps every requirement and SEC invariant to evidence.

## Task graph

Legend: done = acceptance evidence recorded; partial = code in tree, agent paused (usage limit) — resume.

| Task ID | Summary | Stack | Status | Dependencies | Owner/model | Verification / evidence |
|---|---|---|---|---|---|---|
| OPS-001..003, BE-001..008, TEST-001, FE-001 | M0 foundations | all | done | — | mixed | see M0 rows above |
| API-001 | M1 contract | contract | done | — | Opus | API_CONTRACTS.md M1 |
| BE-010 | API foundation | Rust | done | BE-008 | Sonnet | 13 tests + CSRF regression |
| BE-012 | Tenancy + Daneel | Rust | done | BE-010 | Sonnet | tests/tenancy.rs |
| BE-011 | Auth flows + mailer | Rust | done | BE-012 | Sonnet | tests/auth.rs 13 |
| BE-016 | /v1/me, admin users, audit lists, bootstrap-admin | Rust | done | BE-011 | Sonnet | tests me/admin/audit 9; CLI run by orchestrator |
| BE-014 | API keys, CLI device flow, resolve fix | Rust | done | BE-012 | Sonnet | tests keys/cli; S-04 + revoke-kind fix by orchestrator |
| BE-013 | Invitations | Rust | done | BE-011 | Sonnet | tests/invitations.rs 5 (token never in responses); checks by orchestrator |
| BE-015 | OIDC + mock IdP | Rust | done | BE-011 | Sonnet | tests/oidc.rs |
| FE-011 | Dashboard shell | Next.js | done | BE-012 | Sonnet | 14 vitest; build; curl smoke |
| FE-010 | Auth pages + CSP proxy | Next.js | done | BE-011 | Sonnet | 22 vitest; CSP nonce/nosniff headers verified; /join on generated client |
| FE-013 | Account, workspace settings, admin users | Next.js | done | BE-014, BE-016 | Sonnet | |
| FE-012 | Org pages | Next.js | done | BE-013, FE-013 | Sonnet | |
| TEST-010 | Permission-matrix test | Rust test | done (b6dfde39; defects → SEC-FIX-03) | BE-013..016 | haiku→Sonnet | ~169 routes × 13 principals; coverage test vs openapi.json |
| TEST-011 | Playwright sign-up → workspace (Daneel) → invite → accept | e2e | done (`bash scripts/e2e/m1.sh` 1 passed, rerun by orchestrator) | FE-010..013 | nextjs-developer | |
| BE-020 | aurora-policy | Rust | done | BE-001 | Sonnet | 10 + 30-case corpus |
| BE-021 | aurora-secrets | Rust | done | BE-003 | Sonnet | 11 tests |
| BE-022 | aurora-llm providers | Rust | done | BE-002 | Sonnet | 19 tests |
| GW-001 | Gateway transport core | Rust | done | BE-008 | Sonnet | 22 tests; generate-ca verified |
| GW-002 | Credential resolver, invalidation hub, instances | Rust | done | GW-001 | Sonnet | 28 tests |
| GW-003 | Resolution, decision, injection, guidance, request log | Rust | done | GW-002 | Sonnet | pipeline 8 + policy_db |
| GW-004 | Approval holds + ApprovalSummary | Rust | done | GW-003 | Sonnet | hold 6 + summary 5 |
| API-002 | M2 contract | contract | done | — | Opus | API_CONTRACTS.md M2 |
| GOV-001 | Governance library | Rust | done (15 tests; I-17; from_stored) | BE-022 | Sonnet | |
| GW-005 | Gateway governance enforcement | Rust | done (15 governance tests + TEST-020 SEC-37/write-back) | GOV-001 | Sonnet | |
| BE-031 | Secrets + LLM keys API | Rust | done (tests/secrets.rs 8) | GOV-001 | Sonnet | |
| BE-032 | BYO agents API + credentials + container config | Rust | done (tests/agents.rs 5) | BE-016 | Sonnet | |
| BE-033 | Policy authoring, grants, simulate, effective | Rust | done (tests/policy.rs 5) | BE-031, BE-032 | Sonnet | |
| BE-030 | Approvals API + SSE + jobs + activity | Rust | done (approvals/activity/jobs tests) | GW-004 | Sonnet | |
| BE-034 | Governance API | Rust | done (tests/governance.rs 3) | GOV-001, BE-031 | Sonnet | |
| CLI-001 | aurora-client + CLI login/run/approvals watch | Rust | done (13 tests; token import design) | BE-030, BE-032 | Sonnet | |
| FE-020 | Credentials pages (secrets, LLM keys, proposals) | Next.js | done | BE-031 | Sonnet | 50 vitest; build |
| FE-023 | Agents list/create, agent page, BYO Connect | Next.js | done | BE-032 | Sonnet | 44 vitest; m1 e2e green |
| FE-021/022/024 | Policy editor + access tab; approvals page; governance pages | Next.js | done (72 vitest; build; m1 e2e green) | BE-033/030/034 | Sonnet | |
| TEST-020 | Gateway SEC-37 + origin write-back tests | Rust test | done | GW-005 | haiku→Opus fix | 2 tests |
| REG-003 | Snapshot compiler, claims, diff, index | Rust | done | REG-002 | Sonnet | 11 tests; I-18 |
| GW-010 | Gateway apps: snapshot, candidates, mechanisms | Rust | done | REG-003 | Sonnet | tests/apps.rs 10; SigV4 vector (I-19) |
| GW-011 | Token refresh/mint, reauth, stale retry, GW-010 follow-ups | Rust | done (refresh/CAS/mint tests; I-20) | GW-010 | Sonnet | |
| REG-001 | Manifest model + in-manifest validation | Rust | done | BE-020 | Sonnet | 37 rule fixtures |
| REG-002 | Package format, signatures, remaining validation | Rust | done | REG-001 | Sonnet | 15 package tests |
| PROTO-001 | W5/W6/W7 wire types | Rust | done (spec issues logged below) | BE-006 | Sonnet | 22 tests + schemas |
| TEST-012 | e2e-gateway M2 acceptance (real API + 2 gateways) | e2e | done | M2 | Sonnet | `bash scripts/e2e/gateway.sh` 11 passed (orchestrator rerun) |
| FUP-M2 | Done: governance typing (FIX-006), SSE revocation (FIX-005), ask-tool hold test (FIX-006), empty snapshot at startup (BE-041). Open (non-blocking): services bind fixed ports (configurable ports would ease e2e) | both | mostly done | — | — | |
| BE-040/041 | Plugin ingestion, trusted keys, approval, host claims, snapshot activation, internal routes | Rust | done | REG-003 | Sonnet | plugins 6 + lifecycle 5 (enable-requires-approval fix by orchestrator) |
| BE-042 | Apps catalogue, connections, OAuth clients, availability (C-20) | Rust | done | BE-041 | Sonnet | apps_connections 3 |
| HOOK-001/002 | WIT + SDK + component validation (I-22); hook host (wasmtime 50.0.1) | Rust | done | I-02a | Sonnet | hookhost 11; guest components |
| SUM-001 | Summary templates + decoders (single impl) | Rust | done | — | Sonnet | 12 tests |
| CLI-002/003 | aurora plugin subcommands, check-snapshot, reserved hosts, --pubkey | Rust | done | — | Sonnet/Haiku | fixtures 33/33 |
| FIX-001 | Fixture packages (7 valid + 25 invalid) | fixtures | done | CLI-002 | Sonnet | `bash scripts/fixtures/build.sh` 33 pass |
| HOOK-004 p1 | Gateway↔hookhost (summarize, mint) e2e | Rust/e2e | done | HOOK-002 | Sonnet | e2e-gateway 15/15 |
| FE-031 | Admin plugins/host claims/trusted keys UI | Next.js | done | BE-041 | Sonnet | 76 vitest |
| BE-043 | Connect flows (fields+probe, oauth2, callback, reconnect, blocklist) + API connect hooks | Rust | done | BE-042 | Sonnet | connect tests; C-22 |
| BE-044 | Connection grants + effective tools | Rust | done | BE-042 | Sonnet | grants tests; C-21 |
| FE-030/032 | Apps catalogue, connect window, connections, OAuth clients (C-21), availability UI | Next.js | done | BE-042/043 | Sonnet | vitest |
| FUP-M3 | Done: governance typing, missing_scopes, tenant_host first connect, tool identity SQL (FIX-006); skill exec bit (SUP-003). Open (non-blocking, perf): pre-warm hook modules on snapshot load | — | mostly done | — | — | |
| FUP-M4 | (1) durable answer text, (4) model-list cache, (5) render inputs: done in FIX-005. Open: (2) harness capabilities stored in `sandboxes.resource_profile` and `steer_delivered_at` reused as send-now marker — give them proper homes; aurora-api dev-depends on aurora-supervisor for one wire test (move render-input types to aurora-protocol). Original note — From BE-052: (1) answer text buffered in memory on the API — durable `text` must come from the supervisor's answer-rule events or a persisted buffer so a channel move mid-turn loses nothing; (2) harness capabilities stored in `sandboxes.resource_profile` and `steer_delivered_at` reused as send-now marker — give them proper homes (spec fix with migration); (3) `gate.result` handling (M5 schedules); (4) model-list 10-min cache; (5) render inputs: plugin snippets/env/stubs, channels, peers, recall snippets (M5/M6) | Rust | todo | BE-052 | — | |
| FUP-M5 | Memory recall uses `websearch_to_tsquery` (AND) over the whole message — consider OR semantics for recall to get useful snippets (BE-060); cross-crate checksum-trailer test (API render vs supervisor harvest) | Rust | todo | BE-060 | — | |
| FUP-M6 | (closed by TEST-015: `Connector::attach_core_client` hook; fake `file.fetch` + outbound download) | Rust | done | CON-001 | — | |
| FUP-M7 | From BE-067: `agents.avatar_public_key` has no index (public lookup scans; add with the next migration); `presence.provision` payload lacks `avatar_url` (set_icon + config feed carry it). From FIX-002: a granted-but-disabled connection still contributes app env/stubs (non-secret); boots before FIX-002 never flag `restart_to_update`; stubs capped at 32 KiB total (boot doc ≤ 64 KiB). (`auth::login_rate_limited` flake fixed in 37a021c1: test now accounts for token refills.) From BE-064: `attachments.blob_key` has no index (orphan-blob sweep scans; add with the next migration). From RUN-004: API's `SandboxSpec.env` is empty, so `AURORA_AGENT_HANDLE` and snapshot app `env` (11 §The boot document) never reach sandboxes on any backend; k8s pod name falls back to sandbox id; no `imagePullSecrets` config key; per-pod pids limit not enforceable on k8s; `/run/aurora` emptyDir cannot be `noexec`. Helm: evaluation Postgres rendered as hook resources (not removed on uninstall — document or move to a subchart); hook jobs **and the API `seed-plugins` init container (OPS-013)** use the third-party `alpine/k8s` image (pin a digest or build an Aurora tools image); kind e2e not run (no cluster) | Ops | todo | OPS-010 | — | |
| API-004 | M4 contract (conversations, turns, transcript, SSE, sandbox, models; C-23..C-26) | contract | done | — | Opus | API_CONTRACTS.md M4 |
| RUN-001/002/003 | Runner core, supervisor channel server, podman backend + sandbox images | Rust | done | PROTO-001 | Sonnet | runner tests; S-07/S-08 |
| SUP-001..004 | Supervisor core, home projection/harvest, MCP bridge + tools socket + process manager, claude-code adapter + conformance | Rust | done | PROTO-001 | Sonnet | supervisor tests; I-23 |
| BE-050 | Runner plane (W5), admin runners | Rust | done | RUN-001 | Sonnet | runner_plane tests |
| BE-051/052/053 | Conversation plane, due-work engine + report handlers, tool registry | Rust | done | BE-050 | Sonnet | conversations/runner_plane tests; I-25 |
| FE-040/041 | Agent Chat + Computer tabs, model picker, conversation calls | Next.js | done | BE-051 | Sonnet | vitest |
| TEST-013 | M4 e2e-hosted (podman) | e2e | done | M4 | Sonnet | 9/9 incl. SEC-01/02/09/24/25/26/34 probes |
| SEC-FIX-01 | Supervisor child env rebuilt via spawn.rs (SEC-34) | Rust | done | TEST-013 | Sonnet + Opus fix (app env kept) | spawn_env re-exec test; clippy disallowed-methods |
| SEC-FIX-02 | Internal listener binds (SEC-09) | Rust/ops | done | TEST-013 | Sonnet + Opus fix (Helm nesting) | listener_binds test; TEST-013 s8b |
| BE-060 | Memory (API, revisions, redaction, language-aware search, recall, tools, harvest plane) | Rust | done | BE-053 | Sonnet | memory tests |
| BE-061 | Skills (agent/org API, files, limits, tools, projection) | Rust | done | BE-053 | Sonnet | skills tests |
| FE-050 | Memory + Skills tabs, org skills page | Next.js | done | BE-060/061 | Sonnet | vitest |
| BE-062 | Schedules (API, cron, gates, tools, due-work arm 9) | Rust | done (c519c441) | BE-052 | Sonnet | schedules 5; full api/db suite 215 tests |
| BE-063 | Webhooks (API, receive endpoint, deliveries, tools) | Rust | done (c519c441) | BE-052 | Sonnet | webhooks 11 |
| BE-064 | Attachments API side (BlobStore, routes, runner plane, delivery, pending + orphan-blob sweeps) | Rust | done (8aa7ea19) | BE-052 | Sonnet (worktree) | |
| BE-065 | Action approvals, agent links, peer tasks, app-turn cap, agent tools | Rust | done (2b58ee61) | BE-052 | Sonnet (worktree) | |
| RUN-005 | Inbound attachments `file.in` (S-11) + `send_file` local tool | Rust | done (2e799b23) | S-11 | Sonnet (worktree) | protocol/runner/supervisor 24 binaries, 155 tests pass on phase0; inbound tests (runner 3, supervisor 9); API side pending BE-064 |
| SEC-FIX-03 | Permission matrix on reads (I-27 + grant-candidate exception, TEST-010 findings) | Rust | done (8132a630) | TEST-010 | Sonnet (worktree) | |
| API-005 | M5 contract (attachments, action approvals, links, peer tasks; C-27..C-30) | contract | done | — | Opus | API_CONTRACTS.md M5 |
| FIX-002 | Sandbox env (`AURORA_AGENT_HANDLE`, app `agent.env`) + app stub files at boot; `restart_to_update` | Rust | done (ecbd49fc) | — | Sonnet (worktree) | |
| OPS-012 | Operator documentation (`docs/operator/`) | Docs | done (e771adda) | OPS-010, RUN-004 | Haiku (worktree) | 8 pages; keys/commands cross-checked against code; 13 known gaps listed |
| OPS-013 | Deployment gaps from OPS-012 (Helm nodeSelector/tolerations, plugin mount, compose CA, just dev, comments, eval Postgres) | Ops | done | OPS-012 | Haiku (worktree) | helm lint; aurora-config 20 (helm-render, dev-config); setup.sh CA idempotent (run twice); compose config |
| FIX-003 | Unused config wired (condition_body_bytes, resolve cache TTL, held_awake_ceiling), connector bootstrap tokens + allow_unsigned, OTLP export, sandbox image from config (OPS-011), k8s harness advertisement (TEST-017) | Rust | done (75f73761) | BE-070 | Sonnet (worktree) | |
| BE-066 | Apps tools `request_app_connection` (I-29), `propose_custom_connection` (08 §Agent-proposed) | Rust | done (73dab75f) | BE-031, BE-042 | Haiku (worktree) | |
| FE-051 | Schedules, webhooks, peers tabs; action approvals; chat attachments; app-turn Continue | Next.js | done (55480baf) | BE-062..065 | Sonnet (worktree) | web 17 files / 123 tests; typecheck, lint (0 errors), build |
| TEST-014 | M5 hosted suites: audit 18 §Hosted agents, fill gaps | Rust/e2e | done (f5b4a155; S-13 follow-up 0e1a7b95) | `docs/tests/hosted-coverage.md` no gaps; fix: boot end converts armed watches | M5 | Sonnet (worktree) | |
| CHAN-001 | aurora-channels (content, cards, action tokens, capabilities; I-26) | Rust | done | — | Sonnet | channels tests |
| CON-001 | Connector contract W8 + SDK + fake connector | Rust | done | CHAN-001 | Sonnet | connector tests; FUP-M6 |
| API-006 | M6 contract (connector admin, integrations, presences, bindings, user links, DM defaults, reach; C-31..C-37) | contract | done | — | Opus | API_CONTRACTS.md M6 |
| BE-070 | Connector registration, admin, ownership, config feed (I-28) | Rust | done (d4d04b36) | API-006 | Sonnet (worktree) | |
| BE-071 | Inbox ingest, ops engine, correlated ops, connector files, link-start | Rust | done (6fff9b52) | BE-070, BE-064 | Sonnet (worktree) | |
| BE-074 | Channel management API (API-006) + shared channel repos | Rust | done (1b48f47a) | BE-071 | Sonnet (worktree) | |
| BE-072 | Inbox processing (13 §5.1), mirror (§5.2), commands (§5.4) | Rust | done (68e1c128; reconciled with BE-073, S-12) | BE-071, BE-074 | Sonnet (worktree) | |
| BE-073 | Cards, action tokens, clicker authorization, approval fan-out | Rust | done (20fc85cc) | BE-071, BE-074 | Sonnet (worktree) | |
| BE-075 | Messaging tools + `send_message` action handler | Rust | done (bea142ff) | BE-072, BE-074 | Sonnet (worktree) | |
| TEST-015 | Connector conformance suite (fake core) | Rust test | done (872d94d3) | CON-001 | Sonnet (worktree) | 12/12 scenarios in-process and binary mode (`scripts/conformance.sh`); fake/SDK fixes; FUP-M6 closed |
| FE-060 | Channel pages, admin connectors, /link/channel, account links + DM defaults | Next.js | done (1fe32db7) | API-006 | Sonnet (worktree) | web 18 files / 142 tests; build |
| FIX-004 | Presence options for members (C-39), connector manifest diff (C-40) | both | done (442db5bc) | FE-060 | Sonnet (worktree) | |
| TEST-016 | M6 e2e: real core + fake connector | e2e | done (b3868c42) | BE-070..075 | Sonnet (worktree) | `AURORA_TEST_PODMAN=1 bash scripts/e2e/channels.sh` 6/6; SDK/fake fixes; SEC-18..20 mapped |
| OPS-010 | Helm chart | Ops | done | — | Sonnet | `helm lint`, `helm template` |
| RUN-004 | Kubernetes backend (+ runner.k8s.core_namespace config fix) | Rust | done (e65ec66d) | RUN-003 | Sonnet (Orca worker, worktree) + Opus (kube → aws-lc-rs) | config 18, k8s unit 15, k8s_mock 15 (fake API server), runner 10; clippy; cargo deny ok; helm-rendered ConfigMap loads |
| OPS-011 | kind e2e (Helm from scratch, k8s backend, NetworkPolicy/SEC-02 probes) | Ops | done (3f97f950) | RUN-004, OPS-013 | Sonnet (worktree) | |
| TEST-017 | VidaiMock (pinned, Aurora scenario files) behind real LiteLLM (I-30); live claude-code conformance + hosted CLI e2e | e2e | done (dab35c24) | smoke 8/8; live conformance pass; hosted claude-code e2e 2/2 | SUP-004, TEST-013 | Sonnet (worktree) | |
| FIX-005 | Spec-required runtime follow-ups: SSE closes on revocation, durable answer text, render inputs, model-list cache | Rust | done (bb9ca8d6) | — | Sonnet (worktree) | sse_revocation 5, home_render_inputs 2, hosted answer-restart test; I-32 |
| FIX-006 | Spec-required apps/gateway follow-ups: missing_scopes (I-33, C-42), tenant_host on first OAuth connect (S-14), governance typing, ask-hold test, tools SQL | both | done (4143733a) | — | Sonnet (worktree) | connect 12, ask_tool_hold 1; 552 tests across 6 crates; web 146 |
| FIX-007 | restart_to_update on image change; render inputs/env respect app availability | Rust | in-progress | FIX-005, OPS-014 | Sonnet (worktree) | |
| TEST-018 | Security invariant coverage SEC-01..39 (`docs/tests/security-coverage.md`) | tests | done (13ac4bba) | 39 rows; 14 tests/guards added; SEC-18..20 proven via TEST-016; SEC-30 avatars → BE-067 |
| BE-067 | Agent avatars: upload, re-encode to 256² PNG, rotating public URL, presence.set_icon (SEC-30) | both | done (4f7101df) | BE-064 | Sonnet (worktree) | avatars 7; web 149; I-31, C-41; SEC-30 proven |
| OPS-014 | Gateway performance budgets (18) + upgrade test | Ops | done (51f6732c) | OPS-011 | Sonnet (worktree) | `bash scripts/perf/gateway.sh` PASS: (a) p50 0.033/p99 0.175 ms, (b) 11 563 rps/core, (c) p99 4.36 ms; `bash scripts/e2e/upgrade.sh` 31/31 (image-only restart flag → FIX-007) |

## Open spec issues (found by PROTO-001; S-06..S-11 resolved)

- (resolved S-06) `file.part` ≤ 144 KiB decoded.
- (resolved S-07) deterministic UUIDv5 report ids from (boot_id, seq) — add `NAMESPACE_AURORA_REPORT` const in M4 runner task.
- Unspecified shapes: `HarnessEventWire`, `usage`, `ProcessState`, home-sync `entries`; `ack`/`shutdown` bodies; `ModelChoice` vocabularies; mcp-bridge ⇄ supervisor JSON-lines format; correlation ids for `memory.write`/`file.part`; misc formats/bounds (provisional constants in `aurora-protocol::runner`).

## Decisions requiring user input

- none

## Open security findings

- I-27 member-read defects (TEST-010) fixed by SEC-FIX-03 (8132a630).
- SEC-34 fixed by SEC-FIX-01 (9b61c0f1) and SEC-09 by SEC-FIX-02 (f9130aa3); TEST-013 s8b passes. TEST-013 should gain a `process_start` env probe for SEC-34 now that `process_*` tools are listed (follow-up).

## Blockers

- (resolved 2026-11-06) kind v0.33 + kubectl installed by the owner → OPS-011.
- (resolved 2026-11-06) No real LiteLLM endpoint: owner chose a mock behind compose → I-30 (VidaiMock, owner-pointed), TEST-017.

## Standing notes (not blockers)

- Workspace `Cargo.toml` licenses all first-party crates `GPL-3.0-or-later` (owner's choice — do not change). `cargo deny`'s permissive-only allowlist applies to third-party deps only; our crates are `publish = false` and skipped via `private = { ignore = true }`, so the check still passes.

## Recently completed

- 2026-11-07 (late night): TEST-018 SEC coverage (38/39; SEC-30 → BE-067), OPS-014 perf PASS + upgrade 31/31, FIX-005 runtime follow-ups.
- 2026-11-07 (night): TEST-014 (hosted coverage; S-13 memory encoded-size bound), TEST-016 channels e2e 6/6.
- 2026-11-07 (evening): BE-075 messaging tools, FIX-004 (C-39/C-40), FIX-003 (config wiring, connector bootstrap, OTLP, runner-reported image + held-awake ceiling, k8s claude-code); all M6 backend merged.
- 2026-11-07 (later): TEST-017 (VidaiMock+LiteLLM live conformance; claude-code adapter wiring, podman harness, guard LLM hosts fixed), BE-072 inbox + BE-073 cards (seam reconciled; S-12 approval codes; real dashboard links), FE-060 channel dashboard; C-39/C-40.
- 2026-11-07: BE-074 channel management API, OPS-011 kind e2e (chart fixes: key generation, API CA mount, migrate NetworkPolicy); permission matrix back to 0 inconclusive; rate-limit test flake fixed.
- 2026-11-07 early: BE-071 (inbox/ops engine), FIX-002 (sandbox env + stubs), BE-066 (apps tools), OPS-012/013 (operator docs + deployment gaps); I-29, I-30 (VidaiMock), C-38; phase sequencing rule.
- 2026-11-06 night: BE-070 connector plane registration (I-28), SEC-FIX-03 (matrix on reads), FE-051 M5 dashboard; 195 leaked testkit containers removed (worktrees lacked `.env`; spec template now copies it).
- 2026-11-06 late: RUN-005 (file.in, send_file; `.aurora/` excluded per owner), BE-064 attachments (+ orphan-blob sweep), BE-065 action approvals/links/peer tasks/app-turn cap, TEST-015 conformance 12/12; guards fixed (no platform names in core) and now run at every merge; I-27 grant-candidate exception.
- 2026-11-06 evening: SEC-FIX-01/02 (SEC-34, SEC-09 closed), TEST-013 9/9, BE-062 schedules, BE-063 webhooks, TEST-010 permission matrix (defects → SEC-FIX-03), RUN-004 kubernetes backend; API-005 (M5) and API-006 (M6) contracts; decisions I-27, S-11; briefs BE-064/065, RUN-005, SEC-FIX-03, BE-070/071, TEST-015.
- 2026-11-06 afternoon: M3 (BE-040..044, HOOK-001/002/004, CLI-002/003, FIX-001, FE-030..032), M4 (RUN-001..003, SUP-001..004, BE-050..053, FE-040/041, API-004), M5 memory/skills (BE-060/061, FE-050), M6 foundations (CHAN-001, CON-001), OPS-010 Helm.
- 2026-11-06 morning: M1 (BE-010..016, FE-010..013, TEST-011), M2 (GW-001..005, BE-020..034, GOV-001, CLI-001, FE-020..024, TEST-012/020), REG-001..003.
- 2026-11-05: OPS-001..003, FE-001, API-001, BE-001..008, TEST-001.
