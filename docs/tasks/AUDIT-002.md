# AUDIT-002: Phase 1 + 2 exit verification from scratch

## Goal
Produce the evidence for the phase-1 and phase-2 exit, using the twelve exit criteria in `docs/PROGRESS.md` §Phase exit criteria → "Phase 2 (with phase 1; AUDIT-002)". Run everything from a clean state on **one commit** (master HEAD when the lane starts, after P2-UI-5 has merged) and record the exact results.

This task **verifies**; it does not develop. Report every failure. A fix is made only if it is trivial and clearly correct, and then it is listed. Anything bigger becomes a finding, and the coordinator turns it into a fix task. Use the shape of `docs/audit/phase0-exit.md`: environment, results table, per-step notes, fixes, open findings, leftovers.

## Known deviation, owner-accepted (2026-11-08)
GitHub Actions doesn't start jobs (billing). Exit criterion 10 ("CI on GitHub green… plus the nightly jobs") is replaced by running **every CI and nightly command locally** on the audited commit: the `ci.yml` steps, `nightly.yml`'s `first-run` / `kind-e2e` / `mattermost-conformance`, and `scripts/plugins/deny-hooks.sh`. Record it as a deviation, not a failure.

## Steps (record command, exit code, summary line, counts, duration for each)
1. **Clean build:** a fresh worktree `target/`.
   - `cargo build --workspace --all-targets` (time it), `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check`, `bash scripts/plugins/deny-hooks.sh`, `bash scripts/guards/run-all.sh`.
2. **Unit and service tests:** `cargo test --workspace --no-fail-fast --exclude aurora-e2e-gateway --exclude aurora-e2e-hosted -- --test-threads=8`. Use `--test-threads=8` because the shared dev Postgres refuses connections under full parallelism. Exclude every e2e crate, since `gateway.sh`/`hosted.sh` run them. Record totals per crate.
3. **Web:** `pnpm -C web install --frozen-lockfile`, `typecheck`, `lint`, `test`, `build`. Then the Playwright suites that need no stack: `stage4-routes`, `stage4-axe`, `shell-theme`, `stage2-narrow`, `smoke`, `chat-v2`. Compare the production CSP header with master (`pnpm start` + `curl -sI`).
4. **Contracts and data:**
   - The OpenAPI drift test (`UPDATE_OPENAPI` unset; it must pass as is).
   - `scripts/extract-schema.py --check`.
   - Permission matrix: 0 inconclusive rows.
   - Every phase-1 and phase-2 C-number in `docs/API_CONTRACTS.md` has an implementation: sample each section and list any you can't find.
5. **Plugins:**
   - `bash scripts/fixtures/build.sh`, `bash scripts/plugins/check-builtin.sh` (all built-ins, incl. hook and token-lifecycle coverage), `bash scripts/plugins/build-builtin.sh --build-only --out <scratch>`.
   - The plugin link workflow (`scripts/plugins/check-links.sh`, if it exists; it needs the network, so record the result either way).
   - Signing: the release job can't run on GitHub. Record that the project-key signing path (`build-builtin.sh --sign-only`) was not exercised with the real key, which only CI holds.
6. **E2E suites**, each run separately and never concurrently:
   - `bash scripts/e2e/m1.sh`; `bash scripts/e2e/gateway.sh` (incl. instances and the database lane);
   - `AURORA_TEST_PODMAN=1 bash scripts/e2e/hosted.sh` (sandbox image rebuilt from scratch), plus once with `AURORA_E2EH_FULL_IMAGE=1` for the chromium scenario;
   - `AURORA_TEST_PODMAN=1 bash scripts/e2e/channels.sh`;
   - `bash scripts/conformance.sh` (fake) and `bash scripts/conformance.sh --mattermost`;
   - the Mattermost live scenario (`scripts/e2e/mattermost.sh`, if it exists);
   - `AURORA_TEST_PODMAN=1 AURORA_TEST_LLM=1 bash scripts/e2e/llm.sh`;
   - `UPGRADE_STRICT=1 bash scripts/e2e/upgrade.sh`, from the phase-0 release point to HEAD;
   - `bash scripts/perf/gateway.sh` (the budgets);
   - `bash scripts/e2e/kind.sh --fresh` and `--release-images`;
   - `deploy/dev/kind-dev.sh up --llm-mock` on a fresh cluster followed by `scripts/e2e/first-run.sh` (exit criterion 12, the owner's phase-2 bar), run **twice**.
7. **Install from scratch following the docs literally:**
   - `docs/operator/install-helm.md` on a fresh kind cluster (`aurora-audit`). Note every deviation between the doc and reality.
   - The real-cluster checklist in that doc can't be run (no cluster). Record it as not run.
8. **Security coverage:** for every row of `docs/tests/security-coverage.md` (SEC-01..SEC-60), confirm the cited test ran in steps 2–6 and passed. List any row whose test was not executed.
9. **Spec sweep** (exit criterion 2):
   - `grep -rn "(phase 1\|(phase 2" docs/*.md`, excluding `80`, PROGRESS and FEATURES. For each rule, confirm it is implemented or explicitly deferred by the owner. Sample the dense documents rather than reading everything, and say how you sampled.
   - `docs/03 §Planned changes` must hold no phase-1/2 rows except the owner-scheduled drop (CONTRACT-001).
10. **FEATURES:** every phase-1 and phase-2 row of `docs/FEATURES.md` is `done` with evidence, or `deferred` by the owner. List exceptions.
11. **Rechecks owed:** FU-P2-010a (`channels.sh` and `m1.sh` on P2-010: covered by step 6); FU-P2-025c (hosted e2e on P2-025: covered by step 6); FUP-M7 "granted-but-disabled connection still contributes env/stubs" (verify with a targeted test or the hosted run, and say which).

## Constraints
- **Owns:** `docs/audit/phase2-exit.md` (new), trivial listed fixes, and nothing else.
- Kind clusters `aurora-audit*` only. The e2e scripts' own fixed names are fine when nothing else uses them: check `kind get clusters` and `podman ps` before each.
- Disk: check `df -h /home` before each image-heavy step; stop and report below 50 GB.
- Delete everything you created at the end (clusters, podman objects with your prefix, your `target/`). List any leftovers.

## Deliverable
`docs/audit/phase2-exit.md`: a verdict, the environment, a results table for every step (command, exit code, counts, duration), per-step notes, deviations (incl. CI), fixes made, open findings (each a candidate fix task), the security-row execution check, and leftovers.
