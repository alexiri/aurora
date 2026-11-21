# P5-REFINE-20 — pass 9 report (UX review 5 verification)

Brief: `docs/tasks/P5-REFINE-20.md`. Reference: <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>, local copy
`/home/alex/orca/workspaces/aurora/design-ux-review-5/` (`UXR5/`). Earlier passes: `REPORT-pass1.md` … `REPORT-pass8.md` (same
folder). The screenshots of this pass are in `docs/audit/ux-review-5-verify/pass9/` in the verification worktree. They are
**not in git**. `pass9/_log.jsonl` records, per capture, the final URL, console errors, 4xx/5xx `/v1` responses, horizontal
overflow, axe wcag2a/aa results and the step output (boxes measured in the page).

Verdicts as before: **Match**, **Minor**, **Needs changes**, **Not checked**. "bN#m" is item m of change-list batch N (batch 13:
this pass).

## Environment (2026-11-21, master `9fe25317`: batch 12, FIX-204, FIX-206)

- Cluster `aurora-r20` (:4720, SSH 127.0.0.1:2272), kept from pass 8. `git merge master` was a fast-forward to `9fe25317`.
  `AURORA_DEV_CLUSTER=aurora-r20 AURORA_DEV_PORT=4720 AURORA_DEV_SSH_PORT=2272 heavy.sh p5-platform deploy/dev/kind-dev.sh dev
  api web` (1121 s under load, exit 0; the API too, because `9e002f47` changed the settings service since pass 8).
- Nothing was written to the cluster in this pass (the dialogs were opened and closed without submitting).
- Never used `aurora-dev`, :4000 or the `aurora` DB.

## Batch 12 (re-check)

Threads: Alice's **archived** direct chat (Restore beside Live), **another member's chat** (Carol, workspace owner, reading
Alice's: "Only Alice Admin and workspace owners can see this conversation"), plus the neighbours Alice's direct chat, Carol's
own chat and Carol's schedule run thread. Widths 390, 768, 820, 900, 960, 1023, 1024, 1100, 1280, both themes (90 captures;
the dark and light measurements are identical, except two dark rows at 1023/1024 that were measured before the archived
thread's meta had loaded, re-measured with a wait in `b12-edge-*`).

| Item | Verdict | Evidence (pass9/…) | Note |
|---|---|---|---|
| 1 the meta line readable without hover, inside the header | Needs changes (**b13#1**) | b12-1-{archived,member,direct,carol-own,carol-sched}-{dark,light}-{390,…,1280}, b12-edge-* rows in `_log.jsonl`, b12-zoom-archived-light-1024, b12-zoom-archived-dark-390, b12-1-member-crop-dark-768 | **What works:** below 1280 the meta line is its own full-width row. The whole privacy note and "Archived 21 Nov 2026 · read only" are visible at every width (no text box clipped by an overflow ancestor, `scrollHeight` = `clientHeight`), and the header holds no `title` attribute. Every text line lies inside the header and the card (0 outside at 90 captures). The title row's border is level with the rail's: 217/217 at 768–1100, 153/153 at 1280. No text runs into the thread (`nextTop` ≥ header bottom). The archived header at 390 is 140 px (was 189 px with the 8-line column). The title truncates with an ellipsis beside Live/Restore/⋯ (115 px at 768, as in pass 8). **But** the new row's top and bottom borders, which are now the line between the title row and the meta row and the header's bottom line, stop short of the card's right edge. They end **24 px** short at 390 and **40 px** short at 768–1100, for every thread with a meta line (meta 289–639 vs header 289–679 at 768; 521–895 vs 521–935 at 1024). Cause: `thread-header.tsx`'s meta `<p>` has `w-full` plus `-mx-3 md:-mx-5`, so its box is the header's content width shifted left by the padding. Beside the rail at 768–1100 the gap shows between the line's end and the rail's border |

## Owner fixes since pass 8 (smoke at 390 and 1280, both themes)

| Fix | Verdict | Evidence (pass9/…) | Note |
|---|---|---|---|
| FIX-204 account Rename | Match | f204-rename-{dark,light}-{390,1280} | `Field` "Name" with the 6 px label gap and `border-line-strong`, like every other dialog field; focus in the field, text selected; Cancel and Save in the footer row. The footer's reserved one-line error slot (FormDialog, channels-2) sits above the buttons as in every FormDialog |
| FIX-204 account Replace | Match | f204-replace-* | `Field` "New value" (SecretInput with Show), Cancel and Replace in the footer |
| FIX-204 app upload | Match | f204-upload-* | Administration › Apps › Upload app: `Field` "App package", Cancel and Upload in the footer |
| FIX-204 bulk role | Match | f204-bulkrole-* | Acme › People, two people ticked (the "more controls" switch is on for Alice), Change role…: `Field` "New role" + `Select`. It is a `ConfirmDialog`, whose buttons sit at the end of the body like every other confirmation (not a docs/14 rule; FIX-204 left `ConfirmDialog` as is) |
| FIX-206 workspace Internet access, member (Carol) and approver (Dave) | Match, with a Scope-7 fix | f206-ws-member-*, f206-ws-approver-* | The summary sentence ("Agents here can reach any website. Nothing asks you first."), the who-to-ask notice and "Your rules". No Limits section, no "Set for you by", no Restrict further…. The empty "Your rules" still said "Requests follow the limits and defaults below", and nothing is below any more. **Fixed here** (Scope 7, see below) |
| FIX-206 workspace Internet access, owners (Alice of Acme team, Carol of her own) | Match | f206-ws-owner-*, f206-own-ws-owner-* | "Limits and defaults" with "Set for you by your organization's admins" and Restrict further… (something can still be tightened) |
| FIX-206 organization Internet access, member (Carol) | Needs changes (**b13#2**) | f206-org-member-{dark,light}-{390,1280} | The page is the description "Which websites agents can reach. Only owners and admins can change this." and the notice "Only organization owners and admins can change this. Ask an organization admin." Nothing says what is in force. `docs/14` §Scope ladder (D-444, written by FIX-206) says that a viewer who can change nothing, "an organization member" included, "reads what is in force in words and who to ask". The description's second sentence also repeats the notice |
| FIX-206 organization Internet access, owner (Alice) | Match | f206-org-owner-* | The page as before: posture, "Set above you" (the instance baseline), "Restricted further below", history |

Every pass-9 capture (148 rows in `_log.jsonl`): no horizontal scroll, no axe violation, no console error, no 4xx/5xx `/v1`
response, no CSP report.

## Fixed in this lane (Scope 7)

- `web/src/components/internet/internet-level.tsx` (`ReadOnlyRules`, shown only to members and approvers): the empty state
  "No rules yet. Requests follow the limits and defaults below." pointed at the Limits section FIX-206 removed. It now reads
  "No rules yet. The sentence at the top says what agents here may reach." (the summary sentence is in the page header,
  rendered from the same read). The owner's own empty state ("Until you add one, requests follow the limits and defaults
  below.") is unchanged; owners do have that section below. Test: `web/tests/internet-pages.test.tsx`, a new case "(D-444) a
  member with no published rules is not pointed at limits below". The cluster still runs the old text; pass 10 sees it.
  - `heavy.sh p5-platform pnpm exec vitest run tests/internet-pages.test.tsx tests/policy.test.tsx`: 2 files, 36 tests passed.
  - eslint on both files: clean. `pnpm typecheck`: clean. `scripts/guards/run-all.sh`: all guards passed.
  - No other test sees the old text (grepped `web/tests`, `web/e2e` (mocked and production), `scripts/e2e`).

## Change list sent (batch 13, with the worker_done message)

1. **Lane A — Needs changes.** `web/src/components/chat/thread-header.tsx`. Below 1280 the meta row's top and bottom borders
   must run edge to edge of the header (the card's inner edge on the left, the rail's border or the card's edge on the right),
   like the title row's border above 1280. Today they end 24 px short at 390 and 40 px short at 768–1100 (`w-full` with
   `-mx-3 md:-mx-5`: the box keeps the content width and only moves left). Evidence: b12-zoom-archived-light-1024,
   b12-zoom-archived-dark-390, b12-1-member-crop-dark-768; the `b12-edge-*` rows (`shortRight` 24/40). The lane picks how,
   e.g. `basis-full grow` instead of `w-full` (the negative margins then add the padding back; check it still wraps onto its
   own line), or the header without horizontal padding and the padding on its rows. No arbitrary values. Test
   (agent-header-widths, mocked): below 1280 the meta row's left and right edges equal the header's at 390/768/1023/1024/1100,
   in both themes, for the archived chat and another member's chat. Keep everything batch 12 achieved (full text visible, no
   `title`, every line inside the header and card, 217/217 and 153/153).
2. **The lane that owns FIX-206 (lane B, or any free web lane) — Needs changes.** `web/src/components/internet/org-internet.tsx`,
   the `!admin` branch. An organization member must read what is in force in words, as `docs/14` §Scope ladder (D-444) says
   and as the workspace page does for its members: one summary sentence for the organization (e.g. with `internetSummary`,
   "Agents in Acme can reach any website. Nothing asks you first."), from a read a member may make. The lane decides which,
   e.g. the effective governance FIX-206 stopped reading for members (it is readable; only its "Set for you by" rows are
   gone) or a member route like the workspace's. Drop "Only owners and admins can change this." from the description, which
   the notice already says. Test: `web/tests/internet-pages.test.tsx` "(D-444) an organization member…" asserts the
   sentence and still no Limits/"Set for you by"; adjust its "effective limits are not read" assertion to the chosen read.

Not sent (further ideas, they break no `docs/14` rule):

- When the meta line wraps, a segment can start its line with its separator ("· Archived 21 Nov 2026 · read only" under the
  privacy note at 390–1100).
- `ConfirmDialog` keeps Cancel and the action at the end of its body while `FormDialog` pins them in a bordered footer; a
  confirmation with a field (bulk role) looks slightly different from a form dialog.
- From pass 8, still open: the Jira family card keeps the Cloud description when only Data Center is usable; below 1280 the
  Chat page scrolls 64 px (allowed by docs/14's 1280×800 rule); at 768 the agent Settings sub-navigation keeps 208 px beside
  the content.

## Counts (pass 9, the 119 findings)

**Match 119 · Minor 0 · Needs changes 0 · Not checked 0** (119; none of them touched by batch 12, FIX-204 or FIX-206 except
through the items above).

- Outside the 119: b12#1 Needs changes (b13#1: the meta row's borders stop short of the card's edge below 1280). FIX-204 Match
  (four dialogs). FIX-206 Match for workspace members, approvers and owners and for organization owners (the member's empty
  state fixed here). FIX-206 Needs changes for an organization member (b13#2: nothing says what is in force).

So P5-REFINE-20's exit is **not** reached: the 119 are all Match, but two Needs changes outside them are open. The cluster
stays up.
