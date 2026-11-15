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

- D-338, D-339, D-340, D-351; contracts **C-966, C-967**; uses C-963 (`ScopeLadder`), C-971
  (`SettingsSectionFields`), C-917 (accounts, for the keys notice).
- Normative: `14 §Information architecture` (AI models and Agents rows, the settings map),
  `08 §11` (model rules), `10 §Agents` (BYO).

## Wave and order

- **Wave 2.** Starts after P5-IA-1 (the pages, `SettingsSectionFields`), P5-IA-2 (`ScopeLadder`),
  P5-UX-4 (AI keys are accounts) and P5-UX-9 (which takes the app availability switch out of the old
  Governance view).
- Disjoint from P5-IA-4 (other pages) and P5-IA-5 (the settings form's rendering).

## Context

- **Administration:** `components/admin/settings/llm-settings-view.tsx`, `llm-controls.tsx`,
  `llm-endpoint-dialog.tsx`; the `governance` section's AI-key rules, BYO and agent-token keys; the
  `agents`, `approvals`, `terminal`, `previews` and `skills` sections; "Where organizations stand"
  (`GET /v1/admin/settings/governance/orgs`).
- **Organization:** `components/governance/governance-view.tsx` (Model access, Compliance with its count
  from `non_compliant_secrets` ~145, ~405–413), `byo-panel.tsx`; the Skills and libraries pages; Port
  previews.
- **Workspace:** `components/settings/settings-view.tsx` (General: the BYO setting).
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
   - **Workspace:** the agents list shows the workspace's BYO setting as a ladder row in its header (the
     organization's value above, the workspace's choice behind **Tighten…**). Remove it from Settings ›
     General.
3. **Remove what is left of Governance.** With this task, `governance-view.tsx` has no tabs left: delete
   the view (its route already redirects).
4. **Words.** Page and tab names follow the vocabulary. Field labels inside settings sections are
   P5-IA-5's.
5. **Tests.**
   - Vitest: each tab renders its content; ladder rows per level; the keys notice with zero and with
     some keys.
   - Playwright: an org admin narrows the allowed models and the page keeps Administration's limit
     visible above; a workspace's agents list shows the organization's BYO value; old URLs land on the
     right tabs.
   - Axe and narrow-route suites, both themes.

## Constraints / file ownership

- Owns: the AI models and Agents pages at each level; the moved components (composition);
  `byo-panel.tsx`; the workspace BYO field's move; the deletion of `governance-view.tsx`.
- No API change.

## Acceptance criteria

1. AI models and Agents are one page per level, with at most four tabs.
2. Limits from above are shown as locked rows with their source and who can change them.
3. Nothing about model rules, BYO, previews or skills remains under Governance or Settings, and the
   Governance page no longer exists.
4. Web gates; axe and narrow-route suites, both themes.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`): page composition over existing components across three levels.
