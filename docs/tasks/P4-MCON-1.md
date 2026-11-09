# P4-MCON-1: Managed connectors — API, data and protocol

## Goal
First part of phase 4 (owner, 2026-11-09, D-128): an instance admin adds a connector from the dashboard and Aurora runs it. This task builds the **API side** so that the connector host (P4-MCON-2) and the dashboard (P4-MCON-3) can be built against it. Spec: `docs/13-channels-connectors.md` §3 and **§3.1**, contracts **C-450..C-459** in `docs/API_CONTRACTS.md`, `03 §Planned changes` (`connectors`, `connector_hosts`, `connector_host_tokens`, `connector_host_status`), `08` (purpose `connector.settings`), `05` (routes and auth table), `02` (`CONNECTOR_HOST_STALE`), SEC-69, SEC-70, `18 §Managed connectors`.

## Scope
1. **Migration** (next timestamp after the newest file in `crates/aurora-db/migrations`): the `03 §Planned changes` rows for `connectors` (incl. `token_hash` nullable; keep its unique constraint) and the three new tables. Move those rows from `03 §Planned changes` into the machine-checked tables (`scripts/extract-schema.py --check` and `cargo test -p aurora-db --test it schema` must pass). Repos in `crates/aurora-db/src/repo/`.
2. **Protocol** (`crates/aurora-protocol`): `ConnectorManifest` gains optional `instance_setup: { fields }` (the integration form-field schema plus `generated_secret`); manifest widening (`13 §3`) counts a new `instance_setup` field. New module for the host plane DTOs (C-455..C-457). Keep JSON schemas (`schemas/`) in sync as the repo does for the other connector DTOs.
3. **Secrets**: `Purpose::ConnectorSettings` (`connector.settings`), included in `Purpose::ALL` and handled by `secrets migrate` like the other purposes (`services/aurora-api/src/secrets_migrate.rs` and its tests). Org = nil UUID, owner = `connectors.secret_owner`.
4. **Admin API** (`services/aurora-api/src/domains/connectors/`): C-450 catalogue; C-451 managed create (validation of `settings`/`secrets` against `instance_setup`, generation of `generated_secret` values: 32 random bytes from the OS RNG, base64); C-452 `Connector` DTO fields; C-453 PATCH (incl. `enabled` for self-hosted too); C-454 token/delete behaviour; C-459 overview field. Audit events with field names only.
5. **Host plane** (`/v1/connector-host/*`): `auh_` authentication (hash lookup in `connector_host_tokens`; bootstrap upsert from `AURORA_BOOTSTRAP_CONNECTOR_HOST_TOKEN` at start, mirroring the runner token bootstrap), C-455 register (signature verification exactly as connector `register`; reuse that code), C-456 assignments long poll (revision bumped on every managed connector change; implement with the existing event bus / `NOTIFY` pattern the ops long poll uses), C-457 status.
6. **Delegated auth** C-458 in the connector-plane authentication (`ConnectorCtx`).
7. **OpenAPI**: regenerate (`UPDATE_OPENAPI=1 cargo test -p aurora-api --test it foundation::openapi`) and `cd web && pnpm gen:api` so P4-MCON-3 can use the generated types; commit both.
8. **Config**: nothing new in `aurora.toml` for the API beyond the bootstrap env var; add the env var to `docs/15 §Required secrets` if that table lists the runner one.

## Not in scope
The `aurora-connector-host` service, connector crates, SDK, Helm, operator docs (P4-MCON-2); the web UI (P4-MCON-3). Don't touch `connectors/`, `crates/aurora-connector-sdk`, `deploy/`, `web/src` (except the generated `web/src/lib/api/schema.d.ts`).

## Acceptance
- New `services/aurora-api/tests/it/connectors_managed.rs` covering every bullet of `18 §Managed connectors` except Host/E2E/Web, with SEC-69/SEC-70 test names you then write into `docs/tests/security-coverage.md` (status `proven`). Secrets-absent checks scan every admin response body, the audit rows and a captured log for the plaintext values.
- Existing connector, channels and conformance tests stay green: `cargo test -p aurora-api --test it -- --test-threads=8` (full), `cargo test -p aurora-db`, `cargo test -p aurora-protocol`, `cargo test -p aurora-secrets`, `bash scripts/conformance.sh` (fake).
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `python3 scripts/extract-schema.py --check`, OpenAPI drift test without `UPDATE_OPENAPI`, `cd web && pnpm typecheck`, `bash scripts/guards/run-all.sh`.

## Constraints
- Never weaken an existing check. A secret value never appears in a response, log line, audit metadata, error message or test snapshot.
- Contract numbers: only C-450..C-459 (already written; if you must change a shape, ask through Orca `ask` first). Decision: D-128 exists; add D-129 only if you need a new decision.
- AUDIT-003 is running e2e suites on this machine: **do not run `scripts/e2e/*` or `scripts/perf/*`**; `scripts/conformance.sh` (fake) is fine.
