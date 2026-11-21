# P5-REFINE-20 — pass 5 report (UX review 5 verification)

Brief: `docs/tasks/P5-REFINE-20.md`. Reference: <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>, local copy
`/home/alex/orca/workspaces/aurora/design-ux-review-5/` (`UXR5/`). Earlier passes: `REPORT-pass1.md` … `REPORT-pass4.md` (same
folder). The screenshots of this pass are in `docs/audit/ux-review-5-verify/pass5/` in the verification worktree. They are
**not in git**. `pass5/_log.jsonl` records, per capture, the final URL, console errors, 4xx/5xx `/v1` responses, horizontal
overflow, axe wcag2a/aa results and the step output.

Verdicts as before: **Match**, **Minor**, **Needs changes**, **Not checked**. "bN#m" is item m of change-list batch N (batch 9:
this pass).

## Environment (2026-11-21, master `ccc56fdd`: all of batch 8)

- Cluster `aurora-r20` (:4720, SSH 127.0.0.1:2272), kept from pass 4. `git merge master` was a fast-forward to `ccc56fdd`.
- Rebuild: `AURORA_DEV_CLUSTER=aurora-r20 AURORA_DEV_PORT=4720 AURORA_DEV_SSH_PORT=2272 heavy.sh p5-platform
  deploy/dev/kind-dev.sh dev api terminator web` (280 s). **`dev` accepts `terminator` now** (built, image unchanged since
  FIX-199, so only api and web restarted). The migrate step ran `helm upgrade aurora --reuse-values -f
  deploy/kind/values-llm-mock.yaml` (revision 9); the release's values before and after are identical (the egress rule set by
  hand in pass 4 is the same as the file's), so **the dev-stack item is a Match**.
- The runner was scaled to 0 for computer-2 (10:22:08–10:23:16 local), then back to 1.
- Data added: **Frank** (`frank@example.test`, first sign-in this pass, personal workspace `dev-frank-example-test`). His
  Daneel's Company key grant was removed through the API (`DELETE /v1/agents/{id}/grants/secrets/{id}` as Frank, 204) and
  "Frank mock key" (LiteLLM mock, non-master secret) added, which ended `not_checked` after the re-checks (09:26:13Z). The
  grader of Acme team's Daneel was tried with `claude-mock-nope` (refused, nothing saved) and stays on **scn-pong**.
- Never used `aurora-dev`, :4000 or the `aurora` DB.

## Batch 8 (re-check)

| Item | Verdict | Evidence (pass5/…) | Note |
|---|---|---|---|
| 1 access-3 header at 390 | Needs changes (**b9#1**) | b8-1-hdr-nokey-access-*-390, b8-1-hdr-nokey-chat-*-390 | The give-key row is its own full-width row now ✓ (button 16,133 226×44, under the name row) and the name is back (h1 155 px on Apps & access, 77 px on Chat). But the state "Can't answer: no AI key" is one unbreakable item (`whitespace-nowrap`) 156 px wide starting at x 116, and the column ends at the buttons: on Apps & access its last letter is under Settings (267 px) — "no AI ke"; on Chat, where Status sits beside Settings, it reads "Can't answ" then Status hides the rest |
| 2 org-7 effective key scopes | Match | b8-2-org-ai-* | "Agents in Acme use Company key. People may add their own." · "People may add their own key: Allowed", switch on (disabled: the instance sets it) · preset Open ("…people may add their own AI keys…"), both themes, 1256 and 390, no axe violation. Pick list names "Workspace: Erin (personal)", "Workspace: Dave (personal)" (no address) |
| 3 computer-2 Files words | Match | b8-3-files-0…4, b8-3-files-end | Runner at 0: "Reconnecting… (attempt 2 of 4)", 3, 4 after the 503s at 16, 28, 41 s, then at 53 s the end card "Couldn't connect to the computer · Tried 4 times. The computer didn't answer. · Try again" with Upload/New file off and "Nothing loaded. Actions come back when the computer answers." No "restarted" anywhere |
| 4 schedules-7 chip | Match | b8-4-chip-dark-1256, -1600 | The chip wraps inside the text column: folder row 359–470 px (chevron after 480), run row 383–462 px (⋯ after 480); never clipped. At 390 the list is behind the chat, as in earlier passes |
| 5 admin-12 words | Match | admin-12-upload-* | "That isn't an app package. Use a .tar.gz made with aurora plugin pack." with `.tar.gz` and `aurora plugin pack` both as code; one file name, Choose another, Technical details, Upload disabled after the error; 1256 and 390, both themes, no axe violation, no overflow |
| 6 firstday-3 control name | Match | firstday-3-frank-welcome-*, firstday-3-frank-keys-* | Frank's only key `not_checked`: "Frank mock key has not been checked yet, so it does not count yet. It counts once a check says it works: open the key and press Check again. Open AI keys"; the key's row says "Not checked: the AI service didn't answer Aurora's check…" with **Check again**. "Welcome, Frank". 1256 and 390, both themes, no axe violation |
| 7 e-mail as a name | Match (the three named places); see b9#3 | b8-7-email-*, b8-7-email2-* | Welcome "Welcome, Erin" / "Welcome, Frank"; instance audit "Erin changed their profile", "Erin signed in" (no address on the page); organization People "Frank frank@example.test", "Erin erin@example.test"; Administration › Users "Frank frank@example.test · User" (the address once). Not yet: the shell's account button (b9#3) |
| 8 behaviour-4 grader model | Match | b8-8-grader-model-dark-1256 | `claude-mock-nope`: 422 `eval_judge_not_usable` (cause model) shown in the save bar as "This key can't use that model. Check the model name."; the current grader stays "Company key with scn-pong · Ready" |
| dev stack | Match | dev log (scratchpad) | `kind-dev.sh dev … terminator …` accepted; migrations layer `values-llm-mock.yaml` on `--reuse-values` |

## Pass 4's Minor rows

firstday-3, schedules-7, admin-12, computer-2 and behaviour-4: all **Match** (rows 6, 4, 5, 3 and 8 above).

## Found on the way

| Key | Verdict | Evidence | Note |
|---|---|---|---|
| Agent header at tablet widths (768–~950 px) | Needs changes (**b9#2**) | b8-1-hdr-daneel-*-768, b8-1-hdr-daneel-w-chat-dark-820/-900, b8-1-hdr-nokey-*-768, b8-1-hdr-nokey-w-access-dark-900 | At md and up the header is one line (name · tabs · Settings) and the name gives way first: the h1 is **0 px** wide at 768, 820 and 900 for every agent (Daneel too, so not batch 8's doing), and the handle/state runs under the Chat/Automations/Computer tabs. 960 px gives 51 px ("Dane…"), 1024+ is fine. At 768 the access-3 card's "Give Nokey the Company key" (226 px) is wider than the card (right column 216 px) and the page scrolls sideways (scrollWidth 775). Outside the 119 (they were checked at 1256 and 390) |
| Shell account button shows the address | Minor (**b9#3**) | b8-7-email-6-dark-1256 (bottom left) | Erin's account button (and its menu header) reads "erin@example.test": `app-shell.tsx:130` and `:171` use `display_name` raw. By reading, the same raw `display_name` of a person is in "Paused by …" (`automations/schedule-row.tsx:217`, `webhook-row.tsx:130`), "decided by …" (`admin/host-claims-view.tsx:23`) and the linked people of a chat platform (`channels/channels-view.tsx:57`) |

## Change list sent

- **Batch 9** (see the worker_done message): 1 access-3 state hidden under Settings/Status at 390 (lane A); 2 the agent header
  at 768–950 px (lane A); 3 personName in the shell's account button and the remaining raw person names (lane B).

## Counts (pass 5, the 119 findings)

**Match 118 · Minor 0 · Needs changes 1 · Not checked 0** (119).

- Needs changes → Match (1): org-7. Minor → Match (5): firstday-3, schedules-7, admin-12, computer-2, behaviour-4.
- Still Needs changes (1): access-3 (the header at 390: the name and the row are fixed, the state is now hidden under the buttons).
- Outside the 119: the e-mail as a name (b7#8c) Match where batch 8 named it, Minor in the shell (b9#3); the agent header at
  tablet widths Needs changes (b9#2); the dev stack Match.

## Notes

- A personal workspace's stored name is still the address ("frank@example.test's workspace" from `/v1/resolve`); every screen
  seen shows "Frank's workspace" / "your workspace", so this is not a send-back.
