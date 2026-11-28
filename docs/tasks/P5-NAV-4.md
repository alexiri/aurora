# P5-NAV-4: Review the agent pages against nav-2a; send back what differs; repeat until it matches (owner, 2026-11-27)

## Goal

After **P5-NAV-1** is merged, walk every agent page in the real app and compare it with the nav-2a mockups and D-507.
Whatever differs goes back to P5-NAV-1's lane (or a FIX once that lane has closed) with a change list precise enough to
act on; re-check **only those items** after the fix; **repeat until every row is Match**, or the owner accepts a
difference in writing. This brief is a review; it does not implement (small direct fixes: see Scope 5).

The owner (2026-11-27): "write briefs for nav-2a and nav-3, plus their reviews."

## Linked requirements

- `docs/design/navigation/README.md` (precedence, how to read a mockup — binding), `review.md` (N-1, N-2, N-3, N-10,
  nav-2a, "nav-2a, page by page"), `after/nav-2a*.png|html`, `after/nav-5.*` (middle frame), `before/*.png`.
- `docs/tasks/P5-NAV-1.md` and its completion report (read its listed differences first; a difference no brief or
  decision requires still goes back). D-507.

## Environment (read first)

- **Never** the shared `aurora-dev` cluster, `localhost:4000` or the `aurora` database in `aurora-dev-pg`. Use a throwaway
  cluster from your worktree (`AURORA_DEV_CLUSTER=<name> AURORA_DEV_PORT=<free port> deploy/dev/kind-dev.sh up
  --llm-mock`; `df -h /home` first) or, when the Orchestrator says one is up for review lanes, that one. Data: an agent
  with memories and skills (`docs/design/skills-memory/seed.py` makes them), one automation of each kind if the mock
  allows, the terminal on (`--ssh`) so Computer shows its sub-pages, a BYO agent, an emergency reader. Tear down what you
  started; never prune podman volumes. Heavy commands through `scripts/dev/heavy.sh`; tests through `scripts/dev/test.sh`.
- Capture at **1256 px and 390 px**, **dark and light**; full pages by scrolling and stitching. Commit only the report
  and the captures of rows that are not Match (`docs/design/navigation/verify/nav-2a-pass<N>/…`).

## The registry

| Row | Check | Mockup |
|---|---|---|
| Header | Same header on Chat, Automations, Computer and every Settings page: avatar, name, `@handle` (Expert only), status words (Ready, Asleep, Working, Starting its computer…), attention chip / Update available, actions menu labelled for the agent; it does not move between tabs | `after/nav-2a*.png` |
| Tabs | Underline tabs Chat · Automations · Computer · Settings; Settings is a tab (no button); active state; BYO and emergency variants | `after/nav-2a.png` |
| Second row | Pills only under Computer and Settings, same component for both, gating (altitude, features, Manage for owners), scroll with a fade, never wrap | `after/nav-2a.png`, `-computer` |
| Settings pages | No "‹ Chat", no h2 frame, no left sub-nav; content title row with its action; Memory and Skills list + detail side by side at 1256 px | `after/nav-2a.png` |
| Chat | Conversation list and status rail as content panels; matches the mockup | `after/nav-2a-chat.png` |
| Automations | Full width like every section; title, filter, list | `after/nav-2a-automations.png` |
| Computer | Files with the second row, breadcrumb, table, actions | `after/nav-2a-computer.png` |
| Phone | Compact header, single-line tab row and pill row, Chat opens on its list, no wrap, no horizontal page scroll | `after/nav-2a-phone.png` |
| Addresses | Every old `?tab=` and Computer sub-page address opens the right page | — |
| N-1..N-3, N-10 | Each finding's problem is gone (redo its Before screen) | `before/*` |
| Tests | CI green after the merge; no skipped or loosened test; no test still clicking a Settings **button** or expecting "‹ Chat" | CI, `rg` |
| Rules | `14` §Visual design on every agent page: tokens, shared primitives, both themes, 390 px; `14` updated to D-507 | — |

## Scope

1. Check every row: verdict **Match**, **Minor difference** or **Needs changes** (the last two go back), with capture and
   mockup paths.
2. **Change list** per difference: file or component, row, mockup path, **what differs** (measurable), **what it should
   be**, severity (broken / flow / rough / words).
3. Look for regressions on the agent pages (deep links from notifications, Try it in chat, the Status sheet, the
   terminal page).
4. **Report** `docs/design/navigation/verify/NAV-2A-REPORT-pass<N>.md`: counts, registry with verdicts, change list,
   regressions, environment (cluster, commit, torn down).
5. **Small direct fixes** (a token, an `aria-label`, a spacing value) only where no running lane owns the file, with their
   tests; list each.
6. **Send back and iterate**: message the Orchestrator (`SendMessage` to `Orchestrator`) with the change list; it queues
   the work in P5-NAV-1's lane or a FIX. When it reports the fix merged, re-check those rows and write the next pass.
   Done only when a pass ends with zero Needs changes and zero Minor difference (owner-accepted ones count as Match).

## Tests move with the UI

Any direct fix updates every test that sees it in the same commit (`rg` the old text in `web/tests`, `web/e2e`,
`scripts/e2e`), never loosens, skips or deletes a test, and runs only the touched tests and the drift guards through
`scripts/dev/test.sh`; GitHub CI is the full gate.

## Docs to update

The reports; PROGRESS (this row per pass) and FEATURES (`P5/14§agent-tabs` done when a pass is clean).

## Suggested agent/model and reason

Sonnet `sonnet-integration-reviewer` with a browser (comparison and change lists; no implementation).

## Completion report

Passes, verdict counts per pass, report paths, what was sent back and merged, owner-accepted differences, cluster torn
down.

**Pass 1 (2026-11-28, on `99c41755`): not clean — 9 Match, 1 Minor difference, 2 Needs changes.**

- Report: `docs/design/navigation/verify/NAV-2A-REPORT-pass1.md`; captures in `…/verify/nav-2a-pass1/`.
- To send back: C-1 (broken, CI red: the Terminal page scrolls since the new header; `.term-pane` height) to
  P5-NAV-1; C-2 (rough, older than P5-NAV-1: unfocusable code boxes on the BYO Connect page).
- Two owner questions, counted as Match: the agent "…" menu, and phone Chat opening on the conversation.
- Merged fixes: none yet. Owner-accepted differences: none recorded.
- No direct fixes and no product code changed, so no tests or fail-first evidence of this lane's own.
- Cluster `aurora-mem5` (:4530) **kept up** for P5-SKILLS-5 pass 2. PROGRESS and FEATURES not edited
  (`P5/14§agent-tabs` is not done).

**Pass 2 (2026-11-28, on `f7e5769b`, with `579cbf0f`): 11 Match, 0 Minor, 1 Needs changes.**

- Report: `docs/design/navigation/verify/NAV-2A-REPORT-pass2.md` (no non-Match captures: the open row is CI).
- C-1 and C-2 are fixed on the live stack: no page scroll for Terminal, Files, the editor or Chat at 1280×800 and
  1256×900 with more controls on or off (notice dismissed), and no axe finding on BYO Connect or the token dialog.
- To send back: T-1 (broken, CI red: the fix's unlayered `main … [data-fill-pane]` rules fail `design-guards`, so CI
  never reaches Playwright; `terminal.spec` and `agent-fill-pane.spec` pass 53/53 locally).
- Merged fixes since pass 1: `579cbf0f`. Owner questions still open.
- No product code changed. Cluster `aurora-mem5` **kept up** for P5-SKILLS-5 pass 3.
