# Memory verification, pass 1 (P5-MEMORY-5)

Date: 2026-11-27. Reviewer lane, no product code changed (no direct fixes made).

## Result

**Not clean.** 21 rows: **17 Match, 3 Minor difference, 1 Needs changes.**

The one "Needs changes" is not in the Memory code: an organization owner with emergency read access can't reach any
agent page, so the read-only Memory view can't be reached at all. Everything else in the Memory work does what the
mockups and briefs ask on a real stack.

## Environment

- Commit checked: `ae473292` (master; CI run 46061994703 passed).
- Own throwaway kind cluster `aurora-mem5` on port 4530, `--llm-mock` (real Claude Code harness, mock model). Brought
  up through `heavy.sh`. **Torn down** (`kind-dev.sh down`). Its cargo cache volumes (`p5-memory-5-kind-dev-*`) were
  kept; no volume was pruned. `aurora-dev` and `aurora-skm` were not touched.
- Browser: Playwright (Chromium) against that cluster, 1256 and 390 px, dark and light, full-page captures, locale
  en-GB. On the walked pages: console and CSP messages, `/v1` 4xx answers, sideways overflow and axe wcag2a/aa. Some
  one-off interaction probes ran at one width or one theme, a few without axe.
- Data: `seed.py` (12 memories, 2 locked; skills at three levels). Alice owns Acme. Bob owns his personal workspace
  and the agent Daneel. Carol is a **co-owner** of that workspace and Dave a **member**.
- Agent saves were real: `scn-memory-tool` created, changed and was refused by a lock. The scenario fixes the key
  (`conformance`) and the text, so the agent's memory and the cards carry those instead of the mockup's samples.
  Recall recorded real "last used" entries.
- My own testing changed some data. A member's API write without a title left `grafana-dashboards` untitled (used as
  an extra untitled case). A delete test removed `aurora-migration`, which I then created again from the seed.

### Pages checked for errors

No CSP violation, no sideways overflow of the page or of a scroll box, and no axe wcag2a/aa finding on any walked page.
The only `/v1` 4xx answers on the Memory page and cards were:

- 409 on the duplicate-key test (expected);
- 404 when a card read the memory its own Undo had just deleted (expected; it also shows in the browser console);
- one 403 on `…/revisions`, see observation O-2;
- for the emergency reader, 403 on `sandbox`, `answer-readiness`, `accounts` and (chat tab) `conversations/direct`.
  These are the agent page and shell, not Memory.

## Registry

Captures for rows that are not Match are in [`memory-pass1/`](memory-pass1/) as `<key>-<theme>-<width>.png`. Match rows
were compared with captures kept in the lane's scratch folder (not committed).

| Row | Verdict | Mockup | What was checked / evidence |
|---|---|---|---|
| mem-1 | **Minor difference** (C-2, C-3) | `after/mem-1*.png` | Chips with counts, filtering, Recently changed / Name sort, source lines (you, another's key, a person, learned by Daneel), lock icons, first row selected, no banner or empty column. Rendered reading view with a table and checklists, key chip (copy checked against the clipboard), Edit · Lock/Unlock · "…" (History, Delete with confirm), meta line. Captures `mem-1-*` (they show both differences) |
| mem-2 | Match | `after/mem-2*.png` | Edit: all fields and exact words, Write/Preview, size line, Locked switch, SaveBar, dirty dot. New: key made from the title, editable, "Made from the title" goes once edited, 409 and bad-key messages on the field, no Key field after saving, Done when unchanged. Leaving with changes: Keep editing, Discard, Save and leave all work at 390 and 1256 px |
| mem-3 | Match | `after/mem-3*.png` | "1 memory matches “rotation”", Clear, highlights in snippet and title, no `<b>` in page or API, first match opened with its line marked, "Match 1 of 1"; empty result "No memories match “…”" |
| mem-4 | **Minor difference** (C-4) | `after/mem-4*.png` | History replaces the reading view; names, verbs, "through an API key", "in “Release prep”", Current; What changed with `DiffView` and counts; first version; disabled remove with its reason on Current; Restore, toast and Undo; two-step remove, then "This version's text was removed" and "its text was removed". Captures `mem-4-*` |
| mem-5 | Match | `after/mem-5*.png` | Both cards on real events. View opens `?tab=memory&memory=conformance`. Undo then Redo on a changed and on a created memory (checked in the API's revisions), "Memory undone" toast, "Changed since" with its reason. Refused card with reason and attempted text; Dismiss, Show; Save it yourself on a locked memory opens Edit with the text appended as an unsaved draft. The tool group says **Failed**, not Deny |
| mem-6 | Match | `after/mem-6*.png` | List screen, tap to memory screen, "‹ Memory", Edit, "…", Lock/Unlock and History at the bottom, History screen, full-width editor; the deep link survives a reload |
| M-1 | Match | `before/M06` | No raw tags; the open memory is one of the results |
| M-2 | Match | `before/M03`, `M04` | Opens rendered; the editor is tall |
| M-3 | Match | `before/M01` | Source line per row, meta line with last use ("Daneel last used it in “Weekly report” today", or "hasn't used it yet") |
| M-4 | **Minor difference** (C-2) | `before/M08`, `M09` | Description on both forms. An untitled memory is titled by its first line, but with that line's Markdown marks. Captures `mem-1-*-1256` |
| M-5 | Match | `before/M05` | Readable timeline, diff, reason on the disabled action, in place of the reading view |
| M-6 | Match | `before/C01` | Cards say what was saved or why not; Failed, not Deny |
| M-7 | Match | `before/M01` | Banner and empty column gone. Settings › General still has "Language agents remember in" (under Show more controls), with languages by name |
| M-8 | Match | `before/M07`, `M08` | One Locked switch on both forms; the lock shows in the list, as Unlock, and in the meta line (a Locked badge on a phone) |
| M-9 | Match | `before/P01`, `P02b` | Two screens |
| M-10 | Match | `before/M11`, `M12` | Key chip without Show more controls |
| D-497 | Match | — | API and UI. Bob and co-owner Carol get conversation titles (an own-right owner may read the chat, as accepted in the SEC-157 review). Member Dave gets `conversation: null` for `added_by`, `last_used` and revisions, and the page says "in a conversation". Alice without a grant gets 403. With a grant (view check below) she sees the title. No e-mail address in list, search or revision responses for Bob, Carol or Dave, including after Dave (whose display name is his address) wrote a memory: it reads "dave" |
| SEC-157 | Match | — | Row in `docs/17-security.md`; the three tests it names are in `services/aurora-api/tests/it/memory.rs`. They pass by CI on `ae473292`; I did not run them locally |
| Read-only | **Needs changes** (C-1) | — | An emergency reader is redirected away from the agent. Captures `read-only-*`. The view itself is right when reached (see C-1) |
| Tests | Match, with a note | — | CI passes on `ae473292` (and `c5f1dbcf`, which holds the first three merges). No `.skip`, `.fixme`, `.only` or `#[ignore]` added since `d96d979f`; `memory-one-box.test.tsx` went with its feature; no hit for "Lock it", "Technical details · Memory key", "Remove content of #" or `RevisionHistory`. The one "Deny" badge left is for a policy DENY. **Note:** CI was not green at each merge. Run 46045491601 (with P5-MEMORY-1) failed clippy in the new `conversations/reference.rs`, plus an unrelated mocked spec (`fix-251`); run 46051034468 (with -1 and -2) failed `arbitrary-values` (-2's list row) and `audit-actions` (P5-SKILLS-1's actions). Both are fixed on master, so nothing to send back |
| Rules | Match | — | Both themes, no sideways scroll at 390 px, shared SaveBar, `Markdown`, `DiffView`, chips, dialogs; Delete and Remove text ask first. History times use the shared relative form; list ages and meta dates follow the mockup's wording |

## Change list

### FIX-235 / FIX-236 (emergency access; not a Memory brief, needs a new FIX)

**C-1 · broken · rows Read-only, P5-MEMORY-2 scope 10 · no mockup**

- File: `web/src/app/o/[org]/w/[ws]/layout.tsx` (the `manage` condition).
- Differs: it redirects whenever `my_role == null && administers === true`. The real API answers an organization owner
  who holds an **active emergency grant** with `my_role: null`, `administers: true`, `emergency_reader: true`. So every
  page under `/o/<org>/w/<ws>` (Memory, Instructions, Skills, chats, Activity) lands on the workspace manage page. The
  page still says "In an emergency you can open read access" while access is open.
- Why tests pass: `web/e2e/fix-236.spec.ts` mocks the reader with `administers: false` (`readerWs`), which the real API
  does not send for this person.
- Should be: no redirect while `emergency_reader === true`. The mocked reader should have the real shape
  (`administers: true`), seen failing first. Check any other place that picks the manage page from `administers` alone.
- Reproduce: Alice (organization owner, not in Bob's workspace), `POST /v1/workspaces/{ws}/emergency-access`, open
  `/o/acme/w/dev-bob/agents/daneel?tab=memory`; the address becomes `/o/acme/workspaces/dev-bob`.
- How the Memory view was still checked: I overrode **only** `administers` to `false` in the browser's copy of the
  workspace answer; every other answer was the real API's. The page then had no search, Edit, Lock, "…", History, New
  memory or editor; `&history=1` showed the reading view; chips, list and reading view worked. **Match**, but only
  under that override.

### P5-MEMORY-2

**C-2 · rough · rows mem-1, M-4 · `after/mem-1.png` (the "Carol drinks green tea, no sugar." row)**

- File: `web/src/lib/memory-key.ts`, `memoryName()`.
- Differs: for an untitled memory it removes only leading `#`. A memory starting with a list item shows
  `- *API latency*: `` `platform/api-overview` `` — p50/p95 by route.` as its title (list row; the same function names
  the reading view, History's subtitle, the editor's leave dialog and the Name sort). Agent-written memories often
  start with a list item or bold text.
- Should be: the first line as plain text, as search snippets already are: no list, task-list, quote or heading marks,
  no emphasis or code marks, a link shown by its text. Here: "API latency: platform/api-overview — p50/p95 by route."
  Underscores inside a word stay.
- Capture: `memory-pass1/mem-1-*-1256.png`, an untitled row in the list.

**C-3 · rough · row mem-1 · `after/mem-1.png` (the reading view's lists)**

- File: `web/src/app/globals.css`, `.md ul.contains-task-list` (a shared rule older than the Memory work, from
  2026-11-14; the reading view is where it shows).
- Differs: in a list that mixes ordinary and checklist items, the ordinary items also lose their bullet. The seeded
  Aurora memory's "The Grafana service account still needs read access…" reads as a paragraph indented about 4 px
  from its heading.
- Should be: checklist items keep the box and no bullet; ordinary items in the same list keep their bullet and indent.
  It applies wherever `Markdown` renders, so the markdown tests are the neighbours to run.
- Capture: `memory-pass1/mem-1-*.png`, under "Risks".

### P5-MEMORY-3

**C-4 · rough · row mem-4 · `after/mem-4.png` (diff lines 10 and 13)**

- File: `web/src/components/ui/diff-view.tsx`, `LineRow` with `wrap`.
- Differs: a wrapped line continues under the `+`/`-` mark, not under its text. A long word with no spaces moves to the
  next line and leaves the mark alone on the first.
- Should be: with `wrap`, a hanging indent (continuations aligned with the text after the mark), and the word starts
  on the mark's line and breaks where it must. Only History passes `wrap`; other callers stay as they are.
- Capture: `memory-pass1/mem-4-*.png`: the "Switch the old bot off by 1 December…" line and the long identifier.

## Differences kept by the lanes that I accept

Each is required by a brief or decision, or is the shared component or real shell the design README asks for. The owner
may veto any of them.

- Real shell, sub-nav and header; the reading view's title wraps to three lines at 1256 px in that column (D-496).
- Shared ink primary button; floating shared SaveBar without the amber dot; **Done** on an unchanged edit.
- Chips wrap at 390 px (`14`: no inner scroll); key chip in the reading view (scope 3); search marks titles (scope 7).
- Sub-screens below 1100 px, not only below 768 px: wider than the brief says; the reason (two columns don't fit
  beside the rail and sub-nav, and stacking is M-9 again) holds. I did not capture 768–1099 px.
- "By you" includes your own API key; locked memories add "Locked by … on …".
- History: relative times from the shared `When`; two line-number columns, folded lines, **Show the whole file**
  (shared `DiffView`, D-496); no Restore while Current is selected (scope 4).
- Chat: the tool group stays above each card (P5-MEMORY-4 scope 1); app type scale and tokens; actions under the text
  at 390 px. The card matches the mockup's layout and words (about 566 px wide against 570).

## Regressions looked for

- Settings › Instructions and Skills (agent), Settings › General (workspace) and Manage: no new errors.
- Export dialog: "Instructions · 14 memories (2 locked) · 5 skills". Save as template lists 14 memories with the
  warning about what they can hold. I opened both dialogs; I did not download, import or deploy.
- None found, apart from C-1.

## Observations and further ideas (not counted, not blocking)

- **O-1** An untitled memory's chat card and refusal sentence name it by its key ("conformance"), as C-1172 says, while
  the list names it by its first line. The mockup's cards show a readable title; asking the agent to send a title when
  it saves would close that.
- **O-2** Under the override, one of four emergency loads with `&history=1` asked for `…/revisions` once and got 403;
  the page ended in the reading view. Probably History rendering before the workspace answer arrived. Re-check in pass
  2, once C-1 is fixed.
- **O-3** An untitled memory's reading view repeats its first line as the heading and again in the text.
- **O-4** The editor's Preview draws checklists as the browser's greyed-out checkboxes, while the reading view draws
  ticked, struck-through boxes. No mockup shows Preview.

## Not checked on the live stack

- Memory cards for an emergency reader (the chat tab did not open the conversation for that reader; vitest covers it).
- `template` and `import` source lines (no template was deployed; no server path writes `import`).
- "Save it yourself" after a refusal that isn't a lock (the new-memory draft).
- A keyboard-only walk beyond what axe covers.

## For pass 2

Re-check C-1 (the Read-only row on the real flow, without the override, plus O-2), C-2 (mem-1, M-4), C-3 (mem-1, and a
chat message with a mixed list) and C-4 (mem-4), with a quick look at their neighbours.
