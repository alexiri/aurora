# Memory verification, pass 2 (P5-MEMORY-5)

Date: 2026-11-27. Reviewer lane, no product code changed (no direct fixes made).

## Result

**The Memory registry is clean: 21 rows, all Match.** The four items sent back (C-1 with O-2, C-2, C-3, C-4) are fixed
on a real stack. The emergency reader walk found **three things outside the Memory rows** (W-1, W-2, W-3) to send
back. They aren't Memory defects, so I haven't re-opened a row for them; whether they hold up "done" is the
coordinator's call.

## Environment

- Commit checked: `1e067b16` (local master with FIX-277, `936eca3c`). CI run 46073002423 for `936eca3c` passed.
- `aurora-mem5` (:4530) was **torn down and created again** on that commit, so pass 1's and the Skills review's scratch
  changes (the hand-edited `last_head_sha`, the extra mock scenarios) are gone. Own throwaway cluster, `--llm-mock`,
  `heavy.sh`; **still up** for P5-SKILLS-5 pass 2. `aurora-dev` and `aurora-skm` untouched; no volume pruned.
- Data: `seed.py`; real agent saves (created, changed, refused by a lock); recall uses; three untitled memories that
  start with a list item, a task item and a quote; a chat message with a mixed list; a schedule with one run; two
  webhooks, one with an accepted delivery and one with a refused one. Carol co-owner, Dave member.
- **No override of any answer in this pass.** Alice opened emergency access through the manage page's own button and
  two-step dialog, and ended it with **End now**.
- Playwright, 1256 and 390 px, dark and light for the pages walked; a few interaction probes at one width or theme.
  Per walked page: console and CSP, `/v1` 4xx, sideways overflow, axe wcag2a/aa, and every request that wasn't a GET.

## Rows re-checked

| Row | Pass 1 | Pass 2 | Evidence on the live stack |
|---|---|---|---|
| Read-only (C-1) | Needs changes | **Match** | After opening, Alice lands on `/o/acme/w/dev-bob` and stays. Memory has no search, Edit, Lock, "…", History or New; its controls are the rail of settings, the chips, the sort, the rows, Copy Key and Show. The manage page says "Emergency read access is open until …", offers no second grant and links **Read the workspace's agents**. After End now, Alice is back on the manage page and can open again |
| O-2 | observation | **Fixed** | A cold load of `?tab=memory&memory=conformance&history=1` shows the reading view at all four settings, with no `/revisions` request and no 403 |
| mem-1, M-4 (C-2) | Minor | **Match** | "API latency: platform/api-overview — p50/p95 by route.", "Renew the Grafana token before Friday", "Use the snake_case_name in config_file.yaml, not 2 * 3 * 4…" in the list, the Name sort, History's subtitle and the leave dialog; the reading view's title too. Underscores and spaced stars are kept; the third name is cut before its `#277`, so that part wasn't seen |
| mem-1 (C-3) | Minor | **Match** | Seeded Aurora memory: the ordinary item has its bullet (`list-style: disc`), the checklist item none; numbered lists unchanged. A chat message with a mixed list: ordinary items have bullets, checklist items boxes only (1256 and 390 px) |
| mem-4 (C-4) | Minor | **Match** | Wrapped diff lines hang under their text; the long identifier starts on the mark's line. No sideways scroll of the diff at 390 px |

The other rows were Match in pass 1. This pass saw their neighbours again (list, reading view, History, the memory
cards in chats, for Bob and for the reader) and nothing changed their verdict. **21 Match, 0 Minor, 0 Needs changes.**

## The emergency reader walk

Real rows on every page. **Only two requests that weren't GETs** in the whole walk: opening the grant and ending it.

| Page | Result | `/v1` 4xx | Write controls shown |
|---|---|---|---|
| Overview | Opens: agents, recent activity | none | none (End now) |
| Agents | Opens; the agent link works | none | none |
| Agent, Chat (default tab) | Lists automation and other members' chats, all "Read-only"; creates nothing | none | none |
| A conversation (a refused save, a changed save) | Opens with the visibility line and "Read-only. You hold emergency read access…"; no composer. Memory cards: View on the saved one, no Undo; no Save it yourself | none | **Reply** on each message (W-2). "…" holds only Copy link. Dismiss on the refused card only folds it locally |
| Conversation search | "Search messages for “conformance”": 3 results | none | none |
| Memory (list, reading, `&history=1`) | See above | none | none |
| Instructions (and the header's Settings) | Name and text read-only (typing changes nothing), the switch disabled, no upload, no save bar | none | none |
| Automations (All, Webhooks filter) | The schedule and both webhooks; **Runs** and **Deliveries** show their rows; "Refused: wrong signature" | none | none |
| A run's and a delivery's conversation | Open read-only from Runs, Deliveries and activity details' Open | none | none beyond Reply (W-2) |
| Agent Skills, and a skill panel | List and panel read-only, "You can read this skill. You can’t change it.", switches disabled | **403** `GET /v1/skills/{id}/files/scripts%2Ffiring.sh` on opening a file (W-1) | none |
| Workspace Skills and a panel | Read-only, "Managed by the workspace’s owners.", no switch | none | none |
| Activity: Requests, Details; Usage | Real rows; details open | none | none |
| People | The three people, and the emergency history with this grant | none | none |
| `?tab=computer`, `?tab=manage` | Fall back to Chat | none | none |
| Settings, Apps, Internet access, Approvals, Welcome, terminal, new agent from a template | "Emergency read access doesn’t cover this page." with the link to the agents | none, except below | none |

- **Not-covered pages:** exactly those in the last row said so. No covered page did.
- My template address (`agents/new/template`) was not a real template. It logged a 400 on
  `/v1/orgs//governance/effective` (an empty organization id) behind the notice; Bob gets the same 400 and a 404 there,
  so it isn't about emergency access.
- Right after **End now**, the agent page that was open got 403s on `accounts`, `sandbox`, `answer-readiness` and
  `conversations` as it left for the manage page.
- No console or CSP error otherwise, and no axe finding. The only sideways scroll: two chart boxes on **Usage** at
  390 px.

## To send back

**W-1 · broken · an emergency reader opening a skill's file** (FIX-277 follow-up, with P5-SKILLS-3's panel)

- Differs: the panel's file tree lets the reader open `scripts/firing.sh`. The server refuses skill files to anyone
  without own access (on purpose: `route_inventory.rs` keeps "the skill item and its files" on own access), so the
  request is a 403 and the panel shows "You manage this workspace but you are not one of its people.", a **Manage this
  workspace** link, a reference and **Try again**, which can only fail again.
- Should be: for a reader the files are listed but don't open (or the tree says emergency access doesn't cover file
  contents); no request, no error box. The same on the workspace and organization panels for skills with files. Add the
  case to FIX-277's reader walk with a skill that has a file, seen failing first. I did not check what the server should
  allow; widening it would be a security decision.
- Captures: `memory-pass2/walk-skill-file-*`.

**W-2 · flow · a dead Reply button on read-only conversations** (chat; wider than FIX-277)

- Differs: hovering a message in a read-only conversation offers **Reply**. There is no composer, and pressing it does
  nothing. It shows for the emergency reader and also for a co-owner with own access reading another member's direct
  chat (Carol, same two buttons), so it isn't new with FIX-277, but FIX-277 says a reader sees nothing that acts.
- Should be: no Reply where there is no composer (D-470: no control that silently does nothing). Copy and Copy link
  stay.
- Captures: `memory-pass2/walk-reply-*` (taken as Carol, with the button focused).

**W-3 · words · the manage page while access is open** (FIX-277)

- Differs: the notice says "Emergency read access is open until …" and links **Read the workspace's agents**, while
  the **Agents** section below still says "You can pause or resume them; **you cannot open them**."
- Should be: while the viewer's own grant is open, the sentence doesn't say the agents can't be opened (the notice
  already dropped "open its chats or").
- Captures: `memory-pass2/walk-manage-open-*`.

## Observations (not counted)

- **Overview › Recent activity says "No activity yet."** for the reader and for Bob, although
  `GET /v1/workspaces/{ws}/activity` returns 11 rows to both and the Activity page shows the runs, deliveries and
  answers. The panel is marked "Live", so it may show only what happens while it is open; if so, the empty words are
  wrong. Not checked further, and not about Memory.
- Usage's chart boxes scroll sideways at 390 px.
- For a reader, Instructions still shows the picture hint "PNG, JPG or WebP, up to 512 KiB" with no way to upload.
- Pass 1's O-1 (an untitled memory's chat card names it by its key), O-3 (its reading view repeats the first line) and
  O-4 (Preview's plain checkboxes) are as before.

## Not checked

- Approvals for the reader with a real held approval, and a failed agent start in a reader's chat (no data for either;
  FIX-277's mocked walk covers them).
- Workspace Skills with a file or a library skill, for the reader.
