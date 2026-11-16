# P5-IA-2: Internet access as one page at every level, and the scope ladder (owner, 2026-11-15)

## Goal

"What may agents reach on the internet" lived on four pages under three names: Administration's
governance settings, the organization's Governance › Egress and Policy, and the workspace's Policy. No
page showed what the level above set. The workspace page opened on operator controls most workspaces
never need, and the organization's page would have had "Save" and "Apply changes" side by side.

This task builds:
- the **scope ladder** (limits from above, this level's controls, what the levels below tightened);
- the **Internet access** page at each level, simple in a workspace (D-347);
- **one way to save** it;
- the sentences that summarize it.

## Linked requirements

- **Design** (D-354): the canvas “Aurora — agents, accounts and access”, https://claude.ai/artifact/3UwmUgrhh6MLCsfqdhTFmQ, boards Workspace › Internet access; Organization › Internet access; Read me › Patterns (scope ladder, Apply bar). Read each board's note; where a board and the spec disagree, the spec wins.
- D-339, D-347, D-351, D-353; contracts **C-926 (the Internet access labels), C-963, C-964, C-965, C-986
  (`internet.ts`), C-989**.
- Normative: `14 §Information architecture` (scope ladder, Internet access row), `14 §Agents, accounts and
  access` §8, `07 §Evaluation` and `§Authoring` (semantics unchanged), `08 §11.6` (postures).

## Wave and order

- **Wave 1.** Starts after P5-IA-1 (the pages exist, hosting the old editors with their old labels),
  P5-UX-1 (the `WhoCan` component, which ladder rows render) and FIX-159 (which edits
  `simulate-panel.tsx` first).
- Disjoint from P5-UX-4 and P5-UX-13, which run beside it.
- **Release gate (D-352):** wave 1 ships together with P5-IA-1.
- P5-IA-3 and P5-IA-4 use the ladder. P5-UX-7 builds the agent's summary on `internet.ts`.

## Context

- **Organization limits:** the egress panel inside `web/src/components/governance/governance-view.tsx`;
  saved with `PUT /v1/orgs/{org}/governance` (`{ document, revision }`, applied at once,
  `lib/api/governance.ts` ~58–70). `GET /v1/orgs/{org}/governance/effective` is readable by every member
  and marks each value `source: instance|org`.
- **Rules and defaults:** `components/policy/policy-view.tsx` (default rule ~312–315, uncredentialed
  ~334–343, tunnels ~348, the three buttons ~420–435, empty state ~240), `rule-editor.tsx`,
  `org-policy.tsx`, `workspace-policy.tsx`.
  - **Apply changes** calls `PUT …/policy/draft` then `POST …/policy/publish`; **Save draft** only the
    first; **Discard draft** writes the published generation back (`lib/api/policy.ts` ~81–98).
  - A workspace's defaults (default rule, requests without an account, browser connections, allow-list)
    are **in the same draft** as its rules (`policy.ts` ~210–224).
- **Check what would happen:** `components/policy/simulate-panel.tsx` (engine fixed by FIX-159).
- **Administration:** the `governance` section's egress keys, rendered by P5-IA-1's
  `SettingsSectionFields`, and "Where organizations stand" from
  `GET /v1/admin/settings/governance/orgs` (C-778).
- **By default** (`07 §Evaluation`, `08 §11.6`): in a fresh workspace with posture `open` and no rules, a
  request without an account is allowed, and a request with an account that no rule matches is allowed.
  The default rule applies only where an account's credential is injected.

## Scope

1. **`ScopeLadder` (C-963)** in the shared primitives:
   - **above** rows: label, the value in words, the source ("Set by Administration", "Set by <org>") and
     `WhoCan`;
   - **here**: this level's controls, with an optional sentence naming the limit ("Looser options are not
     available: Administration requires …");
   - a **collapsed** mode for workspaces: a **Tighten…** disclosure around the controls;
   - **below**: an optional table (columns, rows with links).

   Tokens and shared primitives only. At 390 px, rows stack and the table uses the stacked-row pattern.
2. **Summary sentences (C-986).** `web/src/lib/summary/internet.ts`: a pure function from a level's
   effective Internet access and a subject ("Agents here" on these pages, "It" in an agent's summary) to
   two sentences, worded as in `14` §8:
   - a **reach** sentence: "<Subject> can reach any website." / "<Subject> can reach only the websites
     on the allow-list." / "<Subject> can't reach the internet.";
   - an **ask** sentence: "Nothing asks you first." / "A rule asks you first on <n> websites."

   Each condition is a row of a table the unit tests walk. Every sentence must be true under
   `07 §Evaluation`; the reviewer checks the table against `07`, not against this brief.
3. **Workspace overrides (C-964).** `GET /v1/orgs/{org}/workspace-overrides` (org owner/admin): only
   workspaces that differ from the organization unless `?all=true`; paginated. OpenAPI, generated types,
   a `permission_matrix` row.
4. **Workspace page** `…/internet-access`, in this order:
   1. the summary, then the line "What agents may do in apps is set for each agent, on its Access tab."
      linking to Agents;
   2. **Common changes** (C-989): **Ask before changes on a website** and **Block a website**. Each
      asks for the website, adds an ordinary rule to the draft and shows it for confirmation. The first
      uses the words of the access choice on purpose: it is the same behaviour for a site where the
      agent has no account;
   3. the rules people wrote, with **Add rule**; empty: "No rules yet. Until you add one, requests
      follow the limits and defaults below.";
   4. the limits from above as ladder rows, and **Tighten…** holding the workspace's four controls;
   5. **Check what would happen**, with Speaker and Lane under **More**.

   Members see it read-only: no Common changes, Add rule or Tighten…, and one `WhoCan` sentence in place
   of Add rule.
5. **Labels (C-926).** Apply the Internet access renames of C-926 to these pages and to
   `simulate-panel.tsx`: **Let the organization decide** / **None: only rules decide**, **Requests
   without an account**, **Browser connections (not inspected)**, **Check what would happen**.
6. **Organization page** `/o/<org>/internet-access`: the same order, with the organization's limits open
   (posture, allow-list, browser connections), Administration's limits above them, and the workspaces
   table below.
7. **Administration page** `/admin/internet-access`: the `governance` egress and browser-connection keys
   (`SettingsSectionFields` with `keys`) and the organizations table.
8. **One way to save (C-989).**
   - One pending-changes bar with **Apply changes** and **Discard**. "Save draft" is removed; edits to
     rules and workspace defaults are written to the policy draft as they are made.
   - Organization: Apply sends the governance document when it changed, then publishes the draft. If the
     second step fails, the bar says which part was applied and keeps the rest pending.
9. **Remove the hosted editors** that P5-IA-1 placed on these pages; their components are reused here.
10. **Tests.**
   - Vitest: `ScopeLadder` (above, here, collapsed, below, read-only); the summary table, including the
     fresh workspace ("Agents here can reach any website. Nothing asks you first."); each Common change
     produces the expected draft rule; the save bar (workspace; organization with the second step
     failing).
   - API tests for C-964: difference detection, pagination, refusal for members.
   - Playwright:
     - an org admin tightens the allow-list and a workspace's page shows it as a limit from above;
     - a workspace that tightened appears in the organization's table;
     - a workspace owner blocks a website with a Common change and Apply;
     - the workspace page shows no default controls until **Tighten…** is opened.
   - Axe and narrow-route suites for the three pages, both themes.

## Constraints / file ownership

- Owns: the `ScopeLadder` primitive; `lib/summary/internet.ts`; the three Internet access pages; the
  egress panel and policy components (composition, labels, the save bar); `simulate-panel.tsx` (labels
  and the More disclosure; FIX-159's result text stays); the Common changes component; the C-964 route.
- No policy-semantics change and no API change other than C-964.
- Access is not shown as rules on these pages (D-346).

## Acceptance criteria

1. Internet access is one page at each level; nothing about it remains on another page.
2. A workspace's page is the summary, two Common changes, the rules people wrote, the limits from above
   and one closed **Tighten…**.
3. Each page has one Apply.
4. The summary's sentences are true for every row of the test table.
5. Rust and web gates; axe and narrow-route suites, both themes.

## Suggested agent/model and reason

Sonnet: one shared primitive, a small API route, a pure function whose truth matters, and three page
compositions over existing components.

## Completion report — PARTIAL (lane `p5-apps`, worker 20; handed off at the context limit, 2026-11-17)

**Not done: P5-IA-2 as a whole.** Built and committed (`P5-IA-2: part 1`): scope item 1 (the `ScopeLadder` primitive) and item 2 (the summary sentences), both with unit tests. Everything else below is open.

**Done.**
- `web/src/components/ui/scope-ladder.tsx` (C-963): `above` rows (label, value in words, "Set by Administration" / "Set by your organization", `WhoCan`), `here`, optional `minimumNote`, `collapsed` (**Tighten…**, `aria-expanded`/`aria-controls`, closed until opened), `below` (a `DataTable` from `md` up and a stacked `ItemList` below it, an `empty` sentence). Tests: `web/tests/scope-ladder.test.tsx` (6). A `readOnly` prop is not built: a member's page simply passes no controls and shows `MainAction`/`WhoCan` (P5-UX-1) in place of Add rule.
- `web/src/lib/summary/internet.ts` (C-986): `reachSentence`, `askSentence`, `internetSummary`, `internetAccessOf(generation, aboveBlocks)`. The table in `web/tests/internet-summary.test.ts` (20 rows + 3) walks each condition and says why it is true under `07 §Evaluation`. Deviations from the brief's wording, all for truth: a blocking rule gives "…can reach any website, except where a rule blocks it."; an enabled allow rule under `block` gives "…only the websites on the allow-list and those a rule allows."; a rule with no targets matches every request ("A rule asks you first on every website." / "can't reach the internet."); a rule whose targets are secrets or apps says nothing about websites ("A rule asks you first."). The reviewer should check the table against `07`, as the brief says.

**Open, in the brief's order.**
3. C-964 route `GET /v1/orgs/{org}/workspace-overrides` (org owner/admin; only workspaces that differ unless `?all=true`; `Page<{ workspace: { id, name, personal_of? }, uncredentialed: "inherit"|"block", open_lane: "inherit"|"allowlist_only"|"disabled", default_rule: "none"|"allow"|"block", rule_count }>`), OpenAPI, `gen:api`, a `permission_matrix` row, IT tests (difference detection, pagination, member refused). Not started.
4./6./7. The three pages (workspace `…/internet-access`, organization `/o/<org>/internet-access`, Administration `/admin/internet-access`): P5-IA-1's placeholders are still there; compose the summary (item 2), the line "What agents may do in apps is set for each agent, on its Access tab." (link to Agents), Common changes (C-989: **Ask before changes on a website**, **Block a website**), the rules with **Add rule**, the ladder with **Tighten…** (workspace) or open limits (organization), **Check what would happen** with Speaker and Lane under **More**, and the organizations/workspaces tables. Reuse `policy-view.tsx`, `rule-editor.tsx`, `org-policy.tsx`, `workspace-policy.tsx`, the egress panel of `governance-view.tsx`, `SettingsSectionFields` with `keys` for Administration (C-778 `GET /v1/admin/settings/governance/orgs` for its table).
5. C-926 labels on these pages and in `simulate-panel.tsx` (FIX-159's result text stays).
8. One save bar (**Apply changes** / **Discard**; no "Save draft"; edits write to the draft as they are made; organization: governance document first, then publish, with the "which part was applied" message when the second step fails).
9. Remove the hosted editors P5-IA-1 placed on these pages.
10. The remaining tests: save bar (workspace; organization with the second step failing), each Common change's draft rule, the four Playwright journeys, axe and narrow-route suites for the three pages in both themes.

**Where things are.** `MainAction`, `WhoCan`, `useWhoCan` (P5-UX-1) are in `components/ui` and `lib/api/who-can.ts`; the policy view's view-only notice is already a `WhoCan` sentence. Contracts are in `docs/API_CONTRACTS.md` C-963..C-965, C-986, C-989 (read on master; this branch has them after the merge of ac36d862). The batch gate for P5-IA-2 has not run (only `pnpm typecheck` and the two new vitest files).
