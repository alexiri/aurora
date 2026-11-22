# P5-SKILLDIR: one name for the skills folder on every harness (owner, 2026-11-22)

## Goal

Owner, 2026-11-22: "what if we kept the projection as is, to `<harness skills dir>/<name>/`, added an
environment variable `$SKILLS_DIR=<harness skills dir>/` and then we can just have the skills reference
`$SKILLS_DIR/myskill/scripts/helper.sh`? this seems simpler". The problem: Aurora writes each skill into
the harness's own folder (`.claude/skills` for claude-code, `.codex/skills/aurora` for codex,
`.agents/skills` for pi, goose and opencode; `services/aurora-api/src/domains/hosted/home.rs:21`). A skill
that ships a helper script and names its path (`~/.claude/skills/foo/scripts/helper.sh`) works on one
harness only, and stops working when the agent's harness is changed.

After this brief every place an agent or a person runs a command in the sandbox has
**`SKILLS_DIR=<home>/<harness skills dir>`** (absolute, no trailing slash, e.g.
`/workspace/.agents/skills`), and the instruction file names the same folder in words, so a skill says
`"$SKILLS_DIR/<name>/scripts/helper.sh"` and works on every harness. The projection itself does not
change.

## Decisions that bind this brief

- **D-454** (the variable, its name, where it is set) and contract **C-1140** (exact value, the
  instruction line, the docs).
- Why `SKILLS_DIR` and not `AURORA_SKILLS_DIR`: the terminal, SSH and `process_start` environments
  strip every `AURORA_*` variable (`12 §Terminal` :990, `§SSH channels` :1135, `exec.rs:302`, `pty.rs:796`).
  Those are exactly the environments where a person or a background process runs a skill's helper. Do
  **not** add an exception to the `AURORA_*` sweep. Aurora sets the variable last, so a value from the
  image or an agent's profile does not win in the environments Aurora builds.

## Order and lanes

- One small Rust lane (Sonnet), size S. No migration, no API or wire change, no web change.
- It edits `services/aurora-supervisor/src/` only (`supervisor.rs`, `process.rs`, `pty.rs`, `exec.rs`,
  `instructions.rs`, the adapters' `config.rs` tests, `harness.rs` if the type changes), its snapshots,
  `tests/e2e-hosted/tests/hosted.rs` (one case), and the docs below.
- **Shared file with P5-APPSKILL-2** (written 2026-11-22 by another session, not merged yet):
  `instructions.rs` (the connected-apps paragraph, C-1135) and the supervisor's skills path handling.
  P5-SKILLDIR is small, so it goes **first**; P5-APPSKILL-2 rebases on it, and P5-APPSKILL-4's scripts
  can then use `$SKILLS_DIR` for sibling skills instead of `$(dirname "$0")/..`.

## Read first

- `docs/12-supervisor-harness.md` §Harness interface (:84, `skills_dir`), §Adapter `codex` (:315, "else
  none"), §Home projection (:818), §Process manager (:925), §Terminal (:990), §SSH channels (:1135).
- `docs/10-agents-conversations.md` §Skills (:865), §Agent instructions (:736).
- Code: `supervisor.rs` :860–900 (the harness env, where `AURORA_SESSION` is pushed) and :204 (the home
  projector gets `caps.skills_dir`); `process.rs` :48–70 (`ProcessConfig.env`); `pty.rs` :760–820 and
  `exec.rs` :300–350 (the terminal and SSH environments); `spawn.rs` `child_env`, `may_pass` (the name
  passes the secret filter; check it); `instructions.rs` :320–345 (preamble) and :504 (the Skills
  fragment); the five adapters' `skills_dir` in `adapters/*/mod.rs`.

## What to build

1. **The value.** One function in the supervisor computes it from the working dir (the home root) and
   the adapter's `skills_dir`: `working_dir.join(skills_dir)` as a string, no trailing slash. Every
   adapter declares a folder today, so make the harness interface require it: `skills_dir: RelPath`
   (not `Option`) in `12 §Harness interface` and in code. If changing the type is more than mechanical,
   keep the `Option` and leave the variable unset when it is `None`; say which in the report.
2. **Harness processes**: push `SKILLS_DIR` next to `AURORA_SESSION` in `supervisor.rs`. Extend the
   adapters' existing "session variables reach the harness process" tests (`claude_code/config.rs:345`,
   `opencode/config.rs:423`, `codex/config.rs:235`, `pi/config.rs`, `goose/config.rs`).
3. **Background processes** (`process_start`): add it to `ProcessConfig.env` where the proxy variables
   are added. Unit test: a started process sees it.
4. **Terminal and SSH**: add it to the base environment after the `AURORA_*` sweep, alongside `TERM`.
   It is not in the SSH client's accepted-name list (`LANG`, `LC_*`): a client cannot override it. Unit
   tests in `pty.rs` and `exec.rs`: present, with the adapter's value, and still no `AURORA_*` variable.
5. **Instruction file** (C-1140): the Skills fragment (`instructions.rs:504`) says where the folder is
   and how to refer to a skill's own files. The exact text:
   `Skills are read-only folders in <abs path> (also $SKILLS_DIR in every shell); list and read them with the skill tools. A skill's own files are under <abs path>/<skill name>/; run its scripts as "$SKILLS_DIR/<skill name>/scripts/<file>".`
   `RenderInputs` (or `render_with`) gets the path as an input; the path is Aurora's, not third-party
   text. Update the instruction snapshots for each harness's path.
6. **Hosted e2e** (`tests/e2e-hosted/tests/hosted.rs`, next to the `AURORA_SESSION` case :3133): a
   `process_start` command `test -x "$SKILLS_DIR/<a fixture skill with an executable file>/<file>"`
   exits 0, and `echo "$SKILLS_DIR"` equals the fake harness's folder. CI runs it; the lane does not.

## Docs in the same commits

- `12 §Harness interface` (`skills_dir` required), `§Adapter codex` (drop "else none … listed in
  `AGENTS.md`"; the value is `.codex/skills/aurora`), `§Home projection` (one sentence: the folder is
  exported as `SKILLS_DIR`), `§Process manager`, `§Terminal`, `§SSH channels` (the variable is added
  after the sweep).
- `10 §Skills`: a bullet "A skill refers to its own files as `$SKILLS_DIR/<name>/…`; a path with a
  harness folder in it (`.claude/skills/…`) works on one harness only". `10 §Agent instructions`: the
  Skills line.
- `docs/24-apps-for-agents.md:881`: the "codex may have no skills directory" reason is wrong (codex has had
  `.codex/skills/aurora` since P4-HAR); say so in one line and point to D-454.
- `docs/operator/skill-libraries.md`: one paragraph for library authors (scripts by `$SKILLS_DIR`).

## Tests move with the code (owner, 2026-11-20 — binding)

In **every commit**: find every test that sees what you change
(`rg -n "Skills are read-only folders|skills_dir|AURORA_SESSION" services/aurora-supervisor tests scripts`),
update it in the same commit, never `.skip`, `#[ignore]`, delete or loosen a test to get green. The
instruction snapshots change on purpose: review each diff, don't regenerate blindly.

## Acceptance

1. The harness process, a `process_start` process, a terminal shell and an SSH exec each have
   `SKILLS_DIR` equal to `<home>/<the adapter's skills_dir>`, for all five adapters (unit tests); none
   of them gains any `AURORA_*` variable it did not have before.
2. The instruction file's Skills section has the C-1140 text with the harness's real path (snapshots
   for claude-code, codex and one of pi/goose/opencode).
3. The hosted e2e case is written and compiles (`cargo test -p aurora-e2e-hosted --no-run`); CI runs it.
4. No change to the projection: the same files land in the same folders (existing home tests unchanged).
5. Docs above updated; `python3 scripts/extract-schema.py --check` and `bash scripts/guards/run-all.sh`
   pass.
6. Lane checks (fast and targeted; CI is the full gate): `cargo fmt --check`;
   `cargo clippy -p aurora-supervisor --all-targets -- -D warnings`;
   `cargo test -p aurora-supervisor` filtered to `config`, `process`, `pty`, `exec`, `instructions`,
   `home`. Run builds and tests through `/home/alex/orca/workspaces/aurora/heavy.sh <lane>`.

## Out of scope

- Moving the skills to one shared folder or linking the harness folders (rejected, D-454).
- A warning in the skill editor or the library check for hard-coded harness paths (possible follow-up;
  needs an API `warnings` field).
- BYO agents: they have no projected home, so no variable; `10 §Skills` says so.
- App-shipped skills (P5-APPSKILL-1..4).

## Completion report (lane A, Sonnet, 2026-11-22)

**Done: every item of "What to build" and the docs list.**

- `skills_dir` is now `String` (required) on `HarnessCapabilities` (type change was mechanical: five adapters, fake, their tests); `HomeConfig.skills_dir` stays `Option` (home tests unchanged, the projection is untouched).
- `harness::skills_dir_abs(home, rel)` computes the value (`<home>/<skills_dir>`, no trailing slash). Set in `supervisor.rs`: harness env (after `AURORA_SESSION`, existing value replaced), `ProcessConfig.env` for `process_start`. Terminal and SSH: `PtyEngine::spawn_with_harness_env`/`spawn_with_exec` take `skills_dir: Option<String>`; `exec::with_skills_dir` adds it to the child env map after the client's accepted names (last wins; `main.rs` passes `/workspace` + the adapter folder). The `AURORA_*` sweep has no exception.
- Instructions: `render_with(i, agent_cli, skills_path: Option<&str>)`; `home.rs` passes the absolute folder; `render(i)` (tests) uses `/workspace/.claude/skills`. `None` keeps the old plain sentence (only a home without a skills folder, tests).
- Hosted e2e `skills_dir_names_the_harness_skills_folder_in_started_processes` in `tests/e2e-hosted/tests/hosted.rs` (written, compiles; CI runs it).
- Docs: 10 (§Agent instructions, §Skills), 12 (§Harness interface, §Adapter codex, §Home projection, §Process manager, §Terminal, §SSH channels), 24:881 reason corrected, `operator/skill-libraries.md` author paragraph. C-1140 was already in API_CONTRACTS.

**Test files changed:** `src/adapters/*/config.rs` (5, SKILLS_DIR next to AURORA_SESSION), `src/adapters/*/tests.rs` (4, `skills_dir` is a `String`), `src/harness.rs` (new unit test: the five adapters' values), `src/instructions.rs` (new test + 3 new snapshots claude-code, codex, `.agents`), the 5 existing instruction snapshots (diff = the Skills line only, reviewed), `tests/it/pty_env_probe.rs` (terminal has SKILLS_DIR), `tests/it/exec_ssh.rs` (shell/exec with and without tty have it; client `SKILLS_DIR` refused), `tests/it/exec_sftp.rs` (rig passes it), `tests/it/spawn_env.rs` (a `process_start` process sees it, an image value is replaced), `tests/it/home.rs` (`skills_dir: Some(..)`; two tests that compared instruction files across different temp roots now reuse one root path, because the file now contains the absolute folder).

**Commands (through heavy.sh):** `cargo fmt --check` ok; `cargo clippy -p aurora-supervisor -p aurora-e2e-hosted --all-targets -- -D warnings` ok; `cargo test -p aurora-supervisor` (lib + `--test it`) filtered to `config process pty exec instructions home skills_dir a_started_process harness:: spawn_env sftp ssh`: lib 71 passed, it 88 passed; `cargo test -p aurora-e2e-hosted --no-run` ok; `extract-schema.py --check` ok; `guards/run-all.sh` ok.

**Deviations / notes:** none against the brief. A few mechanical multi-file edits (adapter `skills_dir` literals, five identical test additions) were done with sed/python, not Edit. The adapter "session variables" tests mirror the supervisor's push (the supervisor wiring itself is covered by the `spawn_env` IT for `process_start`; the harness-process push in `supervisor.rs` has no direct test, as the existing AURORA_SESSION push has none). PROGRESS/FEATURES not touched (Orchestrator). Open questions: none.
