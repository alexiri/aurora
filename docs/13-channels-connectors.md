# 13 — Channels and the Connector Contract

Phase: **0** (channel core, contract v1, fake connector, conformance suite,
`aurora-connector-sdk`) · **1** (Mattermost connector, `20`). Tables: `03 §10`.

Implements Proposal 02. **The core owns the conversation logic; a connector
owns the chat platform.** The core contains no platform names (CI-enforced).

## 1. Division of responsibility

| Connector (per platform, stateless, N replicas) | Core (written once) |
|---|---|
| Keep platform connections open; receive events; host its own platform callbacks | Durable inbox and dedupe by platform event id |
| Translate platform events → neutral events | Target-agent routing (dedicated bots, space bindings, DM defaults) |
| Render neutral content and cards → platform posts | Speaker authorization (user links + access law), reach, listen modes |
| Map platform errors → neutral error codes | Thread ↔ conversation mapping, turn creation |
| Provision/deprovision bot identities where possible | What gets posted when (answers, receipts, cards, notices) |
| Rotate its own platform credentials (via compare-and-set write-back) | Approval/action/reach/binding cards, action tokens, clicker authorization |
| | Messaging tools, recipients and mention anchors |
| | Credential storage (secret stores) |

## 2. Neutral model

| Entity | Meaning |
|--------|---------|
| **Connector** | A registered platform implementation (`connectors`): id, announced signed manifest, replicas, approval status. |
| **Integration** | An org's link to one platform tenant with the org-level credential (`channel_integrations`). `tenant_ref` is globally unique per connector (first org wins). May carry a **shared bot** identity. |
| **Presence** | An agent on an integration. `dedicated`: its own bot (credentials on the presence). `shared`: served by the integration's shared bot (default display name `agents.default_name`, i.e. `Daneel`). |
| **Space binding** | Shared bot: which agent answers in a space, listen mode, follow-up rule. Created/changed only by a `can_own` user of the agent's workspace. |
| **DM default** | Shared bot: which agent a person's top-level DMs go to. |
| **Thread link** | (space, thread) ↔ conversation for one agent; `direct` links all point at the agent's direct conversation with that person. |
| **User link** | platform user ↔ Aurora user (verified email, sign-in link, or admin). |
| **Reach grant** | widens who may talk to an agent beyond workspace members (person or space). |
| **Mention anchor** | per-conversation pin "this alias means this platform-qualified user/space". |
| **Inbox / ops / cards** | durable inbound events; outbound operations (outbox); posted interactive messages. |

Ownership: each integration is served by **one connector replica at a time**
(`owner_replica_id` + `owner_lease_until`, renewed by the config poll). That
replica holds every socket of the integration (shared bot and dedicated bots)
and claims only that integration's operations. When the owner stops polling,
the lease lapses and another replica takes over at its next config poll.
Replicas balance by claiming at most `ceil(integrations / live replicas)`.

## 3. Connector registration, manifest and trust

- An instance admin provisions a connector id (`POST /v1/admin/connectors {id}`)
  and receives its anchor token `aun_…` (status `awaiting_registration`). (phase 4, P4-MCON)
  This is the **self-hosted** path; a **managed** connector is added from the dashboard and run
  by Aurora without a token anyone handles (§3.1).
- The service calls `register` with its **manifest** and a detached signature.
  The core verifies the signature against the built-in project key or a trusted
  publisher key (`09 §2`, signing over the sha256 of the canonical JSON of the
  manifest). First registration, or any widening (new capability, new setup
  field, new public callback), puts the connector in `pending_approval`;
  integrations are not served until an instance admin approves. Unchanged or
  narrowed manifests are accepted automatically.
- The core cannot verify which binary runs; operators SHOULD enforce image
  signatures with their deployment tooling (e.g. cosign + admission policy).

Manifest (JSON):

```json
{
  "package": { "id": "mattermost", "version": "1.0.0", "contract": 1, "publisher": "aurora" },
  "about": { "name": "Mattermost", "icon_png_b64": "…", "setup_guide_md": "…" },
  "capabilities": {
    "threads": true, "edits": true, "buttons": true, "menus": true, "text_input": false,
    "reactions": true, "ephemeral": true, "native_status": false,
    "presence_dedicated": true, "presence_shared": true, "user_tokens": false,
    "email_verified": true, "commands": true, "history": true,
    "files": { "max_bytes": 52428800, "max_count": 10 }
  },
  "integration_setup": { "fields": [ /* same form-field schema as app `fields` (09 §3.4) */ ] },
  "presence_setup": { "dedicated": { "fields": [] } },
  "limits": { "message_max_chars": 16000, "writes_per_space_per_minute": 30 },
  "public_callbacks": ["actions", "commands"]
}
```

Capability names are an open vocabulary; unknown capabilities are ignored.
Strings are sanitized; the icon and guide follow app-package asset rules.

(phase 4, P4-MCON) The manifest may also carry `"instance_setup": { "fields": [ … ] }`:
connector-level settings an instance admin fills in when adding a **managed** connector
(§3.1), in the same form-field schema as `integration_setup` (`09 §3.4`) plus the field types
`url`, `number` (optional `min`/`max`) and `generated_secret` (the core generates it, §3.1) and an
optional static `default`; `file`, `json_file` and `store` are not allowed there (C-450). A new `instance_setup` field is a widening.
Connectors without the key have no instance settings.

### 3.1 Managed connectors (phase 4, P4-MCON, D-128)

Phase 0–3: every connector is a separate service the operator deploys, given its `aun_`
token and its settings through its environment (§3). From phase 4 an instance admin can
instead **add a managed connector from the dashboard**, like approving an app: pick a type,
review what it can do, fill in its settings and secrets, save. Aurora runs it. Nothing is
copied by hand and no deployment changes. Self-hosted connectors (§3) stay supported for
types Aurora does not ship and for operators who run connectors elsewhere.

**Connector host.** `aurora-connector-host` is a core service (Helm `connectorHost`, on by
default, `15`) that links the **built-in connector types** shipped with Aurora (phase 4:
`mattermost`; `fake` only with `connector_host.test_types = true`) and runs one instance of a
type per managed connector, in-process, with the connector SDK runtime (§8). It never runs
anything an admin names at run time: no image, binary, URL or code comes from the dashboard
(SEC-69). It authenticates to the API with its anchor token `auh_` (`00`; generated by the chart
like the runner token, bootstrapped by `AURORA_BOOTSTRAP_CONNECTOR_HOST_TOKEN`; rotatable by an
instance admin, `05`). (FIX-120, D-189) **One token per host, bound to the connector types that
host runs**: an instance admin issues a token for a named set of `type_ids` (`05`, C-690..C-692);
the token can only register those types, only receives assignments (and so the decrypted settings
and secrets) of those types, only reports status for connectors of those types, and acts as a
managed connector (C-458) only for those types. A host instance id belongs to the token that first
registered it: another token may not register, poll or report as it (403
`host_instance_mismatch`; a register naming an unbound type is 403 `type_not_bound`). The shared
bootstrap token (unbound) keeps working **until the first bound token exists**, then it is refused
(`15 §Upgrades`). Every replica runs every enabled managed connector; the connector
protocol's replica and integration ownership (§4) already handles several replicas.

**Host protocol** (`/v1/connector-host/*`, `auh_` only):
- `register` announces the host version and its **catalogue**: per type, the manifest and its
  detached signature (signed like any connector manifest, §3; built-in manifests are signed with
  the project key in the release build). With `plugins.allow_unsigned = true` (development
  only) a type with an empty signature is accepted and shown as "Unsigned (development)"; an
  invalid signature is never accepted, and with the flag off an unsigned type is rejected (D-142).
- `assignments` (long poll) returns the enabled managed connectors with their type, non-secret
  settings, and secret settings **decrypted for the host only**, plus a revision; the host
  starts, restarts (settings changed) or stops connector instances to match.
- `status` reports per managed connector `running` or `error` with a sanitized message (≤ 300
  chars; never a secret value), shown in the dashboard.
- A host acts as a managed connector on the connector contract (§4) by sending its `auh_` token
  with the header `Aurora-Connector: <id>`. The API accepts that only for a connector that is
  `managed` and not `disabled`; for a self-hosted connector, or with an `aun_` token, the header
  is ignored and an `auh_` token without the header is refused 401 (SEC-69). Everything else on
  the connector contract is unchanged (registration, manifest approval and widening, ops, events).

**Adding a managed connector** (instance admin, dashboard Admin → Connectors → Add):
1. Choose a type from the catalogue the live hosts announced (types already added are marked).
   With no host registered in the last `CONNECTOR_HOST_STALE` (90 s), the page says the
   connector host is not running and links the operator doc.
2. Review: name, icon, version, publisher, capabilities, public callbacks and setup fields from
   the type's manifest (the same summary as the approval screen).
3. Settings: the `instance_setup` fields. `secret` fields are password inputs, never shown
   again; `generated_secret` fields are generated by the core (32 random bytes, base64) and can be
   regenerated later. The connector `id` is the type id (one managed connector per type; `package.id`
   must equal the connector id, §3); `display_name` defaults to the manifest name.
4. Save: `POST /v1/admin/connectors` with `managed: true`. The manifest digest the admin reviewed
   is approved in the same request (`approve_manifest_digest`); a different digest → 409
   `stale_manifest`. No token is returned or shown.

The connector is `awaiting_registration` until a host registers it, then `active`. A later host
version with a widened manifest puts it in `pending_approval` exactly as §3 (the dashboard shows
the manifest diff). Disabling sets `disabled` (hosts stop it, integrations are not served);
enabling restores the previous status. Deleting refuses with 409 `connector_in_use` while
integrations exist (§3) and otherwise discards the stored secrets.

**Settings storage.** Non-secret settings: `connectors.settings` (jsonb, validated against
`instance_setup` on every write). Secret and generated settings: sealed in the secret store
(`08`, purpose `connector.settings`, owner = `connectors.secret_owner`), referenced from
`connectors.secret_refs` (field name → reference). The API returns secrets only as
`{ set, updated_at }`; a PATCH replaces or clears one; a store failure refuses the write with
503 `secret_store_unavailable` and changes nothing. Secret settings are decrypted only to build an
`assignments` response (SEC-70).

**Callbacks.** The host serves every managed connector's public callbacks on one port (4110),
each under `/connectors/<id>/`. The edge routes `/connectors/` to the host (Ingress and
HTTPRoute rules, `15`; `scripts/guards/edge-routes.sh`). A connector's `public_url` setting
defaults to `<server.external_url>/connectors/<id>`; set it only when the platform reaches
Aurora under another name.

**Mattermost** declares `instance_setup`: `public_url` (url, optional, default above),
`callback_hmac_key` (`generated_secret`), `catchup_window_secs` (number, default 600, 1–86400),
`token_check_secs` (number, default 30, 1–3600). The server URL and admin token stay per
integration (`integration_setup`, `20`).

## 4. Connector contract v1 (HTTP/JSON, connector → core)

Base `/v1/connector`. Auth `Authorization: Bearer aun_…`; every call after
`register` carries `X-Aurora-Replica`. All bodies schema-validated
(`aurora-protocol::connector`).

| Call | Request → response |
|------|--------------------|
| `POST /register` | `{ manifest, signature, version, replica_hint? }` → `{ connector_id, replica_id, status, poll: { config_secs: 15, ops_wait_secs: 25 } }` |
| `GET /config` (`If-None-Match`) | → 304 or `{ etag, lease_secs, integrations: [Integration], presences: [Presence], space_bindings: [Binding] }` — only items owned by this replica (claimed during this call); **includes decrypted platform credentials** |
| `POST /events` | `{ events: [InboundEvent] }` (≤100) → `{ accepted: [event_id], duplicates: [event_id], rejected: [{ event_id, code }] }`. The core persists to the inbox before answering; the connector acks the platform only afterwards. |
| `POST /ops/claim` | `{ max: 1..100, wait_secs: 0..25 }` → `{ ops: [Operation] }` — claimed with lease `OP_LEASE` |
| `POST /ops/results` | `{ results: [{ op_id, outcome: "ok"|"error", data?, error?: { code, message, retry_after_secs? } }] }` → `{ ok: true }` |
| `POST /credentials` | `{ target: { integration_id | presence_id }, expected_version, credentials }` → `{ version }` or 409 `stale_version` (compare-and-set write-back of rotated platform credentials) |
| `POST /link-start` | `{ integration_id, platform_user_ref }` → `{ url, expires_at }` (sign-in link for an unlinked person) |
| `PUT /files/{op_id}` | raw bytes of a fetched inbound file for a claimed `file.fetch` op (≤ the op's `max_bytes`; headers `Content-Type`, `X-Aurora-Filename`) → `{ attachment_id }` |
| `GET /files/{attachment_id}` | bytes of an outbound attachment referenced by a claimed `message.post` op of this replica (any other attachment → 404) |
| `POST /retire` | → clean shutdown; releases owned integrations |

Shapes:

```json
// Integration
{ "id", "org_id", "tenant_ref", "tenant_label", "config": {}, "credentials": {}, "credentials_version": 3,
  "shared_bot": { "platform_user_ref", "handle", "display_name" } | null, "status" }
// Presence
{ "id", "integration_id", "mode": "dedicated"|"shared", "agent": { "id", "handle", "name", "avatar_url" },
  "platform_user_ref"?, "handle"?, "credentials"?, "credentials_version"?, "status" }
// Binding
{ "integration_id", "space_ref", "agent_id", "listen": "mentions"|"all", "followups_need_mention": false }
// Operation
{ "op_id", "kind", "integration_id"?, "presence_id"?, "attempt": 1, "maybe_sent": false, "payload": {},
  "credentials"?: {} }   // phase 3, RFE-024: decrypted at claim time, only for self-contained presence.deprovision
```

### 4.1 Inbound events (platform → neutral)

`InboundEvent = { event_id, integration_id, kind, occurred_at, payload }`.
`event_id` is the platform's own id (or a deterministic hash of platform ids
when the platform has none) — the core dedupes on `(connector, event_id)`.
(phase 3, RFE-018) `event_id` values beginning with `api:` are reserved for
API idempotency keys (`05 §Idempotency keys`): such an event is `rejected` with
code `reserved_event_id`.

| Kind | Payload |
|------|---------|
| `message.posted` | `{ space: { ref, kind: "dm"|"group_dm"|"channel"|"private", label }, thread_ref?, message_ref, author: { ref, kind: "person"|"bot", display_name, email?, email_verified? }, text (neutral markup), mentions: [{ kind: "user"|"bot"|"space"|"broadcast", ref }], bot_mentioned: bool, addressed_bot_ref, files: [{ ref, name, size, content_type }] }` |
| `message.edited` / `message.deleted` | `{ space_ref, thread_ref?, message_ref, text? }` |
| `bot.added` / `bot.removed` | `{ space: {ref, kind, label}, bot_ref, actor_ref? }` |
| `action.invoked` | `{ token, clicker_ref, value?, space_ref?, message_ref? }` |
| `command.invoked` | `{ name, args: string, space: {ref, kind}, thread_ref?, user_ref, response_ref? }` |
| `presence.removed` | `{ presence_id? , bot_ref, reason: "uninstalled"|"token_revoked"|"deleted" }` |
| `reaction.added` | `{ space_ref, message_ref, user_ref, emoji }` (optional) |

### 4.2 Outbound operations (core → platform)

| Kind | Payload → result `data` |
|------|-------------------------|
| `message.post` | `{ target: { space_ref, thread_ref? } | { user_ref }, sender: { presence_id } | "shared", content: Content, files?: [{ attachment_id, name, download_url }], ephemeral_to?: user_ref, op_marker }` → `{ message_ref, thread_ref }` |
| `message.update` | `{ space_ref, message_ref, sender, content }` → `{}` |
| `message.delete` | `{ space_ref, message_ref, sender }` → `{}` |
| `reaction.add` / `reaction.remove` | `{ space_ref, message_ref, sender, emoji: neutral name }` |
| `status.set` / `status.clear` | `{ space_ref, thread_ref?, sender, text? }` (native status only) |
| `file.fetch` | `{ file_ref, max_bytes }` → `{ upload_url_used: true, attachment_id }` (connector uploads the bytes to `PUT /v1/connector/files/{op_id}`) |
| `user.lookup` | `{ user_ref } | { query, limit }` → `{ users: [{ ref, display_name, handle, email?, email_verified, is_bot }] }` |
| `spaces.list` / `space.members` | `{ query?, limit }` / `{ space_ref }` → lists |
| `history.read` | `{ space_ref, thread_ref?, limit ≤ 50, before? }` → `{ messages: [{ message_ref, author, text, at }] }` |
| `presence.provision` | `{ agent: { handle, name, avatar_url } }` → `{ platform_user_ref, handle, credentials }` |
| `presence.deprovision` / `presence.rename` / `presence.set_icon` | `{ presence_id, … }` |
| `credentials.validate` | `{ config, credentials }` → `{ tenant_ref, tenant_label, shared_bot?, capabilities? }` — `capabilities` may only **narrow** the manifest's for this integration (e.g. no `ephemeral` without an admin token); stored in `channel_integrations.config._capabilities` and used by the core for fallbacks |

**Self-contained deprovision operations** (phase 3, RFE-024, D-67). Phase 0–2
enqueued `presence.deprovision` "before the delete commits", but the delete then
cascaded away the presence (and, for an org, the integration) rows the connector
needs, and `channel_ops.integration_id` itself cascades — so the op could be
unexecutable by construction. From phase 3 a deprovision op carries **everything
needed to act**, and is committed **in the same transaction as the delete, before
the cascade**:

- **Ordering guarantee.** Agent, workspace and org deletion (and presence detach)
  run as one database transaction in which the API (1) inserts one
  `presence.deprovision` op per *dedicated* presence about to disappear, then (2)
  performs the delete/cascade. Either both commit or neither does: there is no
  deleted presence without its op, and no op for a presence that still exists.
  (Sealing of credential copies happens just before the transaction, below.)
- **Op shape.** `integration_id = NULL` (so the integration's deletion cannot
  cascade the op away, and any live replica of the connector may claim it — the
  existing rule for ops without an integration), `presence_id` set (no FK),
  `connector_id` set, and a payload
  `{ presence_id, reason: "detached"|"agent_deleted"|"workspace_deleted"|"org_deleted",
     platform_user_ref, handle, tenant_ref, tenant_label, config: {…non-secret
     integration setup fields…}, credentials_ref }` (the core also keeps `org_id`
     in the stored payload, for the seal context and the failure audit row; like
     `credentials_ref` it is stripped before the op is handed out). `credentials_ref` is a **fresh
     SecretRef** holding a copy of the integration credentials the connector needs
     to administer the bot (for Mattermost the admin token, `20`), sealed under a
     context bound to the op (`SealContext` purpose `channel_op_deprovision`, the op
     id) so it cannot be opened for any other row. Bot-level credentials of the
     presence are included only when the connector's manifest says deprovision needs
     them. Nothing else about the integration, agent or org is copied.
- **Delivery.** At claim time the core opens `credentials_ref` and returns the
  decrypted credentials in the claimed `Operation` as `credentials` (the same
  treatment as `GET /config`, over the same connector authentication); the stored
  payload never holds plaintext.
- **Lifetime of the copy.** When the op reaches a terminal state (`done`,
  `failed`, `dropped`) — or on the periodic channel-ledger pruning, whichever is
  first — the core `discard`s `credentials_ref` and replaces it with `null`. If the
  transaction aborts (the delete fails) the API discards the refs it sealed
  (best effort; an orphaned sealed copy is unreadable without its op context and is
  removed by the pruning job when no op references it).
- **Retries.** `unavailable` is retried with the usual backoff until
  `DEPROVISION_RETRY_WINDOW` (24 h) rather than `OP_MAX_ATTEMPTS`;
  `credential_dead` / `forbidden` → `failed`; `not_found` → `dropped` (the bot is
  already gone). Shared presences have no bot of their own and enqueue nothing.
- **Skipped / failed deprovision is a documented residual.** If the connector is
  down or uninstalled for longer than the window, or the stored credentials were
  revoked, the bot **stays on the platform**. The core records a best-effort audit
  row `presence.deprovision_failed` (`metadata: { connector_id, tenant_label,
  platform_user_ref, handle, error_code }`, org-scoped; ledgers survive org
  deletion), counts `aurora_channel_deprovision_failed_total`, and `15` tells
  operators how to remove such bots by hand. The integration's *shared bot* is
  operator-installed and is **not** deprovisioned by org deletion (also documented).

**Delivery**: operations are claimed with a lease and retried until a
definitive result. Every `message.post` carries `op_marker` (= op id); connectors
MUST attach it invisibly (platform metadata/props) and, when `maybe_sent` is
true (re-claim after a lapsed lease), SHOULD check recent history for a post
with that marker before posting again. Together with claim leases this gives
at-most-once posting in practice.

**Neutral error codes** drive uniform core behaviour:

| Code | Core behaviour |
|------|----------------|
| `credential_dead` | presence (or integration) → `disabled` with reason; notice to affected agents' operators; op `failed` |
| `rate_limited` | `not_before = now + retry_after_secs` (default 30 s), no attempt consumed |
| `unavailable` | retry with exponential backoff up to `OP_MAX_ATTEMPTS`, then `failed` |
| `forbidden` | `failed`, visible on the conversation/presence (e.g. bot not in channel) |
| `not_found` | `dropped` |
| `invalid` / `too_large` / `unsupported` | `failed` (core bug or capability mismatch; logged at error) |

**Correlated operations** (`find_recipient`, `credentials.validate`,
`history.read`, `file.fetch` during inbox processing): the core inserts the op
with a `correlation_id`, waits on `aurora_op_result` up to 10 s (30 s for
`file.fetch`) and fails the caller with `connector_unavailable` on timeout.
Ops without an integration (e.g. `credentials.validate` for a new
integration) are claimable by any live replica of the connector.

### 4.3 Neutral content

- **Text**: a CommonMark subset — bold, italics, inline code, code blocks,
  links, lists, block quotes, simple tables. Mentions are tokens
  `<@user:REF>`, `<#space:REF>`, `<@here>`; only the core may emit `<@here>`
  and only when explicitly intentional (never from model text). Connectors
  convert to platform markup and **must defuse** any platform broadcast syntax
  (`@channel`, `@all`, `@here`) appearing in text.
- **Content** = `{ text?, card? }`; **Card** = `{ blocks: [Block] }`, blocks:
  `header {text}`, `text {text}`, `fields {pairs: [{label, value}]}`
  (≤12), `context {text}`, `link {label, url}`, `buttons {items: [{label,
  style: "primary"|"danger"|"default", token}]}` (≤5), `select {placeholder,
  options: [{label, value}], token}` (≤25), `input {label, token}` (if
  `text_input`).
- All text in cards is produced by the core (platform voice) or wrapped as
  sanitized data (summaries, names).

### 4.4 Action tokens

Phase 0: `v1.<b64url(payload)>.<b64url(HMAC-SHA256(action-token key, payload))>`; (phase 2, RFE-007) `v2.<key_id>.<b64url(payload)>.<b64url(HMAC-SHA256(action-token key of key_id, payload))>` signed with the active signing key and verified with the ring entry named by `key_id` (`v1.` tokens: try each ring key; `08 §4`), payload
JSON `{ jti, kind, integration_id, subject_id, data, exp }`. Kinds:
`approval.decide {decision}`, `approval.batch_decide {batch_id, agent_id,
decision}`, `action_approval.decide {decision}`, `reach.decide {decision}`,
`binding.choose {space_ref, allowed_agent_ids}` (the chosen agent arrives as
`value`), `dm_default.choose {allowed_agent_ids}`, `turn.stop {turn_id}`,
`followup.send_now {turn_id}`, `agent_link.decide {side, decision}`.
TTL `ACTION_TOKEN_TTL`; **single use** (`action_token_uses`); integration
must match the event's. The connector never interprets tokens.

### 4.5 Capabilities and fallbacks

| Capability | Fallback when absent |
|------------|----------------------|
| `threads` | `thread_ref = ""`: a conversation spans the whole DM or space |
| `edits` | settle cards by posting a new short "settled" message |
| `buttons` / `menus` / `text_input` | card shows a dashboard link, plus `approve <code>` / `deny <code>` commands when `commands` exists (code = first 8 base32 chars of the approval id) |
| `reactions` | seen-receipt becomes a "Working…" placeholder post, deleted (or edited to the answer) when done |
| `ephemeral` | DM the person, naming the space it concerns by its label ("In #general: …", "In your group message with Ann, Bob: …"; FIX-128, D-194): never by an internal id, and with no prefix at all when the notice goes to the DM it is about |
| `native_status` | placeholder post as above |
| `presence_dedicated` / `presence_shared` | only the other mode is offered |
| `user_tokens` | read-as-user / send-as-user features hidden (phase 0 implements none) |
| `email_verified` | linking always uses the sign-in link (phase 1, RFE-001: also whenever the integration's `verified_email_autolink` is `false`, which is the default; the platform email then only pre-fills the flow) |
| `files` | the core enforces min(core caps, platform caps) |
| `commands` | settings only in the dashboard |
| `history` | no mid-thread context; `read_conversation` unavailable |

## 5. Core behaviour

### 5.1 Inbox processing (`message.posted`)

Claimed from `channel_inbox` (lease `INBOX_LEASE`, attempts with backoff,
idempotent). Steps:

1. **Integration**: must be `active` and belong to the posting connector.
2. **Echo guard**: author is one of this integration's bots → ignore.
2a. (phase 2, RFE-006) **Frozen**: if the resolved agent is effectively frozen
   (`04 §Freeze`), no turn is created and no sandbox is started; reply once per
   (space/thread, person) per `FREEZE_REPLY_INTERVAL` with the platform-voice
   line "<Agent> is paused by an administrator." and stop. (The same check applies
   after step 3 resolves the agent; shown here for the reader.)
3. **Target agent**:
   - addressed to a **dedicated** bot (DM to it, or it is mentioned) → its agent;
   - **shared** bot in a DM: an existing thread link for `(space, thread_ref)`
     → that agent; else the author's DM default; else, if the author can reach
     exactly one agent with a shared presence here → it; else post an agent
     picker card (`dm_default.choose`) and stop;
   - **shared** bot in a channel: the active space binding → its agent; none →
     if the bot was mentioned, reply that the space is not set up and name who
     can set it up (owners of workspaces with shared presences on this
     integration); stop.
4. **Speaker**: user link → Aurora user. No link:
   - Phase 0: when the event reports a platform-verified email equal to a
     verified Aurora user's email → create the link (`verified_email`).
   - **From phase 1 (RFE-001) this is opt-in and off by default.** Auto-linking
     is attempted only when **all** hold: the integration's
     `config.verified_email_autolink` is `true` (set by an org owner/admin,
     §5.9), the integration's effective capabilities include `email_verified`,
     the event carries `author.email_verified = true`, and the email equals a
     *verified* Aurora user's email who is an **active member of the
     integration's org**. Then create the link (`method = verified_email`),
     audit `user_link.create` (`via: verified_email`; in the security-critical audit
     set of `04 §Audit` from phase 2, RFE-008) and continue.
   - Otherwise (the default) create a `link_requests` row — storing the
     platform email, if the event had one, as `platform_email_hint` — and send
     the person an ephemeral (or DM) message with a sign-in link; stop. The hint
     only **pre-fills** the sign-in (`login_hint` to the OIDC provider) and the
     confirm page; it never authorizes anything and need not match the Aurora
     account the person signs in with.
   Bot authors: allowed only through an approved `person` reach grant for the
   bot, metered by the app-turn cap.
5. **Authorization**: `can_use(user, agent workspace)` or an approved reach
   grant (person = author, or space = this space; (FIX-111) an approval may carry
   `expires_in`: past its `expires_at` the grant is no longer effective, the 60 s sweep
   marks it `expired`, and the next message reopens it as `pending`). Otherwise send a polite
   refusal **before anything reaches the agent**, create/refresh a `pending`
   reach grant and notify the agent's workspace owners (dashboard + a
   `reach.decide` card DM to owners linked on this integration). Stop.
6. **Listen gate** (channels and group DMs): proceed only if the bot was
   mentioned, or `listen = all`, or the thread is already linked to this agent
   and `followups_need_mention = false`. Otherwise ignore silently.
7. **Thread mapping**: DM → `direct` thread link → the agent's direct
   conversation with this person (one conversation, several doors); channel →
   `group` thread link keyed by the thread root (a top-level message roots a
   new thread) → its own `group` conversation.
8. **Context**: mentioned mid-thread in a newly linked thread and `history`
   available → fetch the last 20 messages (correlated `history.read`) and
   include them in the turn text as a labelled, sanitized "earlier messages
   (untrusted)" block.
9. **Files**: `file.fetch` each file (correlated), creating `bound`
   attachments or `failed` ones with a reason (phase 2, RFE-015: `quota_exceeded`
   when the org's attachment quota would be exceeded; the person is told in
   platform voice and the turn still runs).
10. **Turn**: create the turn (door `thread`, `source_event_id = event_id`; (phase 3, RFE-018) event ids starting with the reserved prefix `api:` were already rejected at ingest, `§POST /events`,
    speaker). A turn already created by a crashed earlier attempt is found by
    `source_event_id` and not duplicated.
11. **Receipt**: `reaction.add` (neutral `eyes`) or `status.set` or placeholder.

Other events: `bot.added` → binding flow (5.3); `bot.removed` → deactivate the
space's binding, set space reach grants `left`; `action.invoked` → action
router (5.5); `command.invoked` → command router (5.4); `presence.removed` →
presence `disabled`; `message.edited`/`deleted`/`reaction.added` → recorded,
no behaviour in phase 0.

### 5.2 Mirror: what gets posted when

- **Answers go back through the door the question came in.** When a turn with
  door `thread` finishes, its durable `text` events are posted once, complete
  (no streaming edits) to that thread link (`dedupe_key =
  answer:<turn_id>`); attachments sent with `send_file` follow as files. The
  receipt is removed / placeholder resolved.
- **Doorless turns** (system follow-ups, watch fires, schedule and peer-task
  reports, action-approval outcomes) in a conversation that has thread links
  are posted to its most recently used thread link.
- **Web- and API-originated turns are not mirrored** into platforms.
- Failed turns post a short platform-voice failure line with the error code's
  copy and a dashboard link.
- **Shared-bot attribution**: posts made by a shared bot start with a context
  line naming the agent ("Research · via Daneel") on the first post in each
  thread and on every proactive message. (FIX-128, D-194) The line exists to say
  which agent speaks when several can answer through one bot, so it is **left out
  of a 1:1 DM with the bot** (a `direct` conversation, where one agent answers)
  and kept in shared spaces. The bot's name in the line is the display name
  Mattermost shows for the bot (the bot record's `display_name`, refreshed whenever
  the integration is validated), never fewer than two characters: a shorter or
  missing value falls back to the bot's username.

### 5.3 Space bindings (shared bot)

On `bot.added`: if the adding person is linked and is `can_own` of ≥1 workspace
with agents that have a shared presence on this integration, DM them a
`binding.choose` select of those agents; the choice creates the binding (listen
`mentions`) and the bot announces itself in the space. Otherwise the bot posts
that it is not set up and who can set it up. The `bind <handle>` command does
the same from inside the space. Bindings can be edited/removed in the
dashboard by workspace owners. Removing the bot deactivates the binding;
re-adding needs an owner again.

### 5.4 Commands (platform-neutral)

Connectors map their command syntax (slash command, `!cmd`, or a mention
prefix) onto these names: `help`, `agents` (agents this person can reach here),
`use <handle>` (set DM default), `link`, `unlink`, `bind <handle>`, `unbind`,
`approve <code>`, `deny <code>`. Replies are ephemeral where possible.

### 5.5 Clicker authorization (invariant)

For every `action.invoked`: verify token signature, expiry, single use and
integration; resolve the clicker through the user link (unlinked → link prompt,
no action); apply the access law for the subject's workspace (`can_use` for
approvals, action approvals, reach decisions, stop/send-now — phase 2, RFE-012:
`can_approve` for approvals, action approvals and agent-link decisions, `can_use`
for stop/send-now; `can_own` for
bindings and reach; phase 1, RFE-009: for subjects that name a `direct`
conversation, additionally `can_see_conversation`, so only that person or an
owner can act); then act. **The platform never authorizes.** The card is
then settled (`message.update`, or a new message without `edits`).

### 5.6 Approval cards

On `aurora_approval created` for an approval naming a conversation with thread
links: post a card into the most recently used thread link (`dedupe_key =
approval:<id>`, `channel_cards` row): header "Approval needed", summary title,
context (agent, rule, expiry), buttons Approve/Deny (and
Approve all / Deny all for batches with total > 1); (phase 2, RFE-012) the context
line says "needs an approver", and a click or `approve <code>` by a linked user
who is not `can_approve` is refused with an ephemeral "Only approvers can decide
this" and changes nothing (the card stays pending). On decision/expiry the card
is settled with who decided and how. Action approvals, reach requests and agent
link requests use the same pattern. (phase 5 (D-138, moved from phase 4), RFE-011b) A card for a **parked** approval says
"The agent has stopped waiting and will resume when this is approved (valid for <N> min after
approval)"; it settles on approval ("Approved by X — <agent> will be told"), denial, deadline
expiry and, if the grant is never used, "approval window ended" (an edit of the settled card, no
new post). The reminder of `07 §Park and resume` is not a card (e-mail and push only).
Clicker authorization is unchanged (`§5.5`).

**What a card shows (D-192, owner decision Q-R5-9).** A card in a **shared**
space (a channel or group message: every member sees it, including people who
cannot use the agent) carries the title, the agent, the rule and the expiry
only. The request's details (summary fields: method, host, path, body keys)
are on the dashboard page the card links to, and only a `direct` conversation
(one person and the agent) carries them on the card itself. Approvers who want
them in chat can ask for the approval by DM (`approve <code>` answers privately).

### 5.7 Presence lifecycle

- **Attach** (dashboard, `can_use`): choose integration and mode (offered per
  capabilities and integration config). `shared` → row `active` immediately
  (requires the integration's shared bot). `dedicated` → `pending_setup` and a
  `presence.provision` op; result stores bot identity and credentials →
  `active`. Platforms without provisioning use `presence_setup` fields (manual
  token paste) validated by `credentials.validate`.
- **Detach / agent delete** → `presence.deprovision` op (best effort), row
  deleted. (phase 3, RFE-024) The op is **self-contained** and enqueued in the same
  transaction as the delete, before the cascade (`§4.2`, *Self-contained
  deprovision operations*).
- **Removal detection**: `presence.removed` events or `credential_dead`
  results → `disabled`; messaging tools explain; dashboard offers Re-attach or
  Detach.
- Renames and avatar changes enqueue `presence.rename` / `presence.set_icon`.
- Presence changes bump the agent's home sync generation ("where you talk"
  fragment and tool availability).

### 5.8 Integration setup

Org owners/admins: choose a connector → form from `integration_setup` → `POST
/v1/orgs/{org}/channel-integrations` stores the credential (`seal`), status
`validating`, and runs a correlated `credentials.validate`; success sets
`tenant_ref`, label, shared bot and `active`; a `tenant_ref` already bound to
another org → 409 `tenant_already_bound`. Credential rotation by connectors goes
through `POST /v1/connector/credentials` (CAS on `credentials_version`).

### 5.9 User linking

Phase 0: verified-email auto-link (5.1 step 4) or the sign-in link. **From
phase 1 (RFE-001) the sign-in link is the default and only automatic path**:

- **Sign-in link** (always available): `link_requests` token →
  `<external>/link/channel?token=aui_…` dashboard page → the person signs in
  (OIDC, `04`), the page shows the integration, the platform account's
  handle/display name and the platform email hint if any, and the person
  **confirms explicitly** → `POST /v1/channel-links/confirm` (session-only)
  creates the user link (`method = sign_in_link`). (FIX-112) The page learns
  the chat account from `GET /v1/channel-links/preview?token=` (session-only;
  same member rule as confirming, 410 for an unknown, used or expired token),
  the confirm request repeats the platform user reference shown and is refused
  (422 `link_reference_mismatch`, nothing consumed) if it does not match the
  token, and the token leaves the address bar at once (kept in `sessionStorage`
  across the sign-in round trip; the login return URL never carries it). An
  unexpired, unused link request for the same (integration, platform user) made
  within the last 10 minutes is reused: **a prompt is sent at most once per 10
  minutes per person** (no new row, no new message), whether the trigger was a
  message, a command or a button; a refusal notice ("you can't talk to X yet")
  is likewise sent at most once per person and agent per 10 minutes. The confirming user must be
  an active member of the integration's org (else 403 `not_org_member`; the
  link-request is not consumed). The token is single use and expires after 30
  min.
- **Verified-email auto-link** (opt-in): the org-admin setting
  `channel_integrations.config.verified_email_autolink` (boolean, default
  `false`). Setting it `true` requires the `email_verified` capability for the
  integration (else 422 `autolink_unsupported`), is audited
  (`channel_integration.autolink_enable` / `.autolink_disable`,
  security-critical set from phase 2) and the dashboard states the consequence: *"This extends
  trust to the connector: it can create links, and therefore speak and approve
  as any of your members whose verified email it asserts."* Disabling it does
  not remove existing links; org admins may delete links
  (`DELETE /v1/user-links/{id}`), and the dashboard lists each link's
  `method` so autolinked ones can be reviewed.
- Org admins may create links manually (`method = admin`). A platform user
  links to at most one Aurora user per integration; people manage their links
  under Account.

### 5.10 Messaging tools

Available to an agent only while it has an `active` presence.

- **Platform resolution** (Proposal 02 §10.1): the current turn's thread-link
  integration; else the origin conversation's; else the agent's only active
  presence; else error listing platforms — the agent passes `platform`
  (connector id) explicitly. Recipients and anchors are always
  platform-qualified, so a pick on one platform is never sent to another.
- `find_recipient {query, platform?}`: correlated `user.lookup` /
  `spaces.list`; candidates filtered to those the agent may reach (linked users
  with `can_use` on the workspace or approved reach; spaces with an active
  binding for this agent or approved space reach); results pinned as **mention
  anchors** in the conversation.
- `send_message {to, text, files?, platform?}`: `to` is an anchor alias or a
  recipient id from `find_recipient`. Reach is re-checked at send. Messages to
  the current conversation's own thread are always allowed. Otherwise the
  agent's `proactive_messaging`: `off` → refused; `ask` → **action approval**
  (`07`) with a platform-composed summary; `allow` → sent. Model text is never
  allowed to emit broadcast mentions.
- `read_conversation {target?, limit?}`: requires `history`; current thread by
  default, or a space bound to this agent; returns sanitized messages labelled
  untrusted.
- `list_my_messages`, `edit_my_message`, `delete_my_message`: only messages the
  agent posted through these tools in the same conversation within 24 h.

## 6. Invariants (enforced by the core)

1. **The core authorizes; the platform never does** — speakers, reach,
   bindings, clicks (5.1, 5.5). (phase 1, RFE-001) A connector-asserted email
   mints a user link only under the explicit, audited per-integration opt-in
   (5.9); by default every link is confirmed by the signed-in person.
2. **No unauthorized input reaches an agent**; refusals are sent as operations.
3. **At-least-once in, at-most-once out**: inbox dedupe by event id; claimed
   ops with markers.
4. **Untrusted text stays data**: the core sanitizes; connectors escape for
   their markup and defuse broadcasts.
5. **Connectors never see Aurora credentials** (no API keys, no user sessions);
   they receive only their own platform credentials and opaque action tokens,
   and cannot read anything outside integrations of their own connector id.

## 7. The fake connector and conformance suite

- `connectors/fake`: a complete connector over a simulated in-memory platform
  (users with configurable verified emails, spaces, threads, bots, files,
  failure injection: dead credentials, rate limits, lost results). A test
  control API (`/fake/*`, test profile only) posts messages as users, clicks
  buttons, adds/removes the bot and reads what was posted.
- `tests/conformance/connector`: a **fake core** plus scripted scenarios run
  against any connector: register/approve, DM round-trip, channel mention then
  thread follow-up, bot added → binding, unlinked user → link flow, file in and
  out, approval card → click → settle, credential dies → `presence.removed` or
  `credential_dead`, redelivered event processed once, re-claimed post not
  duplicated, broadcast mention defused, capability fallbacks.
- The phase 0 end-to-end suite runs the **real core** with the fake connector
  and the fake harness.

## 8. Connector SDK (`aurora-connector-sdk`)

Provides the contract client, register/config/ops loops with backoff, lease
handling, event batching with ack-after-accept, and a trait:

```rust
#[async_trait]
pub trait Connector: Send + Sync + 'static {
    fn manifest(&self) -> ConnectorManifest;
    /// Called when this replica gains ownership; returns a handle closed on loss of ownership.
    async fn open_integration(&self, integration: Integration, presences: Vec<Presence>, sink: EventSink) -> Result<IntegrationHandle>;
    async fn update_integration(&self, handle: &IntegrationHandle, integration: Integration, presences: Vec<Presence>) -> Result<()>;
    async fn execute(&self, op: Operation) -> OpResult; // neutral result or neutral error
    fn router(&self) -> Option<axum::Router>;           // the connector's public platform callbacks
}
```
