# Phase 4 exit verification (AUDIT-004) — run lane (`audit-run`)

**DRAFT — steps 1–4 done; 8, 7, 10 pending (updated as the lane proceeds).**

**Audited commit:** `1f77f68f38d39c1322ef2b2ecccbdce6c85a9ff3` (master HEAD at lane start; branch `alexiri/audit-run`).
Static lane (`audit-static`, steps 5, 6, 9, 11) is in `phase4-exit-static.md`; the coordinator folds it in.

## Environment

| Item | Value |
|------|-------|
| Host | Arch Linux, kernel 6.6.31-lts, 16 CPUs; `/home` 85 GB free at start |
| Postgres | `aurora-dev-pg` 127.0.0.1:55432 (owner's); only sqlx/testkit scratch databases were created |
| My names | kind `aurora-audit4*`, podman `audit4-*` |
| Runner | `cargo nextest` (step 2) |

## Results (run lane)

Logs: summary lines only under `docs/audit/phase4-logs/`; raw logs were kept outside the repo.

| # | Command | Exit | Result | Duration |
|---|---------|------|--------|----------|
| 1 | `cargo build --workspace --all-targets` (fresh `target/`) | 0 | Finished | 13m34s |
| 1 | `cargo fmt --all --check` | 0 | clean | 9s |
| 1 | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 0 | clean | 3m56s |
| 1 | `cargo deny check` | 0 | advisories ok, bans ok, licenses ok, sources ok | 2s |
| 1 | `bash scripts/plugins/deny-hooks.sh` | 0 | 3 hook crates clean | 2s |
| 1 | `bash scripts/guards/run-all.sh` | 0 | all guards passed | 13s |
| 2 | `bash scripts/fixtures/build.sh` | 0 | passed=57 failed=0 xfail=0 | 2m11s |
| 2 | `cargo nextest run --workspace --exclude aurora-e2e-gateway --exclude aurora-e2e-hosted --no-fail-fast --message-format libtest-json-plus` (`NEXTEST_EXPERIMENTAL_LIBTEST_JSON=1`) | 0 | **2400 passed, 0 failed, 1 skipped** (`print_fixture`, `#[ignore]`), 76 binaries, 55 slow | 21m26s |
| 3 | Vault dev server `audit4-vault` (image digest as in `ci.yml`), `AURORA_TEST_VAULT=1 cargo test -p aurora-secrets` | 0 | 4 + 22 passed, 0 skipped | 24s |
| 3 | `… cargo test -p aurora-api --test it secrets_migrate` (gate on) | 0 | 13 passed | 3m50s |
| 3 | `… cargo test -p aurora-gateway --test it refused_vault_reference` (SEC-61) | 0 | 1 passed | 1m34s |
| 3 | `bash deploy/helm/check-vault.sh` | 0 | `helm-vault: ok` | 2m01s |
| 4 | `pnpm -C web install --frozen-lockfile` | 0 | up to date | 1s |
| 4 | `pnpm -C web typecheck` / `lint` | 0 / 0 | clean | 11s / 17s |
| 4 | `pnpm -C web test` | 0 | 68 files, **705 passed** | 19s |
| 4 | `pnpm -C web build` | 0 | production build ok | 31s |
| 4 | `pnpm gen:api && git diff --exit-code src/lib/api/schema.d.ts` | 0 | no client drift | 2s |
| 4 | `playwright test --project mocked` (the CI form; superset of stage4-routes, stage4-axe, onboarding, fix-027-selectors, shell-theme, stage2-narrow, smoke, chat-v2; `next dev` :4100) | 0 | **538 passed**, 0 failed | 7m42s |
| 4 | `AURORA_E2E_PROD=1 playwright test` (production project, :4110) | 0 | 5 passed (`terminal.prod.spec.ts`, CSP header) | 35s |

Steps 1–4 notes: `docs/audit/phase4-logs/step2-nextest.txt` (per-crate counts; `passed-tests.txt` lists the 2400
passed names, format `crate::binary$path`, parsed from nextest's libtest-json-plus stream). The known flake
`plugins_upgrade::major_without_impact_…` passed. `first-run` and `no-default-model` need the kind stack (step 8, kind part).
Step 3: the `secrets migrate` round trip is the 13 `secrets_migrate` tests (inline → Vault → inline, racing migrators, CAS);
the manual walk of `docs/operator/vault.md` §4 was not repeated this phase.
