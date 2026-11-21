# P5-REFINE-20 — pass 3 report (UX review 5 verification)

Brief: `docs/tasks/P5-REFINE-20.md`. Reference: <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>, local copy
`/home/alex/orca/workspaces/aurora/design-ux-review-5/` (`UXR5/`). Passes 1 and 2: `REPORT-pass1.md`, `REPORT-pass2.md` (same
folder). The screenshots of this pass are in `docs/audit/ux-review-5-verify/pass3/` in the verification worktree. They are
**not in git**. `pass3/_log.jsonl` records, per capture, the final URL, console errors, 4xx/5xx `/v1` responses, horizontal
overflow, axe wcag2a/aa results and the step output.

Verdicts as before: **Match**, **Minor**, **Needs changes**, **Not checked**. "bN#m" is item m of change-list batch N (batch 7:
this pass).

## Environment (2026-11-21, master `b4401a1e`: every send-back of passes 1 and 2)

- Cluster `aurora-r20` (:4720, SSH 127.0.0.1:2272), kept from pass 2. Rebuilt with
  `AURORA_DEV_CLUSTER=aurora-r20 AURORA_DEV_PORT=4720 AURORA_DEV_SSH_PORT=2272 heavy.sh p5-platform deploy/dev/kind-dev.sh dev api web runner`
  (833 s; api, runner and web restarted).
- **Migration needed a Helm upgrade.** `kind-dev.sh dev` restarts the API but runs no migrations (they are the chart's
  pre-upgrade hook). Migration 000269 (`eval_judge_test_*`) was missing, so `GET /eval-settings` and `POST /eval-runs` were 500
  (`column a.eval_judge_test_result does not exist`) until `helm upgrade aurora deploy/helm/aurora --reuse-values` (revision 2,
  no pod restarted).
- Helm changes made for the runner bug (all on `aurora-r20`): revision 3 `ssh.enabled=false`; revision 4 also
  `terminal.enabled=false`; revision 5 `ssh.enabled=true terminal.enabled=true`. The end state equals the starting values. The
  dashboard's stored `terminal.enabled` was removed, so the setting now reads source **helm**.
- The runner was scaled to 0 for computer-2 (07:39–07:52 local), then back to 1.
- Data added: user **Erin** (fresh Welcome; her Daneel's Company key grant removed, a personal key "My Anthropic key" on
  LiteLLM); Bob's **Nokey** lost its Company key grant (the key-less agent of access-3/G2); Dave added to Acme team as
  **Approver**; three grader saves on Acme team's Daneel (ends on scn-pong) and one test run. One rejected org key attempt
  (fake Anthropic key) was not saved. The instance's "docs.example.org" was left unsaved (Go back not pressed, the page was
  closed).
- Never used `aurora-dev`, :4000 or the `aurora` DB.

## Batch 5 (re-check)

| Item | Verdict | Evidence (pass3/…) | Note |
|---|---|---|---|
| 1 computer-1 keep-awake (D-416) | Needs changes (**b7#1**) | b5-1-terminal-started-*, b5-1-terminal-end-*, `scratchpad` ssh logs | web terminal `sleep 300 && echo DONE`: DONE on the same computer (sbx-daneel-c2a68857) ✓. SSH `sleep 590`: DONE on the same computer (04029147) ✓. An approval asked over SSH stayed **pending** for its whole 5 min and ended `expired` (pass 2: `abandoned` at ~40 s) ✓. But the terminal prints "The computer restarted at 07:02; your running commands were stopped." on the **first** open of a sleeping computer (seen twice) |
| 2 SSH to a sleeping computer | Match | API log | `ssh` straight to Bob's sleeping Daneel: channel in 3 s, first try |
| 3 admin-9 | Match | b5-3-admin-9-*, bug-helm-off-admin-* | switch `aria-checked`, "Reset to default (Follows what your deployment provides)" wraps at 390 (no page scroll); after the Helm change the API says `source: helm` |
| 4 behaviour-4 grader | Match | b5-4-grader-kept-*, b5-4-run-* | "Test failed" chip and "Grader saved, but the test failed: Aurora couldn't reach the model provider." kept after a reload; test budget 8.0 s; a queued run's row says "Waiting to start". `rate_limited` not producible here (see Notes) |
| 5 one zone per screen | Match | b5-5-delivery-*, b5-5-deleted-*, b5-5-automations-* | title "Deploy notice · 21 Nov 04:41", stamps 04:41, list "Webhook · 04:42", run notice 04:43 (all Europe/Zurich, the browser's); a daily schedule keeps "09:00 · UTC" as its own setting with "next at 10:00" local |
| 6 channels-3 empty Automations ask | Match | b5-6-* | Bob and Carol: "Only organization owners and admins can connect a chat platform. Ask an admin to connect Mattermost · Why do you need it? (optional) · Send request" |
| 7 access summary | Match | b6-1-bob-access-* | "Daneel can read US holidays. It can send to Ops alerts 3 and Ops alerts 2, and asks you first." |
| 8 minor words | Needs changes (**b7#4**) for the chip; rest Match | b5-8-*, b5-5-deleted-* | "The other agent only works…", "Client ID: 123-…", no "for OAuth", "The starting rules, version 1", group rows on two lines with the run count ✓; the amber "Schedule deleted" chip is clipped by the title clamp at 1256 and 1600 |

## Batch 6 (re-check)

| Item | Verdict | Evidence | Note |
|---|---|---|---|
| 1 accounts list | Match | b6-1-bob-*, API log | `GET …/accounts` 42–65 ms (was 51 s); Bob's Apps & access shows "Add an AI key" and the summary |
| 2 org AI models | Needs changes (**b7#3**) | b6-2-org-ai-*, b6-2-org-internet-* | preset Open on both pages ✓, Company key named ✓, no page-level Save/Discard ✓; but "People may not add their own." / "People may add their own key: Not allowed" under Open, while workspaces may add keys on LiteLLM and AIGW |
| 3 audit words | Match | b6-3-*, b6-3-ws-audit-new2-* | no "eval"; "Alice changed the grader settings", "started a test run"; a new add reads "Alice added dave@example.test as Approver" (the old "changed Carol Member to Member" row predates the fix). The e-mail as a name is b7#8c |
| 4 Approver in Add people | Match | b6-4-* | Member · Approver · Owner offered; the add worked |
| 5 small words | Match | b6-5-* | "Back to your workspace", "Dave's workspace", "last just now", "Previews aren't available here." |

## Rows checked for the first time

| Key | Verdict | Evidence | Note |
|---|---|---|---|
| access-3 (b4#1) | Minor (**b7#8a**) | b4-1-nokey-header-*, b4-1-nokey-access-* | Nokey (Company key grant removed): header "Can't answer: no AI key" + "Give Nokey the Company key"; Apps & access card with "Give Nokey the Company key · Choose another". The card is neutral (mockup: amber callout), and its "…was created before it was set" is false for a key taken away |
| G2 can't answer (b4#3) | Match | b4-3-overview-* | "Nokey can't answer: no AI key. Give it an AI key so it can reply. Since 1 h ago. Fix" (the "Since" is b7#8a) |
| firstday-3 | Needs changes (**b7#2**) | firstday-3-erin-* | Erin's only key is `verification: checking`, yet Welcome ticks "This workspace has an AI key: My Anthropic key": the guide reads verification from `GET …/secrets`, which has none |
| computer-2 | Minor (**b7#7**) | computer-2-terminal-down-*, computer-2-files-down-* | runner scaled to 0: terminal "Couldn't connect to the computer. Tried 4 times. The computer didn't answer. Try again · Report it", no cursor ✓; Files ends with the same card and Upload/New file off ✓, but only after ~2.5 min of "The computer restarted; retrying…" and 13 tries |
| admin-3 | Minor (**b7#8b**) | admin-3-look-*, admin-3-confirm-1256 | Add gives a website field; Save asks "Allow more than before? … Agents everywhere may now reach docs.example.org." with the key under Technical details ✓; heading "Instance rules" and plain bullets vs the mockup's section names and amber callouts |
| admin-12 | Needs changes (**b7#6**) | admin-12-upload-* | "That isn't an app package. Upload the .tar.gz that \`aurora plugin pack\` makes." shows literal backticks; file name twice; no Cancel; Upload enabled after the error |
| access-8 reject path | Match | access-8-reject-dark-1256 | fake key on the external Anthropic endpoint: "The provider rejected this key. Check it and try again; what you typed is still here.", value kept |
| runner bug (`#bug`) | Match | bug-*, runner log | Dashboard off plus Helm `ssh.enabled=false terminal.enabled=false`: the runner registered with :4108 closed. Then `helm upgrade` turned terminal and SSH on. The new runner registered with the old API pod at 07:10:14Z and opened :4108 on a **work poll** at 07:10:35Z (C-1057). Terminal prompt, Files listing and `ssh … echo` (SSH-OK sbx-daneel-3e306043) all worked with no manual restart. Before that (Helm terminal still on), a dashboard Off → runner restart → On also worked on the same runner pod. With Helm's wiring off the page says "The web terminal isn't available on this Aurora…" and offers no switch (by design) |

## Change list sent

- **Batch 7** (`msg_d398087dfa54`, ~08:20): 1 the false "computer restarted" line on first open (computer-1); 2 firstday-3 checking
  key ticked (secrets list has no verification); 3 org-7 "People may not add their own" under Open; 4 Schedule deleted chip
  clipped, nested run rows identical; 5 Technical details contrast on the delivery bubble; 6 admin-12 backticks, Cancel,
  Upload state; 7 computer-2 Files' 13 tries saying "restarted"; 8 minor: access-3 card tint and sentence, "Since", admin-3
  frame, e-mail as a person's name.

## Counts (pass 3, the 119 findings)

**Match 111 · Minor 3 · Needs changes 5 · Not checked 0** (119). Runner bug Match; badges G1, G2 (now also with a key-less
agent), G4, G5, G6 Match.

- Needs changes → Match (5): setup-2, channels-3, behaviour-4, webhooks-4, admin-9. Still Needs changes (2): computer-1 (b7#1),
  org-7 (b7#3).
- Minor → Match (4): access-8, workspace-3, admin-10, workspace-4. Minor → Needs changes (1): admin-12 (b7#6).
- Not checked → Minor (3): access-3, computer-2, admin-3. Not checked → Needs changes (1): firstday-3 (b7#2).
- Match → Needs changes (1): schedules-7 (b7#4: the chip batch 5 asked for exists but never shows on desktop).
- Outside the 119: the delivery bubble's contrast (b7#5) and the e-mail as a person's name (b7#8c).

## Notes

- **SSH idle timeout.** `sleep 600` with no output was cut at exactly 600 s: "aurora: disconnected: no activity". That is
  C-794's `SSH_IDLE_TIMEOUT` (600 s without a channel byte), and OpenSSH keepalives (`ServerAliveInterval`) don't reset it.
  The computer stayed awake the whole time, so keep-awake is fine. Whether keepalives should count is an owner question.
  `sleep 590` finished.
- **The kind llm-mock stack can't verify keys.** The API's NetworkPolicy egress excludes 10.0.0.0/8, so the API never reaches
  the in-cluster LiteLLM. Every key check and grader test there ends `unreachable`/`not_checked` after the budget, and
  `rate_limited` can't be produced. This is also why pass 2 saw scn-pong fail the grader test while it answers in chat (the
  gateway can reach it).
- **`kind-dev.sh dev` runs no migrations.** After a merge that adds one, run `helm upgrade --reuse-values` (or `up`) before
  checking.
