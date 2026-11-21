# P5-REFINE-20 — pass 6 report (UX review 5 verification)

Brief: `docs/tasks/P5-REFINE-20.md`. Reference: <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>, local copy
`/home/alex/orca/workspaces/aurora/design-ux-review-5/` (`UXR5/`). Earlier passes: `REPORT-pass1.md` … `REPORT-pass5.md` (same
folder). The screenshots of this pass are in `docs/audit/ux-review-5-verify/pass6/` in the verification worktree. They are
**not in git**. `pass6/_log.jsonl` records, per capture, the final URL, console errors, 4xx/5xx `/v1` responses, horizontal
overflow, axe wcag2a/aa results and the step output (boxes measured in the page).

Verdicts as before: **Match**, **Minor**, **Needs changes**, **Not checked**. "bN#m" is item m of change-list batch N (batch 10:
this pass).

## Environment (2026-11-21, master `0ddd06b3`: all of batch 9)

- Cluster `aurora-r20` (:4720, SSH 127.0.0.1:2272), kept from pass 5. `git merge master` was a fast-forward to `0ddd06b3`.
  Batch 9 changed only `web/` and docs, so only web was rebuilt: `AURORA_DEV_CLUSTER=aurora-r20 AURORA_DEV_PORT=4720
  AURORA_DEV_SSH_PORT=2272 heavy.sh p5-platform deploy/dev/kind-dev.sh dev web` (72 s, exit 0).
- Data added: on Erin's Daneel (personal workspace) a schedule "Weekly digest" and a webhook "Build hook", both paused by Erin
  (`paused_by` = Erin, whose stored name is her address). Erin's Daneel was paused ("pass 6 check") to read the banner and resumed
  right after through the API (`frozen: null`).
- Never used `aurora-dev`, :4000 or the `aurora` DB.

## Batch 9 (re-check)

| Item | Verdict | Evidence (pass6/…) | Note |
|---|---|---|---|
| 1 access-3 header at 390 | Match | b9-hdr-nokey-{access,chat,computer,settings}-{dark,light}-390 | Nokey ("Can't answer: no AI key"): on Apps & access and Computer the h1 is 155 px and the state wraps to 2 lines in the name column (116,104 128×33); on Chat, where Status sits beside Settings, the h1 is 77 px and the state wraps to 4 lines (51 px wide). Every line of it is on top at its left, middle and right (`elementFromPoint`), no ancestor clips it, no horizontal scroll, no axe violation. The give-key row is its own full-width row (16,141 226×44) |
| 2 agent header at 768–1280 | Match (the header); the card: Needs changes (**b10#1**) | b9-hdr-nokey-*-{768,820,900,1024,1280}, b9-hdr-daneel-{chat,access}-*, b9-probe-access-dark-*, b9-card-access-dark-* | Header: at 768–1024 the name and state have the full column (h1 172 px for Nokey, 125 px for Daneel), the tabs take their own row under it (y 138 for Nokey, 102 for Daneel), Settings stays right; at 1280 one line again (name · state · tabs · Settings). The state is one line, whole and uncovered at every width; both agents, both themes, no axe violation. **But** on Apps & access the starter-key card's button "Give Nokey the Company key" (234 px) is still wider than the card from 768 to ~810 px: 39 px out at 768 (page scrollWidth 775, scrolls sideways) and 17 px out at 790; 820+ fits. The new e2e only drives the Chat tab's card (`answer-fix.tsx`), not this one (`starter-key-card.tsx`) |
| 3 e-mail as a name | Match (the named places); see b10#3 and b10#4 | b9-3-account-*, b9-3-account-m-*, b9-3-paused-*, b9-3-hostclaims-dark-1280, b9-3-channels-* | Erin's account button reads "Erin" (1280 and the 390 menu), its menu header "Erin" with "erin@example.test" under it as the address; Automations: "Paused by Erin" on the schedule and the webhook, no address on the page. "decided by" (Administration › Apps › Self-hosted) and the chat-platform linked people (Account › Linked channels) have no rows on this cluster: by reading, both use `personName` (`host-claims-view.tsx:23`, `channels-view.tsx:57`), and `person-name-guard.test.ts` keeps a raw `display_name` out. **Found:** batch 9 dropped the space after the dot: "Paused by Erin ·Last run: never" and "Paused by Erin ·Last delivery: never" (b10#3) |

## Smoke of the neighbours (390 and 1280, both themes)

Carol's Daneel: Chat, Automations, Computer (Files), Instructions, Apps & access, Memory, Skills; workspace Settings; Account
(smoke-*). No horizontal scroll, no axe violation, no console error, nothing like "undefined", "NaN" or an address in `main`.
The only `/v1` errors are the 503s of Files while the asleep computer wakes (dark 390, the first load), as in earlier passes.
`?tab=settings|tests|behaviour` open Chat (they aren't tab names; Settings is the header button), as before.

## Found on the way

| Key | Verdict | Evidence | Note |
|---|---|---|---|
| Chat at tablet widths (768–~950 px) | Needs changes (**b10#2**) | b9-hdr-daneel-chat-*-{768,820,900}, b10-chat-tablet-dark-*, pass5/b8-1-hdr-daneel-chat-dark-768 | From md the conversation list (232 px), the thread and the Need/Now/Did rail sit side by side, so the thread is 158 px at 768, 210 at 820, 290 at 900 (414 at 1024). At 768: the "Live" badge lies on the privacy line ("Only you and workspace owners can see this conversation"), the Send button is 46 px outside the composer, a message bubble wraps every word, the first-success callout's "See what it did" is 23 px outside the callout. At 820 and 900: "See what it did" / "Got it" sit **on top of** the callout's sentence ("Daneel answered and cha… noth…"); "Live" still on the privacy line at 820. 960+ fits. Not batch 9's doing (pass 5's 768 shot is the same); outside the 119 (checked at 1256 and 390) |
| "Paused by Erin ·Last run" | Minor (**b10#3**) | b9-3-paused-light-1280 | `schedule-row.tsx:218` and `webhook-row.tsx:131` now end the string with " ·" and the next JSX line starts a new expression/text, so the space is lost. Before batch 9 the string ended "· " |
| An address as the name in "Paused by" (agent/workspace/org pause) and "Last change" (Admin › Settings) | Minor (**b10#4**) | b9-3-freeze-chat-dark-{1280,390}, b9-3-freeze-settings-dark-1280; `GET /v1/admin/settings` | A paused agent's banner reads "Paused by erin@example.test on Nov 21, 2026, 11:24: pass 6 check" (`freeze-panel.tsx:42` shows `Frozen.by_label`, which C-200 defines as the freezer's e-mail, so it is an address for everyone, Alice Admin too). Admin › Settings "Last change: alice@example.test, …" (`updated_by_label`, C-771/772; `settings-index-view.tsx:174`, `settings-section-view.tsx:69`, `operational-settings-views.tsx:192`) shows Alice's address although her name is "Alice Admin". The guard misses them because the fields are not `display_name`. A contract change (the person's name, or `{ id, display_name }` like `paused_by`) |

## Change list sent

- **Batch 10** (with the worker_done message): 1 the starter-key card's button at 768–810 (lane A); 2 Chat at 768–~950 (lane A);
  3 the lost space after "Paused by <name> ·" (lane B); 4 the freezer and the settings editor shown by address (lane B, contract
  first: docs/05 and API_CONTRACTS C-200, C-771, C-772).

## Counts (pass 6, the 119 findings)

**Match 119 · Minor 0 · Needs changes 0 · Not checked 0** (119).

- Needs changes → Match (1): access-3.
- Outside the 119: the agent header at tablet widths Match, its Apps & access card Needs changes (b10#1); Chat at tablet widths
  Needs changes (b10#2, pre-existing); the e-mail as a name Match where batch 9 named it, Minor in two more places (b10#4) and
  a lost space (b10#3).

So P5-REFINE-20's exit is **not** reached: the 119 are all Match, but four items outside them are open. The cluster stays up.
