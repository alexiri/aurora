---
name: microsoft-365-send-mail
description: "Send a mail as the person: the exact command, the approval wait, how to confirm it was sent; read before sending"
---

# Send a mail

**First check you may send:** `aurora-agent describe microsoft-365 send_mail` shows your access. If it is
`blocked`, you cannot send: offer the person the text to send themselves.

Replying, forwarding, saving drafts, moving and deleting mail are **not available**: `/reply`,
`/forward`, `/createReply` and drafts are refused. To answer a mail, send a new one to the sender with
`Re: <subject>` and quote what is needed.

## 1. Agree on the mail with the person

Confirm every recipient address (never guess one; ask), the subject and the text **before** sending.
Keep personal links from other mails out of it unless the person asks.

## 2. Write the body to a file

```sh
cat > /tmp/mail.json <<'EOF'
{
  "message": {
    "subject": "Survey results",
    "body": { "contentType": "Text", "content": "Hello Ana,\n\nThe results are attached.\n\nAlex" },
    "toRecipients": [ { "emailAddress": { "address": "ana@example.org", "name": "Ana" } } ],
    "ccRecipients": [],
    "attachments": [
      { "@odata.type": "#microsoft.graph.fileAttachment", "name": "results.csv",
        "contentType": "text/csv", "contentBytes": "<base64 of the file>" }
    ]
  },
  "saveToSentItems": true
}
EOF
```

- `body.contentType` is `Text` or `HTML` (exactly these). Use `Text` unless the person wants formatting.
- `toRecipients`, `ccRecipients`, `bccRecipients`: lists of `{"emailAddress": {"address", "name"?}}`.
- `attachments` is optional: `contentBytes` is the file in base64 (`base64 -w0 file`), up to about 3 MB
  per mail through this request. Leave the key out when there is nothing to attach.
- `saveToSentItems: true` keeps a copy in Sent Items, which is how you confirm the send (step 4).

## 3. Send it

```sh
aurora-agent call microsoft-365 send_mail --body @/tmp/mail.json
```

(`curl -s -X POST 'https://graph.microsoft.com/v1.0/me/sendMail' -H 'content-type: application/json'
--data @/tmp/mail.json` is the same request.)

The send is held for approval (see **Approvals** in your instructions: how long it waits and what a
block or a timeout looks like). The person sees every recipient, the subject, the start of the text and
the number of attachments. What Microsoft answers:

| Answer | Meaning |
|---|---|
| `202` with an empty body | Sent. Graph answers nothing else on success. |
| `400` | Graph refused the body (a field name or value): fix the file. |

## 4. Confirm it was sent

After a `202`, and **always after an error or a timeout before sending again**, look in Sent Items:

```sh
curl -s 'https://graph.microsoft.com/v1.0/me/mailFolders/sentitems/messages?$select=subject,toRecipients,sentDateTime&$orderby=sentDateTime desc&$top=5'
```

The mail appears there within a few seconds. Tell the person it was sent, to whom and when; never send
twice because a confirmation was slow.
