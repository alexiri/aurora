# Phase-0 exit verification (AUDIT-001)

Evidence for the docs/19 phase-0 exit: "all milestones accepted; every SEC invariant has a passing
proving test; the compose stack and the Helm chart install from scratch with documented steps".
Everything was run from a clean worktree `target/` on base commit `2a6a0f8c` (branch
`alexiri/audit-001`, code freeze after FIX-007), 2026-11-07, plus the fixes listed under
[Fixes made](#fixes-made-and-doc-corrections). Raw logs were kept outside the repo.

**Verdict: all verification commands are green at the end of the run.** Two real defects were found
and fixed (a debug-build logging panic and a compose start-up failure); three documentation/behaviour
gaps remain open (see [Open findings](#open-findings)).

## Environment

| Item | Value |
|------|-------|
| Host | Arch Linux, kernel 6.6.31-lts, 16 CPUs, 31 GB RAM, LUKS ext. fs; `/home` 101 GB free at start, 95 GB lowest |
| podman | 6.1.4 (rootless); compose provider `docker-compose` 5.7.0 |
| kind / helm / kubectl | kind 0.33.0 (podman provider), helm 4.3.1 |
| Toolchain | rustc 1.99.1 (edition 2024 workspace), node 20.20.2, pnpm 10.34.9 |
| Postgres | shared dev Postgres 127.0.0.1:55432 (`aurora-dev-pg`); hosted.sh/upgrade/kind use their own |
| Base commit | `2a6a0f8c7338096c405eba79d2cf104ea50fb07e` |

## Results

| # | Command | Exit | Result | Duration |
|---|---------|------|--------|----------|
| 1 | `cargo build --workspace --all-targets` (fresh `target/`) | 0 | Finished | 7m25s |
| 1 | `cargo fmt --check` | 0 | clean | 1s |
| 1 | `cargo clippy --workspace --all-targets -- -D warnings` | 0 | clean | 2m19s |
| 1 | `cargo deny check` | 0 | advisories/bans/licenses/sources ok (1 warning: unmatched license allowance in `deny.toml`) | 1s |
| 1 | `bash scripts/guards/run-all.sh` | 0 | schema, no-integration-names, dep-graph, edge-routes all pass | 2s |
| 2 | `cargo test --workspace --no-fail-fast` (see note) | 101 | 893 passed, 0 failed, 3 ignored outside the e2e crates | 14m49s |
| 3 | `pnpm -C web install --frozen-lockfile` | 0 | lockfile up to date | 2s |
| 3 | `pnpm -C web typecheck` | 0 | clean | 8s |
| 3 | `pnpm -C web lint` | 0 | 0 errors, 1 warning (`window.location.assign` in `delete-view.tsx:32`) | 11s |
| 3 | `pnpm -C web test` | 0 | 19 files, 151 tests passed | 6s |
| 3 | `pnpm -C web build` | 0 | Next.js production build ok | 16s |
| 4 | `bash scripts/fixtures/build.sh` | 0 | passed=33 failed=0 xfail=0 | 1m27s |
| 4 | `bash scripts/conformance.sh target/debug/aurora-connector-fake` | 0 | conformance crate passes | 38s |
| 4 | supervisor harness conformance (fake) | 0 | `services/aurora-supervisor/tests/conformance.rs::fake_conforms` passed (step 2 and again in llm.sh) | in step 2 |
| 5 | `bash scripts/e2e/m1.sh` (Playwright) | 0 | 1 passed | 2m11s |
| 5 | `bash scripts/e2e/gateway.sh` | 101 first, **0 after fix** | first run 11/11 `gateway` + 3 passed/**1 failed** `hooks` (`h1`); after the logging fix: 11 + 4 passed | 3m43s first run |
| 5 | `AURORA_TEST_PODMAN=1 bash scripts/e2e/hosted.sh` (sandbox image rebuilt, no SKIP_IMAGE_BUILD) | 0 | `channels` 6, `hosted` 9 passed (k8s 4 and llm 2 skipped by their gates); SEC probes all `ok` | 26m22s |
| 5 | `AURORA_TEST_PODMAN=1 bash scripts/e2e/channels.sh` | 0 | 6 passed | 11m19s |
| 5 | `AURORA_TEST_PODMAN=1 AURORA_TEST_LLM=1 bash scripts/e2e/llm.sh` | 0 | scenario smoke ok; live conformance 2 passed; hosted e2e `l1`,`l2` passed | 4m45s |
| 5 | `UPGRADE_STRICT=1 bash scripts/e2e/upgrade.sh` | 0 | 32 passed, 0 failed, 0 defects (baseline 8ba42973 -> HEAD) | 5m14s |
| 5 | `bash scripts/perf/gateway.sh` | 0 | all PASS: p50 0.029 ms (<1), p99 0.091 ms (<5), 20386 req/s (>=2000), resolve p99 2.371 ms (<25) | 5m36s |
| 5 | `bash scripts/e2e/kind.sh --fresh` | 0 | NetworkPolicy enforced; `k1`..`k4` 4 passed | 16m19s |
| 6 | compose install (below) | | passes after fix | |
| 6 | helm install (below) | | passes, no fix needed | 79s install |

Final re-verification after the fixes: `cargo fmt --check` 0, `cargo clippy --workspace --all-targets
-- -D warnings` 0 (55s), `bash scripts/guards/run-all.sh` 0, `cargo test -p aurora-core` 0 (28 passed),
`bash scripts/e2e/gateway.sh` 0.

### Step 2: per-crate totals (passed / failed / ignored)

aurora-api 376/0/0, aurora-channels 11, aurora-cli 10, aurora-client 6, aurora-config 22,
aurora-connector-sdk 5, aurora-core 33, aurora-crypto 9/0/1, aurora-db 18, aurora-gateway 102,
aurora-hook-wit 1, aurora-hookhost 11, aurora-llm 34, aurora-plugin-sdk 4 (+1 doctest, 1 ignored),
aurora-policy 11, aurora-protocol 30, aurora-registry 33, aurora-runner 55, aurora-secrets 12,
aurora-summary 12, aurora-supervisor 75, aurora-testkit 4, conformance-connector 12, connector-fake 6;
aurora-config doctest 1 ignored. Total outside the e2e crates: **893 passed, 0 failed, 3 ignored.**

Note: the command excluded the e2e crates by the wrong package names (`e2e-gateway`, `e2e-hosted`; the
packages are `aurora-e2e-gateway`, `aurora-e2e-hosted`), so they ran too. `aurora-e2e-hosted` (21, all
gated tests skipping) passed. `aurora-e2e-gateway` showed 1 passed / 14 failed because it needs the
`gateway.sh` harness (ports on 127.0.0.31-34); that is why the cargo exit code was 101. All 15 pass
under `gateway.sh` (step 5). Use `--exclude aurora-e2e-gateway --exclude aurora-e2e-hosted` next time.

### Step 5 notes

- `hosted.sh` runs every test in the `aurora-e2e-hosted` crate, so it executes the `channels` tests too
  (6 passed there, 11 min); `channels.sh` then runs the same tests again over the shared dev Postgres.
- `hosted.sh` rebuilt `localhost/aurora-sandbox:0.1.0`, a tag shared with other lanes.
- Probes in `s8_security_probes` all printed `ok` (SEC-01, 02, 09, 24, 25, 26, 34).

## Step 6: install from scratch

### Compose (`docs/operator/install-compose.md`)

Run in `deploy/compose` with `COMPOSE_PROJECT_NAME=aurora-audit`. Deviations from the doc, with reasons:

1. **Project name.** Leftovers from earlier lanes exist: volumes `aurora_aurora-pg`, `aurora_aurora-ca`,
   the shared `aurora-sandbox` network (same subnet, empty, reused as `setup.sh` expects) and
   `aurora-*:dev` images dated before HEAD. A project named `aurora` would reuse that data, so I used
   `aurora-audit` and `AURORA_VERSION=audit001` in `.env` (images built fresh by `up -d --build`). Nothing
   of other lanes was modified or deleted.
2. **CA minted by an old image.** `setup.sh` found `aurora-gateway:dev` locally and used that stale image
   for `generate-ca`. The volume `aurora-ca` is fixed-name; removing it to redo this was blocked by the
   sandbox's permission classifier, so I kept it. `generate-ca` output is unchanged by the checked-in code
   and the gateway loaded it.
3. **Failure on first `up -d`.** `aurora-audit-gateway-1`: `IPAM error: requested ip address 10.89.249.2 is
   already allocated`. Postgres starts first and takes the first free dynamic address, `10.89.249.2`, which
   is the gateway's static address. Deterministic on any fresh start. **Fixed** by `ip_range:
   10.89.249.128/25` on the `core` network in `compose.yaml`. After the fix `up -d` started
   postgres, migrate, api, gateway, hookhost, web, edge.
4. Smoke test steps (all against `localhost:4000`):
   - `curl -i localhost:4000/v1/instance` -> 200 (`signup_mode: invite_only`, `hosted: true`); `curl -i localhost:4102` -> 400 (gateway answers, as documented).
   - `bootstrap-admin --email audit@example.org` printed the reset link: the subcommand works (doc claim "confirm it works" is now confirmed; the stale "available once BE-016 lands" text in `setup.sh` is removed).
   - `POST /v1/auth/password/reset` 204, `POST /v1/auth/login` 200, `POST /v1/orgs` (needs `Origin` header or `csrf_rejected` 403, SEC-22) created the org, default workspace and Daneel.
   - Runner profile: `--profile hosted up -d runner` registered with the API (fake backend).
   - `llm-mock` profile: LLM secret created, granted, agent PATCHed to `claude-code` / `litellm-mock` / `scn-pong`.
   - Chat: `POST .../conversations/direct` 200, `POST .../messages` 202. With no key granted the turn fails
     fast with `no_model_key` (correct). **With the key and the shipped `fake` backend the turn stays `queued`
     (sandbox `unprovisioned`)**, because the fake backend boots nothing: the compose smoke test therefore
     covers sign-up -> agent -> chat acceptance, but no answer comes back from a sandbox.
   - Switching to `backend = "podman"` as documented (with `sandbox_hosts` and the LLM sandbox image):
     the runner container exits with ``podman socket `/run/podman/podman.sock` does not answer: ... Connect``
     (non-root container uid vs the rootless socket). **Not fixed**; documented in `runners.md`.
     The sandbox answer path is proven by `hosted.sh`/`channels.sh`/`llm.sh`/`kind.sh`, which run the runner on the host.
   - No panics in api/gateway logs. Stack then taken down with `podman compose down` (volumes kept).

### Helm (`docs/operator/install-helm.md`) on a fresh kind cluster `aurora-audit`

Steps run: `helm lint`, `helm template` (1433 lines), `kind create cluster --config deploy/kind/kind-config.yaml`
(41s), `helm install aurora deploy/helm/aurora -n aurora --create-namespace -f deploy/kind/values-e2e.yaml
--wait --timeout 8m` (79s; hooks secrets -> gateway-ca -> migrate ran, all pods Running), then the post-install
checks. Deviations from the doc:

- The doc's minimal `my-values.yaml` assumes a registry, an Ingress, and an external database. I used the
  repo's own `deploy/kind/values-e2e.yaml` (bundled Postgres, no ingress, images loaded with
  `podman save` + `kind load image-archive`, from the `localhost/aurora-e2e/*:e2e` images that `kind.sh` built a few
  minutes earlier at the same commit). The "Install" section does not say how to get images into a kind cluster.
- `kubectl -n aurora exec deploy/aurora-api -- /usr/local/bin/aurora-api bootstrap-admin --email ...` worked
  exactly as documented and printed `http://localhost:4100/auth/reset?token=...`.
- Smoke test via `kubectl port-forward svc/aurora-api 14101:4101`: open sign-up -> verify email (token from the api log) ->
  `POST /v1/orgs` -> Daneel (harness set to `fake`, allowed by the values) -> direct conversation -> message.
  Events: `turn.queued`, `turn.started`, `text "echo: hello from the helm smoke test"`, `turn.finished done`;
  a sandbox Pod `sbx-daneel-...` ran in `aurora-sandboxes`. Cluster deleted afterwards.
- No doc/reality deviation found in Prerequisites, Secrets (Option A), Database or Gateway CA for this path.
  Not exercised here: Option B secrets, ingress/Gateway API, connectors, plugin sources, gVisor/Kata, `helm upgrade`.

## Step 7: security coverage (`docs/tests/security-coverage.md`)

All 39 rows (SEC-01..SEC-39) were checked mechanically: every cited test function name was matched to a
`test <name> ... ok` line in the logs of steps 2 and 5, and every cited script was matched to a passing
run. Result: **every cited test ran and passed; no row has an unexecuted test.** Details:

- Gated e2e rows ran with their gates on: `e2e-h/hosted.rs::s8_security_probes`, `s8b_...` (hosted.sh),
  `e2e-h/k8s.rs::k1..k4` (kind.sh, SEC-02/09/26), `e2e-h/channels.rs::s1..s6` (channels.sh and hosted.sh),
  `e2e-g/gateway.rs::s10_...` (gateway.sh; it failed only in the un-harnessed step-2 run).
- Script rows: `edge-routes.sh` (SEC-09), `dep-graph.sh` (SEC-13), `no-integration-names.sh` (SEC-33): all pass in `run-all.sh`.
- UI rows (SEC-18): `web/tests/approvals.test.tsx`, `apps.test.tsx` are inside the 151 vitest tests.
- The extraction flagged tokens that are not tests, so they are not counted as missing: error codes or result
  strings inside tests (`approval_store_unavailable` in `gw/apps.rs::tool_grants_and_graphql`, `skipped_unauthorized`
  and `authorization` in `api/schedules.rs`/`webhooks.rs`), the prefix `sup`, the word `attachment`, and the
  scenario ids `s1`/`s5` of `e2e-h/channels.rs` (they ran).
- Limits of this check: matching is by function name (the table gives no module paths), and rows that cite
  a conformance scenario run through `scripts/conformance.sh`, which reports aggregate counts only.

## Fixes made and doc corrections

| File | Change | Why |
|------|--------|-----|
| `crates/aurora-core/src/service.rs` | `fmt::layer().fmt_fields(JsonFields::new())` | `h1_held_request_gets_hook_summary_without_egress` failed deterministically: the hook host's `http.request` span stored fields as `method=GET` text, and the JSON event formatter parses span fields back as JSON and panics in debug builds (`span 'http.request' had malformed fields!`). The panic killed the `approval.summarize` request, so the gateway fell back to `template`. In release builds the same text is emitted as a `field_error` instead of panicking, so logs were silently degraded there. Committed separately (`e7ff523f`). |
| `deploy/compose/compose.yaml` | `ip_range: 10.89.249.128/25` on `core` | Fresh `up -d` fails on the static-IP race (above). |
| `deploy/compose/setup.sh` | removed "(available once BE-016 lands)" | `bootstrap-admin` exists and works. |
| `docs/operator/install-compose.md` | first-admin flow, `Origin` header, fake-backend limit, `ip_range`, collision advice | match what happened |
| `docs/operator/runners.md` | podman backend does not work from the compose runner container | observed failure |

## Open findings

1. **Compose podman backend does not start** (rootless socket not reachable from the non-root runner
   container). Compose can only demonstrate the fake backend; no sandbox answers a chat. Needs a decision
   (run the runner on the host in the doc, or make the socket reachable).
2. `scripts/e2e/hosted.sh` also runs the channel tests (duplicates `channels.sh`, +11 min).
3. `cargo deny` warning: an allowed license in `deny.toml` is unmatched (not a failure).
   `pnpm lint`: 1 warning (`window.location.assign` to an internal page in `delete-view.tsx`).
4. The `core` network fix and the logging fix are outside the AUDIT-001 file ownership, so they need the
   coordinator's review before merging into `phase0`.

## Leftovers on the host (nothing of other lanes was removed)

Created by this audit and left in place: podman volumes `aurora-ca` (fixed name) and `aurora-audit_aurora-pg`,
images `aurora-*:audit001`, rebuilt `localhost/aurora-e2e/*:e2e` and `localhost/aurora-sandbox:0.1.0` /
`localhost/aurora-llm-sandbox:0.1.0`, and the git-ignored `deploy/compose/.env`. The kind cluster `aurora-audit`
and the `aurora-audit` compose containers and network were removed; the e2e scripts clean up their own resources.

## Follow-up OPS-015

Branch `alexiri/ops-015`, 2026-11-07. Closes open findings 1-3 (finding 4 was reviewed when AUDIT-001 merged).

### Finding 1: compose `hosted` profile answers from a podman sandbox

Fix in `deploy/compose/`: `runner` runs with `userns_mode: keep-id` and `user: ${AURORA_UID}:${AURORA_GID}` (written by
`setup.sh`), so it is the owner of the rootless socket; the profile selects the podman backend, `runner.image`
(`AURORA_SANDBOX_IMAGE`) and the sandbox network; `setup.sh` warns when `podman.socket` is inactive. The clean run found
three further defects that each stopped the answer, now fixed:

| Symptom | Cause | Fix |
|---------|-------|-----|
| runner poll: `503 gateway_ca_unavailable` | the `api` service had no `ca.pem` (`gateway.ca_dir` is not mounted into it) | `setup.sh` exports the public `ca.pem` to `deploy/compose/ca/` (git-ignored); `api` mounts it read-only. `ca.key` stays gateway-only |
| turn fails `harness_failed: API Error: 502` | sandbox proxy dialed `localhost:4102` (default `advertised_proxy_url` derived from `external_url`) | `gateway.advertised_proxy_url = "http://aurora-gateway:4102"` in `aurora.toml` (`sandbox_hosts` is set there too) |
| (same, next layer) | gateway destination guard refuses private IPs, LiteLLM is on the core network | static `10.89.249.4` for `litellm`, `gateway.allowed_private = ["10.89.249.4/32"]` (only that address, not the core network) |

Transcript (project `aurora-o15`, `COMPOSE_PROFILES=hosted,llm-mock`, `AURORA_VERSION=o15`, `AURORA_SANDBOX_IMAGE=localhost/aurora-llm-sandbox:0.1.0`;
`.env` regenerated by `setup.sh`, `podman compose up -d --build`-equivalent via `build` then `up -d`, Postgres volume new):

```
GET /v1/instance -> 200
POST /v1/auth/password/reset -> 204        (token from `run --rm api bootstrap-admin --email o15@example.org`)
POST /v1/auth/login -> 200
POST /v1/orgs -> org o15, workspace 01a1b56b-5512-..., agent daneel 01a1b56b-5513-...
PATCH agent (claude-code, litellm-mock, scn-pong) -> 200
PUT grant llm secret -> 200
POST message -> 202                          "Reply with exactly the word: pong"
1 turn.queued
2 turn.started
3 text {"message_id": "msg_scn_pong", "text": "pong"}
4 turn.finished {"status": "done", ...}
podman ps: aurora-sbx-48d540d7-... localhost/aurora-llm-sandbox:0.1.0  Up
```

Run-to-run note: a first attempt on the same stack was wedged by me deleting the live sandbox container while its
turn was running (the turn stayed `queued`); the transcript above is a second run from a fully clean state
(project torn down, Postgres volume removed, `.env` regenerated) with the final files.

Left on the host on purpose: the pre-existing fixed-name volume `aurora-ca` (shared by every lane; reused, a CA
minted earlier) and the shared `aurora-sandbox` network. Removed: all `aurora-o15` containers, network and Postgres volume, the sandbox
container and its `home-*` volume, images `aurora-*:o15`, `deploy/compose/.env` and `ca/`. Podman build cache for the images remains.

### Finding 2: `scripts/e2e/hosted.sh`

Runs only `--test hosted`, plus `--test k8s` when `AURORA_TEST_KIND=1` and `--test llm` when `AURORA_TEST_LLM=1`. Channel tests
stay in `channels.sh`.

### Finding 3: warnings

`deny.toml`: removed the unmatched `MPL-2.0` allowance. `delete-view.tsx`: `router.push("/auth/login")` instead of `window.location.assign`.

### Verification (OPS-015)

| Command | Result |
|---------|--------|
| `AURORA_TEST_PODMAN=1 bash scripts/e2e/hosted.sh` | exit 0; only `tests/hosted.rs` ran: 9 passed, 0 failed (709 s test time); no channels, k8s or llm target |
| `cargo deny check` | advisories, bans, licenses, sources ok; no warnings |
| `pnpm -C web lint` | 0 errors, 0 warnings |
| `pnpm -C web typecheck` / `test` / `build` | clean / 19 files, 151 tests passed / ok |
| `bash scripts/guards/run-all.sh` | all guards passed |
