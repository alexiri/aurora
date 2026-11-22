---
name: calendar-api-notes
description: "How to list and change Google Calendar events and check free/busy; read before the first request to Calendar"
---

# Google Calendar API v3 notes

Base URL `https://www.googleapis.com/calendar/v3` (also in `$GOOGLE_CALENDAR_API_URL`). Aurora adds the
credentials: send no token, no `-k`. The calendar id `primary` is the person's main calendar; other
calendars are listed by `GET /calendar/v3/users/me/calendarList` (`id`, `summary`, `accessRole`).

## List events

```sh
curl -s "https://www.googleapis.com/calendar/v3/calendars/primary/events?timeMin=2026-11-18T00:00:00Z&timeMax=2026-11-25T00:00:00Z&singleEvents=true&orderBy=startTime"
```

- `timeMin` and `timeMax` are RFC 3339 with an offset (`Z` or `+01:00`).
- `singleEvents=true` expands recurring events into their occurrences; `orderBy=startTime` needs it.
- Search text with `q=`. Page with `pageToken=<nextPageToken>`; `maxResults` defaults to 250.
- An event has `start` and `end`, either `dateTime` (with `timeZone`) or, for all-day events, `date`.

## Free and busy

`POST /calendar/v3/freeBusy` reads only, though it is a POST, and is held for approval:

```sh
curl -s -X POST https://www.googleapis.com/calendar/v3/freeBusy -H 'content-type: application/json' -d '{
  "timeMin": "2026-11-19T08:00:00Z", "timeMax": "2026-11-19T17:00:00Z",
  "items": [{"id": "primary"}, {"id": "colleague@example.org"}]
}'
```

The answer lists the `busy` intervals per calendar; everything else is free. A calendar the person cannot
read comes back with `errors`.

## Create, change, delete

All three are held for approval. Send JSON with `content-type: application/json`.

```sh
curl -s -X POST "https://www.googleapis.com/calendar/v3/calendars/primary/events" -H 'content-type: application/json' -d '{
  "summary": "Planning",
  "start": {"dateTime": "2026-11-19T10:00:00+01:00"},
  "end": {"dateTime": "2026-11-19T10:30:00+01:00"},
  "attendees": [{"email": "colleague@example.org"}]
}'
```

- Attendees are invited by email only when the request has `sendUpdates=all` (or `externalOnly`); without it
  none is told. Ask the person which they want.
- Change an event with `PATCH /calendar/v3/calendars/primary/events/<id>` and only the fields that change.
- `DELETE .../events/<id>` cancels the event and cannot be undone; confirm first.

## Errors

`404` is an unknown id or a calendar the person cannot see; `403` is a missing scope or an event the person
may not change; `410` is an event that is already deleted.
