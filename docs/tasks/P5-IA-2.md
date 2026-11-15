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

- D-339, D-347, D-351; contracts **C-963, C-964, C-965, C-986 (`internet.ts`), C-989**.
- Normative: `14 §Information architecture` (scope ladder, Internet access row), `14 §Agents, accounts and
  access` §8, `07 §Evaluation` and `§Authoring` (semantics unchanged), `08 §11.6` (postures).

## Wave and order

- **Wave 1.** Starts after P5-IA-1 (the pages exist, hosting the old editors, with the new labels).
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
   effective Internet access to ordered sentences. Each condition is a row of a table the unit tests
   walk. Every sentence must be true under `07 §Evaluation`; the reviewer checks the table against `07`,
   not against this brief.
3. **Workspace overrides (C-964).** `GET /v1/orgs/{org}/workspace-overrides` (org owner/admin): only
   workspaces that differ from the organization unless `?all=true`; paginated. OpenAPI, generated types,
   a `permission_matrix` row.
4. **Workspace page** `…/internet-access`, in this order:
   1. the summary, then the line "What agents may do in apps is set for each agent, on its Access tab."
      linking to Agents;
   2. **Common changes** (C-989): **Ask me before changes on a website** and **Block a website**. Each
      adds an ordinary rule to the draft and shows it for confirmation;
   3. the rules people wrote, with **Add rule**;
   4. the limits from above as ladder rows, and **Tighten…** holding the workspace's four controls;
   5. **Check what would happen**.

   Members see it read-only.
5. **Organization page** `/o/<org>/internet-access`: the same order, with the organization's limits open
   (posture, allow-list, browser connections), Administration's limits above them, and the workspaces
   table below.
6. **Administration page** `/admin/internet-access`: the `governance` egress and browser-connection keys
   (`SettingsSectionFields` with `keys`) and the organizations table.
7. **One way to save (C-989).**
   - One pending-changes bar with **Apply changes** and **Discard**. "Save draft" is removed; edits to
     rules and workspace defaults are written to the policy draft as they are made.
   - Organization: Apply sends the governance document when it changed, then publishes the draft. If the
     second step fails, the bar says which part was applied and keeps the rest pending.
8. **Remove the hosted editors** that P5-IA-1 placed on these pages; their components are reused here.
9. **Tests.**
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
  egress panel and policy components (composition, the save bar); the Common changes component; the C-964
  route.
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
