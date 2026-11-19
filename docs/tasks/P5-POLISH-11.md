# P5-POLISH-11: Final review: compare every fixed screen with its mockup and send back what's off (owner, 2026-11-19)

## Goal

After P5-POLISH-1..10 are merged, walk the real app as the review did and compare each screen with its
"After" mockup. Check that UX review 4's problems are gone, and send back precise changes for whatever
still differs. The owner asked for this step explicitly: "review all the changes, compare them to the
mockups on the artifacts and send back any needed changes."

This brief is a review, not an implementation. Small fixes in an area nobody is editing may be made
directly (see Scope 5). Everything else goes back to the Orchestrator as a list of changes for the
owning brief's lane.

## Linked requirements

- **The reference (D-380):**
  - “Aurora UX fixes”, <https://claude.ai/artifact/WXbcpcrkbCLuxMSurpkuQW> (proposals A1..F3, the
    owner's comments applied);
  - “Aurora UX review 4”, <https://claude.ai/artifact/33oHaXQpCU3oSVRVmgtY5Z> (100 screens and every
    finding, tagged Broken / Confusing flow / Rough edge / Words);
  - the redesign canvas (D-354), <https://claude.ai/artifact/3UwmUgrhh6MLCsfqdhTFmQ>, for screens the
    fixes didn't touch.
- **Local copy** (workers can't open artifacts): `/home/alex/orca/workspaces/aurora/design-ux-fixes/`.
  - Read `README.txt`.
  - `after/<id>.png` and `after/src/<id>.html` are the targets, `before/<id>.png` today's screens.
  - `review/index.html` holds the review's findings, with screenshots in `review/img/` (each
    screenshot's name starts with the screen number).
  - The canvas copy is `/home/alex/orca/workspaces/aurora/design-canvas/`.
- **Spec:** `14 §Visual design` › "UX fixes" (the rules), D-380..D-393, C-1029..C-1036, and the briefs
  P5-POLISH-1..10 with their completion reports.

## Wave and order

- **Last.** Starts when P5-POLISH-1..10 are all merged to master.
- A fresh lane (Opus or Sonnet with a browser). It does not share a worktree with an implementation
  lane.
- **The loop:** the changes it sends back are implemented in the owning lanes. This review then
  re-checks only those screens, until nothing is left or the owner decides.

## Environment (read first)

- **Never use the shared `aurora-dev` kind cluster, `localhost:4000`, or the `aurora` database in
  `aurora-dev-pg`** (owner's standing rule). Bring up a **throwaway** cluster from your worktree with
  `deploy/dev/kind-dev.sh up --llm-mock --connector-fake`, using `AURORA_DEV_CLUSTER=<your-name>` and
  `AURORA_DEV_PORT=<free port>`.
  - Check `df -h /home` first: a cold cluster takes about 25 minutes and several GB.
  - Tear it down at the end (`kind-dev.sh down` with the same variables) and say so in the report.
- **Data to create**, as the review did:
  - run `kind-dev.sh bootstrap-admin alice@example.test`;
  - save the placement (shared organization "Users", members join as Member, a personal workspace
    each);
  - add an organization AI key on the LiteLLM mock;
  - sign in as Bob Builder and Carol Member through the development sign-in;
  - connect a Calendar link (ICS) account for Bob. The review used Google's public US holidays
    calendar, `https://calendar.google.com/calendar/ical/en.usa%23holiday%40group.v.calendar.google.com/public/basic.ics`;
  - add a second organization with several workspaces to see the switcher's multi-org case;
  - with no Google sign-in app registered, ask for Google Calendar as Bob (C-1030).
- **Browser:** use Orca's browser (`orca` CLI, the `orca-cli` skill) or Playwright, at 1440×900 and
  390×844, in **dark and light** themes. Capture full pages by scrolling and stitching, not with a tall
  viewport (tall viewports repeat tiles).
- Save every capture under `/home/alex/orca/workspaces/aurora/design-ux-fixes/verify/<id>-<theme>-<width>.png`.

## Scope

1. **Per proposal, compare with the mockup** (A1, A2, B1–B5, C1–C6, D1–D6, E1–E8, F1–F3):
   - Open the same screen with the same data the mockup shows. The mockups' sample texts are
     illustrative where the brief didn't quote them.
   - Compare against `after/<id>.png` for:
     - **layout:** order, grouping, columns, widths, alignment, and the 64 px header alignment in C4;
     - **components:** the right primitive (LockPill, StatusLine, Switch, LinkCard, EmptyState,
       Unavailable, AttentionChip);
     - **states:** empty, failing, asked, member versus owner, frozen;
     - **every interactive path** the proposal describes:
       - journey steps;
       - Ask and Undo;
       - Try it in chat with a draft;
       - the … menu;
       - the access select;
       - the switcher's search and organization drill-in;
       - the checklist toast;
       - Approve without expanding;
       - the apps table switch.
   - Check the **rules** of `14` › "UX fixes" on every page you visit, fixed or not:
     - no amber without an action;
     - at most one chip per card, in sentence case;
     - health lines only from real calls (cause a failure: give Bob's GitHub account a bad token and
       check that "Failed … : …" appears and stays until a success);
     - dialogs with three rows and no covered first line;
     - one focus ring;
     - no CSP violation or console error;
     - no horizontal scroll at 390 px;
     - both themes readable (contrast).
2. **Re-check UX review 4's findings.** Go through `review/index.html`. For each finding tagged Broken,
   Confusing flow or Rough edge, mark it **fixed**, **still there** or **changed** (a new problem in
   its place). Leave Words findings for the wording pass, but list any that a P5-POLISH screen
   reintroduced.
3. **Look for new problems** the fixes introduced: text cut off, unreadable buttons, overflow, broken
   links, mismatched labels, layout jumps, dead ends. These are the kinds of problems the owner asked
   to catch.
4. **The report** (in this brief's completion report and as a file
   `/home/alex/orca/workspaces/aurora/design-ux-fixes/verify/REPORT.md`):
   - a table with one row per proposal: ID · verdict (matches / minor differences / needs changes) ·
     screenshots (paths) · notes;
   - a list of **changes to send back**. Each change has:
     - the owning brief (P5-POLISH-n) and the file or component;
     - what is wrong, with the screenshot path and the mockup path;
     - what it should be, precisely: a measurable change such as "the footer's primary button sits at
       the right edge on the Sign in step (it is at the left)";
     - the severity: broken / flow / rough;
   - the re-check table of the review's findings.
5. **Small direct fixes.** A change of a few lines inside one component may be fixed in this lane
   (with tests) when no running lane owns that file. Typical examples are a wrong token, a missing
   `aria-label` or a spacing value. List each fix in the report. Everything else is sent back.
6. **Send back.** Message the Orchestrator (SendMessage `Orchestrator`) with the list of changes
   grouped by owning brief, so it can queue them in those lanes. The Orchestrator also tells the
   coordinator. After they merge, re-check those screens (repeat 1 and 4 for them only) and append a
   "Pass 2" section to the report.

## Out of scope

- The general wording pass, apart from listing regressions.
- New proposals. Ideas beyond the mockups go in a short "Further ideas" list at the end of the report,
  for the owner.

## Acceptance criteria

1. Every proposal A1..F3 has a verdict with dark and light screenshots at 1440 px, and at 390 px where
   the proposal has a phone view (B3, D4, and every full page).
2. Every Broken, Confusing flow and Rough edge finding of UX review 4 is marked fixed, still there or
   changed.
3. The changes to send back are precise and owned, and the Orchestrator has them.
4. The throwaway cluster was used, not `aurora-dev`, and was torn down.
5. After the sent-back changes are merged, a second pass confirms them. Nothing marked "needs changes"
   remains without an owner decision.

## Suggested agent/model and reason

Opus, or Sonnet (`sonnet-integration-reviewer`) with browser access. Judging against a design and
writing precise changes needs a careful eye. Read-only except for the small fixes in Scope 5.
