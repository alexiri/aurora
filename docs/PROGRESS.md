# Implementation progress

Last updated: 2026-11-06 (afternoon, orchestrator)

Scope: **Phase 0 (M0–M7)**. Phases 1–4 are out of scope until phase 0 exits.
Ids per decision I-01. Task briefs live in `docs/tasks/<ID>.md`. Milestones
beyond the active one are tracked as epics and decomposed into briefs when
their dependencies near completion (adaptive planning).

## Environment

- **Concurrency**: owner raised the default to **4 concurrent implementation agents** (2026-11-06), still with disjoint file ownership per lane.

- Rust 1.99 stable (system, no rustup) — **no `wasm32-wasip2` target** (I-02, blocks M3 hook builds only).
- Node 20.20, pnpm 10 (`~/.local/bin`), podman 5 (compose), psql client.
- Dev Postgres 16: container `aurora-dev-pg`, `postgres://aurora:aurora@127.0.0.1:55432/aurora` (`.env`, I-05).
- sqlx-cli 0.8.6 at `~/.cargo/bin/sqlx` (not on PATH; use full path).
- Disk: ~39 GB free on `/home` — keep one `target/`, avoid duplicate container build caches (I-03).

## Milestone status

| Milestone | Status | Acceptance evidence |
|---|---|---|
| M0 | done (CI never executed on GitHub — no remote) | schema guard, compose up, guards, cargo-deny |
| M1 | done except OIDC (BE-015 in progress) and permission-matrix test (TEST-010) | `scripts/e2e/m1.sh` Playwright sign-up → workspace (Daneel) → invite → accept: pass |
| M2 | done (core) — follow-ups FUP-M2 | `scripts/e2e/gateway.sh` 11 scenarios pass; governance SEC-35..39 service tests (GW-005, TEST-020); hold tests incl. two replicas |
| M3 | in progress: registry crate + gateway apps/mechanisms/refresh done; lifecycle API, connect flows, connection grants, apps UI, hook host/WIT/SDK (needs wasm32-wasip2), plugin CLI, fixtures todo | — |
| M4–M7 | todo (wire types done) | — |

## Requirement coverage

| Requirement ID | Summary | Task IDs | Status | Evidence/gap |
|---|---|---|---|---|
| M0/02§layout | Workspace + layout | OPS-001 | done | `cargo check --workspace` ok (2026-11-05) |
| M0/02§core | aurora-core | BE-001 | done | |
| M0/15§config | aurora-config | BE-002 | done | |
| M0/08§3-4 | aurora-crypto | BE-003 | done | |
| M0/03 | aurora-db + all tables | BE-004, BE-005, TEST-001 | done | migrate from empty == 03 (introspection test) |
| M0/16 | aurora-protocol skeleton | BE-006 | done | |
| M0/18 | aurora-testkit | BE-007 | done | |
| M0/15§obs | service binaries | BE-008 | done | |
| M0/18§CI | CI stages 1–2 | OPS-002 | done | guards + cargo-deny pass locally; workflow not executed (no remote) |
| M0/15§compose | compose skeleton | OPS-003 | done | images built (api 34.5 MB…web 271 MB); `podman compose up`: edge→api 404 envelope, edge→web 200, /metrics+/internal 404, gw 4102 503; runner/connector profiles not exercised |
| M0/14 | dashboard scaffold | FE-001 | done | |
| M1..M7 | see FEATURES.md | epics | todo | |
| SEC-10 | tokens hashed at rest | BE-001 (gen/hash), M1 repos | planned | |
| SEC-11 | SecretRef AAD binding | BE-003 (envelope AAD), M2 | planned | |
| SEC-18 | untrusted-text sanitizers | BE-001 (sanitizers), M6 | planned | |
| other SEC-nn | per milestone | — | todo | |

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
| BE-015 | OIDC + mock IdP | Rust | in-progress | BE-011 | Sonnet | |
| FE-011 | Dashboard shell | Next.js | done | BE-012 | Sonnet | 14 vitest; build; curl smoke |
| FE-010 | Auth pages + CSP proxy | Next.js | done | BE-011 | Sonnet | 22 vitest; CSP nonce/nosniff headers verified; /join on generated client |
| FE-013 | Account, workspace settings, admin users | Next.js | done | BE-014, BE-016 | Sonnet | |
| FE-012 | Org pages | Next.js | done | BE-013, FE-013 | Sonnet | |
| TEST-010 | Permission-matrix test | Rust test | todo | BE-013..016 | haiku-tester | |
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
| FUP-M2 | Follow-ups: OpenAPI schema for OrgGovernance document (currently untyped object); SSE stream not closed on session revocation; services bind fixed ports (configurable ports would ease e2e); API must activate an empty snapshot at startup (gateway readyz) — goes with M3 lifecycle; GW-011 real-DB ask-tool hold test | both | todo | — | — | |
| BE-040/041 | Plugin ingestion, trusted keys, approval, host claims, snapshot activation, internal routes | Rust | done | REG-003 | Sonnet | plugins 6 + lifecycle 5 (enable-requires-approval fix by orchestrator) |
| BE-042 | Apps catalogue, connections, OAuth clients, availability (C-20) | Rust | done | BE-041 | Sonnet | apps_connections 3 |
| HOOK-001/002 | WIT + SDK + component validation (I-22); hook host (wasmtime 50.0.1) | Rust | done | I-02a | Sonnet | hookhost 11; guest components |
| SUM-001 | Summary templates + decoders (single impl) | Rust | done | — | Sonnet | 12 tests |
| CLI-002/003 | aurora plugin subcommands, check-snapshot, reserved hosts, --pubkey | Rust | done | — | Sonnet/Haiku | fixtures 33/33 |
| FIX-001 | Fixture packages (7 valid + 25 invalid) | fixtures | done | CLI-002 | Sonnet | `bash scripts/fixtures/build.sh` 33 pass |
| HOOK-004 p1 | Gateway↔hookhost (summarize, mint) e2e | Rust/e2e | done | HOOK-002 | Sonnet | e2e-gateway 15/15 |
| FE-031 | Admin plugins/host claims/trusted keys UI | Next.js | done | BE-041 | Sonnet | 76 vitest |
| BE-043 | Connect flows (fields+probe, oauth2, callback, reconnect, blocklist) + API connect hooks | Rust | todo (next) | BE-042 | Sonnet | |
| BE-044 | Connection grants + effective tools | Rust | todo | BE-042 | Sonnet | |
| FE-030 | Apps catalogue, connect window, connections, OAuth clients, availability UI | Next.js | todo | BE-042/043 | Sonnet | |
| FUP-M3 | Pre-warm hook modules on snapshot load; OpenAPI typing for governance document; OAuth callback should surface `missing_scopes` (store requested scopes in state, show on connection); first connect with a tenant-templated token URL must take `tenant_host` from connect `fields` (zone-validated); skill exec bit (SUP-003, done); move tool-call identity SQL from `domains/tools/service.rs` into an `aurora-db` repo (02 rule; BE-053) | — | todo | — | — | |
| FUP-M4 | From BE-052: (1) answer text buffered in memory on the API — durable `text` must come from the supervisor's answer-rule events or a persisted buffer so a channel move mid-turn loses nothing; (2) harness capabilities stored in `sandboxes.resource_profile` and `steer_delivered_at` reused as send-now marker — give them proper homes (spec fix with migration); (3) `gate.result` handling (M5 schedules); (4) model-list 10-min cache; (5) render inputs: plugin snippets/env/stubs, channels, peers, recall snippets (M5/M6) | Rust | todo | BE-052 | — | |
| M3 rest | snapshot compiler, lifecycle/ingestion API, connect flows, mechanisms in gateway, hook host + WIT + SDK (needs wasm32-wasip2), CLI plugin, fixtures | both | todo | M2 | — | |
| M4–M7 | hosted runtime, hosted features, channels, k8s/Helm | both | todo | per 19 | — | |

## Open spec issues (resolve before M4 implementation; found by PROTO-001)

- (resolved S-06) `file.part` ≤ 144 KiB decoded.
- (resolved S-07) deterministic UUIDv5 report ids from (boot_id, seq) — add `NAMESPACE_AURORA_REPORT` const in M4 runner task.
- Unspecified shapes: `HarnessEventWire`, `usage`, `ProcessState`, home-sync `entries`; `ack`/`shutdown` bodies; `ModelChoice` vocabularies; mcp-bridge ⇄ supervisor JSON-lines format; correlation ids for `memory.write`/`file.part`; misc formats/bounds (provisional constants in `aurora-protocol::runner`).

## Decisions requiring user input

- none

## Blockers

- Sub-agent usage limit (Sonnet) until 14:20 Europe/Zurich: FE-013 and GOV-001 paused mid-task (resume, don't restart).
- Workspace license in Cargo.toml is `GPL-3.0-or-later` (owner's choice; deny ignores private crates).
- (resolved 2026-11-06) `wasm32-wasip2` installed by the owner (`rust-wasm`); component builds verified.

## Recently completed

- 2026-11-06: BE-010..016 (except 015), FE-010/011, GW-001..004, REG-001/002, BE-020..022, API-002; spec fixes S-01..S-04; decisions I-01..I-16.

- OPS-001, FE-001, API-001, BE-001..BE-006 (2026-11-05). Spec fixes S-01, S-02 recorded in 90-decisions.md.
