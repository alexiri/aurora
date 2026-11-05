# 04 — Identity, Tenancy and Authorization

Phase: **0**. Tables: `03 §1`.

## Tenancy model

```
Instance ── instance admins (user flag) · plugins · trusted keys · connectors · host-claim decisions
   └── Organization ── members (role, status) · invitations · org policy · org secrets/connections/OAuth clients
          │             channel integrations · org skills · app availability · org API keys
          └── Workspace ── agents · secrets · connections · grants · workspace policy · API keys
                            access bindings (user → owner|member) · timezone · memory language
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
| `open` | Anyone may register (email+password or any configured OIDC provider). A new user gets **their own organization** (`<Name>'s org`) with a default workspace. Joining another org requires an invitation. |
| `invite_only` (default) | Registration only by accepting an invitation, or for addresses in `auth.instance_admins`. |
| `sso_jit` | Only OIDC sign-ins; a provider's `jit` block decides which org/role/workspaces the user joins on first sign-in. Users without a matching JIT rule and without an invitation are refused (`signup_closed`). |

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

## Instance admins

- `users.is_instance_admin` grants deployment-level powers: plugin packages,
  trusted publisher keys, host-claim decisions, connectors (provision tokens,
  approve manifests), runners (provision tokens), instance overview.
- Bootstrapped by `auth.instance_admins` (list of emails): on every sign-in, a
  user whose *verified* email is in the list gets the flag; removing an email
  from the list does **not** revoke (use `aurora-api admin revoke <email>`).
- `aurora-api bootstrap-admin --email <e>` creates a verified user with a
  printed one-time password-set link (for first install without SMTP).
- Instance admin is **not** an org role: it grants no access to org data.

## Authentication

### Email and password

- Passwords: argon2id (m=19 MiB, t=2, p=1), min 12 chars, max 256; checked
  against a bundled top-10k common-password list.
- Email verification: `aui_` token, 24 h. Unverified users may sign in but
  cannot accept invitations, be auto-linked to chat platforms, or be bootstrap
  admins.
- Password reset: `aui_` token, 1 h; using it revokes all sessions.
- Email delivery: SMTP (`email.smtp_*`). When SMTP is not configured, emails
  are written to the API log at `INFO` with the link (dev mode) and the
  dashboard says so.

### OIDC (SSO)

Configured per provider (`auth.oidc_providers[]`, see `15`): `id`,
`display_name`, `issuer`, `client_id`, `client_secret` (env reference),
`scopes` (default `openid email profile`), `require_email_verified` (default
`true`), `allowed_email_domains` (optional), `jit` (optional:
`{ org_slug, role: member|admin, workspace_slugs: [] }`).

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
   email. Else create a user if the sign-up mode allows (or JIT applies).
4. JIT: if the provider has a `jit` block and the user is not a member of that
   org, add membership with the configured role and workspace bindings.
5. Create a session; 302 to `return_to`.

### Sessions

- Cookie `aurora_session` = `aus_` token; `HttpOnly`, `SameSite=Lax`,
  `Path=/`, `Secure` when the external URL is https. Stored hashed in
  `sessions`.
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
  account, change password, sign out other sessions, accept invitation, create
  API keys, approve a CLI login.

### CLI login (device flow)

1. `POST /v1/auth/cli` → `{ device_code, user_code, verification_url, interval, expires_in }`.
2. The person opens `<external>/auth/cli?code=ABCD-EFGH`, signs in, chooses a
   workspace, approves.
3. CLI polls `POST /v1/auth/cli/token { device_code }` → `authorization_pending`
   | `{ api_key, workspace }` once (status → `consumed`). The key is a `user`
   workspace key named `CLI on <hostname>`.

### Rate limits (auth)

In-memory token buckets per API replica, keyed by client IP (from
`X-Forwarded-For` as **set** by the edge proxy; the edge MUST overwrite, not
append) and by email: sign-in 10/min/IP and 5/min/email; sign-up 5/h/IP;
reset 3/h/email; CLI token polling governed by `interval` (`slow_down` error).

## Authorization

### Roles

| Layer | Values | Notes |
|-------|--------|-------|
| Org role | `owner` > `admin` > `member` | Always enforced. |
| Membership status | `active`, `suspended` | Suspended = non-member for **every** check including API keys, channel clicks, schedules. |
| Workspace access | `owner`, `member` | Org owners and admins are implicit workspace owners of every workspace in their org. |

### The access law

`can_use(user, workspace)` is true iff: the workspace exists **and** the user
has an `active` membership in its org **and** (the user's org role is `owner` or
`admin` **or** a `workspace_access` row exists). `can_own(user, workspace)` is
the same with workspace role `owner` (or org owner/admin).

The access law is implemented once (`aurora-db::authz`) and used by every
surface: HTTP routes, channel speakers and clickers, approvals, schedule and
webhook fire-time checks, peer messaging.

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
| Workspace settings (name, timezone, memory language), access bindings, delete workspace | `can_own` |
| Workspace secrets, connections, grants, workspace policy (incl. the workspace uncredentialed default), workspace API keys | `can_own` |
| Workspace LLM keys | `can_own`, **and** org governance permits `workspace`-scoped keys for that endpoint, **and** the key's origin complies (`08 §11`) |
| Space bindings for an agent of the workspace | `can_own` |
| Create/edit/delete agents, brief, model, avatar; memory; skills; schedules; webhooks; restart sandbox | `can_use` |
| Chat with agents, read transcripts, upload attachments | `can_use` |
| Decide gateway approvals and action approvals for agents of the workspace | `can_use` |
| Read activity (request log) of the workspace | `can_use` |
| Plugins, trusted keys, host claims, connectors, runners | instance admin |

Org admins/owners can therefore always repair a workspace; members of a shared
workspace can operate its agents but not change what credentials they hold.

### Fire-time authority

Schedules, webhooks, peer tasks and reach decisions store their **creator**.
At every fire the access law is re-checked for that creator on the agent's
workspace; if it fails the item is disabled with `disabled_reason =
authorization` and the origin conversation receives a notice.

## Invitations

- Created by org owners/admins: email, role (`admin`|`member`), optional
  workspace bindings. Email contains `<external>/join?token=aui_…`.
- Accepting requires a signed-in user whose **verified** email equals the
  invitation email (case-insensitive). The membership and bindings are created;
  the invitation becomes `accepted`.
- Pending invitations for the same (org, email) are unique; re-inviting rotates
  the token.
- Seat cap: if `organizations.settings.max_members` is set, active members +
  pending invitations must stay ≤ cap (422 `member_limit_reached`).

## Deletion semantics

- **Account deletion** (session-only, requires password or recent OIDC sign-in
  within 10 min): refused while the user is the last owner of any org
  (409 `last_owner`). Removes memberships, bindings, sessions, keys, identities;
  conversations of kind `direct` with that user are deleted; authored content
  keeps a denormalized label.
- **Workspace deletion**: cascades; agents' sandboxes are destroyed through
  runner reconciliation (`11`); channel presences on platforms are
  deprovisioned through connector operations enqueued before the delete.
- **Org deletion**: owner only, typed confirmation; hand-written service deletes
  children in dependency order, enqueues connector deprovisioning, then deletes
  the org. Ledgers (`audit_log`, `request_log`, `runtime_*`) remain.

## Audit

Every state-changing management action writes an `audit_log` row (best-effort,
never failing the action). Actions are dotted names `<entity>.<verb>`. Secret
values, tokens and credential JSON are never written to `metadata`.
