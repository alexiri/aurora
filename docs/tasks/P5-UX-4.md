# P5-UX-4: Give-access defaults, "No agent can use this", effective access in words (owner walkthrough, 2026-11-15)

## Goal

In the walkthrough, a person saved a secret. The "Give agents access" panel then listed the workspace's
only agent **unticked**, with a greyed-out button. They moved on believing the agent could use the secret,
and the agent's Access tab later said "not selected", next to rule ids such as
`selected by 1 rule(s): 01a1e05c-…`. This task makes the obvious choice the default, flags credentials
nobody can use, and explains effective access in words.

## Linked requirements

- D-324; contracts **C-917, C-918**.
- Normative: `14 §First run and connecting` §3, `05` rows for the secrets and connections lists and for
  `GET /v1/agents/{agent}/effective`, `14 §Visual design`.

## Placement in the new structure (D-342)

- The badge and **Give access** appear on the rows of the workspace's and the org's **Apps & services**
  (Overview, Connections and Services tabs) and on **AI models** (LLM keys).
- The agent's **Access** tab hosts the "Not given to <Agent>" group.
- Rows say "app", "service" and "LLM key", never "secret" (vocabulary, `14 §Information architecture`).

## Context

- **Picker.** `web/src/components/credentials/give-access.tsx` `GiveAccessPanel` (~205–341, nothing
  preselected) and `AccessDialog` (~371). Used after saving secrets (`generic-form.tsx`), LLM keys
  (`llm-form.tsx`, which already preselects via `preselect` on the Welcome step, ~78) and connections
  (`connect-dialog.tsx` `ConnectedStep`).
- **Lists.** Workspace and org secrets and connections lists: `services/aurora-api/src/domains/{secrets,
  connections}` DTOs. The rows are `credentials-view.tsx` and `connections-tab.tsx`.
- **Effective view.** `services/aurora-api/src/domains/grants/effective.rs` ~57–66, ~100–104, ~153–157
  (`selected_by` = logical ids of enabled allow rules; grants compile into derived rules,
  `domains/grants/compile.rs`). Rendered raw at `web/src/components/agents/access-tab.tsx:98`.

## Scope

1. **API (C-917).** Add `grant_count` (active agent grants) to the items of `GET /v1/workspaces/{ws}/secrets`,
   `GET /v1/orgs/{org}/secrets`, `GET /v1/workspaces/{ws}/connections` and `GET /v1/orgs/{org}/connections`.
   Compute it with one aggregate join per list query, never per row.
2. **API (C-918).** Effective credential items gain `selected_by_details`. Derived rules of a grant are
   `kind: grant` with label "given on this page". Authored workspace rules are `rule` and org rules
   `org_rule`, both with the rule's name ("unnamed rule" when empty). Keep `selected_by` for
   compatibility. Regenerate OpenAPI and the web types.
3. **Preselection (C-917).** `GiveAccessPanel` and `AccessDialog` take an optional `contextAgentId`.
   - Preselect it. Otherwise, when exactly one agent is grantable by the caller, preselect that agent.
     Otherwise preselect none.
   - The primary button reads **Give <Agent> access** or **Give <n> agents access**. With nothing selected
     it reads **Choose an agent** and is disabled.
   - Apply this to every caller, including the journey's agent step (P5-UX-3 passes `contextAgentId`).
4. **"No agent can use this" (C-917).**
   - Service, LLM-key and connection rows with `grant_count = 0` show a `warning` `Badge` **No agent can use
     this** and a **Give access** button that opens `AccessDialog`.
   - The agent's Access tab gains a collapsed group "Not given to <Agent> (n)" listing the workspace's
     credentials with no grant to this agent, each with **Give access**. LLM keys are listed under the
     model-readiness section's existing "Give access" affordance if there is one; do not duplicate it.
5. **Effective access labels (C-918).** `access-tab.tsx` shows the labels and puts rule ids in a tooltip
   for authored rules only. LLM keys are labelled **LLM key**, never "secret".
6. **Tests.**
   - API ITs: `grant_count` on all four lists (0, 1, 2 grants; a revoked grant not counted);
     `selected_by_details` for a grant, a workspace rule and an org rule.
   - Vitest: the preselection rule (context, sole agent, several agents, none grantable) and the button
     text; the badge; the labels.
   - Playwright: save a service in a one-agent workspace → **Give Daneel access** is enabled and
     preselected → after it, the agent's Access tab lists the service as selected and "given on this
     page".

## Constraints / file ownership

- Owns: the four list DTOs and queries, `domains/grants/effective.rs` (details only),
  `give-access.tsx`, `credentials-view.tsx` and `connections-tab.tsx` (badge only), `access-tab.tsx`,
  `llm-form.tsx` and `generic-form.tsx` (pass-through only).
- Not beside P5-UX-3 (both touch the give-access callers), P5-UX-7 (Apps & services Overview list), or
  P5-UX-11 and P5-UX-14 (both also edit `connections-tab.tsx`): serialize.
  Order: P5-UX-4 → P5-UX-3 → P5-UX-7, or P5-UX-3 first, implementing preselection locally (see P5-UX-3).

## Acceptance criteria

1. In a one-agent workspace, every give-access picker opens with that agent ticked, and from an agent page
   with that agent ticked.
2. A credential no agent holds shows **No agent can use this** on its row; giving access removes the badge
   without a reload.
3. Effective access shows no bare uuid; an LLM key reads "LLM key".
4. `grant_count` adds no N+1 query (asserted by the IT's query count or by `EXPLAIN` in review).
5. `cargo fmt --check`, clippy, targeted ITs; web typecheck, lint, vitest and build; axe and narrow-route
   suites, both themes.

## Dependencies

None. It may land before or after P5-UX-2.

## Suggested agent/model and reason

Sonnet: two small backend additions and several web components. A capable Haiku could do the web part
alone, but the shared picker deserves Sonnet.
