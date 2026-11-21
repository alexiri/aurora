# P5-REFINE-20 — pass 7 report (UX review 5 verification)

Brief: `docs/tasks/P5-REFINE-20.md`. Reference: <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>, local copy
`/home/alex/orca/workspaces/aurora/design-ux-review-5/` (`UXR5/`). Earlier passes: `REPORT-pass1.md` … `REPORT-pass6.md` (same
folder). The screenshots of this pass are in `docs/audit/ux-review-5-verify/pass7/` in the verification worktree. They are
**not in git**. `pass7/_log.jsonl` records, per capture, the final URL, console errors, 4xx/5xx `/v1` responses, horizontal
overflow, axe wcag2a/aa results and the step output (boxes measured in the page).

Verdicts as before: **Match**, **Minor**, **Needs changes**, **Not checked**. "bN#m" is item m of change-list batch N (batch 11:
this pass).

## Environment (2026-11-21, master `23a42a70`: all of batch 10)

- Cluster `aurora-r20` (:4720, SSH 127.0.0.1:2272), kept from pass 6. `git merge master` was a fast-forward to `23a42a70`.
  Batch 10 changed Rust (C-1102) and web: `AURORA_DEV_CLUSTER=aurora-r20 AURORA_DEV_PORT=4720 AURORA_DEV_SSH_PORT=2272 heavy.sh
  p5-platform deploy/dev/kind-dev.sh dev api web` (354 s, exit 0; the migrate step ran, no new migration).
- Pauses made for the banners and lifted right after through the API (each `frozen: null` afterwards): Erin's Daneel by Erin
  (agent), the "Acme team" workspace by Alice Admin, the Acme organization by Alice Admin; reason "pass 7 check".
- Never used `aurora-dev`, :4000 or the `aurora` DB.

## Batch 10 (re-check)

| Item | Verdict | Evidence (pass7/…) | Note |
|---|---|---|---|
| 1 the starter-key card's button at 768–810 | Match (the button); the card's dot: Minor (**b11#3**) | b10-1-card-{dark,light}-{390,768,790,810,820,900,1024,1280} | "Give Nokey the Company key" stays inside its card at every width: 21 px clear at 768–820 (it wraps to two lines, 56 px tall), one line from 900. No horizontal scroll, no axe violation. **But** at 768–~860 the card's amber dot sits alone on its own line above the sentence (the sentence's `basis-60` does not fit beside it in a 175–230 px card); pre-existing (pass6 shot is the same) |
| 2 Chat at 768–~950 | Match (the squeeze); two new defects: Needs changes (**b11#1**, **b11#2**) | b10-2-chat-*, b10-2-thread-*, b10-2-thdr-zoom-light-768, b11-rail-focus-light-{768,900}, smoke7-chat-* | 768–1023 show the thread alone with its back arrow (390 / 442 / 522 / 582 / 645 px wide at 768 / 820 / 900 / 960 / 1023), the rail beside it; from 1024 list (232) + thread (414 at 1024, 670 at 1280) + rail. Live no longer on the privacy line, Send inside the composer, the first-success callout's buttons beside its sentence (none over it) at every width, both themes; the back arrow opens the list. **Found:** (a) at 768–900 the thread header now carries "Conversations" under the title in a fixed 64 px header (`md:h-16`, `items-center`): 72 px of text, so the title's top is cut by the card's edge (h2 at 149, card at 152) and the privacy line's second line lies on the header's bottom border (ends 221, border 217); 960–1023 fit (privacy on one line). (b) In the list view at 768–1023 the list is `w-full shrink-0`, which pushes the status rail out of the card (900: rail 867–923, card ends 868; 1 px shows). Tab onto "Expand status panel" scrolls the card's content 56 px sideways: "Conversations" reads "ersations", Search loses its icon |
| 3 "Paused by Erin ·Last run" | Match | b10-3-paused-{dark,light}-{1280,390} | "Paused by Erin · Last run: never · Owner: Erin" and "Paused by Erin · Last delivery: never · Owner: Erin"; no address on the page |
| 4 an address as the name in pause banners and "Last change" | Match | b10-4-freeze-agent-*, b10-4-freeze-ws-*, b10-4-freeze-org-*, b10-4-settings-index-*; the freeze responses | Agent (paused by Erin, whose stored name is her address): "Paused by Erin on Nov 21, 2026, 12:12: pass 7 check" on Chat and Automations, 1280 and 390. Workspace (by Alice Admin): "Paused by Alice Admin on …" on Overview, the agent page and Settings. Organization: "Paused by Alice Admin on …" on the org Overview and Settings, and in its workspaces "This workspace is paused because its organization is paused. Paused by Alice Admin on …". The API returns `by: { id, display_name }` beside `by_label`. Admin › Settings: "Last change: Alice Admin, Nov 21, 2026, 05:29" (Instance rules) and "…, 08:09" (Terminal); the rows changed by the system show the date only. The section pages (Instance rules, Terminal, Email, LLM endpoints) now redirect into their topic pages (P5-IA-3), which embed the form without a header, so "Last change" shows only on the index. No address anywhere on these pages, both themes |

## Smoke of the neighbours (both themes)

- **Chat tab** at 390, 768, 900, 1024, 1280 (plus 820, 960, 1023 for Alice's Daneel): Alice's Daneel (a thread with the
  first-success callout), Carol's Daneel (its direct chat, automation threads, other members' chats), Bob's Nokey (no key:
  the "Nokey has no AI model yet" card with its give-key button). List, thread, composer, rail and callout as in b10#2; the two
  defects b11#1 and b11#2 are the only ones found. smoke7-chat-*.
- **Apps & access** at the same widths: Carol's Daneel and Bob's Nokey (full page). No overflow, no address, nothing like
  "undefined" or "NaN". smoke7-access-*.
- **Pause banners** with a paused item: agent, workspace, organization, see b10#4. The banner is full width under the header
  (agent) or above the page (workspace, organization) at 1280 and 390.
- **Admin › Settings** "Last change": see b10#4.
- Every pass-7 capture: no horizontal scroll, no axe violation, no console error, no 4xx/5xx `/v1` response, no CSP report.

## Change list sent (batch 11, with the worker_done message)

1. **Lane A — Needs changes.** `web/src/components/chat/thread-header.tsx`: at 768–~950 the header holds the title,
   "Conversations" and a two-line privacy line in 64 px, so the title is cut at the card's top edge and the last line sits on the
   header's bottom border (b10-2-thdr-zoom-light-768). Batch 10 made the "Conversations" line show up to lg. Every line must lie
   inside the header and inside the card, with the header's bottom border still in line with the rail's (P5-POLISH-6). The lane
   picks how: e.g. no fixed height below lg with the rail's header following it, or the back word beside the arrow and the
   privacy line on one truncated line. Test: measure at 768/820/900/960/1023 in both themes (`agent-header-widths` or a chat
   spec): every text box of the header within the header box and the card.
2. **Lane A — Needs changes.** `web/src/components/chat/chat-panel.tsx`: in the list view at 768–1023 the list pane (`w-full
   shrink-0`) pushes the status rail out of the card; the rail is invisible and focusing it scrolls the card 56 px sideways
   (b11-rail-focus-light-768/900). Keep the rail beside the list (e.g. the list `min-w-0 flex-1` below lg, `lg:flex-none
   lg:w-58`/`lg:w-80`), or don't render it in the list view. Test: in the list view at 768/900/1023 the rail lies inside the
   card and the card's `scrollLeft` stays 0 after focusing "Expand status panel".
3. **Lane A — Minor.** `web/src/components/agents/starter-key-card.tsx`: at 768–~860 the amber dot sits alone on its own line
   above the sentence (b10-1-card-light-768). Keep the dot beside the sentence's first line at every width (the access-3
   mockup), e.g. dot and sentence in their own non-wrapping row, the actions wrapping under it.

Not sent (further ideas, they break no `docs/14` rule):

- Below 1280 px the Chat page scrolls 64 px (Send 8 px below the fold in a 900 px tall window at 768–1279): since batch 9 the
  tabs take their own row there, and the panel height still assumes one row (`md:h-[calc(100dvh-7.5rem)]`). `docs/14` asks
  for no page scroll only down to a 1280×800 window, which holds (p7-vfit in `_log.jsonl`). It could go with item 2.
- At 768 the agent Settings pages keep their 208 px sub-navigation beside the content, which leaves the content 216 px
  (smoke7-access-carol-light-768). It could stack above the content below lg.

## Counts (pass 7, the 119 findings)

**Match 119 · Minor 0 · Needs changes 0 · Not checked 0** (119).

- Outside the 119: b10#1, b10#3, b10#4 Match; b10#2 Match for what it named. Batch 10 brought two new defects at
  768–1023, both Needs changes (b11#1 the thread header, b11#2 the rail in the list view), and a pre-existing Minor stays open
  (b11#3 the card's dot).

So P5-REFINE-20's exit is **not** reached: the 119 are all Match, but two Needs changes and one Minor outside them are open. The
cluster stays up.
