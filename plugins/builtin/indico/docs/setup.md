# Connect Indico

This package connects **your organisation's own Indico server**. Indico is open-source software
that every organisation runs itself, so Aurora has no default Indico: an administrator registers
the server first, then people connect to it. Aurora adds the token to requests your agents send to
that server; agents never see it.

The package has two tiers:

- **Official tier**: Indico's documented HTTP API (`/export/…` and `/api/…`). It works on
  Indico 3.0 and later and needs the scopes `read:user`, `read:legacy_api` and (for booking)
  `write:legacy_api`.
- **Extended tier (opt-in)**: Indico's own user-interface endpoints (`/rooms/api/…`,
  `/user/api/favorites/…`, `/user/search/…`, `/search/api/search`, `/category/search`,
  `/event/create/prepare`). They cover what the HTTP API cannot do: rooms free in a time slot, your
  bookings, cancelling a booking, favorites, search, finding people, and preparing an event.
  **Indico makes no promise that these endpoints stay compatible** between versions. They were
  verified against Indico 3.3.14 only (favorite events need 3.2 or later) and may break when
  your Indico is upgraded. They need the broad scopes `read:everything` and `full:everything`.

## 1. Register your Indico server in Aurora (administrators)

Open {{aurora.page:self_hosted}} (organization owners and admins for one organization, instance
admins for everyone), choose Indico and enter the host name, for example `indico.example.org` (host name
only: no `https://`, port or path). Aurora checks `GET /api/user/` first.

- Registering decides where Indico credentials may be sent, so only register servers you trust.
- An Indico served under a sub-path (`https://example.org/indico/`) cannot be registered. Serve
  Indico at the root of a host name instead.
- If the server has a private address, the operator must allow it with `gateway.allowed_private`.
- Deleting an instance disables its connections and its OAuth clients.

## 2. Prepare Indico (Indico administrators)

- Room booking is off by default in Indico (`ENABLE_ROOMBOOKING`). Enable it if you want the room
  tools, and restrict it to the users you want, if needed.
- **Personal tokens**: decide whether users may create them (`/user/tokens/`). If you restrict
  this, administrators create the tokens for users.
- **Sign in with Indico (OAuth)**: add an application at `/admin/apps/`:
  1. Redirect URI: {{aurora.callback_url}}
  2. **Allowed scopes**: exactly the scopes you are willing to offer. Leaving `full:everything`
     out is the Indico-side switch that keeps the extended write tools off for every OAuth user.
  3. Enable the application. Mark it *trusted* (no consent screen) only if you want that.
  4. Check `/.well-known/oauth-authorization-server` on your Indico: it lists the authorize and
     token URLs Aurora uses (`/oauth/authorize` and `/oauth/token`). Indico requires PKCE; Aurora
     uses it.
  5. Enter the Client ID and Client secret in Aurora under *Credentials, OAuth clients*, choosing
     the instance. Every Indico server has its own application.

## 3. Connect (everyone)

Choose the Indico server, then one of:

- **Personal API token**. In Indico open `/user/tokens/`, create a token and tick the scopes you
  want (the menu labels differ between Indico versions), then paste it into Aurora. The scope
  `read:user` is always required: Aurora uses it to check the token and name the connection.
- **Sign in with Indico** (OAuth). Aurora asks for `read:user` and `read:legacy_api` by default.
  Anything more is a separate, clearly marked choice.

Indico issues tokens that **do not expire and cannot be refreshed**. If a token is revoked in
Indico (or Indico discards an old one), requests fail with an upstream 401 until you reconnect.

## 4. Scopes: what they allow

| Indico scope | Lets agents | Tier |
|---|---|---|
| `read:user` | Check who is connected | official |
| `read:legacy_api` | Search and read events, timetables, rooms and bookings | official |
| `write:legacy_api` | Book a single room slot through the HTTP API: start and end on the same day, today or later; no pre-booking, no recurrence | official |
| `read:everything` | Read anything you can see in Indico through the extended read tools | extended |
| `full:everything` | **Opt-in, off by default, listed last.** Extended write: recurring bookings, cancellations, favorites, event preparation — Indico's internal endpoints, which may change between versions (extended write tools) | extended |

### Common tasks

| You want to | Tick |
|---|---|
| Find and book a free room ("a room in building 513 for 5 people at 10am tomorrow") | `read:everything` + `write:legacy_api` (add `full:everything` for pre-bookings, which rooms that need confirmation require, and for recurring bookings) |
| Read events and timetables | `read:legacy_api` |

Finding a free room needs `read:everything`: Indico's official API cannot search by capacity, building
or free time. Booking through `write:legacy_api` needs your Indico **login** (the agent asks for it; it
is not your email address) and cannot book a room that needs confirmation.

### Which tools need which scope, and what you will see

Every tool names the scopes it needs, and Aurora compares them with the scopes your connection has:
the scopes Indico granted (OAuth), or, for a **personal API token**, the scopes you say it has ("Which
scopes did you give this token?" when connecting: Aurora checks the read ones with one harmless read
request each, never the write ones; you can change the list later on the connection, "as you declared").

- A tool whose scopes the connection lacks is **not offered**: the grant forms list it collapsed under
  "Not available with this connection's token", with the scope it needs, and the agent is refused with
  `tool_scope_missing` (nothing is sent to Indico) if it tries.
- If the connection's scopes are unknown (a token with none declared), nothing is hidden or refused; Indico
  answers 403 as before. Declared scopes only ever *remove* tools: they never widen what a token can do.

| Tool group | Needs one of |
|---|---|
| Who is connected | `read:user` |
| Official API, read: events, timetables, rooms, bookings (names end *(official API)* or have no suffix) | `read:legacy_api` |
| Official API, write: *Book a room: one slot (official API)* | `write:legacy_api` |
| Indico web API, read: search, favorites, room search by free time, my bookings, user search (*(Indico web API)*) | `read:everything` or `full:everything` |
| Indico web API, write: *Book a room: recurring, with all options*, cancellations, favorites, event preparation | `full:everything` |

*Official API* is Indico's documented HTTP API; *Indico web API* are the internal endpoints Indico's own
pages use, which may change between Indico versions. The near-identical names tell them apart: *Book a
room: one slot* (official) against *Book a room: recurring, with all options* (web API); *Find a room by
name* / *Read rooms and their bookings* (official) against *List all rooms with details* / *Search rooms
free in a time slot* (web API).

`read:everything` and `full:everything` do **not** include the two `legacy_api` scopes: tick both
if you want both tiers. The connect form pre-ticks only `read:user` and `read:legacy_api`; Indico has
no narrower write scope for its extended endpoints, so `full:everything` stays an explicit choice
(D-196) and the user has to tick it.

> **Warning.** `full:everything` lets the token do **everything you can do in Indico**, on any
> endpoint. If you are an Indico administrator, that includes administration. Aurora narrows what
> agents may do with it in two ways: the token is only added to the eight path prefixes the tools
> use (never to `/user/tokens/`, `/oauth/`, `/admin/` or event management pages), and every
> change is a separate, approval-held tool. In addition, this package is **custom access only**:
> Aurora never offers or accepts a *full* grant for an Indico connection, and a request that is
> not one of the tools listed below is blocked, whatever the grant or rules say. Agents get
> Indico through a **custom** grant (reads allowed, changes asked). Do not connect with an
> administrator account for agent use.

## What changed in 1.1.0

- Indico is now **custom access only** (`grants.custom_only`, `grants.block_untooled`). The first
  time an organization activates this version, every existing *full* grant of an Indico connection
  is converted, in the same step, to a **custom** grant with exactly the tools the package has
  today: agents keep what they could do with catalogued tools, and lose only what a full grant
  added (requests matching no tool, and tools added later, which now arrive blocked). The upgrade is
  held for an administrator's approval, showing how many grants will be converted. Nothing is
  converted back if a later version drops the restriction.

## 5. What agents can do

**Allowed by default in a custom grant (reads)**

- Official: the connected user; events in categories (with date, type, location and room
  filters); events in your favorite categories; event title search; an event with contributions,
  sessions, attachments and notes; event files; timetables; rooms and their bookings; rooms by
  name; room bookings at a location.
- Extended: instance-wide search (events, contributions, categories, notes, files); category
  search; favorite users, categories, events and rooms; room locations and equipment; all rooms;
  room search including **rooms free in a time slot**; a room's details and availability; your
  next bookings, the booking calendar and one booking; and **user search**.

User search (`get_user_search_token` and `search_users`) and the event and booking exports return
personal data such as names, email addresses, affiliations, speakers and booked-for names. Agents
only see what the connected Indico user may see. Block these tools in a grant if agents should not
read such data.

**Held for approval (changes)**

| Tool | Approval shows |
|---|---|
| `book_room` (official, same day) | room, times, time zone, booked-for login, reason |
| `create_booking` (extended, recurrence and pre-booking) | room, start, end, repeat settings, booked-for user, reason, pre-booking, admin override, link |
| `cancel_booking`, `cancel_booking_occurrence` | booking id (and date) |
| `add_favorite_*`, `remove_favorite_*` for users, categories, events and rooms | the id |
| `prepare_event` | title, start, time zone, duration, type |

**Refuse a booking whose summary lacks the room or the times.** The summary is built from the
request body; a body in an unexpected encoding gives a shorter summary. "Admin override" lets an
Indico administrator bypass booking rules; approve it only knowingly.

## 6. Limits

- **Indico has no API to create events.** Agents can *prepare* an event (title, start, duration,
  type): they get a link that opens Indico's creation dialog prefilled, and you finish creating
  it in Indico.
- Bookings through the HTTP API (`book_room`) start and end on the same day (today or later) and are
  direct only (no pre-booking, no recurrence). Use `create_booking` (extended) for those.
- The extended endpoints may change with Indico upgrades. Agents are told to report a 404 or an
  unexpected response instead of guessing.
- Legacy API keys (`ak` and signatures) are not supported; use tokens (Indico 3.0 and later).
- Bookings are not idempotent: agents are told never to retry a booking automatically. If agents
  book often, add an Aurora rate-limit rule on `book_room` and `create_booking`.
- Indico limits how many records an export returns per request; a larger `limit` is an error.
- **Disconnecting**: Aurora does not revoke the token in Indico. Revoke it yourself at
  `/user/applications/` (OAuth) or `/user/tokens/` (personal tokens) when you disconnect.
