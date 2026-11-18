# Microsoft Graph conventions

Base URL `https://graph.microsoft.com/v1.0` (also in `$MICROSOFT_GRAPH_URL`). Requests go through the
proxy with no token and no `-k`. Only the signed-in user's own resources (`/me/...`) and SharePoint
sites and drives by id are available; `tools.md` lists the operations and your access.

## Entry points the catalogue covers

| Area | Requests |
|---|---|
| Profile | `GET /me` |
| Mail | `GET /me/mailFolders`, `/me/mailFolders/<id>/messages`, `/me/messages`, `/me/messages/<id>`, `/me/messages/<id>/attachments`; send: `POST /me/sendMail` |
| Calendar | `GET /me/calendars`, `/me/events`, `/me/events/<id>`, `/me/calendarView?startDateTime=&endDateTime=`; create: `POST /me/events` |
| OneDrive | `GET /me/drive`, `/me/drive/root/children`, `/me/drive/recent`, `/me/drive/items/<id>`, `.../children`, `.../content` |
| SharePoint | `GET /sites`, `/sites/<id>`, `/sites/<id>/drives`, `/drives/<id>/root/children`, `/drives/<id>/items/<id>` |

OneDrive files are addressed by **item id** (`/me/drive/items/{id}`); paths such as
`/me/drive/root:/folder/file` are not available. Downloads (`.../content`) redirect to a
pre-authenticated storage URL outside this connection: follow the redirect once, do not forward headers.

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
