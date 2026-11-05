# Feature inventory

Requirement ids per `90-decisions.md` I-01: `M<n>/<doc>§<section>`; security
invariants `SEC-nn` (`17`). Phase 0 = M0–M7 (in scope now). Phase 1–4 are out
of scope until phase 0 exits (`MASTER_SPEC` "Phases are real").

Status: `todo` · `planned` (briefs written) · `in-progress` · `done` (acceptance evidence recorded in PROGRESS.md) · `blocked`.

| Requirement ID | Feature | Priority | Backend tasks | Frontend tasks | Dependencies | Status | Evidence |
|---|---|---|---|---|---|---|---|
| M0/02§layout | Cargo workspace, repository layout, lints, toolchain | P0 | OPS-001 | — | — | done | `cargo check --workspace` |
| M0/02§core | `aurora-core`: ids, error envelope, sanitizers, limits registry, tokens, `Secret<T>` | P0 | BE-001 | — | OPS-001 | planned | |
| M0/15§config | `aurora-config`: layered TOML + `AURORA__*` env, typed sections, startup validation | P0 | BE-002 | — | OPS-001 | planned | |
| M0/08§3-4 | `aurora-crypto`: inline AEAD envelope, keyring, HKDF purposes, ed25519 | P0 | BE-003 | — | BE-001 | planned | |
| M0/03 | `aurora-db`: pool, embedded migrations for **all** tables, claim helpers, NOTIFY bus | P0 | BE-004, BE-005, TEST-001 | — | OPS-001 | planned | |
| M0/16 | `aurora-protocol` skeleton with JSON Schema generation | P0 | BE-006 | — | BE-001 | planned | |
| M0/18 | `aurora-testkit`: ephemeral Postgres, mock upstream (OAuth/OIDC mocks in M1/M3) | P0 | BE-007 | — | BE-004 | planned | |
| M0/15§obs | Service binaries: health/readiness/metrics/config validation, `aurora-api migrate` | P0 | BE-008 | — | BE-001, BE-002, BE-004 | planned | |
| M0/18§CI | CI pipeline stages 1–2 + architecture guards scaffold | P0 | OPS-002 | — | OPS-001 | planned | |
| M0/15§compose | Compose skeleton (postgres, migrate, api, gateway, hookhost, web, edge), Containerfiles | P0 | OPS-003 | FE-001 | BE-008, FE-001 | planned | |
| M0/14 | Dashboard scaffold (Next.js App Router, TS strict, pnpm, Tailwind, shadcn/ui, TanStack Query, openapi-fetch, vitest, Playwright) | P0 | — | FE-001 | — | planned | |
| M1/04 | Identity & tenancy: email/password, verification, reset, OIDC, sessions, CSRF, API keys, CLI device login, invitations, roles, access law, permission matrix, instance admins, bootstrap, audit | P0 | BE-010..BE-017 | — | M0 | todo | |
| M1/04§signup | Sign-up transaction: org/workspace, default policy generations, default agent Daneel (row) | P0 | BE-012 | FE-012 | BE-010 | todo | |
| M1/05 | API skeleton, OpenAPI generation, `/v1/instance`, `/v1/resolve` | P0 | BE-010, BE-016 | — | M0 | todo | |
| M1/14 | Dashboard: auth pages, org/workspace shells, members, invitations, API keys, account, admin users | P0 | — | FE-010..FE-013 | API-001 | todo | |
| M2/08 | Secrets (`SecretVault`, registry, inline, memory store, reference mode, migrate/rekey), LLM keys, LiteLLM + Anthropic providers | P0 | epic M2 | epic M2 | M1 | todo | |
| M2/06 | Gateway: CA, MITM + open lanes, credential auth, pipeline, destination guard, guidance, caches + NOTIFY, request log, internal port | P0 | epic M2 | — | M1 | todo | |
| M2/07 | Policy engine, authoring (drafts/publish/rollback/simulate/warnings), grants for secrets, approvals (store, API, SSE, UI) | P0 | epic M2 | epic M2 | M1 | todo | |
| M2/08§10-11 | BYO agents (token, container config, context credentials), CLI `login`/`run`/`approvals watch`, LLM & egress governance | P0 | epic M2 | epic M2 | M1 | todo | |
| M3/09 | App plugin system: package/signing/validator/approval, snapshot, mechanisms, connect flows, OAuth clients, connections, refresh, hooks + hook host + SDK + CLI, fixtures | P0 | epic M3 | epic M3 | M2; I-02 (wasm target) | todo | |
| M4/10-12 | Hosted runtime: runner (fake, podman), sandbox image, supervisor, due-work engine, conversations/turns/transcript/SSE, context credentials, chat & computer tabs | P0 | epic M4 | epic M4 | M2 | todo | |
| M5/10,12 | Hosted features: memory (+language), skills, schedules, webhooks, processes/watches, attachments, platform tools, action approvals, agent links, `claude-code` adapter | P0 | epic M5 | epic M5 | M4 | todo | |
| M6/13 | Channel core: connector registration, inbox/ops, routing, linking, bindings, cards, action tokens, presence, messaging tools, connector SDK, fake connector, conformance | P0 | epic M6 | epic M6 | M4 | todo | |
| M7/11,15 | Kubernetes backend, Helm chart, kind e2e, performance budgets, upgrade test, operator docs | P0 | epic M7 | — | M4 | todo | |
| SEC-01..SEC-39 | Security invariants each with a proving test (`17`) | P0 | per milestone | — | — | todo | see PROGRESS coverage |
| P1/20 | Mattermost connector | P1 (later phase) | — | — | Phase 0 exit | out of scope now | |
| P2..P4/21 | App catalogue, DB lane, Vault, SSH, previews, evals, … | later | — | — | Phase 0 exit | out of scope now | |
