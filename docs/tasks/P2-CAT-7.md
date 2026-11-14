# P2-CAT-7: Built-in `indico` package (phase 2)

## Goal
Add a built-in package `indico` so agents can work with a self-hosted **Indico** (the open-source
event-management and room-booking system): search for and read events and meetings, prepare new
ones, search for rooms (including free rooms in a time slot), book rooms and cancel their own
bookings, and read and set favorites. Indico is always self-hosted, so the app is
**instance-bound** (D-74): an admin registers the organisation's Indico server, people connect to
it.

The evidence for every endpoint, scope and behaviour below is in the appendix
`docs/tasks/P2-CAT-7-indico-api.md` (rows A1..A12 = documented HTTP API, B1..B30 = undocumented,
verified in the Indico 3.3.14 source). **Do not add an endpoint that is not in the appendix.**

## Linked requirements
- Owner request 2026-11-08 (`PROGRESS.md` P2-CAT-7; `FEATURES.md` `P2/owner§pkg-indico`).
- `docs/21-later-phases.md` §Phase 2 — App catalogue (the MUST list for every built-in).
- `docs/09-app-plugins.md` §2, §3 (incl. §3.4 *Instance-bound hosts*), §4, §5, §6, §12.
- `docs/tasks/P2-CAT.md` (common rules for catalogue lanes), `docs/tasks/P2-CAT-T.md`.
- Decisions D-63 (deployment-neutral defaults), D-74 and D-79 (instances), D-75 (built-in
  framework, `docs/setup.md` layout), D-77 (hooks; not used here).
- `docs/07-policy-approvals.md` §Matching primitives, §Grants and their compilation, §Approvals.

## Context to read (and nothing else)
- `plugins/builtin/README.md` (layout, checks, fixtures) and the reference package
  `plugins/builtin/gitlab/` (instance-bound, OAuth + personal token; `tests/instance.yaml` shows
  the `connection.instance_host` fixture key). `plugins/builtin/aws-keys/` shows a package without
  a `token` section and parameter-selected operations held as writes.
- `scripts/plugins/check_builtin.py` (coverage rules: every host entry, every tool positive and
  `negative <tool>:` case, every summary field asserted, read tools GET/HEAD only, default scopes
  read only).
- Gateway facts you need (do not change them): the app host entry is the one with the **longest
  matching path prefix**, and `path_prefix` is a **literal string prefix**
  (`services/aurora-gateway/src/apps.rs`); the tool is the **first** catalogue tool that matches,
  **read items before write items, in declaration order** (`identify_tool`, same file). Path
  patterns: `*` one non-empty segment, `**` zero or more trailing segments, a trailing `*` inside a
  segment is a prefix match (`/export/categ/favorites*`); query strings never participate
  (`07 §Matching primitives`). A summary field whose source yields nothing is silently omitted
  (`crates/aurora-summary/src/template.rs`).

## 1. Scope (owner's list → package)

| Owner asked for | What the package offers | Tier |
|---|---|---|
| Search events / meetings | Title search (A6), events of categories with date/type/location/room filters (A2); instance-wide search incl. contributions, notes, files (B1); category search (B2) | official + extended |
| Read events / meetings | Event metadata, contributions, sessions, attachments list, notes (A4); timetable (A5); file download (A7) | official |
| Create events / meetings | **Prepare** an event (title, start, duration, type) and hand the user a link that opens Indico's creation dialog prefilled (B27). Indico has no API that creates events. | extended |
| Search available rooms | Rooms free in a slot, with filters (B15), per-room availability (B17/B18); official fallback: room and booking exports (A9, A10, A11) | extended + official |
| Book rooms | HTTP API booking (A12, same day, direct bookings only); room-booking app booking with recurrence and pre-booking (B22) | official + extended |
| Read favorites | Events of my favorite categories (A3); favorite users, categories, events, rooms (B3, B5, B7, B9) | official + extended |
| Set favorites | Add/remove favorite users, categories, events, rooms (B4, B6, B8, B10) | extended |
| "etc." | Current user (A1); room details (A9, B16); bookings of a room or location (A9, A11); my bookings (A11 by booked-for name; B19, B20, B21); cancel my booking or one date of it (B23, B24); locations and equipment (B12, B13); user search to find people to favorite or book for (B25, B26) | official + extended |

**Tiers.** *Official* = Indico's documented HTTP API (`/export/`, `/api/`), scopes `read:user`,
`read:legacy_api`, `write:legacy_api`. *Extended* = Indico's own UI JSON endpoints, which Indico
explicitly does not keep compatible (appendix §1) and which need the broad scopes
`read:everything` / `full:everything`. **The extended tier ships only if the owner answers Q-A
"yes"**; the coordinator records the answer before dispatch. If "no", drop every row marked
*extended* in §4, the extended host entries in §2.3, the two `everything` scopes, and the related
fixtures and guide text.

Out of scope: legacy signed API keys (§3.3, G1); the full event-creation form (B28, Q-B);
contribution favorites (B11) and category info (B29) (Q-E); registrants/check-in (`registrants`
scope); anything administrative (`/admin/**`, `/rooms/api/admin/**`, approve/reject bookings,
blockings, acting on another user under `/user/<id>/…`).

## 2. Hosting model

### 2.1 Instance-bound, no default host
The app declares `instances` and uses `{instance}` in every host, tool, probe and OAuth URL
(`09 §3.4`, D-74). **No `default_host`.** Indico has no vendor-run SaaS service; every Indico is
some organisation's own server. Making any one organisation's server (for example this
deployment's) the product default would bind every Aurora installation's Indico credentials to
that host by default, which D-63 forbids (product defaults are deployment-neutral). A deployment
that wants its server pre-registered does so as an instance-wide app instance (instance admin),
not in the package (Q-F).

Consequences: every connection has an `instance_id`; `POST /v1/apps/indico/connect` without one is
refused (`05 §App instances`); `about.web_hosts` stays empty (approval links use
`{request.host}`, which resolves to the instance host and is a declared host, as in GitLab).

Indico servers deployed under a sub-path (`https://example.org/indico/`) cannot be registered:
`{instance}` is a host and paths are absolute (G7). Document it in the guide.

### 2.2 `instances`
```yaml
instances:
  label: Indico server
  probe: { method: GET, path: /api/user/, expect_status: [200] }   # A1: answers JSON `null` without a token
```

### 2.3 `hosts[]` — narrow path prefixes (defence in depth)
The token is injected **only** under the prefixes the tools use, so a request elsewhere on the
server (`/admin/…`, `/user/tokens/…`, `/event/<id>/manage/…`, `/oauth/…`) never carries it, even
under a `full` grant (§9, G2). `path_prefix` is a literal prefix, so keep the trailing `/` where
shown.

| # | match | inject | Tier | Serves |
|---|---|---|---|---|
| H1 | `{instance}`, `/export/` | `bearer: "{credentials.access_token}"` | official | A2–A7, A9–A11 |
| H2 | `{instance}`, `/api/` | bearer | official | A1, A12 |
| H3 | `{instance}`, `/rooms/api/` | bearer | extended | B9–B10, B12–B24 |
| H4 | `{instance}`, `/user/api/favorites/` | bearer | extended | B3–B8 |
| H5 | `{instance}`, `/user/search/` | bearer | extended | B25, B26 |
| H6 | `{instance}`, `/search/api/search` | bearer | extended | B1 |
| H7 | `{instance}`, `/category/search` | bearer | extended | B2 |
| H8 | `{instance}`, `/event/create/prepare` | `[]` (nothing) | extended | B27 |

H8 injects nothing on purpose: preparing an event needs no login (appendix B27), and injecting a
token there would make Indico demand `full:everything` for a `POST`. The request is still an
`indico` app request, so grants, approvals and the request log apply. If the validator refuses an
empty `inject` without another mechanism, stop and report it (do not fall back to injecting).

## 3. Authentication

### 3.1 Methods offered
1. **Personal API token** (`kind: fields`). Indico ≥ 3.0; the user creates it on their profile's token
   page (`/user/tokens/`) with the scopes they want. Write menu paths in the guide as paths
   (`/user/tokens/`, `/user/applications/`, `/admin/apps/`): the UI labels were not verified. Field `access_token`, `type: secret`,
   `store: credentials`, label "Personal API token", placeholder `indp_…`, `help_url`
   `https://docs.getindico.io/en/stable/http-api/access/#api-token-authentication` (a static URL;
   `help_url` cannot point at the instance, G9).
2. **OAuth** (`kind: oauth2`) through an OAuth application that an **Indico administrator**
   registers on that Indico server (only admins can create applications, appendix §2). In Aurora
   it is a BYOC client bound to the instance (`app_oauth_clients.instance_id`, D-74).
3. **Legacy API keys** (`ak` + HMAC-SHA1 signature): **not supported** (§3.3).

Both supported methods produce the same credential, a bearer token, so one host injection serves
both.

### 3.2 Manifest sketch (fill in, keep the shapes)
```yaml
oauth_clients:
  - id: indico
    authorize_url: "https://{instance}/oauth/authorize"
    token_url: "https://{instance}/oauth/token"
    pkce: true                 # Indico requires PKCE, S256 only
    client_auth: body          # Indico accepts client_secret_post (and basic)
    body_format: form
    fields:
      - { name: client_id, label: Client ID }
      - { name: client_secret, label: Client secret, secret: true }
    setup_note: "An Indico administrator adds an application on that Indico server (/admin/apps/), registers the Aurora redirect URI and allows only the scopes you will offer. Every Indico server has its own application."
apps:
  - about:
      id: indico
      name: Indico
      description: Search and read events and meetings, book rooms and manage favorites on your Indico server
      category: productivity
      api_docs: https://docs.getindico.io/en/stable/http-api/
      connect_note:
        text: "An administrator registers your Indico server first (Instances). Bookings, favorites and room search by free time use Indico's internal endpoints and the broad 'Everything' scopes; read the setup guide before choosing them."
        url: https://docs.getindico.io/en/stable/http-api/access/
      label_hint: Indico account email
    instances: { … §2.2 … }
    credentials:
      - { name: access_token, secret: true }
    connect:
      - id: token
        kind: fields
        fields: [ { … §3.1 item 1 … } ]
        probe: { method: GET, url: "https://{instance}/api/user/", expect_status: [200], label: /email }
      - id: oauth
        kind: oauth2
        client: indico
        scopes: [ … §3.4 … ]
        default_scopes: ["read:user", "read:legacy_api"]
        callback: query
        token_response: { access_token: /access_token, scope: /scope }
        probe: { method: GET, url: "https://{instance}/api/user/", expect_status: [200], label: /email }
    # no `token:` section (static), see §3.5
```
No `metadata`, no `agent.env` (agent env cannot be per instance, D-79 (d)), no `default_blocklist`,
no `permissions` (no hooks, no context variables).

### 3.3 Why no legacy API keys
They need a per-request HMAC-SHA1 signature over the path and sorted query with a timestamp
(appendix §2). No mechanism in `09 §5` signs like that, and hooks never run on the request path
(`09 §1` principle 2), so it cannot be expressed (G1). Indico deprecated them in 3.0; bearer
tokens work on every Indico ≥ 3.0, and where an admin restricts token creation the admin can still
create a token for the user.

### 3.4 Scopes
| Scope (Indico) | `access` | Default | Needed by |
|---|---|---|---|
| `read:user` | read | yes | probe; `get_current_user` |
| `read:legacy_api` | read | yes | official read tools (H1) |
| `write:legacy_api` | write | no | `book_room` (A12). Indico checks `write:legacy_api` for any non-GET on the HTTP API; it does not include read. |
| `read:everything` | read | no | extended read tools (GET on any endpoint as the user) |
| `full:everything` | write | no | extended write tools except `prepare_event` (any method on any endpoint as the user: everything the person can do in Indico, including administration if they are an Indico admin) |

`registrants` is not offered. The `everything` scopes do **not** include `legacy_api`; a
connection that wants both tiers needs both (appendix §2). Scope names in the connect window:
plain words, the write ones clearly marked (as GitLab does).

### 3.5 Token lifecycle
Indico issues **no refresh token and no expiry** (appendix §2). So: omit `token` (static, like
`aws-keys`); `token_response` maps only `access_token` and `scope`; no `refresh_token` credential.
There are no `reauth_codes` because there is no refresh exchange. A token revoked in Indico (or
discarded by Indico's keep-limit of 50 tokens per user/app/scope set) shows up as an upstream
401 while the connection stays `active` until the user reconnects (G8). P2-CAT-T's replay rule
covers `oauth2_refresh` (and declarative mint) only, so this package has no refresh/re-auth
cases; §7 covers what remains.

### 3.6 What the Indico administrator configures
- Room booking enabled (`ENABLE_ROOMBOOKING`, default off) and, if restricted, the users allowed.
- For OAuth: the applications admin page (`/admin/apps/`): new application, redirect URI
  `<Aurora external URL>/v1/oauth/callback`, **allowed scopes** = exactly the scopes the org will
  offer (leaving `full:everything` out of the allowed scopes is the Indico-side switch that keeps
  the extended write tier off for every OAuth user), enabled; "trusted" (no consent screen)
  only if the org wants that. The server's `/.well-known/oauth-authorization-server` lists the
  authorize and token URLs to double-check.
- For personal tokens: whether users may create them (if restricted, admins create them).

## 4. Tool catalogue
Host is `{instance}` for every tool. Put the items in the manifest **in this order**: the
"before" notes are first-match overlaps that fixtures must prove (G6). Read tools are GET only.
Scope column = the Indico scope the *token* needs; the Aurora grant is separate.

### 4.1 Read tools (27)
| # | id | Name | Method | Path | Scope | Tier | Notes |
|---|---|---|---|---|---|---|---|
| R1 | `get_current_user` | Get the connected Indico user | GET | `/api/user/` | `read:user` | official | A1 |
| R2 | `list_favorite_category_events` | List events in my favorite categories | GET | `/export/categ/favorites*` | `read:legacy_api` | official | A3; **before R3** |
| R3 | `list_category_events` | List events in categories | GET | `/export/categ/*` | `read:legacy_api` | official | A2; `detail`, `from`/`to`, `type`, `location`, `room` are query parameters |
| R4 | `search_events` | Search events by title | GET | `/export/event/search/*` | `read:legacy_api` | official | A6; **before R6** |
| R5 | `get_event` | Read an event (contributions, sessions, attachments, notes) | GET | `/export/event/*` | `read:legacy_api` | official | A4 |
| R6 | `get_event_file` | Download an event file or read part of an event | GET | `/export/event/*/*/**` | `read:legacy_api` | official | A7 (`…/material/<folder>/<attachment>.bin`); the pattern also matches the undocumented per-session/contribution exports of the same scope, hence the name |
| R7 | `get_timetable` | Read an event timetable | GET | `/export/timetable/*` | `read:legacy_api` | official | A5 |
| R8 | `get_rooms` | Read rooms and their bookings | GET | `/export/room/*/*` | `read:legacy_api` | official | A9 (`detail=reservations`) |
| R9 | `find_room_by_name` | Find a room by name | GET | `/export/roomName/*/**` | `read:legacy_api` | official | A10; names may contain `/` (send it literally: the gateway refuses `%2F`) |
| R10 | `list_reservations` | List room bookings at a location | GET | `/export/reservation/*` | `read:legacy_api` | official | A11; `bookedfor`, `occurs`, `from`/`to` |
| R11 | `search` | Search events, contributions, categories, notes and files | GET | `/search/api/search` | `read:everything` | extended | B1 |
| R12 | `search_categories` | Search categories | GET | `/category/search` | `read:everything` | extended | B2 |
| R13 | `list_favorite_users` | List my favorite users | GET | `/user/api/favorites/users` | `read:everything` | extended | B3 |
| R14 | `list_favorite_categories` | List my favorite categories | GET | `/user/api/favorites/categories/**` | `read:everything` | extended | B5 (list and single check) |
| R15 | `list_favorite_events` | List my favorite events | GET | `/user/api/favorites/events/**` | `read:everything` | extended | B7 |
| R16 | `list_favorite_rooms` | List my favorite rooms | GET | `/rooms/api/user/favorite-rooms/**` | `read:everything` | extended | B9 (real path ends in `/`) |
| R17 | `list_room_locations` | List room booking locations | GET | `/rooms/api/locations` | `read:everything` | extended | B12 |
| R18 | `list_equipment_types` | List room equipment types | GET | `/rooms/api/equipment` | `read:everything` | extended | B13 |
| R19 | `list_all_rooms` | List all rooms | GET | `/rooms/api/rooms/` | `read:everything` | extended | B14 (trailing `/` is part of the pattern) |
| R20 | `search_rooms` | Search rooms, including rooms free in a time slot | GET | `/rooms/api/rooms/search` | `read:everything` | extended | B15; **before R22** |
| R21 | `get_room_availability` | Check a room's availability | GET | `/rooms/api/rooms/*/availability/**` | `read:everything` | extended | B17, B18 |
| R22 | `get_room_details` | Read a room's details | GET | `/rooms/api/rooms/*` | `read:everything` | extended | B16; also matches `/rooms/api/rooms/permissions` (a read) |
| R23 | `list_my_upcoming_bookings` | List my next room bookings | GET | `/rooms/api/bookings/mine` | `read:everything` | extended | B19 (next 5 only); **before R25** |
| R24 | `get_booking_calendar` | Read the booking calendar (optionally only mine) | GET | `/rooms/api/calendar` | `read:everything` | extended | B20; its `POST` form is not a tool |
| R25 | `get_booking` | Read a room booking | GET | `/rooms/api/bookings/*` | `read:everything` | extended | B21 |
| R26 | `get_user_search_token` | Start a user search | GET | `/user/search/token` | `read:everything` | extended | B25 |
| R27 | `search_users` | Search Indico users | GET | `/user/search/` | `read:everything` | extended | B26; returns personal data (names, emails, affiliations), §9 and Q-G |

No read wildcard (`all_read`) tool: the extended reads need a different, broader token scope, and a
wildcard would hide that from whoever writes the grant.

### 4.2 Write tools (13) and their approval summaries
Path segments are 1-based (`/rooms/api/bookings/42/cancel`: 1 `rooms`, 4 `42`). Every write tool
gets an `approvals` entry; summaries show ids, times, the person booked for and the reason, nothing
else.

| # | id | Name | Method | Path | Scope | Tier | Summary title | Summary fields (label ← source) | Link |
|---|---|---|---|---|---|---|---|---|---|
| W1 | `book_room` | Book a room (HTTP API) | POST | `/api/roomBooking/bookRoom*` | `write:legacy_api` | official | Book room | Room ID ← body `form: roomid`; From ← `form: from`; From (f) ← `form: f`; To ← `form: to`; To (t) ← `form: t`; Time zone ← `form: tz`; Booked for ← `form: username`; Reason ← `form: reason` | `https://{request.host}/rooms/` |
| W2 | `create_booking` | Book a room (room booking) | POST | `/rooms/api/booking/create` | `full:everything` | extended | Book room | Room ID ← `json: /room_id`; Start ← `/start_dt`; End ← `/end_dt`; Repeats ← `/repeat_frequency`; Repeat interval ← `/repeat_interval`; Booked for ← `/user`; Reason ← `/reason`; Pre-booking ← `/is_prebooking`; Admin override ← `/admin_override_enabled`; Linked to ← `/link_type`; Linked id ← `/link_id` | `https://{request.host}/rooms/` |
| W3 | `cancel_booking` | Cancel a room booking | POST | `/rooms/api/bookings/*/cancel` | `full:everything` | extended | Cancel room booking | Booking ← path 4 | `https://{request.host}/rooms/` |
| W4 | `cancel_booking_occurrence` | Cancel one date of a room booking | POST | `/rooms/api/bookings/*/*/cancel` | `full:everything` | extended | Cancel room booking date | Booking ← path 4; Date ← path 5 | `https://{request.host}/rooms/` |
| W5 | `add_favorite_user` | Add a favorite user | PUT | `/user/api/favorites/users/*` | `full:everything` | extended | Add favorite user | User ← path 5 | `https://{request.host}/user/favorites/` |
| W6 | `remove_favorite_user` | Remove a favorite user | DELETE | `/user/api/favorites/users/*` | `full:everything` | extended | Remove favorite user | User ← path 5 | same |
| W7 | `add_favorite_category` | Add a favorite category | PUT | `/user/api/favorites/categories/*` | `full:everything` | extended | Add favorite category | Category ← path 5 | same |
| W8 | `remove_favorite_category` | Remove a favorite category | DELETE | `/user/api/favorites/categories/*` | `full:everything` | extended | Remove favorite category | Category ← path 5 | same |
| W9 | `add_favorite_event` | Add a favorite event | PUT | `/user/api/favorites/events/*` | `full:everything` | extended | Add favorite event | Event ← path 5 | same |
| W10 | `remove_favorite_event` | Remove a favorite event | DELETE | `/user/api/favorites/events/*` | `full:everything` | extended | Remove favorite event | Event ← path 5 | same |
| W11 | `add_favorite_room` | Add a favorite room | PUT | `/rooms/api/user/favorite-rooms/*` | `full:everything` | extended | Add favorite room | Room ← path 5 | `https://{request.host}/rooms/` |
| W12 | `remove_favorite_room` | Remove a favorite room | DELETE | `/rooms/api/user/favorite-rooms/*` | `full:everything` | extended | Remove favorite room | Room ← path 5 | same |
| W13 | `prepare_event` | Prepare an event for me to create | POST | `/event/create/prepare` | none (H8 injects nothing) | extended | Prepare event | Title ← `json: /title`; Start ← `/start_dt`; Time zone ← `/tz`; Duration ← `/duration`; Type ← `/event_type` | — (the response carries the link) |

Classification notes (conservative, as with FU-P2-CAT-5a):
- Every non-GET is a write, even where it only stores a draft (W13) or toggles a favorite.
- **Add and remove are separate tools** because a summary cannot show the HTTP method (G4); one
  "add or remove" tool would show the approver the same text for both.
- **Reads sent as POST** (`POST /rooms/api/calendar`, `/rooms/api/timeline`,
  `/rooms/api/bookings/active`, `/rooms/api/bookings/linkable`) are not tools at all: reads must
  be GET, the GET forms cover the need, and under a custom grant an untooled request is blocked.
- `W1` aliases: Indico takes `f`/`t` before `from`/`to` (appendix §2), so both are shown; `tz`
  changes how the times are read, so it is shown too. Indico reads a `POST` to `/api/` from the
  form body only, so no query-string fields are needed.
- `W2` accepts JSON or form bodies in Indico; the summary decodes JSON only. The agent is told to
  send JSON; a form body yields a title-only summary (G4).
- Under H2, any other `POST /api/…` (e.g. a hook added by an Indico plugin) selects no tool and is
  blocked by a custom grant.

## 5. Feasibility per requested capability
| Capability | Official API | Undocumented route | Recommendation |
|---|---|---|---|
| Search events | **Yes**: A6 title search, A2 listing with filters, A3 favorite categories | B1 full search, B2 category search | Official in the base set; B1/B2 in the extended tier with the stability warning |
| Read events | **Yes**: A4, A5, A7 (attachments and notes appear in A4's data; field names undocumented) | — | Official |
| Create events | **No** | B28 creation form (XHR-only WTForms, prefixed JSON-encoded composite fields: fragile); B27 prepare-and-hand-off | Offer B27 only (extended, held); do not build on B28; ask the Indico project for an event-creation API (Q-B) |
| Create meetings | **No** (a meeting is the event type `meeting`) | same as above, `event_type: meeting` | Same as create events |
| Book a room | **Yes**: A12 (same day, direct bookings only, needs the booked-for login) | B22 (recurrence, pre-booking, link to an event) | A12 official write; B22 extended |
| Search available rooms | **Partly**: derive from A9 (`detail=reservations`) and A11 plus A10; misses blockings and unavailable periods (A12 refuses those at booking time) | B15, B18 (real availability) | B15/B18 extended; the agent may fall back to the official derivation |
| Read favorites | **Partly**: A3 (events of favorite categories) | B3, B5, B7, B9; B11 contributions | A3 official; B3/B5/B7/B9 extended; B11 deferred (Q-E) |
| Set favorites | **No** | B4, B6, B8, B10 | Extended, held for approval |
| Timetable, contributions, sessions, attachments, notes | **Yes** (A4, A5, A7) | — | Official |
| Room details, a room's bookings | **Yes** (A9, A10, A11) | B16, B17 | Both |
| My bookings | **Partly**: A11 `bookedfor=` matches the booked-for *name* (wildcards), not "made by me" | B19 (next 5), B20 (`my_bookings=true`), B21 | Both |
| Cancel my booking | **No** | B23, B24 | Extended, held |
| Current user | **Yes** (A1) | — | Official (also the probe) |
| Locations, equipment | **No** (locations only as known names) | B12, B13 | Extended |
| Find people (to favorite or book for) | **No** (A8 is deprecated and removed in 3.4) | B25 + B26 | Extended, personal data (Q-G) |

Stability: the official tier works on Indico ≥ 3.0 (bearer tokens). The extended tier is verified
against the 3.3.14 source only; favorite events need ≥ 3.2. Indico gives no compatibility promise
for it (appendix §1). The guide and the connect note say so (Q-H).

## 6. Contract gaps (decisions for the owner; do not change the contract)
| # | Gap | Impact | Workaround inside contract v1 | Hook? |
|---|---|---|---|---|
| G1 | No mechanism for per-request HMAC signing of query strings (legacy API keys) | Legacy keys unusable | Use bearer tokens (every Indico ≥ 3.0) | No: hooks never run on the request path (`09 §1`.2) |
| G2 | The extended tier needs Indico scopes that grant *all* of the user's power (`full:everything`); Aurora tools are then the only fine-grained boundary, and a `full` connection grant allows every request through the connection, including untooled ones, without approval (`07 §Grants`) | An agent with a full grant and a `full:everything` token can do anything the person can in Indico | Injection limited to 8 path prefixes (§2.3); default scopes read only; Indico admin can withhold `full:everything` from the OAuth app; connect note and guide tell users to grant the Indico connection with a **custom** grant (reads allow, writes ask) | No |
| G3 | One registered host serves exactly one app (P2-INST: an instance host is unique per scope across apps), so the package cannot ship `indico` and a separate `indico-extended` app that an org could make unavailable on its own | Extended tier can only be switched off by scopes and grants, not per app | Scopes (§3.4) and per-tool grants | No |
| G4 | Approval summaries cannot show the HTTP method, and cannot require fields or say that one is missing (empty sources are dropped) | A body in another encoding (multipart, form instead of JSON for W2/W13, unexpected aliases) gives a summary with missing fields | Separate add/remove tools; alias fields for `f`/`t`; agent told which encoding to send; guide tells approvers to refuse a booking whose summary lacks room or times | Partly: an `approval.summarize` hook could decode every encoding and flag missing fields. Not recommended now (adds a WASM guest for a cosmetic gain) |
| G5 | Tool matching cannot read query parameters (same root as FU-P2-CAT-5a / Q8) | None that matters here: every query-selected Indico variant (`detail=`, `my_bookings=`, `admin_override_enabled` on a GET search) is a read; POST-reads are excluded | Excluded POST-reads | No |
| G6 | First-match tool selection (reads before writes, declaration order) and the absence of segment exclusions are not stated in `09`; this package relies on four overlaps (R2/R3, R4/R6, R20/R22, R23/R25) | A reordering would silently rename tools in grants | Order fixed in §4, each overlap has a fixture | n/a: documentation (coordinator) |
| G7 | `{instance}` is a host; an Indico under a sub-path cannot be registered (superseded by D-231 for registered instances, FIX-145: a registration may carry a base path) | Such servers unsupported | None; the Indico admin can serve Indico at a host root | No |
| G8 | Static tokens have no re-auth signal (stale/reauth codes exist only for `oauth2_refresh`) and Aurora does not revoke upstream tokens when a connection is deleted (Indico offers `/oauth/revoke`) | Revoked tokens look like 401s on an `active` connection; deleted connections leave valid tokens in Indico | Guide: revoke in Indico (`/user/applications/` for OAuth, `/user/tokens/` for personal tokens) when disconnecting | No (`token.mint` does not run for static tokens) |
| G9 | `help_url` and `connect_note.url` cannot point at the instance | Users get the docs link, not their server's token page | Docs link plus the path `/user/tokens/` in the guide | No |

Multi-step flows (user search needs a token first; prepare-event returns a link) are ordinary
agent steps, not gaps. No hook is specified, so P2-CAT-H is not a dependency.

## 7. Fixtures plan (`plugins/builtin/indico/tests/*.yaml`)
Offline, recorded-HTTP shape, fake values only: host `indico.example.org` registered through
`connection.instance_host`, a second host `indico.other.example`, token `tok-fixture-1`, users
`User:101`, logins `jdoe`, rooms `57`, events `1234`. Never a real-looking `indp_`/`indo_` token.
Every case carries `connection.instance_host` (there is no default host). Suggested files:
`instance.yaml`, `read.yaml`, `write.yaml`, `boundaries.yaml`.

1. **Every host entry** H1–H8: at least one passing case whose request is handled by that entry
   (the checker picks the longest prefix). H8: the upstream request carries **no** `authorization`
   header; if the runner cannot assert absence, record that in the case name and report it.
2. **Every tool** (40): one positive case (`expect.tool`, plus `upstream.headers.authorization:
   "Bearer tok-fixture-1"` for at least one tool per host entry H1–H7) and one
   `negative <tool_id>: …` case. Good negatives: wrong method (`POST` to a read, `GET` to `book_room`,
   `DELETE` for an `add_*`, `PUT` for a `remove_*`), a neighbouring path.
3. **Ordering**: `/export/categ/favorites.json` → R2, `/export/categ/12.json` → R3;
   `/export/event/search/ichep.json` → R4; `/export/event/1234.json` → R5;
   `/export/event/1234/material/5/77.bin` → R6; `/rooms/api/rooms/search?…` → R20,
   `/rooms/api/rooms/57` → R22; `/rooms/api/bookings/mine` → R23, `/rooms/api/bookings/42` → R25.
4. **Every summary template** (13) with **every field label asserted** (the checker requires it):
   W1 needs one case with `from`/`to`/`tz` and one with `f`/`t`; W2 a JSON body with all eleven
   fields; W13 a JSON body. Bodies use the content type the agent is told to send
   (`application/x-www-form-urlencoded` for W1, `application/json` for W2 and W13). Add one W2 case
   with a form body asserting only the title (documents G4).
5. **Boundaries** (security fixtures): with the registered connection, `GET /user/tokens/`,
   `POST /user/tokens/new`, `GET /admin/apps/`, `POST /event/create/meeting`,
   `GET /user/101/api/favorites/users` and `GET /oauth/authorize` are `unmatched` (no injection);
   `POST /rooms/api/bookings/42/approve`, `POST /rooms/api/calendar` and `POST /api/other/x.json`
   are app `indico` with no tool.
6. **Instance binding** (as GitLab's `instance.yaml`): injected on `indico.example.org`;
   `unmatched` on `indico.other.example`; an unregistered host injects nothing.
7. **Token lifecycle** (P2-CAT-T): the package has no `token` section (static, §3.5), so there is
   no refresh or re-auth replay. One case per connect method's credential shape shows the bearer
   injected (both are `access_token`). If P2-CAT-T has merged with a rule that applies to static
   tokens, follow it; if it requires something this package cannot provide, stop and ask.

## 8. Setup guide outline (`plugins/builtin/indico/docs/setup.md`)
CommonMark, no raw HTML, ≤ 64 KiB; `aurora plugin docs` appends the generated permissions table.
1. **What this connects**: your organisation's Indico server; Aurora adds the token, agents never
   see it. Two tiers: the documented HTTP API, and Indico's internal endpoints (extended) with the
   compatibility warning.
2. **Register your Indico server in Aurora** (instance admin for everyone, or org owner/admin for
   one org): *Instances*, choose Indico, host name only (no `https://`, port or path); Aurora checks
   `GET /api/user/`. Registering decides where Indico credentials may go. Private addresses need
   `gateway.allowed_private`. Servers under a sub-path are not supported. Deleting the instance
   disables its connections and OAuth clients.
3. **Indico administrator**: enable room booking if wanted; register the OAuth application
   (`/admin/apps/`): redirect URI `<Aurora URL>/v1/oauth/callback`, allowed scopes, enabled;
   check `/.well-known/oauth-authorization-server`; enter Client ID and secret in Aurora under
   *Credentials, OAuth clients*, choosing the instance. Decide whether users may create personal
   tokens.
4. **Users: connect** with *Sign in with Indico* (OAuth) or a **personal API token**
   (`/user/tokens/`): which scopes to tick for which tier; `read:user` is
   required (Aurora uses it to check the token and name the connection).
5. **Permissions** (scope → what agents can do), mirroring §3.4; state plainly that
   `full:everything` lets the token do everything the person can do in Indico, that Aurora's tool
   grants are then the only limit, and that the Indico connection should be granted to agents with a
   **custom** grant (reads allowed, writes on ask), never `full`; avoid Indico admin accounts.
6. **What agents can do** (reads allowed by default in a custom grant) and **what is held for
   approval** (bookings, cancellations, favorites, preparing events), and what each approval shows.
   Approvers: refuse a booking whose summary lacks the room or the times.
7. **Limits**: no event creation API (the agent prepares, you finish in Indico); HTTP API bookings
   are same-day and direct only; extended endpoints may change with Indico upgrades; no legacy API
   keys; tokens do not expire, so revoke them in Indico (`/user/applications/`, `/user/tokens/`) when you
   disconnect; bookings are not idempotent (agents never retry them).

## 9. Security and privacy
- Every mutating tool is in `tools.write` with an approval template; with a custom grant (reads
  allow, writes ask) each one is held. Nothing in the read list writes.
- Token injection is limited to the eight prefixes of §2.3; `/user/tokens/…`, `/oauth/…`,
  `/admin/…` and event management never receive the token (fixtures in §7.5).
- Default OAuth scopes are `read:user read:legacy_api`; write and `everything` scopes are separate,
  marked choices. The guide and connect note carry the G2 warning.
- Summaries: ids, times, time zone, repeat settings, booked-for identifier or login, reason
  (clamped by `SUMMARY_LIMITS`), pre-booking/admin-override flags. No names, emails or event
  descriptions; never a header (the `authorization` header is not a permitted source anyway).
  "Admin override" is shown because it lets an Indico admin bypass booking rules.
- Personal data: event exports carry speaker and chair names (and, for managers, more); booking
  exports carry booked-for names; user search returns emails and affiliations. Agents only see
  what the Indico user may see; the guide names these tools so admins can block them per grant.
- Bookings are not idempotent: the agent instructions forbid automatic retries; the gateway only
  retries body-less requests on stale codes, which a static token never has.
- Rate limits: Indico documents none for the HTTP API. Exports cap records per request by detail
  level (in 3.3.14: events 1000, contributions 500, sessions 100, rooms 500, reservations 100; a
  larger `limit` is an error), and Indico caches export results per token (`nocache=yes` disables
  it). Source: SRC `indico/web/http_api/hooks/base.py`, `modules/events/api.py`, `modules/rb/api.py`.
  Suggest (guide) an Aurora rate-limit rule on `book_room`/`create_booking` if agents book often.
- Add an `ind[po]_` pattern to `scripts/guards/no-live-tokens.sh` (shared file: report it, do not
  edit; Q-I).

### Agent instructions (`agent.instructions`, ≤ 2 000 chars; write the final text, keep these points)
- Send requests to `https://<host>` of the Indico server the connection is registered for; Aurora
  adds the token: never ask for one, never add `ak`, `apikey`, `signature`, `timestamp` or
  `cookieauth`, never send this connection's requests to another host.
- Prefer the HTTP API: the `/export/…` paths of R2–R10 with their main parameters; files via
  `/export/event/<event>/material/<folder id>/<attachment id>.bin`; current user `/api/user/`.
- Book with `POST /api/roomBooking/bookRoom.json`, form-encoded, `roomid`, `location`, `from`,
  `to` (`YYYY-MM-DDTHH:MM`, same day), `tz`, `reason`, `username` (login of the person booked for);
  long parameter names only.
- If granted, the `/rooms/api/…`, `/user/api/favorites/…`, `/user/search/…`, `/search/api/search`
  and `/event/create/prepare` endpoints are internal to Indico and may change; on a 404 or an
  unexpected shape, say so instead of guessing. Send JSON bodies to them.
- Confirm room, time and person before booking; never retry a booking automatically. To create
  an event, prepare it and give the user the returned link; Indico has no API that creates events.

## 10. Constraints, dependencies, acceptance, model
### Constraints / file ownership
- You own `plugins/builtin/indico/**` only: `plugin.yaml`, `docs/setup.md`, `assets/icon.png`
  (neutral, non-branded, square PNG ≥ 64 px, ≤ 256 KiB; no Indico logo), `tests/*.yaml`. Follow
  `plugins/builtin/README.md`. No `hooks/`.
- Do not change the manifest contract, gateway mechanisms, `check_builtin.py`, guards, CI or the
  README; report needed changes (Q-I) in your completion report.
- Use only endpoints and scopes in the appendix. If an endpoint misbehaves against the contract
  (e.g. the validator refuses a pattern), leave the tool out and report it; do not invent another.
- Edit files with the file tools only (owner rule).

### Dependencies
- Blocked by: nothing (Q-A and Q-B answered, D-84; P2-INST merged).
- P2-CAT-T: start after it has merged so `check_builtin.py`'s lifecycle rule is final; this
  package is static, so the rule is expected not to apply (§7.7).
- Not needed: P2-CAT-H (no hook).
- Unblocks: nothing.

### Acceptance criteria
- [ ] `aurora plugin validate plugins/builtin/indico` passes (unsigned allowed); publisher `aurora`.
- [ ] `aurora plugin test plugins/builtin/indico` passes and covers §7 items 1–7.
- [ ] `bash scripts/plugins/check-builtin.sh indico` passes (coverage, scopes, docs rendered twice
      identically with the permissions table).
- [ ] `bash scripts/guards/run-all.sh` passes (incl. `no-live-tokens.sh`).
- [ ] `bash scripts/plugins/build-builtin.sh --out <dir>` builds the archive; the dev stack lists
      Indico, an instance can be registered (probe `GET /api/user/`), and both connect methods
      appear (manual check or e2e if one exists; report which).
- [ ] Tool ids, scopes, host entries and summaries match §2–§4 (or the reduced set if Q-A = no).
- [ ] Completion report: files, exact command results, any tool left out and why, needed shared
      changes (no-live-tokens pattern, `09` note on tool order).

### Suggested agent/model
Sonnet, own worktree. One manifest, ~100 fixture cases with order-sensitive matching and summary
coverage; Haiku is likely to miss ordering and boundary cases. Effort M.

## 11. Decisions needed from the owner

**Answered 2026-11-08 (D-84):** the owner accepted every recommendation: Q-A yes (extended tier, opt-in, with the compatibility warning), Q-B yes (prepare and hand off), Q-C yes (mitigations now; phase-4 item P4-GRANTS for an app-level "custom grants only" option), Q-G include (named in the guide). Q-D, Q-E, Q-F and Q-H take their recommendations. Q-I is approved by the coordinator: **this lane may make both shared changes** (the `ind[po]_` pattern in `scripts/guards/no-live-tokens.sh`, and the first-match tool-order note in `09 §3.4`), as an exception to the file-ownership rule above.

- **Q-A Extended tier.** Ship the undocumented-endpoint tier (favorites, room search by free time,
  my bookings, cancel, room-booking-app booking, search, prepare event) in phase 2, behind the
  opt-in scopes `read:everything` / `full:everything`, with the compatibility warning?
  *Recommendation: yes.* Without it, set favorites, cancel a booking and real availability search
  are impossible and favorites/my-bookings reads are partial; the risk is bounded by narrow
  injection, read-only defaults, per-tool grants and the Indico admin's control of allowed scopes.
- **Q-B Event creation.** Indico has no event-creation API. Accept "prepare and hand off" (B27: the
  agent prepares, the person finishes in Indico) as the phase-2 answer, and not build on the
  creation form (B28)? *Recommendation: yes;* raise an event-creation API request with the Indico
  project separately.
- **Q-C `full:everything` exposure (G2).** Accept the mitigations (prefix-limited injection, warnings,
  custom grants recommended) for phase 2, and record a phase-4 design-review item for a manifest
  option that forbids `full` grants or always blocks untooled requests for an app?
  *Recommendation: yes,* alongside Q8 (FU-P2-CAT-5a).
- **Q-D Legacy API keys.** Leave them unsupported (G1)? *Recommendation: yes;* deprecated since 3.0
  and not expressible.
- **Q-E Contribution favorites and category info** (B11, B29). They need the token injected on the
  whole `/event/` or `/category/` tree, including management pages. Defer them?
  *Recommendation: defer;* revisit if the contract gains finer host claims or Indico documents them.
- **Q-F Default host.** Confirm: no `default_host` (D-63); a deployment pre-registers its own
  server as an instance-wide app instance. *Recommendation: yes.*
- **Q-G User search** (B25/B26) returns names, emails and affiliations. Include it as an extended
  read tool (needed to favorite people or book for them), named in the guide so admins can block
  it? *Recommendation: include.*
- **Q-H Supported versions.** State "official tier: Indico ≥ 3.0; extended tier: verified against
  3.3.14, may break on upgrades" in the guide and catalogue description? *Recommendation: yes.*
- **Q-I Shared follow-ups (coordinator):** add `ind[po]_[A-Za-z0-9]{40,}` to
  `scripts/guards/no-live-tokens.sh`, and document first-match tool order (reads before writes,
  declaration order) in `09 §3.4` (G6). *Recommendation: yes,* both are small and outside this
  lane's files.
