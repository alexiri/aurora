# P5-IA-3: AI models and Agents: limits from above, merged pages (owner, 2026-11-15)

## Goal

P5-IA-1 moved the existing pieces under two topics. This task makes each a coherent page:
- **AI models** (Administration and organization): which AI endpoints and models agents may use. The
  organization's rules were a Governance tab, with a separate Compliance tab for keys that break them.
- **Agents** (Administration, organization, workspace): bring-your-own (BYO) agents, agent defaults,
  computers, skills. BYO was set on three pages under three names, none showing the level above.

Each page shows the limits from the level above with the scope ladder, and no page has more than four
tabs (D-338, D-339, D-351).

## Linked requirements

- **Design** (D-354): the canvas “Aurora — agents, accounts and access”, https://claude.ai/artifact/3UwmUgrhh6MLCsfqdhTFmQ, boards Organization › AI models and Agents › BYO; Administration › AI models › Endpoints and Agents › Defaults. Read each board's note; where a board and the spec disagree, the spec wins.
- D-338, D-339, D-340, D-351; contracts **C-966, C-967**; uses C-963 (`ScopeLadder`), C-971
  (`SettingsSectionFields`), C-917 (accounts, for the keys notice).
- Normative: `14 §Information architecture` (AI models and Agents rows, the settings map),
  `08 §11` (model rules), `10 §Agents` (BYO).

## Wave and order

- **Wave 2.** Starts after P5-IA-1 (the pages, `SettingsSectionFields`), P5-IA-2 (`ScopeLadder`),
  P5-UX-4 (AI keys are accounts) and P5-UX-9 (which takes the app availability switch out of the old
  Governance view).
- Disjoint from P5-IA-4 (other pages; it also moves the workspace's BYO setting out of Settings, so this
  task does not touch `components/settings/settings-view.tsx`) and P5-IA-5 (the settings form's
  rendering).

## Context

- **Administration:** `components/admin/settings/llm-settings-view.tsx`, `llm-controls.tsx`,
  `llm-endpoint-dialog.tsx`; the `governance` section's AI-key rules, BYO and agent-token keys; the
  `agents`, `approvals`, `terminal`, `previews` and `skills` sections; "Where organizations stand"
  (`GET /v1/admin/settings/governance/orgs`).
- **Organization:** `components/governance/governance-view.tsx` (Model access, Compliance with its count
  from `non_compliant_secrets` ~145, ~405–413), `byo-panel.tsx`; the Skills and libraries pages; Port
  previews.
- `GET /v1/orgs/{org}/governance/effective` marks each value `source: instance|org`.

## Scope

1. **AI models (C-966).**
   - **Administration** `/admin/ai-models`: tabs **Endpoints** (the `llm` section's views) and **Rules**
     (the `governance` AI-key rules), with each organization's effective values as a table under Rules.
   - **Organization** `/o/<org>/ai-models`: one page.
     - Administration's limits as ladder rows above.
     - The organization's model rules (was Governance › Model access).
     - A notice when AI keys no longer follow the rules: "<n> AI keys no longer follow these rules",
       linking to the organization's and workspaces' Accounts lists filtered to AI keys (was the
       Compliance tab). The count comes from the same source as today.
   - There is no workspace page: AI keys are rows of the Accounts list.
2. **Agents (C-967).**
   - **Administration** `/admin/agents`: four tabs.
     - **Defaults**: the `agents` and `approvals` sections.
     - **BYO**: the `governance` BYO and agent-token keys.
     - **Computers**: the `terminal` and `previews` sections.
     - **Skills**: the `skills` section.
   - **Organization** `/o/<org>/agents`: tabs **BYO** (was Governance › BYO agents, with
     Administration's limits as ladder rows above), **Skills** (with libraries) and **Previews**.
   - **Workspace:** nothing in this task. The workspace's BYO setting moves to the agents list with
     P5-IA-4.
3. **Remove what is left of Governance.** With this task, `governance-view.tsx` has no tabs left: delete
   the view (its route already redirects).
4. **Words.** Page and tab names follow the vocabulary. Field labels inside settings sections are
   P5-IA-5's.
5. **Tests.**
   - Vitest: each tab renders its content; ladder rows per level; the keys notice with zero and with
     some keys.
   - Playwright: an org admin narrows the allowed models and the page keeps Administration's limit
     visible above; old URLs land on the right tabs.
   - Axe and narrow-route suites, both themes.

## Constraints / file ownership

- Owns: the AI models pages and the Agents pages of Administration and the organization; the moved
  components (composition); `byo-panel.tsx`; the deletion of `governance-view.tsx`.
- No API change.

## Acceptance criteria

1. AI models and Agents are one page per level, with at most four tabs.
2. Limits from above are shown as locked rows with their source and who can change them.
3. Nothing about model rules, BYO, previews or skills remains under Governance, and the Governance page
   no longer exists.
4. Web gates; axe and narrow-route suites, both themes.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`): page composition over existing components across three levels.

## Completion report (lane p5-apps, 2026-11-17)

Commit: 31684fea. Web only; no Rust, API, OpenAPI or schema change.

- **Already in place from P5-IA-1** (verified, not rebuilt): the routes, tab sets and redirects of Administration › AI models (Endpoints, Rules with "Where organizations stand" under Rules), Administration › Agents (Defaults, BYO, Computers, Skills) and the organization's Agents (BYO, Skills, Previews); `ia-navigation.spec.ts` checks the old addresses.
- **Organization › AI models (C-966):** one page, no tabs (`OrgAiModels`, `components/governance/org-governance.tsx`): the keys notice ("2 AI keys no longer follow these rules", links to the organization's and each affected workspace's Accounts filtered to AI keys; same source, `non_compliant_secrets`; "All AI keys follow these rules." for none), then a `ScopeLadder` with Administration's limits as rows (AI key posture, endpoints organizations may use, key scopes, allowed models; each only when the instance decides it, with source and who can change it), then the organization's model rules and one Save / Discard changes (Save is off until something changed). Members read it with no buttons.
- **Organization › Agents › BYO (C-967):** `OrgByo`: Administration's limits as ladder rows (who may run agents outside Aurora, source list, context credentials, longest token), then `ByoPanel` and the same Save / Discard changes.
- **Governance removed:** `governance-view.tsx` is deleted; `Locked`, `Errors`, `PostureSelect`, `TextArea` are `governance/controls.tsx`, `EgressTab` is `governance/egress-tab.tsx`, `ModelsTab` is `governance/models-tab.tsx`. The saved notice lives in the parent: a save brings a new revision, which re-keys the form (the old view lost its "Saved" for the same reason; found by the new Playwright test).
- **Administration › Agents:** Defaults is the agents groups, the runners table and the approvals group on one tab, Computers is the terminal and previews groups, Skills has no second header (`bare` on the operational views). AI models › Endpoints drops the duplicate page header. Retired words ("Credentials", "Enable") removed from the AI endpoint dialog and controls; the four list entries of `ui-text.test.ts` for the deleted/cleaned files are removed.
- **Left / differences:** the stacked forms (agents + approvals, terminal + previews) are one run of titled groups, but each section is its own document with its own generation and preview, so each still has its own Save and Discard changes; one shared Save needs `settings-section-form.tsx` (P5-IA-5's file) to expose its draft. Workspace: nothing, as specified. The org ladder rows have no "below" table (D-351).
- **Gates (web):** vitest 126 files / 2092 tests pass; typecheck; lint (0 errors; 4 existing warnings); guards (exit 0); `extract-schema --check`; Playwright mocked project 1069 passed (incl. stage4-axe, stage4-routes, ia-navigation, who-can, accounts, connect, new `e2e/ia-3.spec.ts` 16 passed in both themes at 1280 and 390 px); production project 8 passed (it runs `next build`). The new e2e's "Saved." check failed before the re-key fix. Not run: cargo clippy/nextest, OpenAPI drift and `gen:api` (no Rust or API touched), `hosted.sh` (connect path unchanged).
- **Rule note:** the `GovernanceView` → `OrgByo` and `Save governance` → `Save` replacements across byo/agent-tokens/admin-settings-governance tests used `sed` (the same replacement in many places).
- **e2e note:** at 390 px `next dev`'s indicator sits over a Save that ends the page, so the new spec dispatches that click.
