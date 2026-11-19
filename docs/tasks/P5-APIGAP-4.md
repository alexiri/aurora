# P5-APIGAP-4: Workspace People: when someone joined, and suspend one person in one workspace

## Goal
Owner, 2026-11-19 (D-399). The workspace People page (P5-POLISH-7, D6) was designed with a **Joined** date and a
**Suspend** action per person, but `AccessBinding` has no timestamp and suspension exists only organization-wide
(`PATCH /v1/orgs/{org}/members/{user}` `status`), confirmed by P5-POLISH-11 pass 2. Security-relevant (SEC-138):
this adds a new way to lose access, so every authorisation path must honour it.

## Change
1. **Joined (C-1050):** `AccessBinding` gains `joined_at` (when the binding was created; for an implicit/derived
   member, when the source binding was, or null and say why). Read from the existing row's creation time if it has
   one; otherwise add it in the migration with a backfill that is honest (null for rows of unknown age, not "now").
2. **Suspend (C-1051, SEC-138):** a workspace binding gets `status: active | suspended`;
   `PATCH /v1/workspaces/{ws}/access/{user}` (or the binding route that exists) sets it. Mirror the organization's
   suspension semantics scoped to one workspace: read `PATCH /v1/orgs/{org}/members/{user}` and do the same for
   this workspace only — what it ends (sessions, tokens, running turns, terminal/SSH/Files access, approvals the
   person would decide) and what it keeps (the binding, role, their data). A suspended person is not a member of
   that workspace for **every** authorisation check: `WorkspaceAuth` and every view that filters on
   `status = 'active'` (FIX-181's audit of those views is the list to start from). Who may: workspace owners;
   never yourself, never the last active owner; an implicit member (org owner/admin) cannot be suspended here (say
   so, 409). Restore sets `active`. Audit both. Migration `20261201000258`.
3. **Web:** People rows: "Joined <date>" and, in the row menu, **Suspend** / **Restore** with a confirmation that
   says what stops; a suspended row is marked with the neutral pill "Suspended". Replace the P5-POLISH-7
   placeholders. Regenerate the API types.
4. Docs: `05`, `API_CONTRACTS` C-1050/C-1051, `03` (column, without `created_at`/`updated_at`), `07`/`08` where
   membership and suspension are described, `SECURITY` SEC-138 (the invariant: a suspended binding grants nothing).

## Tests
API ITs: `joined_at` present; suspend → the person gets 403/404 on workspace routes from each family FIX-181
tested (agents, chat/turns, files, terminal/SSH token redemption, approvals, connections), their open session for
that workspace ends, other workspaces of the same org unaffected; restore → access back; self, last owner and
implicit member refused; audit rows. A guard or test that fails if a new workspace-membership query ignores
`status`. Vitest for the row and the dialog. Targeted only (CI is the gate); `extract-schema --check`, migration
guards. The Completion report states why no path still grants a suspended member access — security review by the
Orchestrator before merge.

## Ownership
Workspace access/binding domain and `WorkspaceAuth` in `services/aurora-api`, the migration, web People page.
Size M. D-399, C-1050, C-1051, SEC-138, migration `000258`.
