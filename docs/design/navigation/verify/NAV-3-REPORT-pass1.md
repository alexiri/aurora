# Places verification, pass 1 (P5-NAV-5): places, Manage and the place picker

Date: 2026-11-28. Reviewer lane, no product code changed.

## Result

**Not clean: 12 Match, 2 Minor difference, 2 Needs changes** (16 rows). The places model works for everyone I walked
as: one menu column, the back line returns to the exact page, members never see Manage, and the picker lists only places
the person can enter. Five things go back (C-1 to C-5): two are flow gaps, three are small.

## Environment

- Commit: `1e9f0c34` (master with P5-NAV-1..3 and the layer fix). `aurora-mem5` (:4530) redeployed with
  `kind-dev.sh up --llm-mock --ssh` through `heavy.sh`, then `kubectl rollout restart` of every deployment (`up` loads
  the new images but leaves the old Pods running). `aurora-dev` and `aurora-skm` untouched.
- People:
  - **Alice**: instance admin, owner of Acme (6 team workspaces), member of Globex.
  - **Bob**: member of Acme with one workspace.
  - **Carol**: member of Acme (7 workspaces), owner of Globex.
  - **Dave**: member of Acme (2 workspaces) and of Globex.
  - **Emergency reader**: Alice on Bob's workspace. I opened the grant and ended it.
- Behind the counts: two app requests from Bob (Manage 2), one chat platform request (Administration 1), and the
  `anthropics/skills` library on Acme with updates waiting.
- Playwright, 1256 and 390 px, dark and light on the captured screens; the walk-throughs mostly dark. Also 1256×783 for
  the rail. Axe, console, `/v1` 4xx and sideways overflow were recorded.
- Dave created a workspace through the API to check C-5; I deleted it.

## Registry

| Row | Verdict | As whom | Notes |
|---|---|---|---|
| One place | **Match** | all | Workspace, Manage, Administration and Your account each show one menu column, at both widths. No second menu in the page and no select used to change page (the selects on Account and Administration are form fields). |
| Place card | **Match** | all | The same box (16, 110, 223×50) in Manage, Administration and Your account. Kept by the lane: a chevron on every card, no teal ring. |
| Back line | **Minor** (C-2, C-3) | Alice, Bob, Carol | Returns to the exact page with its query, from the rail's foot, from the user menu and through Manage then Administration. A link with no record goes to the last workspace (in Manage, the last one in that organization). The "hidden with none" case is covered only by the mocked tests. |
| Manage menu | **Minor** (C-4) | Alice, Carol (Globex) | Entries, groups, labels, counts and the active entry match D-508(b). Tabs match the table. People shows Members · Groups; Directory sync appears only where the instance has SCIM (`14`). Groups and "Bring your own" need "Show more controls". |
| Workspaces | **Match** | Alice | All 9 listed with Name, Kind, Owners, Agents and Created, plus **New workspace**. Stacked rows at 390 px. Other people's personal workspaces say "Manage". |
| Overview | **Needs changes** (C-1) | Alice | Title, lead, four linked cards and the requests are right. The count and Needs you leave out the things the review names. |
| Titles | **Match** | Alice | All 11 Manage pages read "… in Acme"; Overview is "Acme". |
| Members | **Match** | Bob, Carol | All 17 Manage addresses I tried that exist send Bob to his workspace. (`/o/acme/usage` and `/o/acme/templates` were never pages and show "doesn't exist".) Bob has no Manage in the picker, the rail's foot or Ctrl K. Carol, who owns Globex, is sent from `/o/acme/people` to her own workspace. |
| Picker | **Needs changes** (C-5) | Alice, Bob, Carol, Dave, reader | Groups, order, wording, roles, counts, recent-first order and "All n workspaces" (owners go to Manage; members see the full list in the picker) match. Keyboard: focus starts in the search or on the first row, arrows move, Enter opens, Esc closes and focus returns to the card. The reader's workspace is listed only while on its pages, as "Emergency read access". "Other organizations" opens the last workspace there. The "or its Manage when you have none" case I didn't create live. |
| Search in picker | **Match** | Carol, Alice | Appears above 6 places (not for Bob or Dave). "plat" gives Platform team (Acme) and Platform (Globex), grouped, with "9 workspaces, 2 organizations". Carol finds Manage Globex but no Manage Acme. |
| Rail foot | **Match** | Alice, Carol, Bob, Dave | Manage <Org> above Administration, each with its count, for those who may enter. Carol sees Manage Globex only while in Globex. Not repeated inside its own place. |
| Ctrl K | **Match** | Alice, Bob, reader | Alice finds Manage Acme, Manage Globex, Administration and workspaces; Enter opens Manage with a working back line. Bob gets nothing for "Manage" or "Administration". |
| Phone | **Match** | Alice, Bob | Top bar place card; the picker opens as a sheet; the drawer shows the place's menu with its back line. The page never scrolls sideways; some tab strips and tables scroll inside themselves, as `14` allows. |
| N-4..N-8 | **Match** | — | N-4 no workspace rail beside Manage's menu. N-5 one model and one way back (the agent page's own menu is P5-NAV-4's). N-6 Manage is at the rail's foot and labelled in the picker. N-7 Your account has its own card and a back line. N-8 no native select, and the drawer shows Manage's menu. |
| Tests | **Match** | — | CI is green on `1e9f0c34`. See Tests and CI. |
| Rules | **Match** | — | Tokens and shared parts in both themes; axe and console clean on the places checked, apart from the notes below. `14`'s Places, Account and Manage paragraphs are rewritten to D-508. |

## Change list

### P5-NAV-2

**C-1 (flow): Manage's count and Needs you hold only requests.**
- Row: Overview. Files: the organization feed behind `GET /v1/orgs/{org}/attention` and `/v1/attention/counts`
  (`crates/aurora-db/src/repo/attention.rs`, C-1080), then `components/org/org-overview.tsx`.
- Mockup and text: `after/nav-3b.png` shows "Anthropic skills has an update to review" and a failed GitHub account in
  Needs you. `review.md` nav-3b says the count covers "library updates to review, broken organization accounts, people
  waiting to join".
- What differs: with updates waiting on the Anthropic skills library (`library-update-waiting-dark-1256.png`), the count
  is 2 and Needs you lists only Bob's two app requests (`overview-count-*.png`). C-1080 gives the organization feed only
  `app_request`, `chat_platform_request` and `ai_key_request`, so broken organization accounts can't appear either (I
  checked that one in the contract, not with a broken account). Waiting invitations are an uncounted row.
- What it should be: library updates the viewer can approve and broken organization accounts are counted items in Needs
  you, each linking to its page. The owner should say whether "people waiting to join" is counted too.
- Why it wasn't built: P5-NAV-2 ruled out API changes and asked for a missing route to be reported. Its report doesn't
  mention this one. It needs a contract change first (C-1080 and new attention kinds).

**C-2 (words): from your own workspace, the back line names you in the third person.**
- Row: Back line. File: `components/app-shell.tsx` (`useRecordPlace` in `SidebarContent`).
- What differs: Bob goes from his workspace to Your account. The card said "Your workspace"; the back line says
  "‹ Bob's workspace" (`back-line-name-*.png`). Carol gets "Carol's workspace" the same way. A direct link, with no
  record, correctly says "Your workspace". `nav-4.png` shows "Your workspace".
- Likely cause: the record takes its name from `useWorkspace` (`GET /v1/workspaces/{id}`), whose answer doesn't mark the
  workspace as the viewer's own. The card and the fallback use `/v1/me` memberships.
- What it should be: "Your workspace" for your own, in every place. The test should use the real shape of the workspace
  answer.

**C-3 (rough): after emergency access ends, the back line still goes to the workspace.**
- Row: Back line. File: `lib/nav/manage-return.ts` (`backLink`: a record marked `reader` is always trusted).
- What differs: Alice goes from Bob's agents page to Manage, ends the grant and opens another Manage page. The back line
  still says "‹ Bob's workspace" with the agents address. Clicking it lands on the workspace's manage page
  (`/o/acme/workspaces/dev-bob`), not on a workspace of hers. Nothing she shouldn't read is shown.
- What it should be: once the grant has ended, fall back to her last workspace. The function's own comment says "while
  it is still this person's to open".

**C-4 (rough): on a short window the current page's menu entry is out of sight.**
- Row: Manage menu. File: `components/app-shell.tsx` (the rail's scroller).
- What differs: at 1256×783, open `/o/acme/settings`. The rail scrolls by 109 px and isn't scrolled for you: the active
  **Settings** entry sits at y 627–663, below the visible menu (which ends at y 554). So no entry looks selected, and
  Oversight is cut off (`rail-783-*.png`). At 900 px high nothing scrolls. The lane reported the scrolling (its foot is
  taller than drawn), but not that the active entry can be hidden.
- What it should be: the rail brings the active entry into view when a place opens. A shorter foot would also help.

### P5-NAV-3

**C-5 (flow): a member who may create workspaces isn't offered it.**
- Row: Picker (New). File: `components/nav/scope-switcher.tsx` (`canCreateWorkspace: current.manage`).
- Spec: the table's New row says "People allowed to create them". `04` §authorization allows any member while
  `members_can_create_workspaces` is on, which is the default.
- What differs: Acme has the setting on. Dave, a plain member, created a workspace through the API (201), but Dave's and
  Carol's pickers offer only **New organization** (`picker-member-no-new-workspace-*.png`).
- The old switcher had the same limit, so this isn't a regression. But it no longer has any other door for a member,
  because Manage › Workspaces is closed to them.
- What it should be: **New workspace in <Org>** also shows for members while the organization allows it. If `/v1/me`
  doesn't carry the setting, that is a small contract change.

## Regressions

- **Deep links into Manage:** `/o/acme/apps/jira` (the address in Needs you), `skills?tab=libraries`, `data?tab=audit`
  and a workspace's manage page open in Manage's rail with the right entry marked. `/admin/apps?tab=oauth-clients` and
  `/admin/chat-platforms` do the same in Administration. I opened these addresses directly; I didn't send an e-mail.
- **Old addresses:** `/o/acme/governance` goes to AI models and `/o/acme/credentials` to Apps & accounts.
- **Administration entry:** works from the rail's foot and the picker.
- **SSH keys with `?from=`** (`/account/ssh`): "← Back to Daneel" and the rail's back line both work. Here they point to
  the same address.
- No new regression found.

## Tests and CI

- `e2e/places.spec.ts` and `e2e/place-picker.spec.ts`, mocked, through `test.sh` on `1e9f0c34`: **33 passed** (port
  4197, because another lane held 4100).
- `tests/design-guards.test.ts`: **17 passed**, so NAV-4's T-1 is fixed.
- No `ManageFrame` or "You're in Administration" expectation is left. The one match in `web/tests` asserts that "You're
  in" is absent; two test file headers still mention "Back to <Org>" in comments. The test diff since P5-NAV-2 adds no
  skip, `fixme` or `only`.
- CI for `1e9f0c34`, run 46092801164: **success** in every job that ran. `web`: 4,130 unit tests passed; the
  Playwright runs passed 2,188 (4 skipped) and 13. The runs for the P5-NAV-2 and P5-NAV-3 merges themselves
  (`b5755ed5`, `ec10907a`) were red in `web` on NAV-4's T-1.

## Other observations (not blocking, not counted)

- At 1256 px, `GET /v1/admin/setup` answered 404 once while walking Administration. The page worked; I didn't trace it.
- Manage › Internet access logs a 403 from `GET /v1/workspaces/{id}/activity` for workspaces Alice isn't in.
- The emergency banner prints two full stops when the reason ends with one ("(test cluster)..").
- In the picker's search, an organization's name finds its Manage entry but not its workspaces.

## Not checked

- Live: the back line hidden when there is no workspace, the card without a menu (everyone on this cluster may create an
  organization), and "Other organizations" opening Manage when the person has no workspace there.
- Directory sync (no SCIM on this cluster).

## Cluster

`aurora-mem5` is **still up**, for the next pass.
