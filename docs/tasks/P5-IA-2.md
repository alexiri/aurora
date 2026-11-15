# P5-IA-2: The scope ladder, and Internet access as one topic at every level (owner, 2026-11-15)

## Goal

"What may agents reach on the internet" lived on four pages under three names:
- Administration › Settings › Governance, for the floors;
- org Governance › Egress, for posture, allow-list and browser tunnels;
- org Policy, for rules and the default rule;
- workspace Policy, for the same controls again.

No page showed what the level above set or what the levels below changed.

This task builds the **scope ladder** primitive (above / here / below) and the **Internet access** page at
Administration, organization and workspace level (D-338, D-339).

## Linked requirements

- D-338, D-339; contracts **C-963, C-964, C-965**.
- Hosts the work of other briefs, which build into this page:
  - C-926: the summary, Common changes and labels (P5-UX-7);
  - C-935: "From agents' access" (P5-UX-10);
  - C-930: Test a request (FIX-159).
- Normative: `14 §Information architecture` (scope ladder, Internet access row), `07 §Evaluation` and
  `§Authoring` (semantics unchanged), `08 §11.6` (postures).

## Context

- **Org Governance › Egress:** `web/src/components/governance/governance-view.tsx` (tabs), with the egress
  panel inside it.
- **Org Policy:** `components/policy/org-policy.tsx` and `policy-view.tsx` (default rule ~312–315,
  uncredentialed ~334–343, tunnels ~348, apply/discard ~427–433, empty state ~240).
- **Workspace Policy:** `workspace-policy.tsx`.
- **Test a request:** `simulate-panel.tsx`.
- **Instance floors:** `components/admin/settings/governance-settings-view.tsx`, with "Where organizations
  stand" from `GET /v1/admin/settings/governance/orgs` (C-778).
- **Org governance API:** `GET /v1/orgs/{org}/governance/effective` (any member; values marked
  `source: instance|org`).
- **Save paths:** the governance `PUT` and the policy draft and apply routes. Both stay.

## Scope

1. **`ScopeLadder` primitive (C-963)** in the shared UI set:
   - **Above** rows: label, the value in words, the source chip ("Set by Administration" or "Set by
     <org>"), and a link for viewers who may change it.
   - **Here** slot, with an optional `minimumNote` ("Looser options are not available: Administration
     requires …").
   - **Below** table (columns and rows with links).

   Tokens and shared primitives only. At 390 px, rows stack and the table collapses to the stacked-row
   pattern. Add a story or demo route the way the shared set does.
2. **API (C-964).** `GET /v1/orgs/{org}/workspace-overrides` (org owner/admin), per C-964, from the
   workspace policy defaults and the BYO setting. Only differing workspaces unless `?all=true`; paginated.
   OpenAPI, the generated types, a `permission_matrix` row, ITs (difference detection, pagination,
   refusal for members).
3. **Organization page** `/o/<org>/internet-access` (C-965), in this order:
   1. P5-UX-7's summary and Common changes, if merged; otherwise leave a slot.
   2. **Posture, allow-list and browser connections:** the content of Governance › Egress, as a ladder
      with the instance floor above and the governance save.
   3. **Rules and default rule:** the org Policy editor with draft and apply. Use the P5-UX-7 labels if
      merged; otherwise apply them here, from C-926's rename list.
   4. **From agents' access**, P5-UX-10's group, if merged.
   5. **Test a request**: the simulate panel.
   6. **Workspaces:** the C-964 table.

   Members see the page read-only (C-961).
4. **Workspace page** `…/internet-access`, in the same order, with the workspace's controls:
   - requests without a credential, browser connections, allow-list, rules;
   - the org's values as the ladder's above rows.
5. **Administration page** `/admin/internet-access`: the `governance` section's egress and open-lane keys,
   rendered through C-971's `SettingsSectionFields` with `keys`, plus "Where organizations stand" as the
   ladder's below table. If P5-IA-4's `SettingsSectionFields` hasn't merged, embed the existing governance
   settings form filtered to those fields, and note it.
6. **Retire the old places.** Governance › Egress and the Policy pages are now redirects (P5-IA-1). Remove
   their nav and tab entries; their components are reused here.
7. **Tests.**
   - Vitest: `ScopeLadder` rendering (above, here and below; read-only; minimum note); the page order; the
     member read-only view.
   - API ITs for C-964.
   - Playwright: an org admin tightens the allow-list on the org page and sees it as an above row on a
     workspace's page; a workspace that tightened appears in the org's Workspaces table.
   - Axe and narrow-route suites for the three pages.

## Constraints / file ownership

- **Owns:**
  - the `ScopeLadder` primitive;
  - the three Internet access pages;
  - the egress panel and policy components (composition and labels);
  - the C-964 route.
- **Shared page:** P5-UX-7 and P5-UX-10 add sections to it. Whichever runs later appends its section into
  the order above. Run sequentially, never in parallel.
- No policy-semantics or API change other than C-964.

## Acceptance criteria

1. Internet access is one page at each level; nothing about egress or rules remains on another page.
2. Each page shows the level above as locked rows with sources and links, and an org page shows its
   tightened workspaces.
3. The old Governance › Egress and Policy URLs land on the new page (with P5-IA-1's redirects).
4. Rust and web gates; axe and narrow-route suites, both themes.

## Dependencies

P5-IA-1 merged. FIX-159 (Test a request agrees with the gateway) is independent.

## Suggested agent/model and reason

Sonnet: one shared primitive, a small API route and three page compositions over existing components.
