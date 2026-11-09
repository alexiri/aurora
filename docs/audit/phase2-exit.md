# Phase 1 + 2 exit verification (AUDIT-002)

Evidence for the phase-1 and phase-2 exit (`docs/PROGRESS.md` §Phase exit criteria → "Phase 2 (with phase 1;
AUDIT-002)", twelve criteria). Everything was run on **one commit**, from a fresh worktree `target/`, on
2026-11-09. Raw logs were kept outside the repo in `/tmp/audit-002-logs/` (cited per row below; they are not
part of the commit). This task verified; it did not develop.

**Audited commit:** `cc6ccbb345cf03ad2b881d9585e8a19329ceac9a` (branch `alexiri/audit-002`, master at the time P2-UI-5
had merged and been recorded). No other commit was merged or pulled into the worktree.

**Verdict: every verification command that could run here is green; the exit is NOT fully evidenced.** 0 test
failures anywhere. Three groups of items stop a clean "exit met" and need an owner or coordinator decision (details in
[Open findings](#open-findings) and [Deviations](#deviations)):

1. Exit criterion 10 (GitHub CI + nightly) was replaced by local runs (owner-accepted); the project-key signing
   path and the real-cluster Helm install (criteria 7 and 9) cannot run here and were **not run**.
2. One real defect carried from FUP-M7 is still open and now proven by a probe: a granted-but-**disabled**
   connection still contributes app env and stub files to a sandbox boot (F-1; non-secret).
3. Three documentation gaps (F-2 `/healthz` in the install checklist, F-3 default image registry not pullable,
   F-7 operator docs thin on phase-1/2 settings) and two stale tracking tables (F-4 security-coverage test
   names, F-5 FEATURES.md `P2/14§visual-s4`). (There is no F-6; the number was dropped during drafting.)

No fixes were made (the worktree has no source change; see [Fixes](#fixes-made)).

## Environment

| Item | Value |
|------|-------|
| Host | Arch Linux, kernel 6.6.31-lts, 16 CPUs, 31 GB RAM, LUKS ext. fs; `/home` 92 GB free at start, 78 GB lowest (limit 50 GB) |
| podman | rootless; kind with the podman provider (`KIND_EXPERIMENTAL_PROVIDER=podman`), kind node image `kindest/node:v1.37.0` |
| Toolchain | rustc 1.99.1 (workspace edition 2024), node v20.20.2, pnpm 10.34.9, helm, kubeconform, kubectl (versions not recorded) |
| Postgres | shared dev Postgres 127.0.0.1:55432 (`aurora-dev-pg`, owner's); hosted/upgrade/kind use their own |
| Owner objects left alone | cluster `aurora-dev`, containers `aurora-dev-pg`, `aurora-dev-control-plane`, volumes `aurora-kind-dev-*`, all pre-existing images |
| My names | clusters `aurora-audit`, `aurora-audit-fr`; hosted prefix `audit-002-e2eh`; kind-dev volumes `audit-002-kind-dev-*`; scripts' own `aurora-e2e`, `aurora-upg-*`, `aurora-mm2-*` |
| Audited commit | `cc6ccbb345cf03ad2b881d9585e8a19329ceac9a` |

## Results

Durations are wall-clock. "Log" is the file under `/tmp/audit-002-logs/`.

| # | Command | Exit | Result | Duration | Log |
|---|---------|------|--------|----------|-----|
| 1 | `cargo build --workspace --all-targets` (fresh `target/`) | 0 | Finished | 7m04s | `1-build.log` |
| 1 | `cargo fmt --all --check` | 0 | clean | 1s | `1-fmt.log` |
| 1 | `cargo clippy --workspace --all-targets -- -D warnings` | 0 | clean | 2m27s | `1-clippy.log` |
| 1 | `cargo clippy --workspace --all-targets --all-features -- -D warnings` (the `ci.yml` form) | 0 | clean (reused the build above) | <1s | `1-clippy-allfeat.log` |
| 1 | `cargo deny check` | 0 | advisories ok, bans ok, licenses ok, sources ok (no warnings) | 2s | `1-deny.log` |
| 1 | `bash scripts/plugins/deny-hooks.sh` | 0 | 3 hook crates clean | 3s | `1-denyhooks.log` |
| 1 | `bash scripts/guards/run-all.sh` | 0 | all guards passed | 2s | `1-guards.log` |
| 2 | `cargo test --workspace --no-fail-fast --exclude aurora-e2e-gateway --exclude aurora-e2e-hosted -- --test-threads=8` | 0 | **1292 passed, 0 failed, 3 ignored** (per crate below) | 19m13s | `2-test.log`, `2-percrate.txt` |
| 3 | `pnpm -C web install --frozen-lockfile` | 0 | lockfile up to date | 1s | `3-install.log` |
| 3 | `pnpm -C web typecheck` | 0 | clean | 8s | `3-typecheck.log` |
| 3 | `pnpm -C web lint` | 0 | 0 errors, 0 warnings | 18s | `3-lint.log` |
| 3 | `pnpm -C web test` | 0 | 35 files, **296 passed** | 18s | `3-test.log` |
| 3 | `pnpm -C web build` | 0 | Next.js production build ok | 23s | `3-build.log` |
| 3 | `pnpm -C web gen:api && git diff --exit-code web/src/lib/api/schema.d.ts` (`ci.yml` web job) | 0 | no client drift | 1s | `3-genapi.log` |
| 3 | `playwright test stage4-routes stage4-axe shell-theme stage2-narrow smoke chat-v2` | 0 | **257 passed** (incl. axe in both themes on every route, 390 px narrow shell, chat v2) | 4m40s | `3-pw.log` |
| 3 | `pnpm start` + `curl -sI /` (production CSP) | 0 | header recorded below; `web/src/proxy.ts` unchanged since FE-010 | 8s | `3-csp-head.txt` |
| 4 | `cargo test -p aurora-api --test it openapi_matches_checked_in` (`UPDATE_OPENAPI` unset) | 0 | 1 passed (also passed in step 2) | 2m13s incl. link | `4-openapi.log` |
| 4 | `python3 scripts/extract-schema.py --check` | 0 | `spec_schema.json is up to date` | <1s | `4-schema.log` |
| 4 | `cargo test -p aurora-api --test it permission_matrix -- --nocapture` | 0 | 3 passed; 241 rows checked; `INCONCLUSIVE rows: 0 {}` | 2m13s incl. link | `4-matrix.log` |
| 4 | C-number sweep (scratch script) | n/a | 126 phase-1/2 contract items; see step 4 notes | n/a | `4-cnums.txt`, `4-cn-sample.txt` |
| 5 | `bash scripts/fixtures/build.sh` | 0 | passed=38 failed=0 xfail=0 | 1m56s | `5-fixtures.log` |
| 5 | `bash scripts/plugins/check-builtin.sh` | 0 | 13 built-ins + `hook-example` ok; hook-crate tests 13+7+13 passed | 2m20s | `5-check-builtin.log` |
| 5 | `bash scripts/plugins/build-builtin.sh --build-only --out <scratch>` | 0 | 13 packages built, unsigned (expected) | <1s | `5-build-builtin.log` |
| 5 | `bash scripts/plugins/check-links.sh` | 0 | 81 links ok, 0 not ok (needs network; it had it) | 32s | `5-links.log` |
| 6 | `bash scripts/e2e/m1.sh` | 0 | 1 passed | 1m49s | `6-m1.log` |
| 6 | `bash scripts/e2e/gateway.sh` | 0 | `gateway` 16, `hooks` 4, `database` 1 (`d1`), all passed; includes `s18` (instances) | 4m20s | `6-gateway.log` |
| 6 | `AURORA_TEST_PODMAN=1 AURORA_E2EH_PREFIX=audit-002-e2eh bash scripts/e2e/hosted.sh` | 0 | 14 passed (s1..s11 incl. `s10` freeze, `s11` psql database lane, `s8` SEC probes all `ok`); `s8d` chromium **skipped by its gate** | 19m15s | `6-hosted.log` |
| 6 | same with `AURORA_E2EH_FULL_IMAGE=1` | 0 | 14 passed incl. `s8d_chromium_wrapper_on_the_full_image_fetches_through_the_open_proxy` | 17m57s | `6-hosted-full.log` |
| 6 | `AURORA_TEST_PODMAN=1 bash scripts/e2e/channels.sh` | 0 | 6 passed (s1..s6) | 11m26s | `6-channels.log` |
| 6 | `bash scripts/conformance.sh target/debug/aurora-connector-fake` (binary mode) and `bash scripts/conformance.sh` (in-process) | 0 / 0 | 13 passed each | 23s / 2s | `6-conf-fake.log`, `6-conf-fake2.log` |
| 6 | `bash scripts/conformance.sh --mattermost` | 0 | 13 passed against the pinned Mattermost container; container removed | 1m40s | `6-conf-mm.log` |
| 6 | `AURORA_TEST_PODMAN=1 bash scripts/e2e/mattermost.sh` | 0 | 5 passed (live connector + real Mattermost + core) | 8m34s | `6-mattermost.log` |
| 6 | `AURORA_TEST_PODMAN=1 AURORA_TEST_LLM=1 bash scripts/e2e/llm.sh` | 0 | scenario smoke ok; supervisor conformance 2 passed (live claude-code case included); hosted `l1`, `l2` passed | 3m14s | `6-llm.log` |
| 6 | `UPGRADE_STRICT=1 UPGRADE_BASE=baabe11f bash scripts/e2e/upgrade.sh` | 0 | **36 passed, 0 failed, 0 defects**; 19 baseline migrations then 33 at HEAD all `success` | 5m07s | `6-upgrade.log` |
| 6 | `bash scripts/perf/gateway.sh` | 0 | all PASS: p50 0.029 ms (<1), p99 0.096 ms (<5), 17295 req/s (>=2000), resolve p99 2.144 ms (<25) | 7m17s | `6-perf.log` |
| 6 | `bash scripts/e2e/kind.sh --fresh` | 0 | NetworkPolicy enforced; `k1`..`k4` 4 passed | 14m05s | `6-kind-fresh.log` |
| 6 | `bash scripts/e2e/kind.sh --release-images` | 0 | release-profile images; `k1`..`k4` 4 passed | 23m04s | `6-kind-rel.log` |
| 6 | `AURORA_DEV_CLUSTER=aurora-audit-fr AURORA_DEV_PORT=4051 bash scripts/e2e/first-run.sh --up`, **run 1** (fresh cluster) | 0 | 1 passed (journey 26 s) | 5m33s | `6-firstrun1.log` |
| 6 | same, **run 2** (cluster deleted between runs) | 0 | 1 passed (journey 26 s) | 5m30s | `6-firstrun2.log` |
| 7 | Helm install from `docs/operator/install-helm.md` on `aurora-audit` | 0 | `helm lint` 0 failed, `helm template` 1549 lines, kubeconform 43/43 valid; install, `helm upgrade --set web.replicas=2` and `helm uninstall` all 0 | install ~1 min | `7-*.log`, `7-pods.txt` |
| 11 | targeted probe for FUP-M7 (temporary test, reverted) | 0 | **reproduces the defect** (F-1) | 33s | `11-fupm7.log` |

Production CSP (`curl -sI http://localhost:4100/` after `pnpm start`, nonce varies per request):
`default-src 'self'; img-src 'self' data:; script-src 'self' 'nonce-…'; style-src 'self' 'nonce-…'; connect-src 'self'; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'`.
"Unchanged vs master": the audited commit is master, so the comparison is against history: `web/src/proxy.ts`
(the only place the header is set) has not changed since its creation in FE-010 (`git log -- web/src/proxy.ts`
lists only `cdcc0dba`), and `web/next.config.ts` has no diff against the phase-0 freeze commit `2a6a0f8c`.

### Step 2: per-crate totals (passed / failed / ignored)

aurora (cli) 24, aurora-api 540, aurora-channels 13, aurora-client 6, aurora-config 39, aurora-conformance-connector 14,
aurora-connector-fake 6, aurora-connector-mattermost 35, aurora-connector-sdk 5, aurora-core 41, aurora-crypto 18/0/1,
aurora-db 19, aurora-dev-idp 4, aurora-gateway 190, aurora-hook-wit 1, aurora-hookhost 16, aurora-llm 43,
aurora-plugin-sdk 4 (+1 doctest passed, 1 ignored), aurora-policy 12, aurora-protocol 31, aurora-registry 39,
aurora-runner 63, aurora-secrets 12, aurora-sqlclass 5, aurora-summary 12, aurora-supervisor 99; aurora-config doctest 0/0/1.
Total **1292 passed, 0 failed, 3 ignored** (`print_fixture` in aurora-crypto, two doctests marked ignore). Phase 0 was 893.
The two e2e crates are excluded here and ran in step 6 (`aurora-e2e-gateway` 21 tests under `gateway.sh`,
`aurora-e2e-hosted` 14 + `channels` 6 + `mattermost` 5 + `llm` 2 + `k8s` 4 under their scripts).

## Per-step notes

**Step 1, clean build.** `target/` did not exist before the build. No warnings from `cargo deny` (phase 0 had one).

**Step 2.** `--test-threads=8` was used as instructed and the shared Postgres never refused a connection.

**Step 3.** Playwright 257 passed with `pnpm dev` as its own web server on port 4100 (the owner's kind-dev stack
on :4000 was not touched). Lint is now warning-free (phase 0 had one warning).

**Step 4.** OpenAPI drift passed as is. Permission matrix: 241 route rows checked, 0 inconclusive.
C-number sweep: 126 phase-1/2 contract items in `docs/API_CONTRACTS.md` (the M1 phase-2 block lines 88–98 and
everything from "Phase 1 — RFE-009" down). Method: a script took each item's `METHOD /v1/...` routes and looked
each up in `web/openapi.json` (64 items carry routes; all present except `POST /v1/runner/register` (C-85, C-190: a
runner-plane route, not in the public OpenAPI by design) and `POST /v1/auth/signup` (C-150: a **removed** route that must
404, asserted by `auth.rs`, line 124)). The 62 items without a route were sampled by identifier, one grep each over code, scripts, charts
and web (`4-cn-sample.txt`): C-18, 19, 46, 48..51, 63, 64, 82, 84..86, 95, 123, 126, 135, 138, 139, 150, 154, 175, 176,
182..184, 191, 193, 197, 198, 204, 207, 208, 210, 220..222, 231, 232, 240, 246, 249, 260, 264, 265, 267, 268, 291, 292, 300:
every identifier has an implementation in at least one non-doc file; none was missing (C-150's removed routes are asserted
by `auth.rs`). **Not individually grepped** (14 route-less items): C-83, 185, 192, 194, 209, 223..225, 233, 244, 248, 293,
301, 302.

**Step 5.** `check-builtin.sh` covers all 13 built-ins (atlassian, aws-keys, aws-role, github, github-app, gitlab,
google-workspace, indico, linear, microsoft-365, notion, postgresql, sentry) plus `hook-example`, including the
hook-point coverage and token-lifecycle checks in `check_builtin.py`. Signing: `build-builtin.sh --sign-only` with the
real `AURORA_PROJECT_SIGNING_KEY` was **not exercised** (only CI's `release` environment holds it); the `--build-only`
run correctly produced unsigned archives. The "first signed CI release run" of criterion 7 therefore remains unconfirmed.

**Step 6.**
- `hosted.sh` now runs only the `hosted` test target (14 tests); the channels tests run in `channels.sh`. The `s8` probes
  printed `ok` for SEC-01, 02, 09, 24, 25, 26, 34 and 41. `AURORA_E2EH_PREFIX=audit-002-e2eh` named every object
  (`audit-002-e2eh-sandbox`, `-sandbox-full`, networks, containers). The first run's sandbox image was a **100 % podman
  layer-cache hit** (image created 2026-11-08 22:37 UTC, same inputs), so it is not a from-scratch rebuild (see Deviations).
- `upgrade.sh`: "the phase-0 release point" was taken as `baabe11f` (merge of AUDIT-001, the code behind
  `docs/audit/phase0-exit.md`; the 38 later phase-0 commits up to `2c622783` touch no crate or service). The script's
  default base `8ba42973` was not used. Baseline had 19 migrations, HEAD 33.
- `kind.sh --release-images` built the service images with `images/services/Containerfile` (release profile, what CI
  publishes); those images (`localhost/aurora-e2e/*:e2e`) were then reused in step 7.
- First-run: both runs on a fresh kind cluster (created by `kind-dev.sh up --llm-mock`, then deleted) passed in 26 s
  after a 5.5 min warm bring-up. The nightly's "second user on the same stack" variant was not run (the brief asked for two
  fresh clusters).

**Step 7, install from scratch (`docs/operator/install-helm.md`, cluster `aurora-audit`).** Followed in order:
prerequisites (kind cluster with an ingress port mapping; ingress-nginx from `deploy/kind/ingress-nginx.yaml`), images,
Secrets option A (chart-generated), bundled evaluation Postgres, `helm lint`, `helm template`, `helm install --wait`,
post-install checks, `helm upgrade`, `helm uninstall`. Deviations from the doc, all recorded:
- **The default images cannot be pulled** (F-3): `ghcr.io/alexiri/aurora-api:0.1.0` and `:master` answer
  `unauthorized`, so I used the release-profile images from step 6 with `image.registry=localhost/aurora-e2e`,
  `image.tag=e2e`, `pullPolicy=Never`, loaded with `kind load image-archive` (the doc's "images you built and loaded yourself").
- The doc has no recipe for an ingress controller on kind; I used the repo's `deploy/kind/ingress-nginx.yaml` and a kind
  config with `extraPortMappings` (host 4052).
- Used `postgres.enabled: true` and `devIdp.enabled: true` (kind, evaluation) instead of the doc's external PostgreSQL and
  a real OIDC provider. `values` file kept outside the repo (`my-values.yaml`).
- Result: the three hook Jobs and all Deployments came up; `GET /v1/instance` through the ingress answered with the dev
  provider listed; `/internal/v1/status` and `/metrics` answer 404 at the edge (never routed, as documented);
  `bootstrap-admin --email … ` through `kubectl exec … /usr/local/bin/aurora-api` printed the sign-in URL as the doc says;
  the runner registered (two retries while the API started). `helm upgrade --set web.replicas=2` gave revision 2 with 2/2 web pods; `helm uninstall`
  left exactly what the doc lists (Postgres StatefulSet/Service/PVC, the generated Secrets).
- **`/healthz` through the edge returns 404**, while checklist step 5 says it returns 200 (F-2).
- **Not run:** a chat turn on this particular install (the same chart and the same release images pass `k1`..`k4` in step 6;
  the first-run journey passed on the dev profile), sign-in through the dev IdP by a browser, and the **real-cluster
  checklist** at the end of the doc (no real cluster available): **not run**, so FU-P1-005a stays open.

**Step 8, security coverage (details in the section below).** All 60 rows are `proven` (51) or `added` (9).

**Step 9, spec sweep.** `grep -rn "(phase 1\|(phase 2" docs/*.md` excluding 80, PROGRESS and FEATURES (and 90, the decision
log) gives 375 markers in 19 documents (05: 56, 03: 43, 15: 36, 04: 34, 17: 30, 18: 21, 14: 21, 02: 21, 08: 16, 07: 16,
06: 14, 10: 11, then 13, 16, 12, 11, 09, 01, 00, 20, 19). Sampling: every 11th marker in file order (about 34 markers
across all documents), plus every marker's contract/test through steps 2, 4 and 8, which already cover the dense sections
(05, 03, 17). Each sampled rule was looked up in code, charts, scripts or web: user links `verified_email_autolink`; `GET /v1/orgs/{org}/storage`;
`byo_not_permitted`; approvals marked `abandoned` on freeze; `FREEZE_REPLY_INTERVAL`; `API_RATE_UNAUTH`; `retention`
settings; boot `capabilities` / `browser`; `dev_idp`; per-principal rate limit; "Freeze organization" in org settings;
"Approval timeout" in the rule editor; BYO `minimum_posture`; the `database` manifest section; the offboarding view;
`quota_exceeded`; `new_creator_unauthorized`; `session.purge`; `db_open` (database lane); `automation.reassign`. All were
found. Two first greps returned 0 because of my regex (not the code): they were re-checked by hand and exist
(`approvals.rs:336` sets `abandoned`; `OrgFreeze` at `org/settings-view.tsx:288`). **Not read exhaustively**: the other
~340 markers. `docs/03 §Planned changes` holds only the two `1 → dropped by CONTRACT-001` rows (`users.password_hash`,
`email_tokens`: the owner-scheduled drop) plus phase 3/4 rows. No phase-1/2 row is left.

**Step 10, FEATURES.** Phase-1/2 rows: all `done` with evidence except `P2/14§visual-s4`, which is still `planned` although
P2-UI-4 merged (`23c446bf`) and its suites pass here (F-5). No phase-1/2 row is `deferred`. Rows that are `done` with a
stated exception (each names its follow-up; none is a silent gap): RFE-004 (FU-P1-005a, the real-cluster install, still open),
`P1/19§tooling-kind` (FU-OPS-017a/b open), `P1/19§tooling-reload` (FU-OPS-018a/b deferred), RFE-005 (FU-P2-005a deferred),
RFE-010 (FU-P2-010a, rechecked here), RFE-011a§timeouts (per-grant `ask_timeout` field missing in the agent Access UI,
FU-P2-011A-b), RFE-011a§email (action approvals e-mailed but not pushed on SSE, FU-P2-011A-a), RFE-025 (FU-P2-025c, rechecked here),
`P2/09§3.4-instances` (later-activated static-claim conflict decision not built, FU-P2-INST-a), `P2/21§catalogue` (FU-P2-CAT-0c
deferred; first signed CI release run unverified), `P2/19§pkg-m365` (tenant fixed to `common`), `P2/19§pkg-aws-keys`
(FU-P2-CAT-5a deferred), `P2/owner§pkg-indico` (dev-stack registration check pending), `P2/21§db-wire` (direct-TLS
`sslnegotiation=direct` unsupported), `P2/14§chat-v2` (two owner questions open). The `SEC-01..39` row reads "39/39 proven" (stale beside the 60-row table, F-5).

**Step 11, rechecks owed.**
- FU-P2-010a (`channels.sh` and `m1.sh` on P2-010): **both passed** here (step 6).
- FU-P2-025c (hosted e2e on P2-025): **hosted.sh passed twice**, 14 tests, incl. the BYO/posture paths covered by the
  service suite and `gateway.sh` `s17` (step 6).
- FUP-M7 "granted-but-disabled connection still contributes env/stubs": verified with a **targeted temporary test** (not
  the hosted run, which has no such scenario). The probe granted `app_a`, set the connection `status = 'disabled'`, started
  the agent and printed `env=Some("https://a.example/api") files=1`: **the defect is present** (F-1). The temporary test was
  reverted (`git checkout`), the repository holds no trace of it.

## Security-row execution check (step 8)

Method (scratch script outside the repo, `sec-check.py`): for each of the 60 rows of `docs/tests/security-coverage.md`,
every cited test name (the last `::` segment of each backticked path, with `{a, b}` groups expanded) was searched in
the `test … ok` lines of all step 2–6 logs. Raw output: `8-sec-raw.txt`. Gated tests count as executed only where their
gate was set in this audit: `AURORA_TEST_PODMAN=1` (hosted/channels/mattermost/llm), `AURORA_E2EH_FULL_IMAGE=1`
(`s8d`), kind (`k1`..`k4`), `AURORA_TEST_LLM=1`.

- **58 of 60 rows: every cited test name was found in a passing run.** SEC-40..SEC-52, SEC-59, SEC-60 and the amended SEC-04,
  SEC-28, SEC-32 all executed (counts of cited names found: SEC-40 27, 41 24, 42 18, 43 15, 44 9, 45 12, 46 10, 47 16, 48 15,
  49 22, 50 21, 51 8, 59 16, 60 24, 04 5, 32 12). Status in the table: SEC-04 and SEC-59 read `added` rather than `proven` (the
  tests exist and passed; criterion 3 says `proven`, a wording fix, F-4).
- **SEC-33** cites a guard script (`no-integration-names.sh`), executed by `run-all.sh` in step 1: passed.
- **Cited names that do not exist (stale table text, F-4):** SEC-22 cites
  `auth.rs::open_signup_creates_org_and_cookie` and `auth.rs::sec22_session_cookie_is_secure_on_https`; the test is now
  `auth::sec22_session_cookie_flags` (passed); `foundation::csrf` also passed. SEC-28 cites
  `channels_inbox.rs::member_approve_code_is_refused` and `invitations.rs::approver_bindings_via_invitation_and_role_change_applies_next_request`;
  the nearest existing tests are `channels_inbox::commands_approve_and_deny_apply_the_access_law` and
  `invitations::create_list_cancel_permissions_and_flow` (both passed), but I did not read them to confirm they assert what
  the two stale names claimed. The other 9 SEC-28 citations all passed. These two rows are the ones to re-check by hand.
- **Not executed here:** the one `s8d` chromium test was *skipped* in the plain `hosted.sh` run and executed in the
  `AURORA_E2EH_FULL_IMAGE=1` run (passed). No cited test was skipped without a second run.
- **Limits of the check:** it matches names, not assertions; file-level citations (e.g. `permission_matrix.rs` as a whole) are
  covered by the matrix run in step 4 but not name-checked; a name that exists in two crates would match either.

## Deviations

1. **No GitHub CI (owner-accepted 2026-11-08, billing).** Criterion 10 is replaced by local runs of every `ci.yml` and
   `nightly.yml` command: `ci.yml` rust-lint (fmt, clippy with and without `--all-features`), rust-deny
   (`cargo deny`, `deny-hooks.sh`), rust-build (`fixtures/build.sh`, `check-builtin.sh`, doctests inside `cargo test`),
   e2e-gateway (`gateway.sh`), e2e-hosted (`hosted.sh`), web (install, lint, typecheck, test, build, `gen:api` drift),
   guards (`run-all.sh`, `extract-schema.py --check`); `nightly.yml` first-run (twice, fresh clusters), kind-e2e
   (`kind.sh --release-images`) and mattermost-conformance; `plugin-links.yml` (`check-links.sh`).
2. **What the local runs do not reproduce:** the nextest sharded runner and its `ci` profile (`cargo test` ran instead);
   `cargo test --all-features` (only clippy used `--all-features`); the `images-*` publish jobs, the pushed images and their
   SBOM/provenance (only the release-profile service images were built, via `kind.sh --release-images`; the web and
   sandbox publish jobs were not); the signed built-ins (project key only on CI); arm64.
3. **Layer cache:** `hosted.sh`'s sandbox image was a full podman cache hit, not a from-scratch build (identical inputs; no
   `--no-cache`).
4. **`upgrade.sh` base** is `baabe11f` (the phase-0 exit code), not the script default `8ba42973`.
5. **First-run:** two fresh-cluster runs as instructed; the nightly's second-user-on-same-stack variant was not run.
6. **Install doc:** local images instead of the unpullable ghcr defaults; kind ingress recipe taken from the repo (Step 7).
7. **Criterion 11 (operator docs)** is not one of the eleven steps; only a spot check was made (F-7).

## Fixes made

None. The worktree contains no change except `docs/audit/phase2-exit.md`. (A temporary probe test was added to
`services/aurora-api/tests/it/sandbox_env.rs` for FUP-M7 and reverted before the commit.)

## Open findings

Each is a candidate fix task. None blocks anything that ran green; F-1 is the only behavioural defect.

| Id | Severity | Finding | Evidence | Suggested fix |
|----|----------|---------|----------|---------------|
| F-1 | low (non-secret leak of app env names/stubs; behavioural) | A connection with `status = 'disabled'` that an allow rule selects still puts its app into the boot env and stub files (FUP-M7 item, now reproduced). `compute_inner` in `services/aurora-api/src/domains/grants/effective.rs` (~line 140) sets `selected` without looking at `c.status`, `gateway::app_connections` (`crates/aurora-db/src/repo/gateway.rs:613`) deliberately returns `disabled` rows, and `boot.rs::app_boot_for` trusts `granted_apps` | `/tmp/audit-002-logs/11-fupm7.log` (`env=Some("https://a.example/api") files=1`) | Exclude non-`active` connections from `granted_apps` (keep them in the effective listing as unselected or marked), add the test next to `an_unavailable_app_contributes_no_env_or_stubs_and_flags_restart`, decide whether the disable should also flag `restart_to_update` |
| F-2 | low (docs) | `docs/operator/install-helm.md` real-cluster checklist step 5 says `curl -fsS https://<host>/healthz` returns 200. Through the chart's ingress (`/v1` → API, `/` → web) it answers 404 (`/healthz` exists on the services, but is not routed, by design) | observed on the step-7 install (terminal output; not saved to a file) | Replace with `GET /v1/instance` (answers 200 with the provider list) |
| F-3 | medium (docs/availability; follows from no CI) | The chart's default `image.registry` `ghcr.io/alexiri`, tags `0.1.0` and `master`, are not pullable (`unauthorized`), although the doc calls them "public" and the install section's minimal values assume a registry. A fresh reader following the doc literally cannot install without building and loading images | `podman manifest inspect ghcr.io/alexiri/aurora-api:0.1.0` and `:master` (terminal output; not saved to a file) | Either publish images once billing is fixed, or add a "build and load" path (as `kind.sh` does) to the install doc and state that the ghcr defaults exist only after the first CI publish |
| F-4 | low (tracking table) | `docs/tests/security-coverage.md`: SEC-22 cites two test names that no longer exist and SEC-28 cites two; SEC-04 and SEC-59 read `added` where the exit criterion wants `proven` | `/tmp/audit-002-logs/8-sec-raw.txt` | Correct the names (`sec22_session_cookie_flags`; confirm the two SEC-28 tests by reading `channels_inbox::commands_approve_and_deny_apply_the_access_law` and `invitations::create_list_cancel_permissions_and_flow`) and flip the two statuses |
| F-5 | low (tracking table) | `docs/FEATURES.md`: row `P2/14§visual-s4` is `planned` with no evidence although P2-UI-4 merged (`23c446bf`) and its suites pass; the `SEC-01..39` row says "39/39 proven" next to a 60-row table; the header still says phase 2 is in progress | `docs/FEATURES.md` line 127 and the SEC row; `3-pw.log` | Mark `done` with the Playwright counts; restate the SEC row as 60/60 |
| F-7 | medium (criterion 11, not fully verified) | `docs/operator/*` has no page or section naming the open-lane posture, API rate limits, approval timeouts and e-mail (`approvals.max_timeout_secs`, `email_notifications`), retention and attachment quota settings, the database lane or the app-instance admin flow; they are covered only in the design document `docs/15` (which `docs/operator/README.md` names the source of truth). Retention appears only for `request_log_days` in `backups.md` | `grep -rli` over `docs/operator` (not saved; method in the report text) | The coordinator decides whether docs/15 satisfies "operator docs cover the phase-1/2 configuration"; if not, a docs task writes an operator page per topic |

## Real-cluster install and signing

- Real-cluster Helm install checklist (end of `install-helm.md`, FU-P1-005a): **not run** (no real cluster). Exit criterion 9
  is therefore only half evidenced (kind: yes).
- Project-key signing (`build-builtin.sh --sign-only`): **not exercised** with the real key (CI-only secret). Criterion 7's
  "first signed run confirmed" is open.

## Leftovers

- Deleted: kind clusters `aurora-audit`, `aurora-audit-fr`, `aurora-e2e` (the script's own); podman volumes
  `audit-002-kind-dev-{target,rustup,cargo-registry}`; images `localhost/audit-002-e2eh-sandbox:0.1.0` and
  `-sandbox-full:0.1.0`; the scripts' own containers and networks (`aurora-upg-*`, `aurora-mm2-server`, hosted/channels
  anchors) removed themselves; my worktree `target/` (16 GB at the end).
- **Left, not mine to remove:** the images the scripts rebuilt under shared names (`localhost/aurora-sandbox:0.1.0`,
  `localhost/aurora-e2e/*:e2e` refreshed by `kind.sh --release-images`, `localhost/aurora-llm-sandbox:0.1.0`,
  `mattermost/mattermost-preview` pulled by the conformance scripts) and the pre-existing owner and other-lane objects
  (cluster `aurora-dev`, `aurora-dev-pg`, volumes `aurora-kind-dev-*`).
- Raw logs stay in `/tmp/audit-002-logs/` (scratch scripts `run.sh`, `cnums.py`, `cn-check.sh`, `sec-check.py`,
  `sweep-check.sh`, `percrate.py` live there too); they are not committed.
- `/home` free space: 92 GB at the start, 78 GB lowest, 83 GB at the end of the runs.
