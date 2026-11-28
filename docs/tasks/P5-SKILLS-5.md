# P5-SKILLS-5: Verify the Skills work against the mockups; send back what differs; repeat until it all matches (owner, 2026-11-27)

## Goal

After P5-SKILLS-1..4 are merged, walk the real app and check **every Skills item of the Skills & Memory review** against
its After mockup and its proposal text. Whatever does not match goes back to the brief that owns it with a change list
precise enough to act on; the owning lane fixes it and this brief re-checks **only those items**. **Repeat until every
item is Match**, or the owner accepts a difference in writing.

The owner (2026-11-27): "Add a brief at the end of each part (memory and skills) to compare the work done with the
original mockups and order more work if needed."

This brief is a review. It does not implement features (small direct fixes: Scope 6).

## Linked requirements

- The design package `docs/design/skills-memory/` (D-496): `README.md` (precedence and how to read a mockup — binding),
  `review.md` §Skills and §Also, `after/skill-1..6.png` / `-light.png` / `.html`, `before/*.png`, `seed.py`.
- The briefs and their completion reports: `docs/tasks/P5-SKILLS-1.md` … `P5-SKILLS-4.md` (read each report's listed
  differences and reasons first; a difference no brief or decision requires is still sent back).
- Decisions D-496, D-498, D-499, D-505, D-506; contracts C-1173..C-1177; security SEC-158.

## Wave and order

Last of the skills set: starts when P5-SKILLS-1..4 are merged and CI is green. A fresh lane with a browser, not an
implementation lane's worktree. Pass 1 checks everything; later passes only what was sent back (plus neighbours).
**Done only when a pass ends with zero "Needs changes" and zero "Minor difference"** (apart from owner-accepted ones).
If P5-MEMORY-5 runs at the same time, share one throwaway cluster only if both lanes agree; otherwise each brings its
own.

## Environment (read first)

- **Never** the shared `aurora-dev` cluster, `localhost:4000` or the `aurora` database in `aurora-dev-pg`. Bring up a
  throwaway cluster: `AURORA_DEV_CLUSTER=<name> AURORA_DEV_PORT=<free port> deploy/dev/kind-dev.sh up --llm-mock`
  (`df -h /home` first). Create the review's data as `docs/design/skills-memory/README.md` §Reproducing describes
  (`seed.py`, the "Anthropic skills" library, internal-comms on the organization, pdf on Daneel), plus: a second agent in
  Bob's workspace (to prove an off switch is per agent), skill uses (a `skill_get` from a mock turn and, if the mock
  harness can, a native skill load), a pending library update (check the library after a change, or the mocked
  route), Carol as a co-owner who cannot read Bob's chat. Tear the cluster down at the end and say so. Never prune
  podman volumes. Heavy commands through `scripts/dev/heavy.sh`; tests only through `scripts/dev/test.sh`.
- Capture at **1256 px and 390 px**, **dark and light**, full pages by scrolling and stitching; commit only the report
  and the captures of items that are not Match (`docs/design/skills-memory/verify/skills-pass<N>/<key>-<theme>-<width>.png`).
- Record console errors, CSP violations, 4xx `/v1` responses, horizontal overflow at 390 px, axe wcag2a/aa per page.

## The registry (check every row)

| Row | What to check | Mockup / evidence | Owner |
|---|---|---|---|
| skill-1 | One list; Add skill menu (Write a new skill · Add from a library · Upload a skill folder); chips with counts; columns Skill · Source · For <Agent>; display names; "Use it to …" sentences, no glued label; source chips; per-agent switch on inherited rows; app rows read-only; override line with Compare (DiffView); no "Not applied", no precedence essay | `after/skill-1*.png` | P5-SKILLS-3 (+1) |
| skill-2 | Side panel beside the list (URL keeps it); header, switch, Edit, Try it in chat (opens the chat with the draft), "…"; instructions rendered; file tree with code view and Executable tag; usage line ("in a conversation" for non-readers) | `after/skill-2*.png` | P5-SKILLS-3 (+1) |
| skill-3 | Read-only org/workspace/app skills: no inputs, the lock line per tier, the per-agent switch; library skill: source, pin, Update available, Review update only for approvers; never a Save button; the S-4 label bug gone | `after/skill-3*.png` | P5-SKILLS-3 |
| skill-4 | Editor: Name + "Folder: …", single-line "When should … use it?" with hint, Write/Preview (shared `MarkdownEditor`), size line, Files folded to one line, opened list with in-place edit, Add a file dialog with Executable, SaveBar; Upload a skill folder works | `after/skill-4*.png` | P5-SKILLS-3 |
| skill-5 | Dialog: libraries left with counts and Add a library; addable skills with Added tags; "n can't be added · Why?" folded with plain reasons; preview; Add to <Agent>; server budget | `after/skill-5*.png` | P5-SKILLS-4 (+1) |
| skill-6 | Manage <Org> › Skills entry (no switch needed for owners/admins), tabs Skills · Libraries; the shared list with "Used by N agents"; libraries as cards with every action visible, no sideways scroll; old URLs redirect | `after/skill-6*.png` | P5-SKILLS-2 (+3) |
| Also 1 | Administration › Skills: instance libraries and library settings; gone from Defaults | `before/A01*.png`, `A02*.png` | P5-SKILLS-2 |
| Also 2 | Workspace owners (team and personal) see Skills in the rail at the default altitude; org owners/admins see Manage › Skills | — | P5-SKILLS-2 |
| Also 3 | Words: Turn off for <Agent>, Remove from …, Stop updates and edit; never neighbours; display names except the folder line | `review.md` | P5-SKILLS-3 |
| S-1..S-13 | Each finding's problem is gone (re-do its Before screen) | `review.md`, `before/S*.png`, `L*.png`, `A*.png`, `P03*.png` | per proposal |
| D-499 | Off for one agent: other agents keep the skill; the agent's projected skills folder no longer has it (check the home on the computer's Files page or via the API); a lower tier's same-named skill applies; app skills cannot be turned off; only agent editors can | API + UI | P5-SKILLS-1/3 |
| D-505 | Usage counts and last use correct; a conversation never named to a non-reader; "Used by N agents" | API + UI | P5-SKILLS-1/3 |
| SEC-158 | Its row in `docs/17` and its proving tests exist and pass | `docs/17`, CI | P5-SKILLS-1 |
| Tests | CI green per merge; no skipped or loosened test; no test expecting retired words ("Skills this agent receives", "Not applied", "Use it when", "Detach" as a button beside Remove, the org label on a workspace skill) | CI, `rg` | each lane |
| Rules | `14` §Visual design on every skills screen: tokens only, shared primitives, both themes, no horizontal scroll at 390 px, the save pattern, the confirm ladder, dialogs top-anchored | — | each lane |

## Scope

1. **Check every row**: same screen, same data; compare layout, components, exact interface words, states (empty, off,
   replaced, library with and without an update, member vs owner vs admin, read-only/emergency, failing library) with
   the mockup and the brief's acceptance criteria; apply the precedence. Verdict: **Match**, **Minor difference**,
   **Needs changes** (the last two go back).
2. **Evidence per row:** capture paths (dark/light, 1256/390), the mockup path, the verdict, the change-list entry.
3. **The change list** (one entry per difference): owning brief and file; row and mockup path; **what differs**,
   measurably; **what it should be**, precisely; severity (broken / flow / rough / words).
4. **Regressions** around the changed screens: the agent page, templates and export/import that carry skills, the
   library update review (shared `DiffView`), the chat's Try it in chat.
5. **Report** `docs/design/skills-memory/verify/SKILLS-REPORT-pass<N>.md`: counts per verdict, the registry with verdicts,
   the change list grouped by brief, regressions, further ideas (not blocking), environment (cluster, commit, torn down).
6. **Small direct fixes** only where no running lane owns the file, with tests; list each. Everything else goes back.
7. **Send back and iterate:** message the Orchestrator (`SendMessage` to `Orchestrator`) with the change list grouped by
   owning brief; it queues the work in those lanes (or a FIX when the lane has closed). When fixes merge, re-check those
   rows and write the next pass's report. Owner-accepted differences are "Accepted (owner, date, where)" and count as
   Match.

## Tests move with the UI (owner, 2026-11-20 — binding)

Any direct fix updates every test that sees it in the same commit (`rg` the old text in `web/tests`, `web/e2e`,
`scripts/e2e`), never loosens, skips or deletes a test, and runs through `scripts/dev/test.sh` only. The registry row
"Tests" checks the lanes did the same.

## Docs to update

The reports; PROGRESS (this row's status per pass) and FEATURES (`P5/14§skills-polish` done when a pass is clean).

## Suggested agent/model and reason

Sonnet `sonnet-integration-reviewer` with a browser (comparison and precise change lists; no implementation).

## Completion report

Passes run, verdict counts per pass, report paths, what was sent back and merged, owner-accepted differences, the
cluster torn down.

**Pass 1 (2026-11-27, on `d2783670`): not clean — 21 Match, 4 Minor difference, 2 Needs changes.**

- Report: `docs/design/skills-memory/verify/SKILLS-REPORT-pass1.md`; captures in `…/verify/skills-pass1/`.
- To send back: C-1 (broken: unaddable library skills listed as addable, wrong sizes) and C-2 to P5-SKILLS-1/-4; C-3
  (unclamped descriptions), C-4 (update notice count), C-5 (diffs scroll at 390 px) to P5-SKILLS-3; C-6 ("Default
  branch") and C-7 (suggested label) to P5-SKILLS-2. Outside the registry: B-1 (broken, git libraries: editing a
  library's source stops update proposals and keeps stale ones) needs a new FIX.
- Merged fixes: none yet. Owner-accepted differences: none recorded.
- No direct fixes and no product code changed, so no tests or fail-first evidence of this lane's own.
- Not verified: the emergency reader (the build predates the FIX-277 code), app skills; see the report.
- Cluster `aurora-mem5` (:4530) **kept up** for P5-MEMORY-5 pass 2, on the owner's coordinator's instruction. PROGRESS
  and FEATURES not edited; `P5/14§skills-polish` is not done.

**Pass 2 (2026-11-28, on `406605f6`, with FIX-278 and FIX-279): 26 Match, 1 Minor, 0 Needs changes.**

- Report: `docs/design/skills-memory/verify/SKILLS-REPORT-pass2.md`; captures in `…/verify/skills-pass2/`.
- Confirmed fixed on the real `anthropics/skills`: C-1 to C-7, and B-1 in both directions (a dropped proposal's
  approve is 409). The emergency reader's Skills pages list files without opening them, with no 403 and no write.
- To send back: D-1 (words: the reader's agent list shows "From Acme 0" and "From the workspace 0" although such skills
  apply). Merged fixes since pass 1: FIX-278, FIX-279 (and FIX-280).
- The Tests row waits for CI on `246a5fa9` (the migration-test fix); `a1a59836` was red on it and on
  `terminal.spec.ts`.
- No product code changed. Cluster `aurora-mem5` **kept up** for P5-NAV-4 pass 2. `P5/14§skills-polish` is not done.

**Pass 3 (2026-11-28, on `853fec1a`, with FIX-281): clean, 27 rows Match.**

- Report: `docs/design/skills-memory/verify/SKILLS-REPORT-pass3.md` (no captures: nothing is left that isn't Match).
- D-1 fixed. The Skills tests pass wherever CI ran them; master's `web` is red only on NAV-4's T-1, which stops CI
  before the browser tests, so `skills` and `fix-277-reader` were run locally (68 passed).
- Owner-accepted differences: none. One question stays open ("Use it to …" on list rows).
- No product code changed. Cluster `aurora-mem5` **kept up** for NAV-4 pass 3. PROGRESS and FEATURES
  (`P5/14§skills-polish`) are left to the coordinator.
