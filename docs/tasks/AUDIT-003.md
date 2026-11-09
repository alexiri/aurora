# AUDIT-003: Phase 3 exit verification from scratch

## Goal
Produce the evidence for the phase-3 exit, as defined in `docs/PROGRESS.md` §Phase exit criteria → "Phase 3 (AUDIT-003)". Run everything from a clean state on **one commit** (master HEAD when the lane starts) and record the exact results. Use the shape of `docs/audit/phase2-exit.md`: verdict, environment, results table, per-step notes, deviations, fixes, open findings, leftovers.

This task **verifies**; it does not develop. Report every failure. A fix is made only if it is trivial and clearly correct, and then it is listed. Anything bigger becomes a finding, and the coordinator turns it into a fix task.

## Known deviations, owner-accepted
- **CI** (2026-11-08): GitHub Actions doesn't start jobs (billing). Run **every CI and nightly command locally** on the audited commit (`ci.yml` steps incl. the Vault job, `nightly.yml`'s `first-run` / `kind-e2e` / `mattermost-conformance`, `scripts/plugins/deny-hooks.sh`). Record it as a deviation, not a failure.
- **Performance** (2026-11-09): `scripts/perf/*` run only on an otherwise idle machine. The coordinator runs them outside this lane. **Do not run `scripts/perf/*`**; record "run by the coordinator" and leave the row for them.
- **Real cluster**: the owner is installing on a real cluster (FIX-017). Record the real-cluster checklist as "owner, in progress", not as run.

## Steps (record command, exit code, summary line, counts, duration for each)
1. **Clean build** (fresh worktree `target/`): `cargo build --workspace --all-targets` (time it), `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check`, `bash scripts/plugins/deny-hooks.sh`, `bash scripts/guards/run-all.sh`.
2. **Unit and service tests:** `cargo test --workspace --no-fail-fast --exclude aurora-e2e-gateway --exclude aurora-e2e-hosted -- --test-threads=8`. Record totals per crate. Known flake to watch: `aurora-gateway` `outage::metrics_and_the_state_change_log_line` fails under load at default threads; if it fails, rerun that crate alone and record both.
3. **Vault (exit criterion):** `AURORA_TEST_VAULT=1 cargo test -p aurora-secrets` against a Vault dev server (the test support starts a throwaway container; image as pinned in `.github/workflows/ci.yml`), incl. the CAS race tests; plus the `secrets migrate` round trip inline → Vault → inline (the api `secrets_migrate` tests with the Vault gate, and the operator procedure in `docs/operator/vault.md` followed literally once). Name the podman objects `audit3-`.
4. **Web:** `pnpm -C web install --frozen-lockfile`, `typecheck`, `lint`, `test`, `build`; Playwright `stage4-routes`, `stage4-axe`, `shell-theme`, `stage2-narrow`, `smoke`, `chat-v2`; compare the production CSP header with the phase-2 audit.
5. **Contracts and data:** OpenAPI drift test (`UPDATE_OPENAPI` unset); `scripts/extract-schema.py --check`; permission matrix 0 inconclusive; every phase-3 C-number (C-310..C-385, `docs/API_CONTRACTS.md`) has an implementation: sample each RFE's block and list any you can't find. `docs/03 §Planned changes` holds no phase-3 rows.
6. **Plugins:** `bash scripts/fixtures/build.sh`, `bash scripts/plugins/check-builtin.sh`, `bash scripts/plugins/build-builtin.sh --build-only --out <scratch>`; the link workflow if it exists (network; record either way); project-key signing not exercised (CI-only key).
7. **E2E suites**, each run separately and never concurrently (`gateway.sh` takes a host-wide lock; the others do not):
   - `bash scripts/e2e/m1.sh`; `bash scripts/e2e/gateway.sh`;
   - `AURORA_TEST_PODMAN=1 bash scripts/e2e/hosted.sh` (sandbox image rebuilt from scratch on the neutral public base, RFE-019), plus once with `AURORA_E2EH_FULL_IMAGE=1`;
   - `AURORA_TEST_PODMAN=1 bash scripts/e2e/channels.sh`; `bash scripts/conformance.sh` and `--mattermost`; `bash scripts/e2e/mattermost.sh`;
   - `AURORA_TEST_PODMAN=1 AURORA_TEST_LLM=1 bash scripts/e2e/llm.sh`;
   - `UPGRADE_STRICT=1 UPGRADE_BASE=cc6ccbb345cf03ad2b881d9585e8a19329ceac9a bash scripts/e2e/upgrade.sh` (from the phase-2 exit commit);
   - `bash scripts/e2e/kind.sh --fresh` and `--release-images`;
   - `deploy/dev/kind-dev.sh up --llm-mock` on a fresh cluster, then `scripts/e2e/first-run.sh`, **twice**.
8. **Phase-3 suites of `docs/18`:** for each, name the tests that prove it and confirm they ran green in steps 2–7: BYO token age (RFE-013), database outage incl. `readyz` states (RFE-014), message idempotency (RFE-018), deployment-neutral defaults (RFE-019), workspace-tier skills (RFE-020), Default Rule and LLM hosts (RFE-023), plugin major upgrades (RFE-022), self-contained deprovision (RFE-024), Vault (Q12).
9. **Security coverage:** for every row of `docs/tests/security-coverage.md`, confirm the cited test ran and passed; SEC-53..SEC-58, SEC-61 and the Q12 Vault invariant must be `proven`. List any row whose test was not executed. Also check D-118 (OIDC `trusted_email_domains`) is reflected in SEC-42's evidence.
10. **Install from scratch:** `docs/operator/install-helm.md` literally on a fresh kind cluster (`aurora-audit3`), incl. the new §Behind an external reverse proxy where kind allows (Gateway API CRDs are not on kind by default: record what was and wasn't exercised). Note every doc/reality deviation.
11. **Spec sweep and docs:** `grep -rn "(phase 3" docs/*.md` (excluding `80`, PROGRESS, FEATURES): each rule implemented or explicitly deferred; say how you sampled. Operator docs exist for Vault, outage behaviour and alerts, deployment-specific settings. Every phase-3 row of `docs/FEATURES.md` is `done` with evidence or owner-deferred.

## Constraints
- **Owns:** `docs/audit/phase3-exit.md` (new), trivial listed fixes, nothing else. Do not edit PROGRESS or FEATURES (coordinator).
- Kind clusters `aurora-audit3*` only; the e2e scripts' own fixed names are fine when nothing else uses them: check `kind get clusters` and `podman ps` before each. Podman objects you create directly: prefix `audit3-`.
- Disk: `df -h /home` before each image-heavy step; stop and report below 50 GB.
- Run every command in the foreground with `timeout`; for long runs log to a file and poll it in the foreground until it exits. Never end your turn while a command is running.
- Change files only with the Edit/Write tools; never python, sed, awk, perl, `cat >>` or heredocs to create or modify files.
- Delete everything you created at the end (clusters, podman objects, your `target/`). List any leftovers.

## Deliverable
`docs/audit/phase3-exit.md`: verdict, environment, a results table for every step (command, exit code, counts, duration), per-step notes, deviations (CI, perf by coordinator, real cluster by owner), fixes made, open findings (each a candidate fix task), the security-row execution check, leftovers. Commit on your branch with prefix `AUDIT-003:`; don't push or merge.
