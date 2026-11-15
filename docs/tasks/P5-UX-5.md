# P5-UX-5: Workspace owners add people by e-mail (owner walkthrough, 2026-11-15)

## Goal

The Welcome page promises "Invite a teammate — Share this workspace with people you work with." In the
walkthrough, the personal-workspace owner (an ordinary org member) found no way to do it:

- The link went to the org Members page, which is admin-only, and showed "Could not reach the server".
- Settings › Access had no add control, because its member picker needs the org-admin member list.

This task lets a workspace owner add people by e-mail, without widening who may enter the organization
and without exposing the org's member list.

## Linked requirements

- D-325, D-328 (owner answers); SEC-128; contracts **C-919, C-920** (C-921 not built); the "Invite a
  teammate" part of C-927.
- Normative: `04 §Workspace access by e-mail` (new), `04 §Invitations`, `04 §Permission matrix`
  (workspace access bindings: `can_own`), `03 §Planned changes` (`workspace_pending_access`), `05` rows
  (by-email, pending, the `invite` rate class), `02 §Limits` (`PENDING_ACCESS_TTL`,
  `PENDING_ACCESS_MAX`), `14 §First run and connecting` §5.

## Context

- **Bindings.** `services/aurora-api/src/domains/tenancy/routes.rs`: `put_access` (~631–646) refuses 422
  `not_an_org_member`; members are listed via `require_admin` (~265).
- **Invitations.** `domains/invitations/routes.rs` (~72 `require_admin`), acceptance with a verified
  e-mail match, the seat cap, `/join`.
- **Sign-in.** The placement code runs at first sign-in (`04 §New-user placement`). Find where org
  membership is created or activated at sign-in; the pending rows are applied right after it.
- **Web.** `web/src/components/settings/access-tab.tsx` (`AddBinding` hidden when `useOrgMembers` 403s,
  ~51–52), `settings-view.tsx` (Access tab `ownerOnly`), `components/org/invite-dialog.tsx`.

## Scope

1. **Migration.** `workspace_pending_access` per `03 §Planned changes`.
   - Take the next free migration number from PROGRESS's next-free line when you start, and edit that
     line.
   - Move the planned rows into `03`'s machine-checked tables in the same change.
   - Regenerate `crates/aurora-db/tests/fixtures/spec_schema.json`.
2. **Limits.** Add `PENDING_ACCESS_TTL` and `PENDING_ACCESS_MAX` in `crates/aurora-core/src/limits.rs`,
   and the limits guard entries.
3. **`POST /v1/workspaces/{ws}/access/by-email` (C-919).**
   - `can_own`, rate class `invite`, plus a per-workspace cap of 30 per hour in the counter store.
   - Normalize the e-mail like invitations.
   - **Active org member with that verified e-mail:** upsert `workspace_access` with the role.
   - **Otherwise:** a pending row. 422 `owner_requires_member` for `owner`; 422 `pending_access_limit` over
     the maximum.
   - Compute `org_entry` (`open_signup` or `invitation_needed`). Never create an org invitation or
     membership (D-328).
   - Audit `workspace.access.grant` or `workspace.access.pending_create`.
4. **Pending routes (C-920).** `GET …/access/pending` (Page) and `DELETE …/access/pending/{id}`, both
   `can_own`; audited delete.
5. **Apply at sign-in (C-920, SEC-128).** After the sign-in has made or confirmed the user's active
   membership in an org, apply that org's pending rows whose `email_normalized` equals one of the user's
   **verified** e-mails:
   - upsert `workspace_access` (never downgrading an existing higher role), delete the row, audit
     `workspace.access.pending_apply`;
   - run in the sign-in transaction, or right after it, idempotently;
   - never create an org membership.

   Add a sweeper for expired rows on the existing periodic-task mechanism.
6. **No org setting** (C-921 is not built, D-328).
7. **Web.**
   - Settings › Access › **Add people** dialog: e-mail, role (Member, Approver or Owner; the server
     refuses Owner for a non-member with 422 `owner_requires_member`, shown in words) and the result
     sentence:
     - "<email> now has access as <role>."
     - "<email> will have access as <role> after they first sign in."
     - plus the `org_entry` sentence per `14` §5, with org admin names from `useOwnerNames`' org
       equivalent.
   - The "Waiting for first sign-in" list with **Remove**.
   - Welcome "Invite a teammate" → this dialog (`?tab=access&add=1`), shown when `canOwn` (C-927).
8. **Tests.** API ITs per SEC-128's "Proven by":
   - added vs pending; rate limits; owner for a non-member → 422; the limit;
   - sign-in with verified/unverified e-mail; a non-member at sign-in → nothing applied and no membership;
   - expiry; the audit rows; cross-workspace isolation of pending rows;
   - no add ever creates an org invitation or membership (D-328).

   `permission_matrix` rows. Vitest for the dialog states. Playwright: a member who owns a personal
   workspace adds a second dev-IdP user by e-mail before that user's first sign-in; after it, the second
   user sees the workspace.

## Constraints / file ownership

- Owns: the migration, `domains/tenancy` (access routes), the sign-in hook point, the sweeper,
  `limits.rs`, `access-tab.tsx`, `settings-view.tsx`, `getting-started.tsx` (the invite link only, after P5-UX-1).
- Shared files appended per PROGRESS's rule: `API_CONTRACTS.md`, the OpenAPI document,
  `spec_schema.json`, `permission_matrix.rs`, `security-coverage.md`.

## Acceptance criteria

1. The walkthrough case works: Bob (member, owner of his personal workspace) adds `carol@…` before Carol
   ever signs in; at her first sign-in (open sign-up into the same org) she has access as Member.
2. No route lets a workspace owner who is not an org admin list or search org members (permission matrix
   plus a negative test).
3. Each SEC-128 case passes; `docs/tests/security-coverage.md` row added.
4. Welcome's "Invite a teammate" opens Add people for a workspace owner and is absent for a member.
5. Rust gates (fmt, clippy, targeted ITs, migration check, limits guard); web gates; axe and narrow-route
   suites, both themes.

## Owner answers (D-328, 2026-11-15)

- **Q-UX-1** (may a workspace owner let a non-member into the organization?): **no**, and there is no
  org setting to allow it. C-921 is not built.
- **Q-UX-2** (may the answer tell the workspace owner whether one exact address belongs to an active
  member?): **yes**, one address per call, rate-limited and audited.

## Dependencies

P5-UX-1 (it hides the Welcome link this task brings back). No other.

## Suggested agent/model and reason

Sonnet (`rust-developer` then `nextjs-developer`, or one Sonnet lane in that order). It touches a
migration, the sign-in path and a security invariant, so it needs careful review against SEC-128.
