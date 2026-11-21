# P5-REFINE-20 — pass 1 report (UX review 5 verification)

Brief: `docs/tasks/P5-REFINE-20.md`. Reference: <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>, local copy
`/home/alex/orca/workspaces/aurora/design-ux-review-5/` (`UXR5/`). Mockup of a finding: `UXR5/after/<key>.png` (dark) and
`UXR5/after/<key>-lt.png` (light). Screenshots of this pass: `docs/audit/ux-review-5-verify/pass1/<name>-<theme>-<width>.png` (282 files, 8.5 MB, kept in the verification worktree `/home/alex/orca/workspaces/aurora/p5-platform` and its commit 354a0195 on `alexiri/p5-platform`, not on master: review screenshots stay out of the repository's history, as with UX reviews 4 and 5)
(pngquant-compressed; `capture-log.jsonl` beside them has, per capture, the final URL, console errors, CSP violations,
4xx/5xx `/v1` responses, horizontal overflow and axe wcag2a/aa results).

Verdicts: **Match**, **Minor** (looks or reads slightly different), **Needs changes** (missing, wrong, broken, or a rule
broken), **Not checked** (not exercised in pass 1; carried to pass 2 with the reason). "Fixed (b1)" means the item was sent back in
batch 1, fixed on master and **re-checked on the rebuilt stack** in this pass.

## Summary

- **Environment:** throwaway cluster `aurora-r20` on `http://localhost:4720` (SSH NodePort 127.0.0.1:2272), built with
  `deploy/dev/kind-dev.sh up --llm-mock --connector-fake --ssh` from this worktree. Commits walked: `6ec943c0` (includes
  P5-REFINE-9 and its items, c2465af5), then `4fad882a` (P5-REFINE-19 merged), then **`82f47c39`** (batch-1 fixes). Batch-2 fixes
  (master `fb1d6258`, `08051118`) are **not** on the stack yet. Never used `aurora-dev`, port 4000 or the `aurora` DB.
  **The cluster is kept up between passes** (the Orchestrator asked for live re-checks); it is torn down after the last pass.
- **Data created as the review did:** Alice instance admin (bootstrap), placement `shared_org` "Acme" with personal workspaces;
  Bob and Carol by development sign-in; Company key (LiteLLM mock) as the starter key, given to the 3 existing agents; Bob's
  personal keys; extra agents Counter (`scn-count`) and Grumpy (`scn-ratelimit`, then `scn-nope`) in Default; a calendar link
  (Google US holidays), two outgoing webhooks (httpbin), a schedule "Ping every 5", an incoming webhook "Deploy notice" called
  with curl (good, duplicate, bad signature); Bob's Jira and Notion asks, Carol's Mattermost ask; Mattermost added in
  Administration; a memory, a skill, a test question and run; files uploaded on Carol's computer. Not created: an SSH key, a
  second organization, an invitation, a sign-in app, a skill library, a pending **approval** (the LLM mock cannot make an agent ask
  and there is no fixture route; see G1/G5).
- **Change lists sent to the Orchestrator** (Orca message channel; SendMessage to the Orchestrator session was held by its
  permission mode and expired, so every batch went through `orca orchestration send`):
  - batch 1, 16 items, ~02:25 (msg_d3b2ffe58ac8): routed 1–8 lane A (merged 993ffaa2), 9–14 lane B (merged 9abb2281, 97940131), 15–16 to me;
  - batch 2, 18 items, ~03:30 (msg_055cf098415e): routed 1–10 lane A (merged fb1d6258), 11–18 lane B (merged 08051118);
  - batch 3, 16 items + batch-1 re-check, ~04:15 (msg_aa505ab13469): routed to lanes A and B, in progress;
  - batch 4 (the items new in this report, below) is sent with the worker_done message.
- **Scope-7 fixes in this lane** (`alexiri/p5-platform`, merged by the Orchestrator as `aadf9e27`):
  - `7c5b3d55` — retired words in visible text: "Add LLM key"/"New LLM key" → "Add AI key"/"New AI key" (`credentials/llm-form.tsx`),
    model-readiness "No LLM key is granted…" → "This agent has no AI key. Give it one below, or add a key under Apps & accounts."
    and list label "AI keys this agent uses" (`agents/model-readiness.tsx`), "This agent has an AI key, but Aurora could not pick a model
    for it." (`lib/chat/transcript.ts`), "AI endpoint"/"AI key" in `lib/api/errors.ts`, "Grader saved and tested…"/"Grader saved, but
    the test failed: … Questions the grader checks will not be scored until it works." (`agents/evals/copy.ts`), "Instructions page"
    (`agents/files-tab.tsx`; D-407 renames Behaviour everywhere, over brief 15's and the mockup's literal "Behaviour page"), and
    `e2e/altitude.spec.ts` asserting the heading "Tests" exactly instead of `/Tests|Evals/` (H4). Tests changed: e2e `accounts`,
    `altitude`, `chat-v2`, `credential-access`, `first-run`, `onboarding-journeys`, `p5-polish-10`, `welcome-summary`; vitest
    `chat-v2`, `credential-access`, `credentials`, `evals-page`, `files-page`, `llm-key-form`, `onboarding`. Runs: vitest 10 files
    168 passed; `tsc --noEmit` clean; eslint on the touched files clean; `scripts/guards/run-all.sh` all passed.
  - `517de2d4` — Tests page "Current judge:" → "Current grader:" (`agents/evals/judge-section.tsx`); `evals-page.test.tsx` 21 passed.
- **Counts (pass 1, the 119 findings):** see the table; Match 68 · Minor 23 · Needs changes 14 · Not checked 14 (119)
  (the 14 are listed in "Carried to pass 2"). Of the findings sent back in batch 1 and re-checked here, 5 are fixed.

## A. The 119 findings

| Key | Owner | Verdict | Evidence (pass1/…) | Note / change list ref |
|---|---|---|---|---|
| setup-1 | 5/18 | Minor | setup-1-*, p2-setup-* | Try it ticks itself and has Open chat (fixed b1#2 partly); row still "Your agent answered Alice Admin 1 h ago" (target "Daneel answered …"). b3 re-check |
| setup-2 | 5/18 | Needs changes | setup-2-added-*, setup-2-*, setup-nokey-* | b1#1: no toast, no "Given to …" line, stale checklist; key verification never leaves "pending" (evidence in b3). No-key text b1#3 |
| setup-3 | 18 | Match | setup-3-dark-1256 | Open chat → own agent's chat with "Hello Daneel! What can you help me with?" prefilled, not sent (opens Alice's personal-workspace Daneel) |
| setup-4 | 5/18 | Match — fixed (b1#6) | p2-setup-health-* | "has not been asked yet"; Web Push in words with Technical details |
| setup-5 | 8/18 | Match — fixed (b1#4) | p2-setup-*, setup-5-* | one Needs you list; Overview/Chat platforms/Administration counts |
| setup-6 | 18 | Match — fixed (b1#7) | p2-email-*, p2-setup-6-* | label, example, error text, no page-top banner, sentence fixed ("Dashboard" source chip remains, unexplained) |
| setup-7 | 2/18 | Minor | setup-7-dialog-*, setup-7-step2-* | b1#9 (error on arrival, help placement, "L(" initials) — fixed on master 9abb2281, re-check pass 2 |
| setup-8 | 8/18 | Match | setup-8-* | Decline + "Message for Carol (optional)" |
| firstday-1 | 1 | Minor | bob-first-dark-1256 | no What moved ✓; title "Welcome, Bob's workspace" (b1#12, fixed on master, re-check pass 2) |
| firstday-2 | 16 | Not checked | — | phone switcher panel not opened (drawer only) |
| firstday-3 | 16 | Not checked | — | needs a personal key still being checked on a fresh Welcome |
| firstday-4 | 16 | Not checked | — | tour not started |
| firstday-5 | 16 | Match | bob-first-dark-1256 | "It can reach websites; you can limit that." with the link |
| chat-1 | 7/11 | Match | chat-1-*, chat-1-toast-* | leaves Active (also after reload), Archived read-only + Restore, Undo toast, no composer |
| chat-2 | 11 | Match | chat-2-dark-1256 | after a 60-line message with 2 attachments: at the bottom, no Jump to latest |
| chat-3 | 11 | Minor | chat-3-*, chat-3-menu-* | New chat, fresh history, auto-title, Rename/Archive/Delete menu ✓; titles cut to one line (b2#10, fixed on master) |
| chat-4 | 7/11 | Minor | chat-4-* | partial kept, "You stopped this answer", header Ready in 0.3 s; list row stays "Working" (b2#8) |
| chat-5 | 11 | Match | behaviour-instr-* | labelled Settings button, Settings page with sub-nav and Pause/Delete zone |
| chat-6 | 11 | Match | chat-6-* | panel overlays at 1256; composer one row; Needs you · Doing now · What Daneel did |
| chat-7 | 11 | Match | chat-7-* | one red box "Something went wrong. Try again." + Details |
| chat-8 | 11 | Match | chat-8-dark-1256 | one row, Aa, preview grows |
| chat-9 | 11 | Minor | chat-9-dark-1256 | title, context sentence, "Daneel: pong", date ✓; titles truncated (b2#10) |
| chat-10 | 11 | Match | chat-2-dark-1256 | image thumbnail "121 B · Image", file card for notes.txt |
| chat-11 | 11 | Match | chat-7-dark-390 | phone opens the thread with the composer; Conversations back control |
| behaviour-1 | 12 | Match | behaviour-1-* | one box, optional title, list by first line |
| behaviour-2 | 12 | Minor | behaviour-2-* | b2#5 (list row prefix, switch label, Advanced placement) |
| behaviour-3 | 12 | Match | behaviour-3-* | ANSWER help in the dialog; "Daneel's reply had no ANSWER line, so Aurora could not compare it." |
| behaviour-4 | 12 | Minor | behaviour-4-* | tested on save, live "Running · 0 of 1 done", Re-run per row ✓; "Ready" after a failed test, "Queued" chip (b2#3) |
| behaviour-5 | 12 | Minor | behaviour-instr-* | Advanced at Expert ✓; two help lines (b2#6) |
| behaviour-6 | 2/12 | Needs changes | behaviour-6-* | save bar ✓ but no "Saved" after Save (b2#1, fixed on master) |
| behaviour-7 | 12 | Minor | behaviour-3-result-* | words ✓ (+ Scope-7 "grader" fixes); run title "Nov 21, 2026, 2:54 AM" not formatWhen (b2#4) |
| behaviour-8 | 12 | Match | behaviour-instr-* | Change picture + "PNG, JPG or WebP, up to 512 KiB. Shown round." |
| access-1 | 6/13 | Match | access-1-*, access-1-done-* | send-only levels; "Daneel can now send to Ops alerts 2, and asks you first." |
| access-2 | 13 | Minor | access-11-*-390 | title full width; select and menu on two extra lines (b2#14) |
| access-3 | 5/12 | Needs changes | access-3-*, access-3-chat-*, access-3-give-* | header action + chat card + one-click give ✓; agent name vanishes from the 1256 header, state clipped at 390, no Access-page card ("Choose a model" link instead) — **b4#1** |
| access-4 | 13 | Needs changes | access-4-*, access-1-done-* | flat wizard ✓; Done's Send a test refuses templates with values (b2#11) |
| access-5 | 13 | Match | access-5-* | red result, plain sentence, body under Details |
| access-6 | 13 | Minor | access-6-* | "docs.github.com" (b2#13) |
| access-7 | 13 | Match | access-7-* | Ready · 8 / Needs an admin · 9 + hint |
| access-8 | 5/13 | Needs changes | access-8-*, p2-access-8-* | typed key kept ✓; error twice; stays "Checking…" (b2#12 + b3 evidence) |
| access-9 | 13 | Match | setup-7-dialog-* | "Which service is this key for?" cards (initials b1#9) |
| access-10 | 13 | Match | access-10-dark-1256 | "Checking…" + "Fetching the calendar link." |
| access-11 | 13 | Match | access-11-* | per-app menus |
| access-12 | 13 | Match | access-5-*, access-ics-after-* | a level select on every app row |
| schedules-1 | 14 | Match | schedules-1-* | presets, time, workspace zone, summary, Advanced |
| schedules-2 | 14 | Needs changes | schedules-2-dark-1256 | no next three runs for Every 5 minutes (b2#16) |
| schedules-3 | 7/14 | Match | schedules-3-* | "Started · Open the conversation"; Runs with outcome and thread links |
| schedules-4 | 1/7/14 | Match | schedules-3-runs-* | On, "Every 5 minutes · next at 02:15 (UTC time)" (zones b2#18) |
| schedules-5 | 14 | Match | schedules-1-* | zone select, gate/anchor under Advanced |
| schedules-6 | 14 | Match | schedules-6-menu-*, schedules-3-runs-*-390 | Run now + Edit + menu; Runs visible on the phone |
| schedules-7 | 7/14 | Not checked | — | rename/delete of a schedule not done |
| webhooks-1 | 14 | Match | webhooks-1-dark-1256 | How to call it with curl/Python/Node |
| webhooks-2 | 1/7/14 | Needs changes | webhooks-2-dark-1256 | "Dani" hard-coded (b2#17) |
| webhooks-3 | 7/14 | Match | webhooks-3-* | When · Result · Size · Conversation in words |
| webhooks-4 | 7/14 | Minor | webhooks-4-* | "03:11 AM", extra chip; axe color-contrast ×1 (b2#18) |
| webhooks-5 | 2/14 | Match | webhooks-5-* | three consequence dialogs |
| webhooks-6 | 14 | Match | carol-automations-*, webhooks-1-* | same frame; one-time box fits |
| channels-1 | 10/12 | Not checked | — | Other agents link flow not exercised |
| channels-2 | 5/12 | Minor | channels-2-* | footer clipped (b2#7, FormDialog pinned footer on master) |
| channels-3 | 1/8/14 | Needs changes | channels-3-*, auto-new-* | no Ask an admin for a chat platform (b1#10, fixed on master 9abb2281, re-check pass 2) |
| channels-4 | 12 | Match | channels-4-* | "Counter is ready", opens chat, first message drafted |
| computer-1 | 4/15 | Not checked | — | long command + idle sleep not exercised |
| computer-2 | 2/15 | Not checked | — | runner-bug / retry-exhaustion path not exercised |
| computer-3 | 15 | Match | file-list.tsx:118, :154–162 | Move to… and drag onto folders (code + menu) |
| computer-4 | 15 | Match | computer-4-* | "Leave without saving?" dialog |
| computer-5 | 2/15 | Match | computer-5-*-390 | stacked rows, menu in its own column |
| computer-6 | 4/15 | Match | computer-6-* | "aurora ssh daneel", stable file name, id behind Copy |
| computer-7 | 15 | Minor | computer-files-* | member sees no image/runner ✓; "Started 03:20 · off" (b3#12) |
| computer-8 | 15 | Match | computer-8-light-1256 | readable light prompt |
| computer-9 | 15 | Match | computer-9-* | own files grouped and explained (Scope-7 word fix) |
| computer-10 | 15 | Match | computer-10-* | upload toast, no shifting |
| computer-11 | 15 | Match | computer-11-* | CSV table view |
| computer-12 | 15 | Match | computer-12-*-390 | phone key row; no Open in new window |
| computer-13 | 4/15 | Minor | computer-6-*, computer-9-* | Sleep now/Restart ✓; "sleeps in 0 s when idle" (b3#13) |
| workspace-1 | 16 | Match | workspace-1-* | "Allowed. No rule stops it, so it goes through." (title repeated in the dialog) |
| workspace-2 | 16 | Needs changes | ws-activity-dark-1256 | technical table overflows its card (b3#7) |
| workspace-3 | 16 | Needs changes | workspace-3-* | e-mails, "did something" fallbacks, dotted placeholder (b3#8) |
| workspace-4 | 16 | Minor | workspace-4-* | "The person can open…", no Approver in the select (b3#10) |
| workspace-5 | 16 | Not checked | — | promotion to Owner in a team workspace not done (org-9 checked) |
| workspace-6 | 16 | Match | workspace-6-* | what a key can do + docs link |
| workspace-7 | 2/16 | Minor | workspace-7-* | switcher updates ✓; no Saved line; "Language Grumpy remembers in" (b3#9, b3#6) |
| workspace-8 | 2/16 | Match | workspace-8-* | visible name typed |
| workspace-9 | 16 | Match | workspace-9-* | grid, search, two groups (reason: b1#11, fixed on master) |
| workspace-10 | 16 | Match | ws-skills-dark-1256 | lead sentence, How skills combine |
| workspace-11 | 16 | Match | ws-approvals-dark-1256 | one empty state + Example card |
| workspace-12 | 2/16 | Match | access-page-* | skeleton rows |
| org-1 | 6/17 | Match | org-1-dark-1256, org-chat-* | Ready · connector running · Connect; Carol's ask with Connect Mattermost |
| org-2 | 17 | Minor | org-2-dark-1256 | Set up → Jira page (URL and page agree); "Register oauth client" link (b4#2) |
| org-3 | 2 | Needs changes | org-3-*, org-new-* | fake org shell + request storm (b3#1) |
| org-4 | 6/17 | Not checked | org-people-* | no invitation created (e-mail-off line ✓) |
| org-5 | 17 | Match | org-5-* | New organization in the switcher |
| org-6 | 6/17 | Match | org-agents-* | agents by workspace, last use, state |
| org-7 | 17 | Needs changes | org-ai-* | preset "Open" vs "People may not add their own"; limits first (b3#2) |
| org-8 | 17 | Minor | org-internet-* | Your rules / Set above you ✓; preset above the h1 (b3#3) |
| org-9 | 17 | Match | org-9-* | promotion confirm with roles |
| org-10 | 6/17 | Not checked | org-10-* | no sign-in app to Test (tab and words ✓) |
| org-11 | 17 | Minor | org-11-* | "They gets …" (b3#4) |
| org-12 | 17 | Minor | org-12-* | precedence paragraph first (b3#5); Connect GitHub path not exercised |
| org-13 | 2/17 | Match | org-13-*-390 | stacked cards at 390 |
| org-14 | 6/17 | Not checked | org-internet-* | no rule change made; only "Someone published version 1" |
| admin-1 | 5/18 | Not checked | — | key-origin switch save not exercised |
| admin-2 | 2/18 | Match | setup-6-*, p2-setup-6-* | save bar at the bottom, "1 field needs fixing" |
| admin-3 | 18 | Not checked | — | widening change confirm not exercised |
| admin-4 | 18 | Match (partial) | p2-setup-6-* | field-level message in words; other fields not exercised |
| admin-5 | 6/18 | Needs changes | admin-5-* | no Next line; host list contradicts status (b3#14) |
| admin-6 | 18 | Match | adm-chat-* | platforms first, Advanced |
| admin-7 | 18 | Match | admin-7-* | plain summary first |
| admin-8 | 18 | Match | admin-8-* | two steps |
| admin-9 | 18 | Not checked | — | terminal is on (values-ssh); off-text not visible |
| admin-10 | 18 | Needs changes | adm-audit-* | e-mails, fallbacks, mm/dd/yyyy (b3#8) |
| admin-11 | 2/18 | Match | adm-apps-* | A–Z, search, plates |
| admin-12 | 6/18 | Minor | admin-12-* | sentence shown twice (b3#15) |
| admin-13 | 18 | Match | adm-people-* | display names, Manage, Roles from sign-in: none |
| admin-14 | 6/18 | Match | adm-storage-* | storage categories (rate limits page not opened) |
| account-1 | 16 | Match | acct-notif-* | no blocked notice until asked (console 404 /v1/push/config, minor) |
| account-2 | 16 | Match | acct-keys-* | Create key |
| account-3 | 16 | Match | acct-chat-* | "No chat platform is connected yet. An admin can connect one." |
| account-4 | 16 | Match | acct-profile-* | rail entries, profile only |
| account-5 | 16 | Match | account-5-* | ed25519 message on paste |

## B. The 10 priorities

| # | Verdict | Notes |
|---|---|---|
| 1 Eleven broken things | Needs changes | fixed: access-1, chat-1, setup-1, access-3 (give key works), org-1, chat-2, access-2 (Minor); still broken: org-3 (fake org + request storm); not checked: admin-1, computer-1/2 + runner bug, org-2 tab switch (Set up now opens the app page) |
| 2 One way to save | Needs changes | save bar + "n fields need fixing" ✓; no "Saved" after saving Instructions or the workspace name (behaviour-6, workspace-7) |
| 3 Every action says what happened | Minor | Run now, Runs, deliveries, Stop mark, test progress ✓; Done's Send a test refuses templates (access-4) |
| 4 People's language | Needs changes | exemption lists empty ✓; dotted keys still visible: "Action, e.g. workspace.update", "llm.endpoints.aigw-mock.roles", e-mails and "did something (“eval run start”)" in audits; "Register oauth client"; raw "member"/"english" select values |
| 5 Webhooks and schedules without a manual | Needs changes | presets, Signed + Generate, How to call it, Deliveries ✓; next three runs missing (schedules-2) |
| 6 Requests list | Match | feeds per level, reasons, Decline with message, history, own request uncounted; counts re-route live when the resolving level changes |
| 7 People by name | Needs changes | display names ✓ in shells and lists; audits use e-mails; personal workspace named three ways |
| 8 New chat | Match | New chat, auto-title, Rename, runs folded under automations |
| 9 Invitations, organizations, roles | Minor | New organization ✓, promotion confirm ✓, rotate confirms ✓; copy invitation link not exercised |
| 10 Newcomer's agent just works | Minor | starter key to all agents ✓, give-key header action ✓, Runs on Aurora default ✓; Access page card missing (access-3), key verification hang (setup-2) |

## C. The runner bug (`#bug`)

**Not checked in pass 1.** The stack was installed with the terminal on (`values-ssh.yaml`), so the runner registered with it on.
Pass 2 toggles the terminal setting off and on in Administration without restarting the runner and checks Terminal, Files and
SSH, the "Couldn't connect to the computer" end state, and the restart notice. Seen: an open terminal works; the terminal page
says "An open terminal keeps the computer awake, so a running command is never cut off." (computer-1 wording).

## D. Themes

| Theme | Verdict | Notes |
|---|---|---|
| Stale live state | Minor | Working clears ~4 s after the answer; list row Working after Stop; badges for admin/org every 5 min (G4); key "Checking…" forever |
| Several names for one thing | Minor | renames in (+ Scope-7 fixes); left: "Register oauth client", "Judge" in comments only, personal-workspace names |
| Tables that don't fit | Needs changes | Activity technical table overflows its card at 1256 (workspace-2); 0 horizontal page overflow in 278 captures at 390/1256 |
| One-time secrets | Match | webhook address/secret box with Copy and the warning |
| Five date formats | Minor | "Nov 21, 2026, 2:54 AM" (test run), "03:11 AM" (delivery), "mm/dd/yyyy" date filters |
| Destructive and privileged actions | Match | ladder: typed name for workspace delete, consequence dialogs for rotate/secret/delete, promotion confirms |
| Contrast and colour meaning | Minor | Critical red ✓, plates ✓; axe color-contrast serious ×1 on the webhook delivery message (dark) |
| Context gets lost | Needs changes | /o/new: no 404 inside the shell (org-3) |
| What the agent did on its computer | Match (partial) | "What Daneel did" rows in the rail; computer steps not exercised (mock does not run commands) |
| Pages and dialogs jump | Minor | dialogs top-anchored ✓, skeletons ✓; New agent footer clipped (fixed on master) |

## E. The layer above (recs 1–16)

| Rec | Verdict | Notes |
|---|---|---|
| 1 Altitudes | Minor | switch, "n more controls hidden · Show", server-stored ✓; Bob at Use missed Memory/Skills (b2#2, fixed on master) |
| 2 Vocabulary | Minor | see Themes "Several names" |
| 3 Agent page | Minor | tabs + Settings + rail headings ✓; Needs-you strip label (b2#9, fixed); header loses the name with the action (b4#1) |
| 4 One timeline | Match (partial) | Activity sentences, rail What Daneel did; live updates not timed |
| 5 No blank forms | Match | templates in New agent, schedule presets |
| 6 Presets | Needs changes | instance Open default ✓; org AI models contradicts its own preset (org-7) |
| 7 One Needs you | Match | same row shape at workspace, organization, Administration |
| 8 Manage Acme | Match | area with sub-nav and back link |
| 9 Administration halves | Match | Policy for everyone / This installation, Set in Helm |
| 10 Teach approvals | Match | Example card; first-answer card |
| 11 Computer as a place | Needs changes | opens on Details instead of Files (b3#11) |
| 12 Automations list | Match | one list, filters, runs link to threads |
| 13 Copy system | Minor | guards empty; leftovers as Priority 4 |
| 14 Expert speed | Match | Ctrl K palette, Select all, Copy as curl, CLI fold, export/import |
| 15 Stop ladder | Match | Pause/Resume, Delete with consequences |
| 16 Blind spots | Minor | Tests at Expert ✓, Came with this workspace ✓; "Memories … written in English" banner outside Expert; org Skills precedence first |

## F. Decisions D-405..D-419

D-405 this loop (in progress) · D-406 Match · D-407 Minor (leftover words above) · D-408 Match (Settings button, tabs, state line) ·
D-409 Match · D-410 Needs changes (org-7) · D-411 Not checked (channels-1) · D-412 Match (member sees no image/runner) ·
D-413 Match · D-414 Minor (no-key texts; setup-2 verification) · D-415 Match (partial) · D-416 Not checked (bug) ·
D-417 Match · D-418 Not checked (no invitation) · D-419 Match.
Owner comments: workspace Apps grid with search and groups — Match; schedule presets from Every 5 minutes with the hint — Match,
custom too-frequent flag not exercised.

## G. Count badges

| Badge | Verdict | Measured |
|---|---|---|
| G1 Approvals | Not checked | no pending approval can be produced (mock); zero state: no badge ✓ |
| G2 Overview | Minor | own request listed and not counted ✓; zero: no badge ✓; an agent that can't answer is not counted (brief 16 lists it) — **b4#3** |
| G3 Organization | Minor | "Manage Acme 2/3" badge, Overview and Apps & accounts/Chat platforms counts ✓; refetch every 5 min (b1#8, fixed to 30 s on master) |
| G4 Administration | Minor | sidebar foot, account menu, Overview and Chat platforms ✓, "1 needs your attention" inside the link ✓; appeared after **289 s** without a reload, decline not reflected in 90 s (b1#8, fixed on master, re-check pass 2); re-routes live to the org when Mattermost is added ✓ |
| G5 Agent Needs you | Not checked | needs an approval (Needs-you label shows for approvers only, b2#9 on master) |
| G6 Phone | Match (partial) | drawer shows the same badges; hamburger dot not verified |
| Promised earlier | — | RFE-012 (G1) and FIX-167 Part B (G4) — G4 worked (account-menu only) before the briefs; G1 not exercised |

## H. Tests moved with the UI

- **H1 — Needs changes (history).** Master's GitHub CI `web` job was red on most P5-REFINE merges 2026-11-20: runs 44726223286
  (a7be0ccf, brief 1), 44735084148 (d6915f19, brief 3), 44735963615 (e7a62ee1, brief 5), 44736737366 (6cbf3a12, brief 2),
  44742541818 (ab7460fe, brief 4), 44763275081 (64f5cd7b, brief 12), 44772677737 (81a117c3, brief 16), 44773677226
  (2243097b, brief 18), 44783526853 (2a0278a8, brief 6), 44804689298 (0b5c46f1, brief 15), 44810051853 (b45695a5, brief 13),
  44813202986 (51e26256, brief 18: ia-3, ia-navigation, agent-guide, fix-027-shots), 44821813375 (58c97b20, brief 16:
  altitude, ia-navigation, other-agents), 44823250244 (1eb05e1c, brief 11), 44824934229/44825277725/44825337998 (briefs 17/18,
  SCHED-ONCE, 17: p5-polish-8-switcher). The CI Watcher fixed them on `alexiri/ci-watch` (owner's rule of 2026-11-20 evening);
  the latest completed run on master before this pass (44835421666, d7a17d57) failed only on a Mattermost proptest (FIX-197).
- **H2 — Match.** `AURORA_E2E_PROD=1 pnpm exec playwright test e2e/{dialogs,files,terminal,markdown,web-push}.prod.spec.ts`
  through heavy.sh on 82f47c39 + 7c5b3d55: **13 passed (2.0 m)**.
- **H3 — Match (sampled).** No `.skip`, `fixme` or `.only` added in `1e1fef73..HEAD`. Deleted: `renames-notice.test.tsx` (feature
  removed, brief 1) and `type-to-confirm-delete.test.tsx` (brief 2, replaced by the confirm ladder). 99 test titles removed
  without the same title re-added; the sample (certificate, unsaved-leave, freeze, menu) were renamed with the new words.
- **H4 — Match after fix.** `e2e/altitude.spec.ts:91` accepted `/Tests|Evals/` (brief 3) — fixed in 7c5b3d55. Other hits of the
  grep are deliberate (BYO is not retired by D-407; server-message stubs). `agent-page-header.test.tsx:140-141` pinned the BYO
  header's raw "byo" + "Default" (sent as b1#13; one chip on master).
- **H5 — Match.** Every completion report of briefs 1–19 has the tests-changed / commands section.
- **Exemption lists (acceptance 6) — Match.** `VOCABULARY_EXEMPT = {}`, `OTHER_EXEMPT = {}` (`web/tests/ui-text.test.ts:121,127`),
  `COPY_EXEMPT = {}` (`web/tests/copy-guard.test.ts:44`).

## The 14 visual-design rules on the pages visited

278 captures: **0** horizontal page overflow (390 and 1256), **0** CSP violations, axe serious/critical: only color-contrast ×1
on webhooks-4 (dark). Console errors: `/o/new` request storm (345 × 404, 33 × 429 on `/v1/resolve`, 18 × 429 on
`/v1/attention/counts` — the storm also rate-limits the badges); 36 × 403 `GET /v1/orgs/{org}/accounts` on every agent Apps &
access page for non-org-admins (b2#15, fixed on master); 404 `/v1/push/config` on Account › Notifications with push off.

## Change list

Batches 1–3 are quoted in the Orchestrator messages listed in the Summary; their items are referenced per row above (b1#n,
b2#n, b3#n). **Batch 4 (new in this report):**

1. **P5-REFINE-12/11 — access-3 [flow].** With "Give Daneel the Company key" in the header, the 1256 header drops the agent's
   name (only "@daneel · Can't answer: no AI key [Give Daneel the Company key]"); at 390 the state text runs under the Status
   button ("Can't answ… ey"). Settings › Apps & access has no top card "Daneel can't answer yet: it has no AI key. Your
   organization's Company key goes to every new agent automatically, and Daneel was created before it was set." with "Give
   Daneel the Company key" and "Choose another" (mockup access-3, brief 12 #7); it shows a red line "Can't answer: no AI key is
   given to this agent" whose link reads "Choose a model" (a model is not the fix) and the collapsed "Not given to Daneel (1)".
2. **P5-REFINE-17/13 — org-2 [words].** The Jira app page reached from Set up shows a second link "Register oauth client"
   (retired word, lowercase) next to "Register a sign-in app for Acme".
3. **P5-REFINE-16/8 — G2 [flow].** An agent that can't answer (no AI key) is not a workspace attention item (count 0), though
   brief 16 lists "agents that can't answer" among the items the Overview counts.
4. **P5-REFINE-15 — computer-7 / D-412 [words].** (also b3#12) the Details line combines "Started 03:20 · off".

## UX review 4 re-check (Broken / Confusing flow / Rough edge)

Checked by theme on the pages walked (the review's 100 tagged items, `design-ux-fixes/review/index.html`; the brief's
`verify/REPORT.md` does not exist in that folder):
- **Still fixed:** dialogs no longer hide their first line; primary links render; "Ready to connect" lists only connectable apps
  (two groups); AI-key dialog is one pattern; organization "Ask alice" texts are gone (role words); role select labels are
  capitalised in People (except the workspace Add access select, "member"); invite dialog no longer offers personal workspaces;
  setup checklist no longer embeds the placement form; Health reports real calls; empty Approvals explains itself; Activity rows
  are sentences; caps chip soup reduced to one chip per row on the pages walked; model plumbing moved under Advanced.
- **Regressed / still open:** "Administration" appears twice in the Administration rail (rail heading and the foot entry, as in
  UX4); the org AI models page still opens with 7 "Set by an Aurora administrator" rows before the summary (UX4 "2,300 px of
  plumbing", org-7); a member's 403s on Apps & access (fixed on master); "mm/dd/yyyy" date inputs in Administration audit.

## Carried to pass 2

Not checked: firstday-2, -3, -4; schedules-7; channels-1; computer-1, -2 and the runner bug (C); workspace-5; org-4, -10, -14;
admin-1, -3, -9; G1 and G5 (need an approval: create one through the gateway with an "Ask before each send" webhook call or a
fixture, and record how). Re-check on the next rebuild: everything routed in batches 1–3 and batch 4.

## Further ideas (not blocking)

- A dev-only fixture route (or `kind-dev.sh` helper) that creates a pending approval would make G1/G5 and the approvals flows
  testable on throwaway clusters.
- `/v1/resolve` retries on 404 should back off (the storm also starves the badges via 429).
