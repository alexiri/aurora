# Phase 4 exit verification (AUDIT-004)

## Coordinator's verdict (2026-11-14)
**Verdict: pass, conditional on FIX-148 (F-3) merging.** Both halves ran on one commit, `1f77f68f`:
- **Run half** (this file, steps 1–4, 7, 8, 10): green, except the findings below.
- **Static half** (`phase4-exit-static.md`, steps 5, 6, 9, 11): green.
  - Every C-number in C-400..C-720 has an implementation.
  - OpenAPI, schema, permission matrix (0 inconclusive) and the 67 migrations are clean.
  - 434 of 452 SEC-cited test names are in `passed-tests.txt`. The other 18 are 9 e2e tests (matched below) and 9 names that are not tests.
  - The SCIM suites have 89 tests, all passed.
  - Its doc findings F-S1..F-S11 are fixed: F-S1/F-S2 in its lane, F-S7 by the coordinator, the rest by FIX-143 (merged `0eb253b3`). F-S12 was withdrawn.

**The nine SEC-cited e2e tests** (static half §Execution cross-check), matched to this lane's step-8 runs:
- **In `hosted.sh`** (15 passed, then 15 passed with the full image): `s8_security_probes`, `s10_freeze_…` and `s11_database_lane_psql_…` run in the podman run. `s11`'s `AURORA_E2EH_DBTARGET` gate is set by the script. `s12_codex_…` needs `AURORA_E2EH_FULL_IMAGE_TAG`, which the full-image run sets.
- **In `gateway.sh`** (24 passed: database 1, gateway 17 …): `d1_database_lane_…` and `s19_custom_only_…`.
- **In `channels.sh`** (6 passed): `s1_…`, `s3_…` and `s5_…`.

Evidence is at suite level: the scripts set every gate these tests check, and each suite passed with 0 failures.

**Findings and their state:**

| ID | State |
|---|---|
| F-1 | Fixed on master `73d07309`. Verified: `upgrade.sh` from `a2d921ff` gives 38 passed, 0 failed. |
| F-2 | Fixed on master `d36213d2`. The validator was wrong; the test now fails on the old rule. |
| F-3 (medium) | Open. FIX-148 item 1 fixes it before sign-off. |
| F-4, F-5, F-6 (low) | Open. FIX-148 items 2–4. |

**Deviations accepted:**
- Performance budgets are run by the coordinator on an idle machine and recorded separately (perf-only-when-idle).
- No SCIM conformance client exists. The Entra, Okta and Keycloak fixture replays are the evidence (26/26 live).
- These were not run: a live terminal session in a browser, a browser preview over wildcard TLS, real IdP consoles.

Phase 4 closes when FIX-148 merges and its gates pass. The coordinator records that on the PROGRESS row AUDIT-004.

---

# Run lane report (`audit-run`)

**Run half complete** (steps 1, 2, 3, 4, 7, 8, 10). The final verdict is the coordinator's, after folding in `phase4-exit-static.md`.
The values file and kind config used for step 10 are `phase4-logs/step10-values.yaml` and `step10-kind-config.yaml`.

**Audited commit:** `1f77f68f38d39c1322ef2b2ecccbdce6c85a9ff3` (master HEAD at lane start; branch `alexiri/audit-run`).
Static lane (`audit-static`, steps 5, 6, 9, 11) is in `phase4-exit-static.md`; the coordinator folds it in.

**Provisional verdict (run half, steps 1–4, 7, 8, 10):** every build, lint, test, web, Vault, kind and e2e command is green
(0 test failures: 2400 nextest, 705 vitest, 538 + 5 Playwright, all e2e suites) **except `upgrade.sh`** (F-1: 35/3 from the
phase-3 base, 32/4 from the phase-0 base; a restart-after-upgrade delay of up to 90 s that the script did not wait for; **fixed on
master afterwards**, see F-1). Step 10 (install from scratch, SCIM guide, previews and terminal notes, `helm upgrade`, `helm uninstall`)
worked end to end on the audited commit: the SCIM management and protocol surface, token rotation and metrics behave as the guide
says and the three vendor fixture replays pass 26/26 against the live cluster. Step 10 found **one defect that blocks a documented
feature** (F-3: the Directory sync error log never fills, `record_scim_error` has no caller), one that blocked the install (F-2,
**fixed on master**), and three doc/reality gaps (F-4 dashboard has no relay line, F-5 uninstall leftovers, F-6 `PUT .../scim` response).
None is a security or data-integrity defect. Provisional run-half verdict: **pass with open findings F-2 (closed by the fix), F-3..F-6
(fix tasks)**; F-3 is the one to fix before sign-off. Not final until the coordinator folds in `phase4-exit-static.md`.

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

Step 10 (install from scratch, `docs/operator/install-helm.md`, cluster `aurora-audit4`):

| # | Command | Exit | Result | Duration |
|---|---------|------|--------|----------|
| 10 | doc's image loop: `podman build` api, gateway, hookhost, runner, connector-host, relay, web, sandbox, dev-idp (`REG=localhost/aurora-audit4 TAG=local`) | 0 each | all 9 builds exit 0 (layer cache warm from kind.sh) | 6m41s |
| 10 | `kind create cluster --name aurora-audit4` (ingress-ready node, host port 4154) + `deploy/kind/ingress-nginx.yaml` + `kind load image-archive` ×9 | 0 | ingress-nginx rolled out | ~3m |
| 10 | `helm lint` / `helm template` / `kubeconform` (values: `step10-values.yaml`: relay + terminal + dev IdP + bundled Postgres) | 0 / 0 / 0 | 0 charts failed; 2125 lines; **55/55 valid** | seconds |
| 10 | `helm install … --wait --timeout 8m`, attempt 1 (previews domain `previews.audit4.example.test`) | **1** | `migrate` hook CrashLoop → `--wait` timed out at 8m58s: `previews.domain: must be a lower-case DNS name of at least two labels…` — see F-2 | 8m58s |
| 10 | same after `helm uninstall` + Postgres cleanup lines of the doc + `kubectl delete ns aurora` (failed hook Job/Secret remained), domain `previews.auditfour.example.test` | 0 | all 9 Deployments Ready, 3 hook Jobs done | 1m09s |
| 10 | post-install checks | 0 | `/v1/instance` 200 (dev provider listed, `setup_errors: []`); `/healthz`, `/metrics`, `/internal/v1/status` 404 through the ingress (as the doc says); `/` 200; `bootstrap-admin` through `kubectl exec … /usr/local/bin/aurora-api` printed the doc's message; dev-IdP sign-in → `is_instance_admin: true`; `GET /v1/admin/runners`: runner `backend: kubernetes`, capabilities incl. `terminal` | |
| 10 | SCIM guide, step 1: `helm upgrade aurora deploy/helm/aurora -n aurora -f step10-values.yaml --set scim.enabled=true --wait` | 0 | revision 2; every Deployment rolled (config changed), `/scim/v2/*` routed at the edge (401 SCIM error body without a token) | 30s |
| 10 | SCIM steps 2–3 through the API (owner session; `412 reauth_required` first, then the dev IdP's `reauth=1` flow): `PUT /v1/orgs/{org}/scim` with domains, `POST …/scim/tokens` | 0 | org switch on, 4 domains, `aud_…` token shown once; token limit: 5 active ok, the 6th → 422 `scim_token_limit_reached` | |
| 10 | Vendor fixture replay against the live instance (`services/aurora-api/tests/fixtures/scim/{entra,okta,keycloak}/provisioning.json`, a small `curl`/`jq` loop in the scratchpad, statuses compared, one token per IdP, org domains `contoso.example`, `okta.example`, `keycloak.example`) | 0 | Entra 9/9, Okta 9/9 (incl. the 409), Keycloak 8/8 | seconds |
| 10 | Rotation: create second token, both 200, `DELETE /v1/scim/tokens/{old}` → 204, old → 401 + `WWW-Authenticate: Bearer`, new → 200; no-token → 401; an `aud_` bearer on `/v1/me` → 401 | 0 | as the guide says | |
| 10 | Negative probes: domain outside the org's list, `roles` attribute, unsupported filter operator | 0 | 400 / 400 / 400 | |
| 10 | Monitoring: API pod :4101 `/metrics` (port-forward), `/healthz`, `/readyz` | 0 | `aurora_scim_requests_total{outcome=ok}` 29, `unauthorized` 3, `client_error` 4; `aurora_scim_auth_failures_total` 3; both probes 200. `GET …/scim/status`: users `removed` 3, groups 3, `last_request_at` set, **`errors` empty despite 4 rejected requests (F-3)** | |
| 10 | Previews/terminal notes: relay Ready and registered (`GET /v1/admin/relays` lists it), relay :4193 `/healthz` `/readyz` 200, org `previews_enabled` switched on through `PATCH /v1/orgs/{org}`, NetworkPolicies read back (below) | 0 | relay `aurora-relay` policy: ingress 4112 from a namespaceSelector only; egress api 4101 and runner 4108 only; runner policy admits 4105 (namespace) and 4108 (two podSelectors: relay + api); runner reports `terminal` | |
| 10 | `helm upgrade … --set scim.enabled=true --set web.replicas=2 --wait` (the doc's value-only upgrade) | 0 | revision 3, `aurora-web` 2/2, UI 200, SCIM token still valid; no migrate Job left (hooks deleted) | 19s |
| 10 | `helm upgrade` with `scim.enabled` unset (back to false) | 0 | revision 4; `/scim/v2/*` answers 404 with and without a token (the edge route is gone, so the 404 comes from the web app, not the API) | 30s |
| 10 | `helm uninstall aurora -n aurora --wait` | 0 | release removed in 6s; what stays: see Leftovers (F-5) | 6s |
| 10 | `KIND_EXPERIMENTAL_PROVIDER=podman kind delete cluster --name aurora-audit4` | 0 | `kind get clusters` → none; `podman ps -a --filter name=audit4` empty; no audit4 image, volume or network | |

Step 10 notes (operator guides). Read literally and run as written; the points where the guides and the cluster differ:
- **SCIM guide, accurate:** every behaviour tested matched (instance switch → edge route + `[scim]`; owner-only org switch with re-auth;
  one-time token; 5 active; rotation order; SCIM 401 body identical for causes; `aurora_scim_*` metric names and outcomes; owner-less
  scope). It never says how to do the dashboard steps through the API; the lane used the C-471/C-472 routes with a session cookie and
  the dev IdP's re-auth (`/v1/auth/oidc/dev/start?reauth=1&login_hint=…`). The Entra/Okta/Keycloak console sections cannot be
  followed without those products; the fixture replays are the evidence (as the guide says), and they pass.
- **F-3** ("The Directory sync page shows … the last rejected requests per org"): see below.
- **F-6**: `PUT /v1/orgs/{org}/scim` answers 200 with a body that does not reflect the stored settings (observed `enabled:false`,
  `email_domains:[]` right after saving `enabled:true` with four domains; a following `GET` is right). C-472 does not specify the body.
- **Previews (docs/15 §Port previews, `operator/previews.md`):** the Helm side is accurate (one switch, `relay.domain` mirrors
  `config.previews.domain`, wildcard Ingress host `*.previews.auditfour.example.test`, NetworkPolicy table matches the rendered
  policies, relay probes and metric port 4193). The guide's "wait for … **Admin → Overview → Previews** to show the relay replicas" has
  no dashboard counterpart in this build: `GET /v1/admin/relays` exists (it lists the relay; a stale replica row from before the upgrade
  stays until the prune job runs) but nothing in `web/src` calls it (F-4). Not exercised here: DNS/TLS wildcard (the cluster is plain
  http on localhost; previews are https only), an actual preview share (covered by `kind.sh` `k6` in step 7).
- **Terminal (`operator/terminal.md`):** `terminal.enabled=true` rendered `config.terminal.enabled` and the runner's `terminal`
  capability and :4108 rule as described; no extra workload. A live terminal session was not opened on this cluster (the guide's
  flows are covered by `terminal.prod.spec.ts` and the hosted e2e in steps 4 and 8); the WebSocket-edge recipe is for nginx front
  doors and was exercised by `FRONTDOOR=1 gateway.sh` (step 8), not by this ingress-nginx.
- **install-helm.md steps 8–9:** `helm upgrade` is value-only and green (hooks run and are removed). `helm uninstall` leaves more than the guide
  lists (F-5). The `aurora-sandboxes` namespace is gone after uninstall (checked once, after; whether it existed during the run was not checked).

## Open findings (run lane)

| # | Severity | Finding | Evidence | Candidate fix |
|---|----------|---------|----------|---------------|
| F-1 | medium (exit-criterion command fails as run) | `UPGRADE_STRICT=1 UPGRADE_BASE=a2d921ff… bash scripts/e2e/upgrade.sh` exits 1, deterministic over 3 runs (35/3); with the phase-0 base 32/4. Everything up to the restart is green (migrations 38→67, old supervisor + new runner/api turn, data readable, TLS). After `POST /v1/agents/{id}/restart` (202) the sandbox **keeps its boot and its old container/image**. Cause (from the code): the old runner replica cannot retire (the script stops the API first: `retire failed: transport: error sending request`), `sandboxes.owner_replica_id` still points at it, `claim_stops` only claims `owner_replica_id = <this replica>`, and `take_over` waits until the old replica's `last_seen_at` is older than `REPLICA_ONLINE_THRESHOLD` (90 s). With a 100 s sleep before the restart POST the same run passes 38/0. So after an upgrade (or any runner crash) restart/park/stop of its sandboxes is deferred up to 90 s, and `upgrade.sh` assumes it is immediate (it passed that way in phase 3: AUDIT-003 36/0 on the phase-0 base) | `8-upgrade-p3*.log` summary lines in the table above; `crates/aurora-db/src/repo/sandboxes.rs` `take_over`/`claim_stops`; `crates/aurora-core/src/limits.rs:175` | Decide: either `upgrade.sh` polls (≥ 100 s deadline) for the new boot, or the runner retires its replica when the API is unreachable at shutdown / the new replica of the same runner id takes over at once; document the ≤ 90 s window in `docs/15 §Upgrades` |
| F-2 | low | The previews domain validator (`crates/aurora-config/src/validate.rs:2756 valid_preview_domain`) requires every label to start **and end** with a lower-case letter (2–63 chars): `previews.audit4.example.test` (label `audit4` ends in a digit) and e.g. `previews2.example.org` are refused, which DNS allows. Two consequences: the message does not name the rule, and `helm lint`/`helm template` accept the value, so the failure only shows as a `migrate` hook crash-loop and an `--wait` timeout of the full 8 min (a failed install also leaves the hook Job/Secrets behind: `helm uninstall` + the doc's two `kubectl delete` lines are not enough, the namespace had to go) | `helm install` attempt 1 above; `kubectl logs job/aurora-migrate` | Allow digits in labels (RFC 1123: only the first character must be a letter/digit and the last not `-`), say the rule in the message, and have the chart `fail` on an invalid `relay.domain` at template time |
| F-3 | medium (a documented feature that cannot work) | The Directory sync status payload `GET /v1/orgs/{org}/scim/status` has an `errors` list ("the last rejected requests per org", C-472, `scim-provisioning.md` §Operation, docs/15) that is **always empty**: it reads audit rows `action = 'scim.error'`, and `record_scim_error` (`services/aurora-api/src/domains/scim/management.rs:563`) has **no caller** anywhere in the workspace. Live: 4 rejected requests (409, 3 × 400) and 3 × 401 later, `errors: []` (also after 20 s; the 401s have no org, so only the authenticated 4xx would be logged) | step 10 table; `grep -rn record_scim_error` → only the definition | Call it from the SCIM plane's error path (authenticated 4xx/5xx, sanitized as the doc comment says) and add an integration test that a rejected request shows in `/scim/status`; or drop the claim from the docs and the page |
| F-4 | low (doc ahead of the dashboard) | `docs/15 §Port previews` and `operator/previews.md` tell the operator to wait for "**Admin → Overview → Previews**" to show the relay replicas the API has seen. `GET /v1/admin/relays` exists, but nothing under `web/src` calls it and `GET /v1/admin/overview` has no relay field, so the dashboard has no such line | `grep -rn adminListRelays web/src` → only `schema.d.ts` | Add the Previews line to Admin → Overview (FEATURES row P4-PRV-2 says what was promised), or change the guides to `GET /v1/admin/relays` and `up{job="aurora-relay"}` |
| F-5 | low (doc gap) | After `helm uninstall` the namespace still holds the evaluation Postgres (StatefulSet, Service, PVC `data-aurora-postgres-0`: as `install-helm.md` §Database says) **and also** the Secrets `aurora-secrets` (generated: DB passwords, relay token…) and `aurora-gateway-ca` (CA **private key**) and the ConfigMap `aurora-config` (all hook resources). The guide's "what remains" (steps 9, §Database) and the chart's `NOTES.txt` cleanup lines name only the Postgres objects, so following them leaves the CA key and the generated secrets behind (a reinstall in the same namespace reuses them, which is also what makes the old PVC usable: worth saying) | `kubectl -n aurora get …` after step 10 uninstall | Extend the two doc places and `NOTES.txt` with the three objects and say which to keep for a reinstall |
| F-6 | low | `PUT /v1/orgs/{org}/scim` answers 200 with a settings body that does not reflect what was just stored (twice: `enabled:false`, `email_domains:[]`; `GET` right after is correct) | step 10 notes | Return the stored settings from the PUT (it is what the dashboard form would expect) and assert it in the SCIM management IT |

Coordinator note under F-1: fixed on master in 73d07309 (`upgrade.sh` stops the runner first, as a Helm rollout does; `docs/15 §Upgrades` documents the ≤ 90 s window after a runner dies without retiring); verified by the coordinator: `UPGRADE_STRICT=1 UPGRADE_BASE=a2d921ff bash scripts/e2e/upgrade.sh` → 38 passed, 0 failed.

Coordinator note under F-2: fixed on master in d36213d2 by the coordinator. The config validator now accepts RFC 1123 labels: 1..=63 chars, alphanumeric at both ends, last label holding a letter. Labels ending in a digit (`previews.audit4`) or one character long (`p.example.org`) were wrongly refused, which crash-looped the migrate hook. The shape test now asserts that valid domains pass validation, and it fails on the old rule. `aurora-config` 66/66, clippy clean. The audited commit is unchanged; this is recorded only. (The chart-side `fail` at template time and the message wording suggested above are not part of that fix.)

## Deviations

1. **CI:** not consulted by this lane (`gh run list` is the coordinator's); every command above ran locally on the audited commit (owner-accepted).
2. **Performance:** `scripts/perf/*` not run — run by the coordinator.
3. **Step 2 runner:** nextest (as AUDIT-004 specifies) instead of `cargo test`; doctests are therefore not in the 2400 (a `cargo test --doc` run was not done).
4. **Step 3:** the `docs/operator/vault.md` §4 operator walk-through was not repeated; the 13 `secrets_migrate` tests are the round trip.
5. **Step 4:** the eight named Playwright specs were run as part of the whole `mocked` project (538 passed) plus the `production` project (5), the `ci.yml` form; the real-stack specs `first-run`, `no-default-model` and `onboarding-journeys` passed in step 8's kind part.
6. **Step 7:** `kind.sh --fresh` / `--release-images` as written skip the previews scenario `k6`; a third run with `AURORA_E2E_PREVIEWS_DOMAIN` covers it (dev images, not release images).
7. **SCIM conformance client:** not exercised by this lane — no conformance client built; IdP replay suites are the evidence (static lane).
8. **Step 10:** done on kind (`ingress-nginx`, plain http on localhost, dev IdP). The Entra/Okta/Keycloak console procedures were not followed (no such products): the fixture replays against the live cluster stand in, as the guide itself says. No wildcard DNS/TLS, so no preview opened in a browser and no live terminal session on this cluster; SCIM dashboard page not driven (API routes used instead).
9. **Shared podman store:** `deploy/dev/kind-dev.sh up` rebuilds `localhost/aurora-dev/aurora-*:dev` — the same image names the owner's `aurora-dev` cluster uses — from the audited commit (the owner's running cluster has its own loaded copies and was not touched).

## Fixes made

None. `scripts/e2e/upgrade.sh` was edited temporarily (a `sleep 100` before the restart, for the F-1 diagnosis) and restored with `git checkout`; no source change in the commit.

## Leftovers

- Deleted at the end of step 10: kind cluster `aurora-audit4` (with the release, its evaluation Postgres, PVC, Secrets and the loaded images, which lived in the node); `kind get clusters` → none, `podman ps -a --filter name=audit4` empty, no `audit4` image, volume or network on the host.
- Kept on purpose, not mine alone: host podman images `localhost/aurora-dev/aurora-*:dev`, `localhost/aurora-sandbox:0.1.0`, `localhost/aurora-llm-sandbox:0.1.0` (the names the owner's dev tooling uses, rebuilt from the audited commit by `kind-dev.sh`/`hosted.sh`; see Deviation 9) and build-cache layers of the script runs. Images `localhost/aurora-e2e-*/…` belong to other lanes' runs and were not touched.
- Scratchpad (outside the repo): replay script and the dead cluster's SCIM tokens.
- The lane's `target/` was deleted; `/home` has 70 GB free (lowest seen 51 GB, during step 10).
- Raw logs were kept outside the repo (scratchpad); only summaries are in `phase4-logs/`.
