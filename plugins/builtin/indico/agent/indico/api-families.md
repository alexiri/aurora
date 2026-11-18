# Indico API families and the booking form

Two tiers. `$H` is the Indico address shown with the account in `guide.md`; Aurora adds the token
(never send `ak`, `apikey`, `signature`, `timestamp` or `cookieauth`).

## Official tier (documented HTTP API, scopes `read:legacy_api` / `write:legacy_api`)

| Purpose | Request |
|---|---|
| Who am I (`read:user`) | `GET /api/user/` |
| Events of categories | `GET /export/categ/<id>[-<id>].json?from=&to=&type=&location=&room=&detail=` |
| Favorite categories' events | `GET /export/categ/favorites.json` |
| One event | `GET /export/event/<id>.json?detail=contributions` or `sessions` |
| Search events by title | `GET /export/event/search/<term>.json` |
| Timetable | `GET /export/timetable/<event id>.json` |
| Rooms and bookings | `GET /export/room/<location>/<room id>.json` |
| Room by name | `GET /export/roomName/<location>/<name>.json` |
| Bookings at a location | `GET /export/reservation/<location>.json` |
| Event files | `GET /export/event/<id>/material/<folder>/<file>.bin` |
| Book a room | `POST /api/roomBooking/bookRoom.json` (form, scope `write:legacy_api`) |

`bookRoom` form fields: `roomid`, `from`, `to` (same day, `YYYY-MM-DDTHH:MM`), `tz`, `reason`,
`username` (the person's Indico login, not the e-mail). It cannot book rooms that need confirmation.

## Extended tier (internal endpoints, scopes `read:everything` / `full:everything`)

Indico's own web endpoints, verified against Indico 3.3.14. They are not a documented API and may
change: on a 404 or an unexpected shape, say so.

| Purpose | Request |
|---|---|
| Free rooms | `GET /rooms/api/rooms/search?building=&capacity=&start_dt=&end_dt=&repeat_frequency=NEVER` |
| Room details | `GET /rooms/api/rooms/<id>` (the search returns ids only) |
| All rooms, locations, equipment | `GET /rooms/api/rooms/`, `/rooms/api/locations`, equipment types |
| Is one room free | `GET /rooms/api/rooms/<id>/availability/...` |
| Booking calendar, one booking, mine | `GET /rooms/api/calendar`, `/rooms/api/bookings/<id>`, `/rooms/api/bookings/mine` |
| Book with all options | `POST /rooms/api/booking/create` (JSON, `full:everything`) |
| Cancel | `POST /rooms/api/bookings/<id>/cancel` and the per-date cancel of a recurring booking |
| Search | `GET /search/api/search`, `GET /category/search?q=`, user search (returns personal data) |
| Favorites | `/user/api/favorites/...` for users, categories, events; rooms under `/rooms/api/` |
| Prepare an event | `GET /event/create/prepare`: Indico cannot create events by API |

`/rooms/api/booking/create` body: `{"room_id", "start_dt", "end_dt", "repeat_frequency": "NEVER",
"reason", "is_prebooking"}`; leave `user` out (it books for the connected person). Times are the
server's local time.

## Rules

- Confirm room, time and person before any booking; never retry a booking.
- Always send `repeat_frequency` to the room search, or busy rooms are returned as well.
- Never guess availability: if a scope is missing, say which one.
- Task procedures: `book-free-room.md`, `find-category-events.md`.
