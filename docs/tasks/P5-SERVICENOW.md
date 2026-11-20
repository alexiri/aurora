# P5-SERVICENOW: built-in `servicenow` package — incidents, requests, knowledge and the catalog on a ServiceNow instance (phase 5, D-431, SEC-145)

Read `docs/tasks/P5-CATALOG.md` first (common rules, acceptance, order). This brief owns
`plugins/builtin/servicenow/**`, its `REFUSED` entry and its `ICONS.md` row.

## Goal

"What's the status of my ticket INC0123456?", "open an incident: the printer in 31/1-012 is jammed",
"add a comment to my request", "find the knowledge article about VPN on Linux", "order a new laptop
from the catalog". An agent reads and updates the person's tickets and searches knowledge, **as the
person**, because ServiceNow's row and field rules give different answers to different users.

## Hosting and connect (D-431)

- **Instance-bound, no default host.** Every customer has its own instance (`<name>.service-now.com`, or a
  custom domain); an admin registers it. Probe at registration: `GET /api/now/table/sys_properties?sysparm_limit=1`
  expecting `[401]` (proves a ServiceNow REST endpoint answers without leaking data; confirm the status an
  unauthenticated Table API call returns, and use `[200, 401]` if it varies).
- Host entry: `{instance}` with `path_prefix: /api` only. Nothing else on the host receives the credential
  (the UI's `*.do` pages and legacy JSONv2 endpoints stay unauthenticated).
- **Method 1, `oauth` — "My ServiceNow account" (preferred):** OAuth 2.0 authorization code with an
  **instance-bound client** (`09 §3.4` *Instance-bound hosts*, OAuth bullet): the instance's admin creates an
  inbound OAuth integration ("OAuth – Authorization code grant", Machine Identity Console or System OAuth ›
  Application Registry), enters the redirect URI `{{aurora.callback_url}}`, and registers client id and
  secret in Aurora per instance.
  - `oauth_clients`: `authorize_url: "https://{instance}/oauth_auth.do"`, `token_url:
    "https://{instance}/oauth_token.do"`, `client_auth: body` (check the docs; `basic` if required), `pkce`
    as the docs allow.
  - `token: oauth2_refresh`; defaults are a 30-minute access token and a 100-day refresh token (the
    instance admin sets both). Whether the refresh token rotates and which error means "reconnect" come from
    the docs (`reauth_codes`, at least `invalid_grant`); add `stale_codes` only for a documented early 401.
  - Scope: ServiceNow's default `useraccount` scope (all the user's own access). If the docs show a narrower
    REST API scope an admin can create, describe it in the setup guide as optional; do not require it.
  - Probe: `GET /api/now/table/sys_user?sysparm_query=user_name=javascript:gs.getUserName()&sysparm_fields=user_name,email&sysparm_limit=1`
    is the usual way to name the account; check the docs for a cleaner "who am I" endpoint, and do **not**
    use a probe that needs `javascript:` if the instance can block it. Take the label from `email`.
- **Method 2, `integration` — "Integration user (shared)":** `kind: fields`, `username` + `password`
  (secret) of a dedicated integration account, injected as `basic`. `connect_note` says that every agent
  using it acts as that one account (row rules apply to it, not to the person), and to give it only the
  roles it needs (`permission_hints`: "Role · itil" for write, "Role · snc_read_only" or a custom read role for
  read). Fields also: `name` (`store: label`).

## Tool catalogue (verify against ServiceNow's REST API reference for the current family release)

The Table API is `/api/now/table/<table>[/<sys_id>]`; ServiceNow also serves `/api/now/v1/table/…` and
`/api/now/v2/table/…`. Cover the versioned forms with the same tools (`/api/now/*/table/…` patterns
declared **after** the unversioned ones, or separate items) and prove both in fixtures.

**Read (GET):**
- Tickets: `incident`, `incident/*`; `sc_request`, `sc_request/*`; `sc_req_item`, `sc_req_item/*`;
  `sc_task`, `sc_task/*`; `task` (any task type, read only); `problem`, `problem/*`;
  `change_request`, `change_request/*`.
- Comments and work notes: `sys_journal_field` (filtered by `element_id`), the documented way to read a
  record's activity.
- Knowledge: the Knowledge Management API (`/api/sn_km_api/knowledge/articles`, `…/articles/*`) if the
  docs show it on a plain instance; otherwise `kb_knowledge`, `kb_knowledge/*`.
- Service Catalog API: `/api/sn_sc/servicecatalog/catalogs`, `…/categories/*`, `…/items` (search),
  `…/items/*` (an item and its variables).
- People and groups (lookups for assignment): `sys_user` and `sys_user_group` with `sysparm_fields`
  limited by the guide (name, user_name, email, department) — say in the guide not to fetch whole user
  records.
- CMDB: `cmdb_ci*` tables (`/api/now/table/cmdb_ci*`) and the CMDB Instance API reads
  (`/api/now/cmdb/instance/*`, `…/*/*`).
- Attachments: `/api/now/attachment` (list, by `table_sys_id`), `/api/now/attachment/*`,
  `/api/now/attachment/*/file`.
- Aggregates: `/api/now/stats/*` ("how many open incidents in my group").
- **`read_other_table`** (`GET /api/now/table/*`, declared last among reads): any other table, for questions
  the list above misses. ServiceNow's own rules decide what comes back; grants can block it.

**Write (each with an approval template; `{request.host}` links to the record):**
- `create_incident` (`POST /api/now/table/incident`): title "open an incident"; fields: short description,
  urgency, impact, category, caller (`caller_id`), assignment group.
- `update_incident` (`PATCH` and `PUT /api/now/table/incident/*`): title "update an incident"; fields: sys_id
  (path), state, `comments` (visible to the caller), `work_notes` (internal), assigned to, close code.
- `comment_request_item` (`PATCH /api/now/table/sc_req_item/*`): title "comment on a request"; fields:
  sys_id, `comments`.
- `order_catalog_item` (`POST /api/sn_sc/servicecatalog/items/*/order_now`): title "order from the
  service catalog"; fields: item sys_id (path), quantity, `requested_for`, the variables object (clamped).
- `attach_file` (`POST /api/now/attachment/file`): title "attach a file to a record"; fields: `table_name`,
  `table_sys_id`, `file_name` (query parameters).

**Refused** (`REFUSED["servicenow"]`, each a `refused …` case): record deletion
(`DELETE /api/now/table/*/*`), writes to any table not listed (`POST /api/now/table/sys_user`,
`PATCH /api/now/table/sys_user/*`), writes to `sys_*` configuration tables (`POST
/api/now/table/sys_script`), OAuth and credential tables (`oauth_entity`, `oauth_credential`,
`sys_user` password fields; SEC-145), Import Set loads (`POST /api/now/import/*`), and scripted endpoints
(`/api/now/*` paths outside the catalogue are untooled anyway; prove one).

## Agent text

- `agent.summary`: "ServiceNow tickets, requests, knowledge and the service catalog: check a ticket, open an incident, comment, order an item."
- `agent.instructions`: base `https://<instance>/api/now`; Aurora adds the credentials; always pass
  `sysparm_fields` and `sysparm_limit`, and `sysparm_display_value=true` when showing results to people;
  ticket numbers (INC…, RITM…, REQ…) are searched with `sysparm_query=number=<n>`; comments are visible to the
  caller, work notes are not; with an integration connection, say that changes are made as that account.
- Guide: `api-notes.md` (encoded queries and their operators, display values, pagination with
  `sysparm_offset` and the `Link` header, journal fields, the catalog ordering flow, 429 and `Retry-After`);
  recipes `check-my-tickets.md` ("What are my open tickets?": the caller is the connected user,
  `caller_id=javascript:gs.getUserID()` or the user's sys_id first, incidents and request items, state in
  words) and `open-an-incident.md`.
- `operations.json`: hand-written for the tools above. ServiceNow can export an OpenAPI document per instance
  from the REST API Explorer, but it is per instance and incomplete; do not depend on one.

## Fixtures

Every host entry, tool (unversioned and versioned Table API paths), approval and refused item; OAuth
lifecycle on a registered instance (`refreshed`, `reauth_required`, and the refresh request built for that
instance only, SEC-59); basic injection for method 2; no credential on `/incident.do` of the same host.

## Owner checks (not blocking)

CERN's ServiceNow (`cern.service-now.com`) admins have to create the OAuth integration for method 1, or an
integration account for method 2. The research found no CERN-specific API restrictions; whether CERN allows
REST access for ordinary users is theirs to say.
