# Phase 4 exit verification (AUDIT-004) — run lane (`audit-run`)

**DRAFT / PARTIAL HANDOFF (worker stopped at its context budget).** Done: steps 1, 2, 3, 4, 7, 8 and the install part of step 10.
**Remaining for the continuing worker (step 10, on the existing cluster `aurora-audit4`, release `aurora`, ns `aurora`, UI/API
`http://localhost:4154`, admin `audit4-admin@example.test` already pre-provisioned and signed in through the dev IdP):**
(a) the SCIM provisioning guide (`docs/operator/scim-provisioning.md`: `helm upgrade --set scim.enabled=true`, org Directory sync
switch + token, protocol probes, rotation, metrics), (b) the port-previews operations section (`docs/15`; relay is already enabled,
`relay.domain=previews.auditfour.example.test`) and the terminal guide (`docs/operator/terminal.md`; `terminal.enabled=true` already), (c) `helm upgrade`
(`--set web.replicas=2`) and `helm uninstall` per `install-helm.md`, (d) then delete everything (kind cluster, images, see Leftovers) and
finish this file (verdict, deviations, final leftovers). The values file and kind config used are `phase4-logs/step10-values.yaml` and
`step10-kind-config.yaml`; images were built with the doc's loop (`REG=localhost/aurora-audit4 TAG=local`) and `kind load image-archive`,
then removed from the host store (the node keeps them).

**Audited commit:** `1f77f68f38d39c1322ef2b2ecccbdce6c85a9ff3` (master HEAD at lane start; branch `alexiri/audit-run`).
Static lane (`audit-static`, steps 5, 6, 9, 11) is in `phase4-exit-static.md`; the coordinator folds it in.

**Provisional verdict (run lane, steps 1–4, 7, 8 + install part of 10):** every build, lint, test, web, Vault, kind and e2e command is
green (0 test failures: 2400 nextest, 705 vitest, 538 + 5 Playwright, all e2e suites) **except `upgrade.sh`** (F-1: 35/3 from the
phase-3 base, 32/4 from the phase-0 base — a restart-after-upgrade delay of up to 90 s that the script does not wait for; passes 38/0 with a
100 s wait). Step 10's SCIM/previews/terminal guides and the upgrade/uninstall legs are still open. Not final until the coordinator
folds in `phase4-exit-static.md`.

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

| 7 | `AURORA_E2E_CLUSTER=aurora-audit4-e2e bash scripts/e2e/kind.sh --fresh` | 0 | NetworkPolicy enforced (allowed before, blocked after default-deny); `k1`..`k5` passed, **`k6` (previews) skipped** by its gate (no `AURORA_E2E_PREVIEWS_DOMAIN`): 6 ok | 17m35s |
| 7 | `… kind.sh --release-images` (the `--fresh` cluster reused) | 0 | release-profile images; same result, `k6` skipped | 29m33s |
| 7 | `AURORA_E2E_PREVIEWS_DOMAIN=previews.e2e.aurora.dev … kind.sh --fresh` (relay on) | 0 | `aurora-relay` pod Running; `k1`..`k6` **6 passed incl. `k6_preview_round_trip_http_and_websocket`**; probes: sandbox→relay path to runner svc/pod :4108 `CONNECT` rc=0 (the k6 probe set), api :4101 control | 8m30s |

| 8 | `AURORA_DEV_CLUSTER=aurora-audit4-dev AURORA_DEV_PORT=4150 deploy/dev/kind-dev.sh up --llm-mock` | 0 | cluster + images + release up (warm cache) | 6m00s |
| 8 | `… scripts/e2e/first-run.sh` run 1 / run 2 (same stack, fresh user each) | 0 / 0 | 1 passed each (journey 30.7 s / 30.1 s, mock answer "pong") | 33s / 31s |
| 8 | `… first-run.sh e2e/no-default-model.spec.ts` | 0 | 2 passed (`first-run` + `no-default-model`) | 71s |
| 8 | `… first-run.sh e2e/onboarding-journeys.spec.ts` | 0 | 4 passed (`first-run` + onboarding journeys 1–3) | 1m46s |

Step 7 notes. **`aurora-terminator`: phase 5 (D-138), skipped.** `aurora-relay` (D-138 phase-4 service) is in the chart behind
`relay.enabled` and a `terminal.enabled`-gated runner :4108 rule. NetworkPolicy (`deploy/helm/aurora/templates/networkpolicy.yaml`),
default-deny posture plus the allow-dns policy:
- **relay** (own policy, Ingress+Egress): ingress only from the edge namespace (`networkPolicy.edgeNamespaceSelector`) on TCP **4112**;
  egress only to the api on **4101** and to the runner on **4108**; DNS via `allow-dns`; **no `aurora.external` egress** (FIX-069), nothing
  else (no gateway, no database, no sandbox namespace).
- Peers' side: the api's :4101 ingress adds the relay peer when `relay.enabled`; the runner's :4108 ingress admits the relay (previews)
  and/or the api (the terminal's WebSocket proxy, when `terminal.enabled`) — never the sandbox namespace; the api's internal 4111 and the
  sandbox-facing ports stay shut to it.
- Enforcement was *probed* by the kind run (the default-deny check, `k4` SEC-02/SEC-09 probes from a sandbox pod, `k6` relay-path
  probes); the per-rule allow/block set above is read from the rendered template, not probed port by port.
Operator note: a plain `kind.sh` never installs the relay, so the previews scenario is only exercised with
`AURORA_E2E_PREVIEWS_DOMAIN` set — the exit command as written in AUDIT-004 step 7 (`--fresh`, `--release-images`) leaves `k6` skipped;
I ran the third invocation to cover it (release-profile images were not used for the relay run).

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

Step 10 (install from scratch, `docs/operator/install-helm.md`, cluster `aurora-audit4`) — in progress:

| # | Command | Exit | Result | Duration |
|---|---------|------|--------|----------|
| 10 | doc's image loop: `podman build` api, gateway, hookhost, runner, connector-host, relay, web, sandbox, dev-idp (`REG=localhost/aurora-audit4 TAG=local`) | 0 each | all 9 builds exit 0 (layer cache warm from kind.sh) | 6m41s |
| 10 | `kind create cluster --name aurora-audit4` (ingress-ready node, host port 4154) + `deploy/kind/ingress-nginx.yaml` + `kind load image-archive` ×9 | 0 | ingress-nginx rolled out | ~3m |
| 10 | `helm lint` / `helm template` / `kubeconform` (values: `step10-values.yaml`: relay + terminal + dev IdP + bundled Postgres) | 0 / 0 / 0 | 0 charts failed; 2125 lines; **55/55 valid** | seconds |
| 10 | `helm install … --wait --timeout 8m`, attempt 1 (previews domain `previews.audit4.example.test`) | **1** | `migrate` hook CrashLoop → `--wait` timed out at 8m58s: `previews.domain: must be a lower-case DNS name of at least two labels…` — see F-2 | 8m58s |
| 10 | same after `helm uninstall` + Postgres cleanup lines of the doc + `kubectl delete ns aurora` (failed hook Job/Secret remained), domain `previews.auditfour.example.test` | 0 | all 9 Deployments Ready, 3 hook Jobs done | 1m09s |
| 10 | post-install checks | 0 | `/v1/instance` 200 (dev provider listed, `setup_errors: []`); `/healthz`, `/metrics`, `/internal/v1/status` 404 through the ingress (as the doc says); `/` 200; `bootstrap-admin` through `kubectl exec … /usr/local/bin/aurora-api` printed the doc's message; dev-IdP sign-in → `is_instance_admin: true`; `GET /v1/admin/runners`: runner `backend: kubernetes`, capabilities incl. `terminal` | |

## Open findings (run lane)

| # | Severity | Finding | Evidence | Candidate fix |
|---|----------|---------|----------|---------------|
| F-1 | medium (exit-criterion command fails as run) | `UPGRADE_STRICT=1 UPGRADE_BASE=a2d921ff… bash scripts/e2e/upgrade.sh` exits 1, deterministic over 3 runs (35/3); with the phase-0 base 32/4. Everything up to the restart is green (migrations 38→67, old supervisor + new runner/api turn, data readable, TLS). After `POST /v1/agents/{id}/restart` (202) the sandbox **keeps its boot and its old container/image**. Cause (from the code): the old runner replica cannot retire (the script stops the API first: `retire failed: transport: error sending request`), `sandboxes.owner_replica_id` still points at it, `claim_stops` only claims `owner_replica_id = <this replica>`, and `take_over` waits until the old replica's `last_seen_at` is older than `REPLICA_ONLINE_THRESHOLD` (90 s). With a 100 s sleep before the restart POST the same run passes 38/0. So after an upgrade (or any runner crash) restart/park/stop of its sandboxes is deferred up to 90 s, and `upgrade.sh` assumes it is immediate (it passed that way in phase 3: AUDIT-003 36/0 on the phase-0 base) | `8-upgrade-p3*.log` summary lines in the table above; `crates/aurora-db/src/repo/sandboxes.rs` `take_over`/`claim_stops`; `crates/aurora-core/src/limits.rs:175` | Decide: either `upgrade.sh` polls (≥ 100 s deadline) for the new boot, or the runner retires its replica when the API is unreachable at shutdown / the new replica of the same runner id takes over at once; document the ≤ 90 s window in `docs/15 §Upgrades` |
| F-2 | low | The previews domain validator (`crates/aurora-config/src/validate.rs:2756 valid_preview_domain`) requires every label to start **and end** with a lower-case letter (2–63 chars): `previews.audit4.example.test` (label `audit4` ends in a digit) and e.g. `previews2.example.org` are refused, which DNS allows. Two consequences: the message does not name the rule, and `helm lint`/`helm template` accept the value, so the failure only shows as a `migrate` hook crash-loop and an `--wait` timeout of the full 8 min (a failed install also leaves the hook Job/Secrets behind: `helm uninstall` + the doc's two `kubectl delete` lines are not enough, the namespace had to go) | `helm install` attempt 1 above; `kubectl logs job/aurora-migrate` | Allow digits in labels (RFC 1123: only the first character must be a letter/digit and the last not `-`), say the rule in the message, and have the chart `fail` on an invalid `relay.domain` at template time |

## Deviations

1. **CI:** not consulted by this lane (`gh run list` is the coordinator's); every command above ran locally on the audited commit (owner-accepted).
2. **Performance:** `scripts/perf/*` not run — run by the coordinator.
3. **Step 2 runner:** nextest (as AUDIT-004 specifies) instead of `cargo test`; doctests are therefore not in the 2400 (a `cargo test --doc` run was not done).
4. **Step 3:** the `docs/operator/vault.md` §4 operator walk-through was not repeated; the 13 `secrets_migrate` tests are the round trip.
5. **Step 4:** the eight named Playwright specs were run as part of the whole `mocked` project (538 passed) plus the `production` project (5), the `ci.yml` form; the real-stack specs `first-run`, `no-default-model` and `onboarding-journeys` passed in step 8's kind part.
6. **Step 7:** `kind.sh --fresh` / `--release-images` as written skip the previews scenario `k6`; a third run with `AURORA_E2E_PREVIEWS_DOMAIN` covers it (dev images, not release images).
7. **SCIM conformance client:** not exercised by this lane — no conformance client built; IdP replay suites are the evidence (static lane).
8. **Step 10:** partial, see the head of this file.
9. **Shared podman store:** `deploy/dev/kind-dev.sh up` rebuilds `localhost/aurora-dev/aurora-*:dev` — the same image names the owner's `aurora-dev` cluster uses — from the audited commit (the owner's running cluster has its own loaded copies and was not touched).

## Fixes made

None. `scripts/e2e/upgrade.sh` was edited temporarily (a `sleep 100` before the restart, for the F-1 diagnosis) and restored with `git checkout`; no source change in the commit.

## Leftovers

- kind cluster `aurora-audit4` (installed release, for the continuing worker) — delete with `KIND_EXPERIMENTAL_PROVIDER=podman kind delete cluster --name aurora-audit4` when step 10 is done.
- Host podman images from scripts: `localhost/aurora-dev/aurora-*:dev` (see Deviation 9), build cache layers/volumes of `kind.sh`/`kind-dev.sh`/`upgrade.sh` runs (`localhost/aurora-upg-*` images were not listed as leftovers when checked, the audit4 and `aurora-e2e` images were removed).
- The lane's `target/` was deleted; `/home` has 70 GB free (lowest seen 51 GB, during step 10).
- Raw logs were kept outside the repo (scratchpad); only summaries are in `phase4-logs/`.
