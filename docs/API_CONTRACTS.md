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
    `auth.trusted_origins` → otherwise 403 `csrf_origin_mismatch` (SEC-22).
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

_Pending (API-001)._

## Shared schemas

_Defined alongside the milestone sections above._
