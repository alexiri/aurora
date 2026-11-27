# P5-NAV-5: Review places, Manage and the place picker against nav-3; send back what differs; repeat until it matches (owner, 2026-11-27)

## Goal

After **P5-NAV-2** and **P5-NAV-3** are merged, walk every place in the real app (workspaces, Manage <Org>,
Administration, Your account, the picker, the phone) as several people, and compare with the nav-3 mockups, the spec
tables and D-508. Whatever differs goes back to the owning brief's lane (or a FIX once it has closed) with a precise
change list; re-check **only those items** after the fix; **repeat until every row is Match**, or the owner accepts a
difference in writing. This brief is a review; it does not implement (small direct fixes: Scope 5).

The owner (2026-11-27): "write briefs for nav-2a and nav-3, plus their reviews. All your proposed decisions are accepted."

## Linked requirements

- `docs/design/navigation/README.md` (binding), `review.md` (N-4..N-8, nav-1, nav-3, nav-4, nav-5, "nav-3 in depth" with
  its two tables, the owner's decisions), `after/nav-1*`, `nav-3*`, `nav-4*`, `nav-5*`, `before/*`.
- `docs/tasks/P5-NAV-2.md`, `P5-NAV-3.md` and their completion reports. D-508, C-1178.

## Environment (read first)

- **Never** the shared `aurora-dev` cluster, `localhost:4000` or the `aurora` database. Use a throwaway cluster from your
  worktree (`AURORA_DEV_CLUSTER=<name> AURORA_DEV_PORT=<free port> deploy/dev/kind-dev.sh up --llm-mock`; `df -h /home`
  first) or the review cluster the Orchestrator names. People and data: an instance admin who owns the organization
  (Alice), a member with one workspace (Bob), a person in several workspaces of **two organizations** (create a second
  organization and enough workspaces to pass the search threshold), something behind Manage's count (a library update to
  review or a failing organization account) and Administration's. Tear down what you started; never prune podman volumes.
  Heavy commands through `scripts/dev/heavy.sh`; tests through `scripts/dev/test.sh`.
- Capture at **1256 px and 390 px**, **dark and light**; commit only the report and the captures of rows that are not
  Match (`docs/design/navigation/verify/nav-3-pass<N>/…`).

## The registry

| Row | Check | Mockup / spec | Owner |
|---|---|---|---|
| One place | Every place shows one menu column: workspace, Manage, Administration, Your account; no second column anywhere (phone: no native select) | `after/nav-3b.png`, `nav-4.png` | P5-NAV-2 |
| Place card | Same component and position in every place; back line outside a workspace; Your account has one | `after/nav-4.png` | P5-NAV-2 |
| Back line | Returns to the exact workspace page you came from; link with no record → last workspace in the org; none → hidden | D-508(h), C-1178 | P5-NAV-2 |
| Manage menu | Grouped entries and labels exactly as D-508(b); counts; active entry; tabs inside pages as the table "Entry · Today · Tabs inside" | `after/nav-3b.png`, `review.md` table | P5-NAV-2 |
| Workspaces | `/o/<org>/workspaces` lists every workspace with New workspace; Overview no longer carries the list | C-1178 | P5-NAV-2 |
| Overview | Title and lead, summary cards (links), Needs you with only what an owner/admin can act on | `after/nav-3b.png`, D-508(j) | P5-NAV-2 |
| Titles | "Agents in <Org>", "People in <Org>"… | D-508(i) | P5-NAV-2 |
| Members | Bob sees no Manage in the picker, the rail foot or search; every Manage address redirects him; no link dead-ends | D-508(e), C-1178 | P5-NAV-2/-3 |
| Picker | Groups, order, visibility and wording per person exactly as the table "Picker group · What it lists · Who sees it"; ✓, roles, counts, All n workspaces, Other organizations → last workspace; static card when nothing else; keyboard | `after/nav-3a.png`, table | P5-NAV-3 |
| Search in picker | Appears above 6 places; crosses organizations; grouped results | `after/nav-3a.png` (third) | P5-NAV-3 |
| Rail foot | Manage <Org> above Administration for owners/admins/instance admins, with counts | `after/nav-3.png` (second frame) | P5-NAV-3 |
| Ctrl K | Finds places for those who may enter them | D-508(g) | P5-NAV-3 |
| Phone | Top bar place card, picker as a sheet, drawer with the place's menu and back line; no horizontal scroll | `after/nav-3d.png`, `nav-5.png` | P5-NAV-3 |
| N-4..N-8 | Each finding's problem is gone (redo its Before screen) | `before/*` | — |
| Tests | CI green after each merge; no skipped or loosened test; no test expecting `ManageFrame`, "You're in Administration" or the old switcher rows | CI, `rg` | each lane |
| Rules | `14` §Visual design on every place page; `14`'s shell and Manage paragraphs rewritten to D-508 | — | each lane |

## Scope

1. Check every row as every relevant person: verdict **Match**, **Minor difference** or **Needs changes** (the last two
   go back), with capture and mockup paths.
2. **Change list** per difference: owning brief and file, row, mockup or spec reference, **what differs** (measurable),
   **what it should be**, severity (broken / flow / rough / words).
3. Look for regressions: deep links into Manage pages from notifications and e-mails, the Administration entry, Account
   pages opened from elsewhere (SSH keys with `?from=`), legacy redirects.
4. **Report** `docs/design/navigation/verify/NAV-3-REPORT-pass<N>.md`: counts, registry with verdicts per person, change
   list grouped by brief, regressions, environment (cluster, commit, torn down).
5. **Small direct fixes** only where no running lane owns the file, with their tests; list each.
6. **Send back and iterate**: message the Orchestrator (`SendMessage` to `Orchestrator`) with the change list grouped by
   brief; it queues the work. Re-check fixed rows in the next pass. Done only when a pass ends with zero Needs changes
   and zero Minor difference (owner-accepted ones count as Match).

## Tests move with the UI

Any direct fix updates every test that sees it in the same commit (`rg` the old text in `web/tests`, `web/e2e`,
`scripts/e2e`), never loosens, skips or deletes a test, and runs only the touched tests and the drift guards through
`scripts/dev/test.sh`; GitHub CI is the full gate.

## Docs to update

The reports; PROGRESS (this row per pass) and FEATURES (`P5/14§places` done when a pass is clean).

## Suggested agent/model and reason

Sonnet `sonnet-integration-reviewer` with a browser.

## Completion report

Passes, verdict counts per pass, report paths, what was sent back and merged, owner-accepted differences, cluster torn
down.
