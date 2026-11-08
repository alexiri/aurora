# P2-CAT-7 appendix: Indico endpoint inventory and evidence

Companion to `docs/tasks/P2-CAT-7.md`. Every endpoint, scope and behaviour the brief relies on is
listed here with its source. Read this when you need to know *why* a tool exists, which scope it
needs, or how stable it is. Nothing here may be extended from memory: if an endpoint is not in
this file, it is not in the package.

## 1. Sources and citation keys

Official documentation (Indico docs, "stable" = 3.3.x; the docs footer reads Indico 3.3.14):

| Key | URL |
|---|---|
| D-ACC | https://docs.getindico.io/en/stable/http-api/access/ (URL structure, API token authentication, scopes, deprecated API keys) |
| D-COM | https://docs.getindico.io/en/stable/http-api/common/ (common parameters) |
| D-CAT | https://docs.getindico.io/en/stable/http-api/exporters/categ/ |
| D-EVT | https://docs.getindico.io/en/stable/http-api/exporters/event/ |
| D-TT | https://docs.getindico.io/en/stable/http-api/exporters/timetable/ |
| D-SRCH | https://docs.getindico.io/en/stable/http-api/exporters/eventsearch/ |
| D-FILE | https://docs.getindico.io/en/stable/http-api/exporters/file/ |
| D-USER | https://docs.getindico.io/en/stable/http-api/exporters/user/ |
| D-RB | https://docs.getindico.io/en/stable/http-api/exporters/room_booking/ |
| D-OAM | https://docs.getindico.io/en/stable/api/oauth/ (OAuth application and token models, Python reference) |
| D-CHG | https://docs.getindico.io/en/stable/changelog/ |
| D-CFG | https://docs.getindico.io/en/stable/config/settings/ (`ENABLE_ROOMBOOKING`, `BASE_URL`) |

The section "API reference" (`/en/stable/api/...`) documents Indico's Python internals (models,
utilities), **not** HTTP endpoints. The HTTP API is only what the `http-api/` pages describe.

Undocumented endpoints are cited in one of two ways, always marked **undocumented**:

| Key | Source |
|---|---|
| MCP:`file`:`function` | `cohm/indico-mcp`, commit `d62a6ac20ec058a4437add7d5de548f6ef77f22d` (2026-04-15), used only as a list of operations and endpoints; no code or text is reused |
| SRC:`path` | Indico source, tag `v3.3.14` (commit `da01c09ce42b9bcdd313e266024a13a6f010aadd`), the route table (`blueprint.py`) and handler of each endpoint. Used where the owner asked for an operation (favorites, cancelling a booking, room availability, event creation) that neither the docs nor indico-mcp cover. Source code is evidence of what exists in that release, not a compatibility promise. |

D-ACC states the stability rule that applies to everything marked undocumented: *"We make
absolutely no promises of backwards compatibility on endpoints that are not part of documented
APIs. You use them at your own risk."*

## 2. Authentication facts

| Fact | Source |
|---|---|
| Personal API tokens (since 3.0) are sent as `Authorization: Bearer <token>`; format `indp_` + 42 chars; tokens from an OAuth flow have prefix `indo_`. They "behave exactly like OAuth tokens". | D-ACC §API Token Authentication |
| Admins can restrict token creation (then only admins create tokens or manage their scopes; users can still reset an existing token). | D-ACC note; D-CHG 3.0rc2 |
| Scopes: `read:user` (User information, read only), `read:legacy_api` (Classic API, read only), `write:legacy_api` (Classic API, write only), `registrants` (Event registrants), `read:everything` (Everything, only GET), `full:everything` (Everything, all methods). | D-ACC §Scopes (rendered list); SRC:`indico/core/oauth/scopes.py` |
| The `everything` scopes work on **any** Indico endpoint, including attachments, i.e. they are not restricted to official APIs. Endpoints covered by the `legacy_api` scopes are **not** included in `everything`; they must be granted explicitly. | D-ACC §Scopes |
| `legacy_api` covers `/export/` (read) and "some `/api/` paths" (write). | D-ACC §Scopes |
| `read:user` covers `GET /api/user/` → `{admin, email, first_name, id, last_name}`. | D-ACC §Scopes |
| On the HTTP API a `GET` needs `read:legacy_api`, any other method `write:legacy_api`. | SRC:`indico/web/http_api/handlers.py` (`handler`) |
| `/api/<path>` accepts only `POST`; `/export/<path>` is read. A `POST` to the HTTP API reads its parameters from the **form body only** (`request.form`), never from the query string. | SRC:`indico/modules/api/blueprint.py`; SRC:`indico/web/http_api/handlers.py` |
| Parameter aliases: the first alias found wins (`f` before `from`, `t` before `to`). | D-COM (short forms); SRC:`indico/web/http_api/util.py` (`get_query_parameter`), `hooks/base.py` |
| Web (non-HTTP-API) endpoints accept bearer tokens with `read:everything` or `full:everything` for `GET`, and only `full:everything` for any other method, unless the endpoint declares its own scope (e.g. `read:user`). | SRC:`indico/web/util.py` (`_lookup_request_user`), `indico/web/rh.py` (`oauth_scope`) |
| CSRF checks are skipped for requests authenticated by a bearer token (OAuth or personal) or a signed URL. | SRC:`indico/web/rh.py` (`_check_csrf`) |
| Deprecated API keys: `ak`/`apikey`, `timestamp`, `signature` query parameters; signature = hex HMAC-SHA1 over path + sorted query string with the secret key; signed URLs expire within seconds/minutes; "persistent signatures" never expire. `onlypublic=yes` restricts to public data. `cookieauth=yes` reuses a browser session (GET only, CSRF token required). | D-ACC §API Key Authentication (Deprecated) |
| Only Indico administrators can create OAuth applications. | D-CHG 2.3.1 note |
| OAuth provider: authorization code flow with PKCE (implicit flow removed), metadata at a well-known URI (RFC 8414), token introspection and revocation endpoints; tokens stored hashed; for a user/app/scope combination only a limited number of tokens are kept and older ones are discarded. | D-CHG 3.0rc1 |
| Endpoints: `GET /.well-known/oauth-authorization-server`, `GET,POST /oauth/authorize`, `POST /oauth/token`, `POST /oauth/introspect`, `POST /oauth/revoke`. Admin UI for applications: `/admin/apps/`. User UI: `/user/applications/` (authorized apps, revoke), `/user/tokens/` (personal tokens). | **undocumented** SRC:`indico/modules/oauth/blueprint.py` (the metadata document of the instance lists the authorize and token URLs; admins should check it) |
| PKCE is **required** (method `S256` only); token endpoint client auth `client_secret_basic`, `client_secret_post` or `none`. | **undocumented** SRC:`indico/core/oauth/oauth2.py`, `grants.py` |
| Indico issues **no refresh token** and **no expiry** (only the authorization-code grant is registered; `OAUTH2_TOKEN_EXPIRES_IN` for it is 0). Token model has no expiry or refresh fields. The keep-limit is 50 tokens per user/app/scope set. | **undocumented** SRC:`indico/core/oauth/oauth2.py`, `util.py` (`MAX_TOKENS_PER_SCOPE`, `save_token`); D-OAM (`OAuthToken` fields) |
| OAuth application attributes an admin sets: name, description, redirect URIs (scheme and host/port must match exactly; path must start with the registered path), allowed scopes, enabled, trusted (no consent screen), PKCE without secret. A token request for a scope outside the allowed scopes fails (`invalid_scope`). | D-OAM (`OAuthApplication`); SRC:`indico/core/oauth/grants.py` (`validate_requested_scope`) |
| Room booking must be enabled by the server admin (`ENABLE_ROOMBOOKING`, default `False`) and may be restricted to users/groups. | D-CFG; D-RB |

## 3. Documented endpoints (official HTTP API)

All are relative to the instance base URL; `TYPE` is the output format (`json`, `jsonp`, `xml`,
`html`, `ics`, `atom`, `bin`; not every element supports every format). Common parameters
(`from/to`, `limit`, `offset`, `detail`, `order`, `descending`, `tz`, `onlypublic`, `nocache`,
`pretty`) apply to every export (D-COM).

| # | Method + path | Purpose | Scope | Source | indico-mcp use |
|---|---|---|---|---|---|
| A1 | `GET /api/user/` | Current user (id, names, email, admin flag) | `read:user` | D-ACC | — |
| A2 | `GET /export/categ/ID.TYPE` | Events in one or more categories (`-`-separated ids); filters `location`, `room` (wildcards), `type` (`simple_event`/`lecture`, `meeting`, `conference`); `detail=events` (+ `contributions` etc. per D-EVT detail levels) | `read:legacy_api` | D-CAT | MCP:`server.py`:`search_category_events`, `browse_category`, `get_category_contributions` |
| A3 | `GET /export/categ/favorites.TYPE` | Special id: in an authenticated request, the user's **favorite categories** (their events) | `read:legacy_api` | D-CAT | — |
| A4 | `GET /export/event/ID.TYPE` | Event metadata; `occ=yes` daily times; `detail=contributions`, `subcontributions`, `sessions` | `read:legacy_api` | D-EVT | MCP:`server.py`:`get_event_details`, `get_event_contributions`, `get_event_sessions`, `list_event_attachments` |
| A5 | `GET /export/timetable/ID.TYPE` | Event timetable | `read:legacy_api` | D-TT | — |
| A6 | `GET /export/event/search/TERM.TYPE` | Event title search (id, title, start, protection flag) | `read:legacy_api` | D-SRCH | MCP:`server.py`:`search_events_by_keyword` (fallback path) |
| A7 | `GET /export/event/EVENT_ID[/session/S][/contrib/C][/subcontrib/SC]/material/MATERIAL_ID/RESOURCE_ID.bin` | Download an attached file (only `bin`) | `read:legacy_api` | D-FILE | — (indico-mcp downloads the web `download_url` instead, see B-row) |
| A8 | `GET /export/user/USER_ID.TYPE` | User data. **Deprecated in 3.3.8, removed in 3.4**; use A1. | `read:legacy_api` | D-USER | — (excluded) |
| A9 | `GET /export/room/LOCATION/ID.TYPE` | Room data (one or `-`-separated ids); `detail=reservations` with `from/to` adds bookings; room-booking filters | `read:legacy_api` | D-RB §Rooms | — |
| A10 | `GET /export/roomName/LOCATION/ROOMNAME.TYPE` | Find a room by name (public room data; names may contain `/` since 2.2.7) | `read:legacy_api` | D-RB §Get room by room name; D-CHG ("Allow slashes in roomName export API") | MCP:`server.py`:`search_rooms`, `find_available_rooms` |
| A11 | `GET /export/reservation/LOCATION.TYPE` | Bookings at one or more locations; filters `occurrences`, `cancelled`, `rejected`, `confirmed`, `archival`, `recurring`, `bookedfor` (wildcard on the booked-for name), `occurs` (dates) | `read:legacy_api` | D-RB §Retrieving bookings | MCP:`server.py`:`discover_rooms`, `search_rooms`, `find_available_rooms`, `get_room_reservations` |
| A12 | `POST /api/roomBooking/bookRoom.TYPE` (`json`/`xml`), form body | Book a room: `location`, `roomid`, `from`/`to` (or `f`/`t`), `reason`, `username` (login of the person booked for). Fails on collision with a booking, blocking or unavailable period; no pre-booking. | `write:legacy_api` | D-RB §Creating bookings | MCP:`server.py`:`book_room` |

Behaviour of A12 visible in the source (SRC:`indico/modules/rb/api.py`, `BookRoomHook`), relevant
to the summary and the agent instructions: `from` and `to` must be on the **same day**, in the
future, `to` after `from`; dates are interpreted in `tz` (default: server timezone); `username`
is **required** and matched against login identities (ambiguous → error); `location` is not used
to find the room (`roomid` is); a room that only allows pre-bookings is refused; the booking is
created by the token's user for the `username` user.

The event export also carries, in current Indico, the attachment folders (`folders[].attachments[]`
with `id`, `title`, `filename`, `content_type`, `size`, `is_protected`, `download_url`) and
minutes/notes (`note`). This is visible in the source (SRC:`indico/modules/events/api.py`,
`indico/modules/attachments/api/util.py`) and used by MCP:`server.py`:`list_event_attachments`, but
the docs examples do not show it: treat the field names as **undocumented**. `download_url` points
to a web route that needs `read:everything`; with `legacy_api` only, download through A7 using the
folder and attachment ids.

## 4. Undocumented endpoints (source-verified, Indico 3.3.14)

Stability judgement scale: **internal-UI** = JSON endpoint of Indico's own React/JS frontend,
changes whenever the UI changes, no deprecation policy; **internal-form** = HTML/WTForms dialog
endpoint, the least stable.

### 4.1 Search

| # | Method + path | Purpose | Scope | Source | Stability |
|---|---|---|---|---|---|
| B1 | `GET /search/api/search?q=…&type=…` | Instance-wide search; `type` one or more of `category`, `event`, `contribution`, `subcontribution`, `event_note`, `attachment`; `page` | `read:everything` | SRC:`indico/modules/search/blueprint.py`, `controllers.py` (`RHAPISearch`), `base.py` (`SearchTarget`) | internal-UI (3.0+ search module; backend may be a plugin with other results) |
| B2 | `GET /category/search?q=…` | Category search by title (≤ 10 results, with path) | `read:everything` | MCP:`server.py`:`search_categories`; SRC:`indico/modules/categories/blueprint.py` (`RHCategorySearch`) | internal-UI |

indico-mcp also calls three things that do **not** work as written on 3.3.14 and must not be used:
`GET /export/categ/.json?q=…` (MCP:`server.py`:`find_events_by_title`; the category export has no
`q` parameter and an empty id does not match its route), `GET /api/search/` and
`GET /api/categories/ID/` (MCP:`server.py`:`search_events_by_keyword`, `list_category_info`; the
`/api/` prefix accepts only `POST` and has no such hooks). SRC:`indico/modules/api/blueprint.py`,
`indico/modules/events/api.py`.

### 4.2 Favorites

"Since" is the changelog entry where one exists; otherwise "≤ 3.3.14" (present in the reviewed
release, introduction version not checked).

| # | Method + path | Purpose | Scope | Source | Since |
|---|---|---|---|---|---|
| B3 | `GET /user/api/favorites/users` | My favorite users (list of identifiers such as `User:123`) | `read:everything` | SRC:`indico/modules/users/blueprint.py`, `controllers.py` (`RHUserFavoritesAPI`) | ≤ 3.3.14 |
| B4 | `PUT`/`DELETE /user/api/favorites/users/IDENTIFIER` | Add/remove a favorite user | `full:everything` | same | ≤ 3.3.14 |
| B5 | `GET /user/api/favorites/categories[/ID]` | My favorite categories / whether one is a favorite | `read:everything` | SRC: same (`RHUserFavoritesCategoryAPI`) | ≤ 3.3.14 |
| B6 | `PUT`/`DELETE /user/api/favorites/categories/ID` | Add/remove a favorite category | `full:everything` | same | ≤ 3.3.14 |
| B7 | `GET /user/api/favorites/events[/ID]` | My favorite events / whether one is a favorite | `read:everything` | SRC: same (`RHUserFavoritesEventAPI`) | 3.2 (D-CHG "Allow events to be favorited") |
| B8 | `PUT`/`DELETE /user/api/favorites/events/ID` | Add/remove a favorite event | `full:everything` | same | 3.2 |
| B9 | `GET /rooms/api/user/favorite-rooms/` | My favorite room ids | `read:everything` | SRC:`indico/modules/rb/blueprint.py`, `controllers/backend/rooms.py` (`RHRoomFavorites`) | ≤ 3.3.14 |
| B10 | `PUT`/`DELETE /rooms/api/user/favorite-rooms/ID` | Add/remove a favorite room | `full:everything` | same | ≤ 3.3.14 |
| B11 | `GET`/`PUT`/`DELETE /event/EVENT_ID/api/contributions/favorites[/CONTRIB_ID]` | Favorite contributions | `read:everything` / `full:everything` | SRC:`indico/modules/events/contributions/blueprint.py` | 3.3.13 (D-CHG) — **deferred**, see brief |

The same favorites routes also exist under `/user/USER_ID/…` (acting on another user, admins
only). They are deliberately **not** tools.

### 4.3 Room booking (the room booking app's JSON backend, prefix `/rooms`)

All need room booking enabled and room-booking access (SRC:`indico/modules/rb/controllers/__init__.py`).

| # | Method + path | Purpose | Scope | Source |
|---|---|---|---|---|
| B12 | `GET /rooms/api/locations` | Locations (sites) | `read:everything` | SRC:`indico/modules/rb/blueprint.py` |
| B13 | `GET /rooms/api/equipment` | Equipment types (search filter values) | `read:everything` | same |
| B14 | `GET /rooms/api/rooms/` | All rooms the user may see | `read:everything` | same (`RHRooms`) |
| B15 | `GET /rooms/api/rooms/search?…` | Room search; with `start_dt`, `end_dt`, `repeat_frequency` (`NEVER`/`DAY`/`WEEK`/`MONTH`), `repeat_interval` it filters by **availability** (bookings, blockings, unavailable periods); other filters `capacity`, `equipment`, `feature`, `favorite`, `mine`, `text`, `division`, `building`, `location_id`, map bounds | `read:everything` | SRC:`controllers/backend/rooms.py` (`RHSearchRooms`), `controllers/backend/common.py` (`search_room_args`) |
| B16 | `GET /rooms/api/rooms/ID` | Room details | `read:everything` | SRC (`RHRoom`) |
| B17 | `GET /rooms/api/rooms/ID/availability` | Availability for the next days | `read:everything` | SRC (`RHRoomAvailability`) |
| B18 | `GET /rooms/api/rooms/ID/availability/simple?start_dt=…&end_dt=…` | Is this room free in this slot | `read:everything` | SRC (`RHCheckRoomAvailable`) |
| B19 | `GET /rooms/api/bookings/mine` | My next 5 upcoming booking occurrences (booked by me or for me) | `read:everything` | SRC:`controllers/backend/bookings.py` (`RHMyUpcomingBookings`) |
| B20 | `GET /rooms/api/calendar?start_date=…&end_date=…&my_bookings=true&text=…` | Calendar of bookings (optionally only mine) | `read:everything` | SRC (`RHCalendar`; also accepts `POST`, excluded) |
| B21 | `GET /rooms/api/bookings/ID` | Booking details | `read:everything` | SRC (`RHBookingDetails`) |
| B22 | `POST /rooms/api/booking/create` (JSON) | Create a booking: `start_dt`, `end_dt`, `repeat_frequency`, `repeat_interval`, `recurrence_weekdays`, `room_id`, `user` (principal identifier of the person booked for), `reason`, `internal_note`, `is_prebooking`, `link_type`/`link_id`/`link_back`, `admin_override_enabled`, `extra_fields` | `full:everything` | SRC (`RHCreateBooking`), `indico/modules/rb/schemas.py` (`CreateBookingSchema`); body JSON or form (`json_or_form`, SRC:`indico/web/args.py`) |
| B23 | `POST /rooms/api/bookings/ID/cancel` | Cancel a booking (Indico checks the user may cancel it) | `full:everything` | SRC (`RHBookingStateActions`) |
| B24 | `POST /rooms/api/bookings/ID/DATE/cancel` | Cancel one occurrence (date) of a booking | `full:everything` | SRC (`RHBookingOccurrenceStateActions`) |

Seen and **excluded**: `POST /rooms/api/bookings/ID/approve|reject` (manager actions),
`DELETE`/`PATCH /rooms/api/bookings/ID` (delete is admin-only; edit is a complex flow),
`POST /rooms/api/bookings/active`, `POST /rooms/api/bookings/linkable`, `POST /rooms/api/timeline`,
`POST /rooms/api/calendar` (reads sent as POST: they cannot be read tools and are not needed),
`/rooms/api/blockings/**`, `/rooms/api/admin/**`.

### 4.4 Users and events

| # | Method + path | Purpose | Scope | Source | Stability |
|---|---|---|---|---|---|
| B25 | `GET /user/search/token` | Short-lived token required by user search | `read:everything` | SRC:`indico/modules/users/blueprint.py` (`RHUserSearchToken`) | internal-UI |
| B26 | `GET /user/search/?token=…&…` | Search users (returns names, emails, affiliations, identifiers) | `read:everything` | SRC (`RHUserSearch`) | internal-UI |
| B27 | `POST /event/create/prepare` (JSON or form) | Store `title`, `start_dt`, `tz`, `duration`, `event_type` for one hour and return a link (`…/#create-event:meeting::<uuid>`) that opens Indico's event-creation dialog prefilled; **does not create an event**; CSRF disabled, no login required | none needed (see brief) | SRC:`indico/modules/events/blueprint.py`, `controllers/creation.py` (`RHPrepareEvent`) | internal-UI |
| B28 | `GET`/`POST /event/create/lecture\|meeting\|conference` | The event-creation dialog: XHR-only (`X-Requested-With`), WTForms with prefix `event-creation-`, JSON-encoded composite fields (category, location, dates, persons) | `full:everything` | SRC (`RHCreateEvent`, `indico/modules/events/forms.py`) | internal-form — **not offered**, see brief |
| B29 | `GET /category/ID/info` | Category with subcategories | `read:everything` | SRC:`indico/modules/categories/blueprint.py` | internal-UI — **deferred**, see brief |
| B30 | `GET /event/ID/attachments/FOLDER/ATTACHMENT/FILENAME` | Web download of an attachment (the export's `download_url`) | `read:everything` | MCP:`server.py`:`download_attachment`; SRC:`indico/modules/attachments/blueprint.py` | web route — not offered (A7 covers downloads) |

## 5. indico-mcp operations and their status here

| indico-mcp tool | Endpoint it uses | Status in this package |
|---|---|---|
| `list_instances` | none (local config) | n/a: Aurora's registered instances |
| `search_categories` | B2 | B2 tool `search_categories` (undocumented tier) |
| `find_events_by_title` | `/export/categ/.json?q=` | not used (does not exist); A6 / B1 instead |
| `browse_category` | A2 | `list_category_events` |
| `search_category_events` | A2 | `list_category_events` |
| `get_category_contributions` | A2 `detail=contributions` | `list_category_events` |
| `get_event_details`, `get_event_contributions`, `get_event_sessions` | A4 | `get_event` |
| `search_events_by_keyword` | `/api/search/` then A6 | A6 `search_events`; B1 `search` |
| `list_category_info` | `/api/categories/ID/` then A2 | not used (does not exist); B29 deferred |
| `list_event_attachments` | A4 (folders) | `get_event` |
| `download_attachment` | B30 (any URL) | A7 `get_event_file` |
| `list_room_locations` | none (local config/cache) | B12 `list_room_locations` |
| `discover_rooms` | A11 (scan) | not needed: B14 lists rooms; A11 available |
| `search_rooms` | A10, A11 | A10 `find_room_by_name`; B15 `search_rooms` |
| `find_available_rooms` | A11 diff, A10 | official approximation via A9/A11; B15 for true availability |
| `get_room_reservations` | A11 | `list_reservations` |
| `book_room` | A12 | `book_room` |

## 6. What Indico does not offer at all (3.3.14)

- No documented API to **create** or **edit** events, meetings, contributions or timetables
  (the only legacy write hook in core is A12; SRC:`indico/modules/rb/api.py`, every other
  registered HTTP API hook is a read).
- No documented API for **favorites** except reading the events of favorite categories (A3).
- No documented API to **cancel** or list "my" bookings (A11 with `bookedfor` matches the booked-for
  *name*, not "created by me").
- No documented availability query: availability must be derived from A9/A11, which ignores
  blockings and unavailable periods (A12 refuses those at booking time).
- No refresh tokens, no token expiry (section 2).
