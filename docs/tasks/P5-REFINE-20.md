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

### G. Count badges in the navigation (owner, 2026-11-20)

The owner: "make sure the numerical badges of pending tasks in the menu … actually work, I think some were supposed
to be implemented already and I don't see them working." Check **every** count badge end to end, on a real stack, as
a person would meet them. A badge that is missing, wrong, stale or shown to someone who cannot act is **Needs changes**
and goes back to its owner brief like any other row.

**The badges, what each counts, and who owns it:**

| # | Where | Counts | Shown to | Today | Owner brief |
|---|---|---|---|---|---|
| G1 | Workspace rail › **Approvals** | pending approvals in this workspace the viewer can decide | owners, approvers | `PendingPill` (`components/app-shell.tsx` ~:219) reads `usePendingApprovals` (`lib/api/approvals.ts:80`): **no refetch interval and no stream outside the Approvals page**, so it is likely stale; RFE-012 | 16 |
| G2 | Workspace rail › **Overview** (and the Overview's Needs you heading) | the workspace's attention items (C-1080/C-1082: approvals, requests, broken accounts, agents that can't answer…) | whoever can act on at least one | none | 16 (data: 8) |
| G3 | Organization area (Manage <org>) entry and its Needs you | the organization's attention items, incl. requests to set up an app (P5-POLISH-4, D-386) | org owners and admins | none in the rail (requests show only on workspace Overviews) | 17 (data: 8) |
| G4 | **Administration**: the sidebar-foot entry, the account-menu entry, Administration › Overview in its rail | instance attention (FIX-167, D-370, C-1022) plus requests only an instance admin can resolve (chat platforms; D-417) | instance admins | `AttentionBadge` (`components/admin/attention-badge.tsx`) is mounted **only in the account menu** (`app-shell.tsx` ~:155), not on the sidebar-foot Administration entry FIX-167 described; people's requests are not counted | 18 (data: 8) |
| G5 | Agent page: the status rail's **Needs you** (collapsed: a count on its icon) and the agent's row in the Agents list | this agent's approvals and access requests waiting for the viewer | those who can decide | a dot, no count | 11 |
| G6 | Phone: the navigation drawer shows the same counts; the hamburger button shows a dot when anything waits | as G1–G4 | as above | not checked in review 5 | 16 (17, 18 for their entries) |

**How to check each badge (every row, both themes, 1256 and 390 px):**
1. **Zero:** with nothing waiting, no badge (never "0").
2. **Appears:** create the item it counts — an approval (use the LLM mock's tool-calling scenario so an agent asks
   before a change, e.g. an "Ask before changes" app on the agent; if the mock cannot produce one, create a pending
   approval through the API/fixture and say how in the report), an ask to an admin (Bob asks for Jira; Carol asks for a
   chat platform), a broken account, an agent with no AI key. The badge appears **without a reload** within the
   feed's live or polling interval (state the measured delay), with the right number.
3. **Matches its list:** the number equals the items the badge's page lists for that viewer.
4. **Goes down:** decide the approval / resolve or decline the request in another tab or as another person; the
   badge decreases and disappears at zero **without a reload**.
5. **Right people only:** a member who cannot act sees no badge (G1: a plain member; G3: a member; G4: a non-admin);
   a workspace the viewer can't open never contributes.
6. **Accessible:** the badge has screen-reader text ("2 need your attention"), sits inside the link (one tab stop), uses
   the amber attention style only because the viewer can act (D-384), and fits at 390 px without wrapping the label.
7. **Promised earlier:** for G1 (RFE-012) and G4 (FIX-167 Part B: "the Administration count"), record whether they
   worked at all before the P5-REFINE changes (check `git log` for their merges and the old specs); if a promised badge
   never worked, say so in the report so the owner knows it was a regression or a gap.

Record each badge as a row of the registry (`G1`…`G6`) with screenshots before and after each step. **Send back** to
the owner brief; when the owner brief has already merged and its lane closed, the Orchestrator opens a FIX for it.

### H. The tests moved with the UI (owner, 2026-11-20)

The owner: "I don't want a ton of broken tests because a button was renamed and the tests look for the old label."
Every brief carries the section "Tests move with the UI". Check, for the whole set, as registry rows `H1`…`H5`:
- **H1** master's GitHub CI is green on the merge of every P5-REFINE brief (`gh run list --branch master`); any red run
  caused by a test that still expected old UI is a send-back to the brief that caused it (list the run ids).
- **H2** the nightly / production Playwright project (`scripts/e2e/*.sh`, `--project production`) passes on your
  throwaway cluster for the specs the completion reports listed as "nightly only" (run them; report failures).
- **H3** no test was skipped, marked `fixme`, deleted or loosened to accept both old and new words across the set:
  `git log -p <first P5-REFINE merge>..HEAD -- web/tests web/e2e scripts/e2e services crates | rg "\.skip\(|fixme|test\.only|^-\s*(it|test)\("`
  and review each hit against the completion reports; an unexplained one is Needs changes.
- **H4** no test still expects a retired word: `rg -n -i "Freeze|Unfreeze|Triggers|Evals|Behaviour|What moved|Ask alice|OAuth client|BYO" web/tests web/e2e scripts/e2e`
  (and the other retired words of D-407's table); every hit is either a deliberate negative assertion ("never shows
  Freeze") or a send-back.
- **H5** every completion report has its "tests changed / removed / commands run" list (the brief's section item 5).

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

## Tests move with the UI (owner, 2026-11-20 — binding)

The owner: "Make sure the briefs not only change the website, but also the tests that test it! I don't want a ton of
broken tests because a button was renamed and the tests look for the old label." (P5-REFINE-1 merged with old labels
in tests, and master's CI broke until the CI Watcher fixed it in `ee7bfa0d`.) So, in **every commit** of this brief:

1. **Find every test that sees what you change.** Before committing a change to visible text, a label, an
   `aria-label`, a role, a heading, a route or `?tab=` id, a page's structure, an API field, an error code or message,
   search all test trees for the **old** strings, selectors and paths, and for the components you touched:
   - `web/tests/**` (vitest + Testing Library);
   - `web/e2e/**` (Playwright: the mocked default project **and** the `production` project that `scripts/e2e/*.sh`
     run against a real stack — `first-run.sh`, `kind.sh`, `files.sh`, `ssh.sh`, `channels.sh`, `approvals-park.sh`…);
   - `scripts/e2e/*.sh` themselves (they grep pages and call routes);
   - Rust tests that assert a message, an error code or a response shape (`services/*/tests/**`, `crates/*/src/**`
     `#[cfg(test)]`, `services/aurora-api/tests/it/**`);
   - fixtures and mocks (`web/e2e/fixtures/**`, `web/tests/**/mocks*`, MSW handlers, `web/openapi.json` snapshots).
   A plain `rg -n "<old text>|<old selector>" web/tests web/e2e scripts/e2e services crates` per change is the minimum.
2. **Update them in the same commit as the change.** New expectations use the new words exactly as the mockup shows
   them. Prefer `getByRole(…, { name })` or a `data-testid` over raw text where the text is not the point of the test;
   where the words are the point (they are the fix), assert the new words. Never loosen an assertion so that both the
   old and the new label pass, never `.skip`/`test.fixme`/delete a test to get green; a test is removed only when the
   feature it covers is removed, and the completion report names it and why.
3. **Run what you touched, plus everything that references it.** Through `heavy.sh`: every vitest file you changed
   **and** every vitest file that renders a component you changed; every Playwright spec (mocked project) that visits a
   page you changed. **Exception to the targeted-tests rule for this set (owner, 2026-11-20):** a commit that renames
   or restructures shared UI (a primitive, the app shell, the rail, the agent page, a label used on several pages) runs
   the **whole** web vitest suite once (`cd web && /home/alex/orca/workspaces/aurora/heavy.sh <lane> pnpm exec vitest run` (not `pnpm --dir web …`, which fails here)) before handing over. Production
   specs (`--project production`) can't run in a lane without a real stack: update them by reading, and list them in
   the completion report so the nightly / P5-REFINE-20 runs them.
4. **New behaviour gets new tests** (the brief's Tests section); old tests are **kept and updated**, not replaced.
5. **The completion report lists** every test file changed (and why), every test removed (and the removed feature),
   the commands run with their results, and the production specs that only the nightly will run.
6. A merge that turns master's CI red because a test still expects old UI is a send-back to this lane, not the CI
   Watcher's job.

## Docs to update

- `docs/PROGRESS.md` and `docs/FEATURES.md`: P5-REFINE-20 status per pass; the final "all Match" line with the
  report path.

## Suggested agent/model and reason

Opus, or Sonnet (`sonnet-integration-reviewer`) with browser access. Judging against a design and writing
precise, measurable change lists needs a careful eye; the volume (about 170 rows) suggests splitting the
walk across two reviewers by area but keeping one report and one send-back list.

## Completion report

Fill in per pass (the owner reads this):

- **Pass 1** (2026-11-21, commits walked 6ec943c0 → 4fad882a → 82f47c39, cluster `aurora-r20` on :4720, lane `p5-platform`):
  - Counts (119 findings): Match 68 / Minor 23 / Needs changes 14 / Not checked 14 / Accepted 0. Sections B–H have verdicts in
    the report (5 batch-1 items re-checked as fixed on 82f47c39).
  - Report path: `docs/audit/ux-review-5-verify/REPORT-pass1.md` (screenshots in `docs/audit/ux-review-5-verify/pass1/`; the
    `UXR5/` folder is read-only, so the report lives in this worktree).
  - Change list sent: batch 1 (16 items, ~02:25, msg_d3b2ffe58ac8) → lanes A/B, merged 993ffaa2, 9abb2281, 97940131; batch 2 (18,
    ~03:30, msg_055cf098415e) → lanes A/B, merged fb1d6258, 08051118; batch 3 (16 + batch-1 re-check, ~04:15, msg_aa505ab13469) →
    lanes A/B; batch 4 (4 items) with the worker_done message. SendMessage to the Orchestrator session was held by its permission
    mode, so every batch went through `orca orchestration send`.
  - Fixed in this lane (Scope 7): 7c5b3d55 (retired "LLM key", "Judge", "Behaviour page" in visible text; altitude.spec's
    old-or-new assertion) and 517de2d4 ("Current grader:"), merged as aadf9e27.
  - UX review 4 regressions found: "Administration" twice in the Administration rail; org AI models still opens with the limits
    list; "mm/dd/yyyy" audit date filters.
  - Carried to pass 2 (not checked): firstday-2/3/4, schedules-7, channels-1, computer-1/2 and the runner bug, workspace-5,
    org-4/10/14, admin-1/3/9, G1/G5 (need an approval). The cluster stays up for pass 2.

- **Pass 2** (2026-11-21; part 1 on master `a84e93d0`, part 2 on `6d548b00`; cluster `aurora-r20` on :4720, lane `p5-platform`):
  - Counts (119 findings): Match 103 / Minor 5 / Needs changes 7 / Not checked 4 / Accepted 0. Badges G1, G2, G4, G5, G6 Match
    (the pending approval was made by `aurora-agent call` over SSH against an outgoing webhook at "Ask before each send"; recipe in
    the report).
  - Report path: `docs/audit/ux-review-5-verify/REPORT-pass2.md` (screenshots in `docs/audit/ux-review-5-verify/pass2/`, not in git).
  - Change list sent: batch 5 (8 items, `msg_4e6735e57b3a`: computer-1 keep-awake for terminal and SSH, SSH to a sleeping computer,
    admin-9, the failed grader not kept, two time zones, the empty Automations ask, "can read Outgoing webhook", minor words);
    batch 6 (5 items, `msg_94d6bb094f30`: the 51 s accounts list after `ba6f4d8c`, org AI models, audit words, Approver, minor
    words). Batches 1–4 re-checked: see the report's tables.
  - Fixed in this lane (Scope 7): `web/src/components/onboarding/tour.tsx` — the tour's Agents stop showed a literal
    "Give <Agent> an app"; now "To let it use GitHub, Linear and more, open the agent's Settings › Apps & access." Also
    `docs/14-web-dashboard.md` (tour paragraph). Tests changed: `web/tests/onboarding.test.tsx` (asserts the new words and no
    "<Agent>"); no Playwright spec asserts the tour text. Runs: `vitest run tests/onboarding.test.tsx` 23 passed; `pnpm typecheck`
    clean; eslint on both files clean; `scripts/guards/run-all.sh` all passed.
  - UX review 4 regressions: "Administration" twice in its rail is fixed (b4#5); none new.
  - Not checked yet: firstday-3, computer-2, admin-3, admin-12, the runner bug live (needs a stack with Helm SSH off), batch 4 #1/#3
    (blocked by b6#1). The cluster stays up for pass 3.

- **Pass 3** (2026-11-21, master `b4401a1e`: every send-back of passes 1 and 2; cluster `aurora-r20` on :4720, lane
  `p5-platform`; rebuilt `dev api web runner`, and migration 000269 needed `helm upgrade --reuse-values` because
  `kind-dev.sh dev` runs no migrations):
  - Counts (119 findings): Match 111 / Minor 3 (access-3, computer-2, admin-3) / Needs changes 5 (computer-1, firstday-3, org-7,
    schedules-7, admin-12) / Not checked 0 / Accepted 0. Runner bug Match: Helm `terminal.enabled` and `ssh.enabled` were
    turned on after the runner registered with :4108 closed, and the runner opened the port on a work poll 21 s later.
    Terminal, Files and SSH worked with no manual restart. Badges G1, G2 (also with a key-less agent), G4, G5, G6 Match.
  - Live keep-awake (D-416): web terminal `sleep 300` and SSH `sleep 590` finished on the same computer; an approval asked
    over SSH stayed pending to its 5-min deadline (`expired`, not `abandoned`). `sleep 600` hit the documented SSH idle
    timeout (C-794, 600 s without a channel byte); keepalives counting is an owner question.
  - Report path: `docs/audit/ux-review-5-verify/REPORT-pass3.md` (screenshots in `docs/audit/ux-review-5-verify/pass3/`, not in git).
  - Change list sent: batch 7 (8 items, `msg_d398087dfa54`). The coordinator routed 1, 6, 7, 8a and 8b to lane A (plus the
    dev-stack fixes: API egress to the in-cluster LiteLLM in the kind values, `kind-dev.sh dev` running migrations) and 2, 3, 4,
    5 and 8c to lane B.
  - Fixed in this lane (Scope 7): none (every difference is in a file a lane owns).
  - UX review 4 regressions: none found.
  - Data and Helm state on `aurora-r20`: see the report's Environment; the Helm values are back to the starting ones, and
    `terminal.enabled` now reads source helm. The cluster stays up for pass 4.

- **Pass 4** (2026-11-21, master `306b6efb` (batch 7), then `cf2e80d5` (FIX-199); cluster `aurora-r20` on :4720, lane
  `p5-platform`; rebuilt `dev api web`, then `api terminator web`; the API's egress to the LiteLLM mock added with
  `helm upgrade --reuse-values --set-json networkPolicy.apiExtraEgress=…`, revision 7):
  - Counts (119 findings): Match 112 / Minor 5 (firstday-3, schedules-7, admin-12, computer-2, behaviour-4) / Needs changes 2
    (org-7, access-3's header at 390) / Not checked 0 / Accepted 0. Outside the 119: the bubble contrast Match, the e-mail as a
    name Minor.
  - With the mock reachable: a key check answers `works`; the grader test passes (scn-pong), fails (scn-overload) and reports
    `rate_limited` (scn-ratelimit). A rejected key cannot be made on the mock (it answers any non-master key with 400 "No
    connected db"), so that path stays judged on the Anthropic endpoint. access-3's new sentence is a Match.
  - FIX-199 live: SSH with `ServerAliveInterval=30` ran `sleep 900` to DONE on the same computer; with no keepalives the
    session ended at exactly 600 s ("no activity"); the SSH page line is there.
  - Report path: `docs/audit/ux-review-5-verify/REPORT-pass4.md` (screenshots in `docs/audit/ux-review-5-verify/pass4/`, not in git).
  - Change list sent: batch 8 (8 items, with the worker_done message): 1, 3, 5 to lane A; 2, 4, 6, 7, 8 to lane B.
  - Fixed in this lane (Scope 7): none (every difference is in a file a lane owns).
  - UX review 4 regressions: none found.
  - Dev stack: `kind-dev.sh dev` does not accept `terminator` (an untracked copy with it added was used and deleted).
  - The cluster stays up for pass 5.

- **Pass 5** (2026-11-21, master `ccc56fdd` (all of batch 8); cluster `aurora-r20` on :4720, lane `p5-platform`; rebuilt
  `dev api terminator web`: `dev` accepts `terminator` and its migrate step layered `values-llm-mock.yaml`, revision 9, values
  unchanged):
  - Counts (119 findings): Match 118 / Minor 0 / Needs changes 1 (access-3: at 390 the state "Can't answer: no AI key" runs
    under Settings, and under Status on Chat) / Not checked 0 / Accepted 0. org-7 and the five Minor rows of pass 4
    (firstday-3, schedules-7, admin-12, computer-2, behaviour-4) are Match.
  - Outside the 119: the agent header at 768–~950 px hides the name (h1 0 px) for every agent and the access-3 card overflows
    at 768 (Needs changes); the shell's account button still shows an address as a name (Minor); the dev-stack fixes Match.
  - firstday-3 live with a new person (Frank): only key `not_checked` → "…open the key and press Check again", the row's button
    is Check again.
  - Report path: `docs/audit/ux-review-5-verify/REPORT-pass5.md` (screenshots in `docs/audit/ux-review-5-verify/pass5/`, not in git).
  - Change list sent: batch 9 (3 items, with the worker_done message): 1, 2 to lane A; 3 to lane B.
  - Fixed in this lane (Scope 7): none (every difference is in a file a lane owns).
  - UX review 4 regressions: none found.
  - The cluster stays up for pass 6.

- **Pass 6** (2026-11-21, master `0ddd06b3` (all of batch 9); cluster `aurora-r20` on :4720, lane `p5-platform`; rebuilt
  `dev web` only, batch 9 changed no Rust):
  - Counts (119 findings): Match 119 / Minor 0 / Needs changes 0 / Not checked 0 / Accepted 0. access-3 is Match: at 390 the
    state wraps inside the name column (2 lines on Apps & access, 4 beside Status on Chat), whole and covered by nothing.
  - Outside the 119: the agent header at 768/820/900/1024/1280 Match for an agent with a long state (Nokey) and one without
    (Daneel); the Apps & access starter-key card's button still leaves its card at 768–810 (page scrolls at 768) — Needs
    changes; Chat at 768–~950 squeezes the thread to 158–290 px (Live on the privacy line, Send outside the composer, the
    callout's buttons on its sentence) — Needs changes, pre-existing; "Paused by Erin ·Last run" lost its space (batch 9) —
    Minor; the agent pause banner and Admin › Settings "Last change" show an address (`by_label`, `updated_by_label`) — Minor,
    needs a contract change. The account button, Paused by, decided by and linked chat people show names (b9#3 Match).
  - Smoke at 390 and 1280, both themes: the agent page's tabs, Computer, Chat, workspace Settings, Account: no overflow, no axe
    violation, no console error.
  - Report path: `docs/audit/ux-review-5-verify/REPORT-pass6.md` (screenshots in `docs/audit/ux-review-5-verify/pass6/`, not in git).
  - Change list sent: batch 10 (4 items, with the worker_done message): 1, 2 to lane A; 3, 4 to lane B (4: contract first).
  - Fixed in this lane (Scope 7): none (every difference is in a file a lane owns).
  - UX review 4 regressions: none found.
  - Exit not reached (outside-the-119 items open); the cluster stays up for pass 7.
  - **Lane B, batch 10 items 3 and 4 (2026-11-21, branch `alexiri/p5-polish-b`):** item 3: the space after "Paused by <name> ·"
    restored in `schedule-row.tsx` and `webhook-row.tsx`, a test on the rendered text in `automations-page.test.tsx`. Item 4
    (C-1102, docs/05 and API_CONTRACTS first): `Frozen.by` and `SectionSummary`/`SettingsSection.updated_by` as
    `{ id, display_name } | null` beside the kept labels; openapi.json and schema.d.ts regenerated (`spec_schema.json`
    unchanged, no migration); ITs `freeze::frozen_records_name_the_person_beside_the_label` (agent, workspace, organization,
    operator list) and `settings::last_writer_is_named_beside_the_label` (index, section, PUT); web uses `personName` (banner
    says "Paused on <date>" with no person); the person-name guard now also bans reading `by_label`/`updated_by_label`/
    `created_by_label`/`started_by_label`. Tests changed: `freeze.test.tsx`, `admin-settings.test.tsx`,
    `automations-page.test.tsx`, `person-name-guard.test.ts`, e2e fixtures `support/terminal.ts`, `files.ts`, `mock-api.ts`.
    Run: the new ITs and `agent_freeze_needs…` pass; `openapi_matches_checked_in` pass; 16 vitest files (161 tests) pass;
    `pnpm typecheck`, eslint on touched dirs, `cargo fmt --check`, `extract-schema --check`, `guards/run-all.sh`, clippy
    `-D warnings` on aurora-db and aurora-api all clean. Not run: Playwright, the full vitest suite.

- **Pass 7** (2026-11-21, master `23a42a70` (all of batch 10); cluster `aurora-r20` on :4720, lane `p5-platform`; rebuilt
  `dev api web`, 354 s, no new migration):
  - Counts (119 findings): Match 119 / Minor 0 / Needs changes 0 / Not checked 0 / Accepted 0.
  - Batch 10: the starter-key card's button stays in its card at 768–1280 (Match); "Paused by Erin · Last run" (Match); the
    agent, workspace and organization pause banners and Admin › Settings "Last change" name the person ("Paused by Erin",
    "Paused by Alice Admin", "Last change: Alice Admin, …"; Match); Chat at 768–1023 shows the thread alone with its back arrow
    and the rail, so Live, Send and the callout fit (Match for what b10#2 named).
  - Outside the 119, found: at 768–900 the thread header's three lines (title, "Conversations", the two-line privacy line)
    overflow its 64 px: the title is cut at the card's edge and the last line sits on the border (Needs changes); in the list
    view at 768–1023 the list pushes the status rail out of the card, and focusing the rail scrolls the card 56 px sideways
    (Needs changes); the starter-key card's dot alone on its own line at 768–~860 (Minor, pre-existing).
  - Smoke, both themes: Chat at 390/768/900/1024/1280 (three agents: the callout, automation threads, no key), Apps & access
    at the same widths, the three pause banners with a paused item, Admin › Settings: no horizontal scroll, no axe violation,
    no console error, no `/v1` error.
  - Report path: `docs/audit/ux-review-5-verify/REPORT-pass7.md` (screenshots in `docs/audit/ux-review-5-verify/pass7/`, not in git).
  - Change list sent: batch 11 (3 items, all lane A, with the worker_done message): 1 the thread header at 768–900, 2 the rail
    in the list view at 768–1023, 3 the card's dot. Two further ideas in the report (the Chat page scrolls 64 px below 1280,
    allowed by docs/14's 1280×800 rule; the agent Settings sub-navigation at 768), not sent.
  - Fixed in this lane (Scope 7): none (every difference is in a file a lane owns).
  - UX review 4 regressions: none found.
  - Exit not reached (two Needs changes and one Minor outside the 119); the cluster stays up for pass 8.

- **Pass 8** (2026-11-21, master `05999572` (all of batch 11, FIX-201/202/203); cluster `aurora-r20` on :4720, lane
  `p5-platform`; rebuilt `dev api web`, 467 s):
  - Counts (119 findings): Match 119 / Minor 0 / Needs changes 0 / Not checked 0 / Accepted 0.
  - Batch 11: the list view keeps the rail inside the card at 768–1023, `scrollLeft` 0 after focusing and expanding it
    (Match); the give-key dot beside the sentence's first line at 390–1280 (Match); the thread header: "Conversations" is
    phones-only and ordinary threads fit their 64 px header, but the meta line cut at two lines hides the end of the
    visibility note at 768 ("Only Alice Admin and workspace owners can see this") and an archived chat's "Archived <date> ·
    read only" at 768–960, reachable only by a `title` tooltip (no hover on tablets; the note exists nowhere else in the UI),
    and an archived chat's header overflows at 1024–~1200 (title cut above the card, the meta line over the thread): Needs
    changes.
  - FIX-201: Self-hosted apps' app names open the package page (`sentry` → its version), a package id typed directly
    redirects, an unknown one shows the shared empty state (Match). FIX-202: with Jira Data Center registered instance-wide
    for the check (`its.cern.ch/jira`, deleted afterwards), Jira is Ready, its tile goes straight to the Data Center access
    and token steps, and the workspace card's Connect opens Jira · Data Center (Match). FIX-203 has no UI.
  - Smoke, both themes: Chat and Apps & access at 390/768/900/1024/1280 (three agents); 202 captures, no horizontal scroll,
    no axe violation, no console error, no `/v1` error.
  - Report path: `docs/audit/ux-review-5-verify/REPORT-pass8.md` (screenshots in `docs/audit/ux-review-5-verify/pass8/`, not in git).
  - Change list sent: batch 12 (1 item, lane A, with the worker_done message): the thread header's meta line readable
    without hover and inside the header at every width (768–960 cut, 1024–~1200 archived overflow, 390 archived column).
    Further idea, not sent: the Jira family card keeps the Cloud description when only Data Center is usable.
  - Fixed in this lane (Scope 7): none (the difference is in a file lane A owns).
  - UX review 4 regressions: none found.
  - Exit not reached (one Needs changes outside the 119); the cluster stays up for pass 9.

- **Pass 9** (2026-11-21, master `9fe25317` (batch 12, FIX-204, FIX-206); cluster `aurora-r20` on :4720, lane
  `p5-platform`; rebuilt `dev api web`, 1121 s under load):
  - Counts (119 findings): Match 119 / Minor 0 / Needs changes 0 / Not checked 0 / Accepted 0.
  - Batch 12 (an archived direct chat, another member's chat and three neighbours at 390–1280, both themes, 90 captures):
    below 1280 the meta line is its own row. The whole privacy note and "Archived … · read only" are visible, there is no
    `title`, every line is inside the header and the card, and the title row's border is level with the rail's (217/217,
    153/153). But the row's top and bottom borders end 24 px (390) / 40 px (768–1100) short of the card's right edge
    (`w-full` with negative margins): Needs changes.
  - FIX-204: account Rename and Replace, app upload and bulk role at 390 and 1280: `Field` everywhere, the form dialogs'
    buttons in the footer (bulk role is a `ConfirmDialog`, buttons in its body like every confirmation): Match. FIX-206:
    workspace members and approvers see the summary, who to ask and "Your rules", with no Limits; owners keep the ladder
    with Restrict further… (Match). An organization member sees only who to ask, with nothing about what is in force, which
    `docs/14` §Scope ladder (D-444) requires: Needs changes.
  - 148 captures: no horizontal scroll, no axe violation, no console error, no `/v1` error, no CSP report.
  - Report path: `docs/audit/ux-review-5-verify/REPORT-pass9.md` (screenshots in `docs/audit/ux-review-5-verify/pass9/`, not in git).
  - Change list sent: batch 13 (2 items, with the worker_done message): lane A, the thread header's meta row borders edge to
    edge below 1280; FIX-206's lane, an organization member's Internet access says what is in force in one sentence, and
    the description stops repeating the notice.
  - Fixed in this lane (Scope 7): `web/src/components/internet/internet-level.tsx` → a member's empty "Your rules" no longer
    points at "the limits and defaults below" that FIX-206 removed ("No rules yet. The sentence at the top says what agents
    here may reach."); test `web/tests/internet-pages.test.tsx` (new case). Ran: vitest `internet-pages` + `policy` (36
    passed), eslint on both files, `pnpm typecheck`, `scripts/guards/run-all.sh` (all passed). No other test sees the text.
  - UX review 4 regressions: none found.
  - Exit not reached (two Needs changes outside the 119); the cluster stays up for pass 10.

- **Pass N** (date, commit checked, cluster name/port):
  - Counts: Match / Minor difference / Needs changes / Accepted:
  - Report path: `UXR5/verify/REPORT-pass<N>.md`
  - Change list sent (message to the Orchestrator, time) and the briefs it went to:
  - Fixed in this lane (Scope 7): file → what:
  - UX review 4 regressions found:
- **Final pass:** zero Needs changes and zero Minor difference: yes/no; Accepted items (owner, date, where):
- **Cluster torn down:** yes/no
- **Further ideas:**
