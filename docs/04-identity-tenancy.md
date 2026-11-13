# 04 — Identity, Tenancy and Authorization

Phase: **0**. Tables: `03 §1`. (phase 1, RFE-004) From phase 1 **all sign-in is OIDC**: the password, e-mail-verification and password-reset surfaces of phase 0 are removed (marked below); the IdP is responsible for MFA.

## Tenancy model

```
Instance ── instance admins (user flag) · plugins · trusted keys · connectors · host-claim decisions
   └── Organization ── members (role, status) · invitations · org policy · org secrets/connections/OAuth clients
          │             channel integrations · org skills · app availability · org API keys
          │             groups (phase 4: membership · org-role mapping · workspace bindings)
          └── Workspace ── agents · secrets · connections · grants · workspace policy · API keys
                            access bindings (user → owner|member; phase 4: group → owner|approver|member)
                            timezone · memory language
```

- A user may belong to several organizations.
- Resources that support **scope** (`secrets`, `app_connections`,
  `app_oauth_clients`, policy generations, skills) are either org-scoped
  (`workspace_id IS NULL`) or workspace-scoped. Org-scoped credentials are
  *visible* to every workspace in the org but are only *used* by an agent if a
  rule or grant selects them.

## Sign-up and organization creation

Configuration `auth.signup_mode`:

| Mode | Behaviour |
|------|-----------|
| `open` | Anyone may register (phase 0: email+password or any configured OIDC provider; **from phase 1: by signing in through any configured OIDC provider**, subject to `allowed_email_domains`). A new user gets **their own organization** (`<Name>'s org`) with a default workspace. Joining another org requires an invitation. (phase 4, P4-ONB-1) Where the new user lands is the instance admin's **placement** choice (`§New-user placement`); "own organization" is only the `personal_org` mode. |
| `invite_only` (default) | Registration only by accepting an invitation, or for addresses in `auth.instance_admins`. (phase 1: "registration" is the user's first OIDC sign-in, see below.) |
| `sso_jit` | Only OIDC sign-ins; a provider's `jit` block decides which org/role/workspaces the user joins on first sign-in. Users without a matching JIT rule and without an invitation are refused (`signup_closed`). |

(phase 1, RFE-004) **Sign-up semantics under OIDC-only.** There is no separate
registration form: a user row is created just-in-time at the first successful
OIDC sign-in whose ID token carries a verified e-mail (`email_verified = true`,
or an address in the provider's `trusted_email_domains`, D-118;
`require_email_verified` is no longer configurable, see below). Whether the
sign-in may create a user depends on `auth.signup_mode`:

- `open` → always (provider `allowed_email_domains` still applies); the new user
  gets their own org and default workspace (and, if the provider has a `jit`
  block, also joins that org).
- `invite_only` → only if a **pending invitation** exists for that verified
  e-mail (the invitation is then accepted as part of the sign-in, 
  *Invitations*), or the e-mail is in `auth.instance_admins`, or a pre-provisioned
  user with that e-mail exists (`bootstrap-admin`); otherwise the sign-in is
  refused (`signup_closed`) and **no user row is created**.
- `sso_jit` → only if the provider's `jit` block applies (it decides org, role and
  workspaces) or a pending invitation exists; otherwise `signup_closed`.
- (phase 4, P4-IDP-1, D-143) In `invite_only` and `sso_jit`, a **mapped role claim also
  admits**: the validated claim of the provider that signed the user in maps, under that
  provider's `role_mappings`, to at least one grant (`instance_admin`, an org role or a
  workspace role). Unmapped values, an absent claim, a mapping that grants nothing, or a
  value mapped only under another provider never admit. Every other check above still
  applies first and unchanged (verified e-mail incl. `trusted_email_domains`,
  `allowed_email_domains`, disabled user). The admission is audited (`user.admit_by_claim`,
  provider and matched values, never the token) in the user-creating transaction. `open`
  is unchanged.

An **existing** user (matched by identity or by verified e-mail) can always sign
in, in every mode (unless disabled or suspended everywhere).

**Creating an organization** (automatically at sign-up in `open` mode, or via
`POST /v1/orgs` by users allowed by `auth.org_creation`: `instance_admins`
(default) or `anyone`) runs one transaction:

1. `organizations` row; creator is `owner`.
2. Default workspace `Default` (slug `default`), creator bound as `owner`,
   timezone from `hosted.default_timezone` (default `UTC`), memory language
   `english`.
3. For the org scope and the workspace scope: a published `policy_generations`
   row `number = 1` with no rules and `default_action = none`, plus an empty
   draft `number = 2`.
4. The workspace's **default agent** (`10 §Default agent`).

`POST /v1/orgs/{org}/workspaces` performs steps 2–4 for a new workspace.

## New-user placement (phase 4, P4-ONB-1, D-140, SEC-79)

**Why.** In `open` mode every new user gets **their own organization**, in which they
are owner. The owner's first production sign-in landed in "Alex Iribarren's Org" with a
workspace "Default". On a shared instance that is wrong: users should land where the
instance admin decided, as members, with their own workspace if the admin wants one.

**What an instance admin configures** (Admin → **Sign-up & placement**; dashboard only,
no config key (owner, 2026-11-09: "roles in Helm values, everything else through the
interface"); stored in the database, audited):

| Setting | Meaning |
|---|---|
| **Placement mode** | `shared_org` — new users join one **default organization**; `personal_org` — today's behaviour (own org, owner); `none` — no automatic placement (users enter only by invitation, role claims or JIT) |
| **Default organization** | an existing org, or "create *Users*" with a name the admin types; new users join it as `member` (or `admin`, chosen) |
| **Shared workspaces** | zero or more workspaces of the default org that every new user joins, each with a role (`member` / `approver`) |
| **Personal workspace** | on/off. When on, each user gets **their own workspace** in the default org, created at their first sign-in, with the default agent in it and the user as its `owner` (role selectable: `owner` / `approver` / `member`) |
| **Workspace name template** | e.g. `{sub}`, `{preferred_username}`, `{name}`, `{email_local}`, `{given_name} {family_name}`, `{claim:cern_upn}`; a fallback chain written with `\|` (`{sub}\|{email_local}`, the default) when a value is missing (D-151, D-162). `{sub}` is the provider's unique, never-reassigned subject, so two providers may recycle the same `preferred_username` without colliding; a name collision still gets a numbered slug (SEC-79). Preview shows the result for a typed sample or the admin's own profile — the sample never stands for `{sub}` or `{claim:…}` (Aurora stores no tokens, D-151); exact syntax in `API_CONTRACTS` C-523 |
| **Starter grants** | org-scope credentials (LLM keys, secrets, connections with full access) granted automatically to the default agent of every new personal workspace, so "bobj signs in → bobj workspace → Daneel can already answer" works with no further step. Each grant goes through the same grant path as any other (D-137, SEC-77), with the system as actor |

**Behaviour.**
- Applied once per user at the first sign-in that creates the user, **after** invitation,
  role claims and JIT (§OIDC role claims step 6). In `open` mode it is what admits a new
  user; in `invite_only` and `sso_jit` it applies to users admitted by another rule (so
  invited people also get their personal workspace).
- Example: `bobj` signs in for the first time. He joins org *Users* as `member`, joins
  workspace *General* as `member`, and gets workspace *bobj* (slug `bobj`) with *Daneel*,
  which already holds the org's LiteLLM key. He lands on its Welcome page (`14 §Onboarding`).
- **The personal workspace is tied to the user id, never to its name or slug.** A
  workspace records `personal_owner_user_id`. Later sign-ins find it by that id; renaming
  the user or the workspace changes nothing. A name collision gets a numbered slug
  (`bobj-2`) and never attaches the user to an existing workspace (SEC-79). Empty or
  unusable template results fall back down the chain, then to `user-<short id>`.
- **Existing users** are not moved. When the admin turns personal workspaces on later,
  users without one get it at their next sign-in (idempotent, same rules). Orgs made by
  the old `personal_org` behaviour stay; their owner can delete them (`§Deletion
  semantics`). The admin page lists them ("12 personal orgs from before") so the admin
  can tell people.
- Placement creates an org membership, so it passes the default org's SCIM admission gate
  and seat cap like every other membership-creating path (`§SCIM`, "Admission gate and
  seat cap", D-179): in an `exclusive` org, or one at `max_members`, a new person is not
  placed, the refusal is audited (denied `member.add`, `path = placement`) and the
  sign-in goes on without a membership.
- Under `open`, mode `none` admits only users with an invitation, a mapped role claim or a
  `jit` org; others get `signup_closed`. Starter grants whose credential has gone away are
  skipped and audited; they never fail a sign-in (D-151, `API_CONTRACTS` C-524..C-527).
- A personal workspace is an ordinary workspace of the default org afterwards: org
  owners/admins have their usual implicit access, the user can invite others into it,
  and leaving the org follows `§Deletion semantics` and the offboarding views (RFE-021).
- Defaults: until an instance admin saves a placement, an instance behaves as today
  (`personal_org` under `open`, so nothing changes on upgrade), and the admin page
  shows a warning while that holds. The **instance setup wizard** (`14 §Onboarding`)
  asks for the placement before anything else and proposes `shared_org` with a personal
  workspace. While it is unsaved on a fresh instance (no organization exists yet), users
  who are not instance admins signing in under `open` see "This instance is being set up — try again shortly" (no user row is created).

## Instance admins

- `users.is_instance_admin` grants deployment-level powers: plugin packages,
  trusted publisher keys, host-claim decisions, connectors (provision tokens,
  approve manifests), runners (provision tokens), instance overview.
- Bootstrapped by `auth.instance_admins` (list of emails): on every sign-in, a
  user whose *verified* email is in the list gets the flag; removing an email
  from the list does **not** revoke (use `aurora-api admin revoke <email>`).
  (phase 1, RFE-004) "Sign-in" is an OIDC sign-in and "verified e-mail" is the
  ID token's `email` with `email_verified = true`; the flag therefore needs an
  IdP identity (and the IdP's MFA) like every other access.
- `aurora-api bootstrap-admin --email <e>`: phase 0 created a verified user with
  a printed one-time password-set link. **From phase 1** it creates (or updates)
  a **pre-provisioned** user with that e-mail — `email_verified_at` set,
  `is_instance_admin = true`, no password, no identity — and prints the sign-in
  URL `<external>/auth/login`. The person's first OIDC sign-in with that verified
  e-mail binds the identity to the user (*OIDC step 3*); no secret is printed
  and nothing can be used to sign in except the IdP. The command refuses to run
  when no OIDC provider is configured (`no_oidc_provider`).
- (phase 4, P4-IDP-1, D-139) A third source: an IdP role value mapped to
  `instance_admin` in the provider's `role_mappings` (`§OIDC role claims`). Unlike the
  e-mail list, this source **is** revoked by the sync when the value disappears from
  the token, at the next sign-in.
- Instance admin is **not** an org role: it grants no access to org data.

## Authentication

Phase 0 supported two sign-in methods: e-mail+password and OIDC. **From phase 1
(RFE-004, D-46) the only interactive sign-in is OIDC**; there is no local
credential at all. Rationale: instance admins approve plugins (arbitrary WASM and
egress), connectors, trusted keys and host claims, so a phished password must not
be enough; the organisation's IdP already enforces MFA, conditional access and
offboarding, and Aurora does not duplicate a weaker second factor. **API keys**
(`auk_`/`auo_`) and the **CLI device login** stay (the device approval step
requires an OIDC-authenticated browser session). Phase 0 subsections that
phase 1 removes are marked *(phase 0 only)*.

### Email and password *(phase 0 only; removed in phase 1, RFE-004)*

From phase 1 the routes `POST /v1/auth/signup`, `/v1/auth/login`,
`/v1/auth/verify-email[/resend]`, `/v1/auth/password/{forgot,reset,change}` answer
404, the dashboard pages `/auth/signup|forgot|reset|verify` do not exist,
the local password column was cleared by the phase-1 migration and dropped by
CONTRACT-001 (`15 §Upgrades`), and the `argon2`/common-password machinery is removed. The
phase-0 behaviour, for reference:

- Passwords: argon2id (m=19 MiB, t=2, p=1), min 12 chars, max 256; checked
  against a bundled top-10k common-password list.
- Email verification: `aui_` token, 24 h. Unverified users may sign in but
  cannot accept invitations, be auto-linked to chat platforms, or be bootstrap
  admins.
- Password reset: `aui_` token, 1 h; using it revokes all sessions.
- Email delivery: SMTP (`email.smtp_*`). When SMTP is not configured, emails
  are written to the API log at `INFO` with the link (dev mode) and the
  dashboard says so. (phase 1, RFE-004) E-mail remains in use for **invitations**
  and, from phase 2, approval notifications (RFE-011a); it is no longer used for
  verification or password reset.

### OIDC (SSO)

Configured per provider (`auth.oidc_providers[]`, see `15`): `id`,
`display_name`, `issuer`, `client_id`, `client_secret` (env reference),
`scopes` (default `openid email profile`), `require_email_verified` (default
`true`; **removed in phase 1** — an ID token without `email_verified = true` is
always refused with `email_unverified`, and a configuration that still sets
`false` fails startup naming the key), `allowed_email_domains` (optional),
`trusted_email_domains` (optional, D-118: exact domains the provider is
authoritative for; an `email` in one of them counts as verified even when the
ID token lacks `email_verified = true`, for IdPs that never send the claim.
Exact, case-insensitive domain match, no subdomains; any other address still
needs the claim. Only list domains whose mailboxes the provider's own accounts
own: a federated IdP that also admits external accounts must not let them
assert an address in the domain), `jit`
(optional: `{ org_slug, role: member|admin, workspace_slugs: [] }`; phase 2, RFE-012: optional `workspace_role: member|approver`, default `member`).
(phase 1, RFE-004) At least one provider MUST be configured, or startup fails
(`no_oidc_provider`) unless the development IdP is enabled
(*Development and end-to-end sign-in* below). Provider-specific MFA/step-up is
the IdP's concern; Aurora requests `prompt=login&max_age=0` only for
re-authentication (below) and never assumes an `acr`/`amr` claim.

Flow:

1. `GET /v1/auth/oidc/{provider}/start?return_to=…` → 302 to the provider with
   PKCE (S256), `nonce`, and an **encrypted state** (AEAD, purpose `oidc-state`,
   TTL `OAUTH_STATE_TTL`) also set as a `__Host-aurora_oidc` cookie; `return_to`
   must be a same-origin path.
2. `GET /v1/auth/oidc/{provider}/callback` validates state == cookie, exchanges
   the code, verifies the ID token (issuer, audience, nonce, expiry, signature
   via discovery JWKS).
3. Identity resolution: `user_identities (provider_id, sub)` → user. Else, if
   the email is verified by the provider, link to the existing user with that
   email (this also binds a **pre-provisioned** user, e.g. from
   `bootstrap-admin`). Else create a user if the sign-up mode allows (or JIT
   applies). A user created here has `email_verified_at = now()` (the provider
   verified it) and no local password. A disabled user (`users.disabled_at`)
   is refused (`user_disabled`).
4. JIT: if the provider has a `jit` block and the user is not a member of that
   org, add membership with the configured role and workspace bindings.
5. Create a session; 302 to `return_to`.

(phase 4, P4-GRP-2, D-132, SEC-73) **SCIM and sign-in.** SCIM provisions *directory
state* (users, memberships, groups); it never signs anybody in and is never evidence of
identity. A user pre-provisioned by SCIM is linked by step 3 exactly like a
`bootstrap-admin` user — **only** through the provider's verified e-mail (D-118 trusted
domains included) — and `externalId` is stored for the IdP's own correlation and is never
used to find, link or authenticate a user. Steps 3 and 4 also never **create or
reactivate a membership of an org that has a SCIM link for the user in state
`suspended` or `removed`** (the sign-in succeeds for other orgs; JIT and invitation
acceptance for that org are skipped, audited as refused), so a deprovisioned person cannot
re-enter through JIT while their IdP session lingers (*SCIM*).

**Re-authentication** (phase 1, RFE-004). `GET /v1/auth/oidc/{provider}/start?
return_to=…&reauth=1` sends `prompt=login` and `max_age=0`; on the callback the
core requires the same user and (when the IdP returns `auth_time`) a value within
60 s, then stamps `sessions.reauth_at = now()`. Operations that formerly
required the password (account deletion) require `reauth_at` within 10 minutes
(412 `reauth_required` otherwise; the dashboard starts the flow).

**Session and IdP lifetime.** Aurora sessions are independent of the IdP session
(no back-channel logout in phase 1). Offboarding is therefore: disable/suspend in
Aurora (`users.disabled_at` or org suspension kills sessions and keys at once,
`SEC-21`/`SEC-29`) and/or shorten `auth.session_absolute_secs`; see `17 §Residual
risks`.

### OIDC role claims (phase 4, P4-IDP-1, D-139, SEC-78)

**Why.** The organization's IdP already knows who administers what: the owner's
production IdP (CERN SSO) assigns application roles to e-groups and puts them in the
token. Aurora should read those roles instead of having someone re-enter them in the
dashboard. Mapping groups to roles is the IdP's job; Aurora maps **role values** in
the token to **Aurora roles**.

**Configuration** (per provider in `auth.oidc_providers[]`, `15`; set in the **Helm
values** (`config.auth.oidc_providers`) like the rest of the provider, never editable in
the dashboard, because it can create instance admins and owners; the dashboard only
shows it, read-only):

```yaml
auth:
  oidc_providers:
    - id: cern
      # ...issuer, client_id, ...
      role_claim: cern_roles            # dot path into the ID token, e.g. roles, groups,
                                        # resource_access.aurora.roles (Keycloak)
      role_claim_source: id_token       # id_token (default) | userinfo
      require_role: false               # true: a sign-in whose claim maps to nothing is refused
      role_mappings:
        - value: aurora-admins          # exact, case-sensitive match on one claim value
          instance_admin: true
        - value: aurora-users
          org: { slug: users, role: member }
        - value: it-ai-team
          org: { slug: it, role: owner }
          workspaces: [{ slug: infra, role: owner }, { slug: research, role: approver }]
```

- The claim may be an array of strings or a single string with space- or
  comma-separated values; anything else counts as no values (logged once per
  provider).
- One value may grant several things. Targets: `instance_admin`, an org role (`owner` |
  `admin` | `member`), and workspace roles (`owner` | `approver` | `member`) in that
  org's workspaces. An org or workspace named in a mapping that does not exist is created
  at the first sign-in that needs it (an org with the default workspace and default agent,
  `§Sign-up and organization creation`; a workspace with its default agent), so a fresh
  instance needs no hand-made orgs.
- Startup validation: unknown keys, duplicate values, an invalid role or slug, and
  `require_role: true` without `role_mappings` refuse startup naming the key.

**Behaviour at every sign-in** (not only the first):

1. Aurora computes the user's **claim-derived** assignments from the validated token
   (after step 2 of the flow, never from unvalidated data) and **syncs** them. Missing
   ones are added. Claim-derived ones whose value is gone are removed. Removing an org
   role drops the user to the strongest role they still hold from another source. If
   they have no other source, the membership is removed: it was the claim that admitted
   them.
2. Each assignment remembers its source (`idp:<provider>:<value>`). Roles given by hand
   (invitation, dashboard, JIT, SCIM, `bootstrap-admin`, `auth.instance_admins`) are
   **never** changed or removed by the sync, and the strongest source wins (as for
   groups, `§Groups`).
3. In the dashboard a claim-derived role shows a badge "From IdP role `aurora-admins`" and
   cannot be edited or removed there (the IdP is the source of truth). Org owners can
   still add a manual role on top.
4. **Last owner**: a sync never removes an org's last owner; it keeps the role, audits
   `org_role.idp_sync_blocked`, and shows a notice to instance admins.
5. `require_role: true` refuses a sign-in whose claim maps to nothing (`no_mapped_role`,
   no user row created). This lets an operator say "only people the IdP gave an Aurora
   role may enter".
6. Precedence when the user is new: invitation → role claims → provider `jit` →
   new-user placement (`§New-user placement`). They add up and never subtract.
7. **Owner-integrity floor** (FIX-050, D-165, C-630): after the sync's org
   step, an organization that any org-role mapping of the signing-in provider
   created or touched must not end the sign-in transaction without an active
   owner. If it would, the syncing user is promoted to `owner` (their
   claim-derived `idp_role`/`idp_source` stay; audited `member.update` with
   `reason: "ownerless_org"`). A suspended or absent membership is never
   promoted. The last-owner rule (4) then protects the floor like any other
   owner, and once the claim grants a real owner the sync demotes the floor
   member back to the claim's role. (Found as: org created by a `member`
   mapping, audit `member.add … "created_org": true`, and every gateway
   request of the org refused 407 — the gateway's credential lookup requires
   an active owner, `06 §Credential authentication`.)

**Timeliness.** Claims are read at sign-in, so a role removed in the IdP takes effect
at the user's next sign-in (bounded by `auth.session_absolute_secs`). For immediate
deprovisioning use SCIM (`§SCIM`) or disable the user; `17 §Residual risks` records it.

**Visibility.** The account page shows "Roles from your sign-in" (value → what it
granted). Instance admins see per user the last synced claim values and what they
mapped to. That is the claim debugger an operator needs when "it doesn't work".

**Audit** (security-critical): `instance_admin.grant|revoke`, `org_member.add|update|remove`
and `workspace_access.set|remove` rows written by the sync carry `actor_kind = service`,
`actor_label = idp:<provider>` and `metadata.claim_value`.

### Development and end-to-end sign-in (phase 1, RFE-004)

With OIDC-only there must be a way to sign in without a corporate IdP:

- **Dev IdP** — the existing mock IdP of `aurora-testkit` (`aurora_testkit::oidc`:
  discovery, JWKS, PKCE, nonce, configurable claims) is packaged as a tiny
  binary/image `aurora-dev-idp` (development and tests only; never in the Helm
  chart's default values). The kind dev stack runs it as the chart component `devIdp`
  (`deploy/kind/values-dev.yaml`, served through the Ingress at `/dev-idp`); `just dev`
  starts it as a host process on `:4107`. It serves a login page listing the users of a
  fixture file (`deploy/dev/dev-idp-users.json`, in the chart `devIdp.users`:
  `{ sub, email, name, email_verified }`) and signs in non-interactively when the
  authorize request carries `login_hint=<email>` of a listed user (used by Playwright and
  scripts). The dev stacks also set `AURORA_DEV_IDP_ALLOW_ANY_HINT=1` / `devIdp.allowAnyHint`,
  which accepts an unlisted hint (the e2e suites mint their own e-mails). `GET
  /v1/auth/oidc/{provider}/start` forwards an optional `login_hint=<email>` to the provider.
- **Wiring** — `auth.dev_idp.enabled = true` (default `false`) registers the extra
  provider `id = "dev"`, `display_name = "Development sign-in"`,
  `issuer = auth.dev_idp.issuer`, with a fixed development client. Startup logs a
  prominent warning, the dashboard login page shows a banner, and startup
  **refuses** the combination when `server.external_url` is https on a
  non-loopback host or is not one of `localhost`, `*.localhost`, `127.0.0.1`,
  `[::1]` — the dev IdP can never be exposed by accident.
- **Tests** — unit/service suites use the in-process `aurora_testkit::oidc` mock
  and a helper `sign_in(email)` that drives the real start/callback and returns
  the session cookie; **headless suites authenticate with API keys** and never
  touch OIDC. e2e (Playwright, `scripts/e2e/*`) signs in through the dev IdP with
  `login_hint`. The IdP mock can issue `email_verified = false`, wrong
  nonce/audience, expired tokens and `auth_time` values to prove the refusals.

### Sessions

- Cookie `aurora_session` = `aus_` token; `HttpOnly`, `SameSite=Lax`,
  `Path=/`, `Secure` when the external URL is https. Stored hashed in
  `sessions`. (phase 4, P4-PRV-2) It is **host-only: it never has a `Domain`
  attribute** (an invariant, `§Preview access`), so no other host, preview hostnames included, ever receives it.
- Idle expiry `SESSION_IDLE`, absolute `SESSION_ABSOLUTE`. Revocation is
  immediate (every request reads the session row; a 30 s in-process cache keyed
  by token hash is allowed, flushed on sign-out by `NOTIFY`).
- "Sign out other sessions" revokes all of the user's sessions except current.
- **CSRF**: every cookie-authenticated request with an unsafe method MUST carry
  an `Origin` header equal to the external origin (or one of
  `auth.trusted_origins`); otherwise 403 `csrf_rejected`. JSON bodies only
  (`Content-Type: application/json`) except multipart upload routes, which are
  covered by the same Origin check.

### API keys

- `auk_` workspace key: confined to its workspace; workspace routes infer the
  workspace from the key.
- `auo_` org key: org-wide authority of its owner's role; workspace routes
  require `X-Workspace-Id` (or the workspace in the path).
- `Authorization: Bearer <key>`. A request that presents an Aurora bearer
  prefix commits to key auth: failure is 401, never a fall-through to cookies.
- Liveness: a key is dead when revoked, expired, or its owner is no longer an
  *active* org member (checked on every request; cached ≤30 s).
- A key acts with **the owner's** current permissions intersected with the key's
  scope. Service keys (`kind = service`) are minted by platform features
  (none in phase 0 beyond tests) and hidden from personal key lists.
- Session-only routes (refuse API keys with 403 `session_required`): delete
  account, change password *(phase 0 only)*, sign out other sessions, accept
  invitation, create API keys, approve a CLI login. (phase 1, RFE-004) Delete
  account additionally needs a recent re-authentication (above).

### CLI login (device flow)

1. `POST /v1/auth/cli` → `{ device_code, user_code, verification_url, interval, expires_in }`.
2. The person opens `<external>/auth/cli?code=ABCD-EFGH`, signs in (phase 1:
   through OIDC, returning to this page), chooses a workspace, approves.
3. CLI polls `POST /v1/auth/cli/token { device_code }` → `authorization_pending`
   | `{ api_key, workspace }` once (status → `consumed`). The key is a `user`
   workspace key named `CLI on <hostname>`.

### Rate limits (auth)

In-memory token buckets per API replica, keyed by client IP (from
`X-Forwarded-For` as **set** by the edge proxy; the edge MUST overwrite, not
append) and by email: sign-in 10/min/IP and 5/min/email; sign-up 5/h/IP;
reset 3/h/email *(all phase 0 only)*; CLI token polling governed by `interval`
(`slow_down` error). (phase 1, RFE-004) Replaced by: OIDC `start` and `callback`
`OIDC_AUTH_RATE` (20/min/IP each) and CLI token polling as before. The general
per-principal API limiter of phase 2 (RFE-017, `05 §Conventions`) applies to every
authenticated request on top of these and returns the same 429 + `retry-after` shape.
(phase 4, P4-CNT, D-119) These per-IP and per-e-mail buckets are **in memory per
replica by default** and **shared across API replicas** with
`ratelimit.counters.backend = postgres | redis`; they are the class `security` of
`05 §Conventions` *Shared counters*: if the backend is unreachable they fall back
to the per-replica bucket (`local`, the default) or refuse with 503
`counter_unavailable` (`closed`), and never fail open (SEC-62). IP and e-mail
values are stored only as keyed hashes.

## Authorization

### Roles

| Layer | Values | Notes |
|-------|--------|-------|
| Org role | `owner` > `admin` > `member` | Always enforced. |
| Membership status | `active`, `suspended`, `pending` (phase 4, D-175) | Suspended = non-member for **every** check including API keys, channel clicks, schedules. `pending` (SCIM prepared it for an existing account; becomes `active` only in that person's next verified sign-in) is the same: a non-member for every check, but it counts against the seat cap and may sit in SCIM groups. An admin can neither set nor leave `pending` (409 `member_pending`), only remove the member. |
| Workspace access | `owner`, `member` — phase 0/1. **From phase 2 (RFE-012): `owner` > `approver` > `member`** | Org owners and admins are implicit workspace owners of every workspace in their org. `approver` is one deliberate notch between chatting and owning: it adds the authority to decide approvals. |
| Group (phase 4, P4-GRP-1) | member of zero or more org groups | A group can hold workspace roles (`owner`, `approver`, `member`) and map to an org role (`admin`, `member`; never `owner`). The **effective** role is the strongest of the direct and group-derived ones, see *Groups* below. A group never creates membership. |
| Freeze state (phase 2, RFE-006) | not frozen / `frozen` — per **agent**, **workspace** and **organization** | A reversible, audited emergency switch, see *Freeze* below. |

### The access law

`can_use(user, workspace)` is true iff: the workspace exists **and** the user
has an `active` membership in its org **and** (the user's org role is `owner` or
`admin` **or** a `workspace_access` row exists). `can_own(user, workspace)` is
the same with workspace role `owner` (or org owner/admin).

**`can_approve`** (phase 2, RFE-012, D-55): `can_approve(user, workspace)` is the
same with workspace role `owner` **or** `approver` (or org owner/admin). The
hierarchy is `can_own ⊂ can_approve ⊂ can_use`. A plain `member` chats with the
agents, reads what `can_see_conversation` allows and operates agents (memory,
skills, schedules, …) but **cannot decide gateway approvals, action approvals or
agent-link requests** — approval authority is the crown jewel of the product and
no longer comes with the cheapest role. `can_approve` is implemented once in
`aurora-db::authz` with the others and used by every decision surface: HTTP
routes, channel clicks and `approve <code>` commands, batch decisions, long-polls
and SSE feeds. There is no separation-of-duties rule: an approver may decide a
request their own turn caused.

The access law is implemented once (`aurora-db::authz`) and used by every
surface: HTTP routes, channel speakers and clickers, approvals, schedule and
webhook fire-time checks, peer messaging.

(phase 4, P4-GRP-1, D-130) **Groups extend the law without changing its shape.** In
the three definitions above, "the user's org role" means the **effective org role**
and "a `workspace_access` row exists / workspace role" means the **effective
workspace role**, both defined in *Groups* below (the strongest of the direct and the
group-derived value). With no groups in the database the result is exactly the
phase 0–3 result. Nothing else in this section changes: `can_approve`,
`can_see_conversation` and every other predicate are built from `can_use` / `can_own`.

**Conversation visibility** (phase 1, RFE-009, D-47). Phase 0 gated transcript
reads by `can_use` alone, so every workspace member could read every other
member's `direct` (1:1) conversation with an agent, including DMs sent from a
chat platform. From phase 1, `can_see_conversation(user, conversation)` — also in
`aurora-db::authz`, applied by every surface that names or returns a
conversation — is true iff:

- the conversation's kind is **not** `direct`, and `can_use(user, workspace)`
  (unchanged); or
- the kind is `direct` and **either** `user = conversation.direct_user_id` **and**
  `can_use(user, workspace)` (a person who lost access to the workspace stops
  seeing even their own direct conversation), **or** `can_own(user, workspace)`
  (workspace owners and org owners/admins, who need repair access).

Writing into a `direct` conversation (posting messages, uploading attachments for
it) is the direct user's alone: a workspace owner who is not that user can
**read** it and can **abort/send-now** its turns and (from phase 2, RFE-010)
archive, delete or redact it, but cannot post as that person (403
`direct_conversation_forbidden`). An owner reading another person's direct
conversation writes an audit row `conversation.owner_view` (once per actor,
conversation and hour; best-effort).

(phase 1, RFE-009, D-47 clarification C) Conversation visibility is evaluated within
the conversation's own workspace. A row in another workspace that merely references the
conversation (the target side of a cross-workspace action approval, `07 §Action
approvals`) is governed by its own workspace's law and carries the reference redacted
(`null` + `conversation_hidden: true`); its decider never receives the conversation id or
the frozen payload.

### Permission matrix

| Action | Required |
|--------|----------|
| Org settings, delete org, transfer ownership | org `owner` |
| Members, invitations, role changes (admins cannot touch owners or grant `owner`) | org `owner`/`admin` |
| Org policy, org secrets/connections/OAuth clients, org skills, app availability, **org governance** (LLM and egress), org API keys, channel integrations | org `owner`/`admin` |
| Org LLM keys | org `owner`/`admin`, **and** org governance permits `organization`-scoped keys for that endpoint (`08 §11`) |
| Read effective governance | any active org member |
| Org audit log | org `owner`/`admin` |
| Create workspace | org `owner`/`admin`, or any member if `organizations.settings.members_can_create_workspaces` (default `true`) |
| Workspace settings (name, timezone, memory language), access bindings (phase 2, RFE-012: including granting `approver`), delete workspace | `can_own` |
| Workspace secrets, connections, grants, workspace policy (incl. the workspace uncredentialed default), workspace API keys | `can_own` |
| Workspace LLM keys | `can_own`, **and** org governance permits `workspace`-scoped keys for that endpoint, **and** the key's origin complies (`08 §11`) |
| Space bindings for an agent of the workspace | `can_own` |
| Create/edit/delete agents, brief, model, avatar; memory; agent-tier skills; schedules; webhooks; restart sandbox | `can_use` |
| Create/edit/delete **workspace-tier skills** (phase 3, RFE-020); read them | `can_own` to write (org owners/admins included); read: `can_use` |
| Create a **BYO** agent; regenerate a BYO agent's token (phase 2, RFE-025) | phase 0/1: `can_use`. **From phase 2: by the effective BYO posture** (`08 §11.9`): `enabled` → `can_use`; `owners_only` (default) → `can_own`; `disabled` → nobody |
| Edit a BYO agent's source CIDR list and "require context credentials" setting (phase 2, RFE-025) | `can_own` |
| Workspace BYO posture (phase 2, RFE-025) | `can_own` (tighten-only) |
| Org BYO posture, org default source CIDRs, org require-context default (phase 2, RFE-025) | org `owner`/`admin` (through governance, `governance.update`) |
| List BYO agent tokens (expiry, last use, stale) of a workspace / of an org (phase 3, RFE-013) | `can_own` / org `owner`/`admin`; org maximum age of agent tokens: org `owner`/`admin` through governance (`governance.update`, which records the retrofit) |
| Chat with agents, read transcripts, upload attachments | `can_use`; (phase 1, RFE-009) for `direct` conversations: the direct user (read + write) and `can_own` (read, abort/send-now only) — `can_see_conversation` |
| Decide gateway approvals, action approvals and agent-link requests for agents of the workspace; read the approvals feed (pending, history, long-poll, SSE) | phase 0/1: `can_use`. **From phase 2 (RFE-012): `can_approve`** (a plain member sees an approval waiting inline in their own visible conversation, read-only, but not the feed and cannot decide); (phase 1, RFE-009) an approval that names a `direct` conversation is visible and decidable only by callers who `can_see_conversation` it (so: the direct user and `can_own`) |
| Archive/unarchive a conversation (phase 2, RFE-010) | `can_see_conversation` and, for non-direct kinds, `can_use` |
| Delete a conversation; redact transcript events; export a conversation (phase 2, RFE-010) | `direct`: its user or `can_own`; other kinds: `can_own` (redaction also: the event's turn author; export: not the turn author); always subject to `can_see_conversation` |
| Org retention settings (phase 2, RFE-010/015) | org `owner`/`admin` (`retention.update`, security-critical) |
| Read activity (request log) of the workspace | `can_use`; (phase 1, RFE-009) rows whose conversation is a `direct` conversation the caller cannot see are **omitted** (owners see all) |
| Reassign a schedule/webhook creator, set its anchor, bulk reassign; read the offboarding view of a workspace binding (phase 2, RFE-021) | `can_own` (the offboarding view of an **org** member: org `owner`/`admin`) |
| Freeze / unfreeze an **agent** (phase 2, RFE-006) | `can_own`; lifting a freeze imposed by a higher authority needs that authority (below) |
| Freeze / unfreeze a **workspace** (phase 2, RFE-006) | `can_own` (org owners/admins included); same lifting rule |
| Freeze / unfreeze an **organization** (phase 2, RFE-006) | org `owner`, or instance admin |
| Create, rename, delete a group; add/remove members of a `manual` group; set or clear a group's org-role mapping (phase 4, P4-GRP-1) | org `owner`/`admin` (a mapping to `admin` is as powerful as granting `admin` by role change, which admins may already do; `owner` can never be mapped) |
| List groups with names, source and member counts; read a group's members (phase 4, P4-GRP-1) | members and the impact view: org `owner`/`admin`; names and counts only (for the binding picker): `can_own` of any workspace of the org; a member sees only the groups they belong to (`/v1/me`) |
| Bind / unbind / change the role of a **group** on a workspace, read the workspace's group bindings and the group-derived members (phase 4, P4-GRP-1) | `can_own` of the workspace (the same authority as user bindings, including granting `owner`); the group must belong to the workspace's org |
| Register a Web Push subscription for the caller's browser, list, pause, remove one's own, send oneself a test (phase 5 (D-138, moved from phase 4), RFE-011b) | any signed-in user, own subscriptions only; session only (an API key cannot register a browser); a push is delivered only to the live session that registered it |
| Enable SCIM and set its e-mail domains / `exclusive`; create, list, revoke SCIM tokens; read Directory sync status (phase 4, P4-GRP-2) | org `owner` only; session only; enabling and token creation need a recent re-authentication (412 `reauth_required`) |
| Switch **Port previews** on or off for the org (`settings.previews_enabled`, default off) (phase 4, P4-PRV-1) | org `owner`/`admin` (`org.previews.update`, security-critical) |
| Create, pause/resume, change the start command of, delete a preview share; list the agent's shares; start the app now (phase 4, P4-PRV-1) | `can_use` of the agent's workspace, session or API key; **never an agent tool**; BYO agents have none; frozen agent: 423 |
| Open a preview in the browser (phase 4, P4-PRV-2) | a **live dashboard session** of a user for whom `can_use(workspace)` holds, re-checked at least every `PREVIEW_AUTH_CACHE` (15 s); never an API key, never an agent credential, no other visibility (`04 §Preview access`) |
| Create, edit, pause, delete a **skill library**; "Check now"; read library settings and status (phase 4, P4-GSL) | org `owner`/`admin`; the connection must be org-scoped |
| Browse a skill library's catalogue and read one skill at a commit (phase 4, P4-GSL) | any user who `can_use` a workspace of the org (names, descriptions and content an agent could be told; nothing is stored) |
| Add a library skill to an agent / a workspace / the org; approve, reject a proposed library update; detach; remove (phase 4, P4-GSL) | the **tier's write authority**, unchanged from manual skills (SEC-56): agent `can_use`, workspace `can_own`, org `owner`/`admin`; **never an agent tool**; session or API key for adds, **session only** for approve/reject/detach |
| Plugins, trusted keys, host claims, connectors, runners | instance admin |

Org admins/owners can therefore always repair a workspace; members of a shared
workspace can operate its agents but not change what credentials they hold.

### Fire-time authority

Schedules, webhooks, peer tasks and reach decisions store their **creator**.
At every fire the access law is re-checked for that creator on the agent's
workspace; if it fails the item is disabled with `disabled_reason =
authorization` and the origin conversation receives a notice.

**Keeping automations alive across offboarding (phase 2, RFE-021, D-58).** Phase 0
was correct as a default but destructive in practice: when an employee left, every
schedule and webhook they had set up silently died, with a conversation notice as
the only warning and no way to re-anchor an item except recreating it. From
phase 2, for **schedules and webhooks** (peer tasks are 24 h-bounded and reach
decisions are not automations; both are unchanged):

- **(a) Reassignment.** A `can_own` user of the agent's workspace may change an
  item's `creator_user_id` to any **active org member who `can_use` the
  workspace** (422 `new_creator_unauthorized` otherwise); the new creator is the
  fire-time authority from then on. Reassigning an item disabled for
  `authorization` can re-enable it in the same call (`re_enable: true`, clears
  `disabled_reason`, recomputes `next_fire_at`). Audited `automation.reassign`
  (security-critical, `04 §Audit`: kind, id, old and new creator). Bulk form for
  offboarding: all items of one creator in a workspace in one transaction.
- **(b) Offboarding view.** Before removing or suspending a person, an owner can
  ask *what would break*: `GET /v1/orgs/{org}/members/{user}/offboarding` (org
  `owner`/`admin`; every workspace of the org) and
  `GET /v1/workspaces/{ws}/members/{user}/offboarding` (`can_own`; for removing the
  person's workspace binding). It lists the enabled schedules and webhooks whose
  fire-time authority is that user (`would_disable`), plus open peer tasks and a
  count of API keys that would die. The removal routes accept `reassign_to`
  (a user id or `"workspace"`) to reassign everything **atomically** in the same
  transaction as the removal; the dashboard shows the list and offers the
  reassignment before it confirms (*"Removing Alice disables these N schedules and
  webhooks — reassign?"*).
- **(c) Workspace-anchored automations.** A schedule or webhook has an `anchor`:
  `creator` (default; the rule above) or **`workspace`**, whose authority is
  **"an active owner exists"**: at every fire/delivery the check is that the
  workspace has at least one active user with `can_own` (workspace `owner`, or org
  owner/admin — an org always has an active owner while it is active, so an
  anchored item survives any individual offboarding and dies only with the org's
  or workspace's existence, a suspension of every owner, or a freeze). Only a
  `can_own` user may create an anchored automation or switch an item's anchor; **an
  agent can never create or anchor one through its tools** (a prompt-injected agent
  must not be able to mint immortal automations) — tool-created items are always
  `creator`-anchored to the turn's speaker. `creator_user_id` stays on anchored
  items as provenance only. The dashboard labels them "Workspace" and the audit row
  records the anchor change (`automation.reassign`, `to: workspace`).

## Groups (phase 4, P4-GRP-1, D-130, D-131, SEC-71)

Until phase 3 every grant of access names one person: a workspace binding per user,
a role per member. An organization with an IdP wants "everyone in *Platform Team* may
use workspace *Infra*", maintained in one place. A **group** is a named, flat set of
an organization's members (no nesting) that can hold workspace access and, optionally,
an org role. Tables: `03 §Planned changes` (`groups`, `group_members`,
`group_workspace_access`). Groups are **manual** (created and filled in the dashboard or
API, this section), **IdP-linked** (created in the dashboard, membership synced from a
sign-in claim, *IdP-linked groups* below, D-156) or **SCIM** (created and filled by the IdP,
`§SCIM`); everything else is identical. Nothing changes for an organization that has no groups: the access law
returns the phase 0–3 answers and no new row is read on a path that finds none.

**Membership.** A group lists users who already have an `org_members` row of the group's
org (the table's composite foreign key makes removal from the org remove the user from
every group, and a member can be added only if the org knows them). A group **never
creates, reactivates or removes an org membership**: admission stays with invitations,
JIT and SCIM user provisioning, so a group cannot be used to put a stranger into an
organization. A `suspended` member stays listed but derives nothing from any group
(suspended = non-member for every check, *Roles*). Limits (`02 §Limits`): 500 groups per
org, 5 000 members per group, 100 groups per user, 200 workspace bindings per group.
Names are unique per org, case-insensitively, 1–80 characters, sanitized.

**What a group can hold.** Only two things:

1. **Workspace bindings**: `(group, workspace, role)` with the same vocabulary as a user
   binding, `owner` > `approver` > `member`. A group may hold `owner`; binding it needs
   `can_own` on the workspace exactly as for a user, and the group must belong to the
   workspace's org.
2. **An org-role mapping** (optional): `admin` or `member`. `owner` cannot be mapped,
   so ownership, ownership transfer, the last-owner invariant and `instance admin`
   remain direct, human decisions and no directory change can create an owner.
   (phase 4, D-139) The one exception is operator configuration: a provider's
   `role_mappings` (`§OIDC role claims`) may name `owner` and `instance_admin`, because
   the operator who writes the config file already controls the instance. Groups and SCIM
   still cannot.

**Effective roles** (the single definition; implemented once in `aurora-db::authz` as
`effective_access(user, workspace)` by one query, and no other code reads
`workspace_access` or the group tables to authorize):

- *Effective org role* = the strongest of `org_members.role` and the `org_role` of every
  group the user belongs to in that org, **only while the membership is `active`**
  (a user without an active membership has none, whatever groups say).
- *Effective workspace role* = the strongest of the direct `workspace_access` row, the
  role of every `group_workspace_access` row whose group contains the user, and
  implicit `owner` when the effective org role is `owner` or `admin`. Strongest, never a
  sum or an intersection: a group can raise a person above their direct binding and can
  never lower it, and removing a direct binding leaves what groups give.
- `can_use`, `can_own` and `can_approve` are the phase 2 predicates evaluated on those
  effective values (*The access law*). The effective result carries its **sources**
  (`direct`, `group <id>`, `org_role via group <id>`, `org role`) so the dashboard can
  explain it; sources are informational and never an input to a decision.

Consequences that need no special case: API keys act with the owner's *current* effective
permissions (a group change reaches keys on the next request); `can_see_conversation`,
approval decider sets and the e-mail/push recipient set (`07 §Approvals`) are built from
`can_use`/`can_own`/`can_approve`; schedules and webhooks re-check their creator at every
fire, so a creator whose only access came through a group stops firing when removed
(`disabled_reason = authorization`, notice, *Fire-time authority*); anchored automations
survive as before. The offboarding views (RFE-021) are computed from effective access
and add `via_groups` to each row (the groups that currently give the person access), so
"what would break" is accurate for people who have no direct binding. A **group impact
view** (`GET /v1/groups/{id}/impact`, optional `?remove_member=<user>`) answers the same
question for deleting a group, unbinding it, or removing one member: per workspace the
users who would lose all access (at most 200 listed, with the total), the automations
that would be disabled, and the API-key count; the dashboard shows it before it confirms.

**Policy identities** (`07 §Rule anatomy`): a rule may name `{ "kind": "group", "id" }`.
It matches only when the request has a **speaker** who is in the group (the evaluator's
principal set gains the speaker's group ids; no speaker, no group ids), consistent with
the `user` identity binding of RFE-005 (SEC-44). That is the only place a group is
visible to the gateway.

**Recomputation and invalidation.** The access law reads the database on every
request: there is **no per-user authorization cache** in the API (the 30 s caches of
sessions and key liveness hold the session or key row, not roles), so a group change is
effective for the next request of every user it touches. The slow consumers are told:

| Change | Signals (after commit, `05 §Gateway cache invalidation`) |
|--------|--------------------------------------------------------|
| Member added/removed (manual route or SCIM) | `aurora_gateway_invalidate` scope **`user`** (new, id = the user: gateways drop every cached identity whose speaker is that user and re-resolve their group set and `can_use`); `aurora_auth_revoked { kind: "access", id: user }` |
| Group binding added/changed/removed; group deleted | scope `workspace` for each affected workspace (`org` when a deletion touches more than 20); `aurora_auth_revoked { kind: "access", org_id, id: null }` (every user of the org) |
| Org-role mapping changed; group deleted with a mapping | scope `org`; the same `access` notification |
| User removed from the org / suspended | unchanged (`org`/`workspace` scopes of the existing rules) |

`aurora_auth_revoked` with `kind = "access"` makes every API replica **re-authorize the
live SSE streams and long-polls** of that user (or of the whole org when `id` is null):
a stream the user may no longer open is closed with a final `access.changed` event and
the client reconnects and gets 403/404 (`05 §Conventions`). Sessions are **not** revoked:
a session is an identity, not an access grant, and the user may still hold other orgs.
**Running turns** are not aborted by a group change (exactly as a membership change does
not abort them today); the speaker binding of the turn's context credential is
re-resolved at the next gateway request, so a person who lost `can_use` stops matching
`user`/`group` rules from then on (`06 §Credential authentication`, RFE-005), and a held
approval can only be decided by someone who passes the decider check at decision time.
Bulk changes (SCIM) deduplicate notifications per user within a transaction; above 1 000
users in one transaction a single org-wide notification is sent instead.

**Audit** (security-critical, `§Audit`): `group.create|update|delete`,
`group_member.add|remove`, `group_access.set|remove`. A mapping change is a
`group.update` with `metadata.change` (`org_role: member → admin`). One row per member
and per binding changed (a SCIM group `PUT` writes one row per added or removed member).
Rows written for the IdP carry `actor_kind = service`, `actor_label = scim:<token name>`
and `metadata.via = "scim"`; there is no new `audit_log.source` value (the column is
checked, `03 §1`) — SCIM rows use `source = api`.

**Deletion.** Deleting a group deletes its memberships and bindings in one transaction
and invalidates as above. Policy rules that name the group are **not rewritten**: a
rule whose identities are all dead groups must keep matching nobody rather than become
an "anyone" rule (`07 §Rule anatomy`, SEC-71). Deleting a workspace deletes its group
bindings; deleting a user or removing an org member deletes their memberships; org
deletion deletes the org's groups after their children (the foreign key is `restrict`).
There is no nesting, no import and no export in phase 4; JIT (`§OIDC` step 4) assigns
org membership and workspace bindings only, never groups.

### IdP-linked groups (phase 4, P4-GRP-1, D-156)

Many IdPs already maintain the groups an organization cares about (CERN: e-groups). An
**IdP-linked group** is an Aurora group whose membership the IdP decides, so nobody
maintains the list twice.

- **Link.** An org owner/admin creates a group with `source = idp` and names one
  **provider id** and one **claim value** (`idp_provider`, `idp_value`; exact,
  case-sensitive match, ≤ 256 chars). The provider's claim path is configured per provider
  as `group_claim` (and `group_claim_source`: `id_token` (default) or `userinfo`, with the
  P4-IDP-1 rules: a userinfo answer counts only if its `sub` equals the ID token's) in
  `auth.oidc_providers[]` (`15`). For CERN SSO the recommended claim is `cern_roles` (an
  application role the owner assigns to e-groups in the Application Portal; CERN expands
  nested e-groups), or `groups` once CERN has enabled that claim for the application
  (ServiceNow request; CERN discourages it as the default). One value links at most one
  group per org (unique `(org_id, idp_provider, idp_value)`).
- **Sync.** At every sign-in through that provider, in the sign-in transaction after the
  P4-IDP-1 role sync, the user's memberships in that provider's IdP-linked groups of each
  org they are an **active member** of are set to exactly the groups whose value the
  validated claim contains: added if present, removed if absent (an absent claim removes
  them all). Only rows with `group_members.source = idp` of that provider are touched;
  values that link no group are ignored and stored nowhere. **When a person's
  membership of an org leaves `active`** (suspended or removed, in the dashboard or by
  SCIM), their IdP-linked rows in that org are deleted in the same transaction, and the
  next sign-in after reactivation adds back whatever the claim then says: access the IdP
  took away during a suspension never returns with the reactivation (SEC-71; REVIEW-5,
  FIX-083). A provider whose `group_claim` is removed from the configuration syncs as an
  absent claim. A group never creates or
  reactivates an org membership (above): a user who is not a member of the org gains
  nothing from the claim, and admission stays with invitations, JIT, D-143 role claims,
  placement and SCIM.
- **Read-only.** Members of an IdP-linked group cannot be added or removed by hand, by
  API or by SCIM (409 `managed_by_idp`); the dashboard shows "Members come from your
  sign-in provider (claim value `…`)" and each member's last sync time. Name, description,
  workspace bindings, org-role mapping and policy use are edited like any group.
- **Freshness (residual risk, `17`).** Membership changes at the IdP take effect at the
  person's next sign-in through that provider. Until then an Aurora session keeps the
  memberships of its last sign-in, bounded by `auth.session_absolute_secs` and the idle
  limit. An org that needs faster removal shortens those limits; a live query of the IdP
  (CERN Authorization Service API) is not part of phase 4.
- **Unlinking.** Changing a group's `source` is not allowed; deleting an IdP-linked group
  deletes it like any group. Removing `group_claim` from a provider's configuration stops
  the sync and leaves existing members until an admin deletes the group (startup logs a
  warning naming the orphaned groups).
- **Audit.** `group.member_add|remove` with `actor_kind = service`, `actor_label =
  idp:<provider>`, the claim value, in the sign-in transaction (security-critical).

## SCIM (phase 4, P4-GRP-2, D-132, D-135, SEC-72, SEC-73)

SCIM 2.0 (RFC 7643/7644) lets an organization's IdP (Entra ID, Okta, Keycloak) create,
update, deactivate and delete **users** and maintain **groups** and their membership
without anyone touching Aurora. It feeds `§Groups` and org membership and **nothing
else**. Wire details (endpoints, filter and PATCH subset, errors): `16 §W15`; tables:
`03 §Planned changes`; routes: `05`. Invisible until used: the surface answers 404
unless the operator sets `scim.enabled = true` *and* an org owner enables SCIM for the
org and creates a token.

**Enabling and tokens.**

- Instance `scim.enabled` (default `false`; `15`): while false `/scim/v2/**` answers 404
  to everyone and the dashboard shows no SCIM page. An optional instance allow-list
  `scim.allowed_email_domains` caps which domains any org may provision.
- An org **owner** enables SCIM (`organizations.settings.scim = { enabled, email_domains,
  exclusive }`, no new column) and **must list at least one e-mail domain**
  (`email_domains`, exact and case-insensitive like D-118, no subdomains, each also in
  the instance allow-list when that is set). Every provisioned e-mail must be in one of
  them (*Users*). Only an org owner creates or revokes tokens and changes these
  settings: the token can provision accounts and fill groups, so it is as sensitive as
  an owner role. Session only; creating a token and enabling SCIM need a recent
  re-authentication (412 `reauth_required`, as account deletion, `§OIDC`).
- **Token** `aud_` + 43 base64url characters (`00`), shown once, stored hashed
  (`scim_tokens.token_hash`) with a display prefix, per organization (at most 5 active),
  with an expiry (default 365 days, maximum `scim.token_max_days` = 730; no "never"), a
  name, `last_used_at` (churn column), `revoked_at`. It authorizes `/scim/v2/**` of its
  **one** org and is refused with 401 everywhere else (a request that presents an `aud_`
  bearer commits to it: no fall-through to cookies, and the SCIM routes ignore cookies and
  `Origin` entirely). A token is **dead** when revoked, expired, its org's SCIM switch is
  off, the instance switch is off, or the org no longer exists; every failure is the same
  401 SCIM error (no oracle). Creating, revoking and expiring are audited
  (`scim_token.create|revoke`, critical; `scim.update` for the settings, critical).

**Scope of authority (the whole list).** A SCIM token may: create, update, suspend,
reactivate and remove **members it manages** (below) and fill, rename, and delete
**SCIM groups** of its org. It may **not**: set or read any role (there is no `roles`
attribute; SCIM-created members are always org role `member`), create or change an
`owner` (owners are never touched: every SCIM mutation of an owner is refused 403, see
below), touch **manual** groups, any org-role mapping or workspace binding (those stay
human decisions in the dashboard), touch another org, read or write any other Aurora
data, or change a global attribute of a person who has signed in (*Users*).

**Users.** A SCIM user is a **link** `scim_users` (`03`) between the org and a global
`users` row, with its own SCIM id (not the Aurora user id, which would leak across orgs).
`POST /Users` (all of it in one transaction, critical audit):

1. The Aurora e-mail is the primary (else first) `emails[].value`, else `userName`;
   lower-cased, syntactically valid, ≤ 254 chars, and **its domain must be in the org's
   `email_domains`** — otherwise 400 `invalidValue`. `externalId` ≤ 256 chars. A second
   user with the same e-mail or `externalId` in the org → 409 `uniqueness`.
2. Resolve the global user **by e-mail only**: (a) none → create a **pre-provisioned**
   user: `email_verified_at = NULL` (Aurora has verified nothing; SCIM is the org's claim,
   not proof), no identity, no password, `display_name` from SCIM (sanitized, truncated to
   80); it can do nothing until its first OIDC sign-in binds it (*OIDC step 3*); (b) a user
   exists → link it and leave the `users` row **untouched**; (c) an unremoved link of this
   org exists → 409.
3. Create the `org_members` row (role `member`; `active: false` → `suspended`), or adopt an
   existing membership as it is (an invited admin keeps their role). Its status depends on
   who the user is (D-175, REVIEW-5; implemented by FIX-082): for a **new pre-provisioned
   user** (2a) it is `active`, and inert until the first sign-in; for an **existing account
   that is not yet a member of this org** (2b) it is **`pending`** — it counts against the
   seat cap and can be placed in SCIM groups, but confers no access of any kind (every
   check treats `pending` as a non-member) until that person's **next verified OIDC
   sign-in**, which makes it `active` in the sign-in transaction and writes the
   `member.add` audit row (`via: "scim"`). The directory's assertion prepares a membership;
   only the person's own sign-in grants it (SEC-73). `active: false` or `DELETE` before
   that sign-in removes the pending membership. SCIM responses carry the IdP's own
   `active` value throughout. The seat
   cap `max_members` applies (403 with `detail` `member_limit_reached`). Group membership
   comes from SCIM groups, never from the user resource. No e-mail is sent, no session or
   key is created, a pending invitation is left alone.

Updates (`PUT`, `PATCH`): `userName`/`emails` re-validated as above; `displayName` and
`name.*` are stored on the link; **Aurora never changes `users.email` or
`users.display_name` of a user who has an identity or a verified e-mail** (the org's IdP
must not be able to rewrite a global account that other orgs and other IdPs use), and
changes them only for a still-unbound pre-provisioned user. `active` drives *Deprovisioning*.
An attempt to change the e-mail of a signed-in user is accepted into the link
(`userName`) and ignored for the account (`200`, the response shows the link's value).
**An update never moves a link to another account** (D-167, FIX-062): the link's user is
fixed, so deprovisioning always reaches the person who signed in (an owner's link is treated
the same way). The one exception is a still-unbound pre-provisioned user (no identity, e-mail
unverified): a new e-mail that is free globally renames it in place **only when no other org
has a membership or a SCIM link (any state) for it**; otherwise — the address belongs to another
user, or the placeholder is shared with another org — the link moves exactly as a `POST` of the
new address would link it (to that user, or to a new pre-provisioned user; membership adopted
or inserted under the seat cap), the placeholder's membership in this org is removed (its SCIM
group memberships follow the link) and the placeholder's `users` row stays as it is (SCIM never
deletes a user). A `POST` that revives a removed link keeps the link's person (the e-mail is not
re-resolved).

**Members it manages.** SCIM may act on a membership only through a link. Where a person was
already a member before SCIM (invited, JIT) and the IdP later `POST`s them, the membership
is **adopted** with its role, and `owner` memberships can be linked (so they can be placed in
SCIM groups) but are **never mutated by SCIM**: `PATCH active=false`, `PUT`, `DELETE` of an
owner answer 403 (`detail`: "Organization owners are managed in Aurora"), so a compromised
IdP cannot lock out the owners or touch the last-owner invariant. Admins (direct role) can be
deprovisioned: that is the point of offboarding.

**Groups** (`/Groups`, `source = scim`). `displayName` is the group name (unique per org, also
against manual groups: 409 `uniqueness`), `externalId` is stored, `members[].value` are SCIM
user ids of **this org's links** (an unknown id, a removed link, or a user of another org →
400 `invalidValue`); a suspended link may be a member (it derives nothing, `§Groups`).
Replacing or patching members writes one critical `group_member.add|remove` row per change,
within the limits of `02 §Limits` (`GROUP_MEMBERS_MAX`, `GROUPS_PER_USER`; exceeding →
400 `invalidValue` / `detail`). A SCIM group **grants nothing until an org admin maps or
binds it** in the dashboard; renaming and deleting it in the dashboard is refused while SCIM
is enabled (`group_managed_by_scim`), the mapping and the bindings are editable. Deleting a
group in the IdP (`DELETE /Groups/{id}`) deletes it in Aurora with its bindings, which is how a
compromised IdP can remove access — an availability, not a confidentiality, risk, and audited.

**Deprovisioning** (D-135). Link states: `active` → `suspended` (IdP `active=false`) →
`removed` (`DELETE /Users/{id}`); `POST` of a removed user re-provisions it. All in one
transaction with critical audit rows (`member.update|remove`, `metadata.via = "scim"`), then
invalidation (`org` and `user` scopes, `aurora_auth_revoked` kinds `access` and `session`):

| Effect | Suspend | Remove |
|--------|---------|--------|
| Org membership | `suspended` (role kept; "non-member for every check", *Roles*) | row deleted; workspace bindings and group memberships go with it |
| Sessions | revoked **only if the user has no other active membership** in any org (they would be signed into nothing); otherwise kept: the org's data is already 404 | same |
| API keys (`auk_`, `auo_`) of this org owned by the user | **revoked** (`revoked_at`; not merely dormant: a reactivation never revives a key that may have leaked); keys of other orgs untouched | same |
| Running work | turns spoken by the user in this org and `joining` follow-ups: queued ones `aborted`, live ones get `turn.abort` and end `aborted` with `error_code = user_deprovisioned`; their speaker binding is gone from the next gateway request | same |
| Schedules, webhooks, peer tasks they created | disabled at their next fire by the creator re-check (`disabled_reason = authorization`, notice), anchored ones survive (RFE-021); directory sync cannot reassign, so the dashboard lists them in the org's offboarding view and a notice tells the owners | same |
| BYO agents they created, agents and workspaces | nothing is deleted or rotated (agents belong to workspaces, a BYO token rotation would break a live service on an IdP hiccup); the audit row lists the BYO agents whose token the person saw (`agents.created_by`) and the dashboard shows "N BYO agents created by <user>: regenerate their tokens" to the owners | same |
| Direct conversations, authored content | kept; owners still read them (`§The access law`); the content keeps its label | kept |
| Pending invitations of this org for the link's `userName` or the account's e-mail | cancelled | cancelled |
| JIT / invitation / sign-in re-entry | refused for this org while `suspended` or `removed` (*OIDC* note) | same |
| Reactivation (`active=true`) | membership `active` again **only if SCIM suspended it** (`scim_users.suspended_by_scim`): a membership an org admin suspended by hand is never reactivated by the IdP; revoked keys stay revoked, aborted turns stay aborted, disabled automations stay disabled (owners re-enable) | `POST` re-adds a member |

SCIM never deletes a `users` row or an account and never touches other orgs: a person in two
orgs deprovisioned by one remains a user of the other.

**`exclusive` mode** (org setting, default `false`). When on, JIT, invitation acceptance,
new-user placement and IdP role-claim sync for this org are refused for people without an
`active` SCIM link: the IdP is then the only way in. It never changes existing members.

**Admission gate and seat cap (D-179, FIX-080).** *Every* path that creates an `org_members`
row — invitation acceptance, JIT, new-user placement and IdP role-claim sync — passes the
admission gate of that org (a `removed` or `suspended` link refuses, `exclusive` without an
`active` link refuses, SEC-73) and then its seat cap (`max_members`), decided under the org
lock the SCIM writers take, so a directory removal that has committed is always seen and one
that has not waits. A refusal writes a denied `member.add` audit row (`metadata.path` =
`invitation` | `jit` | `placement` | `idp_sync`, `metadata.reason` = `scim_removed` |
`scim_suspended` | `scim_exclusive` | `member_limit_reached`) **on its own connection**, so it
survives a rolled-back request or an abandoned savepoint. A refused role claim, placement or
seat-capped JIT never fails the sign-in itself (the person signs in without that
membership); a JIT refused by the directory gate (`removed`, `suspended`, `exclusive`) still
fails it with `scim_refused`, and a refused invitation answers 403 `scim_refused` (seat cap:
422 `member_limit_reached`) and stays pending.

**Rate limits and size.** Per token `SCIM_RATE` (600 requests/min, burst 120) through the
`CounterStore` (class `management`, `05 §Conventions`), request bodies ≤ `SCIM_BODY_MAX` (1 MiB),
`SCIM_MAX_PATCH_OPS` (100) operations per PATCH, `SCIM_MAX_MEMBERS_PER_REQUEST` (1 000),
list pages ≤ `SCIM_PAGE_MAX` (100, default 50) — a list pages **in SQL** (count plus `OFFSET`/`LIMIT`), with no bound on the size of the directory (`totalResults` is the true count; FIX-081). Requests with a rejected `aud_` bearer fall
under the existing 30/min/IP rejected-bearer limit (class `security`, never fails open). A refusal
is `429` in SCIM error format with `retry-after`.

**Audit.** `scim_token.create|revoke`, `scim.update`, and the membership and group rows above;
SCIM rows have `actor_kind = service`, `actor_label = scim:<token name>`, `source = api`,
`metadata.via = "scim"`. The dashboard shows "Directory sync" status (last request, counts,
last 20 sanitized errors, token expiry, a warning 14 days before expiry).

## Preview access (phase 4, P4-PRV-2, D-146, SEC-81, SEC-85)

How a browser proves, on a preview hostname (`<label>.<previews.domain>`, `11 §Port previews`), that
a person with a live dashboard session who can use the workspace is asking, **without a
credential of the dashboard ever reaching a preview** and without a preview ever being able to act
as the dashboard. Applies only when `previews.enabled`; otherwise none of it exists.

**Origin isolation (the rules that make previews safe to host next to the dashboard).**

1. **Separate origins, enforced at startup** (`15 §Port previews`): `previews.domain` is never the
   dashboard host or a parent or child of it, of any `auth.trusted_origins` entry, or of the
   gateway host; and no `auth.trusted_origins` entry may sit under `previews.domain` (a preview
   origin is never a trusted origin for the CSRF `Origin` check above).
2. **The session cookie is host-only.** `aurora_session` (and `__Host-aurora_oidc`,
   `__Host-aurora_oauth`) never carry a `Domain` attribute: that has always been the rule
   (`§Sessions`) and is now an invariant tested by SEC-81. A browser therefore never sends
   them to a preview host, whatever the DNS layout. The API never sets a cookie for any host
   under `previews.domain`, and the relay never sets, reads or forwards `aurora_session`.
3. **Previews get their own credential**, valid for one preview host only: the cookie
   `__Host-aurora_preview` (prefix `__Host-`: `Secure`, `Path=/`, **no `Domain`**, so it
   cannot be set by, or shared with, another preview hostname or a parent domain), `HttpOnly`,
   `SameSite=Lax`, value `auv_` + 43 characters, stored hashed in `preview_sessions` (`03`), bound
   to **one share** and **one dashboard session**. It is not accepted anywhere but the relay and
   the relay **removes it from the `Cookie` header** before forwarding to the app: the app inside
   the sandbox never sees an Aurora credential of any kind.
4. **The relay refuses cross-site use of a preview**: it answers 403 `preview_cross_site` to a
   request whose `Sec-Fetch-Site` is `same-site` or `cross-site` unless it is a top-level
   navigation (`Sec-Fetch-Mode: navigate`, `Sec-Fetch-Dest: document`, method `GET`), so another
   preview (a sibling under `previews.domain` is *same-site*), the dashboard or any other web page cannot make
   credentialed subrequests, form posts or fetches to a preview. A WebSocket upgrade must carry
   an `Origin` equal to the preview's own origin. A request without Fetch Metadata headers is
   accepted only if it carries no `Origin` or one equal to the preview's own origin
   (non-browser callers hold the cookie anyway). The relay also strips upstream `Set-Cookie`
   attributes `Domain` (the cookie is made host-only), and drops upstream cookies named
   `__Host-aurora_*` or `aurora_*`, so an app cannot toss cookies onto its siblings or shadow the
   preview credential.
5. **The dashboard side is tightened, never loosened** (only when `previews.enabled`): a
   cookie-authenticated API request whose `Sec-Fetch-Site` is `same-site` or `cross-site` is
   refused with 403 `csrf_rejected` unless it is a top-level `GET` navigation (the existing
   `Origin` rule for unsafe methods stays), so a page served from a preview cannot read or change
   anything through the visitor's dashboard session even where the browser would send the cookie
   (a preview domain that is *same-site* with the dashboard). The API sends no CORS headers (unchanged).

**Authorization flow** (grant exchange; the OAuth `state` pattern, login-CSRF safe):

1. The browser opens `https://<label>.<previews.domain>/<path>`. The relay finds no valid
   `__Host-aurora_preview`, so it answers (for a top-level `GET`/`HEAD` navigation only; any other
   request gets 401 `preview_login_required` and no redirect) `302` to
   `<external_url>/v1/previews/authorize?host=<label>&state=<nonce>`, after setting
   `__Host-aurora_pstate` (`HttpOnly`, `Secure`, `SameSite=Lax`, `Max-Age=300`, `Path=/`, value `nonce.base64url(path)`,
   the path limited to 2 KiB, starting with `/` but not `//`, never under `/_aurora/`).
2. `GET /v1/previews/authorize` is a **session-only** route on the dashboard origin. Without a
   session it redirects to sign-in with a same-origin `return_to` of itself. With one it checks, in
   this order and answering the **same 404 page** for every failure (unknown label, previews
   off, org off, share paused, no access: the existence of a share is never revealed to someone
   who cannot use its workspace): the label names a share, previews are on for instance and org,
   the user `can_use(workspace)`, the share is `active`. A frozen agent answers the 423 page. On success it mints a
   **grant**: AEAD-sealed (`08 §4` key ring, purpose `preview-grant`) `{ jti, share_id, user_id,
   session_id, nonce, exp = now + PREVIEW_GRANT_TTL (60 s) }`, and redirects to
   `https://<label>.<previews.domain>/_aurora/grant?g=<grant>` — a URL **constructed from the
   share's own label**, never from the request (no open redirect, no host injection), https only.
3. The relay's `/_aurora/grant` requires the `__Host-aurora_pstate` cookie, calls the API
   (`POST /v1/relay/grants/redeem`, `16 §W17`) with `{ grant, label, nonce }` (`nonce` from the
   cookie) and the API verifies: seal and expiry, `label` matches the share, `nonce` equal, the
   grant **unused** (`jti` insert-on-conflict into `preview_grant_uses`, single use), the dashboard
   session still live, the user still `can_use`. It creates the `preview_sessions` row and returns
   the `auv_` token **once** (never stored in clear). The relay sets the preview cookie, deletes the
   state cookie and redirects (`302`) to the stored path. A grant replayed, expired, for another host, or
   redeemed without the matching state cookie (a *login-CSRF* attempt: an attacker feeding a victim
   a grant for the attacker's session) fails with 400 `grant_invalid` and no cookie.
4. Every later request carries only the preview cookie. The relay calls `resolve` with its
   SHA-256 (never the token); the answer is cached at most `PREVIEW_AUTH_CACHE` (15 s; 5 s for
   a negative answer). The API recomputes access on each uncached resolve: session row live,
   not past `PREVIEW_SESSION_TTL` (8 h absolute) or `PREVIEW_SESSION_IDLE` (1 h without use), user
   still an active member with `can_use`, share active, agent not frozen, previews on. Failing any →
   `login` (when the session expired but a dashboard session may exist: the flow restarts silently
   from step 1 and needs no click) or `deny`.
5. **Revocation**: signing out or revoking the dashboard session deletes that session's preview
   sessions (cascade); suspending or removing a member, losing `can_use` (including through a
   group, `§Groups`), deleting the share, pausing it, freezing (`§Freeze`) or turning previews off
   all end access at the next resolve; **open streams** (WebSockets, long polls) are
   re-validated by the relay every `PREVIEW_AUTH_CACHE` through `POST /v1/relay/validate` and
   closed with 4403 when no longer allowed. Worst-case latency 15 s + one round trip.
6. **API keys, agent credentials, connectors and instance admins as such have no preview access**:
   only a user session whose user passes `can_use(workspace)`. The preview credential grants
   nothing on any other route (it is accepted by no route of the API).

**Audit.** `preview.session.create` (first redemption of a session: actor, share, label; best
effort, not security-critical), `preview.session.revoked` is not written (it is a consequence).
Denials are metrics only (`aurora_relay_requests_total{outcome}`), never audit rows (volume,
unauthenticated sources).

## Freeze (phase 2, RFE-006, D-50)

An emergency, **reversible** switch that stops an agent, every agent of a
workspace, or every agent of an organization from acting, while keeping every
byte of state for forensics. It exists because the phase-0 options (delete the
agent, regenerate a token, restart, hand-edit policy) are destructive or partial.

**State.** `agents`, `workspaces` and `organizations` each gain `frozen_at`,
`frozen_by_user_id`, `frozen_reason` (1..500 chars, sanitized, required) and
`frozen_authority` (`workspace_owner` < `org_admin` < `instance_admin`) — see
`03 §Planned changes`. An agent is **effectively frozen** when it, its workspace
or its organization is frozen. The flags are independent: unfreezing an agent does
not unfreeze its frozen workspace.

**Who.** Freezing an agent or a workspace needs `can_own` (a workspace owner,
or an org owner/admin, who are implicit owners): the person closest to the agent
decides to pull the plug, and freezing is safe to do fast. Freezing an
**organization** needs org `owner` or an instance admin (an instance admin
receives no org data access by this: the switch is an operator kill-switch for
an incident such as a compromised org account, and lifts only as described
next). **Lifting** requires at least the authority that froze (`frozen_authority`
= the caller's highest applicable authority at freeze time): a workspace owner
cannot unfreeze what an org admin froze, an org owner cannot lift an
instance-admin freeze. All of it is idempotent (freezing a frozen scope keeps the
original record, returns 200).

**Effects** (all effective at the next gateway cache refresh ≤ 5 s, by
`aurora_gateway_invalidate` scope `agent`/`workspace`/`org`, except where noted):

1. **Gateway** refuses every credential of a frozen agent (403 `frozen`,
   `details.level`), closes its live tunnels/WebSockets, and answers held
   requests 403 `frozen` (their approvals become `abandoned`) — `06`, `07`.
2. **Turns**: no turn starts. Creating one (any door) is refused: API 423
   `frozen`; channel doors reply once per (space/thread, person) per
   `FREEZE_REPLY_INTERVAL` (10 min) "*<Agent> is paused by an administrator.*"
   and create nothing (`13 §5.1`); peer messaging says the same. `queued` and
   `joining` rows are `aborted` (`error_code = frozen`); `dispatched`/`running`
   turns get `turn.abort` and end `aborted` (`error_code = frozen`).
3. **Due work**: the start, dispatch, steer, schedule and watch arms skip
   effectively frozen agents (`05 §The due-work engine`); a running sandbox is
   **parked** (`stop_reason = frozen`: container removed, home volume kept for
   forensics, processes `parked`, armed watches kept but never fired). Reconcile
   never destroys a frozen agent's home.
4. **Webhooks** answer 410 `frozen` and record a `webhook_deliveries` row
   `paused`; **schedules** are not fired and not disabled — `next_fire_at`
   advances (missed fires coalesce, no catch-up burst) and a `schedule_runs` row
   `skipped_frozen` is written; **presences** reply "paused" as above;
   BYO `GET /v1/agent/container-config` and `POST /v1/agent/context-credentials`
   answer 423 `frozen`.
5. **Not frozen**: the management plane. Owners and admins can still read
   transcripts, activity, audit, change policy/grants, rotate tokens, edit the
   agent and unfreeze — only actions that *start agent work* are refused.
   Credentials are **not revoked** (so unfreezing needs no redistribution);
   boot/context credentials end with their boot/turn as usual.
6. **Audit and notice**: `agent.freeze|unfreeze`, `workspace.freeze|unfreeze`,
   `org.freeze|unfreeze` are in the security-critical set (written in the same
   transaction, *Audit*). Each effectively frozen agent's direct conversations get
   a platform `notice` (`code: "agent_frozen"`), and the dashboard shows a banner
   at the scope.
   (phase 4, P4-PRV-1/2) **Previews** of an effectively frozen agent answer 423 `frozen`, never
   wake or start anything, and their open streams close within `PREVIEW_AUTH_CACHE`; the shares
   and preview sessions are kept (`11 §Port previews`, `§Preview access`).
7. **Reversal**: unfreezing clears the flags, invalidates the gateway and wakes due
   work. Nothing is replayed: aborted turns stay aborted, schedules resume at
   their next occurrence, sandboxes start on demand.

## Invitations

- Created by org owners/admins: email, role (`admin`|`member`), optional
  workspace bindings (role `member`; phase 2, RFE-012: each binding may instead be
  `approver`, never `owner`). Email contains `<external>/join?token=aui_…`.
- Accepting requires a signed-in user whose **verified** email equals the
  invitation email (case-insensitive). The membership and bindings are created;
  the invitation becomes `accepted`.
- Pending invitations for the same (org, email) are unique; re-inviting rotates
  the token.
- Seat cap: if `organizations.settings.max_members` is set, active members +
  pending invitations must stay ≤ cap (422 `member_limit_reached`).

## Deletion semantics

- **Conversation deletion** (phase 2, RFE-010) is a separate, explicit operation
  (`10`); deleting a workspace or agent still cascades as below.
- **Account deletion** (session-only, requires password or recent OIDC sign-in
  within 10 min; phase 1: recent OIDC re-authentication only, RFE-004): refused while the user is the last owner of any org
  (409 `last_owner`). Removes memberships, bindings, (phase 4) group memberships, sessions, keys, identities;
  conversations of kind `direct` with that user are deleted; authored content
  keeps a denormalized label.
- **Workspace deletion**: cascades; agents' sandboxes are destroyed through
  runner reconciliation (`11`); channel presences on platforms are
  deprovisioned through connector operations enqueued before the delete.
- **Org deletion**: owner only, typed confirmation; hand-written service deletes
  children in dependency order, enqueues connector deprovisioning, then deletes
  the org. Ledgers (`audit_log`, `request_log`, `runtime_*`) remain.
- **Deprovision ordering and content (phase 3, RFE-024).** For agent, workspace and
  org deletion the connector `presence.deprovision` operations are **inserted in
  the same transaction as the delete, before the cascade**, and are
  **self-contained**: each snapshots the presence's `platform_user_ref`, handle, the
  integration's tenant and non-secret config, and a sealed, op-bound copy of the
  credentials needed (`13 §4.2`, *Self-contained deprovision operations*), with
  `integration_id = NULL` so neither the presence nor the integration (nor the
  org) deleting can take the op with it. A failed or impossible deprovision
  (connector down beyond 24 h, credentials revoked) leaves the bot on the
  platform: an accepted, documented residual with an audit row
  (`presence.deprovision_failed`) so operators can remove it by hand.

## Audit

Every state-changing management action writes an `audit_log` row. Actions are
dotted names `<entity>.<verb>`. Secret values, tokens and credential JSON are
never written to `metadata`.

**Two classes (phase 2, RFE-008, D-52).** Phase 0: every audit write was
best-effort and never failed the action. That is right for high-volume, low-stakes
rows and wrong for the actions SEC-32 exists for: a database hiccup — or an
attacker timing one — must not silently lose the trail of a policy publish or a
role change. From phase 2 actions are classified:

- **Security-critical**: the audit row is written **in the same database
  transaction as the change** (same `Transaction`, committed or rolled back
  together). If the audit insert fails, the whole action fails and nothing is
  changed (503 `audit_unavailable`, fail closed). The service function that owns a
  critical change commits it only through `audit::commit_critical` (which writes
  the row with `insert_tx`); the audit coverage test scans the sources, so a
  critical action written through the best-effort writer or outside a
  transactional writer fails the build's tests (D-83).
- **Best-effort** (everything else): written after the change, failures logged and
  counted (`aurora_audit_dropped_total`), never failing the action — as in phase 0.

**The security-critical set** (defined once as `SECURITY_CRITICAL_ACTIONS` in
`aurora-core` and listed here; `audit_log.critical` is set for these rows,
`03 §audit_log`). The table names the actions descriptively; the emitted action
strings are the ones the API already used and are listed in `SECURITY_CRITICAL_ACTIONS`
(D-83): `grant.put\|delete` is emitted as `grant.create\|delete`, `org_member.*` as
`member.add\|update\|remove` (role changes and ownership hand-over are `member.update`),
`workspace_access.put\|delete` as `workspace_access.set\|remove`, `runner.*` as
`admin.runner_create\|rotate\|delete`, `connector.*` as `admin.connector_create\|rotate\|approve\|delete` (phase 4, P4-MCON: managed create and PATCH are `connector.created` and `connector.updated`, field names only),
`secret.value_replace` as `secret.value_update`, `trusted_key.add\|remove` as `trusted_key.create\|revoke`,
`reach.decide` as `reach_grant.decide`, `agent_link.decide` as `agent_link.policy`,
`app_availability.update` as `org.app_availability.update`, `host_claim.decide` as
`plugin.host_claim.decide`, `user.instance_admin_grant\|revoke` and `user.disable\|enable` as
`admin.user_update`, `approval.batch_decide` as one `approval.decide` row per approval
(`batch: true`); also critical: `invitation.accept` (including first sign-in),
`connection.reconnect`, `cli_login.consume`, `member.add` (SSO provisioning):

| Area | Actions |
|------|---------|
| Keys and credentials | `api_key.create`, `api_key.revoke` (personal, workspace, org), `agent.token_regenerate`, `agent.byo_settings.update` (source CIDRs, require-context; RFE-025), `workspace.byo_posture.update` (RFE-025), `cli_login.approve`, `secret.create`, `secret.update`, `secret.value_replace`, `secret.delete`, `secret.accept`, `connection.create`, `connection.update`, `connection.delete`, `oauth_client.create\|update\|delete`, `app_instance.create\|update\|delete` (D-74), `runner.provision\|token\|delete`, `connector.provision\|token\|approve\|delete` |
| Grants | `grant.put`, `grant.delete` (connection and secret grants) |
| Policy | `policy.publish`, `policy.rollback` (org and workspace) |
| Plugins and registry | `plugin.approve\|reject\|disable\|enable\|uninstall`, `trusted_key.add\|remove`, `host_claim.decide` |
| Approval decisions | `approval.decide`, `approval.batch_decide`, `action_approval.decide`, `reach.decide`, `agent_link.decide` (whatever the door: dashboard, API, channel click) |
| Roles and membership | `org_member.add\|role_change\|suspend\|reactivate\|remove`, `org.transfer_ownership`, `invitation.create\|accept\|cancel`, `workspace_access.put\|delete`, `user.instance_admin_grant\|revoke`, `user.disable\|enable`, `user.delete` |
| Previews (phase 4, P4-PRV-1) | `preview.share.create\|update\|delete`, `org.previews.update` (emitted as written; `11 §Port previews`) |
| Skill library (phase 4, P4-GSL) | `skill_library.create\|update\|delete`, `skill.library.add`, `skill.library.update_apply`, `skill.library.update_reject`, `skill.library.detach` (emitted as written; the proposal row `skill.library.update_proposed` is a best-effort system row; `10 §Skills`) |
| Freeze (RFE-006) | `agent.freeze\|unfreeze`, `workspace.freeze\|unfreeze`, `org.freeze\|unfreeze` |
| Groups (phase 4, P4-GRP-1) | `group.create\|update\|delete`, `group_member.add\|remove`, `group_access.set\|remove` (emitted as written; `§Groups`) |
| SCIM (phase 4, P4-GRP-2) | `scim_token.create\|revoke`, `scim.update`; membership and group changes made by the IdP are the ordinary `member.add\|update\|remove` and `group*` rows with `metadata.via = "scim"` (`§SCIM`) |
| Governance and tenant settings | `governance.update`, `app_availability.update`, `org.update`, `org.delete`, `workspace.delete`, `retention.update` (RFE-010/015) |
| Channels | `channel_integration.create\|update\|delete\|autolink_enable\|autolink_disable` (RFE-001), `user_link.create\|delete` |
| Data erasure and reassignment | `conversation.delete`, `transcript_event.redact`, `memory.redact`, `automation.reassign` (RFE-021) |

Critical rows record `metadata.change` — the changed field names and, where
non-secret, old → new values (e.g. `role: member → admin`) — and use `outcome =
success`. Attempts refused by authorization (`outcome = denied`) are written
best-effort (there is no transaction to join). Rows for the gateway's own writes
(`approval.expire`, `approval.abandon`) stay best-effort system rows. Everything
not listed — draft policy edits, agent and brief edits, memory and skill writes,
schedules, webhooks, conversation reads, `conversation.owner_view`,
`agent.sandbox_retry` (FIX-029: retrying a failed sandbox start, metadata: the
previous `failure_code`; the start failure itself is not an audited action, the
`sandbox_start_failures` row is its record), `agent.terminal.open` (P4-TERM:
opening a web-terminal session, metadata: agent and workspace; the terminal's
bytes are never recorded anywhere, the `terminal_stream_grants` row is its
record) — is
best-effort.
