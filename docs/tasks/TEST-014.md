# TEST-014: M5 hosted suites: audit spec 18 §Hosted agents and fill the gaps

## Goal
Make every item in `docs/18-testing.md` §Hosted agents (and the M5 parts of §Secrets that touch hosted runtime) have a passing test, by mapping each to an existing test or adding one.

## Linked requirements
- `docs/18-testing.md` §Hosted agents. Each bullet item must map to a test:
  - due-work engine: every arm, claim races with concurrent pollers, ownership fencing, replica death and takeover
  - turn lifecycle: queued → done; follow-up joined/promoted; abort; send-now; never-started revival; stall; ceiling warning and failure; restart
  - home sync: parts packing at worst-case encodings, crash mid-generation, manifest pruning, memory harvest round-trip with checksum verification
  - memory search with `english`, `german` and `simple` languages; language change re-render
  - schedules: coalescing missed fires, gate quiet/wake/fail, authorization disable, failure disable
  - security probes inside a real sandbox (SEC-01, 02, 09, 26, 34), already in TEST-013 / OPS-011
- `docs/19` M5 acceptance ("hosted suites of 18"). Behaviour specs: `docs/10`, `docs/11`, `docs/12`, `docs/05` §The due-work engine.

## Scope
### In scope
1. **Audit first**: write `docs/tests/hosted-coverage.md`, a table of each bullet item → existing test(s) (file::name) or "missing". Search `services/aurora-api/tests/{hosted,runner_plane,conversations,memory,skills,schedules,webhooks,jobs,*}.rs`, `services/aurora-supervisor/tests/*`, `services/aurora-runner/tests/*` and `tests/e2e-hosted/tests/*`.
2. **Fill the gaps** with focused tests in the owning crate, at the cheapest level that proves the behaviour: service tests with the fake runner backend and scripted supervisor where possible, and e2e only when a real sandbox is required.
3. **Fix the real defects** the new tests expose, minimally, each listed in the report. If a fix is non-trivial or crosses into contract/architecture territory, stop and report instead.

### Out of scope
- Channels (TEST-016), gateway performance (OPS-014).

## Constraints / file ownership
- You own `docs/tests/hosted-coverage.md`, new test files and new test functions appended to existing test files in aurora-api/aurora-supervisor/aurora-runner, and minimal production fixes with justification.
- Other lanes may edit aurora-api production code (FIX-003/004). Keep production edits minimal.

## Acceptance criteria
- [ ] The coverage table shows no "missing" rows. Each row cites a passing test.
- [ ] `cargo test -p aurora-api -p aurora-supervisor -p aurora-runner` is green; fmt, clippy `-D warnings` and guards pass.

## Suggested agent/model
Sonnet, own worktree (audit + tests).
