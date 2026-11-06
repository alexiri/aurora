# Feature inventory

Requirement ids per `90-decisions.md` I-01: `M<n>/<doc>§<section>`; security
invariants `SEC-nn` (`17`). Phase 0 = M0–M7 (in scope now). Phase 1–4 are out
of scope until phase 0 exits (`MASTER_SPEC` "Phases are real").

Status: `todo` · `planned` (briefs written) · `in-progress` · `done` (acceptance evidence in PROGRESS.md) · `blocked`.
Last reconciled with git history: 2026-11-06 night (after FE-051 merge, 55480baf).

| Requirement ID | Feature | Priority | Backend tasks | Frontend tasks | Dependencies | Status | Evidence |
|---|---|---|---|---|---|---|---|
| M0/02§layout | Cargo workspace, layout, lints, toolchain | P0 | OPS-001 | — | — | done | `cargo check --workspace` |
| M0/02§core | `aurora-core`: ids, error envelope, sanitizers, limits, tokens, `Secret<T>` | P0 | BE-001 | — | OPS-001 | done | aurora-core tests |
| M0/15§config | `aurora-config`: layered TOML + env, typed sections, validation | P0 | BE-002 (+ SEC-FIX-02, RUN-004 keys) | — | OPS-001 | done | tests/config.rs 18 |
| M0/08§3-4 | `aurora-crypto`: AEAD envelope, keyring, HKDF purposes, ed25519 | P0 | BE-003 | — | BE-001 | done | crypto tests |
| M0/03 | `aurora-db`: pool, migrations for all tables, claim helpers, NOTIFY bus | P0 | BE-004, BE-005, TEST-001 | — | OPS-001 | done | migrate-from-empty == 03 (schema_matches_spec) |
| M0/16 | `aurora-protocol` + JSON Schema generation | P0 | BE-006, PROTO-001 | — | BE-001 | done | wire + schema drift tests |
| M0/18 | `aurora-testkit` (ephemeral Postgres, mocks) | P0 | BE-007 | — | BE-004 | done | |
| M0/15§obs | Service binaries: health/readiness/metrics, config validation, `migrate` | P0 | BE-008 | — | BE-001/002/004 | done | |
| M0/18§CI | CI stages + architecture guards | P0 | OPS-002 | — | OPS-001 | done (never run on a CI host: no remote) | `scripts/guards/run-all.sh` |
| M0/15§compose | Compose stack, Containerfiles | P0 | OPS-003 | FE-001 | BE-008 | done | compose up |
| M0/14 | Dashboard scaffold (App Router, TS strict, Tailwind, TanStack Query, openapi-fetch, vitest, Playwright) | P0 | — | FE-001 | — | done | |
| M1/04 | Identity: email/password, verification, reset, OIDC, sessions, CSRF, API keys, CLI device login, invitations, roles, instance admins, audit | P0 | BE-010..016 | FE-010, FE-013 | M0 | done | auth 13, oidc 17, keys, cli, invitations 5, me/admin/audit |
| M1/04§matrix | Access law + permission matrix on every route (I-27: reads too) | P0 | TEST-010, SEC-FIX-03 | — | BE-010..016 | done | permission_matrix.rs (all OpenAPI routes × 13 principals, 0 inconclusive) + org-scope credential reads test |
| M1/04§signup | Sign-up: org/workspace, default policy, Daneel | P0 | BE-012 | FE-011, FE-012 | BE-010 | done | tenancy tests; `scripts/e2e/m1.sh` |
| M1/05 | API skeleton, OpenAPI generation, `/v1/instance`, `/v1/resolve` | P0 | BE-010, BE-016 | — | M0 | done | OpenAPI drift test |
| M1/14 | Dashboard: auth, shells, org pages, members, invitations, keys, account, admin users | P0 | — | FE-010..013 | API-001 | done | vitest; TEST-011 Playwright |
| M2/08 | Secrets (vault, stores, inline AEAD, reference mode), LLM keys, providers | P0 | BE-021, BE-022, BE-031 | FE-020 | M1 | done | secrets 8, providers 19 |
| M2/06 | Gateway: CA, MITM/open lanes, credential auth, pipeline, guard, guidance, caches + NOTIFY, request log | P0 | GW-001..003 | — | M1 | done | gateway tests; TEST-012 e2e 11/11 |
| M2/07 | Policy engine, authoring, grants, approvals (holds, API, SSE, UI) | P0 | BE-020, BE-033, GW-004, BE-030 | FE-021, FE-022 | M1 | done | policy corpus 30; hold tests incl. 2 replicas |
| M2/08§10-11 | BYO agents, context credentials, CLI login/run/approvals, LLM & egress governance | P0 | BE-032, CLI-001, GOV-001, GW-005, BE-034 | FE-023, FE-024 | M1 | done (FUP-M2 open) | governance 15 + TEST-020 |
| M3/09 | Plugins: package/signing/validator/approval, snapshot, host claims, trusted keys | P0 | REG-001..003, BE-040, BE-041 | FE-031 | M2 | done | plugins 6 + lifecycle 5; fixtures 33/33 |
| M3/09§apps | Apps catalogue, connect flows (fields/OAuth), OAuth clients, connections, grants, availability | P0 | BE-042..044, GW-010, GW-011 | FE-030, FE-032 | REG-003 | done (FUP-M3 open) | apps 10, connect, grants |
| M3/09§hooks | Hooks: WIT, SDK, hook host (wasmtime), gateway summarize/mint, CLI plugin | P0 | HOOK-001/002/004, SUM-001, CLI-002/003, FIX-001 | — | I-02a | done | hookhost 11; e2e-gateway 15/15 |
| M4/11 | Runner core, channel server, fake + podman backends, sandbox images | P0 | RUN-001..003 | — | PROTO-001 | done | runner tests; TEST-013 |
| M4/12 | Supervisor: boot doc, channel, loopback proxy, home projection, harvest, MCP bridge, process manager, scrubbed spawns | P0 | SUP-001..003, SEC-FIX-01 | — | PROTO-001 | done | supervisor tests; spawn_env |
| M4/10 | Conversations, turns, transcript/SSE, due-work engine, context credentials, tool registry, models | P0 | BE-050..053, FIX-002 | FE-040, FE-041 | M2 | done (FUP-M4 open; sandbox env/stubs gap → FIX-002 in progress) | hosted 7, runner_plane 10; TEST-013 9/9 |
| M5/10§memory | Memory (+language), search, recall, harvest, tools, UI | P0 | BE-060 | FE-050 | M4 | done (FUP-M5) | memory 7 |
| M5/10§skills | Skills (agent/org), files, limits, projection, tools, UI | P0 | BE-061 | FE-050 | M4 | done | skills 5 |
| M5/10§schedules | Schedules: cron (5-min floor), gates, fire lease, fire-time authority, tools | P0 | BE-062 | FE-051 | M4 | done | schedules 5; web m5-tabs |
| M5/10§webhooks | Webhooks: receiver, verification, rate limit, idempotency, tools | P0 | BE-063 | FE-051 | M4 | done | webhooks 11; web m5-tabs |
| M5/10§processes | Processes and watches (local tools, process.state, watch turns, parked/lost) | P0 | SUP-003, BE-052 | FE-041 (computer tab) | M4 | done | supervisor process tests; hosted tests |
| M5/10§attachments | Attachments in/out (BlobStore, routes, `file.in` S-11, `send_file`) | P0 | BE-064, RUN-005 | FE-051 | M4 | done; e2e todo (TEST-014) | attachments 9; runner inbound 3, supervisor inbound 9 |
| M5/07§actions | Action approvals, agent links, peer tasks, app-turn cap, agent tools | P0 | BE-065 | FE-051 | API-005 | done | action_approvals 10, peer_tasks 12 |
| M5/12§catalogue | Full phase-0 platform tool catalogue | P0 | BE-060..065; apps tools BE-066 (in progress); messaging tools BE-075 | — | — | in-progress | |
| M5/12§claude-code | `claude-code` adapter + conformance | P0 | SUP-004 | — | SUP-001 | done offline; live conformance **blocked** (no LiteLLM endpoint) | adapter tests |
| M5/18 | Hosted M5 suites (18) | P0 | TEST-014 | — | M5 above, FIX-002 | todo | |
| M6/13§content | Neutral content, cards, action tokens, capabilities/fallbacks | P0 | CHAN-001 | — | — | done | channels tests; I-26 |
| M6/13§contract | Connector contract v1 types, SDK, fake connector | P0 | CON-001, TEST-015 fixes | — | CHAN-001 | done | connector tests; FUP-M6 closed |
| M6/13§plane | Registration, approval, ownership, config feed, inbox, ops, correlated ops, files | P0 | BE-070, BE-071 | — | API-006 | in-progress (BE-070 done, BE-071 running) | connectors 12 |
| M6/13§core | Inbox processing, routing, linking, reach, bindings, commands, mirror, cards, presence, messaging tools | P0 | BE-072..075 | FE-060 | BE-071 | todo | |
| M6/13§7 | Conformance suite (fake core) + e2e channel scenario | P0 | TEST-015, TEST-016 | — | CON-001 / BE-075 | conformance done (12/12); e2e todo | `scripts/conformance.sh` |
| M7/11§k8s | Kubernetes runner backend | P0 | RUN-004 | — | RUN-003 | done (no cluster: mock API only) | k8s_mock 15, unit 15 |
| M7/15§helm | Helm chart (jobs, policies, namespaces, RBAC, routing) | P0 | OPS-010 | — | — | done (FUP-M7) | `helm lint`, `helm template`; ConfigMap loads (RUN-004) |
| M7/18 | kind e2e, SEC-02 on k8s, performance budgets, upgrade test | P0 | OPS-011 | — | RUN-004 | **blocked** for kind (no cluster on host); rest todo | |
| M7/15§docs | Operator docs (install, LLM endpoints, runners, plugins, backups) | P0 | OPS-012 (+ OPS-013, FIX-003 for the gaps it found) | — | — | done (docs); gap fixes in progress | `docs/operator/` |
| SEC-01..39 | Security invariants, each with a proving test (`17`) | P0 | per milestone | — | — | in-progress | proven so far: SEC-01/02/09/24/25/26/34 (TEST-013), SEC-35..39 (GW-005, TEST-020), CSRF (BE-010); full map at final audit |
| P1/20 | Mattermost connector | P1 (later phase) | — | — | Phase 0 exit | out of scope now | |
| P2..P4/21 | App catalogue, DB lane, Vault, SSH, previews, evals, … | later | — | — | Phase 0 exit | out of scope now | |
