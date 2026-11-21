# P5-REFINE-20 — pass 4 report (UX review 5 verification)

Brief: `docs/tasks/P5-REFINE-20.md`. Reference: <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>, local copy
`/home/alex/orca/workspaces/aurora/design-ux-review-5/` (`UXR5/`). Earlier passes: `REPORT-pass1.md` … `REPORT-pass3.md` (same
folder). The screenshots of this pass are in `docs/audit/ux-review-5-verify/pass4/` in the verification worktree. They are
**not in git**. `pass4/_log.jsonl` records, per capture, the final URL, console errors, 4xx/5xx `/v1` responses, horizontal
overflow, axe wcag2a/aa results and the step output.

Verdicts as before: **Match**, **Minor**, **Needs changes**, **Not checked**. "bN#m" is item m of change-list batch N (batch 8:
this pass).

## Environment (2026-11-21, master `306b6efb`: all of batch 7; then `cf2e80d5`: FIX-199)

- Cluster `aurora-r20` (:4720, SSH 127.0.0.1:2272), kept from pass 3.
- Rebuild 1 (batch 7): `AURORA_DEV_CLUSTER=aurora-r20 AURORA_DEV_PORT=4720 AURORA_DEV_SSH_PORT=2272 heavy.sh p5-platform
  deploy/dev/kind-dev.sh dev api web` (403 s). `kind-dev.sh dev` now ran the migrations itself (`helm upgrade --reuse-values`,
  revision 6).
- **API egress to the LiteLLM mock.** The cluster's values predate `networkPolicy.apiExtraEgress`, and `--reuse-values` keeps
  them, so I added the rule from `deploy/kind/values-llm-mock.yaml`:
  `helm upgrade aurora deploy/helm/aurora -n aurora --reuse-values --set-json 'networkPolicy.apiExtraEgress=[{"to":[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":"aurora-dev-mock"}},"podSelector":{"matchLabels":{"app":"litellm"}}}],"ports":[{"protocol":"TCP","port":4000}]}]'`
  (revision 7). Every later upgrade reuses it (checked after revision 8).
- Rebuild 2 (FIX-199): `api terminator web`. **`kind-dev.sh dev` refuses `terminator`** ("unknown service: terminator": its
  service list lacks it, although `RUST_SVCS` has it). I ran an untracked copy with `terminator` added to that list
  (`deploy/dev/kind-dev-p4tmp.sh`, deleted afterwards; 366 s, revision 8).
- The runner was scaled to 0 for computer-2 (09:22:13–09:23:20 local), then back to 1. Two sandbox pods of Acme team's Daneel
  were deleted on purpose (computer-1's real restart, 09:14 and 09:19).
- Data added: Erin's "Wrong mock key" (LiteLLM, a non-master key); Erin's "My Anthropic key" checked (now `works`); Acme team's
  Daneel grader saved with scn-ratelimit, then claude-mock-nope (refused), scn-overload, claude-mock, and back on **scn-pong**.
  The admin-3 confirm was opened and left with Go back (nothing saved).
- Never used `aurora-dev`, :4000 or the `aurora` DB.

## Batch 7 (re-check)

| Item | Verdict | Evidence (pass4/…) | Note |
|---|---|---|---|
| 1 computer-1 restart line | Match | b7-1-term-first-open-*, b7-1-term-gave-up-*, b7-1-term-after-restart-* | First open of a **stopped** computer with an old `boot_id` (Acme team's Daneel, twice): prompt in 6 s, **0** restart lines after 20 s. Then the sandbox pod was deleted: the terminal tried 4 times, ended "Couldn't connect to the computer · Tried 4 times", Try again (the API answered the terminal with 404 for ~110 s until the runner made a new pod), then the prompt on the new computer with exactly **one** "— The computer restarted at 09:20; your running commands were stopped. —" |
| 2 firstday-3 only a key that works | Minor (**b8#6**) | firstday-3-erin-first-*, firstday-3-erin-welcome-*, b7-key-erin-* | Erin's only key was `not_checked`: the step stays open, "My Anthropic key has not been checked yet, so it does not count yet. It counts once a check says it works: open the key and use Test. Open AI keys" ✓. After a check (now reachable) it is `works` and the step ticks "This workspace has an AI key: My Anthropic key" ✓. But no control is called Test: the row has **Check again** and its menu **Test the connection** |
| 3 org-7 one rule | Needs changes (**b8#2**) | b7-3-org-ai-* | Still "Agents in Acme use Company key. People may not add their own." and "People may add their own key: Not allowed" (switch off, disabled) under Open. The new rule reads the form's `key_scopes`, which `formFromDocument` (`lib/api/governance.ts:217`) fills from the **stored** document (`[]` on all three endpoints), not the effective value (`["organization","workspace"]`, source instance). Erin added a personal key on LiteLLM in this pass |
| 4 schedules-7 chip | Minor (**b8#4**) | b5-8-chip-*-1256, -1600 | The chip is out of the title's clamp now and visible on desktop ✓; run rows keep their time ("Nightly report · 21 Nov 04:43") ✓. But the chip (133 px) is wider than the row's text column (~106 px): it runs under the folder row's chevron (359–492 px) and under the run row's ⋯, where the list column clips it (383–516 px) |
| 5 `--bubble-muted` | Match | b7-5-delivery-* | Delivery bubble with Technical details: no axe violation in dark or light, 1256 and 390 |
| 6 admin-12 upload | Minor (**b8#5**) | admin-12-upload-* | One file name with Choose another ✓, `aurora plugin pack` set as code ✓, Cancel ✓, Upload disabled after the error ✓, Technical details ✓, no overflow at 390. The words differ from the brief's quote (P5-REFINE-18 §17): "Upload the .tar.gz that aurora plugin pack makes." vs "Use a .tar.gz made with `aurora plugin pack`."; `.tar.gz` is not set as code (the mockup sets both) |
| 7 computer-2 Files | Minor (**b8#3**) | b7-7-files-* | Runner at 0: Files stops after **4** tries (503 at 16, 28, 41, 53 s) with "Couldn't connect to the computer · Tried 4 times. The computer didn't answer. · Try again", Upload/New file off, "Nothing loaded. Actions come back when the computer answers." ✓ (mockup). During the tries it still says "The computer restarted; retrying…" (36 s), and the computer had not restarted |
| 8a access-3 card, Since | Match (card); see access-3 below for the header | b4-1-nokey-access-*, b4-3-overview-* | Amber callout with its dot ✓. "Nokey can't answer yet: it has no AI key. Your organization's Company key goes to every new agent automatically. Nokey doesn't have it." Overview "Since 1 h ago": the grant was removed 06:29Z, checked 07:49Z (its creation, 04:52Z, would read 2 h or more) ✓ |
| 8b admin-3 confirm | Match | admin-3-confirm-1256, admin-3-save-* | "Allow more than before?" · INTERNET ACCESS · amber "Agents everywhere may now reach docs.example.org." · Technical details · "Affects 1 organization: Acme" · Go back / Yes, save |
| 8c no e-mail as a name | Minor (**b8#7**) | b7-8c-* | Audit "Alice added Dave as Approver" ✓; pick list "Workspace: Erin (personal)", "Workspace: Dave (personal)" ✓. Still an address as a name: Welcome "Welcome, erin@example.test"; the instance audit (`/admin/data?tab=audit`) "erin@example.test signed in", "… joined Aurora", "… changed their profile"; the People lists (organization, workspace, Administration › Users) show the address as the name and again as the address |

### access-3's sentence (task item 4)

The brief (P5-REFINE-12 §7) gives the card's second sentence as "Your organization's Company key goes to every new agent
automatically, and Rex was created before it was set." **shown only when that is the cause**. Under D-414 an agent created
before the starter key gets it when the key is added, so the card now appears only for an agent whose key was taken away (or
never given for another reason). Lane A's "Your organization's Company key goes to every new agent automatically. Nokey doesn't
have it." is true in that case and keeps the mockup's first half word for word; the conditional half is correctly absent.
**Match.**

## The mock is reachable now (task item 3)

| Row | Verdict | Evidence | Note |
|---|---|---|---|
| Key that works | Match | b7-key-erin-check, b7-key-erin-works-* | Erin's `not_checked` key: Check again → `works` ("Test passed just now · Works"); origin `verified`, method `endpoint` |
| Rejected key | Match (judged on Anthropic, pass 3) | b7-key-reject-* | A non-master key on the LiteLLM mock cannot be rejected: the mock runs without a database and answers **any** non-master key with `400 No connected db`. With no origin rule Aurora treats a non-401/403 failure as unanswered, so "Wrong mock key" ended `not_checked` (`endpoint_unreachable`) after the 2-min re-checks. The reject path stays judged on the external Anthropic endpoint (pass 3: "The provider rejected this key…") |
| Grader test passes | Match | API | scn-pong: `test.result ok` in 37 ms and 11 ms; claude-mock ok. Pass 2's "scn-pong fails the grader test" was the missing egress |
| Grader test fails | Match | API | scn-overload: `failed`, `unreachable` (62 ms); chip "Test failed" |
| `rate_limited` | Match | b5-4-grader-ratelimit-saved-dark-1256 | scn-ratelimit: "Grader saved, but the test failed: the model provider is rate limiting (try again in a minute). Questions the grader checks will not be scored until it works." kept after a reload |
| Grader with a model the key doesn't list | Minor (**b8#8**) | b5-4-grader-unreach-dark-1256 | claude-mock-nope: 422 `eval_judge_not_usable` ("The judge key or model cannot be used right now."), shown as "That key cannot be used right now. Check its status under the agent's model settings." The key works; the model is the problem |

## Found on the way

| Key | Verdict | Evidence | Note |
|---|---|---|---|
| access-3 header at 390 | Needs changes (**b8#1**) | b4-1-nokey-access-viewport-light-390, hdr-nokey-light-390 (also pass3/b4-1-nokey-*-390, missed in pass 3) | The row with "Give Nokey the Company key" (`agent-page.tsx:214`, `order-3 … md:basis-full`) has no `basis-full` below md, so it joins the name's flex line: the name is 0 px wide (box 100,85 0×20), the state "Can't answer: no AI key" (100,116) sits under Settings (29,87 107×44) and the button (148,87 226×44). Daneel, with no fix row, is fine (name 61 px) |

## FIX-199 (SSH keepalives, D-434), live

| Check | Verdict | Evidence | Note |
|---|---|---|---|
| SSH page line | Match | fix199-ssh-page-* | "What works": "A session with no output stays open while your SSH client is connected, up to 4 hours." and "A session also ends when you lose access to the agent, or after 10 minutes in which your client sends nothing at all." |
| `ServerAliveInterval=30`, `sleep 900 && echo DONE` | Match | session log (scratchpad) | Bob's Daneel, woken by the connection: in at 08:28:39Z on sbx-daneel-e8652c6b, "DONE-KEEPALIVE 08:43:39 sbx-daneel-e8652c6b" (same computer), exit 0. The sandbox's `keep_awake_reasons` was `["ssh"]` meanwhile |
| `ServerAliveInterval=0`, no output | Match | session log (scratchpad) | Same computer, started 2 s later: in at 08:28:39Z, "aurora: disconnected: no activity" at 08:38:39Z, exactly 600 s, exit 255 |

## Change list sent

- **Batch 8** (see the worker_done message): 1 access-3 header at 390 (lane A); 2 org-7 stored vs effective key scopes (lane B);
  3 computer-2 "restarted; retrying…" (lane A); 4 schedules-7 chip under the row's actions (lane B); 5 admin-12 words (lane A);
  6 firstday-3 "use Test" (lane B); 7 e-mail as a name: Welcome, instance audit, People lists (lane B); 8 grader
  `eval_judge_not_usable` words (lane B).

## Counts (pass 4, the 119 findings)

**Match 112 · Minor 5 · Needs changes 2 · Not checked 0** (119).

- Needs changes → Match (1): computer-1. Minor → Match (1): admin-3.
- Needs changes → Minor (3): firstday-3, schedules-7, admin-12. Still Needs changes (1): org-7.
- Minor → Needs changes (1): access-3 (the 390 header; its card is now a Match). Still Minor (1): computer-2.
- Match → Minor (1): behaviour-4 (the unknown-model refusal, first seen with the mock reachable).
- Outside the 119: the delivery bubble's contrast (b7#5) Match; the e-mail as a person's name (b7#8c) Minor.

## Notes

- **`kind-dev.sh dev` and the terminator.** `dev` lists `api|gateway|hookhost|runner|connector-host|connector-fake|dev-idp|web|sandbox`;
  rebuilding the terminator needs it added there (one word).
- **Values for the API egress.** A cluster made before `306b6efb` needs `--set-json networkPolicy.apiExtraEgress=…` (or a fresh
  `up --llm-mock`): `dev`'s `--reuse-values` does not bring in new keys of `values-llm-mock.yaml`.
- **A crashed computer.** After its pod was deleted, `/v1/agents/{agent}/terminal` answered 404 for ~110 s until the runner made
  a new pod. The terminal is honest about it (4 tries, the end card, Try again), but a crash costs about two minutes.
