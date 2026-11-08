# P2-TEST-1: Test debt (phase 2)

Close the test gaps phases 1 and 2 left open. **Tests and test tooling only**: no product behaviour change. If a test exposes a product bug, stop and report it (Orca `ask`); don't fix product code in this lane. Decision: **D-98** if needed.

## Items
1. **SEC-34 `process_start` env probe** (TEST-013 follow-up; `17` SEC-34: boot material is not in the agent-visible environment).
   - In `tests/e2e-hosted` (`scripts/e2e/hosted.sh`), add a probe that starts a background process through the `process_start` supervisor tool and checks that the process's environment does not contain `AURORA_BOOT`, any `aub_`/`aup_`/`auc_` value, or the bootstrap token.
   - Do it like the existing s8b probes (`/proc/1/environ` unreadable).
2. **Chromium wrapper on the full sandbox image** (FU-P1-003b, SEC-41).
   - A hosted scenario on the full sandbox image (the image with the `browser` capability label) that launches the chromium wrapper.
   - Check that it starts behind the open proxy and that a page fetch goes through the gateway: an allowed host is fetched, a denied host is refused.
   - If the full image isn't built by `hosted.sh` today, add the build behind a flag. Say how long it takes.
3. **Remove the 3 s sleep** in `tests/e2e-gateway` `h1_held_request_gets_hook_summary_without_egress` (FU-P1-003c).
   - Wait on the actual condition instead: poll the approval row or the event, with a deadline.
   - It must pass at high load (run it with a parallel CPU hog to show it).
4. **Checksum-trailer cross-crate test** (FUP-M5).
   - One test that renders a home file with the API's renderer and harvests it with the supervisor's harvester, using the real functions from both crates and not copies.
   - Assert that the checksum trailer round-trips and that tampering is detected.
   - Put it where both crates are dev-dependencies without creating a dependency cycle; propose the location in your report.
5. **`api_ratelimit::each_class_has_its_own_limit` flake** (failed once at load ~17, passes alone).
   - Find the timing assumption and make the test deterministic: controllable time, or a wide enough margin with an explanation.
   - Show it passing 20× in a row under a parallel full `aurora-api` test run.
6. **`hosted.sh` isolation** (FU-P2-TEST-hosted).
   - Derive the container, network and image names (`aurora-e2eh-*`) and the subnet (`10.89.252.0/24`) from an env prefix with today's values as the default, so two runs from different worktrees don't collide.
   - Look into the s9 race: `podman exec` rc 125 right after a turn, against the 10 s idle stop. Fix the test's timing or ordering, not the product's idle stop.

## Constraints
- Tests and scripts only; no product code changes.
- Keep every existing scenario green.
- **Owns:**
  - `tests/e2e-hosted/**`, `tests/e2e-gateway/**`, `scripts/e2e/hosted.sh`;
  - the ratelimit test file in `services/aurora-api/tests/it/`;
  - a new test location for item 4 (supervisor/api `tests/it` or a test-only crate);
  - `docs/18-testing.md` notes.
- Don't touch gateway or API source (P2-FU-1 and P2-FU-2 run in parallel there).

## Acceptance criteria
- [ ] `scripts/e2e/hosted.sh` full run green, including the new probes and the chromium scenario (or that scenario behind a documented flag, with its run shown green).
- [ ] `scripts/e2e/gateway.sh` green; item 3 green under load.
- [ ] Item 4 test green.
- [ ] Item 5 shown 20/20 under load.
- [ ] fmt, clippy `-D warnings` on touched crates, and `bash scripts/guards/run-all.sh`.
