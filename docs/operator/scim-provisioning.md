# Directory sync: SCIM 2.0 provisioning (Entra ID, Okta, Keycloak)

(phase 4, P4-GRP-2.) SCIM 2.0 (RFC 7643/7644) lets an organization's identity provider
create, update, deactivate and delete members and maintain groups in Aurora without anyone
touching the dashboard. The design is
[docs/04 §SCIM](../04-identity-tenancy.md#scim-phase-4-p4-grp-2-d-132-d-135-sec-72-sec-73),
the wire subset [docs/16 §W15](../16-wire-protocols.md#w15--scim-20-phase-4-p4-grp-2) and the
deployment section [docs/15 §SCIM directory sync](../15-deployment-operations.md#scim-directory-sync-phase-4-p4-grp-2-d-132-sec-72-sec-73).
Nothing changes for a deployment that does not turn it on: while the instance switch is
false every `/scim/v2` path answers 404 and the dashboard shows no Directory sync page.

## Turn it on

1. **Instance switch.** Set the Helm value `scim.enabled: true`
   (`values.yaml`; the chart then renders the `/scim/v2` edge route before the `/v1` rules
   and puts `[scim] enabled = true` into the API config). Optionally cap which e-mail
   domains any org may provision with `config.scim.allowed_email_domains`
   (a config key, not a chart value of its own; empty = every domain an org owner lists is fine). The
   other chart value, `scim.ratePerMinute`, is the per-token rate limit (600/min, burst
   120).
2. **Per-org switch.** An org **owner** opens *Organization settings → Directory sync*,
   lists the org's e-mail domains (at least one; exact, case-insensitive, no subdomains),
   optionally turns on *Directory-managed membership (exclusive)* — which refuses
   invitations, JIT, role-claim and new-user placement memberships of people the directory
   did not provision (a refused role claim or placement lets the person sign in without
   that membership; a refused JIT fails the sign-in) — and saves. Turning
   SCIM on asks for a fresh sign-in (step-up re-authentication).
3. **Token.** On the same page the owner creates a provisioning token, names it after the
   IdP ("Entra ID"), and copies the **base URL** (`<your-aurora>/scim/v2`) and the
   `aud_…` token into the IdP. The token is shown **once**; at most 5 tokens can be active
   per org; a token expires (default 365 days, at most 730) and the owners get a notice 14
   days and 1 day before it does.

A token authorizes `/scim/v2/**` for its **one** org and nothing else. It can provision
members and fill SCIM groups, so treat it like an owner credential: name it, store it in
the IdP's secret store, revoke it when an IdP is decommissioned.

## What Aurora supports

The wire is the subset Entra ID, Okta and Keycloak actually send
([docs/16 §W15](../16-wire-protocols.md#w15--scim-20-phase-4-p4-grp-2)):
`POST/GET/PUT/PATCH/DELETE` for Users and Groups, a single-attribute `eq` filter,
`startIndex`/`count` pagination, PATCH with `add`/`replace`/`remove`, SCIM-format errors,
`ServiceProviderConfig`/`ResourceTypes`/`Schemas` discovery. Soft deletes: deactivating a
user (`active: false`) suspends the membership (keys revoked, sessions ended when the
person has no other org, running turns aborted); `DELETE /Users/{id}` removes it
entirely; re-provisioning re-adds. Everything else (the enterprise extension, `roles`,
bulk, sort) is declared unsupported in `ServiceProviderConfig` and answered with a SCIM
error, never silently ignored.

## Entra ID (Microsoft Entra, former Azure AD)

1. **Enterprise application.** *Entra admin center → Enterprise applications → New
   application → Create your own application* (integrate any other application; skip SSO
   if you already federated sign-in).
2. **Provisioning.** *Provisioning → Get started* → set **Provisioning Mode: Automatic**.
3. **Admin credentials.**
   - *Tenant URL*: `https://<your-aurora>/scim/v2`
   - *Secret token*: the `aud_…` token from the Directory sync page.
   *Test Connection* runs a discovery request; it must answer within a few seconds.
4. **Mappings.** Provision Azure Active Directory Users → edit **Attribute mapping** and
   keep exactly: `userName` → `emails[type eq "work"].value` (matching precedence),
   `active`, `displayName`, `name.givenName`, `name.familyName`, `externalId`. **Remove
   any `roles` mapping** — Aurora refuses the attribute (400 `invalidValue`) so a mapping
   that thinks it set a role fails the whole sync. Aurora never sets roles over SCIM.
   Add `emails[type eq "work"].value` if Entra does not send it by default; the primary
   work e-mail is what the org's domain allow-list is checked against.
5. **Group provisioning.** In the same mapping table, provision Azure Active Directory
   Groups → Aurora SCIM groups (`displayName`, `externalId`, `members`). Aurora answers
   the group operations Entra sends (`POST/PUT/PATCH/DELETE`, membership deltas).
6. **Start.** Set *Provisioning Status: On*. Entra syncs every ~40 minutes, and on demand
   with *Provision on demand*.

Entra soft-deletes: an assignment removal or account disable sends `active: false`
(suspend), a hard delete sends `DELETE` (remove). Aurora's reactivate-on-`active: true`
applies only to members SCIM itself suspended.

## Okta

1. **App integration.** *Admin console → Applications → Browse App Integration Catalogue*:
   use **SCIM 2.0 Test App (Header Auth)** (or build a custom *SAML 2.0* app with
   *Provisioning → SCIM*; the header-auth test app is the documented path for a generic
   SCIM service).
2. **Configure SCIM.** *Provisioning → Configure API Integration → Enable API integration*:
   - *SCIM 2.0 Base URL*: `https://<your-aurora>/scim/v2`
   - *Authentication mode*: HTTP Header, *Authorization*: the `aud_…` token.
   The *Test API Connectivity* button runs the same discovery request.
3. **To App feature flags.** Enable *Push New Users*, *Push Profile Updates*, *Push
   Groups* (and *Deactivate Users*). Leave *Import Users* off unless you want to pull the
   directory into Okta first.
4. **Attribute mapping.** *To App → Profile*: `userName` (the e-mail), `givenName`,
   `familyName`, `displayName`, `active`; Aurora links by the e-mail and never by an
   external id, so the mapping needs no extra attribute. Remove custom role attributes if
   the app template defines any.
5. **Assignments.** Assign people (and groups, for *Push Groups*). Okta pushes creates on
   assignment, updates on profile change, `active: false` on suspend, `DELETE` on
   deactivation of the assignment.

Aurora's group names are unique per organization (also against manually created groups);
Okta group renames arrive as `displayName` updates and fail with 409 when the name is
already taken by another group.

## Keycloak

Keycloak has **no built-in SCIM server**; use a maintained SCIM bridge/extension in front
of the realm (several exist as Keycloak extensions or as standalone provisioning services
that translate Keycloak events to the SCIM 2.0 protocol). Aurora speaks the standard
protocol, so any of them works when pointed at Aurora:

- **Base URL**: `https://<your-aurora>/scim/v2`, bearer token as above.
- **Events**: user create/update/enable/disable → SCIM `POST/PUT/PATCH` Users; group
  create/rename/member changes → SCIM Groups (`displayName`, `members[]`); the extension
  must map the Keycloak user's e-mail to `userName` — that e-mail is what Aurora links
  the account by and checks against the org's domain allow-list.
- **Attributes**: `userName`, `displayName`, `name.givenName`/`familyName`, `active`,
  `externalId` (the Keycloak user id is a good correlation id; Aurora stores it but never
  resolves by it).

The vendor fixtures in this repository (`services/aurora-api/tests/fixtures/scim/`) encode
the exact request/response sequences Entra, Okta and Keycloak are expected to send, and
the replay suite proves Aurora accepts them.

## Operation and rotation

- **Rotate on schedule.** Create the new token first, paste it into the IdP, watch one
  sync succeed, then revoke the old one. Old and new are both valid in between (up to 5
  active). An expired or revoked token fails the IdP's next request with the same 401
  provisioning never silently degrades on: the sync stops, existing state does not
  change.
- **Deactivate vs delete.** Deactivating a person in the IdP suspends their Aurora
  membership (org API keys revoked, sessions ended when they have no other org, running
  turns aborted with `user_deprovisioned`); deleting removes it. Reactivation in the IdP
  restores only memberships **SCIM suspended** — a member an org admin suspended by hand
  stays suspended.
- **Groups grant nothing by themselves.** A SCIM group becomes useful only when an org
  admin maps it to an org role or binds it to a workspace in the dashboard; the IdP
  cannot grant access, only fill groups. Deleting a group in the IdP deletes it in
  Aurora with its bindings — an availability risk, audited, never a silent one.
- **Monitoring.** `aurora_scim_requests_total{outcome}` (`ok`, `unauthorized`,
  `rate_limited`, `client_error`, `server_error`) and `aurora_scim_auth_failures_total`:
  alert on a rising `unauthorized`/auth-failure rate (an expired or rotated-away token
  still configured in the IdP) and on `server_error`. The Directory sync page shows the last request, member counts and the last
  rejected requests per org.
- **Offboarding.** SCIM cannot reassign the automations or BYO agent tokens of a
  deprovisioned person: after the IdP removes them, follow the member's offboarding view
  (`/o/<org>/members`), which lists what was disabled and which BYO agents they created.

## What SCIM never does

Sign anyone in, change a role, create or touch an organization owner, change the global
account (e-mail, name) of a person who has signed in, link an account by `externalId`,
reactivate a person an org admin suspended by hand, or read or write anything outside its
own organization (`04 §SCIM`, SEC-72).
