# Phase 3 exit verification (AUDIT-003)

Evidence for the phase-3 exit (`docs/PROGRESS.md` §Phase exit criteria → "Phase 3 (AUDIT-003)", which repeats items
4–11 of phase 2 and adds the phase-3 suites, SEC-53..SEC-58, the Q12 Vault invariant, a Vault CI job and
`upgrade.sh` from the phase-2 exit commit). Everything was run on **one commit**, from a fresh worktree `target/`,
on 2026-11-09/10. Raw logs were kept outside the repo in `/tmp/audit-003-logs/` (cited per row; they are not part of
the commit). This task verified; it did not develop.

**Audited commit:** `a2d921ffa034499c8dcaa3806f565d6a4145edc3` (branch `alexiri/audit-003`, master at the time
SPEC-P4a's brief had merged). No other commit was merged or pulled into the worktree.

**Verdict: every verification command that could run here is green (0 test failures anywhere), but the exit is NOT
fully evidenced.** Items that need a decision or a follow-up task (details in [Open findings](#open-findings)):

1. **The phase-2 → HEAD upgrade run cannot be executed as specified.** `UPGRADE_BASE=cc6ccbb… bash scripts/e2e/upgrade.sh`
   fails at its first step (F-1): the script only knows a phase-0 baseline. Substitute evidence is green:
   `upgrade.sh` from the phase-0 base `baabe11f` (36 passed, 0 failed) and the 33 phase-2 migrations → HEAD's
   `aurora-api migrate` (38 migrations, all `success`, second run a no-op).
2. **Tracking documents do not yet say what the code and tests prove:** `docs/tests/security-coverage.md` has **no row
   for SEC-53, SEC-57 or SEC-58** (the tests exist and passed) and carries duplicated SEC-34..39 rows (F-2);
   `docs/03 §Planned changes` still holds **three phase-3 rows** (F-3); 11 phase-3 rows of `docs/FEATURES.md` are still
   `todo`/`planned` (F-4).
3. **Operator docs:** Vault and deployment-specific settings have pages; **outage behaviour and alerts** exist only in
   `docs/15 §Observability`, not under `docs/operator/` (F-5). `install-helm.md` has no build command for the
   `aurora-dev-idp` image, which makes an evaluation install with `devIdp.enabled` hang (F-6).
4. **GitHub CI on the audited commit** exists and is **`failure`**: only the `web`, `vault` and `guards` jobs ran (all
   green); no other job was listed (F-7). Every `ci.yml`/`nightly.yml` command was run locally instead (owner-accepted
   deviation).
5. Not run by me, by design: `scripts/perf/*` (the coordinator), the real-cluster install (owner, FIX-017),
   project-key signing (CI-only key).

No fixes were made: the worktree contains no source change; only this file is added.

## Environment

| Item | Value |
|------|-------|
| Host | Arch Linux, kernel 6.6.31-lts, 16 CPUs, 31 GB RAM, LUKS ext. fs; `/home` 97 GB free at start, 80 GB lowest (limit 50 GB) |
| Toolchain | rustc 1.99.1 (workspace edition 2024), node v20.20.2, pnpm 10.34.9, kind v0.33.0 (podman provider, `kindest/node` default), podman 6.1.4, helm v4.3.1, kubectl client v1.37.2, kubeconform |
| Postgres | shared dev Postgres 127.0.0.1:55432 (`aurora-dev-pg`, owner's); I created and dropped `audit3_vault`, `audit3_mig`; hosted/upgrade/kind use their own |
| Concurrent load | other lanes were active: a `cargo test … permission_matrix` against the same Postgres, a kind cluster `fix-019-fr` (first-run), later `p4-mcon-2-e2e`, and a `next-server` on :4100 from another checkout for a while. None touched my names; they did slow the runs (see Deviations 7, 8) |
| Owner objects left alone | cluster `aurora-dev`, containers `aurora-dev-pg`, `aurora-dev-control-plane`, `opendays-content_*`, all pre-existing images |
| My names | kind clusters `aurora-audit3` (install), `aurora-audit3-fr` (first-run ×2); containers `audit3-vault`; hosted prefix `audit3-e2eh`; image tags `localhost/audit3-e2eh-sandbox*`, `localhost/aurora-audit3/*:local`; scripts' own `aurora-e2e`, `aurora-upg-*`, `aurora-mm2-*` |
| Audited commit | `a2d921ffa034499c8dcaa3806f565d6a4145edc3` |

## Results

Durations are wall-clock on a machine shared with other lanes. "Log" is the file under `/tmp/audit-003-logs/`.

| # | Command | Exit | Result | Duration | Log |
|---|---------|------|--------|----------|-----|
| 0 | `gh run list --commit a2d921ff…` | 0 | run 42671396359 (`ci`, push to master) **completed: failure**; jobs listed: `web` success, `vault` success, `guards` success | n/a | `0-ci-jobs.txt` |
| 1 | `cargo build --workspace --all-targets` (fresh `target/`) | 0 | Finished | 9m21s | `1-build.log` |
| 1 | `cargo fmt --all --check` | 0 | clean | 2s | `1-fmt.log` |
| 1 | `cargo clippy --workspace --all-targets -- -D warnings` | 0 | clean | 3m56s | `1-clippy.log` |
| 1 | `cargo clippy --workspace --all-targets --all-features -- -D warnings` (the `ci.yml` form) | 0 | clean | 1s | `1-clippy-allfeat.log` |
| 1 | `cargo deny check` | 0 | advisories ok, bans ok, licenses ok, sources ok | 2s | `1-deny.log` |
| 1 | `bash scripts/plugins/deny-hooks.sh` | 0 | 3 hook crates clean | 3s | `1-denyhooks.log` |
| 1 | `bash scripts/guards/run-all.sh` | 0 | all guards passed (incl. `no-deployment-defaults`) | 4s | `1-guards.log` |
| 2 | `cargo test --workspace --no-fail-fast --exclude aurora-e2e-gateway --exclude aurora-e2e-hosted -- --test-threads=8` | 0 | **1436 passed, 0 failed, 3 ignored** (phase 2: 1292); the gateway `outage` flake did not fire | 25m32s | `2-test.log`, `2-percrate.txt` |
| 3 | `AURORA_TEST_VAULT=1 … cargo test -p aurora-secrets` (Vault dev server `audit3-vault`, digest pinned as in `ci.yml`) | 0 | **22 passed** (vault_managed, vault_reference, vault_cas incl. the CAS race, vault_mock), 0 skipped | 1m01s | `3-secrets.log` |
| 3 | `… cargo test -p aurora-api --test it secrets_migrate` (Vault gate on) | 0 | **12 passed** (round trips through Vault and memory, racing migrators, CAS loss, failure between put and commit, rekey) | 4m57s | `3-migrate.log` |
| 3 | `… cargo test -p aurora-gateway --test it refused_vault_reference` (SEC-61, gate on) | 0 | 1 passed | 2m13s | `3-sec61.log` |
| 3 | `bash deploy/helm/check-vault.sh` | 0 | `helm-vault: ok` (rendered configs pass the `aurora-api` loader) | 2m04s | `3-helmvault.log` |
| 3 | operator procedure of `docs/operator/vault.md` §4 followed once (scratch DB, real `aurora-api serve`, Vault dev server) | 0 | see Step 3 notes: 3 secrets inline → Vault (dry-run, migrate, rerun), new secret lands in Vault, rollback → inline, doc's verification SQL all zero | ~10m | `vault-op/` |
| 4 | `pnpm -C web install --frozen-lockfile` | 0 | lockfile up to date | 59s | `4-install.log` |
| 4 | `pnpm -C web typecheck` | 0 | clean | 2m03s | `4-typecheck.log` |
| 4 | `pnpm -C web lint` | 0 | 0 errors, 0 warnings | 1m15s | `4-lint.log` |
| 4 | `pnpm -C web test` | 0 | 39 files, **329 passed** (phase 2: 296) | 41s | `4-test.log` |
| 4 | `pnpm -C web build` | 0 | production build ok | 2m57s | `4-build.log` |
| 4 | `pnpm gen:api && git diff --exit-code src/lib/api/schema.d.ts` | 0 | no client drift | 5s | `4-genapi.log` |
| 4 | `playwright test stage4-routes stage4-axe shell-theme stage2-narrow smoke chat-v2` (against my own `next dev` on :4120 via `AURORA_E2E_BASE_URL`) | 0 | **263 passed** (phase 2: 257) | 8m06s | `4-pw.log` |
| 4 | `next start` + `curl -sI /` (production CSP) | 0 | header identical to phase 2 (below) | 7s | `4-csp-head.txt` |
| 5 | `python3 scripts/extract-schema.py --check` | 0 | `spec_schema.json is up to date` | <1s | `5-schema.log` |
| 5 | `cargo test -p aurora-api --test it openapi_matches_checked_in` (`UPDATE_OPENAPI` unset) | 0 | 1 passed (also in step 2) | 1s | `5-openapi.log` |
| 5 | `cargo test -p aurora-api --test it permission_matrix -- --nocapture` | 0 | 3 passed; 246 route rows `checked`; `INCONCLUSIVE rows: 0 {}` | 4m14s | `5-matrix.log` |
| 5 | C-number sweep C-310..C-385 (scratch script `5-cnums.sh`) | n/a | every identifier found in code/charts/scripts (below) | <1s | `5-cnums.txt` |
| 6 | `bash scripts/fixtures/build.sh` | 0 | passed=38 failed=0 xfail=0 | 3m00s | `6-fixtures.log` |
| 6 | `bash scripts/plugins/check-builtin.sh` | 0 | 13 built-ins + `hook-example` ok; hook-crate tests 13+7+13 passed | 4m13s | `6-checkbuiltin.log` |
| 6 | `bash scripts/plugins/build-builtin.sh --build-only --out /tmp/audit-003-logs/builtin-out` | 0 | 13 packages built, unsigned (expected) | 1s | `6-buildbuiltin.log` |
| 6 | `bash scripts/plugins/check-links.sh` (network) | 0 | 81 links ok, 0 not ok | 32s | `6-links.log` |
| 7 | `bash scripts/e2e/m1.sh` | 0 | 1 passed | 7m56s | `7-m1.log` |
| 7 | `bash scripts/e2e/gateway.sh` | 0 | `gateway` 16, `hooks` 4, `database` 1: 21 passed | 10m53s | `7-gateway.log` |
| 7 | `BUILDAH_LAYERS=false AURORA_TEST_PODMAN=1 AURORA_E2EH_PREFIX=audit3-e2eh bash scripts/e2e/hosted.sh` | 0 | **14 passed**; sandbox image built with **no layer cache** on the public digest-pinned AlmaLinux 10 base (0 "Using cache" lines); `s8d` chromium **skipped by its gate** | 35m10s | `7-hosted.log` |
| 7 | same with `AURORA_E2EH_FULL_IMAGE=1` (cached layers) | 0 | 14 passed incl. `s8d_chromium_wrapper_on_the_full_image_fetches_through_the_open_proxy` | 21m01s | `7-hosted-full.log` |
| 7 | `AURORA_TEST_PODMAN=1 bash scripts/e2e/channels.sh` | 0 | 6 passed (s1..s6) | 12m17s | `7-channels.log` |
| 7 | `bash scripts/conformance.sh target/debug/aurora-connector-fake` (binary mode) and `bash scripts/conformance.sh` | 0 / 0 | 14 passed each | 1m28s / 2s | `7-conf-fake.log`, `7-conf-fake2.log` |
| 7 | `bash scripts/conformance.sh --mattermost` | 0 | 14 passed against the pinned Mattermost container; container removed | 2m48s | `7-conf-mm.log` |
| 7 | `AURORA_TEST_PODMAN=1 bash scripts/e2e/mattermost.sh` | 0 | 5 passed (rerun, see Deviations 6) | 14m13s | `7-mattermost.log` |
| 7 | `AURORA_TEST_PODMAN=1 AURORA_TEST_LLM=1 bash scripts/e2e/llm.sh` | 0 | scenario smoke ok; supervisor conformance 2 passed; hosted `l1`, `l2` passed | 4m29s | `7-llm.log` |
| 7 | `UPGRADE_STRICT=1 UPGRADE_BASE=cc6ccbb345cf03ad2b881d9585e8a19329ceac9a bash scripts/e2e/upgrade.sh` (as the brief says) | **1** | **FAILED at `baseline migrate`**: the baseline's config is invalid for phase-2 code (F-1) | 6m31s | `7-upgrade-cc6ccbb.log` |
| 7 | `UPGRADE_STRICT=1 UPGRADE_BASE=baabe11f bash scripts/e2e/upgrade.sh` (phase-0 base, the one AUDIT-002 used) | 0 | **36 passed, 0 failed, 0 defects** | 6m16s | `7-upgrade.log` |
| 7 | `bash /tmp/audit-003-logs/mig-from-p2.sh`: the 33 migrations at `cc6ccbb` applied with psql and recorded as sqlx does (SHA-384), then HEAD `aurora-api migrate` | 0 | 33 → **38** rows, `bool_and(success)` true; `email_tokens` and `users.password_hash` gone; second `migrate` exit 0 | 10s | `7-mig-p2.log`, `mig-from-p2.sh` |
| 7 | `bash scripts/e2e/kind.sh --fresh` | 0 | NetworkPolicy enforced; `k1`..`k4` 4 passed | 22m53s | `7-kind-fresh.log` |
| 7 | `bash scripts/e2e/kind.sh --release-images` | 0 | release-profile images; NetworkPolicy enforced; `k1`..`k4` 4 passed | 40m27s | `7-kind-rel.log` |
| 7 | `AURORA_DEV_CLUSTER=aurora-audit3-fr AURORA_DEV_PORT=4052 bash scripts/e2e/first-run.sh --up`, **run 1** (fresh cluster) | 0 | 1 passed (journey 69 s on a loaded machine) | 13m02s | `7-fr1.log` |
| 7 | same, **run 2** (cluster deleted between runs) | 0 | 1 passed (journey 46 s) | 14m29s | `7-fr2.log` |
| 8 | targeted checks of the `18` phase-3 suites | n/a | table in Step 8 | n/a | `2-test.log` and the step 3/7 logs |
| 9 | security-row execution check (`sec-check.py`) | n/a | see the section below | n/a | `9-sec-raw.txt`, `9-vitest-sec.log` |
| 10 | `docs/operator/install-helm.md` on `aurora-audit3` | 0 | `helm lint` 0 failed; `helm template` 1548 lines; kubeconform 43/43 valid; install (second attempt) ok; upgrade and uninstall ok | ~25m | `10-*.log`, `10-pods2.txt`, `10-checks.txt` |
| 11 | spec sweep, operator docs, FEATURES | n/a | Step 11 notes | n/a | n/a |

Production CSP (`next start`, nonce varies per request):
`default-src 'self'; img-src 'self' data:; script-src 'self' 'nonce-…'; style-src 'self' 'nonce-…'; connect-src 'self'; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'`.
It is **identical to the phase-2 audit's header**; `web/src/proxy.ts` and `web/next.config.ts` have no diff against the phase-2 exit commit
`cc6ccbb3` (`git diff cc6ccbb3 --stat -- src/proxy.ts next.config.ts` is empty).

### Step 2: totals

1436 passed, 0 failed, 3 ignored (`print_fixture` in aurora-crypto, two doctests marked `ignore`). `cargo test` does not print a package
name per binary, so the totals are per test target (`2-percrate.txt`): `tests/it/main.rs` 994 in total (aurora-api 555, aurora-gateway 174 and the
other crates' `it` binaries), unit tests (`src/lib.rs`) 302, `src/main.rs` 20, `tests/config.rs` 48, `tests/crypto.rs` 18 (+1 ignored),
`tests/scenarios.rs` 14, `tests/hooks.rs` 8, `tests/fake.rs` 6, `tests/runtime.rs` 5, and the rest 1–4 each. The two e2e crates are excluded here
and ran in step 7 (`aurora-e2e-gateway` 21, `aurora-e2e-hosted` 14 + 14 (full image) + 6 channels + 5 mattermost + 2 llm + 4 k8s ×2).
The Vault-gated tests print "skipped" in step 2 (the gate was off, as in `ci.yml`'s `rust-test`) and were executed with the gate on in step 3.

## Per-step notes

**Step 1, clean build.** `target/` did not exist before the build. No warnings from `cargo deny`.

**Step 2.** `--test-threads=8`; the known gateway `outage::metrics_and_the_state_change_log_line` flake did **not** fail (all 28 `outage::*` tests
passed in the one run), so no rerun was needed.

**Step 3, Vault.** The test support does not start a container: I started `audit3-vault` (`docker.io/hashicorp/vault@sha256:962a9287…`, the
digest `ci.yml` pins, `server -dev`, token `aurora-dev-root`, 127.0.0.1:18200) and set the three `AURORA_TEST_VAULT*` variables.
`ci.yml`'s `vault` job has four more commands than the brief names; all four were run: `cargo test -p aurora-secrets`,
`secrets_migrate`, `refused_vault_reference` (SEC-61) and `deploy/helm/check-vault.sh`.
Operator procedure, followed literally from `docs/operator/vault.md` §1, §2, §4 and §5 on a scratch database `audit3_vault` with a real
`aurora-api serve`, signing in through the dev IdP with curl and creating three org secrets through the API:
- `secrets migrate --to vault --dry-run` → `secret.value: would move 3`, every other purpose 0; `migrate --to vault` → `moved 3, failed 0`;
  a rerun → `already in target 3`. The three `value_ref`s read `ext:vault:aurora/<org id>/secret…`; Vault listed the org under `aurora/`.
- `default_store = "vault"` + restart: a new secret got an `ext:vault:` ref; `GET /v1/secret-stores` listed
  `{id: vault, kind: hashicorp-vault, capabilities: {managed: true, references: true}}` next to `inline`.
- Rollback: `default_store = "inline"` + restart, `--to inline --dry-run` (`would move 4, already in target 1`), `--to inline` (`moved 4`,
  exit 0), the doc's six-table verification SQL → all zero (`vault-op/verify.out`), a final dry-run → `already in target 5`.
- §5 "stop Vault": with the container stopped, `GET /v1/instance` answered 200 and a reference create answered `reference_not_permitted`
  (my scratch config had no `reference_scopes`, so this is **not** a demonstration of `503 secret_store_unavailable`; that behaviour is
  covered by `vault_mock::unavailable_store_fails_closed_without_leaking` and the gateway `outage` tests). Not exercised: the value read
  back through the gateway after migration (values are write-only in the API), Kubernetes/AppRole auth against a real Vault (token auth only).
- Cleanup: API and dev IdP stopped, scratch DB dropped, `audit3-vault` removed.

**Step 4, web.** Port 4100 was occupied by a `next-server` of **another checkout** (`/home/alex/agent-platform/aurora/web`) while I started, and
`playwright.config.ts` has `reuseExistingServer: true`, which would have silently tested that checkout. I ran my own `next dev -p 4120` and pointed
Playwright at it with `AURORA_E2E_BASE_URL` (F-8). 263 passed, including axe in both themes on every route and the 390 px shell.
`m1.sh` (which uses 4100 itself) ran when the port was free.

**Step 5, contracts and data.** OpenAPI drift passed as is; `spec_schema.json` current; permission matrix 246 `checked` rows, 0 inconclusive.
C-number sweep: the phase-3 blocks of `docs/API_CONTRACTS.md` carry C-310..C-317, C-320..C-326, C-330..C-335, C-340..C-343, C-350, C-351,
C-360..C-364, C-370..C-375, C-380..C-385 (the other numbers of the reserved 10-blocks are unused; the doc says "reserved"). For each contract I grepped one to
four distinctive identifiers in `crates services web/src web/openapi.json deploy scripts schemas plugins/builtin` (`5-cnums.sh`, 50 greps):
every one has at least one non-doc file; none was missing. Thinnest hits (1–2 files, all real): C-315 `AGENT_TOKEN_RETROFIT_GRACE`,
C-321 `api:` key format, C-326 `reserved_event_id`, C-343 `breaks_connections`, C-360/C-361 the connector op schemas, C-363/C-364 deprovision failure
audit and metric, C-380 `listener_down`, C-383 gateway metrics, C-384 `GATEWAY_LISTEN_GRACE` (constants only). Matching names is not reading behaviour:
the behaviours are what steps 2, 3 and 7 execute. `docs/03 §Planned changes` is **not** empty of phase-3 rows (F-3).

**Step 6, plugins.** All 13 built-ins (atlassian, aws-keys, aws-role, github, github-app, gitlab, google-workspace, indico, linear, microsoft-365, notion,
postgresql, sentry) plus `hook-example` pass `check-builtin.sh`. The link workflow (`plugin-links.yml` → `check-links.sh`) exists and ran: 81 links ok.
Project-key signing (`--sign-only` with `AURORA_PROJECT_SIGNING_KEY`) was **not exercised** (CI-only key); `--build-only` correctly produced unsigned archives.

**Step 7, e2e.** Highlights:
- `hosted.sh`: the first run set `BUILDAH_LAYERS=false`, so the sandbox image (RFE-019) was built from scratch: `FROM docker.io/library/almalinux:10@sha256:5a4b41a4…`
  (the neutral public base, no private registry), the Rust supervisor compiled inside the build; 0 cache hits in the log. This answers phase 2's deviation 3.
  The `s8` probes passed. `s8d` is skipped by its gate in the plain run and passed in the `AURORA_E2EH_FULL_IMAGE=1` run (which reuses layers).
- `upgrade.sh`: see F-1. With `baabe11f` the whole flow ran: baseline stack with a running sandbox, HEAD migrate over the baseline schema, old supervisor
  working against the new runner/API, fail-closed old gateway transport, `restart_to_update` flagged and cleared, history/memory/schedule preserved.
  The script reports "every plane is still at protocol 1 on both refs, so N-1 does not exist yet".
- `kind.sh --fresh` and `--release-images` both enforce NetworkPolicy and pass `k1`..`k4` (the second builds with `images/services/Containerfile`, the release profile).
- First-run: both runs on a fresh `aurora-audit3-fr` cluster created by `kind-dev.sh up --llm-mock`; the cluster was deleted between runs.
  The nightly's "second user on the same stack" variant was not run (the brief asked for two fresh clusters).
- `mattermost.sh`: the first attempt was cut off by my own `timeout 590` (the e2e processes got SIGTERM, 3 of 5 tests had passed); I removed that log and reran
  the script to completion in the background (5 passed, exit 0). Only the rerun counts.

**Step 8, the `18` phase-3 suites.** Each is proven by named tests that ran green (step in brackets):

| Suite (`docs/18`) | Tests that prove it | Ran green |
|---|---|---|
| BYO token age (RFE-013, SEC-53) | `agent_token_age::{effective_maximum_none_instance_org_both_and_bounds, org_tighter_than_instance_wins_and_org_above_instance_is_refused, mint_and_regenerate_set_expires_at_and_context_credentials_are_capped, retrofit_on_introduce_and_tighten_keeps_the_grace_and_never_extends, expired_token_is_refused_by_the_agent_routes_with_agent_token_expired, last_used_at_is_written_on_agent_token_routes_at_most_once_a_minute, stale_and_expiring_lists_thresholds_filters_authorization_and_no_secrets, instance_retrofit_at_start_writes_one_system_audit_row_only_when_something_changed, …}` (10 in the file); gateway `byo::expired_agent_token_is_407_agent_token_expired_on_connect_and_absolute_form`, `byo::live_tunnel_closes_when_its_agent_token_expires`, `credentials::{expired_agent_token_is_distinguished_from_unknown_and_revoked, last_used_at_is_batched_and_throttled_to_once_a_minute, expired_context_boot_unknown_and_revoked_are_plain_refusals}`; config `agent_token_max_age_is_optional_bounded_and_named_in_the_error` | step 2 |
| Database outage incl. `readyz` (RFE-014, SEC-54) | 28 `outage::*` tests, e.g. `state_machine_ready_degraded_ready_and_readyz_bodies`, `cached_state_is_served_until_its_ttl_then_refused_and_unknown_credentials_are_503_never_407`, `a_statement_error_is_not_an_outage`, `started_without_a_snapshot_or_database_is_down_until_one_loads`, `listen_only_loss_keeps_caches_does_not_flap_within_the_grace_and_flushes_after_resubscribing`, `the_request_log_buffers_drops_the_oldest_beyond_the_cap_and_flushes_on_recovery`, `metrics_and_the_state_change_log_line`; 8 `health::tests::*`; `refresh_proxy::refresh_is_refused_while_the_database_is_unreachable` | step 2 |
| Message idempotency (RFE-018, SEC-55) | `message_idempotency::{key_validation_absent_key_and_other_routes, replay_turn_then_followup_with_current_state_and_no_second_notify, concurrent_identical_requests_create_exactly_one_turn, scope_users_session_vs_api_key_and_access_loss, replay_while_frozen_returns_stored_new_message_is_423, replay_consumes_one_message_token, mismatch_text_and_attachments_name_the_stored_row, conversation_delete_frees_the_key}`; `channels_inbox::api_prefixed_event_id_is_rejected_reserved_and_dedupe_is_unchanged`; web `composer-idempotency.test.tsx` | step 2, step 4 (`9-vitest-sec.log`: 7 tests in the two files) |
| Deployment-neutral defaults (RFE-019) | `default_brief_is_neutral_and_validated` (config); guard `scripts/guards/no-deployment-defaults.sh` (in `run-all.sh`); the sandbox image built from scratch on the public pinned base by `hosted.sh` (step 7). **Not found:** the "build with and without a `SANDBOX_BASE_IMAGE` override yields the same package list and `org.aurora.capabilities` label" smoke test of `18` (F-9) | steps 1, 2, 7 |
| Workspace-tier skills (RFE-020, SEC-56) | `skills::workspace_tier::{workspace_skill_authorization_matrix, agent_tools_cannot_write_workspace_or_org_and_list_shows_tier_and_shadows, effective_set_all_tier_and_enabled_combinations, names_tier_local_reserved_and_check_constraint, projection_reaches_workspace_agents_only_read_only_and_pruned, workspace_delete_cascades_and_overridden_in}`; `permission_matrix` (0 inconclusive); web `workspace-skills.test.tsx` | steps 2, 4, 5 |
| Default Rule and LLM hosts (RFE-023) | policy `corpus` (incl. `06_default_llm_carveout.yaml`, `07_default_denied.yaml`, `08_egress_denied.yaml`) and `connect_time_corpus`; `policy::rfe023_simulate_reports_default_rule_skipped_for_llm_host`; `governance_db::generic_secret_is_never_injected_on_governed_llm_hosts`; web policy note present in `web/src/components/policy/policy-view.tsx` ("Does not affect LLM endpoints") | steps 2, 4 |
| Plugin major upgrades (RFE-022, SEC-57) | `plugins_upgrade::*` (7: major vs minor, required vs optional fields, unknown legacy fails closed, rollback of the whole transaction, approval needs an admin, impact), `connect::connect_records_credential_key_names_and_a_reconnect_clears_a_major_upgrade_flip`, `upgrade::tests::*` (registry), gateway `refresh::flipped_connection_is_never_minted_or_refreshed` | step 2 |
| Self-contained deprovision (RFE-024, SEC-58) | `channels_mgmt::{deprovision_ops_commit_with_agent_workspace_org_integration_and_detach, deprovision_rolls_back_with_a_failed_delete, deprovision_survives_org_delete_and_runs_on_another_replica, deprovision_copy_is_bound_to_the_op_context, deprovision_copy_is_discarded_at_every_end_and_by_pruning, deprovision_retries_unavailable_for_the_window_then_fails, deprovision_failure_audit_metric_and_dropped, deprovision_is_never_claimed_without_its_credentials, shared_presences_enqueue_nothing_on_org_delete, agent_delete_enqueues_deprovision, workspace_and_integration_delete_enqueue_deprovision}`; conformance (fake and Mattermost, 14 each) | steps 2, 7 |
| Vault (Q12, SEC-61) | step 3: 22 aurora-secrets tests with the gate (`vault_reference::*`, `vault_cas::*`, `vault_managed::*`), 12 `secrets_migrate`, `refused_vault_reference_injects_nothing`, `check-vault.sh`; step 2: config `vault_reference_scope_*_refuses_startup` | steps 2, 3 |

**Step 9, security coverage.** See the section below.

**Step 10, install from scratch (`docs/operator/install-helm.md`, cluster `aurora-audit3`).** Followed in order: prerequisites (a kind config with an
ingress-ready node and host port 4054; ingress-nginx from `deploy/kind/ingress-nginx.yaml`, `rollout status` ok), the doc's own "3. Build and load" commands
(`podman build` of api, gateway, hookhost, runner, web, sandbox with `REG=localhost/aurora-audit3 TAG=local`: all succeeded, 124 s, mostly layer-cache hits),
`kind load image-archive`, Secrets option A (chart-generated), bundled evaluation Postgres, `helm lint`, `helm template`, kubeconform, `helm install --wait`,
post-install checks, `helm upgrade --set web.replicas=2`, `helm uninstall`. Values file outside the repo (`10-values.yaml`).
- **First install failed** (`--wait` timed out after 8 min, "context canceled"): `aurora-dev-idp` stayed `ErrImageNeverPull`. The doc says the chart needs
  `aurora-dev-idp` "only if you enable those", but the build loop lists no command for it (F-6); I built it from `deploy/dev/dev-idp.Containerfile` (the path `kind.sh` uses),
  loaded it, ran `helm uninstall`, removed the evaluation Postgres leftovers with the doc's two `kubectl delete` lines, and reinstalled: `helm install --wait` 1m20s,
  all Deployments Ready, three hook Jobs done.
- Checks through the ingress on `localhost:4054`: `GET /v1/instance` 200 (dev provider listed, `setup_errors: []`); `/healthz`, `/metrics`, `/internal/v1/status` answer **404** (not routed, as the
  doc now says); `/` 200. `bootstrap-admin --email audit3-admin@example.org` through `kubectl exec … /usr/local/bin/aurora-api` printed the sign-in URL as the doc says;
  sign-in through the dev IdP bound the identity (`is_instance_admin: true`); `GET /v1/admin/runners` showed the runner registered (`backend: kubernetes`, capabilities incl.
  `harnesses: [claude-code, fake]`, image `localhost/aurora-audit3/aurora-sandbox:local`; the runner logged two registration retries while the API started).
  `helm upgrade --set web.replicas=2` gave revision 2 with 2/2 web pods; `helm uninstall` left exactly what the doc lists (Postgres StatefulSet/Service/PVC, the generated Secrets).
- **§Behind an external reverse proxy:** kind does not provide Gateway API CRDs, a Gateway controller, Cilium or an external nginx. I rendered the chart with the doc's values
  (`ingress.enabled: false`, `gatewayApi.enabled: true`, `parentRefs: [{name: aurora}]`, `networkPolicy.cilium.enabled: true`, `externalUrl: https://aurora.example.org`):
  `helm lint` 0 failed, `helm template` ok, 48 resources (kubeconform 44 valid, 4 skipped for missing CRD schemas), an `HTTPRoute`, **three `CiliumNetworkPolicy`** objects, no `Ingress`.
  **Not exercised:** the Gateway and its HTTPS listener, the Cilium behaviour, the nginx recipe, `trusted_proxies`/`client_address` behind a real proxy (`GET /v1/me` showed a node-network
  address on kind), PROXY protocol.
- **Not run:** a chat turn on this install (it has no LLM; the same chart and release images pass `k1`..`k4` in step 7 and the first-run journey passes on the dev profile),
  the real-cluster checklist at the end of the doc (**owner, in progress, FIX-017**).
- Doc/reality deviations: F-6; the doc has no recipe for an ingress controller on kind (I used `deploy/kind/ingress-nginx.yaml` and a kind config with `extraPortMappings`, as phase 2 did);
  the doc's "Open limitation … none of the lines above has been run on a real cluster" remains true.

**Step 11, spec sweep and docs.** `grep -rn "(phase 3" docs/*.md` excluding `80`, PROGRESS, FEATURES (and `90`, the decision log): **89 markers in 19 documents**
(05: 10, 17: 10, 02: 7, 14: 8, 15: 8, 18: 8, 08: 8, 03: 5, 06: 5, 10: 4, 13: 4, 04: 3, 01: 2, 16: 2, 00, 07, 09, 20, 21: 1 each).
Sampling: every 5th marker in file order (18 markers) was looked up in code, charts, scripts or web; plus every contract through step 5 and every suite through step 8.
Found: `agents.default_brief` (00, 10), `REQUEST_LOG_OUTAGE_BUFFER` (02), `app_connections.credential_keys` migration `20261120000001_connection_credential_keys.sql` (03), the `api:` reserved event id (13),
org-id-keyed `reference_scopes` (08), `agent_tokens.max_age_days` (08), the Default Rule note in the policy UI (14), `Idempotent-Replayed` (05), `agent_token_expired` (06),
effective-set skills parts (16), the `resolve_cache_ttl_secs > 300` start warning (`services/aurora-gateway/src/main.rs:147`, 15), `@latest` reference refs (21), SEC-54/SEC-61 (17).
All were found. **Not read exhaustively:** the other ~70 markers.
- Operator docs: Vault (`docs/operator/vault.md`, indexed in the README) and deployment-specific settings (`deployment-defaults.md`: neutral brief, `SANDBOX_BASE_IMAGE`) exist.
  **Outage behaviour and alerts have no operator page**: `docs/15 §Observability` (lines ~383–392: alert on `aurora_gateway_state{state="degraded"} == 1` for 1 min, `aurora_channel_deprovision_failed_total`)
  is the only place; `docs/operator/limits-and-approvals.md` mentions `/readyz` only as an unlimited route (F-5).
- `docs/FEATURES.md` phase-3 rows: of 15, **4 are `done`** (RFE-013 ×2, RFE-022, RFE-024) and **11 are `todo`/`planned`** although their tests ran green here: RFE-014 ×2 (`SEC-54 pending`), RFE-018
  (`SEC-55 pending`), RFE-019 ×2, RFE-020 (`SEC-56 pending`), RFE-023, `P3/21§vault-managed`, `§vault-reference`, `§vault-ci`, and `P?/03§contract-migration` (`planned`, yet the migration
  `20261117000001_drop_password_hash_email_tokens.sql` exists and the upgrade checks in step 7 apply it) (F-4).

## Security-row execution check (step 9)

Method (scratch script `sec-check.py` outside the repo): for each row of `docs/tests/security-coverage.md`, every cited test name (the last `::` segment of each backticked path, `{a, b}` groups
expanded) was searched in the `test … ok` lines of all step 1–7 logs (`9-sec-raw.txt`). Gated tests count only where their gate was set in this audit: `AURORA_TEST_PODMAN=1`
(hosted/channels/mattermost/llm), `AURORA_E2EH_FULL_IMAGE=1` (`s8d`), kind (`k1`..`k4`), `AURORA_TEST_LLM=1`, `AURORA_TEST_VAULT=1` (step 3).

- The table has **64 rows but 58 distinct ids**: SEC-34..SEC-39 appear twice, and **SEC-53, SEC-57 and SEC-58 have no row at all** (F-2). The statuses read `proven` (54), `proven (AUDIT-002)` (9, SEC-03, 04, 06, 10, 13, 21, 22, 26 and SEC-59), `proven (P3-VLT-1 …)` (SEC-61).
  No row reads `added`, `pending` or `gap`.
- **Exit criterion: SEC-53..SEC-58 and SEC-61 `proven`.** SEC-54, 55, 56 and 61 have rows (`proven`) and **every named test ran and passed**: SEC-54 (all 18 `outage::*` names, `refresh_proxy::…`, `health::tests`, `coalesce::tests`), SEC-55
  (7 `message_idempotency` tests, `channels_inbox::api_prefixed…`, vitest `composer-idempotency`), SEC-56 (6 `skills::workspace_tier` tests, matrix 0 inconclusive, vitest `workspace-skills`), SEC-61
  (the six `vault_reference::*` tests and `vault_mock::reference_allowed_reads_latest_and_refusals_never_reach_vault` with the gate on in step 3, the two `config` tests in step 2, `refused_vault_reference_injects_nothing` with the gate on in step 3).
  (The script's "missing" list for SEC-55/56 is its parser tripping on prose inside the braces, and for SEC-52/54/60 on the path segment `tests`; I checked each name by hand against the logs.)
  **SEC-53, SEC-57 and SEC-58 are proven by tests, not by the table:** SEC-53 → `agent_token_age.rs` (10 tests) plus the gateway `byo`/`credentials` tests listed under Step 8; SEC-57 → `plugins_upgrade.rs` (7),
  `connect::connect_records_credential_key_names_…`, gateway `refresh::flipped_connection_is_never_minted_or_refreshed`; SEC-58 → the 11 `channels_mgmt::deprovision_*` tests. All passed in step 2. The table itself needs the rows (F-2).
- **Q12 Vault invariant (SEC-61):** proven and executed, with the Vault gate on (steps 3).
- **D-118 (OIDC `trusted_email_domains`) vs SEC-42:** `docs/17`'s SEC-42 row names D-118, and the tests exist and passed
  (`oidc::trusted_email_domain_counts_as_verified_only_for_exact_domain`, config `oidc_trusted_email_domains_validation`), but **`docs/tests/security-coverage.md`'s SEC-42 row does not cite them** (it still lists
  `unverified_email_is_always_refused_and_never_links` etc. only) (F-2).
- **Rows whose cited tests were not all executed:** none of the 58 rows has a cited name missing from the logs apart from the parser artefacts above. Rows that ran only under a gate: SEC-61 (Vault, step 3);
  `s8d` (SEC-41's chromium wrapper) skipped in the plain `hosted.sh` run, executed in the full-image run; kind rows via `k1`..`k4`.
- **Limits of the check:** it matches names, not assertions; file-level citations (e.g. `permission_matrix.rs` as a whole, `gw/src/health.rs::tests`) are covered by the matrix run and the module's tests but not name-checked one by one;
  a name that exists in two crates would match either; SEC-33 cites a guard script (executed by `run-all.sh`, step 1: passed).

## Deviations

1. **No usable GitHub CI (owner-accepted 2026-11-08, billing).** The audited commit's GitHub run **exists** (`ci`, run 42671396359, push) and its conclusion is **failure**; only three jobs are listed, all green
   (`web`, `vault`, `guards`); the rust, e2e and image jobs never ran. Every `ci.yml` and `nightly.yml` command was run locally instead: `rust-lint` (fmt, clippy with and without `--all-features`), `rust-deny` (`cargo deny`, `deny-hooks.sh`),
   `rust-build` (`fixtures/build.sh`, `check-builtin.sh`), `rust-test` (as `cargo test`), `e2e-gateway`, `e2e-hosted`, `web` (install, lint, typecheck, test, build, `gen:api` drift), `vault` (all four commands plus `check-vault.sh`), `guards`
   (`run-all.sh`, `extract-schema.py --check`); `nightly.yml` `first-run` (twice), `kind-e2e` (`--release-images`), `mattermost-conformance`; `plugin-links.yml` (`check-links.sh`).
2. **What the local runs do not reproduce:** the nextest sharded runner and its `ci` profile (`cargo test` ran instead); `cargo test --all-features` and the `--all-features --doc` doctest step (only clippy used `--all-features`);
   the `images-*` publish jobs, SBOM/provenance, signed built-ins (project key only on CI), arm64.
3. **Performance:** `scripts/perf/*` were not run (the coordinator runs them on an idle machine). Row left to the coordinator.
4. **Real cluster:** the checklist at the end of `install-helm.md` is **owner, in progress (FIX-017)**; not run here.
5. **`upgrade.sh` from the phase-2 exit commit** failed as written (F-1); substitutes: phase-0 base `baabe11f` (36 passed) and the phase-2 schema → HEAD migrator check (script `mig-from-p2.sh`, schema only, no data).
6. **`mattermost.sh` first attempt** was cut off by my own `timeout`; only the clean rerun is reported.
7. **Concurrent lanes.** Other lanes ran a `cargo test … permission_matrix` on the shared Postgres, kind clusters (`fix-019-fr`, `p4-mcon-2-e2e`) and a web dev server on :4100 during my runs. No name or port of mine collided, but the durations in the table are inflated, and the brief's
   "no other lane runs e2e" did not hold; the same load makes the first-run journey 69 s instead of 26 s.
8. **Playwright server:** my own `next dev` on :4120 via `AURORA_E2E_BASE_URL` instead of the config's default :4100 (occupied by another checkout; F-8).
9. **Install doc:** local images (built with the doc's own commands) and a kind ingress recipe from the repo (Step 10); the `aurora-dev-idp` image needed a manual build (F-6).
10. **First-run:** two fresh-cluster runs as instructed; the nightly's second-user-on-same-stack variant was not run.

## Fixes made

None. The worktree contains no change except `docs/audit/phase3-exit.md`. Nothing was hacked around in the repository; scratch scripts and configs are in `/tmp/audit-003-logs/`.

## Open findings

Each is a candidate fix task. Only F-1 and F-2 touch an exit criterion as written; none is a product defect found by a test.

| Id | Severity | Finding | Evidence | Suggested fix |
|----|----------|---------|----------|---------------|
| F-1 | medium (exit criterion not executable as written) | `UPGRADE_STRICT=1 UPGRADE_BASE=cc6ccbb… bash scripts/e2e/upgrade.sh` exits 1 at `baseline migrate`: `write_cfg` always writes a **phase-0 baseline** (no OIDC provider, plaintext `http://aurora-gateway:4102` advertised URL), which the phase-2 baseline binary rejects (`auth.oidc_providers: no_oidc_provider`, `gateway.advertised_proxy_url: must be an https:// URL`). Beyond the config, the baseline session is created by `POST /v1/auth/signup` (line ~206), a route removed in phase 1 (C-150), and the script's whole "old supervisor, plaintext proxy, fail-closed, restart_to_update" story is the phase-0 → phase-1 TLS change. So the script can only upgrade from a phase-0 base; the criterion "from the phase-2 exit commit" needs a different baseline mode (OIDC dev IdP config, https proxy, sign-in via the dev IdP, and a baseline-phase-aware set of assertions) | `7-upgrade-cc6ccbb.log`; `scripts/e2e/upgrade.sh` lines 76–125, 195–207; substitute runs `7-upgrade.log` (base `baabe11f`, 36/0) and `7-mig-p2.log` | A task to give `upgrade.sh` a `UPGRADE_BASE_PHASE` (or auto-detect) so a phase-1/2 base starts with OIDC + TLS config and signs in through the dev IdP; keep a data-bearing check (a conversation, a memory, a Vault-free secret) across the phase-2 → HEAD migrations. Meanwhile redefine the exit criterion as "phase-0 base (`baabe11f`) full flow + phase-2 migration chain" if the owner accepts |
| F-2 | medium (tracking table; criterion 9 wording) | `docs/tests/security-coverage.md` has **no row for SEC-53, SEC-57 or SEC-58** (so "SEC-53..SEC-58 proven" cannot be read from the table), carries **duplicate rows for SEC-34..SEC-39** (the old text with a stale file path `api/governance.rs` next to the current rows), and the **SEC-42 row does not cite the D-118 tests** (`oidc::trusted_email_domain_counts_as_verified_only_for_exact_domain`, config `oidc_trusted_email_domains_validation`). The header still says "SEC-01..SEC-60" | `9-sec-raw.txt`; `docs/tests/security-coverage.md` lines 56–89 | Add the three rows with the tests listed in Step 8/9 (SEC-53: `agent_token_age.rs` + gateway `byo`/`credentials`; SEC-57: `plugins_upgrade.rs` + `connect::…credential_key_names…` + `refresh::flipped_connection_is_never_minted_or_refreshed`; SEC-58: `channels_mgmt::deprovision_*`), delete the six duplicates, add the D-118 tests to SEC-42, rename the header to SEC-01..SEC-61 |
| F-3 | low–medium (criterion "no phase-3 rows left in `03 §Planned changes`") | `docs/03-data-model.md` §Planned changes (line ~1175) still holds **three phase-3 rows**: `organizations.governance.agent_tokens.max_age_days` (RFE-013), `turns.source_event_id` value convention (RFE-018), `plugin_packages.status_reason = breaks_connections` (RFE-022). All three are "no schema change" rows that the code has implemented (C-310, C-321, C-343). The section's own rule: "the corresponding rows move into the tables above in the same change" | `docs/03-data-model.md` §Planned changes (from line 1175); `5-cnums.txt` | Move each into the owning table's notes (or delete the rows with a note), keeping the extractor-safe level-4 headings; run `scripts/extract-schema.py --check` |
| F-4 | low (tracking) | 11 of 15 phase-3 rows of `docs/FEATURES.md` are `todo`/`planned` with "SEC-nn pending" evidence although their tests ran green here (RFE-014 ×2, RFE-018, RFE-019 ×2, RFE-020, RFE-023, the three `P3/21§vault-*` rows, `P?/03§contract-migration`); the header still says "phases 3 and 4 start on the owner's go" | the `RFE-013`..`RFE-024`, `P3/21§vault-*` and `P?/03§contract-migration` rows of `docs/FEATURES.md` | Coordinator: mark `done` with the evidence in Steps 3 and 8 |
| F-5 | medium (criterion 11 wording) | No operator page for **database-outage behaviour and alerts**: the `readyz` states, `gateway_degraded` refusals, the cache-TTL tolerance and the alert expressions (`aurora_gateway_state{state="degraded"} == 1` for 1 min, `aurora_channel_deprovision_failed_total`, `aurora_pg_notify_queue_usage` at 0.5) exist only in the design document `docs/15 §Observability`; `docs/operator/limits-and-approvals.md` mentions `/readyz` only as unlimited. Same shape as AUDIT-002's F-7 | `grep -n -i "readyz\|alert" docs/operator/*.md`; `docs/15-deployment-operations.md` ~383–445 | Either the coordinator accepts docs/15 as the operator reference (the README calls it the source of truth), or a docs task writes `docs/operator/outage-and-alerts.md` (states, what stays served for how long, what is refused with which code, the alert rules, a recovery check) and indexes it |
| F-6 | low–medium (docs; an evaluation install hangs) | `docs/operator/install-helm.md` §Images "3. Build and load" lists `aurora-dev-idp` among the images but has no build command for it (`podman build -f deploy/dev/dev-idp.Containerfile`, which `kind.sh` and `kind-dev.sh` use), and the loop covers only api, gateway, hookhost, runner, web, sandbox. With `devIdp.enabled: true` and `pullPolicy: Never` the pod sticks in `ErrImageNeverPull` and `helm install --wait` fails after the full timeout ("context canceled", release left `cancelled`; recovery needed `helm uninstall` plus the doc's two Postgres cleanup commands). Everything else in the doc worked literally; the doc's note that the build commands "were not run" can now say they were (six builds, exit 0) | `10-pods.txt`, `10-install.log`, `10-build.log`, `10-build-idp.log` | Add the dev-IdP build line (and `images/services/Containerfile` targets `connector-fake`/`connector-mattermost` if connectors are enabled), say that `devIdp.enabled` needs it, and update the "not run" sentence |
| F-7 | info → medium (owner decision) | The GitHub run for the audited commit is `completed: failure` with only `web`, `vault` and `guards` (all success) listed; the rust, e2e and image jobs have no job records. This is consistent with the billing deviation but the conclusion is `failure`, not "not started"; the owner should look at the run before relying on "CI green" for the phase-3 exit | `0-ci-jobs.txt`, `gh run view 42671396359` | Owner/coordinator: check the run page; if billing is fixed, re-run on the exit commit and record green jobs here |
| F-8 | low (test hygiene) | `web/playwright.config.ts` hard-codes port 4100 with `reuseExistingServer: true`; when another checkout's dev server is on 4100 (it was, from `/home/alex/agent-platform/aurora/web`) Playwright silently tests that code. I worked around it with `AURORA_E2E_BASE_URL` and my own server | `ss -ltnp` output in my session; `web/playwright.config.ts` | Make the port configurable (`AURORA_E2E_WEB_PORT`) or add a fail-fast check that the server on the port belongs to this checkout |
| F-9 | low (missing test named in `18`) | `docs/18 §Deployment-neutral defaults` asks for an image-build smoke test ("with and without a `SANDBOX_BASE_IMAGE` override yields the same package list and `org.aurora.capabilities` label"); I found only the guard `no-deployment-defaults.sh` and the build scripts' `SANDBOX_BASE_IMAGE` pass-through. The neutral public base was exercised by the from-scratch `hosted.sh` build; the override path was not | `grep -rln SANDBOX_BASE_IMAGE scripts tests .github images` | Add the smoke script (or amend `18` to say the guard plus one real build covers it) |

## Follow-ups

Added by AUDIT-003-FU (the findings above are left as written):

| Finding | Status | Closed by |
|---------|--------|-----------|
| F-1 | open → FIX-024 | `upgrade.sh` from a phase-2 base |
| F-2 | closed | `docs/tests/security-coverage.md`: rows SEC-53, SEC-57, SEC-58 added (`proven`), the six stale SEC-34..SEC-39 duplicates removed, the D-118 tests added to SEC-42, header range now SEC-01..SEC-79 |
| F-3 | closed | `docs/03-data-model.md`: the three phase-3 "no schema change" rows moved into the notes of `organizations.governance`, `turns.source_event_id` and `plugin_packages.status_reason`; the planned sections removed |
| F-4 | closed | `docs/FEATURES.md`: the 11 phase-3 rows marked `done` with their test evidence (Steps 3 and 8) and the header sentence fixed |
| F-5 | closed | new `docs/operator/outage-and-alerts.md`, indexed in `docs/operator/README.md` |
| F-6 | closed | `docs/operator/install-helm.md` §Images: dev-IdP build, connector image targets, "were not run" sentence updated |
| F-7 | resolved | GitHub CI runs again since 2026-11-09 (billing restored); later master runs complete |
| F-8 | open → FIX-024 | Playwright port |
| F-9 | open → FIX-024 | base-image smoke test |

## Real-cluster install and signing

- Real-cluster Helm install checklist (end of `install-helm.md`, FU-P1-005a / FIX-017): **owner, in progress**. Exit criterion 9 is evidenced for kind only.
- Project-key signing (`build-builtin.sh --sign-only`): **not exercised** (CI-only secret).
- Performance budgets (`scripts/perf/*`): **run by the coordinator**; no row here.

## Leftovers

- **Deleted:** kind clusters `aurora-audit3`, `aurora-audit3-fr` (twice) and the scripts' `aurora-e2e` (the script deletes it); container `audit3-vault`; images `localhost/aurora-audit3/*:local` (7), `localhost/audit3-e2eh-sandbox:0.1.0`,
  `localhost/audit3-e2eh-sandbox-full:0.1.0`; volumes `audit-003-kind-dev-{target,rustup,cargo-registry}`; databases `audit3_vault`, `audit3_mig` (and `aurora_e2e`, `aurora_upg_*` by their scripts);
  the scripts' own containers and networks (`aurora-upg-*`, `aurora-mm2-*`, hosted/channels anchors) removed themselves; my worktree `target/` (removed at the end).
- **Left, not mine to remove:** the images the scripts rebuilt under shared names (`localhost/aurora-sandbox:0.1.0`, `localhost/aurora-e2e/*:e2e` refreshed by `kind.sh --release-images`, `localhost/aurora-upg-sandbox:head`, `mattermost/mattermost-preview`
  pulled by the conformance scripts) and the pre-existing owner and other-lane objects (cluster `aurora-dev`, `aurora-dev-pg`, `aurora-dev-control-plane`, `opendays-content_*`, the other lanes' clusters). The `aurora-sandbox` podman *network* (shared name) and `kind`, `podman` networks exist as before.
- **In the worktree, gitignored:** `.env` and `plugins/fixtures/dist/` copied per the brief (never committed).
- Raw logs and scratch scripts stay in `/tmp/audit-003-logs/` (`run.sh`, `5-cnums.sh`, `sec-check.py`, `mig-from-p2.sh`, `10-build.sh`, `vault-op/`); they are not committed.
- `/home` free space: 97 GB at the start, 80 GB lowest, 83 GB before cleanup.
