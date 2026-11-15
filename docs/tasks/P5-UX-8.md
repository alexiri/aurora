# P5-UX-8: App setup status, backend (owner app-setup walkthrough, 2026-11-15)

## Goal

In the app-setup walkthrough, a team lead and an org owner had no place that answered three questions
about an app:
- Can people here use this app?
- If not, what is missing?
- Who can fix it?

The answers were spread over three OAuth client lists, Self-hosted apps, Governance › Apps and Org ›
Settings, and parts of them were shown nowhere (the AWS external id). This task adds one read-only route
with that answer. It feeds the setup page (P5-UX-9), and the connect plan (C-955, P5-UX-2) becomes a
projection of it.

## Linked requirements

- D-330; contract **C-931**; reuses C-955 `Unlock` and readiness rules; `09 §11` (OAuth client resolution),
  D-74 (instances), C-20 (availability), I-27 (org connection read access).
- Normative: `14 §App setup` §7, `05` (setup row).

## Placement in the new structure (D-342)

The setup status feeds the per-app setup pages inside each level's **Apps & services** topic (P5-UX-9).
Its `Unlock.admin_path` values and every other dashboard path use P5-UX-2's server-side path table, which
mirrors C-960.

## Context

- OAuth client resolution: `crates/aurora-db/src/repo/connections.rs` ~366 (workspace → org → instance,
  enabled only, `instance_id IS NOT DISTINCT FROM`). Lists:
  - workspace (`can_own`): `repo/oauth_clients.rs` ~80, `oauth_clients/routes.rs` ~39–45;
  - org (owner/admin);
  - instance (`/v1/admin/oauth-clients`).
- `AppDetail.oauth_clients[]` holds the manifest client definitions with `redirect_uri` built from
  `config.external_url` (`domains/apps/service.rs` ~258–295).
- The external id is `organizations.external_id`, readable by any org member through `GET /v1/orgs/{org}`
  (`tenancy/routes.rs` ~156–161). An app needs it when its `permissions.context` includes
  `org_external_id`.
- Instances: `/o/<org>/instances` and `/admin/instances` (FIX-132 "Self-hosted apps").
- **One function, one direction (C-931).** `setup_status` is the source and the connect plan (C-955) is
  a projection of it.
  - If P5-UX-2 merged first, move its plan logic into `setup_status` and turn the plan route into a
    projection, with its tests unchanged.
  - If this task runs first, write `setup_status` and let P5-UX-2 project from it.

## Scope

1. **One function** in `services/aurora-api/src/domains/apps/` (`setup.rs`): `setup_status(ctx, app,
   view)`, where `view` is `Workspace(ws)` or `Org(org)`. It computes C-931 from the snapshot, posture,
   clients by level (refs only, never secrets or client ids), instances, connections by scope, values
   (`callback_url` always; `external_id` when the app uses `org_external_id`) and `can` (the caller's
   abilities at each level). `Unlock` follows C-955, never naming instance admins.
2. **Route** `GET /v1/apps/{app}/setup?workspace_id=|org_id=`:
   - `workspace_id` → `can_own`, the workspace view;
   - `org_id` alone → org owner/admin, the org view;
   - neither, or both → 400.

   Add it to OpenAPI, regenerate the web types, and add `permission_matrix` rows.
3. **Connect plan.** If P5-UX-2's plan exists, re-implement it as a projection of `setup_status`, with its
   tests unchanged and green.
4. **Tests** (`tests/it/app_setup.rs`):
   - GitHub, no clients: methods `oauth`/`oauth_app` need `oauth_client`, `github_app` is
     `admin_material`, `token` is ready.
   - A workspace client for Linear makes `oauth` ready in that workspace only; the org view has
     `clients.workspace = null` and still reports `oauth` as needing a client.
   - An org client shows in both views as refs.
   - An instance client gives `instance.set_up = true` with no id.
   - `aws-keys` returns `values.external_id` equal to the org's; `linear` does not.
   - GitLab without an instance shows the instance-bound methods needing `instance`.
   - Restricted posture without the app gives `available: false` with `enable_app`.
   - Permission matrix: a member who is not `can_own` → 403 with `required_role` (if P5-UX-2's C-958
     helper has merged); a workspace owner asking for the org view → 403.
   - No client secret, client id or e-mail appears in any answer (asserted by a scan of the JSON).

## Constraints / file ownership

- Owns: `domains/apps/setup.rs` (new), the route registration, the OpenAPI document and the generated
  web types, `permission_matrix.rs` rows. It touches `domains/apps/plan` only if P5-UX-2 merged first.
- No dashboard work (P5-UX-9).
- Rust gates: fmt, clippy, targeted ITs.

## Acceptance criteria

1. The C-931 answer for the walkthrough's apps (GitHub, Linear, AWS, PostgreSQL, GitLab, Notion) matches
   the test table.
2. The connect plan and the setup status agree for every method (one function; a test compares them).
3. Nothing secret or personal beyond C-955's names is returned.
4. Gates green; counts reported.

## Dependencies

None hard. Coordinate with P5-UX-2 (same function); run in the same lane, after it if both are queued.

## Suggested agent/model and reason

Sonnet (`rust-developer`). A read-only aggregation, but it touches several domains and must not leak
client material.
