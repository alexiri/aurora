# P5-REFINE-20 — pass 2 report (UX review 5 verification)

Brief: `docs/tasks/P5-REFINE-20.md`. Reference: <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>, local copy
`/home/alex/orca/workspaces/aurora/design-ux-review-5/` (`UXR5/`); mockups `UXR5/after/<key>.png` and `-lt.png`. Pass 1:
`REPORT-pass1.md` (same folder). Screenshots of this pass: `docs/audit/ux-review-5-verify/pass2/` in the verification worktree
(`/home/alex/orca/workspaces/aurora/p5-platform`), **not in git** (`pass*/` is in git's exclude list); `pass2/_log.jsonl` has, per
capture, the final URL, console errors, CSP violations, 4xx/5xx `/v1` responses, horizontal overflow and axe wcag2a/aa results.

Verdicts as in pass 1: **Match**, **Minor**, **Needs changes**, **Not checked** (with the reason). "bN#m" is item m of change-list
batch N (batches 1–4: pass 1; batch 5: this pass).

## Part 1 (2026-11-21, stack at master `a84e93d0`: batches 1 and 2 in, batches 3 and 4 not yet)

### Environment and data

- Cluster `aurora-r20` (:4720, SSH 127.0.0.1:2272), kept from pass 1. Rebuilt with
  `AURORA_DEV_CLUSTER=aurora-r20 AURORA_DEV_PORT=4720 AURORA_DEV_SSH_PORT=2272 heavy.sh p5-platform deploy/dev/kind-dev.sh dev api web`
  (no Rust change since pass 1's last build, so only the web image was rebuilt). Never used `aurora-dev`, :4000 or the `aurora` DB.
- Data added in this pass: hosted agent **Rex** (New agent dialog, Acme team) and BYO agent **Robo** (API, Acme team); Daneel ↔ Rex
  link; Carol in Acme team as Member; invitation for dana@example.test; a Google sign-in app with fake credentials (Test failed);
  org internet rule "Block tracker.example.com"; schedules "Morning summary" (renamed "Daily digest") and "Nightly report" (deleted),
  one run each; a webhook delivery to "Deploy notice"; Bob's outgoing webhook "Ops alerts 3", personal keys "Bad key" and
  "My gateway key 3", SSH key "pass2"; user **Dave** (development sign-in, fresh Welcome); Slack chat-platform request by Carol
  (declined); web terminal saved **on** from the dashboard (`terminal.enabled` source `dashboard`, see admin-9); Counter's
  instructions edited; the grader of Daneel (Acme team) saved twice.
- **How the pending approval was made (G1/G5):** no fixture. Bob's Daneel has the outgoing webhook "Ops alerts 2" at
  **Ask before each send**. After a chat message woke its computer, an SSH session (key registered with `POST /v1/me/ssh-keys`,
  certificate from `POST /v1/agents/{id}/ssh-certificate`, `ssh -p 2272 <agent id>@127.0.0.1`) ran
  `aurora-agent call webhook send_ops_alerts_2 payload:='{"text":"Disk 91% on web-2 (pass 2, G1)"}'`. The gateway held it
  ("waiting: the request is slow or held for approval") and a pending approval "Send to Ops alerts 2" appeared
  (`conversation_id: null`). It turns `abandoned` when the computer goes to sleep (about 40 s later, see computer-1), so every
  capture ran right after the call (`scratchpad/g-run.sh`).

### Batch-1 leftovers (re-check)

| Item | Verdict | Evidence (pass2/…) | Note |
|---|---|---|---|
| b1#1, b1#2 (key verification, Try it row) | Not checked | — | routed in batch 3; part 2 |
| b1#3 no-key step | Match (code) | `setup-view.tsx:353` | "Add an AI key so agents can answer"; the no-key state needs a fresh install |
| b1#8 G4 refresh | Match | g4-p2-appear-*, g4-p2-decline-* | Administration badge appeared **17 s** after Carol's request, cleared **28 s** after the decline (was 289 s / none in 90 s) |
| b1#9 AI-key dialog | Match | b1-9-step1-*, b1-9-step2-* | no error on arrival; "Works with Claude Code." under the service; tiles "LV", "AS", "A" |
| b1#10 channels-3 | Needs changes (**b5#6**) | b1-10-new-*, b1-10-empty-* | New automation has the ask with "Why do you need it? (optional)" ✓; the empty Automations page has none for a personal-workspace owner (every member) |
| b1#11 workspace-9 | Match | b1-11-ask-* | "Why do you need it? (optional)" + Send request |
| b1#12 firstday-1 | Match | b1-12-welcome-* | "Welcome, Bob Builder" |
| b1#13 BYO header | Match | b1-13-byo-*, b1-13-list-* | one chip "Bring your own" |
| b1#14 first-answer card | Match | b1-14-first-answer-* | "Rex answered. It thought with the Company key and changed nothing outside this chat." |

### Batch 2 (re-check)

| Item | Verdict | Evidence | Note |
|---|---|---|---|
| 1 behaviour-6 Saved | Match | b2-1-saved-flash-* | "Saved" from 155 ms to ~1.9 s |
| 2 Settings at Use | Match | b2-2-bob-settings-* | Instructions · Apps & access · Memory · Skills |
| 3 failed grader, queued run | Needs changes (**b5#4**) | b2-3-grader-fail-*, b2-3-grader-fail-reload-*, b2-3-run-0-* | "Test failed" after Save ✓ but "Ready" again after a reload; "Waiting to start" ✓ but its row says "Running…"; the grader test takes 30 s and fails for a working model |
| 4 run title | Match | b2-4-runview-* | "Test run, Just now" (formatWhen) |
| 5 skill rows | Match | b2-5-skills-*, b2-5-skill-open-*, b2-5-skill-new-* | "Use it when …", "On for Daneel", files fold inside the card above Save (open at Expert by its altitude) |
| 6 model hint | Match | b2-6-model-* | one line |
| 7 New agent footer | Match | b2-7-newagent-* | footer pinned (Create agent at y 831 of a 900 px window) |
| 8 Working after Stop | Match | b2-8-stop-*, b2-8-answer-* | row and header leave Working 385 ms after Stop; ~6 s after an answer |
| 9 rail label | Match | b1-14-first-answer-* | Need · Now · Did |
| 10 titles | Match | b1-14-first-answer-*, b2-10-search-* | two lines; automation group rows still one line (b5#8) |
| 11 Done's Send a test | Match | b2-11-done-*, b2-11-test-* | asks for the values in place ("This webhook needs a few values…" + payload field) |
| 12 personal key | Minor | b2-12-valid-*, b2-12-reject-* | valid key: dialog closes (11 s), row "Not used yet" (no endless Checking…); the rejection path could not be produced (the mock accepts any key: "Bad key" saved) — part 2 |
| 13 github.com | Match | b2-13-github-* | "Paste a token from github.com; Daneel acts as you." |
| 14 phone row | Match | b2-14-access-*-390 | level select and … on one line |
| 15 403 / key card | Match | b2-14-access-*-1256 | no 403; one status line "Worked 1 min ago" |
| 16 next three runs | Match | b2-16-every5-* | "Every 5 minutes · next 03:50, 03:55, 04:00. Schedules run at most every 5 minutes." |
| 17 "Dani" | Match | b2-17-webhook-form-* | "Tells Daneel what to do …", "…can wake Daneel." |
| 18 times | Needs changes (**b5#5**) | b2-18-delivery-*, s7-deleted-thread-* | 24-h ✓, no extra chip ✓; titles in UTC, stamps and list in the browser zone |

### The 14 rows pass 1 did not check

| Key | Verdict | Evidence | Note |
|---|---|---|---|
| firstday-2 | Match | firstday-2-* | the switcher opens its list at 1256 and 390 (loading state not caught) |
| firstday-3 | Not checked | — | needs an unchecked key on a fresh Welcome (Dave's ticks "Your organization provides an AI key: Company key"); part 2 with batch 3's key states |
| firstday-4 | Match after fix | firstday-4-step1-1256, firstday-4-* | page dimmed, Overview bright, card beside it; step 2 showed a literal "Give <Agent> an app" — fixed in this lane |
| schedules-7 | Match | s7-*, s7-deleted-thread-* | rename: the group and run threads follow ("Daily digest"); delete: "Schedule deleted" + "It was started by a schedule that no longer exists. No new runs."; plain text, not the amber chip (Minor, b5#8) |
| channels-1 | Match | channels-1-*, channels-1-chosen, channels-1-link-* | "Daneel isn't linked to another agent" + Link another agent; picker, three directions, "Linked by Alice Admin · Rex allows it" |
| computer-1 | Needs changes (**b5#1**) | c1-terminal-started-*, c1-terminal-end-* | terminal open, `sleep 150` cut; computer restarted ~every minute; SSH session cut at 51 s; the restart notice prints |
| computer-2 | Not checked | c1-terminal-* | the retry loop never reached its end state (Reconnecting 1–3 of 4, then Waking…, then connected) |
| workspace-5 | Match | workspace-5-confirm-* | "Make Carol Member an Owner?", Member now → Owner, three ✓ lines, "You can change them back to Member at any time." |
| org-4 | Match | org-4-flow-*, org-4-copied-* | "Invitation created for dana@example.test." + Copy invitation link; row Copy invitation link · Cancel (no Resend: e-mail off); "Invitation link copied. Send it to dana yourself." |
| org-10 | Match | org-10-row-*, org-10-tested-* | "Ready (not tested)" + Test → "Test failed Just now: the provider didn't accept this client ID or secret."; raw "client_id:" line and "for OAuth" (b5#8) |
| org-14 | Match | org-14-saved-* | "Alice Admin added ‘Block tracker.example.com’ · Just now · current", Roll back to this on the older row; seed row "Someone published version 1" (b5#8) |
| admin-1 | Match | admin-1-*, admin-1-after-save-* | "Not turned on. 2 AI endpoints have no key origin… set in Helm, so add the origin there, then try again." + "Nothing changed"; Off after reload |
| admin-3 | Not checked | admin-3-* | Add under "Websites always allowed" gave no entry I could fill; Save then stored "Dashboard · None yet" without a confirm; part 2 |
| admin-9 | Needs changes (**b5#3**) | admin-9-before-*, admin-9-saved-off-*-390, admin-9-try-off-* | switch showed Off while the terminal was on; no aria-checked; 390 px overflow (458 px); refusing Off while SSH is on is explained in words ✓ |
| runner bug | Not checked (live) | admin-9-try-off-* | the dashboard cannot turn the terminal off while Helm SSH is on (422 `requires_terminal_for_ssh`); code re-decides on every work poll (`services/aurora-runner/src/runner.rs:284`, tests `:380`); a live check needs a cluster without `--ssh` |

### Count badges

| Badge | Verdict | Evidence | Measured |
|---|---|---|---|
| G1 Approvals | Match | g1-live-dark-1256 | rail "Approvals 1", the card "Daneel wants to Send to Ops alerts 2 in Outgoing webhook", Allow once · Always allow · Block, "Expires in 4m 54s" |
| G2 Overview | Match | g2-overview-wait-* | "1 thing needs you.", "Daneel is waiting for your approval · Review"; own requests listed, not counted |
| G4 Administration | Match | g4-p2-* | 17 s / 28 s |
| G5 Agent Needs you | Match | g5-live-dark-1256 | header "Needs you", rail "Need 1" |
| G6 Phone | Match | g6-live-header-dark-390, g6-live-drawer-dark-390 | menu button "Open menu, 1 needs your attention", drawer Overview 1 · Approvals 1 |

### Change list sent in part 1

- **Batch 5** (8 items, `msg_4e6735e57b3a`): computer-1 keep-awake (terminal and SSH) and abandoned approvals; SSH to a sleeping
  computer; admin-9 (switch shows Off while on, no aria-checked, 390 overflow); the failed grader not kept and the 30 s grader test;
  two time zones on one screen (b2#18); the empty Automations page ask for personal-workspace owners (b1#10); "can read Outgoing
  webhook (…)"; minor words.

### Fixed in this lane (Scope 7)

- `web/src/components/onboarding/tour.tsx`: the Agents stop read "Open an agent, then **Apps & access** › **Give <Agent> an app**…"
  (a literal placeholder on screen); now "Chat with an agent here. To let it use GitHub, Linear and more, open the agent's
  **Settings › Apps & access**." `docs/14-web-dashboard.md` (the tour paragraph) and `web/tests/onboarding.test.tsx` (asserts the
  new words and that no "<Agent>" shows) updated. No Playwright spec asserts the tour's text.

## Part 2 (2026-11-21, stack rebuilt at master `6d548b00` + this lane's tour fix: batches 1–4 in)

Rebuilt with the same command (`dev api web`; Rust changed, so both images). Every check below is on the rebuilt stack.

### Batch 3 (re-check)

| Item | Verdict | Evidence | Note |
|---|---|---|---|
| b1#2 setup-1 Try it row | Match | b4-setup-1-light-1256 | "Rex answered Alice Admin 1 h ago" + Open chat; "Uses Company key · no setup left" |
| b1#1 + b2#12 key verification | Needs changes (**b6#1**) | b4-access-btns-*, API log | keys no longer show Checking…, but the accounts list now takes 51 s for Bob (≈10 s per key on the unreachable endpoint) |
| 1 org-3 `/o/new` | Match | b3-1-onew-* | "We couldn't find that organization" inside the shell; **0** `/v1/resolve` requests in 15 s (was 246 × 404) |
| 2 org-7 AI models | Needs changes (**b6#2**) | b3-2-org-ai-* | limits folded under "Set above you (7)" ✓; preset "Custom · based on Balanced" vs Internet access "Open"; "No company key yet" while agents use the Company key; Save/Discard pair |
| 3 org-8 preset above h1 | Match | b3-3-org-internet-* | h1 first |
| 4 org-11 grammar | Match | b3-4-invite-dark-1256 | "They get their own personal workspace automatically." |
| 5 org-12 skills | Match | b3-5-org-skills-* | precedence under "How skills combine" |
| 6 personal-workspace name | Minor (**b6#5**) | b3-6-names-*, b3-6-bob-welcome-* | "Your workspace" in switcher, back link and Welcome ✓; "Back to Your workspace" capital; "dave@example.test's workspace" |
| 7 workspace-2 table | Match | b3-7-activity-* | DECISION column fits at 1256; 0 overflow at 390 |
| 8 workspace-3 / admin-10 audit | Minor (**b6#3**) | b3-8-ws-audit-*, b3-8-admin-audit-* | names and templates ✓, one "All actions" filter ✓; "eval" words, "changed Carol Member to Member", "account Rex" |
| 9 workspace-7 | Match | b3-9-ws-rename-dark-1256 | "Saved. The workspace is now called Acme crew." (renamed back to Acme team); language field not on this page |
| 10 workspace-4 | Minor (**b6#4**) | b3-10-add-people-* | "erin@example.test can open this workspace…" ✓; Approver explained, not offered |
| 11 rec 11 | Match | b3-11-computer-* | `?tab=computer` opens Files ("Waking the computer…") |
| 12 computer-7 / b4#4 | Match | b4-4-details-light-1256 | "Started 05:55 · running" |
| 13 computer-13 | Match | b4-4-details-* | "sleeps in 16 s when idle" counts down |
| 14 admin-5 | Match | b3-14-mattermost-* | "Next: an organization owner connects Mattermost in their Chat platforms page." + Open Acme's Chat platforms; hosts under Advanced |
| 15 admin-12 | Not checked | b3-15-upload-dark-1256 | a .txt selected shows no message before Upload; Upload not pressed |
| 16 endpoints Helm key | Match | b3-16-endpoints-* | no `llm.endpoints…` text |

### Batch 4 (re-check)

| Item | Verdict | Evidence | Note |
|---|---|---|---|
| 1 access-3 header and Apps & access card | Not checked | b4-1-header-*, b4-1-access-card-dark-1256 | every new agent gets the starter key (Nokey too), so no key-less agent; and Bob's Apps & access hangs (b6#1) |
| 2 org-2 "Register oauth client" | Match | b4-2-jira-* | no such link |
| 3 G2 agent that can't answer | Not checked | b4-3-overview-* | needs a key-less agent (item 1) |
| 4 computer-7 line | Match | b4-4-details-* | see b3#12 |
| 5 "Administration" twice | Match | b4-5-admin-rail-* | once in the Administration rail |

### Change list sent in part 2

- **Batch 6** (5 items, `msg_94d6bb094f30`): the 51 s accounts list (regression from `ba6f4d8c`, broken); org AI models preset,
  Company key and save pattern; audit "eval" words and odd rows; Approver not offered; minor words.

## Counts (pass 2, the 119 findings)

Pass 1's table with each finding moved to its verdict in this pass (a finding counts once). Pass-1 Matches were not re-walked
and stay Match. **Match 103 · Minor 5 · Needs changes 7 · Not checked 4** (119).

- Needs changes → Match (7): behaviour-6, access-4, schedules-2, webhooks-2, workspace-2, org-3, admin-5. Still Needs changes (3):
  setup-2 (b6#1), channels-3 (b5#6), org-7 (b6#2). Needs changes → Minor (3): access-8 (reject path not producible), workspace-3
  and admin-10 (b6#3). Needs changes → Not checked (1): access-3 (b4#1: no key-less agent can be made; b6#1).
- Minor → Match (19): setup-1, setup-7, firstday-1, chat-3, chat-4, chat-9, behaviour-2, behaviour-5, behaviour-7, access-2,
  access-6, channels-2, computer-7, computer-13, workspace-7, org-2, org-8, org-11, org-12. Minor → Needs changes (2):
  behaviour-4 (b5#4), webhooks-4 (two time zones, b5#5). Still Minor (2): workspace-4 (b6#4), admin-12 (not re-checked).
- Not checked → Match (9): firstday-2, firstday-4 (after the Scope-7 fix), schedules-7, channels-1, workspace-5, org-4, org-10,
  org-14, admin-1. Not checked → Needs changes (2): computer-1 (b5#1), admin-9 (b5#3). Still Not checked (3): firstday-3,
  computer-2, admin-3. The runner bug (`#bug`, not a finding row) is not checked live.
- Badges: G1, G2, G4, G5, G6 Match (G3 was Match after b1#8; not re-measured).

## Notes for the next pass

- The pending-approval recipe above works on any `--ssh` cluster; it needs the computer awake, and the approval is abandoned when
  the computer sleeps (b5#1), so capture within ~30 s.
- The runner bug needs a stack whose Helm values have SSH **off** (the dashboard refuses to turn the terminal off while SSH is on):
  install without `--ssh`, register the runner, turn the terminal on in Health & runners › Computers, then check Terminal and Files
  without restarting the runner.
- Data left on `aurora-r20`: see Part 1 › Environment, plus agent Nokey (Bob), Erin not added (dialog cancelled), Acme team renamed
  and renamed back, "My gateway key 4" not created (the dialog never opened).
