# P5-ADMDEL: instance admins delete organizations and workspaces

## Goal
An instance admin can delete any organization or any workspace from Admin › Organizations, without gaining access to the organization's content (D-234, C-733).

## Linked requirements
- Master spec IDs/sections: D-234; C-733 (`docs/API_CONTRACTS.md` §Phase 5 — P5-ADMDEL); `04 §Instance admins`, `04 §Permission matrix` (new row), `04 §Deletion semantics` "Deletion by an instance admin", `04` audit actions; `05 §Admin` rows for `/v1/admin/orgs…` and `/v1/admin/workspaces/{ws}`; `14 §Instance administration` **Organizations**.
- Requirement summary: list organizations with counts, list one organization's workspaces, delete either with session + recent re-auth + typed slug; same deletion services as the owner routes; critical instance-level audit; no content exposed.
- Reservation: D-234, C-733. No migration expected (counts are queries); if one turns out necessary, ask the coordinator for a number.

## Context
- Owner deletes: `services/aurora-api/src/domains/tenancy/routes.rs` (`delete_org`, `delete_workspace`, `ConfirmRequest`), `…/tenancy/service.rs` (`delete_org`, `WorkspaceDeletion::{prepare,run,committed,rollback}`, `confirmation_mismatch`).
- Admin orgs + freeze: `services/aurora-api/src/domains/freeze/routes.rs` (`GET /v1/admin/orgs`, admin freeze/unfreeze); new admin org routes may live there or in `domains/admin/**`.
- Re-auth: `services/aurora-api/src/domains/me/service.rs` (`REAUTH_WINDOW_SECS`, `reauth_required`), reused by `domains/scim/management.rs`.
- Audit: `crate::audit::{AuditEvent, commit_critical}`; instance-level rows have `org_id` null (FIX-125 instance audit, C-638).
- Placement: `domains/placement/service.rs` (`instance_placement.shared_workspaces` jsonb; the view already skips missing ids).
- Web: `web/src/app/admin/orgs/page.tsx`, `web/src/components/admin/orgs-view.tsx` (`AdminOrgFreeze`), `components/admin/ownerless-banner.tsx`, `lib/api/freeze.ts` (`useAdminOrgs`); account deletion's re-auth step in `web/src/components/account/delete-view.tsx` (412 `reauth_required` handling for `DELETE /v1/me`; `org/directory-view.tsx` reuses it for SCIM) — reuse it; primitives `DataTable`, `Menu`, `FormDialog`, `Badge`, `Notice`.

## Scope
### In scope
**Part A — API (Rust)**
1. `GET /v1/admin/orgs` rows gain `ownerless`, `member_count` (active), `workspace_count`, `agent_count`, `created_at` — computed in the list query (no N+1).
2. `GET /v1/admin/orgs/{org}`; `GET /v1/admin/orgs/{org}/workspaces` (paginated, metadata only).
3. `DELETE /v1/admin/orgs/{org}` and `DELETE /v1/admin/workspaces/{ws}` with `{ confirm }`: IA, session only, re-auth within `REAUTH_WINDOW_SECS` else 412 `reauth_required`, slug check (400 `confirmation_mismatch`), 404 unknown, allowed when frozen; call `service::delete_org` / `WorkspaceDeletion` exactly as the owner routes do (factor a shared function if needed; no copy of the deletion steps).
4. On every delete path (owner and admin, org and workspace) remove the deleted workspace ids from `instance_placement.shared_workspaces` in the same transaction.
5. Audit `admin.org.delete` / `admin.workspace.delete`, security-critical, `org_id` null, target + metadata per C-733. Add both to the audit action catalogue wherever the code keeps one.
6. OpenAPI regenerated: `UPDATE_OPENAPI=1 cargo test -p aurora-api --test it openapi_matches_checked_in`, then `cd web && pnpm gen:api`.

**Part B — Dashboard (Next.js)**, after Part A's schema is regenerated
1. Admin › Organizations as `14` says: table with counts and badges, Actions menu (Freeze/Unfreeze, Workspaces, Delete organization…).
2. `/admin/orgs/[org]`: counts + workspaces table, Delete workspace… per row.
3. Danger `FormDialog` for both deletes: consequences text with the counts, slug typed exactly (submit disabled until it matches), 412 → the account-deletion re-auth step, then one retry; success notice; queries invalidated.
4. Ownerless banner links to the org's row.

### Out of scope
- Any view of organization content for admins; e-mail notifications; a soft delete or undo; a CLI command; bulk deletes.
- Changing the owner routes' permissions or confirmation.

## Constraints / file ownership
- Owns: `services/aurora-api/src/domains/{tenancy,placement}/**` (only the shared deletion function and the placement prune), `services/aurora-api/src/domains/freeze/routes.rs` + DTOs (admin org list) or a new admin-orgs module, new `tests/it/admin_delete.rs`, `web/openapi.json`, `web/src/lib/api/schema.d.ts`, `web/src/app/admin/orgs/**`, `web/src/components/admin/{orgs-view,ownerless-banner}.tsx` + a new `admin/org-detail-view.tsx`, `web/src/lib/api/freeze.ts` (or a new `lib/api/admin-orgs.ts`), tests named below.
- Not concurrent with lanes editing `domains/tenancy/**` or the admin orgs pages (check PROGRESS "Owns" before starting).
- Backend authoritative: the dashboard hides nothing the API would allow and relies on 403/412 from the API. No inline styles; tokens and primitives; both themes; 390 px without horizontal scroll.
- Security: an org `owner` session that is not an instance admin gets 403 on every admin route (IT); an API key never works on them (IT); the re-auth window is the shared constant, not a new one.

## Acceptance criteria
- [ ] Admin list shows counts and ownerless/frozen state; detail and workspaces routes return metadata only (IT asserts the exact field set).
- [ ] Non-admin session → 403; API key → 401/403; unknown ids → 404.
- [ ] Missing/old re-auth → 412 `reauth_required` and nothing deleted; wrong slug → 400 `confirmation_mismatch` and nothing deleted.
- [ ] Admin org delete on an active and on a frozen org: same post-state as the owner delete (an IT that runs both paths on equivalent fixtures and compares remaining rows, deprovision ops enqueued, gateway invalidation sent, managed secret refs discarded).
- [ ] Admin workspace delete: same post-state as the owner path; sandboxes go through runner reconciliation as today.
- [ ] Placement: a deleted workspace disappears from `shared_workspaces` on owner and admin paths (IT).
- [ ] Audit: one `admin.org.delete` / `admin.workspace.delete` row, critical, `org_id` null, metadata per C-733; it appears on the instance Audit page's default scope.
- [ ] Dashboard: Actions menu, organization page, dialogs with typed slug, re-auth retry, success notice; axe clean both themes; no horizontal scroll at 390 px.

## Verification
- Commands: `cargo fmt --check`; `cargo clippy -p aurora-api --all-targets -- -D warnings`; `cargo test -p aurora-api --test it admin_delete tenancy placement permission_matrix openapi_matches_checked_in`; from `web/`: `pnpm typecheck`, `pnpm lint`, `pnpm test` (new `tests/admin-orgs.test.tsx`: menu, dialog slug gating, 412 → re-auth → retry, 400 message), `pnpm build`, Playwright `e2e/stage4-routes.spec.ts` + `e2e/stage4-axe.spec.ts` with `/admin/orgs/<id>` added.
- Expected results: all green; new ITs fail on the code before the change.

## Dependencies
- Blocked by: phase 5 start (owner's word). No other brief.
- Unblocks: nothing.

## Suggested agent/model and reason
Part A `rust-developer` (Sonnet): destructive, permission-sensitive, must reuse the existing transaction and deprovision machinery. Part B `nextjs-developer` (Sonnet). Opus reviews Part A before Part B starts.

## Completion report
- Files changed:
- Tests/checks:
- Remaining risks:
