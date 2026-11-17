# P5-USAGE-2: Usage views at agent, workspace and Administration level (owner, 2026-11-17)

## Goal

Show the usage P5-USAGE-1 records (owner: "add dashboards, at agent, workspace, and instance level. No
use collecting data nobody can see."). It covers tokens, turns, turn time and awake time, with no cost.
There is one `UsageView` in four places: Activity › Usage in a workspace, a Usage card on an agent's
Computer page, the organization's Data › Usage (owner: "yes, add an organization-level view too"), and
Administration › Data › Usage.

## Linked requirements

- D-359, D-360.
- Contracts **C-1000**, **C-1003** and **C-1006** (the data; read their response shapes), **C-1004** (placement,
  URL state), **C-1005** (the `StatTile` and `BarChart` primitives), all in `docs/API_CONTRACTS.md`
  §Phase 5 — P5-USAGE.
- Normative: `14 §Usage` (the view, words, numbers, states; quote its texts exactly); `14 §One structure
  for every level` (topic table, Activity tabs, who sees what, C-961); `14 §Visual design` (tokens,
  primitives, both themes, 390 px) and the design canvas of D-354 for the look of cards, tables and
  segmented controls.

## Context

- Workspace Activity: `web/src/app/o/[org]/w/[ws]/activity/page.tsx` renders `ActivityView`
  (`web/src/components/approvals/activity-view.tsx`). It has no tabs yet. Other topics use `TopicPage`
  with `tabs` (see `AdminDataPage` in `web/src/components/admin/admin-topics.tsx`).
- Administration › Data: `AdminDataPage`, tabs `retention`, `storage`, `audit`; tab ids are typed in
  `web/src/lib/routes.ts` (`adminData`). The organization's Data page is its sibling in the org topic
  components (find it from `/o/[org]/data`).
- Agent Computer page: `web/src/components/agents/computer-tab.tsx`.
- Primitives: `web/src/components/ui` (`Segmented`, `DataTable`, `Card`, `Section`, `Field`, `Notice`,
  `Meter`, …). There is no stat tile and no chart; add both (C-1005), no chart library.
- Types: P5-USAGE-1 regenerates `web/src/lib/api/schema.d.ts` with `getWorkspaceUsage` and
  `getInstanceUsage`; query hooks go beside the others in `web/src/lib/api/`.
- Tests: vitest in `web/tests/*.test.tsx` (e.g. `activity-agent-filter.test.tsx`), Playwright in
  `web/e2e/*.spec.ts`.

## Scope

1. `StatTile` and `BarChart` in `web/src/components/ui` exactly as C-1005, each with a unit test
   (accessible names, keyboard movement between bars, tooltip text, no colour-only meaning).
2. `UsageView` (new `web/src/components/usage/`) as `14 §Usage`: the opening sentence, the one-row
   filters with URL state, four tiles, the per-day chart with the measure switch and **Show as table**,
   grouping by week below 4 px per bar, the tables (by agent, by model; Administration: by organization,
   top workspaces with "Other workspaces"), deleted names, empty, loading and error states, number and
   duration formatting.
3. Placement (C-1004): Activity tabs **Requests** (default, unchanged content) and **Usage**; the agent
   filter mode (heading names the agent, agent chip, no By-agent table); the Computer page's Usage card
   with **See usage**; the organization's Data › **Usage** tab with the workspace filter;
   Administration › Data's **Usage** tab with the organization filter. `routes.ts` helpers and tab ids;
   Activity without `tab` still opens Requests.
4. Who sees it: Activity's readers (C-961) in a workspace; org owners and admins in the organization
   (as its other Data tabs); instance admins in Administration. Nothing
   new is hidden or shown beyond that.

## Constraints / file ownership

- Owns: `web/src/components/ui/{stat-tile,bar-chart}.tsx`, `web/src/components/usage/**`, the usage
  query hooks, `web/src/app/o/[org]/w/[ws]/activity/page.tsx` (tabs only), the tab lists of
  `AdminDataPage` and the organization's Data page, `routes.ts` (the Activity, `adminData` and org Data
  tab entries), the Usage card in `computer-tab.tsx`, and
  their tests.
- Does not change `ActivityView`'s request log; P5-MODELS-3 owns the request log's model column.
- Tokens and primitives only; no raw colours; both themes; no horizontal scroll at 390 px; no new
  dependency. Never show or compute cost.
- If another lane is editing `admin-topics.tsx`, `routes.ts` or `computer-tab.tsx` when this starts,
  rebase on it first. These are small, shared files.

## Acceptance criteria (each test seen failing first)

1. Unit tests for `StatTile` and `BarChart` (scope 1); `UsageView` tests with mocked responses: range
   switch updates the URL and the query, Custom over 366 days shows the API's message, zero days present,
   week grouping, "Deleted agent" / "Unknown model", "Not counted yet" for a null `runtime_as_of`,
   the empty state.
2. Placement tests: Activity defaults to Requests and old links still work; `?tab=usage&agent=` shows the
   agent mode; the Computer card links there; the organization's Data shows four tabs and the workspace
   filter for owners and admins, and no Usage for a plain member; Administration › Data shows four tabs
   and the organization filter; Administration stays hidden from people who are not instance admins (C-961).
3. Playwright (`web/e2e/usage.spec.ts`): the four places render against the dev stack after a turn,
   light and dark, at 1280 px and 390 px with no horizontal scroll (the existing layout checks).
4. `npm run typecheck`, `npm run lint`, `npm test` and `npm run build` green.

## Dependencies

- **P5-USAGE-1** merged (routes and generated types).
- Activity and Administration › Data as left by P5-IA-1 (merged).

## Suggested agent/model and reason

Sonnet (`nextjs-developer`): two new primitives with accessibility rules, one view in three places, and
URL state. It is bounded, but more than a Haiku task.

## Completion report (worker 33, 2026-11-18)

Built: `StatTile`, `BarChart` (inline SVG, no style attributes, tab stop + arrow/Home/End keys, tooltip, "<label>: <value>" names), `UsageView` (`web/src/components/usage/usage-view.tsx`, three levels, URL state `range|from|to|agent|workspace|org`), `UsageCard`, hooks `web/src/lib/api/usage.ts`, helpers `web/src/lib/usage-format.ts`; Activity gains `?tab=requests|usage` (default Requests), Data › Usage in the organization and Administration, the Computer page's Usage card with **See usage**; `routes.ts` tab ids + `wsRoutes().usage(agent?)`.

Gate: vitest 143 files / 2323 tests, typecheck, lint (0 errors, 5 old warnings), `pnpm build`; Playwright mocked project 1264 passed (incl. stage4-axe, stage4-routes, ia-navigation, new `usage.spec.ts` in both themes at 1280 and 390 px with axe and no horizontal scroll), 1 failed under load (`web-push.spec.ts:97`, a service-worker NotificationEvent error) which passes alone; production project 11 passed.

Not done / differences from the brief: (1) **`TurnUsage` is still unmounted**: no screen of this brief, and no screen in the dashboard, shows a turn's tokens (no turn-details view exists; the chat transcript has none), so mounting it would mean inventing one. (2) Administration's workspace names are plain text (the admin has the workspace id only, no slug to link with); the organization view links them. (3) Tables scroll inside their own region on a phone instead of becoming stacked cards. (4) The Playwright spec runs against the mocked API, not the dev stack after a real turn. (5) Several multi-line edits used python/sed (bulk replace of repeated snippets), against the Edit-only rule.
