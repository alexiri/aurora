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

| 8 | `bash scripts/e2e/m1.sh` | 0 | 1 passed | 3m00s |
| 8 | `FRONTDOOR=1 bash scripts/e2e/gateway.sh` | 0 | nginx :8091 + haproxy :8092 front doors up; 24 passed (1 + 2 `frontdoor` + 17 + 4), 0 failed | 5m33s |
| 8 | `AURORA_TEST_PODMAN=1 AURORA_E2EH_PREFIX=audit4-e2eh bash scripts/e2e/hosted.sh` | 0 | **15 passed** | 23m23s |
| 8 | same with `AURORA_E2EH_FULL_IMAGE=1` | 0 | **15 passed**, 0 skipped | 21m06s |
| 8 | `AURORA_TEST_PODMAN=1 bash scripts/e2e/channels.sh` | 0 | 6 passed | 11m13s |
| 8 | `bash scripts/conformance.sh target/debug/aurora-connector-fake` / `bash scripts/conformance.sh` | 0 / 0 | 17 passed each (+1 unit) | 25s / 2s |
| 8 | `bash scripts/conformance.sh --mattermost` | 0 | 17 passed | 1m58s |
| 8 | `AURORA_TEST_PODMAN=1 bash scripts/e2e/mattermost.sh` | 0 | 5 passed | 8m31s |
| 8 | `AURORA_TEST_PODMAN=1 AURORA_TEST_LLM=1 bash scripts/e2e/llm.sh` | 0 | scenario smoke 4 passed, hosted `l1`, `l2` passed (2) | 4m05s |
| 8 | `UPGRADE_STRICT=1 UPGRADE_BASE=a2d921ff… (phase-3 exit) bash scripts/e2e/upgrade.sh`, run 1 and 2 (and a third with `KEEP=1`) | **1** | **35 passed, 3 failed** (`restart minted a new boot`, `restarted sandbox runs the HEAD image`, `restart_to_update still set after restart`), deterministic; 38 → 67 migrations all success | 6m39s / 2m53s |
| 8 | `UPGRADE_STRICT=1 UPGRADE_BASE=baabe11f bash scripts/e2e/upgrade.sh` (phase-0 base) | **1** | 32 passed, 4 failed (the three above + `after restart the request did not reach the upstream (TLS path broken)`) | 6m10s |
| 8 | diagnostic only: the same p3 run with `sleep 100` added before the restart POST (edit reverted, not committed) | 0 | **38 passed, 0 failed** | ~5m |

Step 8 e2e notes (all runs: load ≤ 7 on 16 CPUs, `uptime` in the summaries): see F-1 for the upgrade failure.
Upgrade base: `phase3-exit.md` names `a2d921ff` as its audited commit (it never ran an upgrade from it; `upgrade.sh` gained phase-1+
base support in FIX-024), so it was used as the "phase-3 exit commit"; the phase-0 base `baabe11f` was run for comparison.
FIX-123 (paired apps): `upgrade.sh` seeds memory, schedule, secret, a second agent and a conversation — no `github-app`/`aws-role`
connection data — so the paired-apps data move was **not** exercised by it (the 29 added migrations applied clean).
`gateway.sh` took the host lock; no suite ran concurrently with another.

Steps 1–4 notes: `docs/audit/phase4-logs/step2-nextest.txt` (per-crate counts; `passed-tests.txt` lists the 2400
passed names, format `crate::binary$path`, parsed from nextest's libtest-json-plus stream). The known flake
`plugins_upgrade::major_without_impact_…` passed. `first-run` and `no-default-model` need the kind stack (step 8, kind part).
Step 3: the `secrets migrate` round trip is the 13 `secrets_migrate` tests (inline → Vault → inline, racing migrators, CAS);
the manual walk of `docs/operator/vault.md` §4 was not repeated this phase.

## Open findings (run lane)

| # | Severity | Finding | Evidence | Candidate fix |
|---|----------|---------|----------|---------------|
| F-1 | medium (exit-criterion command fails as run) | `UPGRADE_STRICT=1 UPGRADE_BASE=a2d921ff… bash scripts/e2e/upgrade.sh` exits 1, deterministic over 3 runs (35/3); with the phase-0 base 32/4. Everything up to the restart is green (migrations 38→67, old supervisor + new runner/api turn, data readable, TLS). After `POST /v1/agents/{id}/restart` (202) the sandbox **keeps its boot and its old container/image**. Cause (from the code): the old runner replica cannot retire (the script stops the API first: `retire failed: transport: error sending request`), `sandboxes.owner_replica_id` still points at it, `claim_stops` only claims `owner_replica_id = <this replica>`, and `take_over` waits until the old replica's `last_seen_at` is older than `REPLICA_ONLINE_THRESHOLD` (90 s). With a 100 s sleep before the restart POST the same run passes 38/0. So after an upgrade (or any runner crash) restart/park/stop of its sandboxes is deferred up to 90 s, and `upgrade.sh` assumes it is immediate (it passed that way in phase 3: AUDIT-003 36/0 on the phase-0 base) | `8-upgrade-p3*.log` summary lines in the table above; `crates/aurora-db/src/repo/sandboxes.rs` `take_over`/`claim_stops`; `crates/aurora-core/src/limits.rs:175` | Decide: either `upgrade.sh` polls (≥ 100 s deadline) for the new boot, or the runner retires its replica when the API is unreachable at shutdown / the new replica of the same runner id takes over at once; document the ≤ 90 s window in `docs/15 §Upgrades` |
