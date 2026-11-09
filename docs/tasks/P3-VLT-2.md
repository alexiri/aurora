# P3-VLT-2: Vault in CI, the migration round trip, the CAS race, Helm and operator docs (phase 3)

## Goal
Finish the Vault store that P3-VLT-1 built:
- a CI job against a Vault dev server;
- `aurora secrets migrate` from inline to Vault and back, proven end to end;
- the CAS race test;
- Helm values for Vault auth;
- operator docs.

`21 §Phase 3 — HashiCorp Vault` ("CI: Vault dev server; migration inline → Vault and back; CAS race test") and `19 §Phase 3` are the spec. The contract C-375 covers the migrate CLI. Exit criterion: "a CI job against a Vault dev server runs `secrets migrate` inline → Vault → inline and the CAS race test".

## Scope
0. **Wire the store into the services** (found by P3-VLT-1: nothing in the API or gateway builds the registry from config yet, so Vault is implemented but unusable).
   - The API and the gateway build their `StoreRegistry` with `StoreRegistry::from_config_with_tls(…)` from `secrets.stores[]`, passing `tls.extra_ca_bundle`. Check whether the hookhost or the runner open secrets; they don't today.
   - A failed store login at startup degrades per the spec (`StoreUnavailable` on use), never a crash loop. Say what the spec requires.
   - Write the pending gateway test from the SEC-61 row: `refused_vault_reference_injects_nothing` (a reference outside scope is refused at open, 502 `credential_unavailable` per C-373, and nothing is injected). Run it against the dev-server container, and flip the SEC-61 row to fully proven.
1. **Tests** in `crates/aurora-secrets/tests` (and the CLI's, if `secrets migrate` lives there):
   - **Migration round trip:** seeded inline secrets of every purpose → Vault → inline. Values, refs and versions are checked at each step; the gateway resolves them at each stage; nothing is left behind.
   - **CAS race:** concurrent writers to the same managed path. Exactly one wins per version, the loser retries or fails cleanly, and no value is lost or mixed.
   - Gated behind the same env flag as P3-VLT-1's integration tests, against a throwaway dev-server container (`p3-vlt-2-vault`, image pinned by digest).
2. **CI:** a `vault` job in `.github/workflows/ci.yml` (dev server as a service container) running these tests. CI can't run on GitHub now (owner-accepted), so **run the job's exact commands locally** and record the results.
3. **Helm:** values and templates for the store config (address, auth method, role, mount, `reference_scopes` keyed by **org id**), a projected service-account token for `kubernetes` auth, and `extraCaBundle` reuse. Prove it with `helm lint` and `helm template` cases.
4. **Operator docs:**
   - A Vault page in `docs/operator/` covering the Vault-side setup (KV v2 mount, policies for the API and gateway roles as `21` recommends, `max_versions` ≥ 10), auth setup per method, how to find an org id for `reference_scopes`, migration steps, and rollback.
   - Link it from the README index.

## Constraints
- Starts **after P3-VLT-1 merges**.
- **Owns:** the registry construction in `services/aurora-api` and `services/aurora-gateway` (startup only; P3-013 may be in the gateway's auth code, so stay out of it), a gateway test for SEC-61, `crates/aurora-secrets/tests` (and the CLI migrate tests), `.github/workflows/ci.yml` (the new job only), `deploy/helm/**` (Vault values and templates), `docs/operator/` (the Vault page and the README link).
- Decision D-113 only if needed.

## Acceptance criteria
- [ ] The round-trip and CAS race tests green, run locally with the flag.
- [ ] The CI job's commands run locally and green.
- [ ] `helm lint` and `helm template`, with and without Vault values.
- [ ] `cargo test -p aurora-secrets`, fmt, clippy `-D warnings`, `cargo deny check`.
- [ ] `bash scripts/guards/run-all.sh`.
