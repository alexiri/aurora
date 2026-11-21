# P5-REFINE-20 — pass 8 report (UX review 5 verification)

Brief: `docs/tasks/P5-REFINE-20.md`. Reference: <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>, local copy
`/home/alex/orca/workspaces/aurora/design-ux-review-5/` (`UXR5/`). Earlier passes: `REPORT-pass1.md` … `REPORT-pass7.md` (same
folder). The screenshots of this pass are in `docs/audit/ux-review-5-verify/pass8/` in the verification worktree. They are
**not in git**. `pass8/_log.jsonl` records, per capture, the final URL, console errors, 4xx/5xx `/v1` responses, horizontal
overflow, axe wcag2a/aa results and the step output (boxes measured in the page).

Verdicts as before: **Match**, **Minor**, **Needs changes**, **Not checked**. "bN#m" is item m of change-list batch N (batch 12:
this pass).

## Environment (2026-11-21, master `05999572`: all of batch 11, FIX-201, FIX-202, FIX-203)

- Cluster `aurora-r20` (:4720, SSH 127.0.0.1:2272), kept from pass 7. `git merge master` was a fast-forward to `05999572`.
  `AURORA_DEV_CLUSTER=aurora-r20 AURORA_DEV_PORT=4720 AURORA_DEV_SSH_PORT=2272 heavy.sh p5-platform deploy/dev/kind-dev.sh dev
  api web` (467 s, exit 0; the builtin packages, which FIX-201 bumped, ride in the API image).
- For the FIX-202 smoke, Jira Data Center was registered instance-wide as the owner did (`its.cern.ch`, base path `/jira`,
  label "pass 8 check"; the registration probe is one anonymous GET of `/rest/api/2/serverInfo`; a made-up host fails the probe
  with `instance_probe_failed`, so no fake server was possible). It was deleted right after (`DELETE /v1/app-instances/…` 204);
  the instance list is back to the two built-in Sentry regions. No connection was made.
- Never used `aurora-dev`, :4000 or the `aurora` DB.

## Batch 11 (re-check)

Measured at 768, 820, 900, 960, 1023 and the neighbours 390, 1024, 1280 (the archived thread also 1100, 1180, 1279, 1440), both
themes (the dark and light measurements are identical). Threads: Alice's direct chat, Alice's **archived** chat (its header
also holds Restore), Carol's schedule run thread, Carol's own direct chat, and the chat of Alice that Carol (workspace owner)
reads ("Only Alice Admin and workspace owners can see this conversation").

| Item | Verdict | Evidence (pass8/…) | Note |
|---|---|---|---|
| 1 the thread header at 768–1023 | Needs changes (**b12#1**) | b11-1-{direct,archived,carol-sched,carol-d1,carol-d2}-{dark,light}-{390,…,1280}, b11-1-zoom-carol-d1-light-768, b11-1-zoom-archived-light-768, b11-1-zoom-archived-dark-1024, b11-1-archived-crop-light-390 | **What works:** "Conversations" is gone from 768 up (phones only); for the ordinary threads every text line now lies inside the 64 px header and the card at every width, and the header's bottom border is in line with the rail's (217/217). **But** the cut at two lines hides information at 768–960, and the only way to it is the `title` tooltip, which a tablet (no hover) and a keyboard user never see. (a) Carol reading Alice's chat at 768: "Only Alice Admin and workspace owners can see this" — "conversation" is cut. (b) An archived chat (Restore beside Live leaves the meta line 115 px at 768): at 768 "Only you and workspace" (two of four lines), and "Archived 21 Nov 2026 · read only" is hidden entirely at 768, 820, 900 and 960; the lock icon is centred on the four-line span, so at 768 only its top shows at the clip edge. The visibility note exists nowhere else in the UI (`chat-thread.tsx:134` is its only source; not in the list row, the rail or the ⋯ menu), so **who can see this chat is not fully reachable without hover**: per this pass's instruction that is Needs changes. (c) Neighbour, same component: the archived thread at **1024–~1200** (`lg:max-h-none` lets the line take three lines in a fixed 64 px header, `items-center`): the title starts 20 px above the header and the card (h2 top 133, card top 152 at 1024; 149/152 at 1100 and 1180) so its top is cut, and "· Archived 21 Nov 2026 · read only" runs 20 px past the header's bottom border over the thread. 1279 and up fit. (d) Neighbour: at 390 the archived header is 189 px tall: the meta line is an 89 px column of 8 lines beside Live, Restore and ⋯ (all inside the header, nothing cut; not a rule break on its own, but the same cause) |
| 2 the list view keeps the rail inside the card | Match | b11-2-list-*, b11-2-focus-*, b11-2-expanded-*, b11-2-focus-crop-light-900; `b11-2-rail*` rows in `_log.jsonl` | In the list view at 768/820/900/960/1023 (Alice's and Carol's Daneel) the rail lies inside the card (e.g. 900: rail 811–867, card 288–868); Tab reaches "Expand status panel" (6 Tabs from New chat; 11 for Carol), and no element in the card has `scrollLeft` > 0 before, after focusing or after expanding. 1024 shows both panes as before |
| 3 the give-key dot beside its sentence | Match | b11-3-card-{dark,light}-{390,768,790,820,860,900,960,1023,1024,1280}, b11-3-card-crop-light-768 | The dot sits on the sentence's first line at every width (dot 541,303 / first line 561,298–315 at 768–1024), the button stays inside the card (0 px out; two lines at 768–820, one from 860) |

## Owner fixes since pass 7 (smoke)

| Fix | Verdict | Evidence (pass8/…) | Note |
|---|---|---|---|
| FIX-201 an app's name in Administration opens its package page | Match | fix201-selfhosted-*, fix201-selfhosted-click-*, fix201-apps-click-*, fix201-by-pkgid-*, fix201-unknown-*, fix201-unknown-crop | Administration › Apps › Self-hosted apps: both Sentry rows link `/admin/apps/packages/sentry`; a click lands on `/admin/apps/packages/<version uuid>`, "Sentry 2.3.1 … App package, version 2.3.1" (1280 and 390, both themes), no "Some of the values are not valid". The Apps tab links by version uuid as before and opens the package. `/admin/apps/packages/atlassian` typed directly redirects to the active atlassian version. `/admin/apps/packages/nosuchapp`: the shared empty state "No app package called nosuchapp is installed." with "Back to Administration › Apps" |
| FIX-202 a registered self-hosted variant is Ready and connectable | Match | fix202-app-step-*, fix202-after-jira-*, fix202-signin-*, fix202-ws-apps-*, fix202-ws-connect-* | With Jira Data Center registered (see Environment; Jira Cloud needs an admin on this cluster, so DC is the one usable choice): **Give Daneel an app** lists Jira under Ready · 12 (Confluence, Indico, Linear, Microsoft 365, Notion need an admin); the Jira tile goes straight to "What may Daneel do in Jira · Data Center?" and then to the Data Center sign-in ("Get a token in 2 steps … Open Jira Data Center … Personal access token"). Workspace **Apps & accounts**: the Jira card says "Ready to connect", **Connect** opens "Which agents can use Jira · Data Center?". 1280 and 390, both themes. GHES not registered (the same code path; a second real server was not needed) |
| FIX-203 Mattermost callback logging | Not applicable | — | No UI |

## Smoke of the neighbours (both themes)

- **Chat tab** at 390, 768, 900, 1024, 1280 for Carol's Daneel and Bob's Nokey (no key), plus every thread above: list, thread,
  composer, rail and back arrow as in pass 7 (thread 356/390/522/414/670 px; Send inside the composer; the list fills the
  card beside the rail at 768–1023 and alone at 390). smoke8-chat-*.
- **Apps & access** at the same widths: Carol's Daneel and Bob's Nokey (the starter-key card). No overflow, no address, nothing
  like "undefined" or "NaN". smoke8-access-*.
- Every pass-8 capture (202 rows in `_log.jsonl`): no horizontal scroll, no axe violation, no console error, no 4xx/5xx `/v1`
  response, no CSP report.

## Change list sent (batch 12, with the worker_done message)

1. **Lane A — Needs changes.** `web/src/components/chat/thread-header.tsx` (with `chat-thread.tsx` if needed). The meta line
   must be readable without hover at every width, and every header line must lie inside the header and the card:
   - at 768–960 the two-line cut hides the end of the visibility note ("…can see this" / "Only you and workspace") and, on an
     archived chat, "Archived <date> · read only" entirely; the tooltip is the only way to it, and tablets have no hover
     (b11-1-zoom-carol-d1-light-768, b11-1-zoom-archived-light-768);
   - at 1024–~1200 an archived chat's three-line meta line overflows the fixed 64 px header: the title's top is cut above the
     card, "Archived … · read only" lies over the thread (b11-1-zoom-archived-dark-1024);
   - at 390 an archived chat's meta line is an 89 px column of 8 lines beside Live, Restore and ⋯ (b11-1-archived-crop-light-390).
   The lane picks how, e.g.: below 1280 put the meta line on its own full-width row under the title/actions row (the header
   grows; the rail's header follows it so the borders stay in line, P5-POLISH-6); or a shorter visibility note that fits one
   line ("Private: you and workspace owners") with the full sentence reachable by tap (a disclosure/popover button, not
   `title`), and the archived state shown by Restore plus a one-word badge. No `title`-only information. Test (agent-header-widths
   or a chat spec, mocked): an archived direct chat and another member's chat at 390/768/820/900/960/1023/1024/1100/1280, both
   themes: every text line of the header inside the header box and the card, and the full visibility sentence and
   "Archived …"/"read only" visible (not clipped) or one tap away.

Not sent (further ideas, they break no `docs/14` rule):

- The workspace Apps & accounts Jira card still describes the family with the hosted variant's text ("Issues, projects and
  comments in Jira Cloud.") when the only usable variant is Data Center (fix202-ws-apps-light-1280).
- From pass 7, still open: below 1280 the Chat page scrolls 64 px (allowed by docs/14's 1280×800 rule); at 768 the agent
  Settings sub-navigation keeps 208 px beside the content.

## Counts (pass 8, the 119 findings)

**Match 119 · Minor 0 · Needs changes 0 · Not checked 0** (119).

- Outside the 119: b11#2 and b11#3 Match; b11#1 Needs changes (b12#1: the cut meta line hides who can see the chat behind a
  hover-only tooltip at 768–960, and an archived chat's header overflows at 1024–~1200). FIX-201 and FIX-202 Match.

So P5-REFINE-20's exit is **not** reached: the 119 are all Match, but one Needs changes outside them is open. The cluster stays
up.
