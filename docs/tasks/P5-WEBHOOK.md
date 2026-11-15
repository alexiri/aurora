# P5-WEBHOOK: Outgoing webhook — agents call a webhook the person set up, without ever seeing its URL — **proposal, spec batch (Opus) after the owner's answers**

> **Status: proposal (2026-11-15).** Nothing here is decided. No D-, C-, SEC- or migration numbers are
> taken yet: the spec batch takes them from PROGRESS's next-free note once the owner has answered the
> questions at the end (`Q-P5-WH-n`). The recommendations are the coordinator's.

## Goal

Owner, 2026-11-15: "We should have an addon that allows agents to trigger external webhooks. The full
webhook URL would be the secret, so this should be handled similar to the ICS addon. When configuring
the addon, the user will probably need to specify the remote URL to hit and the shape of the contents,
so agents know what's expected on the other end."

Typical receivers: Slack, Discord, Mattermost and Google Chat incoming webhooks; Microsoft Teams
*Workflows* ("when a webhook request is received"); ntfy; Home Assistant automations; n8n, Zapier,
Make, Power Automate and IFTTT triggers; a person's own small service. In all of them the URL is a
bearer secret: whoever holds it can post.

**Naming.** Aurora already has *inbound* webhooks (an agent's Triggers › Webhooks, `10 §Webhooks`).
This app is the other direction, so it is called **Outgoing webhook** everywhere (package `webhook`),
and the docs never say just "webhook" for it.

## Today (what it builds on)

**P5-ICS** (D-273, C-843, SEC-107; queued in lane `p5-apps`, not yet implemented) adds the
**secret-link lane**:
- the person enters a name and a full URL, and the URL is the credential and the destination;
- the agent requests `https://<connection_id>.link.aurora.internal/`, listed in `~/.aurora/links.json`;
- the gateway sends the request to exactly the stored URL, under the destination guard and the org
  egress policy, and the URL never reaches the sandbox, a log, a trace, an approval or an API read.

D-273(f) already expects a later "secret URL" package to reuse that lane. But C-843 allows only
`GET`/`HEAD` and has no request body, so a webhook needs one extension to the core: a **send** mode.

## Proposed model

### 1. Connecting: what the person enters
One `fields` connect method, in three steps in the dialog:

1. **Name and URL.** `name` (≤ 60, becomes the connection label) and `url` (secret). As for ICS:
   `https`, a DNS host, ≤ 2,048 characters, private ranges only under `gateway.allowed_private`.
   **No test request at connect**, because a webhook acts when called. Connect only checks the syntax,
   resolves the host and runs the destination guard and the egress policy on it.
2. **What the receiver expects.** This is the payload definition (§2). It starts from a preset or from
   an example the person pastes.
3. **Optional extras** (folded away by default):
   - **Secret values** (≤ 4, name + secret). They can be used in headers (`Authorization: Bearer …` for
     an n8n header-auth node, `X-API-Key`) or inside the body (Pushover's `token`, an ntfy topic sent in
     JSON). They are sealed like the URL and never shown again.
   - **Signing** (§5).
   - **Method** `POST` (default) or `PUT`.
   - A **description for the agent** (≤ 1,000 characters), e.g. "Posts to #ops-alerts. Use it only for
     production incidents; one message per incident."

The connection card shows the name, the host, Read only and Edit. **Edit** changes the payload
definition, description, method and signing without re-entering the URL. A new URL means reconnect.

### 2. The payload definition: three ideas, one recommended

**Idea A — a description only.** The person writes in words what the receiver wants, perhaps with an
example. The agent builds any body, and the gateway checks only the size and that it is valid JSON.
It is the simplest option, but nothing stops a malformed or over-sharing body. The approval card is
the only check.

**Idea B — a JSON Schema.** The person pastes an example payload. The dashboard infers a schema (types;
the keys present become required; `additionalProperties: false`), and the person adds a description
per field. The gateway validates the agent's body against it (a small subset: `type`, `properties`,
`required`, `items`, `enum`, `maxLength`, `minimum`/`maximum`, `additionalProperties`). This is good for
any JSON receiver, but the agent still writes the whole body, including the parts that should never
change (Slack's `username`, a Teams card skeleton, a routing key).

**Idea C — a template with typed slots (recommended).** The person writes, or picks from a preset, the
body the receiver expects, with `{{slot}}` placeholders where the agent fills in values:

```json
{ "username": "Aurora", "icon_emoji": ":robot_face:", "text": {{text}} }
```

Each slot has:
- a name;
- a type: `string`, `number`, `boolean`, `string_list`, `enum`, or `json` with an optional schema as in
  Idea B;
- required or optional (an optional slot that is left out is removed from the rendered body);
- a description for the agent;
- limits (`max_length`, `enum` values).

The agent sends only the slot values, `{"text": "Deploy 42 failed on prod"}`. The gateway validates
them, renders the template with the escaping of the content type, and sends the result. `{{secret:name}}`
inserts one of the connection's secret values, which the agent never sees.

Why C:
- **The agent controls the least.** It cannot add keys, change the fixed parts or route the message
  elsewhere (`channel`, `topic`, a Teams card's target).
- **It covers A and B.** "Free-form JSON" is the template `{{payload}}` with one `json` slot, with or
  without a schema. Idea B is that slot with a schema.
- **It works beyond JSON.** `application/x-www-form-urlencoded` (key = template, values are slots) and
  `text/plain` (ntfy, plain-text receivers) render with their own escaping.
- **The approval card is clearer.** It shows the slot values by name, not a raw body.

**The dashboard editor ("start from an example").** The person pastes the example payload that the
receiving service documents. Each value is shown as a chip: click one and choose "the agent fills this
in", then name it and describe it. Everything else stays fixed. A raw template and slot editor is
there for anything the picker cannot express. A **Preview** shows the rendered body for sample values,
with secret values masked as `«secret:name»`.

**Presets** are data in the package (`presets/*.yaml`), not code per provider:
- Slack, Mattermost and Google Chat (`{"text": {{text}}}`);
- Discord (`{"content": {{content}}}`);
- Teams Workflows (an Adaptive Card skeleton with `title` and `text` slots);
- ntfy JSON (`topic` as a secret value, `title`, `message`, `priority`, `tags`);
- Home Assistant (free-form JSON);
- IFTTT (`value1..3`);
- generic JSON (`{{payload}}`).

A preset may list `suggest_for_hosts` (`hooks.slack.com` → Slack), so the dialog can propose it from the
URL's host. It is only a suggestion: any URL works with any preset or a custom template. This keeps the
owner's rule from P5-ICS that nothing is hard-coded per provider.

Bounds (the spec fixes them in `limits.rs`): template ≤ 16 KiB, ≤ 20 slots, slot values ≤ 64 KiB in
total, rendered body ≤ 256 KiB.

### 3. What the agent sees and does
- The supervisor writes **`~/.aurora/webhooks.json`** (`0600`, removed when empty), one entry per
  granted connection: `name`, `url` (`https://<connection_id>.link.aurora.internal/`), `description`,
  `method`, `slots` (name, type, required, description, limits) and an example call. It never contains
  the template, the fixed values, the secret values or the real URL. It is a separate file from
  `links.json` so the calendar snippet stays simple, and both come from the same home-sync `links` list,
  which gains a `mode`.
- **Sending:** `curl -X POST <url> -H 'Content-Type: application/json' -d '{"text": "…"}'` (the slot
  values).
- **Dry run:** the header `X-Aurora-Dry-Run: 1` validates the values and returns the rendered body,
  with secret values masked, without sending. It goes through the same admission, so the agent can check
  a payload before acting.
- **The answer:**
  - the receiver's status and body come back, with the body capped (64 KiB) and passed through the
    echo check;
  - a gateway refusal carries a fixed `code` and the header `X-Aurora-Error`, so the agent can tell it
    apart from the receiver's own 4xx (e.g. Slack's `invalid_payload`);
  - `invalid_slot` names the slot and the rule it broke.
- **Agent snippet** (≤ 2,000 characters):
  - read `webhooks.json` first and send only to listed webhooks, following each one's description;
  - use the dry run when unsure;
  - **after a timeout, do not retry blindly**, because the receiver may already have acted; say so, or
    ask the person;
  - a refusal code is not the receiver's answer.
- **Later, with P5-GUIDE-6/7:** each webhook could also appear as a typed operation of the `webhook`
  target of `aurora-agent` (`aurora-agent describe webhook`, `aurora-agent call webhook <name> …`), with
  its slots as the input schema. That needs per-connection operations in `api.json`, so it is a
  follow-up and not part of this proposal (Q-P5-WH-9).

### 4. Gateway: the send mode of the secret-link lane
The admission is unchanged from C-843: the connection exists and is of a `secret_link` app, it is
granted to the calling agent, and policy, approvals, Read only (FIX-133) and the request log apply,
with the tool taken from the manifest. Then:
- **Methods:** the connection's `POST`/`PUT` only (405 otherwise). The agent's path and query are
  ignored.
- **Headers:** only the agent's `Content-Type` (`application/json`) is accepted. Upstream go:
  - the rendered `Content-Type`;
  - the connection's secret headers, with a denylist of names (`Host`, `Content-Length`, `Cookie`,
    hop-by-hop, `Forwarded`/`X-Forwarded-*`, `X-Aurora-*` except the signature);
  - the signature, if on.

  Nothing else from the agent goes upstream.
- **Destination:** the stored URL exactly. The destination guard and the org egress policy apply to the
  host and to every redirect hop.
- **Redirects** (≤ 3, `https` only, never shown to the agent): a `307`/`308` is followed with the
  same method and body. A `301`/`302`/`303` is followed as a body-less `GET`, as browsers do; Google Apps
  Script web apps answer a `POST` this way. Q-P5-WH-7 asks about this.
- **Time-out:** 15 s. Then `504 webhook_timeout`, and the snippet says "may have been delivered".
- **No automatic retry and no cache**, because a webhook is not idempotent.
- **Rate limit per connection**, on the shared counters (D-119): defaults 30 a minute and 500 a day. The
  person may lower them, and the instance sets the ceiling.
- **Echo check (FIX-104):** the needles are the URL, its path-with-query (raw and percent-encoded) and
  every secret value. A match gives 502 `upstream_echoed_credential`.
- **Logs and traces:** the connection id, the host, the status, the slot *names* and the rendered size.
  Never the path, query, slot values, rendered body or secret values. The request log keeps no bodies,
  as today.

### 5. Signing (optional, for receivers the person runs)
If signing is on, the gateway adds the header
`X-Aurora-Signature: t=<unix seconds>,v1=<hex HMAC-SHA256(secret, "<t>.<body>")>`. The secret is
generated at connect and shown once, or pasted by the person. It is the same `hmac_sha256`-over-the-raw-
body family as Aurora's inbound webhooks (D-34), plus a timestamp against replay. A receiver that
checks it rejects anyone who only learned the URL. Each send also carries `X-Aurora-Delivery: <uuid>` (also
written to the request log), which a receiver can use to drop duplicates.

### 6. Policy, approvals and governance
- The package's one tool, `send`, is a **write** tool:
  - a Read-only connection refuses every send, so the person can pause a webhook without deleting it;
  - the proposed default is **Ask** (Q-P5-WH-3). The person can allow it per agent with an ordinary
    policy rule ("send to Ops alerts without asking").
- **The approval card** shows the webhook's name and host, the method and each slot value (long values
  folded). It shows neither the URL, the fixed parts of the template nor secret values. It uses a new
  `slots` approval decoder.
- **Org switch** `outgoing_webhooks.enabled` (default on), like `calendar_invitations.enabled` (D-274).
  An org that does not want data leaving to arbitrary URLs turns it off. The egress policy still applies
  per host when it is on.
- **Turn kinds:** unlike calendar invitations (D-274(c)), sends are allowed in schedule, inbound-webhook
  and peer turns. "Something happens → the agent decides → post to a channel" is the main use. The
  destination is fixed by the person and policy applies as usual. A prompt injection can at most make
  the agent post slot-shaped content to that one receiver. With Ask, a person sees it first.
- **Sharing:** a connection is shared the way any connection is (personal or workspace credential,
  grants). A team's Slack channel webhook is a workspace credential.

### 7. Dashboard
- The connect dialog's three steps (§1), the example picker, the raw template and slot editor, the
  Preview and the presets (§2).
- A **Send test** button on the connection card: the person enters sample values and sends once. This
  is a real call, made by the person and logged as theirs. It is never automatic.
- The shared primitives and page patterns of `14 §Visual design`, in both themes and at 390 px. The
  template editor reuses the code-editor primitive used for policy JSON, if one exists; otherwise the
  dashboard brief adds one to the shared set.

## What changes where (for the spec batch)
- **`09` §3.4:** `secret_link` gains `mode: fetch | send` (default `fetch`; the ICS package is
  unchanged). In `send` mode:
  - the tools are `POST`/`PUT` on `*.link.aurora.internal`;
  - `expect_prefix` is refused;
  - the connection carries a payload definition.
- **Storage:** connection metadata fields today are short strings, and a payload definition (≤ 16 KiB
  JSON) does not fit them. Two options: a `config jsonb` column on connections (one migration), or a
  sealed document beside the credentials. The spec picks; the coordinator leans to the column, because
  the definition is not secret and editors read it back.
- **`06`:** the send path of the secret-link lane. **`07`:** the `slots` decoder and the default.
  **`12`:** `webhooks.json`. **`14`:** the dialog and editor. **`15`:** the org switch. **`17`:** extend
  SEC-107 (it says "`GET`/`HEAD` only" today) or add a sibling invariant for send mode.
  **`05`/API_CONTRACTS:** connect with a payload definition, edit, dry run, send test, error codes.
  **`18`:** the tests below.

## Tests the spec will require (sketch)
- **Linter:** `send` mode with `expect_prefix`; a `GET` tool in send mode; a template that refers to an
  undeclared slot or secret.
- **Render:** slot-type validation for each type; JSON, form and text escaping (quotes, newlines,
  `&`/`=`, Unicode); an optional slot omitted; a secret value inserted; a template-injection attempt
  (a value containing `{{secret:x}}` stays literal).
- **Gateway:**
  - an ungranted agent → refused;
  - Read only → refused;
  - `GET` → 405;
  - the agent's headers are not forwarded;
  - redirects per the chosen rule;
  - a time-out → 504, with no retry;
  - the rate limit is enforced;
  - the echo of a secret value → 502;
  - dry run sends nothing upstream;
  - canary tokens in the URL, the secret values and the slot values appear in no log row, span, error or
    API read.
- **Approval:** the card shows the slot values and no fixed part or secret.
- **Signing:** vectors; the receiver-side verification example in the setup guide checked against them.
- **End-to-end on the dev stack:** a recorded receiver gets the rendered body; an agent posts through
  `webhooks.json`.
- **Live check by the lane:** one real incoming webhook of the owner's choice (e.g. a test Mattermost
  channel), kept outside the repo like the ICS test link.

## Alternatives considered
- **One package per service** (a Slack webhook app, a Discord webhook app, …): every new receiver is a
  package change. The owner rejected this for ICS as not scalable. Presets give the convenience without
  it.
- **Give the agent the URL** (an environment secret): the URL is a bearer secret, and it would end up
  in transcripts, files and logs.
- **A built-in platform tool `webhook_send`** (like `calendar_invite`): it would not use the gateway's
  grants, approvals, request log and egress policy, which the app lane already provides. A typed
  interface through `aurora-agent` is the better later step (Q-P5-WH-9).
- **Let the agent write any body (Idea A)**: see §2.

## Risks
- **An exfiltration channel by design.** Any string slot can carry anything the agent knows. This is
  the same as posting to Slack through the `slack` app. Mitigations: the person picks the destination,
  Ask by default, slot limits, the org switch and the egress policy.
- **Duplicate sends** after a time-out. There is no automatic retry, the snippet warns about it, and
  `X-Aurora-Delivery` lets receivers drop duplicates.
- **Template-editor scope.** The example picker is the most work in the dashboard brief. The raw
  editor alone would be enough for a first version (Q-P5-WH-8).

## Size and split (proposed)
- **P5-WEBHOOK-1** (Rust, M–L, `rust-developer`):
  - the registry `mode: send`;
  - the gateway send path (render, validate, sign, redirects, rate limit, dry run);
  - the API connect, edit, send-test and storage (maybe one migration);
  - the home-sync `mode` and the supervisor's `webhooks.json`;
  - the `slots` decoder;
  - the package with its presets and setup guide.

  It goes in the same lane as P5-ICS and **after it**, because it extends the same registry, gateway
  and supervisor code.
- **P5-WEBHOOK-2** (Next.js, M, `nextjs-developer`): the connect steps, the example picker, the raw
  editor, the Preview, Send test and the connection card. It can run in parallel with -1 once the
  contract is fixed.

## Open questions for the owner
- **Q-P5-WH-1** Payload model: a template with typed slots (Idea C) rather than a description only (A)
  or a schema only (B)? *Recommended: C. Free-form JSON is one of its presets.*
- **Q-P5-WH-2** Phase: build it in phase 5, in `p5-apps` right after P5-ICS (and P5-CALDAV), or keep it
  for phase 6? *Recommended: phase 5 after P5-ICS, because it is a small extension of code that lane
  will have just written. Phase 6 is fine if phase 5 should not grow.*
- **Q-P5-WH-3** The default for `send`: Ask, or Allow? *Recommended: Ask. A webhook acts outside Aurora.
  One policy rule per agent turns it into Allow.*
- **Q-P5-WH-4** Secret values (headers and `{{secret:…}}`, ≤ 4) in the first version? *Recommended: yes.
  Without them n8n header auth, Pushover and ntfy-over-JSON do not work.*
- **Q-P5-WH-5** Signing (`X-Aurora-Signature`) in the first version? *Recommended: yes. It is small and
  protects receivers the person runs.*
- **Q-P5-WH-6** `https` only, or also `http` for receivers on a private network (Home Assistant on a
  LAN), and then only inside `gateway.allowed_private`? *Recommended: `https` only, as for ICS. Revisit
  if someone asks.*
- **Q-P5-WH-7** Redirects after a `POST`: follow `307`/`308` with the body and `301`/`302`/`303` as a
  `GET` (≤ 3, guarded), or refuse all redirects? *Recommended: follow as described. Refusing breaks
  Google Apps Script receivers.*
- **Q-P5-WH-8** Dashboard: the example picker in the first version, or the raw template and slot
  editor plus presets first? *Recommended: the picker. It is what makes "the shape of the contents" easy
  for a non-developer. Without it the feature is usable only by people who write JSON.*
- **Q-P5-WH-9** Later: expose each webhook as a typed `aurora-agent` operation (P5-GUIDE-6/7)?
  *Recommended: yes, as a follow-up after both land. Not in this batch.*
- **Q-P5-WH-10** Default rate limits: 30 a minute and 500 a day per connection? *Recommended: yes. The
  instance ceiling becomes a P6-LIMITS quota when that lands.*
