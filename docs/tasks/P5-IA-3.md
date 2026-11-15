# P5-IA-3: AI models and Agents as topics at every level (owner, 2026-11-15)

## Goal

Two topics are spread over unrelated pages.
- **AI models:**
  - instance: Settings › LLM endpoints, plus the LLM floors inside Settings › Governance;
  - organization: Governance › Model access and Compliance, plus Credentials › LLM keys;
  - workspace: Credentials › LLM keys.
- **Bring-your-own (BYO) agents and agent settings:**
  - instance: Settings › Governance › BYO agents and Agent tokens, plus the (still empty) Agents &
    sandboxes, Approvals, Terminal, Previews and Skills sections;
  - organization: Governance › BYO agents, plus Skills and Port previews;
  - workspace: Settings › General's BYO posture.

This task makes each one topic page per level, using the scope ladder (D-338..D-340).

## Linked requirements

- D-338, D-339, D-340; contracts **C-966, C-967**; uses C-963 (`ScopeLadder`), C-964 (below tables), C-971
  (`SettingsSectionFields`), C-973 (read-only current values).
- Normative: `14 §Information architecture` (AI models and Agents rows, settings → topic map),
  `08 §11` (model governance), `10 §Agents` (BYO).

## Context

- **Instance:**
  - `components/admin/settings/llm-settings-view.tsx`, `llm-controls.tsx`, `llm-endpoint-dialog.tsx`;
  - `governance-settings-view.tsx` (LLM, egress, BYO, agent tokens; orgs table).
- **Organization:**
  - `components/governance/governance-view.tsx` (Model access, Compliance), `byo-panel.tsx`;
  - Credentials LLM tab: `credentials-view.tsx` and `llm-form.tsx`;
  - Skills and libraries pages;
  - Port previews: `/o/[org]/settings/previews`.
- **Workspace:** `settings-view.tsx` (General: BYO posture); `components/agents/model-readiness.tsx`.
- **Not yet built:** the SET-5 sections (agents, approvals, terminal, previews, skills). Until they merge,
  they render read-only current values (C-973, built in P5-IA-5; if not merged, show the section's
  description and the operator doc link only).

## Scope

1. **AI models (C-966).**
   - **Administration** `/admin/ai-models`:
     - **Endpoints** tab: the `llm` section's existing views;
     - **Rules for organizations** tab: the `governance` LLM keys via `SettingsSectionFields keys`, with
       the orgs table as the ladder's below rows.
   - **Organization** `/o/<org>/ai-models`:
     - **Access** tab: was Governance › Model access, as a ladder with the instance floor above;
     - **Keys** tab: org LLM keys, the existing LLM tab content;
     - **Keys to fix** tab: was Compliance.
   - **Workspace** `…/ai-models`: LLM keys (the existing list and form), and above it a one-line readiness
     summary per agent (`model-readiness` data): "Daneel: ready (scn-pong on litellm-mock)".
2. **Agents (C-967).**
   - **Administration** `/admin/agents`:
     - **Defaults** tab: `agents` section;
     - **BYO** tab: the `governance` BYO and agent-token keys, with the orgs table;
     - **Approvals**, **Terminal**, **Previews** and **Skills** tabs: their sections.
   - **Organization** `/o/<org>/agents`:
     - **BYO** tab: was Governance › BYO agents, as a ladder (instance above, workspaces below via
       C-964's `byo`);
     - **Skills** tab: with **Libraries** at `/o/<org>/agents/skills/libraries`;
     - **Previews** tab: was Settings › Previews.
   - **Workspace:** the agents list page gets the workspace BYO posture as a ladder row in its header (org
     above). Remove it from Settings › General.
3. **Retire the old places:** Governance › Model access, BYO and Compliance; Credentials' LLM tab; Settings
   › Previews; the BYO field in workspace Settings. They become redirects (P5-IA-1). Remove their nav and
   tab entries.
4. **Labels and words.** Apply the vocabulary. Field labels come from C-972 once P5-IA-5 merges; until
   then, keep today's labels in these moved forms, and do not invent new ones locally.
5. **Tests.**
   - Vitest: each tab renders the moved content; ladder rows per level; the workspace readiness line.
   - Playwright: an org admin narrows allowed models on AI models › Access, and a workspace's AI models
     page shows the org row above; old URLs land on the right tabs.
   - Axe and narrow-route suites, both themes.

## Constraints / file ownership

- **Owns:**
  - the AI models and Agents pages at three levels;
  - the moved components (composition only);
  - the workspace BYO field's move.
- **No API change.** C-964 (P5-IA-2) supplies the BYO below rows.
- **Order:** after P5-IA-1 and P5-IA-2 (the ladder). Not beside P5-IA-4 (shared `SettingsSectionFields`
  and settings files).

## Acceptance criteria

1. AI models and Agents each have one page per level. Nothing about model access, LLM keys, BYO, previews
   or skills remains in Governance, Credentials or Settings.
2. Inherited values show as ladder rows with their sources.
3. Old URLs land on the right tabs.
4. Web gates; axe and narrow-route suites, both themes.

## Dependencies

P5-IA-1, P5-IA-2. P5-IA-4's `SettingsSectionFields` and P5-IA-5's metadata are soft (see Scope).

## Suggested agent/model and reason

Sonnet (`nextjs-developer`): page composition over existing components across three levels.
