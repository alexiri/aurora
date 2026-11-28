# Skills verification, pass 3 (P5-SKILLS-5): D-1 only

Date: 2026-11-28. Reviewer lane, no product code changed.

## Result

**Clean: D-1 is fixed, so all 27 rows are Match** (the others were Match in pass 2). No differences accepted by the owner
are needed. The one open question from pass 1 is below.

**CI on master isn't green yet**, only because of NAV-4's T-1 (the terminal fix's style guard, with lane
`ci-fix-layer`). No Skills test fails (details under CI).

## Environment

- Commit checked: `853fec1a` (local master with FIX-281, `6e1ac7aa`; it also holds P5-NAV-2).
- `aurora-mem5` (:4530) redeployed with `kind-dev.sh up --llm-mock --ssh` through `heavy.sh`, then
  `kubectl rollout restart` of every deployment, since `up` again left the old Pods running. **Still up** for NAV-4
  pass 3. `aurora-dev` and `aurora-skm` untouched.
- Alice opened an emergency grant for this check and ended it afterwards.
- Playwright, 1256 and 390 px, dark and light on the reader's agent page; the other checks at both widths, mostly dark.
  Axe, console, `/v1` 4xx, sideways overflow and every request that wasn't a GET were recorded.

## D-1: the reader's agent Skills page — fixed (row skill-1)

For Daneel, as Alice:
- Chips: **All 6 · Daneel's own 6 · Off 1**. No **From the workspace** or **From Acme** chip.
- No **For Daneel** column and no switches in the list; the skill that is off says **Off** beside its source. The Off
  filter shows it.
- One quiet line above the list: **"Emergency read access shows only Daneel's own skills."**
- For an agent with no skills of its own (Giskard): the same line, then **"No skills of its own" · "None to read."**,
  not "No skills yet".
- Bob's page is unchanged: All 13 · Daneel's own 6 · From the workspace 3 · From Acme 4 · Off 1, the **For Daneel**
  switches, no line.

## Neighbours

- An open skill panel for the reader: its switch and the compact list's are disabled (not a column, as FIX-281 says), and
  the files are listed with "Emergency read access doesn’t cover a skill’s files."
- The reader's workspace Skills page and its Webapp testing panel: unchanged and read-only (disabled switches, files not
  opened).
- **No 403, no other 4xx, and no request that wasn't a GET.** No axe or console finding. The only sideways scroll: code
  blocks inside instructions.

## Tests row and CI

- The migration fix is confirmed: on `9ca42d59` all four `rust-test` jobs pass, and `web` passes all 4,081 unit tests
  and 2,120 browser tests, FIX-278's and FIX-279's among them. Its one failure is `terminal.spec.ts` (that commit
  predates the terminal fix).
- After the terminal fix, `web` fails only `design-guards` › layer guard (NAV-4 T-1): `6e1ac7aa` 1 failed and 4,113
  passed; `ec10907a` 1 failed and 4,129 passed. CI stops there, so **FIX-281's browser walk hasn't run in CI.** I ran
  `e2e/skills.spec.ts` and `e2e/fix-277-reader.spec.ts` myself on `853fec1a` (mocked, through `test.sh`, on port 4197
  because another lane held 4100): **68 passed**.
- FIX-281 skipped and loosened nothing; its report lists the one older reader assertion it moved.

So I count **Tests as Match**: no Skills test fails anywhere it ran.

## Still open for the owner (from pass 1)

List rows show a skill's description as written, as P5-SKILLS-3 scope 1 and D-498(c) say. My registry and
`skill-1.png` show "Use it to …" in front of it. Counted as Match.

## Not checked in any pass

App skill rows (no app with skills was connected), a failing or paused library card, an over-budget library, and
import and templates carrying skills.
