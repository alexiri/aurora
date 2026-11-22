---
name: gmail-api-notes
description: "How to search, read and send Gmail messages, drafts and labels; read before the first request to Gmail"
---

# Gmail API notes

Base URL `https://gmail.googleapis.com/gmail/v1` (also in `$GMAIL_API_URL`). The user id is always `me`.
Aurora adds the credentials: send no token, no `-k`.

## Search and read

```sh
curl -s "https://gmail.googleapis.com/gmail/v1/users/me/messages?q=from:alice+newer_than:7d&maxResults=20"
curl -s "https://gmail.googleapis.com/gmail/v1/users/me/messages/<id>?format=metadata&metadataHeaders=Subject&metadataHeaders=From&metadataHeaders=Date"
```

- `q` uses the Gmail search syntax (`from:`, `to:`, `subject:`, `has:attachment`, `newer_than:`, `label:`,
  `is:unread`); URL-encode it.
- A list answer holds only `id` and `threadId`; read each message to see anything else. Use
  `format=metadata` for headers and `format=full` for the body. A body part is base64url in
  `payload.parts[].body.data` (decode it; long bodies and attachments have an `attachmentId` instead).
- Page with `pageToken=<nextPageToken>` until it is absent. `threads` lists and reads whole conversations
  the same way.
- `GET /gmail/v1/users/me/labels` lists labels (system ones such as `INBOX`, `UNREAD`, `STARRED` and the
  person's own, by `id`).

## Send and draft

`POST /gmail/v1/users/me/messages/send` with `content-type: application/json` and body
`{"raw": "<base64url of an RFC 5322 message>"}`. Build the message with `To`, `Subject` and a `Content-Type`
header, then encode it with URL-safe base64 (`-` and `_`, padding optional). Attachments go inside the
message as MIME parts. Approvals show To, Cc, Bcc, Subject, a preview and the attachment count, taken from
that message, so put every recipient in its headers.

- A draft is `POST /gmail/v1/users/me/drafts` with `{"message": {"raw": "..."}}`; send it with
  `POST /gmail/v1/users/me/drafts/send` and `{"id": "<draftId>"}`.
- Sending is held for approval and cannot be undone. Do not retry a send that returned an error without
  looking in the sent mail first (`q=in:sent newer_than:1h`).

## Labels

`POST /gmail/v1/users/me/messages/<id>/modify` with `{"addLabelIds": ["STARRED"], "removeLabelIds": ["UNREAD"]}`
marks a message read or starred. Deleting mail is not offered as a tool; a full grant can still send `DELETE`,
which cannot be undone: do not unless the person asked for exactly that.

## Errors

`401` means the connection must be renewed by the person; `403` with `insufficientPermissions` means the
grant has no scope for that call (reading needs a read scope, sending needs the send scope); `429` means
slow down and retry after a pause.
