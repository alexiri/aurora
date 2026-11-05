# Implementation progress

Last updated: 2026-11-05 (orchestrator)

Scope: **Phase 0 (M0–M7)**. Phases 1–4 are out of scope until phase 0 exits.
Ids per decision I-01. Task briefs live in `docs/tasks/<ID>.md`. Milestones
beyond the active one are tracked as epics and decomposed into briefs when
their dependencies near completion (adaptive planning).

## Environment

- Rust 1.99 stable (system, no rustup) — **no `wasm32-wasip2` target** (I-02, blocks M3 hook builds only).
- Node 20.20, pnpm 10 (`~/.local/bin`), podman 5 (compose), psql client.
- Dev Postgres 16: container `aurora-dev-pg`, `postgres://aurora:aurora@127.0.0.1:55432/aurora` (`.env`, I-05).
- sqlx-cli 0.8.6 at `~/.cargo/bin/sqlx` (not on PATH; use full path).
- Disk: ~39 GB free on `/home` — keep one `target/`, avoid duplicate container build caches (I-03).

## Requirement coverage

| Requirement ID | Summary | Task IDs | Status | Evidence/gap |
|---|---|---|---|---|
| M0/02§layout | Workspace + layout | OPS-001 | done | `cargo check --workspace` ok (2026-11-05) |
| M0/02§core | aurora-core | BE-001 | planned | |
| M0/15§config | aurora-config | BE-002 | planned | |
| M0/08§3-4 | aurora-crypto | BE-003 | planned | |
| M0/03 | aurora-db + all tables | BE-004, BE-005, TEST-001 | planned | |
| M0/16 | aurora-protocol skeleton | BE-006 | planned | |
| M0/18 | aurora-testkit | BE-007 | planned | |
| M0/15§obs | service binaries | BE-008 | planned | |
| M0/18§CI | CI stages 1–2 | OPS-002 | planned | |
| M0/15§compose | compose skeleton | OPS-003 | planned | |
| M0/14 | dashboard scaffold | FE-001 | planned | |
| M1..M7 | see FEATURES.md | epics | todo | |
| SEC-10 | tokens hashed at rest | BE-001 (gen/hash), M1 repos | planned | |
| SEC-11 | SecretRef AAD binding | BE-003 (envelope AAD), M2 | planned | |
| SEC-18 | untrusted-text sanitizers | BE-001 (sanitizers), M6 | planned | |
| other SEC-nn | per milestone | — | todo | |

## Task graph

| Task ID | Summary | Stack | Status | Dependencies | Owner/model | Verification |
|---|---|---|---|---|---|---|
| OPS-001 | Cargo workspace scaffold, lints, profiles, toolchain | Rust | done | — | Opus | `cargo check --workspace` |
| BE-001 | aurora-core | Rust | done | OPS-001 | rust-developer | `cargo test -p aurora-core --all-features` 29 pass; clippy/fmt clean |
| BE-002 | aurora-config | Rust | done | OPS-001 | rust-developer | `cargo test -p aurora-config` 12 pass; clippy/fmt clean; invented `user` key scope removed |
| BE-004 | aurora-db infra + migrations §1–§5 | Rust/SQL | done | OPS-001 | rust-developer | migrate twice ok; `cargo test -p aurora-db` 10 pass; spec fixes S-01/S-02 |
| FE-001 | Next.js dashboard scaffold | Next.js | done | — | nextjs-developer | pnpm install/lint/typecheck/test(2)/build pass (orchestrator re-ran) |
| BE-003 | aurora-crypto | Rust | done | BE-001 | rust-developer | `cargo test -p aurora-crypto` 9 pass (fixture pinned, AAD swap, rotation); clippy clean |
| BE-005 | migrations §6–§11 + gateway role grants | SQL | done | BE-004 | rust-developer | reset+migrate from empty, rerun no-op; 16 db tests pass; grants tested on role |
| TEST-001 | schema introspection diff vs `03` | Rust test | in-progress | BE-005 | haiku-tester→rust-developer | `cargo test -p aurora-db --test schema_matches_spec` |
| BE-006 | aurora-protocol skeleton + schema gen | Rust | done | BE-001 | haiku-worker | `cargo test -p aurora-protocol` 12 pass incl. schema drift |
| BE-007 | aurora-testkit (ephemeral PG, mock upstream) | Rust | in-progress | BE-004 | haiku-worker | `cargo test -p aurora-testkit` |
| BE-008 | service binaries: health/metrics/config, `migrate` | Rust | in-progress | BE-001, BE-002, BE-004 | rust-developer | run binaries; curl health |
| OPS-002 | CI workflow + cargo-deny + arch-guard scripts | CI | blocked | BE-008, FE-001 | haiku-worker | `act`-free: run scripts locally |
| OPS-003 | compose skeleton + Containerfiles + Caddyfile + setup.sh | Ops | blocked | BE-008, FE-001 | rust-developer | `podman compose config`; up (disk permitting) |
| API-001 | M1 contract schemas in API_CONTRACTS.md | Contract | done | — | Opus | review |
| BE-010 | API foundation: state, errors, extractors, authn/CSRF/authz, OpenAPI, audit | Rust | planned (brief) | BE-008 | rust-developer | brief acceptance |
| BE-012 | Tenancy: orgs/workspaces/members/access + creation txn + Daneel | Rust | planned (brief) | BE-010 | rust-developer | brief acceptance |
| BE-011 | Auth flows: signup/login/verify/reset/change, mailer, password policy | Rust | todo | BE-012 | rust-developer | |
| BE-013 | Invitations | Rust | todo | BE-012 | rust-developer | |
| BE-014 | API keys + CLI device flow + /v1/me/api-keys | Rust | todo | BE-010 | rust-developer | |
| BE-015 | OIDC + mock IdP (testkit) | Rust | todo | BE-011, BE-003 | rust-developer | |
| BE-016 | Instance admins, bootstrap-admin, admin users, /v1/me, deletion | Rust | todo | BE-011 | rust-developer | |
| TEST-010 | Permission-matrix test (route × role) | Rust test | todo | BE-011..016 | haiku-tester | |
| FE-010 | Auth pages (login, signup, forgot, reset, verify, join, cli, setup-error) | Next.js | todo | BE-011 | nextjs-developer | |
| FE-011 | App shell: org/workspace nav, resolve, overview (agents), security headers | Next.js | todo | BE-012 | nextjs-developer | |
| FE-012 | Org pages: workspaces, members, invitations, API keys, audit, settings | Next.js | todo | BE-012..014 | nextjs-developer | |
| FE-013 | Account, workspace settings/access/keys, admin users | Next.js | todo | BE-014, BE-016 | nextjs-developer | |
| TEST-011 | Playwright: sign-up → workspace (Daneel) → invite → accept | e2e | todo | FE-010..012 | haiku-tester→nextjs-developer | |
| M2–M7 epics | see FEATURES.md | both | todo | per `19` graph | — | `19` acceptance |

## Decisions requiring user input

- (none blocking yet) Installing the `wasm32-wasip2` target (rustup or distro package) will be needed before M3 hook work — I-02.

## Blockers

- none

## Recently completed

- OPS-001, FE-001, API-001, BE-001..BE-006 (2026-11-05). Spec fixes S-01, S-02 recorded in 90-decisions.md.
