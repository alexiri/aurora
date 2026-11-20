# P5-REFINE-20: Final verification: compare everything with the review, send back what differs, repeat until it all matches (owner, 2026-11-20)

## Goal

After P5-REFINE-1..19 are merged, walk the real app and check **every item of UX review 5** against its After
mockup and its fix text: all **119 findings**, the **10 priorities**, the **runner bug**, the **10 themes**,
the **16 layer-above recommendations** and the **15 owner decisions D-405..D-419**. Whatever does not match
goes back to the brief that owns it, with a change list precise enough to act on. The owning lane fixes it and
this brief re-checks **only those items**. **Repeat until every item is Match**, or the owner accepts a
difference in writing.

The owner's words (2026-11-20): "add a Brief at the end to verify all the changes are done correctly and they
match the mockups and recommendations faithfully. If they don't, they need to be sent back and iterated on
until they do." And: "make sure the briefs include links to the artifact/screenshots/mockups so they can
reproduce them reliably the first time."

This brief is a review. It does not implement features. Small fixes (Scope 7) are allowed only where no lane
owns the file.

Decisions that bind this brief: **D-405** (this set, its reference, the precedence **brief → owner's
decisions → mockup → finding text**, and this verify-and-send-back loop).

## Linked requirements

- **The review (source of truth, owner's comments applied):** <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>.
  Section anchors: `#top` (priorities), `#bug`, `#themes`, `#above` (layer above, recs 1–16), `#blind`; a
  finding is `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#f-<key>`.
- **Local copy** (workers cannot open artifacts): `/home/alex/orca/workspaces/aurora/design-ux-review-5/`
  (`UXR5/` below). Read in this order: `findings.md` (119 findings: problem, fix, After caption, designer's
  note, pointers; authoritative text), `priorities-themes.md` (10 priorities, the bug, 10 themes, blind
  spots), `layer-above.md` (recs 1–16, owner's decisions written in), then the images:
  - `UXR5/after/<key>.png` (dark) and `UXR5/after/<key>-lt.png` (light): **the visual target**;
  - `UXR5/after/src/<key>.html`: the mockup markup (kit classes of the P5-UX canvas; the PNG wins if they
    differ); yellow dashed sticky notes in a mockup are commentary, not UI;
  - `UXR5/before/<key>-b<n>.png`: the Before crops; `UXR5/screens/<id>.png`: the 511 full screenshots;
    `UXR5/notes/{A,B,C}.md`: the walkers' notes; `UXR5/review.html`: the whole page for a human.
- **The briefs and their completion reports:** `docs/tasks/P5-REFINE-1.md` … `P5-REFINE-19.md`. Each ends with
  a **Completion report** (items done with commit, deviations, screenshots compared, open questions). Read
  each report before checking its items; a deviation the report explains is still checked against the
  precedence rule, and anything that contradicts the brief, a decision or the mockup goes back.
- **Decisions D-405..D-419** are in `docs/90-decisions.md` (and summarized in the table below). **Contracts**
  C-1056..C-1085, SEC-140..SEC-142.
- **Previous review's re-check:** `/home/alex/orca/workspaces/aurora/design-ux-fixes/verify/REPORT.md` and its
  findings list (UX review 4). Scope 5 checks nothing regressed.

## Wave and order

- **Last.** Starts when P5-REFINE-1..19 are all merged to master and CI is green. A fresh lane (Opus, or
  Sonnet `sonnet-integration-reviewer` with a browser); it does not share a worktree with an implementation
  lane.
- **The loop:** pass 1 checks everything; each later pass checks only the items sent back in the previous
  pass (plus a quick smoke of the neighbours of each fix). The brief is **done only when a pass ends with
  zero "Needs changes" and zero "Minor difference"**.

## Environment (read first)

- **Never use the shared `aurora-dev` kind cluster, `localhost:4000`, or the `aurora` database in
  `aurora-dev-pg`** (owner's standing rule). Bring up a **throwaway** cluster from your worktree:
  `AURORA_DEV_CLUSTER=<your-name> AURORA_DEV_PORT=<free port> deploy/dev/kind-dev.sh up --llm-mock
  --connector-fake --ssh` (add the flags the script documents for runners and the terminal; the review
  needed the terminal, Files and SSH on). Check `df -h /home` first (a cold cluster takes about 25 minutes
  and several GB). Tear it down at the end with `kind-dev.sh down` and the same variables, and say so in the
  report. Never prune podman volumes.
- Heavy commands run as `/home/alex/orca/workspaces/aurora/heavy.sh <lane> <command>` (one at a time).
- **Data to create, as the review did:**
  - `kind-dev.sh bootstrap-admin alice@example.test`; save the placement (shared organization "Users", members
    join as Member, a personal workspace each); add the organization AI key on the LiteLLM mock; sign in as
    Bob Builder and Carol Member through the development sign-in; a second organization with several
    workspaces; Bob's Calendar link (ICS) account (Google's public US holidays calendar
    `https://calendar.google.com/calendar/ical/en.usa%23holiday%40group.v.calendar.google.com/public/basic.ics`);
    an app with a level select (for access-1/2); a schedule, a webhook (with a local receiver to call it with
    the shown `curl`), a chat platform host (fake connector); a skill, a memory, a test question; a file made
    on the computer; an SSH key.
  - States the review needed: a freshly installed instance (setup checklist), an instance with **no starter
    key**, an agent created **before** the starter key existed, an unknown organization slug (`/o/new`), a
    failing account (bad token), the terminal turned on **after** the runner registered (the runner bug,
    `#bug`: use `helm upgrade`-equivalent: toggle the setting then check Terminal, Files and SSH without
    restarting the runner).
- **Browser:** Orca's browser (`orca` CLI, the `orca-cli` skill) or Playwright, at **1256 px** and **390 px**,
  in **dark and light**. Capture full pages by scrolling and stitching, not with a tall viewport. Save every
  capture under `/home/alex/orca/workspaces/aurora/design-ux-review-5/verify/pass<N>/<key>-<theme>-<width>.png`
  (other items: `<kind>-<id>-<theme>-<width>.png`, e.g. `rec-3-dark-1256.png`). Put the mockup path beside it
  in the report.
- On every capture also record: console errors, CSP violations (`securitypolicyviolation`), 4xx `/v1`
  responses, horizontal overflow at 390 px, axe wcag2a/aa.

## Scope

1. **Check every item against its mockup and fix text.** For each row of the registry below:
   - open the same screen with the same data the mockup shows (sample texts in a mockup are illustrative where
     its brief did not quote them);
   - compare with `UXR5/after/<key>.png` and `-lt.png`: **layout** (order, grouping, columns, widths,
     alignment), **components** (the shared primitive, not a one-off), **words** (exact text where the
     mockup or the brief quotes it), **states** (empty, loading, failing, member versus owner, paused),
     **every interactive path** the finding describes;
   - check the **fix text** in `UXR5/findings.md` and the **owning brief's acceptance criteria**;
   - apply the **precedence** (D-405): the brief, then the owner's decisions and artifact comments, then the
     mockup, then the finding's fix text. A difference that the brief or a decision requires is a Match.
   - Give one verdict: **Match**, **Minor difference** (looks or reads slightly different: spacing, one word,
     a missing detail), **Needs changes** (missing, wrong, broken, or a rule violated). Both of the last two
     go back.
2. **Per row, evidence:** screenshot paths (dark and light, 1256 and 390 where the mockup has them), the
   mockup path, the verdict, and for anything not Match the change list entry (Scope 4).
3. **The rules of `14` §Visual design on every page you visit,** fixed or not: tokens only, shared
   primitives, both themes, no horizontal scroll at 390 px, no amber without an action, at most one chip per
   card, the save pattern, the confirm ladder, `formatWhen` times, sentence first (no snake_case, UUID or
   dotted key outside Technical details), one focus ring, no CSP violation or console error, dialogs
   top-anchored.
4. **The change list** (what "send back" means). One entry per difference:
   - the owning brief (P5-REFINE-n) and the file or component;
   - the item key and mockup path, and the screenshot path;
   - **what differs**, measurable ("the Save bar is at the top of the page; the mockup has it fixed at the
     bottom", "the button reads 'Tighten…' but the table says 'Restrict further…'");
   - **what it should be**, precisely;
   - severity: broken / flow / rough / words.
5. **Re-check UX review 4 and look for regressions.** Go through `design-ux-fixes/verify/REPORT.md` and its
   review findings tagged Broken, Confusing flow or Rough edge: mark each **still fixed** or **regressed**.
   Look for new problems the brief set introduced: text cut off, overflow, broken links, mismatched labels,
   layout jumps, dead ends, a retired word, a Use-altitude page showing an Expert control, a page that
   needs the switch to be usable.
6. **The report** at `UXR5/verify/REPORT-<pass>.md` (`pass1`, `pass2`, …), committed with its screenshots in
   the lane's worktree and referenced from this brief's completion report. Structure:
   - a summary (counts of Match / Minor difference / Needs changes per registry section, and the items new
     in this pass);
   - the full registry table with a verdict per row (pass 1) or the re-checked rows (later passes);
   - the **change list**, grouped by owning brief;
   - the UX review 4 re-check; "Further ideas" (not blocking);
   - the environment (cluster name and port, commit checked, torn down: yes/no).
7. **Small direct fixes.** A few lines in one component may be fixed here (with tests) when **no running lane
   owns that file**: typical examples are a wrong token, a missing `aria-label`, a spacing value. List each in
   the report. Everything else is sent back.
8. **Send back and iterate.** Message the Orchestrator (SendMessage `Orchestrator`) with the change list
   grouped by owning brief; it queues them in those lanes and tells the coordinator. When a lane reports a
   fix merged, re-check **those items only** (repeat Scope 1–4 for them) and append "Pass N" to the report.
   Items the owner accepts in writing are recorded as "Accepted (owner, date, where)" and count as Match.
   **Do not mark the brief done until a pass ends with zero Needs changes and zero Minor difference** (apart
   from accepted ones).

## The registry (check every row)

Owner = the brief that owns the item (UI brief / backend brief where both). A key that appears in two briefs
is checked against both briefs' acceptance criteria. Every finding's mockup is
`UXR5/after/<key>.png` and `UXR5/after/<key>-lt.png`; its text is in `UXR5/findings.md`.

### A. The 119 findings

| Area | Keys (owner brief) |
|---|---|
| **setup** (First run as administrator) | setup-1 (5 / 18), setup-2 (5 / 18), setup-3 (18), setup-4 (5 / 18), setup-5 (8 / 18), setup-6 (18), setup-7 (2 / 18), setup-8 (8 / 18) |
| **firstday** (A person's first day) | firstday-1 (1), firstday-2 (16), firstday-3 (16), firstday-4 (16), firstday-5 (16) |
| **chat** | chat-1 (7 / 11), chat-2 (11), chat-3 (11), chat-4 (7 / 11), chat-5 (11), chat-6 (11), chat-7 (7 / 11), chat-8 (11), chat-9 (11), chat-10 (11), chat-11 (11) |
| **behaviour** | behaviour-1 (12), behaviour-2 (12), behaviour-3 (12), behaviour-4 (12), behaviour-5 (12), behaviour-6 (2 / 12), behaviour-7 (12), behaviour-8 (12) |
| **access** | access-1 (6 / 13), access-2 (13), access-3 (5 / 12), access-4 (13), access-5 (13), access-6 (13), access-7 (13), access-8 (5 / 13), access-9 (13), access-10 (13), access-11 (13), access-12 (13) |
| **schedules** | schedules-1 (14), schedules-2 (14), schedules-3 (7 / 14), schedules-4 (1 / 7 / 14), schedules-5 (14), schedules-6 (14), schedules-7 (7 / 14) |
| **webhooks** | webhooks-1 (14), webhooks-2 (1 / 7 / 14), webhooks-3 (7 / 14), webhooks-4 (7 / 14), webhooks-5 (2 / 14), webhooks-6 (14) |
| **channels** | channels-1 (10 / 12), channels-2 (5 / 12), channels-3 (1 / 8 / 14), channels-4 (12) |
| **computer** | computer-1 (4 / 15), computer-2 (4 / 15), computer-3 (15), computer-4 (15), computer-5 (2 / 15), computer-6 (4 / 15), computer-7 (15), computer-8 (15), computer-9 (15), computer-10 (15), computer-11 (15), computer-12 (15), computer-13 (4 / 15) |
| **workspace** | workspace-1 (16), workspace-2 (16), workspace-3 (16), workspace-4 (16), workspace-5 (16), workspace-6 (16), workspace-7 (2 / 16), workspace-8 (2 / 16), workspace-9 (16), workspace-10 (16), workspace-11 (16), workspace-12 (2 / 16) |
| **org** | org-1 (6 / 17), org-2 (17), org-3 (2), org-4 (6 / 17), org-5 (17), org-6 (17), org-7 (17), org-8 (17), org-9 (17), org-10 (6 / 17), org-11 (17), org-12 (17), org-13 (2 / 17), org-14 (17) |
| **admin** | admin-1 (5 / 18), admin-2 (2 / 18), admin-3 (18), admin-4 (18), admin-5 (6 / 18), admin-6 (18), admin-7 (18), admin-8 (18), admin-9 (18), admin-10 (18), admin-11 (2 / 18), admin-12 (6 / 18), admin-13 (18), admin-14 (6 / 18) |
| **account** | account-1 (16), account-2 (16), account-3 (16), account-4 (16), account-5 (16) |

That is 8 + 5 + 11 + 8 + 12 + 7 + 6 + 4 + 13 + 12 + 14 + 14 + 5 = **119** keys. The report's table has one row
per key, in this order. Where a finding names a decision (access-1 → D-419, chat-1/4/7 → D-409, setup-2 →
D-414, computer-* → D-412, and so on), check the decision's wording, not only the mockup.

### B. The 10 priorities (`UXR5/priorities-themes.md`, anchor `#top`)

| # | Priority | Owner briefs | Check |
|---|---|---|---|
| 1 | The eleven broken things | 4, 5, 6, 7, 11, 13, 17, 18, 2 | each of: access-1 (Read only on an outgoing webhook; D-419), chat-1 (Archive leaves Active), setup-1 (Try it completes), access-3 (new agent gets the key), org-1 (Mattermost readiness), admin-1 (failed save not persisted), org-2 (Org Apps › Set up changes the tab), org-3 (`/o/new`), access-2 (phone Access row), chat-2 (sending scrolls to the message), computer-1/2 plus the runner bug (Terminal, Files, SSH after enabling the terminal) |
| 2 | One way to save, always say "Saved" | 2 (+ every adopting brief) | every settings form: bar on first change, Saved 2 s, "n fields need fixing", instant controls inline Saved, no page-top banners |
| 3 | Every action says what happened and links to the result | 7, 11, 14, 15 | Run now, Runs outcomes, webhook test results, Stop mark, eval progress, a request that resolves leaves a trace |
| 4 | Speak people's language; machinery behind Technical details | 2 (labels, guard), 1, all area briefs | `copy-guard` exemption list is **empty**; no snake_case/UUID/dotted key outside Technical details on any page walked |
| 5 | Incoming webhooks and schedules usable without a manual | 14 (+7) | presets, Every 5 minutes minimum, next three runs in the workspace's zone, Signed default + Generate, How to call it with curl/Python/Node, Deliveries table |
| 6 | "Ask an admin" becomes a real request list | 8, 16, 17, 18 | one feed shape per level, reasons, decline message, history (30 days), the asker sees their own, counts in the rail of whoever can act (D-417) |
| 7 | People by name, admins by role | 1, 5, 16, 17 | display name from the `name` claim, one personal-workspace name everywhere, no "Ask alice", no slug typed to delete |
| 8 | Let people start a new chat | 7, 11 | several direct chats, New chat, Rename, auto-title, one thread per run folded under the automation (D-409) |
| 9 | Invitations, organizations and roles without dead ends | 6, 17, 2 | Copy invitation link (D-418), New organization in the switcher, promotion confirmation, rotate warning |
| 10 | A newcomer's agent just works, says why when it doesn't | 5, 12, 11 | starter key on every new hosted agent and retroactively (D-414), "Can't answer: no AI key" + one button, New agent defaults to "Runs on Aurora" |

### C. The runner bug (`#bug`)

Owner brief 4 (+15 for the dashboard's retry text). Check: turn the terminal on **after** the runner has
registered (no restart); Terminal, Files and SSH work within the settings-refresh interval; the regression
test exists; after the last retry the dashboard says "Couldn't reach the computer. Try again · Report it" with
no blinking cursor; an open terminal or SSH session keeps the computer awake (D-416); a restart prints a notice
in the terminal.

### D. The 10 themes (`#themes`)

| Theme | Owner briefs | Check |
|---|---|---|
| Stale live state | 11, 4, 2, 5 | "Working" clears after the answer and Stop; "Just now" ticks every 30 s; Try it completes; Awake time is right; the start log leaves `waiting_supervisor` |
| Several names for one thing | 1 | every row of the vocabulary table (rec 2 + rec 15), including Pause/Resume for organizations, glossary in `docs/14`, extended words test, empty exemption list |
| Tables that don't fit | 2 | Activity, Audit (both), Deliveries, People, invitations, Users, Files stack under 900 px; no inner horizontal scroll at 390 px |
| One-time secrets | 2 (+14, 13, 15) | `SecretOnce` everywhere a secret shows once: Copy → Copied, close-before-copy warning, mono box that never wraps |
| Five date formats | 2 | `formatWhen` / `<When>` everywhere, ticks, locale date inputs |
| Destructive and privileged actions | 2 (+14, 16, 17) | the three-step ladder; visible names typed; promote and rotate confirm with consequence text |
| Contrast and colour meaning | 2 (+18) | logo plates in dark, row-action contrast, red Critical, red failure / amber action / green good news |
| Context gets lost | 2, 15 (+17) | 404 inside the shell, Account keeps the last scope, "Back to <Agent>" |
| Nothing shows what the agent did on its computer | 9, 11, 15, 16 | commands and files appear in the timeline and the rail, with a link into Files |
| Pages and dialogs jump | 2 | dialogs top-anchored, reserved error space, skeleton rows, the status panel overlays below 1400 px |

### E. The layer above: recommendations 1–16 (`UXR5/layer-above.md`, anchor `#above`)

| Rec | Title | Owner briefs | Check |
|---|---|---|---|
| 1 | Three altitudes, one switch | 3 (+ every area brief wrapping Expert controls) | switch off/on per person, server-stored, role floor, "n more controls hidden · Show", classification table = registry = `docs/14` |
| 2 | Four nouns, met in order; vocabulary | 1 (+11, 12, 16 for the order of introduction) | every vocabulary row; Welcome and tour follow minute 1 / 5 / 6 / first week |
| 3 | The agent page: Chat with a labelled Settings, a rail that says what it means | 11 (+4 for readiness) | tabs Chat · Automations · Computer, Settings button with sub-nav, status follows the computer's readiness (D-408), rail with labels and headings Needs you · Doing now · What <Agent> did |
| 4 | One timeline per agent | 9 (+11, 13, 16) | live timeline in sentences, three parts (can do / today / did), rail, Technical view, "3 new · Show" |
| 5 | No blank forms: Aurora decides, the person confirms | 12 (+5, 14, 18) | purpose templates (D-413), defaults make forms one-line confirmations |
| 6 | Policy presets | 18 (+17) | Open / Balanced / Locked down above the forms, Open default, "Custom · based on Open" (D-410) |
| 7 | One "Needs you", everywhere | 8, 16, 17, 18 | the same row shape at workspace, organization and Administration |
| 8 | One daily rail; "Manage Acme" and Administration as places | 17 (+18) | "Manage Acme" with sub-nav and back link; org home as an overview |
| 9 | Administration in two halves | 18 | "Policy for everyone" / "This installation" + one "Set by the installation" page |
| 10 | Teach approvals before the first one; first successes | 16, 11 | approvals example card; first answer, first app, first approval, first run cards |
| 11 | The computer is a place with the person's files | 15 (+4) | Files first, the agent's own files grouped, phases in words, Technical details admin-only (D-412) |
| 12 | Automations: one list | 14 (+7) | one "Automations" list, runs link to their thread, Peers moved to Settings › Other agents |
| 13 | A copy system with teeth | 2 (+1, all) | `lib/labels.ts`, `copy-guard`, sentence first, one verb per thing, errors with what/what-you-can-do/Details |
| 14 | Expert speed | 19 | palette, shortcuts, bulk, export/import, copy as curl, CLI fold; all behind the switch except palette nouns |
| 15 | One ladder for "stop it" | 1 (+2) | Pause/Resume, Turn off/on, Suspend/Reinstate, Remove from, Rotate/Promote, Delete — one word and one confirmation style per rung |
| 16 | Blind spots | 1, 3, 11, 12, 16, 17, 18 | each bullet: Evals/Tests Expert; Previews and Connect only when they apply; Remove this message's content…; Language remembered in (Expert); What moved gone; Approver fold open on Add people; own-send approvals use the Needs-you row; access-request card in the tour; global banners follow D-384; Groups and Directory sync Expert unless configured; harness display names and `fake` hidden; "Came with this workspace" |

### F. The owner's decisions D-405..D-419

| Decision | Wording to check | Owner briefs |
|---|---|---|
| D-405 | this set, its reference, precedence, the verify loop | 20 (this brief) |
| D-406 | altitudes Use / Set up / Expert; one per-person "Show more controls" stored server-side like `welcome_seen`; role is the floor | 3, 19 |
| D-407 | all renames together; Pause/Resume replaces Freeze everywhere incl. organizations; admins named by role and the request goes to all of them; display names from the `name` claim; "What moved" removed | 1, 5, 8 |
| D-408 | a labelled Settings button replaces the "…"; tabs Chat · Automations · Computer; header status follows the computer's readiness; Terminal/Files show the same line and connect by themselves | 11, 4, 15 |
| D-409 | several direct chats per person and agent (New chat, Rename, auto-title); one thread per run folded under its automation and linked from Runs/Deliveries; search scope covers them | 7, 11, 14 |
| D-410 | presets Open / Balanced / Locked down; Open default on a fresh install; "Custom · based on Open" | 18, 17 |
| D-411 | people create agent links; "Link an agent" picker; create route | 10, 12 |
| D-412 | members see computer phases in words; image and runner address instance-admin only | 4, 15 |
| D-413 | purpose templates in the web bundle (`web/src/lib/onboarding/templates.ts`), no API, not org-editable | 12, 19 |
| D-414 | starter keys apply to every new hosted agent and to existing ones when added ("Given to Daneel and 1 other agent"); no-starter-key behaviour (no Try it; step 3 text; "<Agent> has no AI model yet" with Add a key / Ask an admin) | 5, 12, 18 |
| D-415 | one live per-agent timeline, feeds the rail and a Technical view, pushed over the chat stream, in-progress rows change in place, "3 new · Show" | 9, 11, 16 |
| D-416 | runner re-decides its stream listener on re-registration; an open terminal or SSH session keeps the computer awake; a restart prints a notice | 4, 15 |
| D-417 | one feed shape per level with reasons, decline message, history (done/declined kept 30 days), the asker sees their own, rail count for whoever can act | 8, 16, 17, 18 |
| D-418 | org owners/admins can copy an invitation link in the UI (SEC-140) | 6, 17 |
| D-419 | send-only apps have their own levels "Ask before each send" / "Send without asking"; Read only never offered | 6, 13 |

Also check the two other owner comments carried into the briefs: workspace Apps is a **grid of square cards**
with search and the two groups (workspace-9); schedule presets start at **Every 5 minutes** with the hint
"Schedules run at most every 5 minutes" and a custom cron that flags too-frequent as you type (schedules-2).

## Out of scope

- New proposals. Ideas beyond the mockups go in a short "Further ideas" list in the report, for the owner.
- Implementing anything that is a lane's job (Scope 7 is the only exception).
- Performance budgets (the perf-only-when-idle rule): note "not measured" where relevant.

## Acceptance criteria

1. Pass 1 has a verdict, screenshots (dark and light at 1256 px, and at 390 px where the mockup has a phone
   view) and a mockup path for **every** one of the 119 findings, the 10 priorities, the bug, the 10 themes,
   the 16 recs and D-405..D-419.
2. Every Needs-changes and Minor-difference item has a change list entry with the owning brief, the
   measurable difference and the target, and the Orchestrator has the list (a message sent, quoted in the
   report).
3. Each later pass re-checks exactly the items sent back and records their new verdicts.
4. The **last pass ends with zero Needs changes and zero Minor difference** (owner-accepted differences are
   listed as Accepted with date and place).
5. UX review 4's Broken/Confusing/Rough items are marked still fixed or regressed, with regressions sent
   back.
6. The `copy-guard` and vocabulary exemption lists are empty (`VOCABULARY_EXEMPT`, `OTHER_EXEMPT`, and the
   guard's record).
7. The throwaway cluster was used, never `aurora-dev`, and was torn down (stated in the report).
8. Each pass's report and screenshots are committed in the lane's worktree and handed to the Orchestrator to
   merge (no push).

## Tests

No new product tests. For a direct fix (Scope 7), add the targeted vitest or mocked Playwright spec for it.
Run heavy commands only through `/home/alex/orca/workspaces/aurora/heavy.sh`. GitHub CI is the full gate.

## Docs to update

- `docs/PROGRESS.md` and `docs/FEATURES.md`: P5-REFINE-20 status per pass; the final "all Match" line with the
  report path.

## Suggested agent/model and reason

Opus, or Sonnet (`sonnet-integration-reviewer`) with browser access. Judging against a design and writing
precise, measurable change lists needs a careful eye; the volume (about 170 rows) suggests splitting the
walk across two reviewers by area but keeping one report and one send-back list.

## Completion report

Fill in per pass (the owner reads this):

- **Pass N** (date, commit checked, cluster name/port):
  - Counts: Match / Minor difference / Needs changes / Accepted:
  - Report path: `UXR5/verify/REPORT-pass<N>.md`
  - Change list sent (message to the Orchestrator, time) and the briefs it went to:
  - Fixed in this lane (Scope 7): file → what:
  - UX review 4 regressions found:
- **Final pass:** zero Needs changes and zero Minor difference: yes/no; Accepted items (owner, date, where):
- **Cluster torn down:** yes/no
- **Further ideas:**
