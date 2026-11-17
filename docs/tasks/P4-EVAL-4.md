# P4-EVAL-4: the Evals page on the agent page (Behaviour section)

## Goal
A hosted agent's **Evals** page, under the Behaviour section, lets people with `can_use` manage questions, set the judge key, start and cancel runs, read results with the fingerprint diff, and manage eval schedules, using only the shared UI primitives and the dashboard's visual identity, in both themes at 390 px.

## Linked requirements
- D-289(e) (dashboard placement), D-291 (fingerprint diff), D-293 (schedules, regression notice), D-292(a) (judge key choice).
- Contracts C-873 (dashboard page), C-861/C-863/C-864/C-866/C-870/C-871 (the routes it calls); `docs/14 §Visual design` and the agent page's section rules (`docs/14` Agent page, P5-AGENT-2).
- SEC-116 (results visible to `can_use` only; nothing copied to the page's URL, storage or logs).

## Context
- Agent page: `web/src/components/agents/agent-page.tsx` (sections and `?tab=` routing; locate the Behaviour section's tab list), `instructions-tab.tsx` (form patterns), `previews-tab.tsx` and `terminal-tab.tsx` (a page that is hidden when unavailable; hosted only), `access-tab.tsx`.
- Shared primitives in `web/src/components/ui/`: `data-table.tsx`, `dialog.tsx`, `confirm-dialog.tsx`, `tabs.tsx`, `segmented.tsx`, `badge.tsx`, `status-dot.tsx`, `notice.tsx`, `meter.tsx`, `card.tsx`, `section.tsx`, `field.tsx`, `form.tsx`, `menu.tsx`, `page-header.tsx`, `toolbar.tsx`. Use them; add a missing one to this set, never a one-off style. Tokens only (`docs/14 §Visual design`).
- API: P4-EVAL-1/2/3 routes (`docs/05 §Evals`, `web/openapi.json` for the generated client). Do not hand-write request shapes; use the generated types.
- Existing UI tests: vitest and Playwright conventions in `web/` (follow the terminal and previews tests).

## Scope
### In scope
- Behaviour section: a fourth page **Evals** (`?tab=evals`) beside Instructions, Memory and Skills; hidden for BYO agents (BYO has no evals, 422 `eval_not_supported`) and for a `can_use`-less caller.
- Questions: a `DataTable` (position, question, kind, expected summary; on a phone a stacked list with the same content), add and edit in a `Dialog` (kind selector, number/tolerance, text, criteria; the question text with its character counter at `EVAL_QUESTION_CHARS`), delete with `ConfirmDialog`, reorder by position fields; the 100-question cap explained.
- Judge: a `Field` showing the granted LLM secrets the caller may see and a model field; saves through `PUT …/eval-settings`; `can_own` only for the change (the control is disabled with the reason for others); a `Notice` for each refusal code of C-872.
- Runs: a list (status `Badge`, progress `Meter`, pass rate, trigger, started by), "Start run" (disabled with the reason when a run is active or the daily cap is reached), "Cancel" with `ConfirmDialog`; a run page with the result table (outcome `Badge`, question, answer excerpt for `can_use`, reason in words, refused writes as a count), the regression `Notice` (C-871) and the fingerprint diff against the previous run (a plain two-column table of changed fields; hashes shown truncated with a copy action).
- Schedules: a section on the same page listing eval schedules (`DataTable`), create and edit in a `Dialog` with the cron field and a plain-language preview, and "Run now" through the start route.
- Copy: plain English, no emoji, sentence case; the help text for "ANSWER:" and for write refusals explains what the agent is told.
- Tests: vitest for the question dialog validation, the judge control's disabled states, the cap messages and the fingerprint diff; Playwright at 390 px and in both themes for the page's list and run view, with no horizontal scroll and axe checks.

### Out of scope
Any change to the API, the agent page's other sections, the status rail, channel settings, or the chat list.

## Constraints / file ownership
- Owns: `web/src/components/agents/evals/**` (new), the Behaviour tab wiring in `web/src/components/agents/agent-page.tsx` (the page list only), `web/src/components/ui/**` only for a primitive that does not exist yet (announce it), vitest and Playwright specs for the page, the generated client files if the repo keeps them.
- Must not touch: the API, `docs/`, the other agent tabs' behaviour.
- The page never stores results or expected answers in `localStorage`, the URL or a cache outside the React query cache; an unsent question draft may use `sessionStorage` only.

## Acceptance criteria (failing-first)
1. vitest: question dialog refuses an empty criteria for judged questions and a non-number for numeric; the judge control is disabled for a `can_use` non-owner with the explanation; the fingerprint diff lists exactly the changed fields; the cap message appears at 100.
2. Playwright at 390 px and at 1280 px, both themes: the Evals page renders, a run's result table and fingerprint diff fit without horizontal scroll, axe reports no violations of the serious or critical level.
3. Hidden on a BYO agent and for a caller without `can_use` (no tab, no route access).
4. Start and cancel run through the API stubs; a 409 `eval_run_active` shows the inline notice, not a toast only.
5. `npm run lint`, `npm run typecheck` (or the repo's equivalents), `vitest`, production build all pass (exit codes reported).

## Verification
Per-task gates of `docs/tasks/LANE_RULES.md`: web lint, typecheck, vitest, Playwright for the page, production build. Report exact commands and exit codes.

## Dependencies
- Blocked by: P4-EVAL-1 (questions and settings routes). Run and schedule screens need P4-EVAL-2's routes; build those parts against the contract (C-864, C-870) with stubs until it merges.
- Unblocks: none.

## Suggested agent/model and reason
`nextjs-developer` (Sonnet): a multi-component page with forms, tables, states in two themes and the axe bar. Size **M**.

## Completion report
- Files changed: `web/src/components/agents/evals/{evals-tab,questions-section,question-dialog,question-form,judge-section,runs-section,run-detail,run-state,schedules-section,cron-words,fingerprint-diff,hash-value,copy}.ts(x)` (new), `web/src/lib/api/evals.ts` (the query/mutation hooks, one file per domain like `schedules.ts`), `web/src/components/agents/sections.ts` (`evals` page in Behaviour, after Skills), `agent-page.tsx` (one render line + import), `web/tests/{evals-page.test.tsx (new),agent-sections.test.ts}`, `web/e2e/{agent-evals.spec.ts,support/evals.ts}` (new) and the Evals screens added to `e2e/stage4-axe.spec.ts`, `stage4-routes.spec.ts`, `stage2-narrow.spec.ts`. No new `ui/` primitive; `web/package.json` and the lockfile untouched; the generated client was regenerated by P4-EVAL-1/2.
- Adapted from the 2026-11-15 brief: the Evals page is a page of the Behaviour section through `sections.ts` (P5-IA-1/P5-AGENT-2 structure; no nav entry). A BYO agent never has it (`isPageAvailable`); a caller without `can_use` does not exist inside a workspace (every role member/approver/owner can use), so the API's 404 shows as the page's error state. Question routes are `/v1/eval-questions/{id}` (not under the agent). A tablet-and-up table plus a stacked list on a phone (`md` split, the `scope-ladder` pattern) is used for questions, runs, results, schedules and the fingerprint diff; the tables pass `min-w-0` to `DataTable` so long text wraps instead of scrolling. The open run is page state, not `?run=` (nothing about results goes in the URL). Run results use the first page of `GET /v1/eval-runs/{id}` (≤ 100 results = the 100-question cap, so one page is the whole run). "Reorder by position fields" is the Position field of the edit dialog (the API shifts the others). `regression_notify` is editable by any member; only the judge choice is owner-only (the dialog says so). Judge key choices come from `GET …/model-readiness` (owner-only), so a non-owner sees the current judge read-only. Start run sends no body (the generated type has none). The mocked Playwright harness has no eval routes in the shared mock; `support/evals.ts` adds them on top.
- Tests/checks (exact commands and results): `pnpm -C web typecheck` exit 0; `pnpm -C web lint` 0 errors (5 existing warnings in files not touched); `pnpm exec vitest run tests/evals-page.test.tsx tests/agent-sections.test.ts tests/design-guards.test.ts tests/arbitrary-values.test.ts` 31/31; `AURORA_E2E_WEB_PORT=4183 AURORA_E2E_WEB_PROD_PORT=4184 AURORA_E2E_WEB_TRACING_PORT=4192 pnpm exec playwright test e2e/agent-evals.spec.ts e2e/stage4-axe.spec.ts e2e/stage4-routes.spec.ts e2e/stage2-narrow.spec.ts -g Evals --project mocked` 53/53 (both themes, 1280 and 390 px, axe serious/critical none, no horizontal page scroll, nothing in storage or the URL). **Fail-first seen** (each rule broken, 9 of the 15 vitest cases failed, restored): the owner-only judge control, the fingerprint diff (an unchanged field listed), the 100-question cap button, the judged-criteria check, the active-run block. Looked at screenshots at 1280 px and 390 px (light, dark). **Gate level: per task** (full vitest, production build and the whole Playwright suite are the batch gate's).
- Remaining risks: the run list's day count for the 20-a-day cap only counts the runs the list has loaded (the API's 429 is shown inline when it differs); the schedule cron preview describes the common shapes only (anything else says the times appear after saving) and its once-an-hour hint is a heuristic, the API's 422 stays authoritative; the fingerprint compare lists run ids truncated with a Copy action but the run page does not link to the eval conversations (they are not in the chat list); a rule-slip: three one-line string edits in `schedules-section.tsx` (the disabled-reason sentences) were made with `sed` instead of the Edit tool.
