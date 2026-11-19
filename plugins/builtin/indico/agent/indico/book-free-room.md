# Find a free room and book it

Use this when the person asks for a meeting room at a time ("a room for 6 people tomorrow at 14:00").
`$H` below is the Indico address shown with the account in `guide.md`; send every request to it
through the proxy. Aurora adds the token: send no credential, no `-k`.

Needs the scope `read:everything` to search (internal endpoints) and `write:legacy_api` to book. If a
scope is missing the request is refused: say which one is needed and stop; never guess availability.

## 1. Gather what you need

Start and end time, number of people, and the building if the person named one. Times are the
server's local time. With no end time assume one hour and say so. Ask for the building when it is
not known; the search is one call per building.

## 2. Search per building

```sh
curl -s "https://$H/rooms/api/rooms/search?building=40&capacity=6&start_dt=2026-11-20T14:00:00&end_dt=2026-11-20T15:00:00&repeat_frequency=NEVER"
```

With the `aurora-agent` command the same search is one call that saves the answer and prints a preview
(`aurora-agent describe indico search_rooms` lists the parameters):

```sh
aurora-agent call indico search_rooms building=40 capacity=6 start_dt=2026-11-20T14:00:00 end_dt=2026-11-20T15:00:00 repeat_frequency=NEVER
```

- Always send `repeat_frequency=NEVER`. Without it busy rooms come back as well.
- The answer lists room ids, nothing else. If it is empty, no room of that building is free: say so
  and offer another building or time.
- `GET /rooms/api/locations` lists the places rooms can be booked, `GET /rooms/api/rooms/` lists all
  rooms with their details when you need to find a building.

## 3. Name the candidate rooms

```sh
curl -s "https://$H/rooms/api/rooms/123"
```

(`aurora-agent call indico get_room_details id=123`.) Report the name, building, floor and capacity of up to five rooms. The official `bookRoom` call
cannot book a room that needs confirmation: if it refuses a room for that reason, say so and offer
another room.

## 4. Confirm with the person

Show room, day, start and end, and the reason, and ask for a yes. Booking needs the person's
**Indico login** (`username`), not the e-mail address; ask if you do not know it. A booking request
is held for the person's approval in Aurora as well.

## 5. Book (once)

```sh
curl -s -X POST "https://$H/api/roomBooking/bookRoom.json" \
  -d roomid=123 -d from=2026-11-20T14:00 -d to=2026-11-20T15:00 \
  -d tz=Europe/Zurich -d reason="Project sync" -d username=jdoe
```

- `from` and `to` are on the same day, `YYYY-MM-DDTHH:MM`; `roomid` is the numeric id from step 2.
  Use long names for the parameters, not the short `f`, `t`.
- The same with the command: `aurora-agent call indico book_room roomid=123 from=2026-11-20T14:00 to=2026-11-20T15:00 tz=Europe/Zurich reason="Project sync" username=jdoe`.
- With `full:everything` recurring bookings are possible: `POST /rooms/api/booking/create` with JSON
  `{"room_id":123,"start_dt":"2026-11-20T14:00:00","end_dt":"2026-11-20T15:00:00","repeat_frequency":"NEVER","reason":"Project sync","is_prebooking":false}`.
  Leave `user` out: it books for the connected person.
- **Never retry a booking** after an error or a timeout: check first (step 6), then ask the person.

## 6. Verify

`GET /rooms/api/bookings/<id>` with the id from the answer shows the booking (internal endpoint, id
from the `create` answer); the person's next bookings are at `GET /rooms/api/bookings/mine`. The
`bookRoom` answer of the official API does not always carry an id: then check `mine`. Tell the person the room, the time and the link `https://$H/rooms/`.

The `/rooms/api/` endpoints are internal to Indico and may change between versions: on a 404 or an
unexpected shape say so instead of improvising.
