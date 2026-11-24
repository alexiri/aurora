---
name: microsoft-365-graph-conventions
description: "How the Microsoft Graph API is addressed ($select, $filter, paging) and written to; read before the first request"
---

# Microsoft Graph conventions

Base URL `https://graph.microsoft.com/v1.0` (also in `$MICROSOFT_GRAPH_URL`). Requests go through the
proxy with no token and no `-k`. Only the signed-in user's own mail, calendar and profile (`/me/...`)
are available; files (OneDrive, SharePoint) are not. `tools.md` lists the operations and your access.

## Entry points the catalogue covers

| Area | Requests |
|---|---|
| Profile | `GET /me` |
| Mail | `GET /me/mailFolders`, `/me/mailFolders/<id>/messages`, `/me/messages`, `/me/messages/<id>`, `/me/messages/<id>/attachments`; send: `POST /me/sendMail` |
| Calendar | `GET /me/calendars`, `/me/events`, `/me/events/<id>`, `/me/calendarView?startDateTime=&endDateTime=`; create: `POST /me/events` |

## Keep responses small

- `$select=subject,from,receivedDateTime` returns only those fields; `$top=25` limits the page; `$filter`
  narrows (`$filter=isRead eq false`, `$filter=receivedDateTime ge 2026-11-01T00:00:00Z`);
  `$orderby=receivedDateTime desc`; `$search="word"` for text search (quote the value).
- In shell, escape the dollar signs (`\$select`) or quote the URL with single quotes.
- Paging: when the answer has `@odata.nextLink`, request that URL as it is until it is absent. Never
  rebuild it.
- Mail bodies: `Prefer: outlook.body-content-type="text"` returns plain text instead of HTML.

## Writing

Send JSON with `content-type: application/json` so approvals can show what a change does. `sendMail`
takes `{"message": {"subject", "body": {"contentType", "content"}, "toRecipients": [{"emailAddress":
{"address"}}]}, "saveToSentItems": true}`; confirm recipients, subject and text with the person first and
never send twice after an error without checking the sent items.

## Errors

`401` the connection's token expired or lacks the scope; `403` the permission was not granted; `404` the
item does not exist or the id is for another account; `429` throttled: wait for the `retry-after`
seconds. A refusal by Aurora has a JSON body with `error.code`: read it and tell the person.
