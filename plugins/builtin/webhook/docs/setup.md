# Let agents trigger a webhook (Outgoing webhook)

Many services give you a **webhook URL**: post a small message to it and something happens (a chat
message appears, a phone buzzes, an automation runs). With this app an agent can send such a message
without ever seeing the URL. You give the connection a name and the URL, and you describe the
message the receiver expects, leaving **blanks** (slots) for the parts the agent should fill in. The
Aurora gateway builds the message from your template and the agent's values, and sends it to exactly
the URL you entered.

## The URL is a password

Anyone who has the URL can post to the receiver, so:

- **Aurora stores the URL encrypted** and shows only its host on the connection. The agent never
  gets the URL, the template, the headers or any secret value: only the names and descriptions of
  the blanks.
- **Revoking is done at the receiver.** Delete or regenerate the webhook there, then connect again
  with the new URL (the connection's *Reconnect*). Until you do, the old URL keeps working. To
  pause an agent without deleting anything, set the connection to *Read only*: every send is then
  refused.
- **The agent controls only the blanks.** It cannot change the destination, the method, the headers
  or any fixed part of the message, and a value it writes is always inserted as plain data, never as
  part of the template. Sends ask you first unless you allow them for an agent.

## Where each receiver shows its URL

This is guidance only; Aurora has no code per receiver, and any `https://` URL works with any preset
or a template of your own. A preset is a ready-made template that you can edit.

- **Slack:** create an *Incoming Webhook* in a Slack app (*Features → Incoming Webhooks*), pick the
  channel and copy the `https://hooks.slack.com/services/…` URL. Format:
  <https://api.slack.com/messaging/webhooks>.
- **Mattermost:** *Integrations → Incoming Webhooks → Add Incoming Webhook*, and copy the URL. Format:
  <https://developers.mattermost.com/integrate/webhooks/incoming/>.
- **Google Chat:** in a space, *Apps & integrations → Webhooks*, and copy the URL. Format:
  <https://developers.google.com/workspace/chat/quickstart/webhooks>.
- **Discord:** *Channel settings → Integrations → Webhooks → New Webhook → Copy Webhook URL*. The
  message is at most 2,000 characters. Format:
  <https://discord.com/developers/docs/resources/webhook#execute-webhook>.
- **Microsoft Teams:** use a *Workflow* from the template "Post to a channel when a webhook request
  is received" and copy the URL it shows. The preset posts an Adaptive Card with a title and a text. Format:
  <https://learn.microsoft.com/en-us/microsoftteams/platform/webhooks-and-connectors/how-to/add-incoming-webhook>.
- **ntfy:** the URL is the server's address (for example `https://ntfy.sh/`); the **topic is a
  secret value** you enter in the second step, because anyone who knows a topic on a public server
  can read it. Format: <https://docs.ntfy.sh/publish/#publish-as-json>.
- **Home Assistant:** an automation with a *Webhook* trigger shows its URL
  (`https://<your Home Assistant>/api/webhook/<id>`); the agent's JSON is available as
  `trigger.json`. Format: <https://www.home-assistant.io/docs/automation/trigger/#webhook-trigger>.
- **IFTTT:** the *Webhooks* service shows `https://maker.ifttt.com/use/<key>`; the URL to enter is
  `https://maker.ifttt.com/trigger/<event>/json/with/key/<key>`. The preset sends `value1`,
  `value2` and `value3`. Format: <https://ifttt.com/maker_webhooks>.
- **n8n, Zapier, Make, Power Automate and your own service:** use the **Generic JSON** preset (the
  agent sends any JSON object) or write a template. Add the service's key as a *secret value* and
  send it in a header (for example `Authorization: Bearer` followed by the secret) when the
  receiver asks for one.

The links above are each receiver's own description of its format. If a receiver changes it, edit
the template on the connection's *Edit* page.

## Connecting

Choose *Outgoing webhook* under *Apps* and go through the steps:

1. **Name and URL.** Aurora checks the URL's form and that it is reachable address space, and
   refuses it with a clear reason when it is not `https`, points at an address Aurora does not reach
   or one your organization blocks. **Nothing is sent**, because a webhook does something when it is
   called.
2. **What the receiver expects.** Start from a preset, or paste an example message and mark the
   values the agent should fill in. A *Preview* shows the message for sample values, with secret
   values hidden.
3. **Extras (optional).** *Secret values* for a key or topic (up to four, usable in headers and in
   the body), *signing*, *PUT* instead of *POST*, and a *description for the agent*: say when to use
   the webhook and when not to.

## Signing

When a receiver is a service you run, turn **signing** on. Aurora shows the signing secret once (or
you paste your own). Each send then carries `X-Aurora-Signature: t=<unix seconds>,v1=<hex>`, where
`v1` is the HMAC-SHA256 of `<t>.<raw body>` with the secret. A receiver that checks it refuses
anyone who only learned the URL. Check the time too, to refuse a replay.

Python:

```python
import hmac, hashlib, time

def verify(secret: bytes, header: str, raw_body: bytes, tolerance=300) -> bool:
    parts = dict(p.split("=", 1) for p in header.split(","))
    t, v1 = parts["t"], parts["v1"]
    if abs(time.time() - int(t)) > tolerance:
        return False
    expected = hmac.new(secret, t.encode() + b"." + raw_body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, v1)
```

Node:

```js
const crypto = require("crypto");

function verify(secret, header, rawBody, tolerance = 300) {
  const parts = Object.fromEntries(header.split(",").map((p) => p.split(/=(.*)/s).slice(0, 2)));
  if (Math.abs(Date.now() / 1000 - Number(parts.t)) > tolerance) return false;
  const expected = crypto
    .createHmac("sha256", secret)
    .update(`${parts.t}.`)
    .update(rawBody)
    .digest("hex");
  const a = Buffer.from(expected);
  const b = Buffer.from(parts.v1);
  return a.length === b.length && crypto.timingSafeEqual(a, b);
}
```

Test vector: with the secret `whsec_test`, `t=1700000000` and the body `{"text":"hello"}`, the
signature is `v1=4de4482f0d79eb97df8f0e9ece4238de4e613a5ad29c846b70bc7dd6e1389e80`.

## No retries, and what `X-Aurora-Delivery` is for

Aurora **never resends** a message: a send that times out may have reached the receiver, and an
automatic retry could post it twice. The agent is told to ask you before sending again. Every send
carries a unique `X-Aurora-Delivery` header (also in the request log), so a receiver you run can
drop a duplicate if you do resend.

## Limits

A webhook has a per-minute and a per-day limit (the lowest of the instance's, your organization's
and the connection's own, which you can lower in *Edit*). Sends over the limit are refused with a
`429` and the time to wait. An organization can switch outgoing webhooks off altogether.
