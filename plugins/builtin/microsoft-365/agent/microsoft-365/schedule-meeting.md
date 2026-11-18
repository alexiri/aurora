# Find a free slot and schedule a meeting

Use this for "find an hour next week and invite Ana and Ben". **Limit of this connection:** only the
signed-in user's own calendar can be read (`/me/...`). Free/busy of other people (`findMeetingTimes`,
`getSchedule`) is not in the catalogue and is refused. So you find a time that is free for the
**person you act for**, propose it, and the invitees accept or decline. Say this to the person.

`$G` is `https://graph.microsoft.com/v1.0`. Send no token, no `-k`.

## 1. Read the person's calendar for the window

```sh
curl -s "$G/me/calendarView?startDateTime=2026-11-23T08:00:00Z&endDateTime=2026-11-27T18:00:00Z&\$select=subject,start,end,showAs,isAllDay&\$orderby=start/dateTime&\$top=100"
```

`startDateTime` and `endDateTime` are required for `calendarView`, in UTC or with an offset. Events with
`showAs` `free` do not block. The times come in UTC unless you send the header
`Prefer: outlook.timezone="Europe/Zurich"`; send it so the answer and your proposal use the person's
time zone. Read `@odata.nextLink` if there are more than 100 events (follow it as is).

## 2. Pick candidate slots

Work only within the person's working hours (ask when not known), skip all-day events that block and
events with `showAs` `busy`, `oof` or `tentative`, and keep a gap between meetings if the person likes one.
Offer at most three slots with date, start, end and time zone. Wait for the person's choice.

## 3. Create the event

```sh
curl -s -X POST "$G/me/events" -H 'content-type: application/json' -d '{
  "subject": "Project sync",
  "start": {"dateTime": "2026-11-24T14:00:00", "timeZone": "Europe/Zurich"},
  "end":   {"dateTime": "2026-11-24T14:30:00", "timeZone": "Europe/Zurich"},
  "attendees": [{"emailAddress": {"address": "ana@example.org", "name": "Ana"}, "type": "required"}]
}'
```

This writes to the calendar and sends invitations: it needs the person's approval in Aurora and you must
confirm subject, time, time zone and the invitees' addresses with the person first. Never guess an
address; ask. Create the event once: if the answer is an error or a timeout, read the calendar (step 1)
before trying again so you do not invite people twice.

## 4. Verify

`GET $G/me/events/<id>?$select=subject,start,end,attendees` shows what was created. Tell the person the
time, the attendees and that responses will arrive by mail.
