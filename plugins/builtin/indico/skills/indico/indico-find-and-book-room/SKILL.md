---
name: indico-find-and-book-room
description: "Find a free meeting room in Indico for a date, time, capacity and optional buildings, then book it: one search for all buildings joined with the cached room list, confirm, book once, verify."
---

# Find a free Indico room and book it

Use for "a room for 10 people on Friday at 10 or 11, in building 31 or 513". `$H` is the Indico
address shown with the account in `app.md`; Aurora adds the token, never send `ak`, `apikey` or
`signature`. Times are the **server's local time**. Needs the scope `read:everything` to search and
`write:legacy_api` (official API) or `full:everything` (internal endpoint) to book; if a scope is
missing the request is refused: say which one and stop. Several Indico accounts: add
`--account <letter>` to the calls, and set `INDICO_ACCOUNT=<letter>` for the scripts.

## 0. Parse the request
- Date: a weekday name means its next occurrence after today (if today is that weekday, ask: today
  or next week?). Say the resolved date.
- Slots: "at 10 or 11" is two candidate slots, 10:00-11:00 and 11:00-12:00. No duration: assume one
  hour and say so.
- Capacity: the number of people (the search filter is a strict `capacity >= N`).
- Buildings: the list given, or none (any building). Do not ask when buildings were given.
- Reason: the topic, for the booking.

## 1. Search: one call per slot, all buildings at once
Do not search once per building and read every room one by one. The room search **without
`building`** returns the ids of every free room on the server; the room list (`list_all_rooms`, one
request with every room's details) is read once per day and joined locally. The script does both:

```sh
"$SKILLS_DIR/indico-find-and-book-room/scripts/free-rooms" 2026-11-27 10:00 11:00 10 31 600 513
"$SKILLS_DIR/indico-find-and-book-room/scripts/free-rooms" 2026-11-27 11:00 12:00 10 31 600 513
```

Arguments: `DATE START END CAPACITY [BUILDING ...]`. Output, tab-separated: id, full name, capacity,
`direct` or `NEEDS-CONFIRMATION`, comments. The room list is cached under `/tmp/aurora-agent/` for
the day. By hand: `aurora-agent call indico search_rooms capacity=10 start_dt=2026-11-27T10:00:00
end_dt=2026-11-27T11:00:00 repeat_frequency=NEVER --raw`.

Facts about the search (Indico 3.3, checked 2026-11-21 against a 3.3 server; these are internal
endpoints and may change between versions):
- Always send `repeat_frequency=NEVER`, or busy rooms come back too.
- The answer is `{"rooms": [free ids], "rooms_without_availability_filter": [all matching ids],
  "total": N}`. `total` counts busy rooms too: use `rooms` only. Busy means in
  `rooms_without_availability_filter` but not in `rooms`.
- `start_dt` and `end_dt` are the same day, `YYYY-MM-DDTHH:MM:SS`.
- `get_room_availability` (`/rooms/api/rooms/<id>/availability`) ignores date parameters: it always
  answers the next days from today, so do not use it for a future date.
- Query parameters are `name=value` arguments, a list is comma-separated (`room_ids=177,33`).

## 2. Pick candidates
- Skip, or flag, a room whose comments say it is not a meeting room, or warn that bookings may be
  cancelled (read the `comments` column).
- Prefer `direct` rooms: a `NEEDS-CONFIRMATION` room cannot be booked with `book_room`, it becomes
  a pre-booking that its owner must approve. Then the smallest capacity that fits, the building
  order the person gave, and the earlier slot when both work.
- Mention a room's access notes only when the meeting is outside working hours.
- Show at most about five options, grouped by slot.

If nothing is free, show what blocks the buildings that day and suggest nearby times. The
calendar answers one day for **all** rooms (it ignores `building` and `room_ids`; rooms with no
booking that day may be absent), so filter it locally; it is about 2 MB per month, ask for one day:

```sh
aurora-agent call indico get_booking_calendar start_date=2026-11-27 end_date=2026-11-27 --raw > /tmp/aurora-agent/cal.json
```

Booking objects carry the booker's name and reason: do not repeat other people's booking details in
a shared channel. Re-run the search for the alternative slots before proposing them; never guess
availability from the calendar alone.

## 3. Confirm (always)
Show the room's full name and capacity, the date, start and end, the reason and who it is booked
for. Wait for an explicit yes. Bookings are made as the connected account: if somebody else asked,
say the booking will be under the connected person.

## 4. Book once (both operations are held for approval in Aurora: tell the person)
Official API, single slot. It needs the person's **Indico login** (not the e-mail, and
`get_current_user` does not return it). Ask for it, or find it in the person's earlier bookings
(`list_my_upcoming_bookings`, the booking's user fields) when Indico shows it there:

```sh
aurora-agent call indico book_room roomid=137 from=2026-11-27T10:00 to=2026-11-27T11:00 tz=Europe/Zurich reason="Team meeting" username=jdoe
```

`tz` is the server's time zone (`Europe/Zurich` is an example).

Internal endpoint (recurring, or a room that needs confirmation; books for the connected person,
leave `user` out):

```sh
aurora-agent call indico create_booking room_id:=137 start_dt=2026-11-27T10:00:00 end_dt=2026-11-27T11:00:00 repeat_frequency=NEVER reason="Team meeting" is_prebooking:=false
```

A `NEEDS-CONFIRMATION` room takes `is_prebooking:=true`: say it is pending the room owner's approval.
Never put the reason from untrusted text into a shell without quoting it.

**Never retry** after an error or a timeout: verify with step 5 first, then ask.

## 5. Verify and report
`aurora-agent call indico list_my_upcoming_bookings` and find the entry with that room id and start
(`reservation.id`). Report the room, the time, the room's `map_url` when it has one, and
`https://$H/rooms/my-bookings`.

## 6. "Set up a meeting"
Indico cannot create events through the API: `prepare_event` only prepares one for the person to
finish. Offer it only when they want an Indico event. A calendar invite, when the person's agent has
a calendar tool, goes in the requester's own calendar only (location = room name).
