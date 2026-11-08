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
| POST `/v1/auth/signup` | none | `{ email, password, display_name, invitation_token? }` | 201 `{ user: User }` + cookie. `open`: also creates `<display_name>'s org` + default workspace (+Daneel) | 400 `invalid_request` (password 12–256, common list, email, display_name 1–80); 403 `signup_closed`; 409 `email_taken`; 429 | _**Removed in phase 1 (RFE-004): 404, C-150.**_
| POST `/v1/auth/login` | none | `{ email, password }` | 200 `{ user: User }` + cookie | 401 `invalid_credentials`; 403 `account_disabled`; 429 `rate_limited` | _**Removed in phase 1 (RFE-004): 404, C-150.**_
| POST `/v1/auth/logout` | S | — | 204, cookie cleared | — |
| POST `/v1/auth/verify-email` | none | `{ token }` | 200 `{ user: User }` | 400 `invalid_token` (unknown/used/expired) | _**Removed in phase 1 (RFE-004): 404, C-150.**_
| POST `/v1/auth/verify-email/resend` | S | — | 202 | 409 `already_verified`; 429 | _**Removed in phase 1 (RFE-004): 404, C-150.**_
| POST `/v1/auth/password/forgot` | none | `{ email }` | 202 (always) | 429 | _**Removed in phase 1 (RFE-004): 404, C-150.**_
| POST `/v1/auth/password/reset` | none | `{ token, password }` | 204 (all sessions revoked) | 400 `invalid_token`, `invalid_request` | _**Removed in phase 1 (RFE-004): 404, C-150.**_
| POST `/v1/auth/password/change` | SO | `{ current_password, new_password }` | 204 | 401 `invalid_credentials`; 400 `invalid_request` | _**Removed in phase 1 (RFE-004): 404, C-150.**_
| POST `/v1/auth/sessions/revoke-others` | SO | — | 200 `{ revoked: number }` | — |
| GET `/v1/auth/oidc/{provider}/start?return_to=` | none | same-origin path | 302 provider | 404 unknown provider; 400 bad `return_to` | _Phase 1 (RFE-004): also `reauth=1`, `login_hint=`; 401 for `reauth=1` without a session; 429 `rate_limited`, C-151._
| GET `/v1/auth/oidc/{provider}/callback` | none | provider params | 302 `return_to` + cookie | 302 `/auth/login?error=<code>` (`oidc_state_invalid`, `oidc_failed`, `signup_closed`, `email_unverified`, `email_domain_not_allowed`) | _Phase 1: also `email_unverified` always, `user_disabled`, `reauth_user_mismatch`, `reauth_stale`; 429; C-151._
| POST `/v1/auth/cli` | none | `{ hostname? }` | 200 `{ device_code, user_code: "ABCD-EFGH", verification_url, interval: 5, expires_in: 600 }` | 429 |
| POST `/v1/auth/cli/token` | none | `{ device_code }` | 200 `{ api_key: string, workspace: {id, slug, name, org_id, org_slug} }` (once) | 400 `authorization_pending` \| `slow_down` \| `expired_token` \| `access_denied` \| `invalid_token` |
| POST `/v1/auth/cli/approve` | SO | `{ user_code, workspace_id }` | 204 | 404 `not_found` (unknown/expired code); 404 workspace not visible |
| GET `/v1/me` | S | — | 200 `Me` | 401 |
| PATCH `/v1/me` | S | `{ display_name?, default_org_id? }` | 200 `Me` | 400; 404 org not a membership |
| DELETE `/v1/me` | SO | `{ password? }` | 204 | 403 `reauthentication_required`; 409 `last_owner` | _**Phase 1 (RFE-004): no body; 412 `reauth_required` replaces 403 `reauthentication_required`, C-152.**_
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

### Phase 2 — RFE-012 (approver workspace role, D-55)

Rule: `04 §Roles`, `04 §The access law` (`can_approve`), `05 §Approvals`, SEC-28. No route is added or removed; these are the wire changes.

- C-60 **`WorkspaceRole`** (`Workspace.my_role`, `GET /v1/me` `memberships[].workspaces[].my_role`, `AccessBinding.role`, `PUT /v1/workspaces/{ws}/access/{user}` body `role`): the enum gains **`approver`** → `"owner" | "approver" | "member"`. `PUT …/access/{user}` (`can_own`) accepts all three. `Workspace.my_role` in `GET /v1/orgs/{org}/workspaces` is the caller's effective role (not a constant `member`). Existing rows are unchanged by the migration.
- C-61 **`approver_required`**: every approvals route needs `can_approve` on the approval's workspace, otherwise **403** `{ "error": { "code": "approver_required" } }` — `GET /v1/workspaces/{ws}/approvals`, `GET /v1/workspaces/{ws}/action-approvals`, `POST /v1/approvals/{id}/decision`, `POST /v1/action-approvals/{id}/decision`, `POST /v1/approvals/batches/{batch_id}/decision` (a plain member who can see the batch). Invisible workspaces, approvals and direct conversations the caller cannot see (C-47) still answer **404** first. `GET /v1/approvals/pending` and `GET /v1/approvals/stream` carry no error: they list/emit only rows of workspaces where the caller `can_approve` (a plain member gets an empty list / no events).
- C-62 **`Invitation.approver_workspace_ids`**: `uuid[]`, always present (default `[]`); `POST /v1/orgs/{org}/invitations` accepts `approver_workspace_ids?: uuid[]` next to `workspace_ids` (bound as `member`). The two sets must be disjoint (400 `invalid_request`, field `/approver_workspace_ids`) and every id must belong to the org (400, as for `workspace_ids`). Invitations never grant `owner`. On accept, `AcceptInvitationResponse.workspace_ids` lists every bound workspace (members and approvers).
- C-63 **JIT** (config, not HTTP): `auth.oidc_providers[].jit.workspace_role: "member" | "approver"` (default `member`) is the role JIT binds the user to every `workspace_slugs` entry with; anything else fails config validation.
- C-64 **Channel decisions** (no wire change): approval, batch, action-approval and agent-link-decision clicks, and `approve|deny <code>`, need `can_approve`; a linked user who lacks it gets the ephemeral "Only approvers can decide this." (result `refused`/`approver_required`), the token is not consumed and the card stays pending. Cards carry "Needs an approver" in the context line.

### Phase 1 — RFE-002 (TLS-only gateway proxy listener, D-44)

- C-54 `ContainerConfig` (`GET /v1/agents/{agent}/container-config`, `GET /v1/agent/container-config`, the `container_config` of `POST /v1/workspaces/{ws}/agents` and `POST /v1/agents/{agent}/token`; 08 §10) gains `proxy_ca_path: string` (the file that holds the trust anchor for the HTTPS proxy leg: curl `--proxy-cacert`, git `http.proxySSLCAInfo`; same value as `ca_path`) and `proxy_tls: "operator_certificate" | null` (`"operator_certificate"` when `gateway.proxy_tls.cert_file` is configured, so the outer leg is verified with the system roots; `null` when the gateway CA issued the listener certificate). `proxy_url` and the `HTTPS_PROXY`/`HTTP_PROXY`/`https_proxy`/`http_proxy` env values are always `https://x:<token>@host:4102` (an `http://` `gateway.advertised_proxy_url` is refused at startup); with a CA-issued certificate `env` also carries `GIT_CONFIG_COUNT=1`, `GIT_CONFIG_KEY_0=http.proxySSLCAInfo`, `GIT_CONFIG_VALUE_0=<proxy_ca_path>`. Clients must ignore unknown fields.
- C-55 `ContextCredential.proxy_url` (`POST /v1/agent/context-credentials`; 08 §10) is `https://…` for the same reason; no other field changes. The hosted sandbox boot document (11 §The boot document, not an API route) carries `gateway: { proxy, tls: true, server_name, boot_token }`; the supervisor refuses any other `tls` value.

### Phase 1 — RFE-003 (open lane: posture, CONNECT-time rules, browser capability, D-45)

Rule: `06 §Open-lane admission and posture`, `07 §CONNECT-time evaluation`, `08 §11`, `11 §The boot document`, SEC-41. No route is added or removed.

- C-80 **Org governance document** (`GET/PUT /v1/orgs/{org}/governance`, `08 §11.2`): `egress.open_lane` = `"enabled" | "allowlist_only" | "disabled"` (absent on write = the instance minimum; absent in a stored phase-0 document = `enabled`). Looser than `governance.egress.open_lane_minimum` → 422 `governance_below_instance_minimum` (`details.fields[].path = "egress.open_lane"`); any other value → 422 `governance_invalid`.
- C-81 **`GET /v1/orgs/{org}/governance/effective`** gains `egress.open_lane: { value: "enabled"|"allowlist_only"|"disabled", source: "instance"|"org" }` (strictest of instance minimum and org). **`GET /v1/governance/instance`** (instance view) gains `egress.open_lane_minimum: string`.
- C-82 **Workspace policy** (`PolicyGeneration` and `PolicyGenerationSummary` of `GET …/policy`, `…/policy/generations`): new always-present `open_lane_posture: "enabled"|"allowlist_only"|"disabled"` (always `enabled` at org scope). `PUT …/policy/draft` accepts optional `open_lane_posture` (absent = `enabled`, whole-draft replace); at org scope any value other than `enabled` → 422 `policy_invalid` (`/open_lane_posture`). A workspace can only tighten: the effective posture is computed, never stored. Publishing invalidates the `workspace` scope in gateways.
- C-83 **`POST …/policy/simulate`** accepts `lane: "request" | "open"` (default `request`). `open`: `url` must be `https://host[:port]` (else 400 `invalid_request` on `/url`), `method`/`body` are ignored, no credential is selected (`winning_credential: null`, `candidates: []`) and the result applies the open-lane posture, then CONNECT-time rule semantics. New `reason: "open_lane"` (guidance `open_lane_disabled`); `allowlist_only` misses give `reason: "egress_denied"`, guidance `blocked_by_egress_policy`; a `require_approval` outcome is reported as `verdict: "block"`, guidance `blocked_by_policy` (a tunnel is never held). Not simulated: the credential capability and the governed-LLM-host refusal.
- C-84 **Gateway refusals on the open lane** (proxy, not `/v1`): 403 `open_lane_not_permitted` (credential without `browser`); 403 `open_lane_disabled` (`details.manage_url`); 403 `blocked_by_egress_policy` with `details.open_lane = true` (`allowlist_only` not satisfied); 403 `blocked_by_policy` with `details.connect_time = true` for a block rule, and `details.reason = "approval_required"` for a `require_approval` rule (replaces phase 0 `manual_approval_denied` on tunnels); 429 for an exhausted rate limit.
- C-85 **Runner registration** (`POST /v1/runner/register`, `16 §W5`): `capabilities.image_capabilities?: string[]` (open vocabulary, phase 1: `browser`; absent/empty = none), read from the image label `org.aurora.capabilities` by the runner. Backends that cannot read image labels (kubernetes) report none, so their sandboxes get no open lane. `schemas/runner_register_request.json` is regenerated.
- C-86 **Boot document** (`AURORA_BOOT`, not an API route): `capabilities: string[]` (absent = none). The API sets it to `image_capabilities ∩ {browser unless the effective open-lane posture is disabled for the workspace}` and mints the `aub_` boot credential (`gateway_credentials.capabilities`) with the same set; `agent` and `context` credentials never hold capabilities. `SandboxView.restart_to_update` is also `true` when the running boot's capabilities differ from what a boot would get now.

### Phase 2 — RFE-010 (conversation archive, delete, redaction, retention, export, D-53)

Rule: `10 §Archive, delete, redaction, retention, export`, `04 §The access law`, `12 §Session purge`, SEC-47. Routes are listed in `05 §Conversations`. A conversation the caller cannot see (C-47) is always **404** first; a visible conversation the caller may not act on is **403** `forbidden`.

- C-120 **Archive**: `POST /v1/conversations/{id}/archive` and `/unarchive` (no body) → 200 `Conversation`; allowed for anyone who can see the conversation (`can_see_conversation`; every workspace member satisfies `can_use`). `Conversation.archived_at: string|null` is new and always present. `GET /v1/agents/{agent}/conversations` takes `archived=true|false` (default `false`): archived conversations are hidden from the default list and only listed with `archived=true`. A person-authored message into an archived conversation unarchives it (same transaction as the turn); bot, agent and system turns do not.
- C-121 **Delete**: `DELETE /v1/conversations/{id}` → 204. Authority: `direct` conversations by their user or `can_own`; every other kind by `can_own` (403 otherwise). **409 `conversation_active`** while a turn is `queued`/`dispatched`/`running`. One transaction removes the conversation with turns, events, answer buffers, attachment rows, thread links, context credentials and mention anchors, nulls `approvals`/`action_approvals`/schedule/webhook conversation references, queues a `session_purges` row for a stored harness session and writes the `conversation.delete` audit row; attachment blob bytes are removed after commit (failures leave orphans for the daily sweep). `audit_log.metadata` of `conversation.delete`: `{ conversation_ids[], kinds[], agent_id, turns, events, attachments, session_purge_queued }` (retention: `agent_ids[]`, `reason: "retention"`, `retention_days`), never content.
- C-122 **Redact**: `POST /v1/conversations/{id}/events/{seq}/redact` and `POST /v1/turns/{id}/redact`, JSON body **required** (send `{}`): `{ reason?: string (≤ 500 chars, no control characters, else 400), reset_session?: bool (default false) }` → 200 `{ redacted_seqs: int[], session_reset: bool }` (`redacted_seqs` lists the events this call changed; `[]` when everything was already redacted: idempotent, no second audit row). Authority: the author of the event's turn (`turns.speaker_user_id`; an event without a turn has none), the direct user of a direct conversation, or `can_own` (403 otherwise); 404 for an unknown seq/turn. The payload is replaced in place by `{ <turn_id|call_id|message_id|approval_id|attachment_id when present>, redacted: true, redacted_at, redacted_by (display label), reason? }`; `transcript_events.redacted_at/redacted_by_user_id` are set. A redacted `turn.queued` (and any turn redaction) also empties `turns.text` and the turn's `turn_answer_buffers`. `reset_session` sets `harness_session_ref` to null, bumps `harness_session_epoch` and queues a session purge of the previous ref. Audit `transcript_event.redact`: `{ seqs[], types[], reason, turn_id?, reset_session }`.
- C-123 **SSE `event.redacted`**: the conversation stream emits the ephemeral event `event.redacted` (no `id:`), `data` = `TranscriptEvent` with `seq: null`, `payload: { "seq": <redacted seq> }`; clients re-read that event (`GET …/events?after_seq=<seq-1>`). Loss is allowed (ephemeral): a reload shows the marker.
- C-124 **Export**: `GET /v1/conversations/{id}/export[?after_seq=<n>&attachments=none|zip]` → 200 `application/json` (`Content-Disposition: attachment; filename="conversation-<id>.json"`, `Cache-Control: no-store`): `{ format: "aurora.conversation.v1", exported_at, conversation: Conversation, turns: [{ id, role, status, parent_turn_id, door, speaker_kind, speaker_label, text, error_code, usage, created_at, started_at, finished_at }], events: TranscriptEvent[] (durable, seq order, redacted ones as their marker), attachments: [{ id, filename, size, sha256, content_type, direction, purged }] }`; `attachments=zip` → `application/zip` (`conversation.json` plus `attachments/<id>-<sanitized name>`, stored without compression). Authority: `can_own`, or the direct user of that direct conversation (a turn author who is neither is 403). 422 **`export_too_large`** over 256 MiB (JSON, plus the attachment bytes for zip) or 200 000 events (page with `after_seq`); 400 for an unknown `attachments` value. Audit (best effort, every export): `conversation.export` `{ events, attachments, kind, after_seq }`. `purged` is always `false` until RFE-015 lands `attachments.purged_at`.
- C-125 **Retention**: `GET /v1/orgs/{org}/retention` → `{ conversations_days: int|null, conversations_days_max: int|null }`; `PUT` body `{ conversations_days?: int|null }` (absent = unchanged, `null` = keep forever) → the same object. Both: org `owner`/`admin` (403 otherwise). 422 `retention_invalid` outside 7..3650; 422 `retention_above_instance_maximum` above `retention.conversation_days_max`. Stored in `organizations.settings.retention.conversations_days` (other keys of `settings.retention`, e.g. RFE-015's, are kept). A change writes the security-critical audit row `retention.update` `{ conversations_days: { from, to } }` in the same transaction. The daily singleton `conversation_retention` job deletes through the delete path (C-121; actor `system`, `reason: "retention"`); the effective window is the smaller of the org's and the instance maximum.
- C-126 **Wire** (`16 §W5/W6`; schemas regenerated): runner work item and runner → supervisor frame `session.purge { conversation_id, session_ref }`; report and supervisor → runner frame `session.purged { conversation_id, session_ref, ok }` (`ok = true` stamps `session_purges.done_at`; `false` clears the lease so it is re-sent; a second report for a done row answers `duplicate`). Live delivery needs a running sandbox on the channel-holding replica with an applied home sync and no active turn in that conversation, and is re-sent every 30 s until reported. Pending purges of the agent (conversations without a live turn) are also delivered in the `render_inputs` home-sync part as `purges: [{ conversation_id, session_ref }]`; the supervisor executes them before it acknowledges the sync and again before it accepts any `turn.deliver`.
- C-127 **Config**: `retention.conversation_days_max` (integer 7..3650, absent = no ceiling; invalid values fail startup). `GET /v1/orgs/{org}/retention` reports it as `conversations_days_max`.
- C-128 **`DeltaKind` / stream**: no other wire change; `GET /v1/conversations/{id}/stream` and `/events` payloads of redacted events carry the marker of C-122, so clients must tolerate an event whose content keys are absent.

### Phase 2 — RFE-025 (governed BYO agents: posture, source CIDRs, required context credentials, D-59)

Rule: `08 §11.9`, `06 §BYO admission`, `04 §Permission matrix`, SEC-52; decisions D-59 and D-71. Reserved numbers C-130..C-139.

- C-130 **`Agent.byo`** (`GET/POST /v1/workspaces/{ws}/agents`, `GET /v1/agents/{agent}`, the `agent` of `CreatedAgent`, `PUT /v1/agents/{agent}/byo`): for `kind = "byo"` an object, `null` for hosted agents (and in responses that do not load it, e.g. the default agent of a workspace-creation response): `{ posture: "enabled"|"owners_only"|"disabled" (effective, of the agent's workspace), posture_level: "instance"|"org"|"workspace", source_cidrs: string[]|null (the agent's own list, null = inherit), require_context: bool (own flag), effective_source_cidrs: string[]|null (agent, else org, else instance; null = unrestricted), effective_source_level: "agent"|"org"|"instance"|null, effective_require_context: bool (instance ∨ org ∨ agent), effective_require_context_level: "instance"|"org"|"agent"|null }`. **Refusals** of `POST /v1/workspaces/{ws}/agents` with `kind = "byo"` and of `POST /v1/agents/{agent}/token`: 403 `byo_not_permitted` (`owners_only`, caller lacks `can_own`; `details: { posture, level, manage_url }`) or 403 `byo_disabled` (`details: { level: "instance"|"org"|"workspace", manage_url }`). Hosted agents are never affected. Default: the instance floor is `owners_only`.
- C-131 **`PUT /v1/agents/{agent}/byo`** (S/K, `can_own`; 404 invisible, 403 `forbidden` for a plain member): body `{ source_cidrs: string[] | null, require_context: bool }` (`PutByoRequest`, unknown fields refused), response `Agent`. Hosted agent → 422 `not_byo`. `source_cidrs`: 1–32 strict CIDRs (host bits must be zero; `/0`, an empty array, more than 32 entries and syntax errors → 422 `byo_cidr_invalid`, `details.fields[].path = "/source_cidrs[/i]"`); every entry must lie inside the inherited list (the org default, else the instance default) when it is non-empty, else 422 `byo_cidr_outside_default`; `null` = inherit. Entries are normalised and deduplicated. Audit `agent.byo_settings.update` is written in the same transaction as the change (no audit, no change: 503 `audit_unavailable`); gateways are invalidated with scope `agent`.
- C-132 **Workspace** (`GET/PATCH /v1/workspaces/{ws}`; `Workspace`): new `byo_posture: "enabled"|"owners_only"|"disabled"|null` and `effective_byo_posture: { value, level: "instance"|"org"|"workspace" } | null` (loaded by GET/PATCH, `null` in list and create responses). `PATCH` accepts `byo_posture` (absent = unchanged, `null` = inherit; `can_own`; invalid value → 400; looser than the instance floor or the org posture → 422 `governance_below_instance_minimum`, `details.fields[].path = "/byo_posture"`); audit `workspace.byo_posture.update` in the same transaction, invalidation scope `workspace`.
- C-133 **Org governance** (`GET/PUT /v1/orgs/{org}/governance`, `OrgGovernance.byo`): `{ posture: "enabled"|"owners_only"|"disabled"|null (null = inherit the floor), source_cidrs: string[] (≤32, empty = no org default), require_context_credentials: bool }`; an all-default `byo` is omitted from the stored document. Errors: 422 `governance_below_instance_minimum` (`byo.posture` below the floor), 422 `byo_cidr_outside_default` (an entry outside a non-empty instance default list), 422 `governance_invalid` (syntax, `/0`, host bits, more than 32). **`GET …/governance/effective`** gains `byo: { posture: Sourced<string>, source_cidrs: Sourced<string[]|null>, require_context_credentials: Sourced<bool> }` (`source` = `instance`|`org`; org level only, the workspace layer is per workspace/agent). **`GET /v1/governance/instance`** gains `byo: { enabled, minimum_posture, default_source_cidrs, require_context_credentials }`.
- C-134 **`ContainerConfig`** (`GET /v1/agents/{agent}/container-config`, `GET /v1/agent/container-config`, `container_config` of agent creation and token regeneration; `ContextCredential` unchanged) gains `byo: { posture, source_cidrs: string[]|null, require_context: bool } | null` (effective values) and `notes: string[]` (empty unless a note applies). For an agent whose effective `require_context` is true, `proxy_url` and the proxy variables carry the placeholder `<CONTEXT_TOKEN>` (never the `aug_` token) and `notes` says so. `GET /v1/agent/container-config` and `POST /v1/agent/context-credentials` with an agent token answer 403 `byo_disabled` while the effective posture is `disabled`; the dashboard route `GET /v1/agents/{agent}/container-config` stays `can_use`.
- C-135 **Policy warnings** (`GET …/policy/warnings`, `PolicyWarning`): new `code: "rule_applies_to_all_agents"` and two optional fields, `severity: "warning"` and `details: { rule_id, targets: Target[] }` (absent on the older codes), for every enabled hand-written allow rule with empty identities and at least one target of type `connection`, `app_connections`, `secret` or `secrets`; the message adds "and by members creating BYO agents" when the scope's effective BYO posture is `enabled`. Never blocks publishing.
- C-136 **Gateway refusals** (proxy port, not `/v1`; `06 §BYO admission`): after the credential authenticated, for `aug_` and BYO `auc_`: 403 `byo_disabled` (`details: { level, manage_url }`); 403 `source_not_permitted` (`details: { source, manage_url }`, `source` = the TLS peer or PROXY-protocol address); for an `aug_` credential of an agent that requires context credentials and a request with a selected credential candidate: 403 `context_credential_required` (`details.mint = "POST /v1/agent/context-credentials"`). A bad token is still 407. Counter `aurora_gateway_byo_refused_total{reason=posture|source|context_required}`; posture/source refusals are logged and written to `request_log` (`decision = blocked`, `error_code`) at most once per credential per minute. Live tunnels and WebSockets of an affected credential are closed when an invalidation tightens it.
- C-137 **`context_label`** (`RequestLogEntry` of `GET /v1/workspaces/{ws}/activity` and `Approval` of the approval routes): `string | null`, the label of the BYO context credential of the request (≤128, sanitized). New nullable columns `request_log.context_label` and `approvals.context_label`.
- C-138 **Configuration** (`15 §Configuration`): `[byo]` (`enabled`, `minimum_posture`, `default_source_cidrs`, `require_context_credentials`) and `gateway.trusted_proxies` (CIDRs; a connection from such a peer must begin with a PROXY protocol v1/v2 header, a PROXY header from any other peer is closed before the TLS handshake). `/0` and malformed CIDRs, an unknown posture and more than 32 default CIDRs refuse startup naming the key.
- C-139 **CLI / client**: `aurora run --agent <h> [--ttl 60..86400] [--context] -- <cmd>` mints one context credential per run (label `aurora run: <command line>`, ≤128; default `--ttl 3600`) and uses it as the proxy credential whenever the agent requires context credentials (or `--context` is given); the client's `ContainerConfig` gains `byo` and `notes`, and `Client::create_context_credential(label, ttl_secs)` is added.

### Phase 1 — RFE-004 (OIDC-only sign-in, D-46, D-72)

Rule: `04 §Authentication`, `§OIDC`, `§Development and end-to-end sign-in`, `05 §Auth and account`, SEC-42. Sign-in is OIDC; there is no local credential.

- C-150 **Removed routes** (answer `404 not_found`, no body schema, absent from `web/openapi.json`): `POST /v1/auth/signup`, `/v1/auth/login`, `/v1/auth/verify-email`, `/v1/auth/verify-email/resend`, `/v1/auth/password/forgot`, `/v1/auth/password/reset`, `/v1/auth/password/change`. The `SignupRequest`, `LoginRequest`, `VerifyEmailRequest`, `ForgotRequest`, `ResetRequest`, `ChangePasswordRequest` and `DeleteMeRequest` schemas are gone. C-01 (`invitation_token` on sign-up) is void: a pending invitation addressed to the verified e-mail is accepted by the first OIDC sign-in (C-153). `User.email_verified_at` stays (always set for OIDC-created users). The error codes `invalid_credentials`, `email_taken`, `already_verified` and `account_disabled` are no longer produced (the callback's `user_disabled` replaces the last).
- C-151 **`GET /v1/auth/oidc/{provider}/start`** query: `return_to?` (same-origin path), `reauth?` (`1`|`true`: send `prompt=login&max_age=0`; the caller must hold a live session, else `401 unauthenticated`; the sealed flow state carries `(session, user)`), `login_hint?` (forwarded to the provider; ≤254 chars). `429 rate_limited` (+ `retry-after`) after 20 requests per minute per client IP (`OIDC_AUTH_RATE`); the `callback` has its own bucket of the same size. **`/callback`**: an ID token without `email_verified = true` is *always* refused (`/auth/login?error=email_unverified`); new refusal codes `user_disabled`, `reauth_user_mismatch` (the authenticating identity is not the session's user), `reauth_stale` (`auth_time` older than 60 s), `signup_closed` per C-153. A successful re-authentication stamps `sessions.reauth_at` on the existing session, issues **no** new session cookie and redirects to `return_to`. `GET /v1/instance`: `oidc_providers` is the complete list of ways to sign in and may contain `{ id: "dev", display_name: "Development sign-in" }` (config `auth.dev_idp`); `require_email_verified` is removed from provider config (`false` fails startup naming the key).
- C-152 **`DELETE /v1/me`** (session only) takes no body. It needs `sessions.reauth_at` within 10 minutes: otherwise `412 reauth_required` (replaces the phase-0 `403 reauthentication_required`; the dashboard then starts `start?reauth=1&return_to=/account/delete`). Other rules unchanged (`409 last_owner`, session only). A re-authentication belongs to one session only.
- C-153 **Sign-up semantics at the first OIDC sign-in** (no wire change; `04 §Sign-up semantics under OIDC-only`): `open` creates user + org + default workspace; `invite_only` creates a user only for an e-mail with a pending invitation, in `auth.instance_admins`, or pre-provisioned by `bootstrap-admin`; `sso_jit` only when the provider `jit` block applies or an invitation is pending; otherwise `/auth/login?error=signup_closed` and **no user row**. For a *new* user in `invite_only`/`sso_jit` every pending, unexpired invitation for that e-mail is accepted in the same transaction (an invitation that cannot be accepted, e.g. over the seat cap, stays pending); existing users accept explicitly with `POST /v1/invitations/accept`. A pre-provisioned user (verified e-mail, no identity) binds the first verified identity that arrives with that e-mail. `aurora-api bootstrap-admin --email <e>` prints `<external>/auth/login` and refuses with `no_oidc_provider` when no provider is configured.
- C-154 **Development IdP** (not an API route): the binary/image `aurora-dev-idp` (`aurora_testkit::oidc`), environment `AURORA_DEV_IDP_{LISTEN,ISSUER,PREFIX,USERS,USERS_JSON,ALLOW_ANY_HINT}`; fixed client `aurora-dev-client` / `aurora-dev-secret` (`aurora_config::DEV_IDP_CLIENT_*`). Config `auth.dev_idp.{enabled,issuer,internal_url}` registers provider `dev`; refused unless `server.external_url` is on `localhost`, `*.localhost`, `127.0.0.1` or `[::1]`. Helm: `devIdp.*` (default off, loopback `externalUrl` only).

### Phase 2 — RFE-015 (attachment retention, per-org quota, storage breakdown, D-54, D-73)

- C-170 **Quota**: every path that stores attachment bytes refuses when `used + size` would exceed the org's effective quota (the org's `attachment_quota_mib` or `attachments.default_org_quota_mib`, capped by `attachments.org_quota_mib_max`; `0` = unlimited), checked under a lock on the org's `org_storage_usage` row in the inserting transaction. `POST /v1/conversations/{id}/attachments` and connector `PUT /connector/files/{op_id}` → **422 `attachment_quota_exceeded`** with `details: { used_mib, quota_mib }`; runner `POST /runner/attachments` (`send_file`) → **422** `attachment_quota_exceeded` (same body; the per-conversation daily cap stays 429 with the same code). A connector `file.fetch` that would exceed it is recorded as an attachment `failed` with `failure_reason = quota_exceeded` and the person gets a platform-voice notice; the turn still runs. A refused file leaves no row and no blob.
- C-171 **`GET /v1/orgs/{org}/storage`** (org `owner`/`admin`, 403 otherwise) → `{ org_id, used_bytes, quota_bytes|null, notice: "clear"|"warning"|"full", files, purged_files, attachments_days|null, attachments_days_effective|null, by_workspace: [{ workspace_id, workspace_slug, workspace_name, direction: "in"|"out", files, bytes }] }`. `notice` is `warning` from 80 %, `full` at 100 % of the quota.
- C-172 **`GET /v1/admin/storage`** (instance admin) → `{ blob_bytes, blob_count, attachment_bytes, attachment_files, purged_files, orgs: [{ org_id, slug, name, attachment_bytes, files, quota_bytes|null }] (top 50), tables: [{ table, bytes }] (blobs, attachments, transcript_events, request_log, audit_log) }`.
- C-173 **`DELETE /v1/attachments/{id}`** → 204, idempotent. Purges the bytes now (`purge_reason = deleted`, the row stays). The uploader (`created_by_user_id`) or `can_own`, always subject to `can_see_conversation` (404 when unseen or unknown, 403 for another member). Audit `attachment.delete` (best-effort) `{ conversation_id, direction, size_bytes }`. `GET /v1/attachments/{id}/content` of a purged attachment → **410 `attachment_purged`**. `Attachment` DTO unchanged.
- C-174 **Retention** (extends C-125): `GET/PUT /v1/orgs/{org}/retention` also carry `attachments_days` (7..3650 or null; absent = unchanged on PUT), `attachment_quota_mib` (MiB, `0` = unlimited, null = default; absent = unchanged), and on the response `attachments_days_max`, `attachment_quota_mib_effective` (null = unlimited) and `attachment_quota_mib_max` (null = no ceiling). 422 `retention_invalid`, 422 `retention_above_instance_maximum` (> `attachments.days_max`), 422 `attachment_quota_invalid`, 422 `attachment_quota_above_instance_maximum` (above the ceiling; `0` too when a ceiling exists). `retention.update` audit metadata gains `attachments_days` / `attachment_quota_mib` `{ from, to }` when they change.
- C-175 **Config**: `[attachments]` `default_org_quota_mib` (10240), `org_quota_mib_max` (51200, `0` = no ceiling), `days_max` (7..3650, absent = none).
- C-176 **Sweep and audit**: the daily singleton job `attachment_sweep` (sharing the daily tick of the conversation retention sweep) purges expired attachment bytes per org in batches (one `attachment.purge` audit row per batch, actor `system`, `{ reason: "retention", retention_days, attachments, bytes }`), removes orphan blobs, reconciles `org_storage_usage` from the rows, and writes one `attachment.quota_notice` audit row (`{ level: "warning"|"full"|"clear", used_bytes, quota_bytes }`) per level change. The export (C-124) reports the real `purged` flag per attachment and the zip omits purged files. Metrics: `aurora_blob_bytes`, `aurora_org_attachment_bytes{org}` (top 20, rest as `other`), `aurora_attachment_quota_rejections_total`.

### Phase 2 — RFE-005 (policy `user` identities bind to the turn's speaker, D-49)

Rule: `07 §Rule anatomy`, `06 §Credential authentication`, `08 §7`, SEC-44; decision D-49. Reserved numbers C-180..C-189.

- C-180 **`POST /v1/workspaces/{ws}/policy/simulate`** (`SimulateRequest`): new optional `speaker_user_id: Uuid | null`. Omitted or `null` = no speaker, so `user` identities never match. A speaker that does not hold `can_use` on the workspace → 400 `invalid_request` with `details.fields[].path = "/speaker_user_id"` (such a speaker never binds in the gateway, so nothing is simulated). Ignored for `lane: "open"`: tunnels carry no speaker. The response shape is unchanged.
- C-181 **`GET /v1/agents/{agent}/effective`**: new optional query `speaker=<user_id>|none` (`EffectiveQuery`). Default: the caller (or no speaker when the caller holds no `can_use`, e.g. an instance admin); `none` = speakerless traffic. A value that is neither `none` nor the id of a user with `can_use` → 400 `invalid_request`, `details.fields[].path = "/speaker"`. Still `can_use`; no permission-matrix change.
- C-182 **`EffectiveCredential`** gains `depends_on_speaker: bool` (always present): true when an enabled allow rule that names a `user` targets the credential, i.e. whether it is selected changes with the speaker.
- C-183 **Policy warnings** (`GET …/policy/warnings`, `WarningCode`): two new codes. `user_rule_needs_speaker` (informational) for every enabled rule with at least one `user` identity — "matches only turns spoken by <name>; never schedules, webhooks, BYO or boot traffic". `user_rule_no_agent_fallback` (`severity: "warning"`) for an enabled allow rule that targets credentials and names only `user` identities. Neither blocks publishing; `details` is absent.
- C-184 **Storage** (no route change): `gateway_credentials.speaker_user_id` (hosted context credentials of person-authored turns only) and `request_log.speaker_user_id`, both nullable (migration `20261110000050_policy_speaker.sql`). `request_log.speaker_user_id` is not part of the activity DTO in this phase.
- C-185 **Gateway** (internal, no wire change): the request identity carries `speaker_user_id`; the roster `principals` is gone. A speaker is bound only for a hosted `context` credential of a non-BYO agent whose speaker still holds `can_use`, re-checked after every `user`, `workspace`, `org` or `agent` invalidation. The API now sends a `user` invalidation when a workspace access binding is removed or an org member's role changes.

### Phase 2 — Self-hosted app instances (D-74)

Rule: `09 §3.4 Instance-bound hosts`, `§6`, `§7`, `06 §MITM lane: connect-time resolution`, `05 §App instances`, SEC-59. Reserved numbers C-240..C-249.

- C-240 **`AppInstance`**: `{ id, org_id: uuid|null (null = instance-wide), package_id, app_id, host, label, status: "active"|"disabled", created_by: { id, display_name }|null, created_at, updated_at, connections: int }`.
- C-241 **`GET /v1/orgs/{org}/app-instances[?app_id=]`** (any org member) → `Page<AppInstance>` of the instance-wide and the org's own instances. **`POST /v1/orgs/{org}/app-instances`** (org owner/admin, else 403) body `{ app_id, host, label }` (unknown fields refused) → 201 `AppInstance`. **`GET/POST /v1/admin/app-instances`** (instance admin) → the same for instance-wide instances. Validation errors (422, `details.fields[].path`): `app_not_instance_bound` (`/app_id`: app unknown, inactive or without `instances`), `instance_host_invalid` (`/host`: not a lower-case DNS name, has a port/path/userinfo, is an IP literal or under `aurora.internal`), `instance_host_conflict` (`/host`: equals or falls under another active package's static claim, is a governed LLM host, or is already registered in a scope that can see this one; `details.conflict: { kind: "static_claim"|"llm_host"|"instance", app_id? }`), `instance_probe_failed` (`/host`: the manifest probe failed; `details: { status?, reason }`; a destination-guard refusal reports `reason: "destination_refused"`). Audit `app_instance.create` in the same transaction (security-critical); gateways invalidated (`org`, or every org for instance-wide).
- C-242 **`PATCH /v1/app-instances/{id}`** body `{ label?, status? }` (the host is immutable: register a new instance instead). **`DELETE /v1/app-instances/{id}`** → 204: in one transaction its connections become `disabled` with `status_reason = "instance_deleted"`, its BYOC clients are deleted (cascade), the row is deleted and `app_instance.delete` is audited. `status = "disabled"` stops injection without deleting. Authority: the owners/admins of its org, or an instance admin for an instance-wide instance (403 otherwise; 404 when invisible).
- C-243 **Connect** (`POST /v1/apps/{app}/connect`): new `instance_id?: uuid`. For an app with `instances`: absent = `default_host` (422 `instance_required` when the app has no `default_host`); present = an `active` instance of that app visible to the org (404 otherwise). OAuth methods then require a BYOC client bound to that instance (`oauth_client_id`, or the only one) — 422 `instance_oauth_client_required` when there is none; templated authorize/token URLs resolve to the instance host. `Connection` gains `instance: { id, host, label } | null` (always present).
- C-244 **BYOC** (`POST …/oauth-clients`): new `instance_id?: uuid` (an instance of the client's package visible to the scope); `OAuthClient` gains `instance_id: uuid|null`.
- C-245 **App catalogue** (`GET /v1/apps/{app}`): new `instances: { label, default_host: string|null } | null` (null when the app is not instance-bound) so the dashboard shows the instance picker and the registration form.
- C-246 **Gateway** (proxy, not `/v1`): resolution and injection per `06` step 2; an instance-bound connection on any other host is refused like a tenant-bound one (421 `host_mismatch`, `details.expected_host` = the instance host). A disabled or deleted instance injects nothing (its host falls back to the ordinary unknown-host handling).

New wire details found while implementing (C-247..C-249):

- C-247 **`PATCH /v1/connections/{id}`** with `status: "active"` on a connection whose `status_reason` is `instance_deleted` → 409 `instance_deleted` (deleting an instance clears the connection's `instance_id`, so re-enabling would make it a default-host connection; connect again instead).
- C-248 **BYOC** `instance_id` that is unknown, of another package, or not visible to the scope (an instance-level client binds instance-wide instances only) → 400 `invalid_request` at `/instance_id`. A client bound to an instance is used only for connects to that instance, an unbound one only for the default host. One client per (scope, package, key, instance): a duplicate is 409 `conflict`.
- C-249 **Internal and tooling**: the hook host request carries `instance_host` (what `{instance}` in `permissions.egress` resolves to; absent = nothing). Offline fixtures (`aurora plugin test`) take `connection.instance_host` (+ optional `instance_app`) and `expect: { refused: host_mismatch, expected_host }`. Config: the API honours `gateway.allowed_private` for the registration probe and connect requests to a registered instance only.

### Phase 2 — P2-DBL-1 (gateway database lane, D-80)

Rule: `06 §Database lane`, `21 §Phase 2 — Database lane`, `09 §3 database`, SEC-60. Reserved numbers C-220..C-229. No `/v1` route or schema change; no migration.

- C-220 **Wire contract** (proxy port 4102, PostgreSQL protocol v3): first bytes `SSLRequest` / `GSSENCRequest` / `CancelRequest` / `StartupMessage` enter the lane; TLS is mandatory before the password; user = database connection id (uuid), password = a gateway credential (`aug_`, `auc_`, `aub_`); the server certificate is a gateway-CA leaf for `<connection_id>.db.aurora.internal` (SNI) or the listener's certificate. Login refusals are `ErrorResponse` FATAL: `28P01` unknown/revoked credential or the database rejected the stored credentials, `28000` bad user / no TLS / frozen / BYO refusal, `42501` no grant, disabled, not a database connection, `57P03` stores unavailable, `08006`/`08001` database unreachable or destination refused.
- C-221 **Statement refusal**: `ErrorResponse` severity `ERROR`, SQLSTATE `42501` (policy, approval denied/expired, classifier refusal), message = the guidance message; `57P03` when policy or approvals cannot be evaluated; `42P05`/`42P03`/`26000`/`34000` for prepared-statement and portal name errors; `54000` for session limits (1,024 statements, 32 MiB of statement text, 1,024 portals, 16 MiB per message).
- C-222 **Synthetic policy request**: `POST https://<connection_id>.db.aurora.internal/<class>` with `<class>` ∈ `read` | `write` | `ddl` | `admin` (`transaction` is never policy checked); port 443; body = statement text; the connection is the credential, the app's tool matching host `*.db.aurora.internal` and the class path is the tool. `request_log`: `lane = 'db'` (already allowed by the table's check), `method = POST`, `host` = the synthetic host, `path = /<class>`.
- C-223 **Stored credentials of a database connection** (`ConnectionCredentials`): `host`, `port` (number or string, default 5432), `dbname`, `user`, `password`, optional `sslmode` (`disable|allow|prefer|require|verify-ca|verify-full`, default `require`), optional `ca_pem`. App requirement: manifest `database: { engine: postgres }`.
- C-224 **Gateway internals** (no wire change): `RequestHandler::db_open(self: Arc<Self>, identity, connection_id) -> Result<DbOpened, DbOpenError>` and `DbAuthorizer::authorize(class, statement)`; `PolicySource::connection(org, ws, id)` and `aurora_db::repo::gateway::app_connection_by_id`; `aurora-sqlclass` API: `classify(&str) -> Result<Classified{class, statements}, Refusal>`, `redact(&str, max_chars)`, `Class`, `MAX_STATEMENT_BYTES`.
- C-225 **Test infrastructure**: `scripts/e2e/gateway.sh` starts a throw-away PostgreSQL container `<worktree>-gwe2e-dbtarget` on a random loopback port (or uses `AURORA_E2E_DB_TARGET=host:port`, user/password/db `aurora`/`aurora`/`postgres`) as the target of the e2e database scenario; without podman or the variable the scenario is skipped with a notice.

### Phase 2 — P2-DBL-2 (sandbox libpq files, `postgresql` package, D-89)

Rule: `12 §Database files`, `21 §Phase 2 — Database lane`, `docs/tasks/P2-DBL.md`. Reserved numbers C-230..C-239. One additive field on `Connection` (OpenAPI regenerated); no migration, no new route.

- C-230 **`Connection.database`** (on `GET/…/connections` and `GET /v1/connections/{id}`): `null` unless the connection's app has a `database` section, else `{ engine: "postgres", service: string }`. `service` is the base libpq service name of the connection: the label lowercased, every run of characters outside `[a-z0-9._]` replaced by one `-`, trimmed of `-` and `.`, at most 48 characters, `db` when nothing is left (`aurora_core::pgservice::service_base`). The dashboard shows `psql service=<service>`.
- C-231 **Home sync `render_inputs` entry `databases`**: `[{ service: string, connection_id: uuid }]`, one per active connection of an app with a `database` section that the agent holds a grant on (the same set as `apps[].accounts`; `accounts[]` entries of such apps also carry `service`). `service` is the base name, except that when two or more of the agent's database connections share a base name each becomes `<base>-<first 8 hex digits of the connection id>` (`aurora_core::pgservice::service_names`). No secret and no gateway address travels in the sync.
- C-232 **Sandbox files** (supervisor, `.aurora/pg/`, regenerated at boot and after every home sync; removed when `databases` is empty): `pg_service.conf` (0644; per service `host=127.0.0.1`, `port=3130` = the supervisor's loopback database relay, `user` = connection id, `dbname` = service, `sslmode=disable` on that loopback leg, `connect_timeout=15`). There is **no** `pgpass`, no `root.crt` and no credential in any file or environment of the sandbox (a leftover of an earlier version is deleted). Every agent process (harness session, `process_start`, gates) gets `PGSERVICEFILE`. The relay (`127.0.0.1:3130`, only while `databases` is non-empty) answers `SSLRequest`/`GSSENCRequest` with `N`, opens TLS to the boot document's gateway address verified against `ca_pem` with the server name `<connection_id>.db.aurora.internal`, forwards the `StartupMessage`, answers the gateway's cleartext password request itself (boot token; the turn context credential while exactly one turn is active) and never passes it to the client, then relays bytes after `AuthenticationOk`; `CancelRequest` is forwarded over TLS (verified against the gateway server name). A TLS verification failure sends no credential (client gets FATAL `08006`).
- C-233 **Tooling**: (a) the manifest linter accepts the tool host `*.db.aurora.internal` for an app with a `database` section without a `hosts[]` entry (`tool_host_uncovered` otherwise); (b) fixtures may send `POST https://<uuid>.db.aurora.internal/` with a SQL `body`, which `aurora plugin test` classifies with `aurora-sqlclass` and routes as `/<class>`; `expect.class` (`read|write|ddl|admin|transaction`) and `expect.refused: statement_refused` (always-refused or unparsable) are new; (c) `check-builtin.sh` requires database tools to be `POST` on `*.db.aurora.internal` and exempts them from the GET-only read rule and the `approvals` template rule (the gateway builds the statement summary); (d) `scripts/e2e/hosted.sh` starts `aurora-e2eh-dbtarget` (10.89.252.6) and passes `AURORA_E2EH_DBTARGET`; the e2e gateway's `allowed_private` then includes that address.

### Phase 2 — P2-CAT-0 (built-in catalogue, D-75)

Reserved numbers C-210..C-219. No route or wire shape changes.

- C-210 **Config** `[plugins] builtin_dir` (string, default `/usr/share/aurora/plugins`, must not be empty; env `AURORA__PLUGINS__BUILTIN_DIR`). The API scans it at startup and every 60 s (same singleton job and idempotence by digest as `plugins.dir`) with source `builtin`. Fail-closed: an archive is ingested only when its `SIGNATURE` verifies against the compiled-in project key (`AURORA_PROJECT_PUBLIC_KEY`, publisher `aurora`); `plugins.allow_unsigned` never applies, and a package signed by a trusted key is refused too (422 `package_invalid`, violation `builtin_not_project_signed`, or the underlying `signature_missing` / `signature_*` code). A refused file creates no row and is logged (`plugin file refused`, `source=builtin`, `code`). Project-signed packages activate on first sight and on upgrades that do not widen permissions (09 §10).

### Phase 2 — RFE-006 (freeze agent / workspace / organization, D-50)

Rule: `04 §Freeze`, `05 §Conventions`, `06 §Credential authentication`, `10 §Freezing an agent`, SEC-45; decisions D-50, D-76. Reserved numbers C-200..C-209.

- C-200 **`POST /v1/agents/{agent}/freeze`** (`{ reason }`, 1..500 chars, sanitized to one line; 400 `invalid_request` otherwise) and **`POST /v1/agents/{agent}/unfreeze`** (no body), `can_own` on the agent's workspace (session or key; a workspace key stays confined to its workspace). Both answer 200 `FreezeResult { frozen: Frozen | null, effective_frozen: bool }`: `frozen` is the flag of **the scope acted on** (set after `freeze`, `null` after `unfreeze`), `effective_frozen` whether the resource is effectively frozen afterwards (agent: agent ∨ workspace ∨ org). `Frozen = { at, by_label, reason, authority: "workspace_owner"|"org_admin"|"instance_admin", level: "agent"|"workspace"|"org" }` (`by_label` = the freezer's e-mail at read time). Both are idempotent: freezing a frozen scope keeps the original record (no new audit row); unfreezing an unfrozen scope is a no-op. **403 `freeze_authority_insufficient`** (`details: { required_authority, caller_authority }`) when the caller's authority ranks below the recorded `frozen_authority`. The caller's authority on agent/workspace routes is `org_admin` for an org owner/admin and `workspace_owner` for a workspace owner.
- C-201 **`POST /v1/workspaces/{ws}/freeze` / `…/unfreeze`**: as C-200, `can_own` on the workspace (org owners/admins included). The workspace flag is independent of its agents' flags and of the org's.
- C-202 **`POST /v1/orgs/{org}/freeze` / `…/unfreeze`**: as C-200, org `owner` only (an org admin gets 403 `forbidden`; non-members 404; workspace keys 403). Authority `org_admin`.
- C-203 **Operator kill-switch**: `POST /v1/admin/orgs/{org}/freeze` (`{ reason }`) and `…/unfreeze`, instance admin, session only (`org` = organization id; unknown id → 404). `frozen_authority = instance_admin`, so only an instance admin lifts it; an org owner gets 403 `freeze_authority_insufficient`. It gives the operator **no** org data access (every org route stays 404 for a non-member instance admin). New: **`GET /v1/admin/orgs`** (instance admin; `limit`, `cursor`, `frozen=true` filter) → `Page<AdminOrg { id, slug, name, frozen: Frozen | null, created_at }>`, newest first, names and slugs only — the operator view the dashboard's *Organizations* page needs (not named in `05`; added so the kill-switch is usable).
- C-204 **Resource fields**: `Agent` gains `frozen: Frozen | null` (the freeze that stops it — its own, its workspace's or its organization's; the **broadest** wins; `level` says where it is set) and `effective_frozen: bool` (always present). `Workspace` gains `frozen: Frozen | null` (own flag or the org's, broadest first). `Org` gains `frozen: Frozen | null` (own flag). Agent and workspace flags are visible to everyone who can use the workspace.
- C-205 **423 `frozen`** (`details: { level }`): messages (`POST /conversations/{id}/messages`, including follow-ups), attachment upload, `POST /turns/{id}/send-now`, `POST /schedules/{id}/run-now`, and the BYO agent-token routes `GET /v1/agent/container-config` and `POST /v1/agent/context-credentials`. Every door that creates a turn is refused in `create_turn_capped` (one choke point), so peer messaging, watches and system turns cannot start work either. Peer messaging through the tool answers a tool error "<Agent> is paused by an administrator." and creates nothing. Channel doors reply once per (integration, space/thread, person) per `FREEZE_REPLY_INTERVAL` (10 min, per replica) "<Agent> is paused by an administrator." and create nothing (inbox outcomes `frozen_reply` / `frozen`).
- C-206 **Webhooks**: `POST /v1/hooks/{token}` of an effectively frozen agent → **410 `frozen`** (`details: { level }`) and a `webhook_deliveries` row `status = paused`, `reason = frozen`; the webhook stays enabled. Checked after the signature (an unauthenticated caller learns nothing) and before the fire-time authority, so it holds for creator- and workspace-anchored hooks alike and never disables one for authorization.
- C-207 **Gateway** (not under `/v1`): a credential of an effectively frozen agent → **403 `frozen`** (`details: { level }`, not 407), every kind (`aug_`, `aub_`, `auc_`); the broadest level is reported. Live tunnels and WebSockets of the affected credentials close on the next `aurora_gateway_invalidate` (scopes `agent`, `workspace`, `org`), requests held for approval are dropped with them. Nothing is revoked, so unfreezing restores the same tokens at once. `aurora_gateway_frozen_refused_total`, `aurora_gateway_frozen_tunnels_closed_total`.
- C-208 **Schedules**: a due schedule of an effectively frozen agent is not fired: `next_fire_at` advances to the next real occurrence after now (no catch-up burst), `schedules.last_outcome = skipped_frozen` and a `schedule_runs` row `skipped_frozen` are written, no turn is created, the schedule is neither disabled nor sent through the authority check (FU-P2-021a: both anchors).
- C-209 **Audit and storage**: audit actions `agent.freeze|unfreeze`, `workspace.freeze|unfreeze`, `org.freeze|unfreeze` (`metadata: { reason, authority }`; unfreeze `{ authority, frozen_authority, frozen_reason }`; actor kind `instance_admin` on the operator routes), written in the same transaction as the state change, its effects and the gateway invalidation. Migration `20261113000006_freeze.sql` (`frozen_*` columns on `agents`, `workspaces`, `organizations`; `sandboxes.stop_reason` gains `frozen`; `schedules.last_outcome` gains `skipped_frozen`). The runner wire `sandbox.stop` reason vocabulary gains `frozen`.

### Phase 2 — RFE-011a (approval deadlines, stall exemption, notifications, D-56, D-81)

- C-260 **Rule timeout**: `PolicyRule` gains `approval_timeout_secs: int | null`; `RuleInput` accepts it (optional, absent = null). Only on `allow` rules with `require_approval`; the value must lie in `[60, approvals.max_timeout_secs]` (default max 3600). Out of range → **422 `approval_timeout_out_of_range`** (same `details.fields` as `policy_invalid`, path `/rules/{i}/approval_timeout_secs`; the code is used whenever any field is a range violation); on a rule without `require_approval` → 422 `policy_invalid` (same path). Stored on `policy_rules.approval_timeout_secs`, copied by publish and rollback.
- C-261 **Grant ask timeout**: `PUT /v1/agents/{agent}/grants/connections/{conn}` accepts `ask_timeout_secs` (optional int); `ConnectionGrant` returns `ask_timeout_secs: int | null`. Only for `mode: custom` with non-empty `ask_tools` (else 422 `grant_invalid`, `details.reason = ask_timeout_without_ask_tools`); range as C-260 (422 `approval_timeout_out_of_range`, path `/ask_timeout_secs`). It is compiled into the derived *ask* rule as `approval_timeout_secs`; replacing the grant without it clears it. The audit `grant.create` metadata carries it.
- C-262 **Effective timeout**: `Simulation` (`POST …/policy/simulate`) gains `approval_timeout_secs: int | null` and `EffectiveTool` (`GET /v1/agents/{agent}/effective`) gains the same, both non-null only for `needs_approval`: the minimum of the timeouts set on the matching requiring rules of both scopes (clamped to the bounds), else `approvals.timeout_secs`. The gateway hold uses the same function (`aurora_core::limits::effective_approval_timeout_secs`), so `approvals.deadline = now() + this`.
- C-263 **`GET /v1/approvals/stream`**: the `approval` event gains an optional `notice` object, present only on `created`: `{ id, workspace: { id, name, slug, org_slug }, agent: { id, name, handle } | null, title, kind: "gateway", expires_at }` (title sanitized to one line ≤120 chars; no summary fields, body or decision link). An open stream, and an `GET /v1/approvals/pending?wait=` long-poll while held open, count as a live delivery surface (`approval_watchers`). `kind: "action"` is not pushed on the stream (action approvals are e-mailed, see C-265).
- C-264 **`/v1/me`**: `GET` returns `approval_email: bool` (default true); `PATCH` accepts `approval_email` (optional bool; the other fields are unchanged).
- C-265 **Notifier (no route)**: every 10 s each replica e-mails the deciders of approvals (gateway and action) still `pending` ≥ 20 s with no delivered channel card and no fresh watcher: explicit workspace owners/approvers, else org owners/admins; active, not disabled, `approval_email` on, able to see the approval's `direct` conversation. Content: platform voice, agent and workspace names, the sanitized title, rule name, expiry, and `<external>/o/<org>/w/<ws>/approvals?focus=<id>` — never a decision link. One mail per `batch_id`; at most 20 per user per hour, then one digest mail ("N more approval(s) waiting") and silent folding; claims in `approval_notifications` make it idempotent across replicas. `approvals.email_notifications = false` switches it off; with `email.smtp_url` unset mails are logged only.
- C-266 **`GET /v1/instance`** gains `approval_email: "smtp" | "log" | "disabled"` (the admin overview warns "approval e-mails disabled" for `log` and `disabled`).
- C-267 **Config**: `approvals.max_timeout_secs` (default 3600, 300–14400, must be < `hosted.turn_ceiling_secs − 900`) and `approvals.email_notifications` (default true); invalid values fail startup of the API and the gateway.
- C-268 **Stall rule** (due-work arm 8, no route): a running turn with a `pending` gateway approval inside its deadline is never `turn_stalled`; otherwise stalled means `now − GREATEST(last_progress_at, latest approvals.resolved_at of the turn) ≥ TURN_STALL`. `resolved_at` is set on approve, deny, expire and abandon (including freeze). `TURN_CEILING` and the frozen-agent turn end are unaffected; action approvals never exempt.

### W9 hook-host JSON encoding (C-18, C-19)

- C-18 Internal snapshot route returns the exact canonical bytes so consumers verify `sha256(body) == x-snapshot-hash` without re-serialization.
- C-19 `POST /internal/v1/hooks/{connect.authorize|connect.exchange|token.mint|approval.summarize}` (`Authorization: Bearer <internal secret>`, I-20) body `{ package_id, package_digest, app_id, input }`; `input`/`ok` objects use the WIT record field names in **snake_case**; `list<kv>` = `[{ "key", "value" }]`; `option<T>` = value or `null` (fields always present); `list<u8>` = JSON array of integers 0–255; `u64` timestamps = integer Unix seconds; variants = single-key objects (`exchange-source`: `{"oauth_callback": [kv]}` \| `{"form": [kv]}`; `exchange-output`: `{"connected": {...}}` \| `{"continue": {"url", "continuation"}}`). Errors: `{ "error": { "code", "message" } }` with codes `unsupported`, `invalid_input`, `upstream`, `reauth_required`, `internal`, `timeout`, `memory`, `output_too_large`, `egress_denied`. Implemented by `services/aurora-hookhost/src/wire.rs`; clients: gateway `mint.rs`, API connect hooks (BE-043), gateway summarize (HOOK-004).

## Shared schemas

_Defined alongside the milestone sections above._
