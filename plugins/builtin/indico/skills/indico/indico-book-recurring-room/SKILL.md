---
name: indico-book-recurring-room
description: "Book a weekly or recurring Indico room series: check every date at once, a per-date availability matrix, a plan with exceptions, create the series, verify the occurrences."
---

# Book a recurring room series in Indico

Use for "book a room every Tuesday 14:30-15:30 until March for 10 people in building 31 or 513".
Builds on `indico-find-and-book-room` (single-slot search, the room-list cache, confirm and verify
rules: read it first). `$H` is the Indico address from `app.md`. Needs `read:everything` to search
and `full:everything` to create the series (internal endpoints, checked on Indico 3.3 on
2026-11-21; they may change between versions). Times are the server's local time. Several accounts:
`--account <letter>` on the calls, `INDICO_ACCOUNT=<letter>` for the scripts.

## 1. Parse
First date (the next matching weekday, say it), last date (or "until X": the last matching weekday
on or before X; no end given: ask), start and end time, interval (weekly = 1, every other week = 2),
capacity, buildings, reason. Monthly or daily series: same approach with another date step.

## 2. Quick check: rooms free on every date (one call)

```sh
aurora-agent call indico search_rooms capacity=10 start_dt=2026-12-01T14:30:00 end_dt=2027-03-30T15:30:00 repeat_frequency=WEEK repeat_interval=1 --raw
```

`start_dt` is the first date with the start time, `end_dt` the **last** date with the end time.
`availability_days` lists every occurrence checked and `rooms` the ids free on all of them. With
`repeat_frequency=NEVER` a multi-day range only checks the first day: do not use it for a series.
Add `building=<n>` to limit it to one building; leave it out to search all of them and filter the
room list.

## 3. Partial availability (the usual case): a per-date matrix

```sh
"$SKILLS_DIR/indico-book-recurring-room/scripts/series-free" 2026-12-01 2027-03-30 14:30 15:30 10 1 31 513 600
```

Arguments: `FIRST LAST START END CAPACITY INTERVAL_WEEKS [BUILDING ...]`. One search per date (no
building filter), joined with the cached room list. It prints the dates checked, then per room (best
15): id, name, capacity, `direct` or `NEEDS-CONFIRMATION`, `free N/M` and the busy dates, the rooms
with most free dates first. Weekly steps only, at most 60 dates per run (split a longer series). For
a daily or monthly series run `"$SKILLS_DIR/indico-find-and-book-room/scripts/free-rooms"` per date.

Not used: `get_room_availability` (ignores dates), the booking timeline endpoint (not reachable
through the gateway), the calendar over months (about 2 MB per month: only for "who has it on
date X"). Apply the room rules of the single-booking skill (skip rooms whose comments say they are
not meeting rooms, prefer `direct`, smallest that fits). Indico does not block the days a site is
closed: ask whether to skip those dates instead of booking them.

## 4. Propose a plan
- A room free on every date: propose it alone.
- Otherwise: the main room is the one with most free dates; for each busy date propose the best
  room free on that date (re-run `free-rooms` for it) or skipping it. Example: "R-068 for 15 of 18
  Tuesdays; 8 Dec and 15 Dec in S-028; 2 Mar skipped?"
- Confirm explicitly: room(s), weekday, time, first and last date, interval, reason, exceptions.

## 5. Book (held for approval in Aurora; never retry)
A series goes through the internal endpoint (`book_room` books one slot only):

```sh
echo '{"room_id":137,"start_dt":"2026-12-01T14:30:00","end_dt":"2027-03-30T15:30:00","repeat_frequency":"WEEK","repeat_interval":1,"recurrence_weekdays":["tue"],"reason":"Team meeting","is_prebooking":false}' \
  | aurora-agent call indico create_booking --body -
```

- `start_dt` is the first date with the start time and `end_dt` the last date with the end time,
  as in the search. Leave `user` out: it books for the connected person.
- Do not include dates where the room is busy hoping Indico skips them: it is untested whether it
  rejects the whole series or skips the conflicts. Book contiguous runs of free dates as separate
  series and the exception dates as single bookings (`book_room`, or `create_booking` with
  `repeat_frequency=NEVER`).
- A room that needs confirmation takes `"is_prebooking":true`: tell the person it is pending the
  room owner's approval.
- A scope refusal: say `full:everything` is needed and stop.

## 6. Verify
`aurora-agent call indico get_booking id=<booking id> --raw` (the id is in the create answer, else
find it in `list_my_upcoming_bookings` by room id and start). Check:
- `repetition` is `[2, <interval>, ["tue"]]` (2 means WEEK) and `start_dt`/`end_dt` are as requested;
- `occurrences.bookings` has the booked dates (count them against the plan), `cancellations` and
  `rejections` are empty, `is_accepted` is true (`is_pending` for a pre-booking).

Report the room, weekday and time, the date range, the number of occurrences, the exceptions with
their rooms, and `https://$H/rooms/my-bookings`.

## Cancelling one date later
`aurora-agent call indico cancel_booking_occurrence id=<booking id> date=2026-12-08` (held for
approval). The whole series: `cancel_booking id=<booking id>`. Confirm with the person first.
