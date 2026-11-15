# P5-FILES-4: Files end to end — kind journey and the Playwright journey

## Goal
Prove the whole Files feature on a real deployment: the API, runner, supervisor and image together on kind, and the dashboard journey in a browser, including the security-relevant refusals and a no-leak check (D-316, SEC-125, SEC-126).

## Linked requirements
- D-316; C-907; SEC-125, SEC-126 ("Proven by" e2e parts); `18 §Files` **Journey** bullet.

## Context
- E2e conventions: `scripts/e2e/hosted.sh` and `gateway.sh` (the e2e lock, `scripts/e2e/lib-skip.sh` and `require_or_skip`, cluster bring-up through `scripts/e2e/kind.sh`, a hosted agent fixture); the routes of C-898 (call them with a dashboard session cookie the way the existing e2e scripts sign in).
- The Playwright spec `web/e2e/files.spec.ts` written by P5-FILES-2/3: extend it to the full C-907 journey rather than duplicating steps.
- A file change "from the terminal" in the conflict step: write it through `PUT …/files/content` with no precondition from a second session, or through the terminal WebSocket if the e2e already has a client for it.

## Scope
### In scope
`scripts/e2e/files.sh` (new): uses the e2e lock; `require_or_skip` when `terminal.enabled` is off; with a hosted agent: upload a folder of three files (mkdir `parents` + PUTs), list it, download one and compare sha256, text read + save with `If-Match`, provoke and observe 409 `file_changed`, rename, recursive delete, `.aurora` write → 403, a symlink to `/etc` (created through the terminal or a turn of the fake harness) → 403 `path_outside_workspace`, a 20 MiB upload/download round trip with matching sha256, freeze → 423 then unfreeze, park the sandbox → next request 503 `sandbox_waking` then success after the wake; a secret-grep for a canary string uploaded inside a file over API, runner and supervisor logs, `audit_log` and `request_log` (no hit). The Playwright journey of C-907 completed in `web/e2e/files.spec.ts`. A `just` recipe or an entry in the e2e index the other scripts use.

### Out of scope
Fixing product bugs found here: report each with evidence and the failing step (the coordinator writes the fix brief); performance budgets (not measured while lanes run).

## Constraints / file ownership
- Owns: `scripts/e2e/files.sh` (new), the e2e index/`justfile` line for it, `web/e2e/files.spec.ts` (journey steps only).
- Test code only; no product code.

## Acceptance criteria
1. `scripts/e2e/files.sh` PASSES on the kind cluster with `terminal.enabled` on, and SKIPS cleanly (exit 0, reason printed) with it off.
2. The Playwright journey passes in both themes.
3. The canary secret-grep finds nothing.
4. `bash scripts/guards/run-all.sh` passes.

## Verification
Run both journeys twice in a row (no state leaks between runs); paste the PASS lines in the report.

## Dependencies
- Blocked by: P5-FILES-1, -2, -3 merged (or committed earlier in the same lane).
- Unblocks: closing D-316's FEATURES row.

## Suggested agent/model and reason
`haiku-tester` for the script against a clear checklist; escalate to Sonnet on non-obvious failures (wake timing, cluster flakiness). Size S–M.

## Completion report
- Files changed:
- Tests/checks:
- Remaining risks:
