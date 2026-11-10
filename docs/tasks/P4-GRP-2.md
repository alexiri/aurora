# P4-GRP-2: SCIM 2.0 provisioning (`/scim/v2`)

## Goal
Implement D-132/D-135 (SPEC-P4b): org-scoped SCIM 2.0 (Users and Groups, the subset Entra ID, Okta and Keycloak use), feeding P4-GRP-1's groups, with real deprovisioning.

## Spec (source of truth; read, don't re-decide)
- D-132, D-135; SEC-72, SEC-73 (and residual risks) in `docs/17`.
- `docs/04` §SCIM (off unless instance `scim.enabled` and the org owner enables it; owner-created expiring hashed `aud_` tokens; e-mail domain allow-list; link by OIDC-verified e-mail only, never `externalId`; owners never mutated; no roles; deactivation effects; `exclusive` mode); `docs/16` W15 (wire subset: filters, pagination, PATCH ops, errors in SCIM format); `docs/03 §Planned changes` (`groups.external_id`, `scim_tokens`, `scim_users`, `organizations.settings.scim`); `docs/05` (SCIM auth surface, management routes, rate limits, token-expiry job); `docs/08` §1/§4; `docs/10` (`user_deprovisioned` turn failure); `docs/14` (Directory sync page, Members badges); `docs/15` (config, edge route `/scim/v2`, §SCIM directory sync); `docs/00` (`aud_`); `docs/01` trust boundary; `docs/02` limits.
- `docs/18` §SCIM (the tests, incl. a conformance client or recorded IdP traffic).

## Scope
Migration and repos (move the rows out of `03 §Planned changes`); the SCIM service (auth, Users, Groups, filter/pagination subset, PATCH, errors), deprovisioning effects (membership suspended, org API keys of the user revoked, sessions revoked only without other memberships, speaker turns aborted, JIT/invitations blocked), management routes and the token-expiry job; edge route `/scim/v2` in the Helm Ingress/HTTPRoute and `scripts/guards/edge-routes.sh`; web Directory sync page and Members badges; operator doc (`docs/operator/`): Entra ID, Okta and Keycloak setup. OpenAPI + `schema.d.ts`. Contracts: `docs/API_CONTRACTS.md` **C-470..C-479** only.

## Acceptance
Every test in `18 §SCIM`; SEC-72, SEC-73 `proven`; full `aurora-api` (`--test-threads=8`), helm lint/template, guards (edge-routes), web checks + Playwright for new screens, fmt, clippy `-D warnings`, OpenAPI drift test.

## Constraints
Depends on P4-GRP-1 merged. SCIM never verifies an e-mail, never changes a signed-in user's global account, never touches owners. Change files only with the Edit/Write tools. No `scripts/perf/*`; ask the coordinator before any `scripts/e2e/*`.
