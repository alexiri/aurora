# P5-UX-5: Workspace owners add people by e-mail (owner, 2026-11-15)

## Goal

The Welcome page promised "Invite a teammate". In the first walkthrough, the owner of a personal
workspace (an ordinary member of the organization) found no way to do it: the link led to a page only
org admins can open, and the workspace's people page had no add control, because its picker needs the
organization's member list.

This task lets a workspace owner add people by e-mail, without widening who may enter the organization
and without exposing the organization's member list.

## Linked requirements

- **Design** (D-354): the canvas “Aurora — agents, accounts and access”, https://claude.ai/artifact/3UwmUgrhh6MLCsfqdhTFmQ, boards Workspace › People › Add people; States › Empty states. Read each board's note; where a board and the spec disagree, the spec wins.
- D-325, D-328 (owner: a workspace owner can never let a non-member into the organization; the dialog may
  say whether one exact address is already a member), D-352 (Approver only where already used).
- SEC-128; contracts **C-919, C-920**.
- Normative: `04 §Workspace access by e-mail`, `04 §Invitations`, `04 §Permission matrix`,
  `03 §Planned changes` (`workspace_pending_access`), `05` rows (by-email, pending, the `invite` rate
  class), `02 §Limits` (`PENDING_ACCESS_TTL`, `PENDING_ACCESS_MAX`), `14 §Agents, accounts and access`
  §10.

## Wave and order

- **Wave 3.** Starts after P5-IA-4 (the workspace's People page), P5-UX-1 (`WhoCan`) and P5-UX-7
  (Welcome's links, to which this task adds one).

## Context

- **Access rows.** `services/aurora-api/src/domains/tenancy/routes.rs`: `put_access` (~631–646) refuses
  422 `not_an_org_member`; the organization's members are listed only for admins (~265).
- **Invitations.** `domains/invitations/routes.rs` (~72, admins only): acceptance needs a verified e-mail
  match; a seat cap applies.
- **Sign-in.** Placement runs at first sign-in (`04 §New-user placement`). Find where the organization
  membership is created or activated; pending rows are applied right after it.
- **Web.** `web/src/components/settings/access-tab.tsx`, hosted by the People page since P5-IA-4
  (`AddBinding` is hidden when the member list is refused, ~51–52).

## Scope

1. **Migration.** `workspace_pending_access` per `03 §Planned changes`.
   - Take the next free migration number from PROGRESS when you start, and edit that line.
   - Move the planned rows into `03`'s tables; regenerate `spec_schema.json`.
2. **Limits.** `PENDING_ACCESS_TTL` and `PENDING_ACCESS_MAX` in `crates/aurora-core/src/limits.rs`, with
   the limits guard entries.
3. **`POST /v1/workspaces/{ws}/access/by-email` (C-919).**
   - `can_own`; rate class `invite`, plus 30 per hour per workspace in the counter store.
   - Normalize the e-mail like invitations.
   - **An active member of the organization with that verified e-mail:** upsert the access row.
   - **Anyone else:** a pending row; 422 `owner_requires_member` for the owner role; 422
     `pending_access_limit` over the maximum.
   - Answer `{ result, org_entry }`. Never create an invitation or a membership (D-328).
   - Audit `workspace.access.grant` or `workspace.access.pending_create`.
4. **Pending routes (C-920).** `GET …/access/pending` and `DELETE …/access/pending/{id}`, both `can_own`;
   the delete is audited.
5. **Apply at sign-in (C-920, SEC-128).** After a sign-in has made or confirmed the person's active
   membership of an organization, apply that organization's pending rows whose e-mail equals one of the
   person's **verified** e-mails: upsert the access row (never lowering a higher role), delete the pending
   row, audit `workspace.access.pending_apply`. Idempotent. Add a sweeper for expired rows on the
   existing periodic-task mechanism.
6. **Dashboard.**
   - The workspace's **People** page gets **Add people**: an e-mail and a role.
     - Roles offered: **Member** and **Owner**. **Approver** is offered only when the workspace already
       has an approver.
     - The server refuses Owner for someone outside the organization; say it in words.
     - The result is one sentence: "<name> can now use this workspace." or "<email> will have access as
       <role> after they first sign in."
     - Aurora sends no e-mail for this. Both results end with "Aurora does not e-mail them. Send them
       this link:" and the workspace's address in a `CopyValue`.
     - When the person cannot sign up by themselves (`org_entry = invitation_needed`), add `WhoCan`:
       "They also need to be invited to <org>. Ask Ada Lovelace or Alan Turing (organization owners and
       admins)."
     - At the maximum (`pending_access_limit`): "50 people are already waiting. Remove one first."
   - A list "Waiting for first sign-in": rows "<e-mail> · <role> · expires in <n> days" with **Remove**.
     The list is not shown when it is empty.
   - Welcome's links gain **Add people**, for people who may (`…/people?add=1`).
7. **Tests.**
   - API (SEC-128's list): added and pending; the rate limits; no prefix or wildcard match; owner for a
     non-member → 422; the maximum; sign-in with a verified and with an unverified e-mail; a matching
     person who is not in the organization → nothing applied and no membership created; expiry; the
     audit rows; one workspace's owner cannot read another's pending rows; no add creates an invitation.
   - `permission_matrix` rows; a `security-coverage.md` row.
   - Vitest: the dialog's states; the roles offered with and without an existing approver.
   - Playwright: a member who owns a personal workspace adds a second dev-IdP user by e-mail before that
     user has ever signed in; after their first sign-in, the second user sees the workspace.

## Constraints / file ownership

- Owns: the migration; `domains/tenancy` (the new routes); the sign-in hook point; the sweeper;
  `limits.rs`; the People page's add dialog and waiting list (`access-tab.tsx`); Welcome's **Add people**
  link.
- Shared files appended per PROGRESS's rule (API_CONTRACTS, OpenAPI, `spec_schema.json`,
  `permission_matrix.rs`, `security-coverage.md`).

## Acceptance criteria

1. The walkthrough case works: Bob, an ordinary member who owns his personal workspace, adds `carol@…`
   before Carol ever signs in; at her first sign-in she has access as Member.
2. No route lets a workspace owner who is not an org admin list or search the organization's people.
3. Each SEC-128 case passes.
4. Welcome's **Add people** opens the dialog for a workspace owner and is absent for a member.
5. Rust gates (fmt, clippy, targeted tests, migration check, limits guard); web gates; axe and
   narrow-route suites, both themes.

## Suggested agent/model and reason

Sonnet (`rust-developer` then `nextjs-developer`, or one lane in that order). A migration, the sign-in
path and a security invariant: it needs careful review against SEC-128.
