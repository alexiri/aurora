# Skills verification, pass 2 (P5-SKILLS-5)

Date: 2026-11-28. Reviewer lane, no product code changed (no direct fixes made).

## Result

**Not quite clean: 27 rows, 26 Match, 1 Minor difference.** Everything sent back in pass 1 (C-1 to C-7 and B-1) is
**fixed on a real stack with the real `anthropics/skills` repository**. The one Minor is new: it is the emergency
reader's skill list, which pass 1 couldn't reach. The "Tests" row is Match **only once CI on the migration fix is
green** (see below).

## Environment

- Commit checked: `406605f6` (local master with `a1a59836`: FIX-278 and FIX-279).
- `aurora-mem5` (:4530) redeployed from it with `kind-dev.sh up --llm-mock --ssh` through `heavy.sh`. Migration
  `20261201000292` applied; **no skill library existed before this pass**. **Still up** for P5-NAV-4 pass 2.
  `aurora-dev` and `aurora-skm` untouched; no volume pruned. No database edit and no scratch mock this time.
  - A tooling note: this `up` loaded the new images but left the old Pods running (nothing in the chart had changed),
    so I restarted every deployment before testing.
- Data: the seeded skills from earlier passes, Giskard as a scratch agent for the add test. "Anthropic skills" added
  through the interface at `e4bb5d9`; `brand-guidelines` added to Daneel and `internal-comms` to Acme through the
  dialog; `webapp-testing` added to Bob's workspace (API) and a file added to its `meeting-minutes`.
- Playwright, 1256 and 390 px, dark and light on the re-checked pages; some interaction probes at one width or theme,
  some without axe. No axe, console or CSP finding. The only sideways scroll boxes: code blocks inside rendered
  instructions, and the agent page's pill row (by design). No 4xx on the pages; the 422s in the add test were the
  expected refusals.

## Rows re-checked

| Row | Pass 1 | Pass 2 | Evidence |
|---|---|---|---|
| skill-5, S-10 (C-1) | Needs changes | **Match** | **Every one of the repository's 19 folders**: listing verdict, preview and a real add (to Giskard) agree, **0 mismatches**. The 5 "addable" ones added (201) with exactly the listed sizes (13,259 · 15,342 · 19,249 · 21,981 · 22,055); the other 14 were refused (422). The budget after adding all five (91,886) equals the sum. Check dialog "5 skills can be added, 14 can't", card "5 skills", fold "14 can't be added". The dialog opens on the first addable skill with **Add** on; once it was added, on the next one. Rows read "2 files · 13 KB". Check took about 2.5 s |
| skill-5 (C-2) | words | **Match** | All 14 reasons are sentences ("This skill is larger than 32,000 characters, the most a skill may hold.", …). Folded rows can't be picked |
| skill-1 (C-3) | Minor | Fixed | Descriptions clamped to two lines; rows 61–93 px (were up to 157), agent and organization lists |
| skill-1, skill-3 (C-4) | Minor | Fixed | With two real proposals pending: Bob's agent page says 1 and marks 1 (Brand guidelines; Acme's Internal comms has no chip and no update box for him); Acme's page says 1 and marks 1; the workspace page says none and marks none |
| skill-1, Rules (C-5) | Minor | **Match** | At 390 px neither **Compare** nor the **update review** has a sideways scroll box; wrapped lines hang under their text, "Show n more lines" buttons are whole |
| skill-6 (C-6) | Minor | **Match** | A library with no branch set reads **Branch main**; one set to a commit shows `ef74077`, shortened |
| S-11 (C-7) | Minor | **Match** | After Check the suggested label is **anthropics/skills** (all four width/theme settings) |
| skill-1 | — | **Minor difference** (D-1) | The emergency reader's list. Captures `skills-pass2/skill-1-reader-*` |
| Tests | Match, note | Match, pending CI | No `.skip`, `.fixme`, `.only` or `#[ignore]` added from `99c41755` to `a1a59836` and no test file removed. "Use it when:" is gone from `web/src`; "Default branch" remains only as the fallback when the server has no name, and for template sources. See CI below |

The other rows were Match in pass 1. I saw their neighbours again where these screens overlap (the list, panels, the
dialog, Add to an agent and to Acme, the cards) and nothing changed their verdict. I didn't repeat pass 1's per-agent
off, usage or editor checks.

## B-1: libraries that change their source (broken in pass 1) — fixed, both directions, through the interface

1. **To another branch.** Edit › Branch or tag = `ef740771…`, Save. Both proposals (`brand-guidelines`,
   `internal-comms`) appeared within seconds, with no "Check now" and no database help; the card then showed **Update
   available**, and Review update `e4bb5d9 → ef74077`.
2. **Back.** Clearing the field and saving returned the library to the default branch (`main`). **Both proposals were
   gone with the save**: 0 pending, both rows `superseded`.
3. **Approving a dropped proposal** with its real content hash: **409 `update_superseded`** for both; each skill stayed
   at `e4bb5d9`, unchanged.
4. A later **Check now** (head `e4bb5d9`) proposed nothing.
5. The `skill_library.update` audit row of the way back lists the two dropped proposals.

Not exercised: the approval race, a change of folder or address, and a paused or instance library (FIX-278's tests
cover them).

## The emergency reader's Skills pages (FIX-280)

An open grant, 1256 and 390 px, both themes. Panels walked: the agent's Triage alerts, Brand guidelines (a library
skill) and Weekly status report; the workspace's Meeting minutes (with a file) and Webapp testing (a library skill with
six files). **Files are listed and none can be opened** ("Emergency read access doesn’t cover a skill’s files.");
switches are disabled; the only controls are the banner's **End now**, the page tabs, search, chips and rows, and a code
block's Copy. **No 403, no other 4xx, and no request that wasn't a GET.** No update notice or chip.

## Change list

### P5-SKILLS-3 (the list), with FIX-277's reader view

**D-1 · words · row skill-1 (emergency state)**

- Differs: on the agent's Skills page the reader sees only the agent's own skills (the grant doesn't cover the effective
  set, as built), but the page still offers **From the workspace 0** and **From Acme 0** and a column **For Daneel**.
  That reads as "no workspace or Acme skill applies to this agent". For the same agent, Bob sees seven that do.
- Should be: for a reader, no chips for sources the page can't read, and one line that says so, for example "Emergency
  read access shows only Daneel’s own skills." Don't widen what the server allows.
- Captures: `skills-pass2/skill-1-reader-*`.

## CI

- `a1a59836` (the FIX-278 push, run 46084086228) is **red**: `web` fails only `e2e/terminal.spec.ts` (NAV-4's C-1,
  not Skills); `rust-test (3)` failed `skill_titles_uses_migration` (P5-SKILLS-1's test, broken by FIX-278's migration
  `000292`); `images-builder` was cancelled.
- The migration test is fixed on master (`246a5fa9`, in `9ca42d59`). Its CI was still running when I wrote this.

## Still open for the owner (from pass 1)

My registry says list rows show "“Use it to …” sentences"; P5-SKILLS-3 scope 1 and D-498(c) say the description as
written, which is what is built. Counted as Match.

## Further ideas (not counted)

- A skill whose **description is too long** now reads "Its SKILL.md is missing, misnamed or not written the way a skill
  needs." (`academy-guide`, `discernment-nudge`). It is true as far as it goes, but the old raw text named the limit; a
  reason code of its own would let the sentence say so.
- Right after saving a changed branch, the card can still read "Up to date" for a moment, while the two proposals are
  being made.

## Not checked

- App skill rows (no app with skills is connected), a failing or paused library card, an over-budget library, import
  and templates — as in pass 1.

## For pass 3

Re-check D-1 (skill-1) with its neighbours, and the Tests row against CI.
