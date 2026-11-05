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
| BE-001 | aurora-core | Rust | ready | OPS-001 | rust-developer | `cargo test -p aurora-core`, clippy |
| BE-002 | aurora-config | Rust | ready | OPS-001 | rust-developer | `cargo test -p aurora-config`, clippy |
| BE-004 | aurora-db infra + migrations §1–§5 | Rust/SQL | ready | OPS-001 | rust-developer | `cargo test -p aurora-db` (db) |
| FE-001 | Next.js dashboard scaffold | Next.js | ready | — | nextjs-developer | typecheck, lint, vitest, build |
| BE-003 | aurora-crypto | Rust | blocked | BE-001 | rust-developer | `cargo test -p aurora-crypto` |
| BE-005 | migrations §6–§11 + gateway role grants | SQL | blocked | BE-004 | rust-developer | migrate on empty db |
| TEST-001 | schema introspection diff vs `03` | Rust test | blocked | BE-005 | haiku-tester→rust-developer | `cargo test -p aurora-db --test schema_matches_spec` |
| BE-006 | aurora-protocol skeleton + schema gen | Rust | blocked | BE-001 | haiku-worker | `cargo test -p aurora-protocol` |
| BE-007 | aurora-testkit (ephemeral PG, mock upstream) | Rust | blocked | BE-004 | haiku-worker | `cargo test -p aurora-testkit` |
| BE-008 | service binaries: health/metrics/config, `migrate` | Rust | blocked | BE-001, BE-002, BE-004 | rust-developer | run binaries; curl health |
| OPS-002 | CI workflow + cargo-deny + arch-guard scripts | CI | blocked | BE-008, FE-001 | haiku-worker | `act`-free: run scripts locally |
| OPS-003 | compose skeleton + Containerfiles + Caddyfile + setup.sh | Ops | blocked | BE-008, FE-001 | rust-developer | `podman compose config`; up (disk permitting) |
| API-001 | M1 contract schemas in API_CONTRACTS.md | Contract | todo | — | Opus | review |
| M1 epic | identity, tenancy, API skeleton, dashboard shell | both | todo | M0, API-001 | — | `19 §M1` acceptance |
| M2–M7 epics | see FEATURES.md | both | todo | per `19` graph | — | `19` acceptance |

## Decisions requiring user input

- (none blocking yet) Installing the `wasm32-wasip2` target (rustup or distro package) will be needed before M3 hook work — I-02.

## Blockers

- none

## Recently completed

- OPS-001 workspace scaffold (2026-11-05).
