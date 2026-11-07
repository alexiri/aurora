# AUDIT-001: Phase-0 exit verification from scratch

## Goal
Produce the evidence for the phase-0 exit (`docs/19`): "all milestones accepted; every SEC invariant has a passing proving test; the compose stack and the Helm chart install from scratch with documented steps". Run everything from a clean state on the current `phase0` HEAD and record the exact results. This task verifies; it does not develop. Fix nothing silently: report every failure. A fix only happens if it's trivial and clearly correct, and then it is listed.

## Steps (record command, exit code, summary line, duration for each)
1. **Clean build**: a fresh worktree `target/`, `cargo build --workspace --all-targets` (record the time), `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check`, `bash scripts/guards/run-all.sh`.
2. **Unit/service tests**: `cargo test --workspace --no-fail-fast` excluding the e2e crates. Record the totals per crate.
3. **Web**: `pnpm -C web install --frozen-lockfile`, then `typecheck`, `lint`, `test`, `build`.
4. **Fixtures and conformance**: `bash scripts/fixtures/build.sh`; `bash scripts/conformance.sh target/debug/aurora-connector-fake`; the supervisor harness conformance (fake).
5. **E2E suites**, each run separately (they share ports/bridges; never concurrently):
   - `bash scripts/e2e/m1.sh` (Playwright)
   - `bash scripts/e2e/gateway.sh`
   - `AURORA_TEST_PODMAN=1 bash scripts/e2e/hosted.sh` (rebuild the sandbox image from scratch: no SKIP_IMAGE_BUILD)
   - `AURORA_TEST_PODMAN=1 bash scripts/e2e/channels.sh`
   - `AURORA_TEST_PODMAN=1 AURORA_TEST_LLM=1 bash scripts/e2e/llm.sh`
   - `UPGRADE_STRICT=1 bash scripts/e2e/upgrade.sh`
   - `bash scripts/perf/gateway.sh`
   - `bash scripts/e2e/kind.sh --fresh`
6. **Install from scratch following the docs literally**: `docs/operator/install-compose.md` on a clean podman state (prefix resources `aurora-audit-`; if a fixed compose project name collides with leftovers from earlier lanes, say so and use a clean project name), then a sign-up → agent → chat smoke test against the running stack (curl or Playwright). Then `docs/operator/install-helm.md` on a fresh kind cluster (the kind script covers the chart, but follow the doc's own steps once and note every deviation between doc and reality).
7. **Security coverage check**: for every row of `docs/tests/security-coverage.md`, confirm the cited test ran in steps 2–5 and passed. List any row whose test was not executed.

## Deliverable
`docs/audit/phase0-exit.md`:
- a results table for every step (command, result, counts, duration)
- environment facts (host, podman, kind and toolchain versions, commit hash)
- deviations and failures, with details
- doc corrections made
- the security-row execution check

Commit it on your branch with prefix `AUDIT-001:`.

## Constraints
- Disk: check `df -h /home` before each image build. The whole run needs ~40 GB; clean up images you built if space falls under 30 GB, but never delete images/containers you didn't create.
- Don't run e2e suites concurrently with each other.
- Some suites take 15+ minutes: keep each foreground command under 10 minutes by running long suites in the background and polling their log in short foreground loops. Never end your turn while anything is running.

## Suggested agent/model
Sonnet, own worktree (long-running).
