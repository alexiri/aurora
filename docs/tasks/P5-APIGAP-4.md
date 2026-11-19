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

## Completion report

Lane `p5-polish-b` (master `18212291` merged first). Contract first: `05` (the access rows + the new PATCH row) and `API_CONTRACTS` C-1050/C-1051, then code.

**Design.** Suspension is a column on the direct binding, `workspace_access.status` (`active | suspended`, migration `20261201000258_workspace_access_status.sql`), and it is enforced in **one place**: the `effective_workspace_access` view (`CREATE OR REPLACE`, same columns, so every grant and dependent query is kept). The view drops a suspended direct row **and** the person's group-derived rows for that workspace (otherwise a group binding would bypass the suspension). An effective org owner/admin keeps the implicit owner row: the org role outranks a workspace owner. Such a person cannot be suspended here (409 `implicit_access`). A suspension recorded before someone became org admin is dormant while they are admin and applies again if they stop being one (C-1051 says so). `joined_at` (C-1050) is the existing `workspace_access.created_at`, so there is no new column and no backfill. It is `null` for an implicit row without a direct row, because the org role's start is not recorded. Group-derived rows get the earliest start of a current source (the later of membership and binding).

**Route.** `PATCH /v1/workspaces/{ws}/access/{user} { status }` (`can_own`) → `AccessBinding`. Refusals:
- 404 when there is no direct binding (group-only access is ended at the group binding).
- 409 `cannot_suspend_self`.
- 409 `implicit_access`.
- 409 `managed_by_idp` (suspend only; restore is always allowed).
- 409 `last_owner`. This is defensive and cannot trigger through the API: the caller is an active owner other than the target, and org owners are always implicit owners.

Sending the current status again is a 200 no-op with no audit row. One transaction holds:
- the row lock and the refusals;
- the status;
- on suspend, the person's queued and live **speaker turns in this workspace** end `aborted`/`user_suspended` with a notice (SCIM's `abort_turns`, generalised to `abort_speaker_turns(org, Some(ws), …)`; deltas flushed after commit);
- `notify_member_changed` (gateway `user` scope + `aurora_auth_revoked {kind: access}`);
- `notify_auth_revoked user`;
- the critical audit row `workspace_access.suspend|restore` with `metadata.change.status`.

`PUT` and the sign-in syncs change the role and never the status.

**What it ends / keeps** (the org PATCH scoped to one workspace). Org `PATCH members/{user}` with `suspended` changes status, drops IdP group memberships org-wide, reconciles the SCIM link, can reassign automations and sends `notify_auth_revoked user`. Everything else stops because every check treats the person as a non-member. The workspace suspend does the same for one workspace:
- **Ends:** every check refuses the person in this workspace. Their open streams re-authorize and end. Terminal, SSH, Files and preview sessions end at their next re-check. Approval decisions are refused, and they are no longer approval mail/push recipients. Schedules and webhooks with `anchor = creator` stop firing, as for a lost `can_use`.
- **Keeps:** the binding and role, sessions (other workspaces and orgs are unaffected), API keys (refused, not revoked), and their data.
- **Not mirrored:** IdP group drops and the SCIM link are org-wide matters. `reassign_to` was not asked for.
- **Goes further than the org PATCH:** it also aborts the person's speaker turns in the workspace. The brief lists running turns among what a suspension ends; the human org PATCH does not abort turns, only SCIM deprovisioning does. That gap in the org route is noted for the owner, not changed here.

**Every query/extractor that decides workspace membership, and how it honours suspension**

Through the view (fixed once, in the migration):
- `aurora_db::authz::effective_access` → `workspace_access`, `can_use`, `can_own`, `can_approve`, `can_ssh`, `automation_authorized`, `workspace_has_active_owner`, `owns_any_workspace`, `uses_any_workspace`, `can_see_conversation_id`, `hidden_direct_conversation_sql`.
- The extractor `Caller::workspace` (`http/auth.rs`), hence `WorkspaceAuth` for every workspace route and every workspace API key.
- All API callers of the above:
  - terminal `admit` + periodic `Recheck`;
  - SSH mint/open/channel/heartbeat;
  - Files;
  - relay/preview `resolve`/`authorize`/`validate`;
  - channels (speakers, clicks, commands, links, messaging, inbox, reach, presences);
  - approvals and action approvals (decide, long-poll, stream with access pings);
  - conversations (stream re-authorizes on access pings), search;
  - schedules and webhooks fire-time;
  - peer tasks, cli login, automations, attachments, avatars, grants, policy simulate, evals, skills, accounts, BYO, agent links.
- Repo queries: `workspaces::list_visible`, `workspaces::effective_roles_of`, `approval_notify` (lines 17/35), `contacts`, `channel_presences`, `search`, `groups::derived_members` (rows come from the view).

Raw readers, each reviewed (the guard `aurora-db it/membership_queries.rs` lists them with counts and fails on any new one):
- `repo::gateway::can_use`: the gateway role cannot read the views, so the law is restated with the suspension; tested.
- `approval_notify::deciders_with`: `wa.status = 'active'`, and suspended people are excluded from the group branch; tested.
- `groups::via_groups` (offboarding explanation): suspended excluded; tested.
- `groups::impact`: anyone with a direct row is never counted as losing access, which is correct because a suspended person has none to lose.
- `authz::effective_access`: the raw joins only explain the sources; the role comes from the view.
- Not access decisions:
  - `workspaces::{list_access, get_access}` (listing returns `status`);
  - `bind_user`/`unbind_user`/`lock_binding`/`set_binding_status`;
  - `roles_of` (placement's "already bound");
  - `idp_roles` (writes roles, never status; its claim removal deletes only claim-created rows, which cannot be suspended);
  - `members::remove`;
  - `account::sole_member_personal_workspaces` (a workspace someone suspended is bound to is kept);
  - `scim` group count;
  - `grants.rs` (gateway SELECT grants).

So nothing grants a suspended person access: every decision reads the view or the restated gateway law, and the guard keeps it that way.

**Web.** People rows have a Joined column (`formatWhen`, exact time in the tooltip; a line under the name below `sm`). A suspended row has a neutral "Suspended" pill (not on implicit rows). The row menu offers **Suspend…** (a confirmation that says what stops and what is kept) or **Restore access**. The last-owner count ignores suspended owners. API types regenerated (`web/openapi.json` + `schema.d.ts`; only this change). No Playwright spec depended on the table's columns.

**Docs.** `03` (column), `04` (access law + audit list), `05`, `06` (gateway speaker law), `07` (deciders), `17` SEC-138, `API_CONTRACTS` C-1050/C-1051, `docs/tests/security-coverage.md`, FEATURES row `P5/04§workspace-suspend`, PROGRESS. `08` has no membership or suspension text to amend.

**Files.**
- Migration `000258`.
- `crates/aurora-db`: `repo/{workspaces,gateway,approval_notify,groups}.rs`, `tests/it/membership_queries.rs`, `tests/fixtures/spec_schema.json`.
- `crates/aurora-core/src/audit.rs`: two critical actions.
- `services/aurora-api`: `tenancy/{dto,routes,service}.rs`, `groups/dto.rs`, `scim/deprovision.rs`, `tests/it/{workspace_suspend,permission_matrix,main}.rs`.
- `web`: `settings/access-tab.tsx`, `lib/api/account.ts`, `openapi.json`, `lib/api/schema.d.ts`, `tests/people-table.test.tsx`.

**Tests run** (targeted; DB = a throwaway `postgres:16` podman container on tmpfs, never aurora-dev-pg; every cargo/pnpm run through `heavy.sh`):
- `aurora-api it workspace_suspend` 6/6:
  - joined_at;
  - every family + key + law + gateway law + deciders + other workspace + restore + audit;
  - group access;
  - refusals;
  - turns;
  - stream.
- With `workspace_access`, `openapi_matches_checked_in`, `audit_atomic`, `scim_deprovision`, `sse_revocation`, `groups::`, `permission_matrix` (incl. the new PATCH row): 80 run in the first pass. Two failures were test-side (the matrix row, an owner-only route in the list) and are fixed. Re-run: `workspace_suspend` + `permission_matrix` + `openapi_matches_checked_in` 11/12, then `workspace_suspend` 6/6 after the last test fix.
- `aurora-db it` `membership_queries` (guard + scanner self-test), `authz_groups`, `schema_matches_spec`, `migration_order`: 20 passed.
- `aurora-core` audit: 2 passed.
- `cargo fmt --all --check`: clean.
- `cargo clippy -p aurora-core -p aurora-db -p aurora-api --all-targets -- -D warnings`: clean.
- `extract-schema.py --check`: ok.
- `scripts/guards/run-all.sh`: all passed.
- `pnpm typecheck`: 0. eslint on the touched files: 0.
- vitest `tests/people-table.test.tsx`: 6/6, including Joined, the Suspend dialog and PATCH, Restore, and the implicit row.

Not run (CI is the gate): full suites, Playwright, the gateway crate's own tests (the gateway calls the changed `repo::gateway::can_use`; it is covered here from the API IT).

**Not done / for the owner.** The org-wide human suspend does not abort the person's running turns, while SCIM's and now the workspace one do. Whether the org PATCH should is an owner question; it is unchanged here. Removing a suspended person's binding (DELETE) ends the suspension; any access their groups give then applies again, as it would for an active person.
