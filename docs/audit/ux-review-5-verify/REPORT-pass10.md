# P5-REFINE-20 — pass 10 report (UX review 5 verification, final pass)

Brief: `docs/tasks/P5-REFINE-20.md`. Reference: <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>, local copy
`/home/alex/orca/workspaces/aurora/design-ux-review-5/` (`UXR5/`). Earlier passes: `REPORT-pass1.md` … `REPORT-pass9.md` (same
folder). The screenshots of this pass are in `docs/audit/ux-review-5-verify/pass10/` in the verification worktree. They are
**not in git**. `pass10/_log.jsonl` records, per capture, the final URL, console errors, 4xx/5xx `/v1` responses, horizontal
overflow, axe wcag2a/aa results and the step output (boxes measured in the page).

**This is the final pass (owner's decision, 2026-11-21, relayed by the Orchestrator):** there is no pass 11. What this pass
finds goes to the lanes as batch 14 and is fixed without another re-check, and P5-REFINE-20 then closes. The change list
below is written to be fixed without a re-check: the rule or brief, the measured difference, the target and the file.

Verdicts as before: **Match**, **Minor**, **Needs changes**, **Not checked**. "bN#m" is item m of change-list batch N.

## Environment (2026-11-21, master `4c28329a`: batch 13, FIX-205, FIX-208, FIX-209)

- Cluster `aurora-r20` (:4720, SSH 127.0.0.1:2272), kept from pass 9. `git merge master` was a fast-forward to `4c28329a`.
  `AURORA_DEV_CLUSTER=aurora-r20 AURORA_DEV_PORT=4720 AURORA_DEV_SSH_PORT=2272 heavy.sh p5-platform deploy/dev/kind-dev.sh dev
  api terminator web`: "done in 1169s: rebuilt+restarted: api terminator web", exit 0 (the terminator because FIX-208 changed
  its certificate check).
- Written to the cluster in this pass: one SSH certificate for Bob's Daneel minted from the page (×4, one per capture) and one
  through `POST /v1/agents/{id}/ssh-certificate`; the schedule form was filled but never submitted.
- Never used `aurora-dev`, :4000 or the `aurora` DB.

## agent-header-widths (mocked Playwright), run as asked

The lane edited `web/e2e/agent-header-widths.spec.ts` (batch 13 #1: the meta row's edges equal the header's below 1280)
without running it. Run here: `AURORA_E2E_WEB_PORT=4193 AURORA_E2E_WEB_TRACING_PORT=4194 AURORA_E2E_WORKERS=2 heavy.sh
p5-platform pnpm exec playwright test --project=mocked e2e/agent-header-widths.spec.ts` → **100 passed (4.1 min)**. (The first
attempt on port 4172 stopped at start-up: the CI Watcher's own `next dev` was listening there; no test ran.)

## Batch 13 (re-check)

| Item | Verdict | Evidence (pass10/…) | Note |
|---|---|---|---|
| 1 the thread header's meta row edge to edge below 1280 | Match | b13-1-{archived,member,direct,carol-sched,carol-own}-{dark,light}-{390,768,900,1023,1100,1280}, b13-zoom-archived-light-1023, b13-zoom-member-dark-768 | Below 1280 the meta row's left and right edges equal the header's at every width and for every thread (`shortLeft` 0, `shortRight` 0: 17–373 at 390; 289–679 at 768; 289–811 at 900; 289–934 at 1023; 521–1011 at 1100). On the right the header and the row end at the rail's left border (679/811/934/1011 = `railLeft`), so the row's top and bottom borders run from the card's inner edge to the rail's border; at 390 (no rail) from card edge to card edge (17–373 = the card's inner box). Everything batch 12 achieved holds: the row is its own line under the title, the whole privacy note and "Archived 21 Nov 2026 · read only" visible, no `title` attribute, 0 text lines outside the header or the card, 0 clipped, the title row's border level with the rail's (217/217 at 768–1100, 153/153 at 1280), nothing over the thread. At 1280 the meta line sits in the 64 px header without borders, as before |
| 2 an organization member reads what is in force | Match | b13-2-org-member-{dark,light}-{390,1280} | Carol (member of Acme) on Acme › Internet access: "In Acme, requests without an account can reach any website unless a rule or a workspace blocks them." in the page header's summary style (`font-display text-xl text-ink`, like the workspace page's sentence), then the notice "Only organization owners and admins can change this. Ask an organization admin." The description no longer repeats the notice. No Limits, no "Set for you by". The sentence is the one `docs/14` §Scope ladder (D-444, amended by this fix) gives as its example, read from `governance/effective` |
| 2, neighbours: organization owner, workspace member, workspace owner | Match | b13-2-org-owner-*, b13-2-ws-member-*, b13-2-ws-owner-* | The owner's organization page as in pass 9 (posture, "Set above you", the instance baseline, "Restricted further below", history). The workspace member page: the summary, who to ask, and pass 9's Scope-7 empty state now live ("No rules yet. The sentence at the top says what agents here may reach."). The workspace owner keeps "Limits and defaults" with "Set for you by your organization's admins" |

## Owner fixes since pass 9

| Fix | Verdict | Evidence (pass10/…) | Note |
|---|---|---|---|
| FIX-205 Once | Needs changes (**b14#1**) | f205-once-{dark,light}-{390,768,1280}, f205-zoom-once-dark-1280, f205-zoom-once-light-768 | Once's date is a `DateField` (app text "Nov 22, 2026" with the calendar button), and at 1280 date, time and zone share one line (y 445 for all three). The summary reads "Once, on Sun 22 Nov at 09:00 (UTC time)." **But the date input is 264 px wide** at every width (time select 103 px). FIX-205 asks for "the size of a date … about the time select's width plus the calendar button … never `w-full`". `DateField`'s own `w-40` (160 px) never applies: `date-field.tsx:155` builds `` `${inputClass} w-40 pr-10` ``, `inputClass` starts with `block w-full`, and in the generated CSS `w-full` wins over `w-40`. The input then fills its shrink-wrapped `inline-flex` span, whose width is the text input's intrinsic width (264 px). At 768 the row is 406 px in the agent's Automations tab (the 256 px sidebar is there), and date + time + zone need at least 160 + 103 + 288 + 16 = 567 px. So the zone wraps under the date at 768, which is what FIX-205 says to do when the row is narrower than the three. At 390 they stack, with no horizontal scroll |
| FIX-205 Custom | Needs changes (**b14#1**) | f205-custom-{dark,light}-{390,768,1280}, f205-custom-often-{dark,light}-{390,768,1280} | Custom is enabled with nothing in front of it (no "needs more controls" anywhere; `useMoreControls`/`Altitude auto.cron` gone from the form). Choosing it shows the cron field and the zone. "* * * * *" shows "Too often: a schedule can run at most every 5 minutes. Try */5 * * * *." and disables Create schedule. "15 7 * * 1-5" previews "next: Mon 23 Nov, 07:15 (UTC time)" and enables it. Leaving for Every day and coming back keeps "15 7 * * 1-5". **But the cron field fills the whole row (918 px at 1280, 406 at 768, 324 at 390), so the zone always sits on the next line** (cron y 445, zone y 493 at 1280). FIX-205 item 2 asks for the zone "beside it on the same line when it fits (cron field ~ `max-w-xs`, mono)". Cause as above: `schedule-form.tsx:283` `` `${inputClass} w-64 max-w-full font-mono` ``, whose `w-64` loses to `inputClass`'s `w-full`. (Carol's "more controls" switch is on, so the live check can't show the switch-off case; the code no longer reads the switch, and `automations-page.test.tsx` covers it without the switch) |
| FIX-208 SSH page | Match | f208-ssh-page-{dark,light}-{390,1280}, f208-ssh-cert-{dark,light}-{390,1280} | Bob's Daneel › Computer › Connect: the plain-ssh block reads in full `ssh -i ~/.ssh/aurora-daneel -o CertificateFile=~/.ssh/aurora-daneel-cert.pub -p 2222 daneel@localhost` before a certificate is minted (the handle) and after (the answer's `username`, `daneel`), with "Certificate valid for 16 more minutes (until 17:28)" and "Save it as ~/.ssh/aurora-daneel-cert.pub". `aurora ssh daneel` above it. At 390 the command wraps inside its block. (Four `f208-ssh-cert` ERROR rows at the top of `_log.jsonl` are a first run against the retired `?tab=connect`, which now opens Chat; the page is `?tab=ssh`) |
| FIX-208 live SSH | Match | `ssh` transcript below | Certificate minted with `POST /v1/agents/01a1fb99-…/ssh-certificate`: `username` "daneel", principals `01a1fb99-8a25-73a7-bb3c-4eac5406f028` then `daneel` (`ssh-keygen -L`), key id `…:a=01a1fb99-8a25-…`. `ssh -p 2272 daneel@127.0.0.1` → "CONNECTED as agent on sbx-daneel-b4c17f3a" (it woke the computer, 8 s); `ssh -p 2272 01a1fb99-8a25-73a7-bb3c-4eac5406f028@127.0.0.1` → the same computer; `ssh -p 2272 counter@127.0.0.1` (a name not in the certificate) → "Permission denied (publickey)" |
| FIX-209 Mattermost callback address | Not checked live | — | The rule and the label ("Aurora's address for Mattermost") live in the Mattermost connector (`connectors/mattermost/src/connector.rs`) and the API's managed-connector setup. This pass rebuilt api, terminator and web as asked, not the connector host, and `aurora-r20` has no Mattermost server. Its ITs and unit tests were run by the lane |

Every pass-10 capture (96 rows in `_log.jsonl`, apart from the four `?tab=connect` rows): no horizontal scroll, no axe
violation, no console error, no 4xx/5xx `/v1` response, no CSP report.

## Change list (batch 14, with the worker_done message; no re-check pass follows)

1. **Lane A (owns FIX-205), Needs changes. A width appended to `inputClass` never applies.**
   - Rule: FIX-205 §Change 1 (the date "the size of a date … never `w-full`") and §Change 2 (the cron field "~ `max-w-xs`",
     the zone "beside it on the same line when it fits").
   - Measured: the date input is 264 px at 390/768/1280 (target 160 px, `DateField`'s own `w-40`). The cron field is
     918 px at 1280 (the whole row; target 256 px, its `w-64`), so the zone is on the next line (y 493 vs 445).
   - Cause: `inputClass` (`components/ui/field.tsx:3`) starts with `block w-full`, and `w-full` beats a width class
     appended in a template string.
   - Change:
     - `components/ui/date-field.tsx:155` `` `${inputClass} w-40 pr-10` `` → `cn(inputClass, "w-40 pr-10")`.
     - `components/automations/schedule-form.tsx:283` `` `${inputClass} w-64 max-w-full font-mono` `` →
       `cn(inputClass, "w-64 max-w-full font-mono")`.
     - `cn` is `twMerge(clsx(…))` (`lib/utils.ts`) and drops `w-full`. No arbitrary values.
   - Expected after the fix:
     - Once at 1280: date 160 px, time and zone beside it on one line.
     - Custom at 1280: cron 256 px with the zone beside it (256 + 8 + 288 ≤ 918).
     - At 768 (406 px row) the zone still wraps for both, which FIX-205 allows when the row is narrower than the three.
     - At 390 everything stacks, with no horizontal scroll.
   - The same pattern loses its width in three more places (same one-line fix, same lane if it owns them, else the
     Orchestrator routes it): `components/connectors/self-hosted-provision.tsx:49` (`w-56 max-w-full`) and `:59`
     (`w-64 max-w-full`), and `components/apps/instances-view.tsx:377` (`w-auto max-w-56`).
   - `DateField` is shared: Usage, Audit, organization and personal API keys get the 160 px date too. Check them at 390 and
     1280 (no overflow, the calendar button inside the field).
   - Also fix the stale doc comment at `schedule-form.tsx:94–95` ("Custom keeps the 5-field cron for the Expert
     altitude"): D-443 removed the altitude.
   - Test: in `web/e2e/automations.spec.ts` (mocked), at 1280:
     - Once: the `Date` input's width is between 140 and 176 px, and the date, time and zone tops are within 4 px (the
       last assertion is already there).
     - Custom: the `Cron (5 fields)` input is at most 320 px wide, and the `Time zone` combobox's top is within 4 px of
       the cron field's.
   - Run that spec, plus the DateField vitest files (`date-field`, usage, audit, keys).

Not sent (further ideas; they break no `docs/14` rule):

- When the meta line wraps below 1280, a segment can start its line with its separator ("· Archived 21 Nov 2026 · read only"
  under the privacy note at 390).
- `ConfirmDialog` keeps Cancel and the action at the end of its body while `FormDialog` pins them in a bordered footer.
- From pass 8, still open: the Jira family card keeps the Cloud description when only Data Center is usable; below 1280 the
  Chat page scrolls 64 px (allowed by docs/14's 1280×800 rule); at 768 the agent Settings sub-navigation keeps 208 px beside
  the content.

## Fixed in this lane (Scope 7)

None: the one difference is in files lane A owns (`schedule-form.tsx`) or a shared primitive (`date-field.tsx`).

## Counts (pass 10, the 119 findings)

**Match 119 · Minor 0 · Needs changes 0 · Not checked 0** (none of the 119 touched by batch 13, FIX-205, FIX-208 or FIX-209
except through the items above).

- Outside the 119:
  - b13#1 Match, b13#2 Match, FIX-208 Match (page and live SSH).
  - FIX-205 Needs changes (b14#1: the widths of the date and cron fields).
  - FIX-209 Not checked live.

P5-REFINE-20's exit (zero Needs changes and zero Minor anywhere) is **not** reached by this pass, because FIX-205's widths
are open. By the owner's decision there is no pass 11: batch 14 is fixed without a re-check and the brief then closes.

**Cluster:** the teardown (`AURORA_DEV_CLUSTER=aurora-r20 AURORA_DEV_PORT=4720 deploy/dev/kind-dev.sh down`) was refused by
this session's permission classifier and did not run. `aurora-r20` (:4720, SSH :2272) is still up and needs that command run
by the Orchestrator or the owner.
