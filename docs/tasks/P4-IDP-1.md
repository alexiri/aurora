# P4-IDP-1: Roles from IdP claims (owner, 2026-11-09)

## Why
On the first production sign-in the owner became owner of a personal org, and nothing in the OIDC setup could say who is an instance admin or an owner. The IdP (CERN SSO) already assigns application roles to e-groups and puts them in the token (`cern_roles`). Aurora must map those **role values** to Aurora roles. Group → role mapping stays in the IdP.

## Spec (source of truth; read, don't re-decide)
- `docs/04-identity-tenancy.md` **§OIDC role claims**, plus the instance-admin bullet in §Instance admins and the D-139 note in §Groups.
- `docs/15-deployment-operations.md`: the `auth.oidc_providers[]` row (new keys).
- `docs/05-control-plane-api.md`: `GET /v1/admin/identity`, the `GET/PATCH /v1/admin/users` additions.
- `docs/14-web-dashboard.md`: Account › "Roles from your sign-in", Admin › Identity.
- `docs/17-security.md` **SEC-78** and the first residual risk; D-139.

## What it looks like
- **Operator**, in the Helm values (`config.auth.oidc_providers[]`), sets a claim path and a table of mappings:
  `role_claim: cern_roles`, then `role_mappings: [{ value: aurora-admins, instance_admin: true }, { value: aurora-users, org: { slug: users, role: member } }, …]`.
  Startup refuses bad mappings and names the key.
- **A user signs in.** Their roles match what the IdP says right now. New values add roles; values that disappeared remove the roles those values gave. Roles that a person gave by hand stay.
- **In the dashboard**, a role from the IdP shows "From IdP role `aurora-admins`" and cannot be edited there.
  - The account page lists each claim value and what it granted.
  - Admin › Users shows the last synced values per user.
  - Admin › Identity shows the mapping table read-only.
- With `require_role: true`, someone the IdP gave no Aurora role cannot get in (`no_mapped_role`).

## Basic shape (implementer decides details)
- **Source tracking** on org memberships, workspace bindings and the instance-admin flag (e.g. a `source` / `managed_by` column, or a side table of claim-sourced assignments), recorded in `03 §Planned changes`.
- **One sync function** run in the sign-in transaction after token validation (flow step 4, beside JIT). Precedence: invitation → claims → JIT → placement (P4-ONB-1).
- Creating missing orgs and workspaces reuses the existing org/workspace creation transaction (default workspace, policies, default agent).
- The dev IdP (`auth.dev_idp`) must be able to emit a configurable roles claim, so e2e and tests can drive it.
- Contract numbers: take the next free `C-` range when writing `API_CONTRACTS.md` (coordinator assigns).

## Acceptance
- SEC-78 tests (listed in `17`), proven, with the `docs/tests/security-coverage.md` row updated.
- Permission matrix covers `GET /v1/admin/identity`.
- An e2e sign-in with the dev IdP: admin value → Admin pages reachable; value removed → admin gone at next sign-in; manual role kept.
- Operator docs (`docs/operator/`): a CERN SSO example (`cern_roles`) and a Keycloak example (`resource_access.<client>.roles`).
- Usual gates: fmt, clippy, full `aurora-api` suite, web checks if UI changes, guards.

## Depends on
Nothing (can start any time). Coordinates with P4-GRP-1: the effective role is the strongest of all sources, so the `effective_access` sources list gains `idp claim <value>`.
