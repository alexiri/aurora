# P4-MCON-3: Managed connectors — dashboard

## Goal
An instance admin adds, configures and manages connectors from **Admin → Connectors** like any other app (D-128). Spec: `docs/14-web-dashboard.md` **§Admin: connectors (phase 4, P4-MCON)**, `docs/13-channels-connectors.md` §3.1, contracts **C-450..C-454** in `docs/API_CONTRACTS.md`, `18 §Managed connectors` (Web bullet). P4-MCON-1 (merged) provides the API and the generated types (`web/src/lib/api/schema.d.ts`).

## Scope
1. Rework `web/src/components/connectors/connectors-admin-view.tsx` (split into focused components under `web/src/components/connectors/`): the card list with Managed/Self-hosted badges, status chips, integration count and host status; **Add connector** flow (choose type → review → settings → add) as specified; the "Self-hosted connector (advanced)" path reusing today's provision + one-time token dialog; the managed detail view (settings form with Replace/Clear for secrets and Regenerate for generated secrets, Enable/Disable, Delete blocked while in use, host status, `pending_approval` diff + Approve reusing `ManifestDiff`).
2. The settings form renders `instance_setup` fields with the **shared form-field components** the Apps connect window uses (find and reuse them; extend the shared set if a field type is missing, e.g. `generated_secret` as a read-only line). API field errors (`details`) map onto their fields. 409 `stale_manifest` reloads the type and returns to Review.
3. API client functions and React Query hooks in `web/src/lib/api/connectors.ts` (typed from the generated schema).
4. Never display a secret after entry; secret inputs are `type="password"` with `autocomplete="off"`.

## Acceptance
- Vitest (`web/tests/`): add flow (type list incl. no-host empty state and added marker, review summary, settings form with secret and generated fields, field errors, stale digest), detail (replace/clear/regenerate, enable/disable, delete blocked), self-hosted path still shows the token once.
- `cd web && pnpm typecheck && pnpm lint && pnpm test && pnpm build`; Playwright `stage4-routes`, `stage4-axe` (0 serious/critical in both themes), `stage2-narrow` (no horizontal scroll at 390 px), extended to the new screens with mocked API data that uses the real built-in connector names (`Mattermost`) and icons.
- Screenshots of the add flow and detail view in both themes at 1280 and 390 px under `/tmp/aurora-screens/p4-mcon-3/` for the coordinator.
- `bash scripts/guards/run-all.sh` (token and `style=` guards).

## Constraints
- `docs/14 §Visual design`: tokens only, shared primitives and page patterns (`web/src/components/ui`), both themes, no one-off styling.
- Owns `web/src/components/connectors/**`, `web/src/app/admin/connectors/**`, `web/src/lib/api/connectors.ts`, new/updated tests and Playwright specs, shared UI primitives only if a missing one is needed (say which). Don't change API code or contracts (ask through Orca `ask`).
- Don't run `scripts/e2e/*` or `scripts/perf/*`.
