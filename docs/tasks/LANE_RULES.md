# Lane rules (coordinator house rules for every worker lane)

Every lane spec names its lane name, its task ids and their briefs (`docs/tasks/<ID>.md`), and points here. Read this file fully before starting.

**Your briefs are normative.** Read each `docs/tasks/<ID>.md` named in your lane spec in full before touching code, then the spec rows it cites (`docs/API_CONTRACTS.md` C-xxx, `docs/17-security.md` SEC-xx, `docs/90-decisions.md` D-xxx). If you find a brief is wrong (the defect isn't real, or the fix would contradict the governing spec), do not "fix" it: write an `## Analysis` section in the brief explaining why with file:line evidence, commit that, and report it.

## Setup (first thing)
1. Your workspace is a fresh worktree of the aurora repo from `master`. Create your branch: `git checkout -b alexiri/<lane-name>`. Confirm `git log --oneline -1` is at or after the commit your lane spec names.
2. `cp /home/alex/agent-platform/aurora/.env .` (scripts and tests need it; it's gitignored).
3. `export DATABASE_URL=postgres://aurora:aurora@127.0.0.1:55432/aurora` for every cargo test (sqlx::test and aurora-testkit create and drop their own scratch databases; **never** write to the `aurora` database itself).
4. Incremental compilation is on by default for workspace crates (`profile.dev`, VEL-004/I-45) — nothing to export. Do **not** `export CARGO_INCREMENTAL=1`: it also makes registry deps incremental (uncacheable by sccache). If `target/` grows, `just clean-stale`.
5. If you run `connect::*` api tests: `bash scripts/fixtures/build.sh` once.

## Working rules
- **One commit per task id** (several are fine if the work is large — "evidence then fix" is OK), message prefix `<ID>: `. Commit as you go; don't leave the tree dirty between tasks. Never add co-authored lines.
- **Scope**: only the files your brief's "Owns" lists. If the fix genuinely needs more, stop that task and report rather than widening.
- **Change files only with the Edit/Write tools.** Never python, sed, awk, perl, `cat >>`, `tee` or heredocs to create or modify files (owner rule), unless it's a bulk edit where it makes sense. Repo generators are fine (`UPDATE_OPENAPI=1 cargo test -p aurora-api --test it openapi_matches_checked_in`, `pnpm -C web gen:api`, `UPDATE_SCHEMAS=1 cargo test -p aurora-protocol`, `cargo fmt`).
- **Do not edit `docs/PROGRESS.md` or `docs/FEATURES.md`** — the coordinator does. Fill the brief's "Completion report" section if it has one.
- New tables/columns → real `### \`table\`` sections in `docs/03-data-model.md` (`created_at`/`updated_at` implicit) and `python3 scripts/extract-schema.py --check`. Migrations: `git fetch origin` and take the next free timestamp after `origin/master`'s newest; say so in the commit.
- Tests must prove the behaviour: no sleeps for synchronisation (use barriers/notify/polling with a deadline), no assertions on mocks where a real path exists, no `#[ignore]`. A new test must fail without your fix — check that for at least the main test of each task and say so in the report. WebSocket test consumers answer Pings and bound every wait (FIX-051/FIX-055 playbooks).

## Gates (owner, 2026-11-13: full suites in batches, not after every fix)
**Per task (before its commit) — fast, targeted:**
- `cargo fmt --all --check`; `cargo clippy -p <touched crates> --all-targets --all-features -- -D warnings`.
- The new/changed tests and the touched area's tests by filter (e.g. `cargo test -p aurora-api --test it -- scim_` / `cargo nextest run -p aurora-api -E 'test(/^scim_/)'`), plus the touched crates' lib tests; the fail-before check for the main new test.
- Web touched: `pnpm -C web typecheck` + the related vitest files (`pnpm -C web exec vitest run <files>`).

**Batch gate — every ~5 tasks, at the end of your chain, and before your final report:**
- The full suites of every crate touched since the last batch gate — **lib/unit tests too** (`cargo test -p <crate> --lib`; a lane once ran only the api it-suite and left a red api lib test on master): for `aurora-api` the full it-suite (`cargo nextest run -p aurora-api` or `cargo test -p aurora-api --test it -- --test-threads=8`; build the fixtures first with `bash scripts/fixtures/build.sh`), `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `bash scripts/guards/run-all.sh`, web `lint`/`test`/`build` if web changed, and `scripts/e2e/*.sh` the batch's changes require (e.g. `gateway.sh` for proxying).
- A batch-gate failure is fixed in its own commit (`<ID>: …` of the task that caused it) before continuing. Say in each per-task report which gate level ran.

## Host discipline (shared 16-core box, up to 3 lanes)
- Heavy commands under `nice -n 10`. Check `uptime` before long runs: wait while the 1-minute load is > 25.
- A compile stalled > 5 min at 0 % CPU with no output: report it, don't kill sccache or other processes.
- `df -h /home` before image-heavy steps; stop and report below 50 GB free.
- e2e scripts take `/tmp/aurora-e2e-gateway.lock`; never run two e2e suites at once; never touch the aurora-dev kind cluster, port 4000, or `/home/alex/agent-platform/aurora` (the owner's checkout — read-only, for `.env`).
- Run commands in the foreground with `timeout`; for long runs log to a file and poll it in the foreground. Never end your turn with a command still running.
- Performance measurements (VEL tasks) are only meaningful on a quiet machine: record `uptime` with every timing; if other lanes keep the load above ~8, measure A/B back-to-back and interleaved, and say the numbers were taken under load.

## Finishing
- Do **not** push or merge; the coordinator reviews your branch and merges it.
- Report (your final message, and `worker_done` per the Orca preamble): per task id — what was wrong (one line), what you changed, files, commit sha(s), each gate's command + result counts, which new tests you saw fail before the fix, anything you invalidated or left open. Never claim a check passed that you didn't run.
- On a provider/streaming error mid-task, end your turn with a one-line status; the coordinator will nudge you.
