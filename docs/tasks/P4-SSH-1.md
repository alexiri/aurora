# P4-SSH-1: SSH keys, the SSH CA and certificate mint (API, dashboard, CLI)

## Goal
A person with access to a hosted agent can register an ed25519 public key and obtain a short-lived OpenSSH user certificate for that agent, from the dashboard or with `aurora ssh`, so that P4-SSH-2's terminator can let them in (D-239(a)–(d), (i)).

## Coordinator note (2026-11-14): instance settings (SPEC-SETTINGS, D-236/D-237)
This spec was written in parallel with SPEC-SETTINGS. Build the `ssh.*` keys as configuration first, as specified here. When SET-5 (limits and per-feature settings) lands, its lane classifies them by D-237. The likely result:
- The policy keys become instance settings: `ssh.cert_ttl_secs`, `ssh.sessions_per_user`/`_per_agent`, `ssh.idle_timeout_secs` and `ssh.awake_cap_secs`.
- `ssh.enabled` follows the terminal's split: an `ssh.available` deployment key rendered by the chart (the terminator Deployment, Service and NetworkPolicy), plus a movable `ssh.enabled`.
- `ssh.public_host`/`public_port`, the CA keys and `terminator.*` stay in Helm.

Don't block on this. Do keep the keys read through `aurora-config`, so SET-5 can move them without touching the SSH code paths.

## Linked requirements
- D-239; C-790, C-791, C-792, C-795 (keys part and the agent SSH page without sessions), C-797 (the `ssh.*` API config keys), C-798 (`docs/API_CONTRACTS.md` §Phase 5 — P4-SSH); SEC-93 (`docs/17`).
- `04 §SSH access` (the `can_ssh` predicate, certificate fields, audit), `04 §Permission matrix` (three SSH rows), `04 §Audit` (`ssh_key.create|delete` critical; `agent.ssh_certificate.mint` best-effort).
- `05 §SSH access` (rows tagged P4-SSH-1), `03 §Planned changes` `ssh_keys`, `ssh_mint_counters`; `02 §Limits` `SSH_KEYS_PER_USER`, `SSH_CERT_TTL`, `SSH_MINTS_PER_USER_AGENT_HOUR`; `11 §SSH access` (flow, CLI); `14 §Account` SSH keys and the agent page's **SSH** page; `15 §Configuration reference` (`ssh.*` rows), `15 §Required secrets` (`AURORA_SSH_CA_KEY`); `18 §SSH access` (API keys and certificates, dashboard).
- Migrations: `20261201000200_ssh_keys.sql`, `20261201000201_ssh_mint_counters.sql` (block `000200..209`, shared with P4-SSH-2; take these two).

## Context
- Terminal precedent (same authority, same opt-in shape): `services/aurora-api/src/domains/terminal/**` (`can_use`, BYO refusal, freeze via `domains/freeze/service.rs::ensure_agent_active`, `terminal_disabled` 404 gating).
- Access law: `crates/aurora-db/src/authz.rs` — add `can_ssh` beside `can_use` (one implementation, reused by P4-SSH-2).
- Session-only routes and the API-key refusal: `domains/keys/**`, `domains/me/**` (`session_required`).
- Audit: `crate::audit::{AuditEvent, commit_critical}`; `SECURITY_CRITICAL_ACTIONS` in `aurora-core`.
- Token prefixes/limits: `crates/aurora-core/src/limits.rs`, `crates/aurora-core/src/token.rs` (`aut_` already reserved; `auz_` is P4-SSH-2's).
- CLI: `services/aurora-cli/src/main.rs` (`clap` derive), `run.rs`/`token.rs` for API-key use and `aurora-client`.
- Web: `web/src/app/account/**` (`api-keys` is the closest page), `web/src/components/account/my-keys-view.tsx`, `key-table.tsx`; the agent page's Computer section (`agent-page.tsx`, the Terminal page as the gating example). Primitives per `14 §Visual design`.
- Crates: add `ssh-key` (RustCrypto, features `ed25519`, `rand_core`, `std`) to the workspace — it parses OpenSSH keys, computes `SHA256:` fingerprints and builds/signs certificates (`certificate::Builder`). Check the licence (Apache-2.0/MIT) and add it to `02 §Libraries`.

## Scope
### In scope
**Part A — API (Rust)**
1. Migrations and real `### \`ssh_keys\`` / `### \`ssh_mint_counters\`` sections in `docs/03` (moved out of §Planned changes; `python3 scripts/extract-schema.py --check`). Retention: prune `ssh_mint_counters` rows older than 24 h in the existing retention sweep.
2. Config `[ssh]`: `enabled` (default false), `public_host` (default: host of `server.external_url`), `public_port` (22), `cert_ttl_secs` (900; 60..86400), `ca.previous_public_keys` ([]); startup refuses `ssh.enabled` without a parsable `AURORA_SSH_CA_KEY` (formats: `ed25519:<base64 32-byte seed>` or unencrypted OpenSSH ed25519), naming the variable. Never log the key.
3. Signer seam `SshCaSigner` (C-791) with the `local` implementation; selected by config; one instance in `AppState`.
4. `can_ssh` in `aurora-db::authz` (C-791 / `04 §SSH access`).
5. Routes C-790 (keys), C-791 (mint), C-792 (`GET /v1/ssh`, with `host_keys`/`known_hosts` empty and `terminators_live: 0` until P4-SSH-2 adds the replicas table — return them from a function P4-SSH-2 will replace), `features.ssh` on `GET /v1/instance`. Every SSH route 404 `ssh_disabled` while off. The mint counter increment runs in the mint transaction (`INSERT … ON CONFLICT … WHERE count < limit RETURNING`).
6. Limits `SSH_KEYS_PER_USER`, `SSH_CERT_TTL`, `SSH_MINTS_PER_USER_AGENT_HOUR` in `limits.rs` with readers; remove their three `doc-only` lines from `scripts/guards/limits-gaps.allow`.
7. Audit rows per `04 §Audit`; `ssh_key.*` into `SECURITY_CRITICAL_ACTIONS`.
8. OpenAPI regenerated (`UPDATE_OPENAPI=1 cargo test -p aurora-api --test it openapi_matches_checked_in`, then `pnpm -C web gen:api`).

**Part B — Dashboard (Next.js)**, after Part A's schema
1. Account › SSH keys (`/account/ssh`, nav entry only with `features.ssh`): table, Add key dialog, Rename, Delete confirm, inline error texts of `14`.
2. The agent page's **SSH** page (`?tab=ssh`): Connect-with-SSH card (CLI command; plain-ssh steps with the certificate download via a Blob, filename `id_ed25519-cert.pub`), host-key card (empty-state notice "no SSH server is running" while `terminators_live` is 0), "What works". No sessions list (P4-SSH-2).

**Part C — CLI**: `aurora ssh <agent> [--key] [-- args]` and `aurora ssh cert <agent>` per C-798 (exec `ssh` on Unix; the argv builder is a pure function with unit tests).

### Out of scope
- The terminator, sessions, grants, runner and supervisor work, Helm/NetworkPolicy, the `secrets` hook keys, the sessions UI (all P4-SSH-2).
- FIDO `sk-` keys, a KMS signer, an org-level switch.

## Constraints / file ownership
- Owns: `crates/aurora-db/src/authz.rs` (the `can_ssh` addition only), new `services/aurora-api/src/domains/ssh/**`, `domains/instance` (`features.ssh`), the two migrations, `crates/aurora-core` (`limits.rs`, the audit action catalogue), `crates/aurora-config` (`[ssh]`), `services/aurora-cli/src/ssh.rs` + `main.rs` registration, `web/openapi.json`, `web/src/lib/api/schema.d.ts`, `web/src/app/account/ssh/**`, new `web/src/components/account/ssh-keys-view.tsx`, a new `web/src/components/agent/ssh-page.tsx` + its registration in the agent page's section/tab list, `Cargo.toml`/`Cargo.lock` (`ssh-key`), `docs/02` (Libraries row), `docs/03` (moved sections), `scripts/guards/limits-gaps.allow` (removals), tests named below.
- Not concurrent with a lane holding the agent page or `web/package.json` (PHASE5-PLAN: `p5-agent`); the coordinator schedules it.
- Backend authoritative: the dashboard hides the SSH page and nav when `features.ssh` is false or the caller lacks `can_use`, but the API refuses regardless.
- The CA key never appears in a response, a log line, an error or the database.

## Acceptance criteria (failing-first: each new IT fails on master)
1. `ssh_keys_*` ITs: add a valid key (canonical form stored, comment dropped, fingerprint equals the one `ssh-keygen -lf` prints for the fixture key — checked-in expected value); RSA, ECDSA, a certificate line, an options-prefixed line, a private key paste and garbage are refused with the C-790 codes; duplicate 409; 11th key 409; another person's id 404; API key on POST/PATCH/DELETE 403 `session_required`; audit rows critical with name + fingerprint only.
2. `ssh_cert_*` ITs: the SEC-93 mint auth matrix (session, own `auk_` inside and outside its workspace, `auo_`, service key, agent token, anonymous, non-`can_use`, BYO 422, frozen agent/workspace/org 423, unregistered key 422); the returned certificate parses with `ssh-key`, verifies against the CA public key, and has principal = agent id, the C-791 key id, non-zero serial, `permit-pty` as the only extension, no critical options, validity `now − 60 s … now + ttl`.
3. Mint limit: 30 mints succeed and the 31st is 429 with `Retry-After`, including when the requests run concurrently on two API pools (two `AppState`s on one database); counter rows pruned after 24 h by the sweep.
4. Every SSH route answers 404 `ssh_disabled` with `ssh.enabled = false`; startup with `ssh.enabled = true` and no/garbled `AURORA_SSH_CA_KEY` fails naming the variable.
5. Secret-grep: a test run with a known CA seed finds the seed in no log line and no response body.
6. `GET /v1/ssh` shape and `known_hosts` formatting for port 22 and a non-22 port (unit test on the formatter).
7. Web: vitest for the keys view (add, inline errors, delete confirm) and the SSH page (hidden without `features.ssh`/for BYO; certificate download; empty host-key notice); Playwright route + axe for `/account/ssh` and `?tab=ssh` in both themes; no horizontal scroll at 390 px.
8. CLI: unit tests for the argv (`StrictHostKeyChecking=yes`, `UserKnownHostsFile`, `CertificateFile`, `IdentitiesOnly=yes`, port, `<agent id>@<host>`), certificate reuse when > 60 s valid, 0600 file mode; the CLI never opens the private key path (test with an unreadable private key file).
9. `bash scripts/guards/run-all.sh` passes (limits ratchet updated).

## Verification
- `cargo fmt --all --check`; `cargo clippy -p aurora-api -p aurora-db -p aurora-core -p aurora-cli --all-targets --all-features -- -D warnings`; `cargo nextest run -p aurora-api -E 'test(/ssh_/)'` plus `openapi_matches_checked_in`, `permission_matrix`, audit coverage; `cargo nextest run -p aurora-cli`; batch gate per `docs/tasks/LANE_RULES.md`.
- Web: `pnpm -C web typecheck`, `lint`, `test`, `build`; Playwright route + axe specs.

## Dependencies
- Blocked by: phase 5 start; a free slot in the agent-page/`web/package.json` ownership chain (PHASE5-PLAN).
- Unblocks: P4-SSH-2.

## Suggested agent/model and reason
Part A `rust-developer` (Sonnet): security-sensitive (credential issuance, access law, exact counters) but well bounded by C-790..C-792. Parts B and C `nextjs-developer` / `rust-developer` (Sonnet). Opus reviews Part A (certificate fields, signer seam, counter transaction) before merge. Size M.

## Completion report
- Files changed:
- Tests/checks:
- Remaining risks:
