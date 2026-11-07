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

##### Phase 2 — RFE-021 (automation continuity, D-58)

Rule: `04 §Fire-time authority`, `05 §Schedules/§Webhooks/§Members`, SEC-51. All new routes are session or key, `can_own` unless stated; refusals use the standard envelope.

- C-90 **`Schedule` and `Webhook` objects** gain `anchor: "creator" | "workspace"` (always present). `created_by` stays `{ id, display_name } | null` and is the fire-time authority when `anchor = creator`, provenance only when `anchor = workspace`. `POST /v1/agents/{agent}/schedules` and `…/webhooks` accept `anchor` (default `creator`); `PATCH /v1/schedules/{id}` and `PATCH /v1/webhooks/{id}` accept `anchor`. Setting or switching to `workspace` needs `can_own` (403 otherwise, also for `anchor: "creator"` on a PATCH by a non-owner); an unknown value is 400. Switching back to `creator` keeps `created_by` as the authority and answers 422 `new_creator_unauthorized` if that user no longer can use the workspace. An anchor change is audited `automation.reassign` (`to: "workspace"` or the creator). Agent tools never accept `anchor` (unknown argument) and always create `creator`-anchored items.
- C-91 **`PUT /v1/schedules/{id}/creator`**, **`PUT /v1/webhooks/{id}/creator`**: body `{ user_id: Uuid, re_enable?: bool }` (unknown fields refused) → `200` the updated `Schedule`/`Webhook`, anchor `creator`. `can_own` of the agent's workspace (403 `forbidden`; 404 for an unknown item). The new creator must be an active org member who can use the workspace, else **422 `new_creator_unauthorized`**. With `re_enable: true`, an item disabled for `authorization` is re-enabled (`disabled_reason` cleared, failure count reset, schedule `next_fire_at` recomputed); an item disabled for another reason stays disabled. One audit row `automation.reassign` (security-critical, written in the same transaction): `target = (schedule|webhook, id)`, `metadata { kind, id, agent_id, to, old_creator, new_creator, old_anchor, new_anchor }`; a failed audit insert fails the call (503 `audit_unavailable`).
- C-92 **`POST /v1/workspaces/{ws}/automations/reassign`**: body `{ from_user_id: Uuid, to: Uuid | "workspace", kinds?: ["schedule","webhook"], ids?: [Uuid] }` → `200 { reassigned: number }`. One transaction, one audit row per item. Without `ids`, only creator-anchored items of `from_user_id` in the workspace move (workspace-anchored items do not depend on the person); with `ids`, exactly those of `from_user_id`. `to: "workspace"` sets `anchor = workspace` and keeps the creator as provenance. 422 `new_creator_unauthorized` (target cannot use the workspace; nothing changes); 400 for an unknown kind or `to` equal to `from_user_id` when items match.
- C-93 **`reassign_to`** on member removal and suspension: `DELETE /v1/orgs/{org}/members/{user}?reassign_to=<user id | workspace>`, `PATCH /v1/orgs/{org}/members/{user}` body field `reassign_to` (only together with `status: "suspended"`, else 400), `DELETE /v1/workspaces/{ws}/access/{user}?reassign_to=…`. Every creator-anchored schedule and webhook of the person (all workspaces of the org, or the one workspace) is reassigned in the **same transaction** as the removal; any failure (422 `new_creator_unauthorized`, 400 `reassign_to` equal to the person or malformed, audit failure) rolls both back. A user target must be an active org member (422 otherwise) and, per workspace holding items, able to use it.
- C-94 **`GET /v1/orgs/{org}/members/{user}/offboarding`** (org `owner`/`admin`; 404 unknown member) and **`GET /v1/workspaces/{ws}/members/{user}/offboarding`** (`can_own`; 404 when the person has no access): `200 { schedules: [Item], webhooks: [Item], peer_tasks: [Item], api_keys: number }`, `Item = { id, name, agent: { id, name }, workspace: { id, name }, enabled: bool, anchor, next_fire_at: timestamp | null, would_disable: bool }`. Lists exactly what removal would disable: enabled creator-anchored schedules and webhooks of the person (`would_disable` is always true), queued/open peer tasks they requested (`name` = "from → to"), and `api_keys` = live personal keys that would stop working (org view: every key in the org; workspace view: keys confined to that workspace, since organization-wide keys survive losing one binding). The workspace view of an org owner/admin is empty (a binding removal cannot take implicit access away).
- C-95 **Fire-time authority** (no route change): a schedule fire or webhook delivery of an `anchor = workspace` item is authorized when the workspace has at least one active member with `can_own` (workspace `owner` binding or org owner/admin); otherwise the item is disabled `authorization` (schedule run `skipped_unauthorized`; webhook 403 `forbidden` + delivery row), as for a creator-anchored item whose creator lost access.

## Shared schemas (M1)

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

### M2 — secrets, LLM keys, BYO agents, grants, policy, approvals, governance (API-002)

Routes/auth: `05 §Secrets and LLM`, `§Grants and effective views`, `§Policy`,
`§Approvals`, `§Agents and sandboxes` (BYO subset), `§Organizations` (governance
rows), `§Workspaces` (activity). Behaviour: `07`, `08`. Rule JSON: I-12.
Approval summary: I-16. Injection recipe: `08 §5` (`aurora_secrets::recipe`).

**Contract decisions**
- C-07 Secret create takes exactly one of `value` (managed) or `reference` (`{store_id, path, field?}`); responses never contain values (SEC-12). LLM secrets take `llm: {endpoint_id, auth_mode?}`; `host_pattern`/`injection` are derived and read-only for them.
- C-08 Policy drafts are replaced whole; rule order = array order (server assigns `priority`); `logical_id` omitted ⇒ new rule.
- C-09 Batch decisions require `agent_id` in the body (`07 §Batching`: "within the same agent").
- C-10 `GET /v1/approvals/stream` events carry no SSE id (approvals are not transcript-sequenced); on reconnect clients refetch `GET …/approvals?status=pending`.
- C-11 `GET /v1/llm/providers?org_id=` adds the caller's effective governance per endpoint (`permitted`, `key_scopes`) for key forms.
- C-12 Agent `handle` is immutable after creation (not in the `05` PATCH list).

#### Shared schemas (M2)

```ts
type InjectionRecipe =
  | { kind: "bearer" } | { kind: "header"; name: string; format: string }
  | { kind: "query"; name: string; format: string } | { kind: "basic"; username: string }
  | { kind: "path_template"; prefix: string } | { kind: "path_regex"; pattern: string; replacement: string };

interface StoredOrigin { status: "verified" | "mismatch" | "unverifiable" | "pending"; method: "endpoint" | "litellm_team" | "provider_account";
  organization_id: string | null; workspace_id: string | null; team_id: string | null; user_id: string | null;
  value_version: number; checked_at: Timestamp }

interface Secret { id: Uuid; org_id: Uuid; workspace_id: Uuid | null; scope: "organization" | "workspace";
  name: string; kind: "generic" | "llm";
  llm: { provider: string; endpoint_id: string; auth_mode: string } | null;
  host_pattern: string; path_pattern: string | null; injection: InjectionRecipe;
  value_source: "managed" | "reference"; reference: { store_id: string; path: string; field: string | null } | null;
  status: "active" | "proposed" | "rejected"; origin: StoredOrigin | null; value_version: number;
  proposed_by_agent_id: Uuid | null; created_by: { id: Uuid; display_name: string } | null;
  last_used_at: Timestamp | null; created_at: Timestamp; updated_at: Timestamp }

interface ContainerConfig { proxy_url: string /* contains <AGENT_TOKEN> unless revealed */; env: Record<string, string>;
  ca_pem: string; ca_path: string; proxy_ca_path: string /* C-44 */; proxy_tls: "operator_certificate" | null /* C-44 */;
  llm_env: Record<string, string>; stubs: { path: string; content: string; mode: string }[] }

type Identity = { kind: "agent"; id: Uuid } | { kind: "user"; id: Uuid };
type Target =
  | { type: "app"; app_id: string; tools?: string[] } | { type: "connection"; id: Uuid; tools?: string[] }
  | { type: "app_connections"; app_id: string; scope: "workspace" | "organization" | "any"; tools?: string[] }
  | { type: "secret"; id: Uuid } | { type: "secrets"; scope: "workspace" | "organization" | "any"; kind?: "generic" | "llm" }
  | { type: "network"; host: string; path?: string; methods?: string[] };
interface Condition { on: "body"; op: "contains" | "not_contains" | "equals"; value: string; json_pointer?: string }
interface PolicyRule { logical_id: Uuid; priority: number; name: string; description: string | null; enabled: boolean;
  identities: Identity[]; targets: Target[]; action: "allow" | "block"; require_approval: boolean;
  rate_limit: { count: number; per: "minute" | "hour" | "day" } | null; conditions: Condition[] }
interface AllowlistEntry { host: string; path?: string }
interface PolicyGeneration { id: Uuid; number: number; status: "draft" | "published" | "superseded";
  default_action: "none" | "allow" | "block"; uncredentialed_default: "none" | "block" /* org scope: always none */;
  uncredentialed_allowlist: AllowlistEntry[]; rules: PolicyRule[]; draft_revision: number | null /* drafts only */;
  published_by: { id: Uuid; display_name: string } | null; published_at: Timestamp | null; created_at: Timestamp }
interface PolicyDocument { scope: "organization" | "workspace"; published: PolicyGeneration; draft: PolicyGeneration }

interface ScopeTrace { verdict: "allow" | "needs_approval" | "rate_limited" | "block" | null;
  considered: { rule_logical_id: Uuid; rule_name: string; source: "hand" | "derived" | "default"; matched: boolean; reason: string }[] }
interface Simulation { verdict: "allow" | "needs_approval" | "rate_limited" | "block";
  reason: "rule" | "default_denied" | "egress_denied" | "governance" | null; guidance_code: string | null;
  winning_credential: { kind: "secret" | "connection"; id: Uuid; name: string } | null;
  candidates: { kind: "secret" | "connection"; id: Uuid; name: string; selected: boolean }[];
  trace: { organization: ScopeTrace; workspace: ScopeTrace } }

interface SecretGrant { id: Uuid; agent_id: Uuid; secret_id: Uuid; created_at: Timestamp }
interface ConnectionGrant { id: Uuid; agent_id: Uuid; connection_id: Uuid; mode: "full" | "custom";
  allow_tools: string[]; ask_tools: string[]; created_at: Timestamp }   // M3

interface ApprovalSummary { title: string; fields: { label: string; value: string }[]; text: string | null;
  link: string | null; group: string | null; source: "template" | "hook" | "generic" }
interface Approval { id: Uuid; org_id: Uuid; workspace_id: Uuid; agent: { id: Uuid; name: string; handle: string } | null;
  conversation_id: Uuid | null; turn_id: Uuid | null; method: string; host: string; path: string;
  app_id: string | null; tool_id: string | null; connection_id: Uuid | null; rule_logical_id: Uuid | null; rule_name: string | null;
  summary: ApprovalSummary; batch: { id: string; label: string | null; total: number | null } | null;
  status: "pending" | "approved" | "denied" | "expired" | "abandoned";
  decided_by: { id: Uuid; display_name: string } | null; decided_via: "dashboard" | "api" | "channel" | null;
  decided_at: Timestamp | null; deadline: Timestamp; created_at: Timestamp }

interface RequestLogEntry { id: Uuid; ts: Timestamp; agent_id: Uuid; conversation_id: Uuid | null; turn_id: Uuid | null;
  credential_kind: "agent" | "boot" | "context"; lane: "mitm" | "open" | "db"; method: string; host: string; path: string;
  app_id: string | null; tool_id: string | null; connection_id: Uuid | null; secret_id: Uuid | null; decision: string;
  rule_logical_id: Uuid | null; rule_name: string | null; upstream_status: number | null; status: number;
  error_code: string | null; injected: number; latency_ms: number; bytes_up: number; bytes_down: number }

// Governance: OrgGovernance = the `08 §11.2` document verbatim.
interface Sourced<T> { value: T; source: "instance" | "org" }
interface EffectiveGovernance {
  llm: { posture: Sourced<"open" | "sanctioned_only">;
         endpoints: { id: string; provider: string; display_name: string; permitted: Sourced<boolean>;
                      key_scopes: Sourced<("organization" | "workspace")[]>; allowed_models: Sourced<string[]>;
                      origin_rule: { method: string; allowed_organization_ids: string[]; allowed_workspace_ids: string[];
                                     allowed_team_ids: string[] } | null }[] };
  egress: { posture: Sourced<"open" | "credentialed_only">; org_allowlist: AllowlistEntry[]; baseline_allowlist: AllowlistEntry[] };
  non_compliant_secrets: { id: Uuid; name: string; workspace_id: Uuid | null; reason: "mismatch" | "unverifiable" | "not_permitted" | "pending" }[] }
```

#### Endpoints (M2)

| Method & path | Auth | Request | 2xx response | Errors |
|---|---|---|---|---|
| GET `/v1/workspaces/{ws}/secrets`, `/v1/orgs/{org}/secrets` | S/K (can_use / org member) | `?kind&status` | 200 `Page<Secret>` | 404 |
| POST `/v1/workspaces/{ws}/secrets`, `/v1/orgs/{org}/secrets` | S/K can_own / owner·admin | `{ name, kind, llm?: {endpoint_id, auth_mode?}, host_pattern?, path_pattern?, injection?, value?, reference? }` (C-07) | 201 `Secret` | 400; 403 `llm_key_not_permitted`; 409 `name_taken`; 422 `llm_host_requires_llm_key` \| `llm_key_invalid` \| `llm_key_wrong_organization` \| `llm_key_unverifiable` \| `llm_endpoint_unreachable` \| `reference_not_permitted`; 503 `secret_store_unavailable` |
| GET/PATCH/DELETE `/v1/secrets/{id}` | S/K | PATCH `{ name?, host_pattern?, path_pattern?, injection? }` (generic only) | 200 `Secret` / 204 | 404; 403; 422 as create |
| PUT `/v1/secrets/{id}/value` | S/K can_own | `{ value }` \| `{ reference }` | 200 `Secret` (`value_version`+1, origin re-verified) | as create |
| POST `/v1/secrets/{id}/accept` | S can_own | `{ value }` | 200 `Secret` (active) + grant to proposer | 409 `not_proposed` |
| POST `/v1/secrets/{id}/reject` | S/K can_own | — | 204 | 409 `not_proposed` |
| GET `/v1/secret-stores` | S/K | — | 200 `{ items: { id, kind, capabilities: {managed, references} }[] }` | — |
| GET `/v1/llm/providers` | S/K | `?org_id` (C-11) | 200 `{ providers: { id, display_name, auth_modes: string[], dialects: string[] }[], endpoints: { id, provider, display_name, base_url, dialects, origin_method: string \| null, permitted: boolean \| null, key_scopes: string[] \| null }[] }` | — |
| GET `/v1/secrets/{id}/models` | S/K | — | 200 `{ items: { id, display_name: string \| null }[] }` (allowed only) | 404; 502 `llm_endpoint_unreachable` |
| POST `/v1/secrets/{id}/verify` | S/K can_own | — | 200 `{ origin: StoredOrigin }` | 404; 422 |
| POST `/v1/workspaces/{ws}/agents` | S/K can_use | `{ name, handle?, kind, brief?, harness?, model_endpoint_id?, model_id?, effort?, proactive_messaging? }` | 201 `{ agent: Agent, token: string \| null /* byo only, once */, container_config: ContainerConfig \| null }` | 400; 409 `handle_taken`; 422 `hosted_disabled` \| `agent_limit_reached` |
| PATCH `/v1/agents/{agent}` | S/K can_use | `{ name?, brief?, harness?, model_endpoint_id?, model_id?, effort?, proactive_messaging?, resource_overrides? (IA) }` (C-12) | 200 `Agent` | 400; 404 |
| DELETE `/v1/agents/{agent}` | S/K can_use | — | 204 (credentials revoked) | 404 |
| POST `/v1/agents/{agent}/token` | S can_use | — | 200 `{ token, container_config }` (BYO; revokes previous agent + context credentials) | 403 `hosted_agent_token_hidden` |
| GET `/v1/agents/{agent}/container-config` | S/K can_use | — | 200 `ContainerConfig` (`<AGENT_TOKEN>` placeholder) | 403 `hosted_agent_token_hidden` |
| GET `/v1/agent/container-config` | A | — | 200 `ContainerConfig` (token embedded) | 401 |
| POST `/v1/agent/context-credentials` | A | `{ label, conversation_id?, ttl_secs? }` | 201 `{ credential: "auc_…", expires_at, proxy_url }` | 400; 404 conversation |
| GET `/v1/gateway/ca.pem` | S/K | — | 200 `application/x-pem-file` | 503 |
| GET `/v1/agents/{agent}/grants` | S/K can_use | — | 200 `{ secrets: SecretGrant[], connections: ConnectionGrant[] }` | 404 |
| PUT/DELETE `/v1/agents/{agent}/grants/secrets/{secret}` | S/K can_own | — | 200 `SecretGrant` / 204 | 404 (secret not in agent's workspace or its org) |
| GET `/v1/agents/{agent}/effective` | S/K can_use | — | 200 `{ credentials: { kind, id, name, scope, selected: boolean, selected_by: Uuid[] }[], tools: { app_id, tool_id, connection_id: Uuid, decision: string, org_ceiling: string \| null, inert: boolean }[] }` (M3 additions: `connection_id`, `inert` for tools still in a grant but removed from the catalogue) | 404 |
| GET `/v1/workspaces/{ws}/policy`, `/v1/orgs/{org}/policy` | S/K (read: can_use / member) | — | 200 `PolicyDocument` | 404 |
| PUT `…/policy/draft` | S/K can_own / owner·admin | `{ revision, default_action, uncredentialed_default?, uncredentialed_allowlist?, rules: Omit<PolicyRule,"priority"\|"logical_id"> & {logical_id?}[] }` (C-08) | 200 `PolicyGeneration` (draft) | 409 `stale_revision`; 422 `policy_invalid` (`details.fields`) |
| POST `…/policy/publish` | same | `{ revision }` | 200 `PolicyDocument` | 409 `stale_revision` |
| POST `…/policy/rollback` | same | `{ generation: number }` | 200 `PolicyDocument` | 404 |
| GET `…/policy/generations` | S/K | `?limit&cursor` | 200 `Page<Omit<PolicyGeneration,"rules"> & {rule_count}>` | — |
| GET `…/policy/warnings` | S/K | — | 200 `{ items: { code: "shadowed" \| "inert" \| "overridden_by_org", rule_logical_id, message }[] }` | — |
| POST `/v1/workspaces/{ws}/policy/simulate` | S/K can_use | `{ agent_id, method, url, body?, use_draft? }` | 200 `Simulation` | 400; 404 agent |
| GET `/v1/workspaces/{ws}/approvals` | S/K can_use | `?status&limit&cursor` | 200 `Page<Approval>` | 404 |
| GET `/v1/approvals/pending` | S/K | `?wait` (≤30) | 200 `{ items: Approval[] }` (decidable by caller) | — |
| GET `/v1/approvals/stream` | S | — | SSE `event: approval`, `data: { event: "created"\|"decided"\|"expired", approval: Approval }` (C-10) | 401 |
| POST `/v1/approvals/{id}/decision` | S/K can_use | `{ decision: "approve"\|"deny" }` | 200 `Approval` | 404; 409 `already_decided` |
| POST `/v1/approvals/batches/{batch_id}/decision` | S/K can_use | `{ decision, agent_id }` (C-09) | 200 `{ decided: number }` | 404 |
| GET `/v1/orgs/{org}/governance` | S/K owner·admin | — | 200 `{ document: OrgGovernance, revision: number }` | 403 |
| PUT `/v1/orgs/{org}/governance` | S/K owner·admin | `{ document, revision }` | 200 same | 409 `stale_revision`; 422 `governance_invalid` \| `governance_below_instance_minimum` |
| GET `/v1/orgs/{org}/governance/effective` | S/K any member | — | 200 `EffectiveGovernance` | 404 |
| GET `/v1/governance/instance` | S | — | 200 `{ llm: { allowed_key_scopes, minimum_posture, require_key_origin, known_llm_hosts: string[], endpoints: {id, provider, display_name, allowed_models, key_origin}[] }, egress: { minimum_posture, baseline_allowlist } }` | — |
| GET `/v1/workspaces/{ws}/activity` | S/K can_use | `?agent_id&conversation_id&decision&host&app_id&limit&cursor` | 200 `Page<RequestLogEntry>` (`ts desc`) | 404 |

### M3 — apps, connections, OAuth clients, connection grants, plugin administration (API-003)

Routes/auth: `05 §Apps, connections, OAuth clients`, `§Grants and effective views`
(connection grants), `§Instance administration` (plugins, host claims, trusted keys,
instance OAuth clients, registry snapshot), internal routes `05 §Authentication
surfaces` (`/internal/v1/registry/*`). Behaviour: `09 §7, §9–§11`, `08 §8 OAuth
clients`, `07 §Grants`. Snapshot JSON: `09 §9` (I-14, I-18).

**Contract decisions**
- C-13 App catalogue/detail data come from the **active snapshot** (no second catalogue); human strings are already sanitized there. `GET /v1/apps/{app}/icon` returns the stored PNG (re-encoded on ingest is out of scope; serve as `image/png`, `nosniff`).
- C-14 Connect responses: `{ kind: "redirect", url }` or `{ kind: "connected", connection: Connection, missing_scopes: string[] }`. The OAuth callback answers a tiny HTML page (no inline script beyond a nonce'd `postMessage({type:"aurora:connected", connection_id})` to `window.opener` with target origin = external origin, then `window.close()`), or on error a page carrying `error=<code>`.
- C-15 Connection credentials are never returned (SEC-12); `metadata` (non-secret, manifest-declared) and `scopes` are.
- C-16 Plugin upload is `multipart/form-data` with one part `package` (≤ `PACKAGE_MAX`); validation failures still create a row with status `invalid` and `violations` (so admins see why), except signature/archive-corrupt failures which return 422 without a row.
- C-17 Host-claim decisions are keyed by the **group** key and fingerprint of I-18.
- C-22 `POST /v1/apps/{app}/connect` also accepts `org_id` (required when `scope = organization`).
- C-21 `AppDetail` adds `oauth_clients: { client_key: string, fields: { name: string, label: string, secret: boolean }[], setup_note: string | null, redirect_uri: string }[]` (the package's `oauth_clients` entries used by this app; `redirect_uri` = `<external_url>/v1/oauth/callback`) so the dashboard renders BYOC forms from data (`14`: no app knowledge in the bundle).
- C-20 `GET/PUT /v1/orgs/{org}/app-availability` body: `{ posture: "open" | "restricted", allow: { app_id: string, workspace_id: Uuid | null }[] }`; PUT replaces the whole list (S-01 rows) and publishes `aurora_gateway_invalidate {scope: org}`.

#### Shared schemas (M3)

```ts
interface FormField { name: string; label: string; type: "text" | "secret" | "select" | "file" | "json_file";
  store: "credentials" | "metadata" | "tenant_host"; required: boolean; placeholder: string | null; help_url: string | null;
  options: { value: string; label: string; warning: string | null }[] | null;
  show_when: { field: string; equals: string } | null }
interface ConnectMethod { id: string; kind: "oauth2" | "fields" | "hook";
  fields: FormField[]; scopes: { scope: string; name: string; access: "read" | "write" }[]; default_scopes: string[];
  access_levels: { id: string; label: string }[]; client_key: string | null; callback: "query" | "fragment" | null }
interface Tool { id: string; name: string; description: string | null; group: "read" | "write"; wildcard: boolean;
  host: string; path: string; methods: string[]; graphql: "query" | "mutation" | null; git: "push" | "fetch" | null }
interface AppSummary { app_id: string; package_id: string; package_version: string; name: string; description: string;
  category: string | null; available: boolean; connection_count: number; has_dark_icon: boolean }
interface AppDetail extends AppSummary { api_docs: string | null; connect_note: { text: string; url: string | null } | null;
  label_hint: string | null; connect: ConnectMethod[]; tools: Tool[]; hosts: { host: string; path_prefix: string | null; claim_status: "active" | "lost" }[];
  agent_instructions: string | null; credentials: { name: string; optional: boolean }[]; metadata: string[] }
interface Connection { id: Uuid; org_id: Uuid; workspace_id: Uuid | null; scope: "organization" | "workspace";
  app_id: string; package_id: string; app_version: string; label: string;
  status: "active" | "reauth_required" | "disabled" | "orphaned"; status_reason: string | null;
  scopes: string[]; metadata: Record<string, string>; tenant_host: string | null; oauth_client_id: Uuid | null;
  credentials_expires_at: Timestamp | null; connected_by: { id: Uuid; display_name: string } | null;
  last_used_at: Timestamp | null; created_at: Timestamp; updated_at: Timestamp;
  injecting: { host: string; lost_to_app_id: string | null }[] }
interface OAuthClient { id: Uuid; scope: "instance" | "organization" | "workspace"; org_id: Uuid | null; workspace_id: Uuid | null;
  package_id: string; client_key: string; fields: Record<string, string> /* non-secret only */; enabled: boolean;
  connection_count: number; created_at: Timestamp; updated_at: Timestamp }
// ConnectionGrant is defined in M2 shared schemas.
interface PluginPackage { id: Uuid; package_id: string; version: string; contract: number; publisher: string; digest: string;
  signer: { kind: "builtin" | "trusted_key" | "unsigned"; key_id: string | null; name: string | null };
  source: "builtin" | "directory" | "upload"; status: "pending_approval" | "active" | "superseded" | "rejected" | "disabled" | "invalid";
  status_reason: string | null; violations: { code: string; path: string; message: string }[];
  apps: { app_id: string; name: string }[]; permissions: { hooks: string[]; egress: Record<string, string[]>; context: string[] };
  host_claims: { claim_key: string; host: string; path_prefix: string | null; app_id: string }[];
  widenings: { kind: "hook" | "egress" | "context" | "host_claim" | "zone"; detail: string }[] /* vs previously approved version */;
  approved_by: { id: Uuid; display_name: string } | null; approved_at: Timestamp | null; created_at: Timestamp }
interface HostClaimConflict { group_key: string; fingerprint: string;
  claims: { package_id: string; app_id: string; claim_key: string }[];
  decision: { winner_package_id: string; winner_app_id: string; decided_by: { id: Uuid; display_name: string } | null; stale: boolean } | null }
interface TrustedKey { id: Uuid; publisher: string; name: string; public_key: string; revoked_at: Timestamp | null; created_at: Timestamp }
```

#### Endpoints (M3)

| Method & path | Auth | Request | 2xx response | Errors |
|---|---|---|---|---|
| GET `/v1/apps?workspace_id=` | S/K | — | 200 `{ items: AppSummary[] }` | 404 |
| GET `/v1/apps/{app}` | S/K | `?workspace_id` | 200 `AppDetail` | 404 |
| GET `/v1/apps/{app}/icon?variant=light\|dark`, `/v1/apps/{app}/guide` | S/K | — | 200 `image/png` / `text/markdown` | 404 |
| POST `/v1/apps/{app}/connect` | S (workspace: can_own; org: owner·admin) | `{ scope, workspace_id?, method_id, fields?, oauth_client_id?, access_level?, reconnect_connection_id? }` | 200 C-14 | 403 `app_unavailable`; 404; 400 `invalid_request` (fields); 422 `connect_probe_failed` (`details.upstream_status`) \| `oauth_client_required` \| `connect_hook_failed` |
| GET `/v1/oauth/callback` | P | provider params + state | 200 HTML (C-14) | HTML with `error` (`oauth_state_invalid`, `oauth_exchange_failed`, `connect_probe_failed`) |
| GET `/v1/workspaces/{ws}/connections`, `/v1/orgs/{org}/connections` | S/K | `?app_id&status` | 200 `Page<Connection>` | 404 |
| GET/PATCH/DELETE `/v1/connections/{id}` | S/K (write: can_own / owner·admin) | PATCH `{ label?, status?: "active" \| "disabled" }` | 200 `Connection` / 204 | 404; 409 |
| GET/POST `/v1/workspaces/{ws}/oauth-clients`, `/v1/orgs/{org}/oauth-clients` | S/K | POST `{ package_id, client_key, fields: Record<string,string> /* incl. secret fields; secret ones sealed */ }` | 200 `Page<OAuthClient>` / 201 `OAuthClient` | 400 (schema of manifest `fields`); 409 `conflict` |
| PATCH/DELETE `/v1/oauth-clients/{id}` | S/K (IA for instance clients) | PATCH `{ fields?, enabled? }` | 200 / 204 | 409 `client_in_use` |
| GET/POST `/v1/admin/oauth-clients` | IA | as above | instance clients | — |
| PUT/DELETE `/v1/agents/{agent}/grants/connections/{conn}` | S/K can_own | `{ mode: "full" \| "custom", allow_tools?: string[], ask_tools?: string[] }` | 200 `ConnectionGrant` / 204 | 422 `grant_invalid` (`details.reason`: `empty_custom` \| `tool_in_both` \| `unknown_tool`) |
| GET `/v1/connections/{id}/grants` | S/K | — | 200 `{ items: ConnectionGrant[] }` | 404 |
| GET `/v1/admin/plugins` | IA | `?status` | 200 `Page<PluginPackage>` | — |
| POST `/v1/admin/plugins` | IA | multipart `package` (C-16) | 201 `PluginPackage` | 413; 422 `package_invalid` (`details.violations`) |
| GET `/v1/admin/plugins/{id}` | IA | — | 200 `PluginPackage` | 404 |
| POST `/v1/admin/plugins/{id}/approve` \| `reject` \| `disable` \| `enable` | IA | `{ reason? }` | 200 `PluginPackage` | 409 `invalid_state`; 409 `claim_conflict_unresolved` |
| DELETE `/v1/admin/plugins/{id}?force=` | IA | — | 204 | 409 `live_connections` (`details.count`) unless `force` (connections → `orphaned`) |
| GET `/v1/admin/host-claims` | IA | — | 200 `{ items: HostClaimConflict[] }` | — |
| PUT `/v1/admin/host-claims/{group_key}` | IA | `{ winner_package_id, winner_app_id, fingerprint }` | 200 `HostClaimConflict` | 409 `stale_fingerprint` |
| GET/POST `/v1/admin/trusted-keys`; DELETE `/v1/admin/trusted-keys/{id}` | IA | POST `{ publisher, name, public_key }` | 200 `Page<TrustedKey>` / 201 / 204 (revoke: disables signed packages) | 400; 409 |
| GET `/v1/admin/registry/snapshot` | IA | — | 200 snapshot JSON + `x-snapshot-hash` | 404 |
| GET `/internal/v1/registry/active?contract_max=` | internal secret (port 4111) | — | 200 body = the **canonical snapshot bytes** (`application/json`); headers `x-snapshot-hash: <sha256 hex of body>`, `x-snapshot-contract: <n>` (C-18) | 404 (none active) |
| GET `/internal/v1/registry/blobs/{digest}` | internal secret | — | 200 `application/wasm` | 404 |

### M4 — conversations, turns, transcript, sandbox (API-004)

Routes/auth: `05 §Conversations and turns`, `§Agents and sandboxes` (sandbox, restart, models). Behaviour: `10 §Conversations`, `§Turns`, `§Transcript events`; SSE conventions `05 §Conventions`.

**Contract decisions**
- C-23 Transcript events are delivered as `{ seq: number | null, type, ts, payload }`: durable events (`10` table "Durable: yes") have `seq`; ephemeral (`text.delta`, `thinking.delta`, `activity`) have `seq: null` and are never returned by `/events`. Payloads are exactly the `10` table shapes (snake_case). Unknown types must be ignored by clients.
- C-24 SSE `GET /v1/conversations/{id}/stream?after_seq=`: each event is `event: <type>`, `id: <seq>` for durable events only, `data: <TranscriptEvent JSON>`; keepalive comment every `SSE_KEEPALIVE`; resume with `Last-Event-ID` or `after_seq`. Clients: fetch `/events?after_seq=` first, then open the stream from the last seq; discard a turn's deltas once its durable `text` arrives (`14 §Key flows`).
- C-26 Follow-ups: creating a follow-up appends a durable `turn.queued` with `turn_id = <followup id>` and extra `followup_of: <parent turn id>`; promotion appends a durable `turn.queued` for the new turn with extra `promoted_from: <followup id>` (clients render the message once). Ephemeral delta payloads use the `10` field names (`message_id`, never abbreviations).
- C-25 `POST /v1/conversations/{id}/messages` returns 202 `{ turn: Turn }` or `{ followup: Followup }` (exactly one non-null).

```ts
type ConversationKind = "direct" | "group" | "schedule" | "webhook" | "watch" | "peer" | "api";
interface Conversation { id: Uuid; agent_id: Uuid; kind: ConversationKind; external_ref: string; title: string | null;
  direct_user_id: Uuid | null; origin_conversation_id: Uuid | null; last_seq: number;
  active_turn: Turn | null; last_activity_at: Timestamp | null; created_at: Timestamp }
type TurnStatus = "queued" | "dispatched" | "running" | "done" | "failed" | "aborted";
interface Turn { id: Uuid; conversation_id: Uuid; status: TurnStatus; door: string /* dashboard|api|channel|schedule|webhook|watch|peer */;
  speaker_label: string | null; text: string; attachment_ids: Uuid[]; error_code: string | null;
  abort_requested: boolean; send_now_requested: boolean;
  usage: { input_tokens: number; output_tokens: number; cache_read_tokens: number | null; cost_usd: number | null } | null;
  created_at: Timestamp; started_at: Timestamp | null; finished_at: Timestamp | null }
interface Followup { id: Uuid; parent_turn_id: Uuid; status: "joining" | "joined" | "promoted"; text: string; created_at: Timestamp }
interface TranscriptEvent { seq: number | null; type: string; ts: Timestamp; payload: Record<string, unknown> }
interface SandboxView { agent_id: Uuid; state: "unprovisioned" | "starting" | "running" | "stopping" | "stopped" | "failed";
  boot_id: Uuid | null; runner_id: Uuid | null; image: string | null; started_at: Timestamp | null;
  keep_awake_reasons: string[]; failure_code: string | null; failure_message: string | null;
  processes: { id: string; name: string; state: string; busy: boolean; started_at: Timestamp | null }[];
  ports: { port: number; protocol: string }[]; restart_to_update: boolean }
interface ModelOption { endpoint_id: string; provider: string; model_id: string; display_name: string | null; dialects: string[] }
```

| Method & path | Auth | Request | 2xx response | Errors |
|---|---|---|---|---|
| GET `/v1/agents/{agent}/conversations` | S/K can_use | `?kind&limit&cursor` | 200 `Page<Conversation>` (ordered by last activity) | 404 |
| POST `/v1/agents/{agent}/conversations/direct` | S can_use | — | 200 `Conversation` (get-or-create caller's direct) | 404 |
| POST `/v1/agents/{agent}/conversations` | K | `{ external_ref, title? }` | 200/201 `Conversation` (kind api, get-or-create) | 400 |
| GET `/v1/conversations/{id}` | S/K can_use | — | 200 `Conversation` | 404 |
| GET `/v1/conversations/{id}/events` | S/K can_use | `?after_seq&limit` (≤500) | 200 `{ items: TranscriptEvent[], last_seq }` (durable only) | 404 |
| GET `/v1/conversations/{id}/stream` | S/K can_use | `?after_seq` / `Last-Event-ID` | SSE (C-24) | 404 |
| POST `/v1/conversations/{id}/messages` | S/K can_use | `{ text, attachment_ids? }` (≤ `TURN_TEXT_MAX`) | 202 (C-25) | 400; 413; 422 `hosted_disabled`, `too_many_attachments` (C-30) |
| POST `/v1/turns/{id}/abort`, `/v1/turns/{id}/send-now` | S/K can_use | — | 202 `Turn` | 404; 409 `turn_finished` |
| GET `/v1/agents/{agent}/sandbox` | S/K can_use | — | 200 `SandboxView` | 404 |
| POST `/v1/agents/{agent}/restart` | S/K can_use | — | 202 `SandboxView` | 404 |
| GET `/v1/agents/{agent}/models` | S/K can_use | — | 200 `{ items: ModelOption[] }` (only usable keys and allowed models, `08 §Model selection`) | 404 |

### M5 — attachments, action approvals, agent links, peer tasks (API-005)

Routes/auth: `05 §Conversations and turns` (attachments), `§Approvals` (action approvals), `§Memory, skills, schedules, webhooks, links`. Behaviour: `10 §Attachments`, `07 §Action approvals`, `§Agent links and peer tasks`; decision S-11; I-27 for reads.

**Contract decisions**
- C-27 App-turn cap reset: `POST /v1/conversations/{id}/continue` (S/K, `can_use`) sets `app_turn_streak = 0` and appends a durable `notice` (`code: "app_turn_cap_reset"`); 200 `Conversation`. Refused bot/agent turns append a durable `notice` with `code: "app_turn_cap_reached"`. `Conversation` gains `app_turn_streak: number` and `app_turn_capped: boolean`.
- C-28 Action-approval decision body mirrors gateway approvals: `{ decision: "approve" | "deny" }`; deciding a non-`pending` row → 409 `already_decided`; unknown `action_key` at decide time → 500 `internal` + error log (07 "fail loud").
- C-29 Link policy update: `PUT /v1/agent-links/{id}/policy` body `{ agent_id, policy: "ask" | "allow" | "blocked" }` sets **that agent's side**; caller needs `can_use` on that agent's workspace; the other side is read-only to them. Optimistic concurrency via `revision` (409 `stale_revision`).
- C-30 Attachments: upload response and list items use `Attachment`; downloads are `application/octet-stream` + `Content-Disposition: attachment` + `nosniff` regardless of stored type. Error codes (BE-064): upload over `ATTACH_IN_FILE_MAX` → 413 `payload_too_large`; a message binding more than `ATTACH_IN_COUNT` attachments → 422 `too_many_attachments`; runner plane `POST /runner/attachments` over `ATTACH_OUT_FILE_MAX` → 413 `payload_too_large`, over `ATTACH_OUT_COUNT` per turn → 429 `attachment_count_exceeded`, over `ATTACH_OUT_DAILY_PER_CONVERSATION` → 429 `attachment_quota_exceeded`, stale boot → 409 `fenced`, turn not dispatched/running → 409 `turn_not_active`. Outbound attachments are stored as `application/octet-stream`.

```ts
interface Attachment { id: Uuid; conversation_id: Uuid; turn_id: Uuid | null; direction: "in" | "out";
  status: "pending" | "bound" | "failed"; filename: string; content_type: string; size_bytes: number;
  caption: string | null; created_at: Timestamp }
type ActionApprovalStatus = "pending" | "approved" | "executed" | "failed" | "rejected" | "expired";
interface ActionApproval { id: Uuid; workspace_id: Uuid; agent_id: Uuid; conversation_id: Uuid | null; turn_id: Uuid | null;
  action_key: string; summary: string /* platform-composed, never raw model text */; status: ActionApprovalStatus;
  decided_by: { id: Uuid; display_name: string } | null; decided_at: Timestamp | null; executed_at: Timestamp | null;
  error: string | null; expires_at: Timestamp; created_at: Timestamp }
type LinkPolicy = "ask" | "allow" | "blocked";
interface AgentLink { id: Uuid; revision: number;
  self: { agent_id: Uuid; policy: LinkPolicy }; peer: { agent_id: Uuid; handle: string; name: string; workspace_id: Uuid; policy: LinkPolicy };
  created_at: Timestamp; updated_at: Timestamp }
type PeerTaskStatus = "queued" | "open" | "completed" | "cancelled" | "expired";
interface PeerTask { id: Uuid; link_id: Uuid; from_agent_id: Uuid; to_agent_id: Uuid; direction: "outgoing" | "incoming";
  requester: { id: Uuid; display_name: string } | null; origin_conversation_id: Uuid; task_conversation_id: Uuid;
  status: PeerTaskStatus; messages_used: number; message_budget: number; report: string | null;
  expires_at: Timestamp; created_at: Timestamp; updated_at: Timestamp }
```

| Method & path | Auth | Request | 2xx response | Errors |
|---|---|---|---|---|
| POST `/v1/conversations/{id}/attachments` | S/K can_use | multipart `file` (≤ `ATTACH_IN_FILE_MAX`) | 201 `Attachment` (pending) | 404; 413 |
| GET `/v1/attachments/{id}/content` | S/K can_use | — | 200 bytes (C-30) | 404 |
| POST `/v1/conversations/{id}/continue` | S/K can_use | — | 200 `Conversation` (C-27) | 404 |
| GET `/v1/workspaces/{ws}/action-approvals` | S/K can_use | `?status&agent_id&limit&cursor` | 200 `Page<ActionApproval>` | 404 |
| POST `/v1/action-approvals/{id}/decision` | S/K can_use (on the row's workspace) | `{ decision }` (C-28) | 200 `ActionApproval` | 404; 409 `already_decided` |
| GET `/v1/agents/{agent}/links` | S/K can_use | — | 200 `{ items: AgentLink[] }` | 404 |
| PUT `/v1/agent-links/{id}/policy` | S/K can_use (C-29) | `{ agent_id, policy, revision }` | 200 `AgentLink` | 404; 409 `stale_revision` |
| GET `/v1/agents/{agent}/peer-tasks` | S/K can_use | `?status&limit&cursor` | 200 `Page<PeerTask>` | 404 |

### M6 — channels management, connector admin (API-006)

Routes/auth: `05 §Channels (management)`, `§Instance administration` (connectors). Behaviour: `13 §3`, `§5.3`, `§5.7`–`§5.9`; connector plane wire shapes are `aurora-protocol::connector` (CON-001) and are not repeated here. Permissions per `04` + I-27: integrations/user links (admin) → org owner/admin; presences, links listing, peer views → `can_use`; space bindings and reach decisions → `can_own`.

**Contract decisions**
- C-31 Connector provisioning: `POST /v1/admin/connectors { id, display_name }` → 201 `{ connector: ConnectorAdmin, token }` (`aun_…`, shown once); `POST …/{id}/token` rotates → `{ token }`; `POST …/{id}/approve { manifest_digest }` approves exactly the pending manifest (409 `stale_manifest` if it changed); `DELETE …/{id}` → 409 `connector_in_use` while integrations exist.
- C-32 `GET /v1/connectors` lists only `active` connectors for any signed-in caller; icons via `GET /v1/connectors/{id}/icon` (sanitized PNG, app-asset rules). Manifest `setup_guide_md` is returned raw and rendered client-side with the existing safe markdown renderer.
- C-33 Integration create is synchronous: the row is inserted `validating`, a correlated `credentials.validate` runs (10 s); success → 201 `active`; connector `invalid`/`forbidden`/`credential_dead` → row removed, 422 `credentials_invalid` (sanitized connector message); timeout → row removed, 503 `connector_unavailable`; tenant owned by another org → row removed, 409 `tenant_already_bound`. Credentials are write-only (never returned); `PATCH` may replace `config`/`credentials` (re-validated the same way) or set `status: "active"|"disabled"`.
- C-34 Presence attach `{ integration_id, mode, fields? }`: `shared` → 201 `active` (422 `no_shared_bot` without one); `dedicated` → 201 `pending_setup` + `presence.provision` op, or, when the manifest has `presence_setup.dedicated.fields`, the fields are validated synchronously like C-33. `POST /v1/presences/{id}/reattach` re-runs provisioning/validation.
- C-35 Reach decision `POST /v1/reach-grants/{id}/decision { decision: "approve"|"block", listen?: "mentions"|"all", revision }` (`can_own`); 409 `stale_revision`.
- C-36 `POST /v1/channel-links/confirm { token }` (session only, SO) consumes a `link_requests` token (`aui_…`, 30 min, single use) for the signed-in user → 200 `UserLink`; expired/used → 410 `link_expired`; the platform user already linked to another Aurora user → 409 `already_linked`.
- C-38 Correlated operations (BE-071, 13 §4.2): when the connector answers a correlated op with a neutral error, management routes that waited on it map `invalid`/`forbidden`/`credential_dead` → 422 `credentials_invalid` (C-33) and other codes → 502 `connector_error` (sanitized message); no answer within the wait → 503 `connector_unavailable`. Connector plane: results from a replica that did not claim the op → 403; unknown op → 404; a repeated result is an idempotent no-op; `file.fetch` payloads must name `conversation_id` (422 `op_context_missing`).
- C-39 Presence options (FE-060 gap): `GET /v1/agents/{agent}/presence-options` (S/K `can_use`) → `{ items: PresenceOption[] }` listing the agent's org integrations with status `active`, **metadata only**: `{ integration_id, connector_id, connector_name, tenant_label, modes: ("shared"|"dedicated")[] /* per connector capabilities and integration `_capabilities`; shared only with a shared bot */, presence_setup: { dedicated?: { fields: FormField[] } }, attached: boolean }`. Never config or credentials. This is how `can_use` members attach presences (04: attach is `can_use`) without reading the owner/admin integration list (I-27).
- C-40 `ConnectorAdmin` gains `approved_manifest: Record<string, unknown> | null` (the last approved manifest, icon stripped, from BE-070's private `_approved` envelope) so the admin approval view can show a real capability/setup-field diff (14 §Instance administration).
- C-41 Agent avatars (BE-067; 10 §agent fields, 05, 17 SEC-30): `PUT /v1/agents/{agent}/avatar` (S/K `can_use`) takes the **raw image bytes as the request body** (not multipart; the client's `Content-Type` is ignored: PNG/JPEG/WebP is sniffed from the bytes) → 200 `Agent`; body > `AVATAR_MAX` (512 KiB) → 413 `payload_too_large`; anything else, corrupt data, or an image over 4096×4096 / 64 MiB decoded → 422 `avatar_invalid`. The image is cover-cropped and re-encoded as a fresh 256×256 PNG (no input byte or metadata is kept), stored via the BlobStore, `avatar_public_key` rotated (32 random bytes, base64url, 43 chars) and the previous blob deleted. `DELETE /v1/agents/{agent}/avatar` → 204 (removes blob and key; added beyond 05). `GET /v1/agents/{agent}/avatar` → 200 `image/png` (404 `no_avatar` when unset, so "agent not visible" stays distinguishable). `GET /v1/public/avatars/{key}` is public (class P): 200 `image/png` + `X-Content-Type-Options: nosniff` + `Cache-Control: public, max-age=3600` only for the *current* key; old, unknown or malformed keys → identical 404. `Agent.avatar_url: string | null` is the **root-relative** path `/v1/public/avatars/{key}`; the connector config feed `Presence.agent.avatar_url` and the `presence.set_icon` payload `{ presence_id, avatar_url }` (null on removal) carry the **absolute** URL (`server.external_url` + path). `presence.set_icon` is queued for each *active dedicated* presence of the agent on set and remove.
- C-42 `Connection` gains `missing_scopes: string[]` (scopes requested at authorize time and not granted; `[]` when none or for connections created before I-33). The OAuth callback page's `aurora:connected` message also carries `missing_scopes: string[]` (the window stays open with a warning when non-empty). `ConnectRequest.fields` is also accepted for `oauth2` methods that declare `fields` (S-14).
- C-43 Verified-email auto-link opt-in (RFE-001, D-43; 13 §5.1 step 4/§5.9, 05, 17 SEC-19/20): no new field, the setting lives in the free-form integration `config`: `config.verified_email_autolink` (boolean, default `false`/absent, set on `POST /v1/orgs/{org}/channel-integrations` or `PATCH /v1/channel-integrations/{id}` by org owner/admin only — the routes already require it). A non-boolean value → 400 `invalid_request` (`/config/verified_email_autolink`). `true` is refused with 422 `autolink_unsupported` (a create removes the row, like a failed validation) unless the integration's effective capabilities (manifest narrowed by `config._capabilities`) include `email_verified`. A `PATCH` replaces `config` as a whole, so omitting the key turns it off. Each change is audited (`channel_integration.autolink_enable` / `.autolink_disable`). With it off the core never creates a `verified_email` link: the sign-in-link flow runs and `link_requests.platform_email_hint` stores the asserted email for pre-fill only.
- C-37 DM defaults: `GET /v1/me/dm-defaults` → items for every (integration, platform user) linked to the caller; `PUT /v1/me/dm-defaults { integration_id, agent_id }` requires a user link on that integration and that the caller can reach the agent (`can_use` or approved person reach) with an active shared presence there (422 `agent_not_reachable`).

```ts
type ConnectorStatus = "awaiting_registration" | "pending_approval" | "active" | "disabled";
interface ConnectorAdmin { id: string; display_name: string; status: ConnectorStatus; version: string | null; contract: number | null;
  manifest: Record<string, unknown> | null; manifest_digest: string | null; approved_manifest_digest: string | null;
  widening: string[] /* human-readable reasons pending approval is required */; replicas: { id: Uuid; version: string; last_seen_at: Timestamp }[];
  integration_count: number; approved_at: Timestamp | null; created_at: Timestamp }
interface ConnectorInfo { id: string; display_name: string; icon_url: string; setup_guide_md: string | null;
  capabilities: Record<string, unknown>; integration_setup: { fields: FormField[] }; presence_setup: { dedicated?: { fields: FormField[] } } }
type IntegrationStatus = "validating" | "active" | "needs_attention" | "disabled";
interface ChannelIntegration { id: Uuid; org_id: Uuid; connector_id: string; tenant_ref: string; tenant_label: string;
  config: Record<string, unknown> /* without credentials; `_capabilities` included */; shared_bot: { handle: string; display_name: string } | null;
  status: IntegrationStatus; status_reason: string | null; created_at: Timestamp; updated_at: Timestamp }
interface Presence { id: Uuid; agent_id: Uuid; integration_id: Uuid; connector_id: string; tenant_label: string; mode: "dedicated" | "shared";
  handle: string | null; status: "pending_setup" | "active" | "disabled" | "needs_attention"; status_reason: string | null; created_at: Timestamp }
interface SpaceBinding { id: Uuid; integration_id: Uuid; space_ref: string; space_label: string; agent_id: Uuid;
  listen: "mentions" | "all"; followups_need_mention: boolean; active: boolean; bound_by: { id: Uuid; display_name: string } | null; created_at: Timestamp }
interface UserLink { id: Uuid; integration_id: Uuid; connector_id: string; tenant_label: string; platform_user_ref: string;
  user: { id: Uuid; display_name: string }; method: "verified_email" | "sign_in_link" | "admin"; created_at: Timestamp }
interface DmDefault { integration_id: Uuid; platform_user_ref: string; agent_id: Uuid | null }
interface ReachGrant { id: Uuid; agent_id: Uuid; integration_id: Uuid; subject_kind: "space" | "person"; subject_ref: string; subject_label: string;
  state: "pending" | "approved" | "blocked" | "left" | "expired"; listen: "mentions" | "all"; revision: number;
  decided_by: { id: Uuid; display_name: string } | null; decided_at: Timestamp | null; expires_at: Timestamp | null; created_at: Timestamp }
```
`FormField` is the app connect-field schema already in the generated client (`09 §3.4`).

| Method & path | Auth | Request | 2xx response | Errors |
|---|---|---|---|---|
| GET `/v1/connectors`; GET `/v1/connectors/{id}/icon` | S/K | — | 200 `{ items: ConnectorInfo[] }`; PNG (C-32) | 404 |
| GET/POST `/v1/admin/connectors`; DELETE `/v1/admin/connectors/{id}` | IA | `{ id, display_name }` | 200 `{ items: ConnectorAdmin[] }`; 201 (C-31); 204 | 409 `connector_in_use`, `conflict` |
| POST `/v1/admin/connectors/{id}/approve`; `…/{id}/token` | IA | `{ manifest_digest }`; — | 200 `ConnectorAdmin`; 200 `{ token }` | 404; 409 `stale_manifest` |
| GET/POST `/v1/orgs/{org}/channel-integrations` | S/K owner/admin | `{ connector_id, config, credentials }` | 200 `Page<ChannelIntegration>`; 201 (C-33) | 409 `tenant_already_bound`; 422 `credentials_invalid`, `autolink_unsupported` (C-43); 503 `connector_unavailable` |
| GET/PATCH/DELETE `/v1/channel-integrations/{id}` | S/K owner/admin | `{ config?, credentials?, status? }` | 200 `ChannelIntegration`; 204 (deprovisions presences via ops) | 404; 422; 503 |
| GET/POST `/v1/channel-integrations/{id}/user-links`; DELETE `/v1/user-links/{id}` | S/K owner/admin | `{ platform_user_ref, user_id }` | 200 `Page<UserLink>`; 201 `UserLink`; 204 | 404; 409 `already_linked` |
| GET/POST `/v1/agents/{agent}/presences` | S/K can_use | `{ integration_id, mode, fields? }` | 200 `{ items: Presence[] }`; 201 (C-34) | 404; 409 `conflict`; 422 `no_shared_bot`, `mode_unsupported`, `credentials_invalid` |
| DELETE `/v1/presences/{id}`; POST `/v1/presences/{id}/reattach` | S/K can_use | — | 204; 200 `Presence` | 404 |
| GET `/v1/agents/{agent}/space-bindings`; PATCH/DELETE `/v1/space-bindings/{id}` | S/K can_own | `{ listen?, followups_need_mention? }` | 200 `{ items: SpaceBinding[] }`; 200 `SpaceBinding`; 204 | 404 |
| GET `/v1/me/channel-links`; DELETE `/v1/me/channel-links/{id}` | S | — | 200 `{ items: UserLink[] }`; 204 | 404 |
| POST `/v1/channel-links/confirm` | S (SO) | `{ token }` | 200 `UserLink` (C-36) | 409; 410 `link_expired` |
| GET/PUT `/v1/me/dm-defaults` | S | `{ integration_id, agent_id }` | 200 `{ items: DmDefault[] }`; 200 `DmDefault` (C-37) | 404; 422 `agent_not_reachable` |
| GET `/v1/agents/{agent}/reach`; POST `/v1/reach-grants/{id}/decision` | S/K can_use (list), can_own (decide) | (C-35) | 200 `Page<ReachGrant>`; 200 `ReachGrant` | 404; 409 `stale_revision` |

### Phase 1 — RFE-009 (direct conversation visibility, D-47)

Rule: `can_see_conversation` (`04 §The access law`), `05 §Conversation visibility`. No route is added or removed; these are the wire changes.

- C-44 **`direct_conversation_forbidden`**: `POST /v1/conversations/{id}/messages`, `POST /v1/conversations/{id}/attachments` and `POST /v1/conversations/{id}/continue` on a `direct` conversation by a caller who can see it but is not its user (a workspace owner, org owner/admin, or a key of one) answer **403** `{ "error": { "code": "direct_conversation_forbidden" } }`. A caller who cannot see the conversation still gets **404** (checked first). Owners keep read, `abort` and `send-now` on the conversation's turns (202).
- C-45 **`Conversation.direct_user`**: `{ id: uuid, display_name: string } | null`, always present. Non-null only on `direct` conversations returned by `GET /v1/agents/{agent}/conversations` to a caller who is not that conversation's user (so: owners); the list returns every direct conversation of the agent to owners and only the caller's own to everyone else. `GET /v1/conversations/{id}`, `…/direct` and `…` create responses carry `null`.
- C-46 **Redacted references**: `Schedule.origin_conversation_id` becomes `uuid | null` and `PeerTask.origin_conversation_id` becomes `uuid | null`; both gain `conversation_hidden: bool` (always present). When the referenced conversation is a `direct` conversation the caller cannot see, the id is `null` and `conversation_hidden` is `true`; otherwise the id is unchanged and the flag is `false`. (Webhook, reach and channel-card DTOs carry no conversation id.)
- C-47 **Omission** (no schema change): `GET /v1/workspaces/{ws}/approvals`, `…/action-approvals`, `GET /v1/approvals/pending` (long-poll, incl. wake-ups), `GET /v1/approvals/stream` (SSE), `POST /v1/approvals/batches/{batch_id}/decision` and `GET /v1/workspaces/{ws}/activity` omit rows whose `conversation_id` is a `direct` conversation the caller cannot see (also when filtered by `conversation_id=`); `POST /v1/approvals/{id}/decision` and `POST /v1/action-approvals/{id}/decision` on such a row answer **404**. Channel card clicks on such an approval are refused (`forbidden`) before the token is consumed. `GET /v1/conversations/{id}/stream` ends at the next resync once the viewer stops being able to see the conversation.
- C-48 **`ActionApproval.conversation_hidden`**: `bool`, always present. When the approval's workspace differs from its conversation's workspace, `conversation_id` and `turn_id` are `null` and `conversation_hidden` is `true` (D-47 clarification C); the frozen payload is never in any DTO. Within the same workspace the RFE-009 predicate (C-47) applies unchanged.
- Audit: an owner (not the direct user) reading a direct conversation (`GET /v1/conversations/{id}`, `…/events`, `…/stream`, `GET /v1/attachments/{id}/content`) writes `conversation.owner_view` (target `conversation`, best-effort, once per actor, conversation and hour).

### Phase 2 — RFE-007 (signing-key / internal-secret rings, D-51)

Rule: `08 §4`, `13 §4.4`, `06 §Ports`, SEC-46. No public route changes; the OpenAPI document and the permission matrix are unchanged. These are the wire changes on the internal surfaces.

- C-49 **`X-Aurora-Key-Id`** (internal calls: gateway/hook host/API `/internal/v1/**`): senders put the active internal-secret ring id in this header next to `Authorization: Bearer <base64 secret>`. The receiver compares in constant time against the ring entry of that id; an id outside its ring or a secret that does not match that entry answers **401**. With the header absent (older sender) the receiver tries every entry of its ring. Responses are unchanged.
- C-50 **`GET /internal/v1/status`** (API internal listener 4111, gateway 4103, hook host 4104; same internal auth as C-49): `200 { service: "api"|"gateway"|"hookhost", internal_secret: { active: string, keys: [{ id: string, fingerprint: string }] }, signing_key?: { active, keys }}`. `signing_key` is present on the API only. `fingerprint` is 8 hex characters of SHA-256 of the entry; key values are never returned. 401 `unauthenticated`/`unauthorized` without a valid ring key.
- C-51 **Action token wire format** (channel buttons, `13 §4.4`): new tokens are `v2.<key_id>.<b64url(payload)>.<b64url(HMAC)>` signed with the active signing key; `v1.<payload>.<HMAC>` tokens still verify by trying each ring key. Payload JSON is unchanged. A `v2` token naming an id outside the verifier's ring is refused like a bad signature. Sealed OAuth/OIDC state values are prefixed `<key_id>.` (cookie and `state` parameter); unprefixed values are tried against each key.
- CLI (not HTTP): `aurora-api keys generate --kind signing|internal [--id <key_id>]` prints one `<key_id>:<base64>` entry; `aurora-api keys status` prints each ring's ids, fingerprints and active id from the process environment.

### Phase 2 — RFE-017 (management API rate limiting, D-57)

Rule: `05 §Conventions` "Rate limiting", SEC-50. No route is added or removed; the wire changes are the refusal shapes and headers below.

- C-52 **`429 rate_limited`** on any authenticated `/v1/**` route (exempt: `/healthz`, `/readyz`, `/metrics`, `/internal/*`, `/v1/runner/*`, `/v1/connector/*`, `POST /v1/hooks/{token}`): body `{ "error": { "code": "rate_limited", "message", "details": { "scope": "general"|"class"|"org", "class": "general"|"write"|"message"|"upload"|"simulate"|"search"|"export"|"org"|"unauth", "limit": number, "per": "minute"|"hour", "retry_after_secs": number } } }`; headers `retry-after` (whole seconds, >= 1), `ratelimit-limit`, `ratelimit-remaining` (always `0` on a refusal), `ratelimit-reset` (= `retry-after`), `x-should-retry: true`. `class` is `unauth` and `scope` is `general` for the per-IP limits of unauthenticated traffic (120/min public GETs; 30/min for requests carrying a rejected bearer). The principal is `key:<api_key_id>`, `user:<user_id>` (all of the user's sessions) or `agent:<agent_id>`; a refused request consumes no token from any bucket.
- C-53 **`429 too_many_streams`**: `GET /v1/conversations/{id}/stream`, `GET /v1/approvals/stream` and any GET with `?wait=<n>` (n > 0, long-poll) beyond 20 concurrent per principal answer `{ "error": { "code": "too_many_streams", "message", "details": { "scope": "streams", "limit": 20 } } }` with no `retry-after`; a slot is released when the response body ends or the client disconnects. Streams that are refused by authorization (non-2xx) release at once.

### Phase 1 — RFE-002 (TLS-only gateway proxy listener, D-44)

- C-54 `ContainerConfig` (`GET /v1/agents/{agent}/container-config`, `GET /v1/agent/container-config`, the `container_config` of `POST /v1/workspaces/{ws}/agents` and `POST /v1/agents/{agent}/token`; 08 §10) gains `proxy_ca_path: string` (the file that holds the trust anchor for the HTTPS proxy leg: curl `--proxy-cacert`, git `http.proxySSLCAInfo`; same value as `ca_path`) and `proxy_tls: "operator_certificate" | null` (`"operator_certificate"` when `gateway.proxy_tls.cert_file` is configured, so the outer leg is verified with the system roots; `null` when the gateway CA issued the listener certificate). `proxy_url` and the `HTTPS_PROXY`/`HTTP_PROXY`/`https_proxy`/`http_proxy` env values are always `https://x:<token>@host:4102` (an `http://` `gateway.advertised_proxy_url` is refused at startup); with a CA-issued certificate `env` also carries `GIT_CONFIG_COUNT=1`, `GIT_CONFIG_KEY_0=http.proxySSLCAInfo`, `GIT_CONFIG_VALUE_0=<proxy_ca_path>`. Clients must ignore unknown fields.
- C-55 `ContextCredential.proxy_url` (`POST /v1/agent/context-credentials`; 08 §10) is `https://…` for the same reason; no other field changes. The hosted sandbox boot document (11 §The boot document, not an API route) carries `gateway: { proxy, tls: true, server_name, boot_token }`; the supervisor refuses any other `tls` value.

### W9 hook-host JSON encoding (C-18, C-19)

- C-18 Internal snapshot route returns the exact canonical bytes so consumers verify `sha256(body) == x-snapshot-hash` without re-serialization.
- C-19 `POST /internal/v1/hooks/{connect.authorize|connect.exchange|token.mint|approval.summarize}` (`Authorization: Bearer <internal secret>`, I-20) body `{ package_id, package_digest, app_id, input }`; `input`/`ok` objects use the WIT record field names in **snake_case**; `list<kv>` = `[{ "key", "value" }]`; `option<T>` = value or `null` (fields always present); `list<u8>` = JSON array of integers 0–255; `u64` timestamps = integer Unix seconds; variants = single-key objects (`exchange-source`: `{"oauth_callback": [kv]}` \| `{"form": [kv]}`; `exchange-output`: `{"connected": {...}}` \| `{"continue": {"url", "continuation"}}`). Errors: `{ "error": { "code", "message" } }` with codes `unsupported`, `invalid_input`, `upstream`, `reauth_required`, `internal`, `timeout`, `memory`, `output_too_large`, `egress_denied`. Implemented by `services/aurora-hookhost/src/wire.rs`; clients: gateway `mint.rs`, API connect hooks (BE-043), gateway summarize (HOOK-004).

## Shared schemas

_Defined alongside the milestone sections above._
