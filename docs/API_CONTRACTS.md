# API contracts

Canonical interface between the Rust API (`services/aurora-api`) and the
Next.js dashboard (`web/`). **Routes, methods and auth columns are owned by
`05-control-plane-api.md §Route catalogue`**; this file does not repeat them.
It records (a) cross-cutting conventions gathered from `02`/`04`/`05`, and
(b) request/response **schemas** that `05` leaves implicit, defined per
milestone *before* backend and frontend work start in parallel (decision I-08).

Machine contract: the API generates OpenAPI with `utoipa`, served at
`GET /v1/openapi.json` and checked in as `web/openapi.json`. The dashboard
generates its types with `openapi-typescript` and calls the API only through
`openapi-fetch` (D-32). CI fails if the checked-in document drifts from the
handlers (`18 §Contract`).

## Conventions

- **Base URL**: same origin as the dashboard (`server.external_url`); the edge
  routes `/v1/*` to the API (D-31). No CORS. Browser calls are relative (`/v1/...`).
- **Versioning**: path prefix `/v1`. Breaking changes require `/v2`.
- **Authentication** (`05 §Authentication surfaces`):
  - Browser: session cookie (`aus_` token; `HttpOnly`, `SameSite=Lax`,
    `Secure` on https; name fixed in API-001). Unsafe methods (POST/PUT/PATCH/DELETE)
    with a cookie MUST carry an `Origin` equal to `server.external_url` or in
    `auth.trusted_origins` → otherwise 403 `csrf_rejected` (SEC-22, `04 §Sessions`).
  - API keys: `Authorization: Bearer auk_…` (workspace) / `auo_…` (org).
    Routes marked **SO** (session only) reject keys with 403 `session_required`.
  - Agent token (`aug_`), runner (`aur_`), connector (`aun_`): machine planes only.
- **Content type**: `application/json; charset=utf-8` both ways, except
  multipart uploads, SSE (`text/event-stream`) and file downloads.
- **Error envelope** (`02 §Error envelope`, D-35):
  `{ "error": { "code": "snake_case", "message": "…", "details": { … } } }`.
  Validation: 400 `invalid_request` with `details.fields = [{ "path": "/json/pointer", "message": "…" }]`.
  Status vocabulary: 400, 401 `unauthenticated`, 403 `forbidden` (or a more
  specific code), 404 `not_found`, 409 `conflict`/`stale_revision`/`already_decided`,
  410, 413 `too_large`, 422 domain-rule codes, 429 `rate_limited` + `Retry-After`,
  502/503/504. Resources the caller cannot see answer **404**, not 403
  (`05 §Request scoping`). Workspace key on another workspace → 403 `workspace_mismatch`.
- **IDs**: UUIDv7 strings (lowercase, hyphenated). Orgs/workspaces also have
  slugs `[a-z0-9][a-z0-9-]{1,47}`; agents have handles `[a-z][a-z0-9-]{0,39}`.
  `GET /v1/resolve?org=&workspace=` maps slugs → ids.
- **Timestamps**: RFC 3339 UTC with `Z` (e.g. `2026-11-05T21:58:00.123Z`); the
  database clock is truth. Nullable timestamps are JSON `null`, never omitted.
- **Nullability**: every documented field is always present; absent values are
  `null`. Clients must ignore unknown fields (additive evolution).
- **Pagination**: `?limit=` (default 50, max 200), opaque `?cursor=`; response
  `{ "items": [...], "next_cursor": "…" | null }`, ordered
  `created_at desc, id desc` unless documented.
- **Optimistic concurrency**: resources with `revision` accept `If-Match: <revision>`
  or a `revision` body field; stale → 409 `stale_revision`.
- **Long-poll**: `?wait=<secs>` capped at 30 s (`LONG_POLL_MAX`).
- **SSE**: durable events carry `id: <seq>`; keepalive comment every 15 s;
  resume with `Last-Event-ID` or `?after_seq=`.
- **Secrets are write-only** (SEC-12): no response ever contains a secret value
  or a token except the documented one-time reveal fields (`token`, `key`) at
  creation/regeneration.

## Endpoint schemas by milestone

Each section is written by the orchestrator (task `API-xxx`) before the
corresponding backend/frontend tasks start.

### M1 — identity, tenancy, API skeleton (API-001)

Routes and auth columns: `05 §Health and instance`, `§Auth and account`,
`§Organizations, members, keys` (excluding governance/app-availability/secrets),
`§Workspaces` (excluding activity/usage), `§Instance administration` (users only).
Behaviour: `04`. Path params `{org}`, `{ws}`, `{agent}`, `{user}`, `{id}` are **UUIDs**.

**Contract decisions** (gaps where `04`/`05`/`14` imply but do not spell out):
- C-01 `POST /v1/auth/signup` accepts an optional `invitation_token`. A valid
  pending invitation for the *same* email permits sign-up in `invite_only` mode
  and marks the email verified (the token was delivered to that inbox). The
  invitation is **not** auto-accepted (`POST /v1/invitations/accept` remains SO).
  Invitation tokens are never returned by any API (only emailed / logged in
  log-only mode), so an admin cannot use this to claim someone else's email.
- C-02 `GET /v1/me/api-keys` lists the caller's own `user`-kind keys across
  workspaces (dashboard `/account` "personal workspace keys", `14`). Any key
  owner may revoke their own key with `DELETE /v1/api-keys/{id}`.
- C-03 `GET /v1/instance` adds `org_creation`, `email_delivery` and
  `setup_errors` (dashboard `/setup-error` and "emails are logged" notice, `14`, `04`).
- C-04 `PATCH /v1/admin/users/{id}` (item path) implements `05`'s "PATCH /v1/admin/users".
- C-05 `GET /v1/workspaces/{ws}/agents` and `GET /v1/agents/{agent}` are
  delivered read-only in M1 (Daneel visible); create/update/delete arrive with M2/M4.
- C-06 Session cookie `aurora_session` (`04 §Sessions`); successful sign-up,
  login and OIDC callback set it; logout clears it.

#### Shared schemas (M1)

```ts
type Uuid = string; type Timestamp = string /* RFC 3339 UTC */;
type OrgRole = "owner" | "admin" | "member";
type WorkspaceRole = "owner" | "member";

interface User { id: Uuid; email: string; display_name: string;
  email_verified_at: Timestamp | null; is_instance_admin: boolean; created_at: Timestamp }

interface OrgSummary { id: Uuid; slug: string; name: string }
interface Org extends OrgSummary { external_id: string; app_posture: "open" | "restricted";
  settings: { members_can_create_workspaces: boolean; max_members: number | null; max_agents: number | null };
  my_role: OrgRole; created_at: Timestamp }

interface Workspace { id: Uuid; org_id: Uuid; slug: string; name: string; timezone: string;
  memory_language: string; my_role: WorkspaceRole /* org owner/admin ⇒ "owner" */;
  created_at: Timestamp; updated_at: Timestamp }

interface Me { user: User; default_org_id: Uuid | null;
  memberships: { org: OrgSummary; role: OrgRole; status: "active" | "suspended";
                 workspaces: { id: Uuid; slug: string; name: string; my_role: WorkspaceRole }[] }[] }

interface Member { user_id: Uuid; email: string; display_name: string; role: OrgRole;
  status: "active" | "suspended"; created_at: Timestamp }

interface Invitation { id: Uuid; org_id: Uuid; email: string; role: "admin" | "member";
  workspace_ids: Uuid[]; status: "pending" | "accepted" | "cancelled" | "expired";
  invited_by: { id: Uuid; display_name: string } | null; expires_at: Timestamp; created_at: Timestamp }

interface AccessBinding { user_id: Uuid; email: string; display_name: string;
  role: WorkspaceRole; implicit: boolean /* true: org owner/admin, no row */ }

interface ApiKey { id: Uuid; name: string; kind: "user" | "service"; org_id: Uuid;
  workspace_id: Uuid | null /* null ⇒ org key auo_ */; display_prefix: string;
  owner: { id: Uuid; display_name: string }; last_used_at: Timestamp | null;
  expires_at: Timestamp | null; revoked_at: Timestamp | null; created_at: Timestamp }

interface AuditEntry { id: Uuid; org_id: Uuid | null; workspace_id: Uuid | null;
  actor_kind: "user" | "service" | "agent" | "system" | "instance_admin"; actor_id: Uuid | null;
  actor_label: string; action: string; target_kind: string | null; target_id: string | null;
  outcome: "success" | "failure" | "denied"; source: "dashboard" | "api" | "cli" | "channel" | "system";
  ip: string | null; metadata: Record<string, unknown>; created_at: Timestamp }

interface Agent { id: Uuid; org_id: Uuid; workspace_id: Uuid; handle: string; name: string;
  kind: "byo" | "hosted"; is_default: boolean; harness: string | null;
  model_provider: string | null; model_endpoint_id: string | null; model_id: string | null;
  effort: "low" | "medium" | "high" | "max" | null; proactive_messaging: "ask" | "allow" | "off";
  brief: string; created_at: Timestamp; updated_at: Timestamp }

interface AdminUser extends User { disabled_at: Timestamp | null; last_login_at: Timestamp | null }

interface Page<T> { items: T[]; next_cursor: string | null }
```

#### Endpoints (M1)

| Method & path | Auth | Request | 2xx response | Errors (status `code`) |
|---|---|---|---|---|
| GET `/v1/instance` | none | — | 200 `{ version, signup_mode: "open"\|"invite_only"\|"sso_jit", org_creation: "instance_admins"\|"anyone", oidc_providers: {id, display_name}[], features: {hosted: bool, channels: bool}, default_agent_name, external_url, email_delivery: "smtp"\|"log", setup_errors: string[] }` | — |
| GET `/v1/resolve?org=&workspace=` | S/K | slugs (`workspace` optional) | 200 `{ org: OrgSummary, workspace: {id,slug,name} \| null }` | 404 `not_found` |
| POST `/v1/auth/signup` | none | `{ email, password, display_name, invitation_token? }` | 201 `{ user: User }` + cookie. `open`: also creates `<display_name>'s org` + default workspace (+Daneel) | 400 `invalid_request` (password 12–256, common list, email, display_name 1–80); 403 `signup_closed`; 409 `email_taken`; 429 |
| POST `/v1/auth/login` | none | `{ email, password }` | 200 `{ user: User }` + cookie | 401 `invalid_credentials`; 403 `account_disabled`; 429 `rate_limited` |
| POST `/v1/auth/logout` | S | — | 204, cookie cleared | — |
| POST `/v1/auth/verify-email` | none | `{ token }` | 200 `{ user: User }` | 400 `invalid_token` (unknown/used/expired) |
| POST `/v1/auth/verify-email/resend` | S | — | 202 | 409 `already_verified`; 429 |
| POST `/v1/auth/password/forgot` | none | `{ email }` | 202 (always) | 429 |
| POST `/v1/auth/password/reset` | none | `{ token, password }` | 204 (all sessions revoked) | 400 `invalid_token`, `invalid_request` |
| POST `/v1/auth/password/change` | SO | `{ current_password, new_password }` | 204 | 401 `invalid_credentials`; 400 `invalid_request` |
| POST `/v1/auth/sessions/revoke-others` | SO | — | 200 `{ revoked: number }` | — |
| GET `/v1/auth/oidc/{provider}/start?return_to=` | none | same-origin path | 302 provider | 404 unknown provider; 400 bad `return_to` |
| GET `/v1/auth/oidc/{provider}/callback` | none | provider params | 302 `return_to` + cookie | 302 `/auth/login?error=<code>` (`oidc_state_invalid`, `oidc_failed`, `signup_closed`, `email_unverified`, `email_domain_not_allowed`) |
| POST `/v1/auth/cli` | none | `{ hostname? }` | 200 `{ device_code, user_code: "ABCD-EFGH", verification_url, interval: 5, expires_in: 600 }` | 429 |
| POST `/v1/auth/cli/token` | none | `{ device_code }` | 200 `{ api_key: string, workspace: {id, slug, name, org_id, org_slug} }` (once) | 400 `authorization_pending` \| `slow_down` \| `expired_token` \| `access_denied` \| `invalid_token` |
| POST `/v1/auth/cli/approve` | SO | `{ user_code, workspace_id }` | 204 | 404 `not_found` (unknown/expired code); 404 workspace not visible |
| GET `/v1/me` | S | — | 200 `Me` | 401 |
| PATCH `/v1/me` | S | `{ display_name?, default_org_id? }` | 200 `Me` | 400; 404 org not a membership |
| DELETE `/v1/me` | SO | `{ password? }` | 204 | 403 `reauthentication_required`; 409 `last_owner` |
| GET `/v1/me/api-keys` (C-02) | S | — | 200 `Page<ApiKey>` | — |
| GET `/v1/orgs` | S | — | 200 `Page<Org>` (mine, active) | — |
| POST `/v1/orgs` | S | `{ name, slug? }` | 201 `{ org: Org, workspace: Workspace, default_agent: Agent }` | 403 `org_creation_forbidden`; 409 `slug_taken`; 400 |
| GET `/v1/orgs/{org}` | S/K | — | 200 `Org` | 404 |
| PATCH `/v1/orgs/{org}` | S/K owner | `{ name?, slug?, settings?: {members_can_create_workspaces?, max_members?, max_agents?} }` | 200 `Org` | 403; 409 `slug_taken` |
| DELETE `/v1/orgs/{org}` | S/K owner | `{ confirm: <org slug> }` | 204 | 400 `confirmation_mismatch`; 403 |
| GET `/v1/orgs/{org}/members` | S/K | `?limit&cursor&status` | 200 `Page<Member>` | 403 (member role) |
| PATCH `/v1/orgs/{org}/members/{user}` | S/K owner/admin | `{ role?, status? }` | 200 `Member` | 403 (admin vs owner / granting owner); 409 `last_owner` |
| DELETE `/v1/orgs/{org}/members/{user}` | S/K owner/admin | — | 204 | 403; 409 `last_owner` |
| GET `/v1/orgs/{org}/invitations` | S/K owner/admin | `?status` | 200 `Page<Invitation>` | 403 |
| POST `/v1/orgs/{org}/invitations` | S/K owner/admin | `{ email, role, workspace_ids? }` | 201 `Invitation` (re-invite of a pending email rotates token, same response) | 409 `already_member`; 422 `member_limit_reached`; 400 |
| DELETE `/v1/invitations/{id}` | S/K owner/admin | — | 204 (→ `cancelled`) | 404 |
| GET `/v1/invitations/preview?token=` | none | — | 200 `{ org: OrgSummary, email, role, inviter_name: string \| null, status, expires_at }` | 404 `not_found` |
| POST `/v1/invitations/accept` | SO | `{ token }` | 200 `{ org: Org, workspace_ids: Uuid[] }` | 403 `email_mismatch` \| `email_unverified`; 404; 410 `invitation_expired`; 409 `already_member`; 422 `member_limit_reached` |
| GET `/v1/orgs/{org}/api-keys` | S/K owner/admin | — | 200 `Page<ApiKey>` (org keys, kind user) | 403 |
| POST `/v1/orgs/{org}/api-keys` | SO owner/admin | `{ name, expires_at? }` | 201 `{ api_key: ApiKey, key: "auo_…" }` | 403 |
| DELETE `/v1/api-keys/{id}` | S/K | — | 204 (revoked) — key owner, or owner/admin (org key) / `can_own` (workspace key) | 404 |
| GET `/v1/orgs/{org}/audit` | S/K owner/admin | `?limit&cursor&action&actor_id` | 200 `Page<AuditEntry>` | 403 |
| GET `/v1/orgs/{org}/workspaces` | S/K | — | 200 `Page<Workspace>` (visible to caller) | 404 |
| POST `/v1/orgs/{org}/workspaces` | S/K | `{ name, slug?, timezone?, memory_language? }` | 201 `{ workspace: Workspace, default_agent: Agent }` | 403; 409 `slug_taken`; 400 (`timezone`, `memory_language` invalid) |
| GET `/v1/workspaces/{ws}` | S/K | — | 200 `Workspace` | 404 |
| PATCH `/v1/workspaces/{ws}` | S/K can_own | `{ name?, timezone?, memory_language? }` | 200 `Workspace` | 403; 400 |
| DELETE `/v1/workspaces/{ws}` | S/K can_own | `{ confirm: <ws slug> }` | 204 | 400 `confirmation_mismatch`; 403 |
| GET `/v1/workspaces/{ws}/access` | S/K | — | 200 `Page<AccessBinding>` (includes implicit org owners/admins) | 404 |
| PUT `/v1/workspaces/{ws}/access/{user}` | S/K can_own | `{ role }` | 200 `AccessBinding` | 422 `not_an_org_member`; 403 |
| DELETE `/v1/workspaces/{ws}/access/{user}` | S/K can_own | — | 204 | 404; 422 `implicit_access` (org owner/admin) |
| GET `/v1/workspaces/{ws}/api-keys` | S/K can_own | — | 200 `Page<ApiKey>` | 403 |
| POST `/v1/workspaces/{ws}/api-keys` | SO can_own | `{ name, expires_at? }` | 201 `{ api_key: ApiKey, key: "auk_…" }` | 403 |
| GET `/v1/workspaces/{ws}/audit` | S/K can_own | `?limit&cursor&action&actor_id` | 200 `Page<AuditEntry>` | 403 |
| GET `/v1/workspaces/{ws}/agents` (C-05) | S/K can_use | — | 200 `Page<Agent>` | 404 |
| GET `/v1/agents/{agent}` (C-05) | S/K can_use | — | 200 `Agent` | 404 |
| GET `/v1/admin/users` | IA | `?q&limit&cursor` | 200 `Page<AdminUser>` | 403 `instance_admin_required` |
| PATCH `/v1/admin/users/{id}` (C-04) | IA | `{ is_instance_admin?, disabled? }` | 200 `AdminUser` | 422 `cannot_modify_self` |

Authorization failures on resources the caller cannot see are **404**; on
visible resources with insufficient role, **403 `forbidden`**. Workspace key
used on another workspace → 403 `workspace_mismatch`. Org key on workspace
routes uses the path workspace (or `X-Workspace-Id`).

## Shared schemas

_Defined alongside the milestone sections above._
