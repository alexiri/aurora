# P5-MEMORY-5: Verify the Memory work against the mockups; send back what differs; repeat until it all matches (owner, 2026-11-27)

## Goal

After P5-MEMORY-1..4 are merged, walk the real app and check **every Memory item of the Skills & Memory review**
against its After mockup and its proposal text. Whatever does not match goes back to the brief that owns it with a
change list precise enough to act on; the owning lane fixes it and this brief re-checks **only those items**. **Repeat
until every item is Match**, or the owner accepts a difference in writing.

The owner (2026-11-27): "Add a brief at the end of each part (memory and skills) to compare the work done with the
original mockups and order more work if needed."

This brief is a review. It does not implement features (small direct fixes: Scope 6).

## Linked requirements

- The design package `docs/design/skills-memory/` (D-496): `README.md` (precedence and how to read a mockup — binding),
  `review.md` §Memory, `after/mem-1..6.png` / `-light.png` / `.html`, `before/*.png`, `seed.py`.
- The briefs and their completion reports: `docs/tasks/P5-MEMORY-1.md` … `P5-MEMORY-4.md` (read each report's listed
  differences and reasons before checking; a difference not required by a brief or decision is still sent back).
- Decisions D-496, D-497; contracts C-1169..C-1172, C-1177; security SEC-157.

## Wave and order

Last of the memory set: starts when P5-MEMORY-1..4 are merged and CI is green. A fresh lane with a browser (Sonnet
`sonnet-integration-reviewer` with Playwright or Orca's browser), not an implementation lane's worktree. Pass 1 checks
everything; each later pass checks only the items sent back (plus a quick look at their neighbours). **Done only when a
pass ends with zero "Needs changes" and zero "Minor difference"** (apart from owner-accepted ones).

## Environment (read first)

- **Never** the shared `aurora-dev` cluster, `localhost:4000` or the `aurora` database in `aurora-dev-pg`. Bring up a
  throwaway cluster from your worktree: `AURORA_DEV_CLUSTER=<name> AURORA_DEV_PORT=<free port>
  deploy/dev/kind-dev.sh up --llm-mock` (check `df -h /home` first; a cold cluster takes about 25 minutes). Create the
  review's data as `docs/design/skills-memory/README.md` §Reproducing describes (`seed.py`), plus: one memory saved by
  the agent in Bob's chat (mock scenario `scn-memory-tool`), one refused by a lock, Carol as a co-owner who cannot read
  Bob's chat. Tear the cluster down at the end (`kind-dev.sh down`, same variables) and say so. Never prune podman
  volumes. Heavy commands through `scripts/dev/heavy.sh <lane> <command>`; tests only through `scripts/dev/test.sh`.
- Capture at **1256 px and 390 px**, **dark and light**, full pages by scrolling and stitching. Keep captures in your
  scratch folder; commit only the report and the captures of items that are not Match
  (`docs/design/skills-memory/verify/memory-pass<N>/<key>-<theme>-<width>.png`).
- On every page also record console errors, CSP violations, 4xx `/v1` responses, horizontal overflow at 390 px, axe
  wcag2a/aa.

## The registry (check every row)

| Row | What to check | Mockup / evidence | Owner |
|---|---|---|---|
| mem-1 | List: chips with counts, sort, source lines per actor kind, lock icon, first memory selected, no banner, no empty column; reading view: Markdown rendered, key chip with copy, Edit · Lock · "…" (History, Delete), meta line | `after/mem-1*.png` | P5-MEMORY-2 |
| mem-2 | Edit: Title, key chip, Description and hint, tall Write/Preview editor (the shared `MarkdownEditor`), size line, Locked switch with text, SaveBar; New: Key field from the title, editable, fixed after save, 409 on the field | `after/mem-2*.png` | P5-MEMORY-2 |
| mem-3 | Search "rotation": count line, Clear, highlighted snippet (no `<b>`), title highlight, first match open and scrolled, "Match 1 of 1" | `after/mem-3*.png` | P5-MEMORY-2 (+1) |
| mem-4 | History replaces the reading view; timeline with names, verbs, context ("through an API key", "in “…”"), Current; "What changed" with the shared `DiffView`, counts; Restore with undo; remove with reason | `after/mem-4*.png` | P5-MEMORY-3 |
| mem-5 | Success and refused cards, View, Undo (incl. "Changed since"), Save it yourself, Dismiss; "Failed" not "Deny" | `after/mem-5*.png` | P5-MEMORY-4 |
| mem-6 | Phone: list screen, memory screen with "‹ Memory", Edit, Lock, History; editor full width | `after/mem-6*.png` | P5-MEMORY-2 |
| M-1..M-10 | Each finding's problem is gone (re-do the Before screen of `before/M*.png`, `P0*.png`, `C01*.png`) | `review.md` | per proposal |
| D-497 | Provenance and last use correct per actor kind; a conversation never named to someone who may not read it (Carol); no e-mail anywhere | API + UI | P5-MEMORY-1/2/3 |
| SEC-157 | Its row in `docs/17` and its proving test exist and pass | `docs/17`, CI | P5-MEMORY-1 |
| Read-only | Emergency reader: reading view only, no search, no History, no edit | — | P5-MEMORY-2/3 |
| Tests | CI green per merge; no skipped or loosened test; no test still expecting retired words ("Lock it", "Technical details · Memory key", "Remove content of #", "Deny" for a failure) | CI, `rg` | each lane |
| Rules | `14` §Visual design on every memory screen: tokens only, shared primitives, both themes, no horizontal scroll at 390 px, the save pattern, the confirm ladder, `formatWhen` times | — | each lane |

## Scope

1. **Check every row:** open the same screen with the review's data; compare with the mockup (layout, components,
   exact interface words, states: empty, locked, untitled, member vs owner, read-only, failing) and with the brief's
   acceptance criteria; apply the precedence. Verdict per row: **Match**, **Minor difference** (spacing, one word, a
   missing detail) or **Needs changes** (missing, wrong, broken, a rule violated). Both of the last two go back.
2. **Evidence per row:** capture paths (dark/light, 1256/390), the mockup path, the verdict, and the change-list entry.
3. **The change list** (one entry per difference): the owning brief and file; the row and mockup path; **what differs**,
   measurably; **what it should be**, precisely; severity (broken / flow / rough / words).
4. **Look for regressions** around the changed screens (other Settings pages, the chat, export/import and templates
   that carry memories).
5. **Report** `docs/design/skills-memory/verify/MEMORY-REPORT-pass<N>.md`: counts per verdict, the registry with
   verdicts, the change list grouped by brief, regressions, "further ideas" (not blocking), the environment (cluster,
   commit checked, torn down).
6. **Small direct fixes** (a token, an `aria-label`, a spacing value) only where no running lane owns the file, with
   tests; list each in the report. Everything else goes back.
7. **Send back and iterate:** message the Orchestrator (`SendMessage` to `Orchestrator`) with the change list grouped by
   owning brief; it queues the work in those lanes (re-opening a brief's lane, or a FIX if the lane has closed). When
   it reports the fixes merged, re-check those rows and append "Pass N" to a new report. Owner-accepted differences are
   recorded "Accepted (owner, date, where)" and count as Match.

## Tests move with the UI (owner, 2026-11-20 — binding)

Any direct fix follows the rule: update every test that sees it in the same commit (`rg` the old text in `web/tests`,
`web/e2e`, `scripts/e2e`), never loosen, skip or delete a test, run through `scripts/dev/test.sh` only. The registry row
"Tests" checks the lanes did the same.

## Docs to update

The reports; PROGRESS (this row's status per pass) and FEATURES (`P5/14§memory-polish` done when a pass is clean).

## Suggested agent/model and reason

Sonnet `sonnet-integration-reviewer` with a browser (comparison and precise change lists; no implementation).

## Completion report

Passes run, verdict counts per pass, report paths, what was sent back and merged, owner-accepted differences, the
cluster torn down.
