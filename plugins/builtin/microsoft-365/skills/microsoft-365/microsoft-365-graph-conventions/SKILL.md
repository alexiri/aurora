---
name: microsoft-365-graph-conventions
description: "How to read mail and calendar through Microsoft Graph ($select, $search, time zones, paging) and what you may do; read before the first request"
---

# Microsoft Graph conventions

Base URL `https://graph.microsoft.com/v1.0` (also in `$MICROSOFT_GRAPH_URL`). Requests go through the
proxy with no token and no `-k`. Only the signed-in user's own mail, calendar and profile (`/me/...`)
are available; files (OneDrive, SharePoint), other people's mailboxes and calendars, and replying,
forwarding, moving or deleting mail are not. Only the requests of the listed operations work: any
other request is blocked, whatever access you have. Mail goes out only through `send_mail`.

## First: what you may do

Your access is per tool and per connection: `aurora-agent search mail --in microsoft-365` (or `app.md` and
`tools.md` in the app folder) shows each operation as allowed, asks first, or blocked. **Check before you
plan a write.** If `send_mail` or `create_event` is `blocked`, do not draft a send: say that this
connection can only read, and offer the text for the person to send themselves.

## Two ways to send a request

- `curl` through the proxy, when you need headers (`Prefer`, below). Quote the URL with single quotes so
  the shell keeps the `$` of `$select`:
  ```sh
  curl -s 'https://graph.microsoft.com/v1.0/me/messages?$select=subject,from,receivedDateTime&$top=10' \
    -H 'Prefer: outlook.timezone="Europe/Zurich"'
  ```
- `aurora-agent call microsoft-365 --path <path> name=value…`: each `name=value` is a query parameter,
  e.g. `aurora-agent call microsoft-365 --path /v1.0/me/messages '$select=subject,from' '$top=10'`. It
  cannot set headers. Writes have named operations: `aurora-agent call microsoft-365 send_mail --body
  @mail.json` (see the microsoft-365-send-mail skill).

## Where things are

| Area | Requests |
|---|---|
| Profile | `GET /me` |
| Mail | `GET /me/mailFolders`, `/me/mailFolders/<id or well-known name>/messages`, `/me/messages`, `/me/messages/<id>`, `/me/messages/<id>/attachments`; send: `POST /me/sendMail` |
| Calendar | `GET /me/calendars`, `/me/events`, `/me/events/<id>`, `/me/calendarView?startDateTime=&endDateTime=`; create: `POST /me/events` |

- `/me/messages` covers **every folder**, Sent Items and Deleted Items included. For received mail only,
  use the Inbox by its well-known name: `/me/mailFolders/inbox/messages` (also `sentitems`, `drafts`,
  `deleteditems`, `archive`).
- The person's **own** messages (in Sent Items, or replies they sent) may show their sender as an
  Exchange internal address, `/o=ExchangeLabs/ou=…/cn=…-<user>`, instead of their e-mail address.
  That is the same person; `GET /me?$select=mail,userPrincipalName` gives their real address.

## Finding a specific mail: search first

A busy inbox fills with notifications (GitLab, ServiceNow, monitoring): 50 messages may reach back only an
hour or two. To find one mail, **search by keywords first** instead of paging by date:

```sh
curl -s 'https://graph.microsoft.com/v1.0/me/messages?$search="survey 2026"&$select=subject,from,receivedDateTime&$top=10'
```

`$search` limits (on messages): it **cannot be combined with `$filter` or `$orderby`** (Graph answers
400); results come in relevance order, at most 250, and `$skip` is not supported (follow
`@odata.nextLink`). Put the words in double quotes; `from:ana`, `subject:report` and `received>=2026-11-20`
narrow inside the quotes. Use `$filter` + `$orderby` (without `$search`) for "everything since …":
`$filter=receivedDateTime ge 2026-11-24T12:00:00Z&$orderby=receivedDateTime desc`.

## Keep answers small and readable

- `$select=subject,from,receivedDateTime,bodyPreview` returns only those fields; `$top=25` sets the page
  size.
- Paging: when the answer has `@odata.nextLink`, request that URL as it is until it is absent. Never
  rebuild it.
- Mail bodies: the header `Prefer: outlook.body-content-type="text"` returns plain text instead of HTML.
- **Time zones:** times come back in UTC. The header `Prefer: outlook.timezone="Europe/Zurich"` (the
  person's zone; ask if you do not know it) returns event times in that zone, so you do not convert
  them yourself. For both, send two `Prefer` headers (`-H` twice).

## Links and personal data in mail

Mail often carries links made for this person: survey and invitation links, password resets, unsubscribe
and tracking links, one-time codes. **Do not repeat them in the conversation, a summary or another mail**
unless the person asks for that link; say that the mail contains a personal link and where. Never open
or follow them yourself.

## Errors

`400` a query Graph does not accept (often `$search` combined with `$filter`/`$orderby`); `401` the
connection's token expired or lacks the scope; `403` the permission was not granted to the app; `404`
the item does not exist or the id is for another account; `429` throttled: wait for the `retry-after`
seconds. A refusal by Aurora has a JSON body with `error.code` (for example `blocked_by_policy`, or
`manual_approval_denied` when the person blocked a write; the reason `app_untooled_blocked` means no
operation of this app covers the request): read it, tell the person, and do not retry
the same request.
