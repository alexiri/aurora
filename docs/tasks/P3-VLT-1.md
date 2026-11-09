# P3-VLT-1: HashiCorp Vault secret store (phase 3; D-12, SEC-11, SEC-61)

## Goal
Add `kind = "hashicorp-vault"` to the `SecretStore` registry: KV v2, auth `kubernetes` / `approle` / `token`, token renewal, **managed** mode and **reference** mode with `reference_scopes`.

The configuration keys and validation are in `docs/15` (API-P3's Vault rows), and the contract entries are C-370..C-379 in `docs/API_CONTRACTS.md`. Read them first. **SEC-61** (Q12: reference scopes enforced, latest version only, no cross-org path) must be proven by the test API-P3 named in `docs/tests/security-coverage.md`.

## Spec (read these)
- `21 §Phase 3 — HashiCorp Vault secret store` (the whole section).
- `08 §2` (the `SecretStore` seam, `SecretRef`, reference mode, `secrets migrate`), `17` SEC-11 and SEC-61, D-12.
- `15` (the Vault keys).

## Scope
1. **Store implementation** in `crates/aurora-secrets` (a new module) behind the existing trait:
   - **Managed mode:** `put` writes `<kv_mount>/data/<managed_prefix>/<org_id>/<purpose>/<owner_id>` with `{ value: base64, purpose, owner_id, org_id }` and returns `ext:<store>:<path>@<version>`. `get` reads that exact version and **checks purpose, owner and org** (mismatch → refuse). `delete` destroys a superseded version.
   - **Reference mode:** `resolve_reference(path, field)` reads the latest version, only if the path matches one of the org's `reference_scopes` globs, otherwise `ReferenceNotPermitted`. A reference ref has no `@version`.
   - **Auth:** kubernetes (service-account JWT), approle (role id and secret id from the environment) and token (dev only, refused outside dev mode if the config spec says so). Renewal is handled by the store; failure → `StoreUnavailable`.
2. **HTTP client:**
   - Use the existing HTTP stack (reqwest/rustls) with TLS verification on and `tls.extra_ca_bundle` honoured.
   - If you would add a Vault client crate, prefer a small direct KV v2 client over a large dependency. `Cargo.lock` and `cargo deny` must stay clean, and you must justify any new crate in the report.
   - Never log secret values or tokens.
3. **Config:** wire the `secrets.stores[]` vault entries per `15`, with startup validation.
4. **Tests:**
   - Unit tests with a mock HTTP server: managed round trip; purpose, owner and org mismatch refused; version pinning; delete/destroy; reference allowed and refused (scope glob, cross-org path); the latest version is read; renewal; `StoreUnavailable`.
   - **Integration tests against a real Vault dev server** in a throwaway container named with your worktree prefix (`hashicorp/vault` pinned by digest), gated behind an env flag like the other container tests.
   - The SEC-61 proving test → proven in `docs/tests/security-coverage.md`.

## Constraints
- **Owns:** `crates/aurora-secrets` (new store and tests), `crates/aurora-config` (store config wiring, if API-P3 only documented it), `Cargo.toml` / `Cargo.lock` / `deny.toml` (only if a crate is added), the SEC-61 coverage row.
- No API route changes unless C-37x says so.
- P3-VLT-2 (CI job, `secrets migrate` round trip, CAS race, Helm, operator docs) comes after you; don't do it here.
- Decision D-110 only if needed.

## Acceptance criteria
- [ ] `cargo test -p aurora-secrets` (unit), and the Vault integration tests with the env flag set, against a dev-server container.
- [ ] `cargo test -p aurora-config`.
- [ ] fmt, clippy `-D warnings`, `cargo deny check`.
- [ ] `bash scripts/guards/run-all.sh`.
