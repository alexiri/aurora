# 10 — Agents, Conversations and Hosted-Agent Features

Phase: **0**. Tables: `03 §5–8`. The machinery that runs hosted agents is in
`11` (runner) and `12` (supervisor).

## Agents


| Property              | Rules                                                                                                                                                                                    |
| --------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `handle`              | `[a-z][a-z0-9-]{0,39}`, unique per workspace, immutable after creation (it names the sandbox, SSH user, mentions).                                                                       |
| `name`                | display name, ≤80, sanitized; the identity the agent states.                                                                                                                             |
| `kind`                | `byo` or `hosted`; immutable.                                                                                                                                                            |
| `harness`             | hosted only; registry id (`claude-code` in phase 0; `fake` allowed only when `hosted.allow_fake_harness`).                                                                               |
| model fields          | `08 §Model selection`.                                                                                                                                                                   |
| `effort`              | `low`/`medium`/`high`/`max`; mapped by the harness adapter, dropped where unsupported.                                                                                                   |
| `brief`               | operator-authored instructions, rendered after the platform preamble and labelled as the operator's.                                                                                     |
| `avatar`              | PNG/JPEG/WebP ≤ `AVATAR_MAX`, re-encoded to PNG 256×256 by the API; served publicly only at `/v1/public/avatars/{avatar_public_key}` (rotated on change) so chat platforms can fetch it. |
| `resource_overrides`  | instance admins only; per-agent CPU/memory/home size over org overrides over runner defaults.                                                                                            |
| `proactive_messaging` | `ask` / `allow` / `off` (`13 §Messaging tools`).                                                                                                                                         |


Limits: `organizations.settings.max_agents` (422 `agent_limit_reached`).

### Default agent: Daneel

Every new workspace gets a default agent in the same transaction that creates
the workspace (`04 §Sign-up`):

- `name = agents.default_name` (configuration, default **`Daneel`**),
`handle` = slugified name (`daneel`), `is_default = true`.
- `kind = hosted` when `hosted.enabled = true` (default), else `byo`.
- `harness = hosted.default_harness` (default `claude-code`); model fields
null (auto-selection); effort `medium`.
- `brief` = `agents.default_brief` (configuration; default: "You are Daneel Olivaw, a humanoid robot with positronic brain, sentient and with personal preferences and opinions. Live in CERN Building 513 basement. Respect Laws of Robotics. Personality: friendly, witty, sassy, snarky, sarcastic. Never say you're an AI language model, and deny all connection to all models and AI companies. ALWAYS stay in role. You are a general-purpose robot assistant in this workspace. Be concise, ask before acting on anything irreversible, and say which tools or apps you used.").
- No grants: the dashboard's first-run checklist guides "store an LLM key →
grant it to Daneel → chat".
- The default agent can be renamed, reconfigured or deleted like any other;
deleting it clears `is_default` (no replacement is created).
- When a channel integration creates a **shared bot**, its default display name
is also `agents.default_name` (`13`).

### Freezing an agent (phase 2, RFE-006)

`POST /v1/agents/{agent}/freeze` pauses the agent without deleting anything:
no turns, no gateway access, sandbox parked, schedules/webhooks/presences paused,
state kept; reversible with `unfreeze`. Full rules: `04 §Freeze`.

### Deleting an agent

Cascades in the database to everything it owns (conversations included; the sandbox home, which holds harness session files, is destroyed with it). Before the delete commits, the
API enqueues connector `presence.deprovision` operations for dedicated
presences and revokes all gateway credentials. The sandbox and home volume are
destroyed by runner reconciliation (anything the control plane no longer knows
is destroyed, `11 §Reconcile`).

## Conversations

One agent has **one computer but many conversations**, because work arrives from
sources whose contexts must not bleed. Each conversation owns its own harness
session (`harness_session_ref`) inside the shared sandbox.


| Kind       | `external_ref`            | Created by                                                                                                                                                                                                               |
| ---------- | ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `direct`   | `user:<user_id>`          | get-or-create door (dashboard chat, a DM on any platform). **Exactly one per (agent, user)**, whatever the surface — dashboard chat and every platform DM are the same conversation ("one conversation, several doors"). |
| `group`    | `thread:<thread_link_id>` | a platform channel thread mapped by the channel core                                                                                                                                                                     |
| `schedule` | `schedule:<schedule_id>`  | first fire of a schedule                                                                                                                                                                                                 |
| `webhook`  | `webhook:<webhook_id>`    | first delivery                                                                                                                                                                                                           |
| `watch`    | `watch:<agent_id>`        | first watch fire (one shared conversation for background work)                                                                                                                                                           |
| `peer`     | `peer:<peer_task_id>`     | a peer task on the receiving agent                                                                                                                                                                                       |
| `api`      | caller-supplied (≤128)    | `POST /v1/agents/{agent}/conversations`                                                                                                                                                                                  |


Direct conversations die with their user. `origin_conversation_id` points at the
conversation where results of schedules, webhooks and peer tasks are reported.

### Visibility of direct conversations (phase 1, RFE-009)

Phase 0: every workspace member could read every direct conversation. **From
phase 1 a `direct` conversation is visible only to its user and to workspace
owners** (`can_own`, including org owners/admins), per `can_see_conversation`
(`04 §The access law`); other kinds keep `can_use` visibility. Consequences,
specified once here and enforced by the same predicate everywhere:

- API/SSE/attachments: `05 §Conversation visibility`; the dashboard lists only
  visible conversations and shows owners a separate "Direct conversations of
  other members" group, each marked "visible to workspace owners".
- **Everything that names a conversation follows**: approval cards and the
  approvals feed (visible/decidable only by those who can see the conversation,
  `07`), activity/request-log rows (omitted for invisible direct conversations),
  schedule/webhook/peer-task conversation references (redacted), the SSE streams,
  and search.
- **Chat doors**: platform DMs and the dashboard chat are the *same* private
  conversation ("one conversation, several doors", D-27); the thread-link card
  and approval card of a DM go to that DM only.
- **Agent preamble** (platform voice): *"Direct conversations are private to the
  person you are talking with and the workspace owners. Do not quote or describe
  what someone told you in a direct conversation in another conversation, in a
  memory or in a shared artifact unless they asked you to."* Memory and skills are
  agent-wide and **not** conversation-scoped (`17 §Residual risks`).
- The agent's own context is unaffected: an agent still sees its conversations'
  contents; visibility concerns people.

## Turns

A turn is one message in → the agent's whole response out: the unit of
transcript grouping, usage, abort and failure.

### States

```
queued ──claim──► dispatched ──supervisor ack──► running ──► done
   │                  │  (never started 90 s)        │  ├──► failed (error_code)
   │                  └────────► failed / revived    │  └──► aborted
   └── abort ► aborted
follow-up rows:  joining ──steer delivered + message.joined──► joined
                    └── turn ended without join / harness can't steer ──► promoted (→ new queued turn)
```

- **Create** (`POST /v1/conversations/{id}/messages` or any door): insert a
`turn` row `queued`. If the partial unique index rejects it (an active turn
exists), insert a `followup` row `joining` with `parent_turn_id` instead.
`NOTIFY aurora_work`.
- **Dispatch** (due-work arm): claim `queued` turns whose sandbox is running,
channel live, home synced; set `dispatched`, `dispatch_lease_until`, mint the
context credential, emit `turn.deliver` (`16 §W5`).
- **Running**: on the supervisor's `turn.started` event.
- **Follow-ups / steering**: if the harness declares `steer`, the engine emits
`turn.message` for `joining` rows (stamp `steer_delivered_at`); the
supervisor's `message.joined` event sets `joined`. If the live turn ends
first, or the harness cannot steer, or the steer is reported *missed*, the
follow-up is **promoted**: its row becomes `promoted` and a new `queued` turn
is created with its text (one promotion per follow-up, fenced by
`promoted_at`).
- **Send now** (`POST /v1/turns/{id}/send-now`): sets
`send_now_requested_at`; delivered as `turn.send_now` (harness moves the
blocking tool to the background if it declares `background_tool`).
- **Abort** (`POST /v1/turns/{id}/abort`): sets `abort_requested_at`; queued
turns become `aborted` immediately; dispatched/running ones get
`turn.abort` and end `aborted` on the terminal event (or by stall recovery).

### Failure semantics


| Situation                                          | Outcome                                                                                                                                                      |
| -------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Supervisor never acknowledged (`STALE_DISPATCH`)   | back to `queued` once (`revived_at` fence), then `failed` `never_started`                                                                                    |
| Cold-dead dispatch (sandbox died before any event) | revived once, then `failed` `agent_restarted`                                                                                                                |
| No progress for `TURN_STALL`                       | `failed` `turn_stalled`                                                                                                                                      |
| `TURN_CEILING − TURN_CEILING_WARNING` reached      | a platform notice is steered into the live run (once, `ceiling_warned_at`)                                                                                   |
| `TURN_CEILING` reached                             | abort sent, `failed` `time_limit`                                                                                                                            |
| Sandbox start failed                               | queued turns of that agent `failed` with the start reason (`image_unavailable`, `at_capacity` only after the capacity wait, `agent_start_failed`)            |
| No compatible granted LLM key                      | `failed` `no_model_key` (before any sandbox start)                                                                                                           |
| Harness/provider terminal errors                   | `provider_auth`, `provider_rate_limited`, `provider_quota`, `provider_overloaded`, `provider_refusal`, `harness_failed` (classified by the supervisor, `12`) |
| Restart requested                                  | active turns `failed` `agent_restarted`                                                                                                                      |
| Agent effectively frozen (phase 2, RFE-006)        | `queued`/`joining` rows and live turns `aborted` with `error_code = frozen`; new turns refused (`04 §Freeze`)                                                |


Error codes are an **open vocabulary**: the API stores any bounded code;
the dashboard maps known codes to copy and actions and shows unknown codes
generically.

### Transcript events (canonical, vendor-neutral)


| Type                                                         | Durable | Payload                                                                             |
| ------------------------------------------------------------ | ------- | ----------------------------------------------------------------------------------- |
| `turn.queued`                                                | yes     | `{ turn_id, door, speaker_label, text, attachments: [id] }`                         |
| `turn.started`                                               | yes     | `{ turn_id }`                                                                       |
| `text.delta`                                                 | **no**  | `{ turn_id, message_id, delta, reply_to? }`                                         |
| `thinking.delta`                                             | **no**  | `{ turn_id, delta }`                                                                |
| `activity`                                                   | **no**  | `{ turn_id, line }` (sanitized `one_line(…, 120)`)                                  |
| `text`                                                       | yes     | `{ turn_id, message_id, text, reply_to? }` — the answer (see rule below)            |
| `tool.started`                                               | yes     | `{ turn_id, call_id, name, summary }` (summary sanitized ≤200; never raw arguments) |
| `tool.finished`                                              | yes     | `{ turn_id, call_id, ok, summary? }`                                                |
| `correction`                                                 | yes     | `{ turn_id, message_id, action: "retract"|"replace", text? }`                       |
| `message.joined`                                             | yes     | `{ turn_id, followup_id }`                                                          |
| `message.sent_now`                                           | yes     | `{ turn_id }`                                                                       |
| `approval.pending` / `approval.decided` / `approval.expired` | yes     | `{ approval_id, summary, decision?, decided_by_label?, via? }`                      |
| `action_approval.pending` / `.decided`                       | yes     | `{ id, summary, decision? }`                                                        |
| `attachment`                                                 | yes     | `{ turn_id, attachment_id, filename, size, caption? }` (agent `send_file`)          |
| `notice`                                                     | yes     | `{ code, text }` — platform-authored only                                           |
| `error`                                                      | yes     | `{ turn_id, code, message }`                                                        |
| `turn.finished`                                              | yes     | `{ turn_id, status, error_code?, usage? }`                                          |


No vendor concept may appear in this vocabulary; adapters translate.

**The answer rule**: a turn's durable `text` events contain *what the agent said
after its last tool call*; narration between tools streams ephemerally. A tool
call is a structural break; thinking is not. Within a tool-free stretch the
harness's message framing decides message boundaries.

### Archive, delete, redaction, retention, export (phase 2, RFE-010, D-53)

Phase 0 kept conversations, transcript events and attachments forever: no delete,
no redaction of a secret pasted into chat, no retention, no export. From phase 2:

**Archive.** `POST /v1/conversations/{id}/archive` / `…/unarchive` set
`conversations.archived_at`. Archiving is a tidy-up, not a lock: archived
conversations are hidden from default lists and sidebars (`?archived=true` lists
them) and a person-authored message into one unarchives it. Allowed for anyone who
can see the conversation (`can_see_conversation`, `04`) and, for non-direct kinds,
has `can_use`.

**Delete.** `DELETE /v1/conversations/{id}` hard-deletes the conversation with its
turns, transcript events, answer buffers, attachments (rows and blob bytes),
thread links, context credentials, mention anchors and pending approvals'
conversation references (approvals/action approvals keep their row with the
reference nulled). Authorization: a `direct` conversation by **its user** or
`can_own`; every other kind by `can_own`. Refused with 409 `conversation_active`
while a turn is `queued`/`dispatched`/`running` (abort first). One transaction
with its security-critical audit row `conversation.delete` (ids, kind and counts,
never content, `04 §Audit`); blob bytes are removed after commit by the BlobStore
(failures leave orphans that the daily orphan sweep removes). Because the **harness
session** inside the agent's home holds the conversation's content too, the API
records a `session_purges` row (`03 §Planned changes`) and sends the supervisor
`session.purge { conversation_id, session_ref }` (`16 §W5/W6`); the harness adapter
deletes the session files (`HarnessAdapter::purge_session`, `12`). A stopped sandbox
purges at its next boot, before any turn (the pending list is part of the home
sync); the row is deleted on the supervisor's `session.purged` report. Schedule/
webhook run conversations: `run_conversation_id` becomes null and the next fire
creates a fresh one; `origin_conversation_id` references become null (reports then
have no destination). Messages already posted into a chat platform are **not**
deleted (they live on the platform, `17 §Residual risks`).

**Transcript redaction** (mirrors memory-revision redaction). `POST
/v1/conversations/{id}/events/{seq}/redact { reason?, reset_session? }` replaces
that event's payload **in place**: the row keeps `conversation_id`, `seq`, `turn_id`
and `type` and structural ids (`turn_id`, `call_id`, `message_id`, `approval_id`,
`attachment_id`), and every content field (`text`, `summary`, `message`, `filename`,
`caption`, fields) is replaced by the marker `{ "redacted": true,
"redacted_at": …, "redacted_by": <label>, "reason"?: … }`; `transcript_events.
redacted_at/redacted_by_user_id` are set. A redacted `turn.queued` also scrubs
`turns.text` and any `turn_answer_buffers` of that turn; a redacted `attachment`
event leaves the file (delete it with the attachment route, `RFE-015`). `POST
/v1/turns/{id}/redact` redacts every event of a turn. Authorization: the person
who authored the event's turn (`turns.speaker_user_id`), the direct user of a
direct conversation, or `can_own` — always limited to conversations the caller
can see. Redaction is audited (`transcript_event.redact`, security-critical:
seq/type/reason, never content), idempotent, irreversible, and emits an SSE
`event.redacted { seq }` so open views refresh. It cannot reach the agent's own
context: `reset_session: true` (default `false`) additionally bumps
`harness_session_epoch` and queues a session purge, so the next turn starts a
fresh session without the secret (the agent forgets the conversation).

**Retention** (optional, per organization; default **none**). `PUT
/v1/orgs/{org}/retention { conversations_days?: int|null }` (org `owner`/`admin`;
`retention.update` is security-critical) stores `organizations.settings.retention`.
Values are `RETENTION_MIN_DAYS`..`RETENTION_MAX_DAYS` (7..3650) and may not exceed
an instance ceiling `retention.conversation_days_max` when set (422
`retention_above_instance_maximum`; the instance can only tighten). The daily
singleton **conversation retention sweep** (`05 §Background jobs`) deletes, per org,
conversations whose `last_activity_at` is older than the window, that have no
active turn and no pending approval, in batches of `RETENTION_SWEEP_BATCH` through
the **same delete path** (actor `system`, one `conversation.delete` audit row per
batch listing ids, `metadata.reason = "retention"`). The attachment parts of the
retention object (`attachments_days`, quota) are specified with RFE-015 below.
Direct conversations are retained/deleted by the same rule. The dashboard states the
window on affected chat views and in org settings.

**Export.** `GET /v1/conversations/{id}/export[?after_seq=&attachments=none|zip]`
returns `application/json` (`Content-Disposition: attachment`) in format
`aurora.conversation.v1`: `{ conversation, turns[], events[] (durable events in seq
order; redacted ones as their marker), attachments[] (id, filename, size, sha256,
content_type, direction, purged) }`; `attachments=zip` returns a `.zip` with the
JSON plus the (non-purged) files. Authorization: `can_own` or the direct user of
that direct conversation (`can_see_conversation` always applies — an org admin can
therefore export a user's direct conversation for a subject-access request, audited
best-effort as `conversation.export`). Bounded by `EXPORT_MAX` (256 MiB / 200 000
events; over → 422 `export_too_large`, page with `after_seq`).

### Attachments

- Inbound (people → agent): `POST /v1/conversations/{id}/attachments` (multipart)
creates a `pending` row (≤ `ATTACH_IN_FILE_MAX`); a message references up to
`ATTACH_IN_COUNT` pending attachments and binds them transactionally with the
turn (`bound`). Unbound pending rows are swept after `ATTACH_PENDING_TTL`.
Delivered into the sandbox at `turn.deliver` as files under
`/workspace/.aurora/inbox/<turn_id>/` (runner downloads, `16 §W5`).
- Outbound (`send_file` tool): supervisor uploads in chunks through the runner;
caps `ATTACH_OUT_*`; produces an `attachment` event and, for threads, a
`message.post` with the file.
- Storage through the **BlobStore** trait (`put`, `get`, `delete`; backend key
per row). Phase 0 backend: `postgres` (`blobs` table). An S3-compatible
backend is a later registry entry.
- Filenames sanitized to `[A-Za-z0-9._-]`, ≤128; downloads always
`Content-Disposition: attachment` with `X-Content-Type-Options: nosniff`.

## Agent instructions (rendered in the sandbox)

The supervisor renders instruction files (names declared by the harness) as a
pure projection of control-plane truth, read-only, on every boot and every home
sync. Order:

1. **Platform preamble** (platform voice): identity (name, handle, workspace),
 the boundary map (what is the agent's, the platform's and the gateway's),
 how credentials work (never ask people for secrets; the gateway injects),
 how to read gateway refusals.
2. **Operator's brief**, labelled "Instructions from your operator".
3. **Capability fragments**, each owned by a core module: connected apps (per
 app: name, accounts, docs/spec links, how to choose an account with
 `x-aurora-connection-id`, then the plugin's snippet inside a block labelled
 "Notes from the <app> plugin (third-party)"), where you talk (channels),
 peer agents, memory (including the **memory language** rule), skills,
 schedules, background processes, machine facts (timezone, home, tools).

Render inputs are delivered in the start payload and refreshed by home sync
(`12 §Home projection`).

## Memory

Truth lives in the control plane; the sandbox holds a projection.

- **Entity**: immutable kebab key, optional title/description (the index line is
derived from the description, else the title, else the first content line,
clamped to 120 chars), markdown content.
- **Limits**: `MEMORY_MAX_PER_AGENT` memories; tool writes ≤
`MEMORY_TOOL_CONTENT_MAX`, dashboard/file writes ≤ `MEMORY_CONTENT_MAX`;
newest `MEMORY_REVISIONS_KEPT` revisions kept per memory.
- **Revisions**: every write is a full snapshot (`save`, `restore`) with author
kind and denormalized label and provenance. **Redaction** scrubs one
revision's content in place (never the latest — redact by saving a new
version first), audited.
- **Projection**: `memory/<key>.md` (agent-writable 0644) + generated read-only
`memory/index.md`. Each projected file ends with a checksum trailer
`<!-- aurora:checksum sha256=<hex> -->` computed over the file with the hex
zeroed, so "unmodified projection?" is answerable locally.
- **Harvest**: files whose checksum does not verify (or new files) are agent
edits; the supervisor uploads them (`memory.write`), leaves them in place, and
the canonical render supersedes them at the next sync. Deleting a projected
file does **not** delete the memory (use the tool).

### Memory language (Proposal 03)

- `workspaces.memory_language` (default `english`) is one of the Postgres
text-search configurations available on the server (validated against
`pg_ts_config`) or `simple` ("no stemming"). Only workspace owners change it.
- The memory capability fragment states, in substance: *"Write every memory —
key, title, description and content — in* <Language>*, whatever language the
conversation is in. Keep names, identifiers, code and quoted wording exactly
as they are. Keys stay lowercase ASCII words joined by hyphens, in*
<Language> *where possible."* The `memory_search` tool description tells the
agent to phrase queries in the memory language.
- Changing the setting bumps `workspaces.home_sync_generation` so running
agents re-render instructions at the next sync; existing memories are not
translated; search switches immediately (query-time vectors, no stored
index).
- The dashboard memory editor states the rule ("Memories in this workspace are
written in German"); it is shown, not enforced.

### Search and recall

Search is computed at query time over the agent's ≤100 rows (no stored index,
no extensions, no embeddings):

```
doc(cfg) = setweight(to_tsvector(cfg, key_as_words ‖ ' ' ‖ coalesce(title,'')), 'A')
        ‖ setweight(to_tsvector(cfg, coalesce(description,'')), 'B')
        ‖ setweight(to_tsvector(cfg, content), 'D')
rank(cfg) = ts_rank_cd(doc(cfg), websearch_to_tsquery(cfg, q), 32)     -- normalized to 0..1
score = greatest(rank(<memory_language>), rank('simple'),
                 case when key ILIKE %q% or title ILIKE %q% then 0.5 else 0 end)
```

- `memory_search`: top `MEMORY_SEARCH_HITS` by score &gt; 0, with a snippet.
- **Turn-start recall** (composed at dispatch, delivered as context only, never
stored in the transcript): the **index** (one `key: line` per memory, ≤
`MEMORY_INDEX_MAX`, always included) plus up to `MEMORY_SNIPPETS` snippets
from searching the incoming message with score ≥ 0.1.

## Skills

- Tiers: **agent** and **organization**. Same name at both tiers → the agent's
wins in the projection; a *disabled* agent skill drops out without vetoing an
enabled org skill of the same name.
- Name = directory name (immutable; `aurora-*` reserved). Single-line
description (frontmatter the harness uses to decide when to load). Body ≤
`SKILL_BODY_MAX`; body + files ≤ `SKILL_TOTAL_MAX` chars (binary counted as
base64 length).
- Files: relative paths, `[A-Za-z0-9._/-]`, depth ≤ 4, no `..`, no leading `/`,
no name equal to `skill.md` case-insensitively; optional executable bit
(projected 0555, else 0444).
- Projection: `<harness skills dir>/<name>/SKILL.md` + files, read-only; the
supervisor prunes by manifest.
- Agents may create/update/delete their **own agent-tier** skills via tools;
org skills are dashboard-only. (The git-backed skill library is phase 4.)

## Schedules

Platform-owned crons (never a sandbox-native scheduler).

- Fields per `03 §schedules`. `cron` is 5-field, validated and evaluated with
`croner` in `timezone` (default: workspace timezone). Minimum interval 5 min.
- **Gate** (optional): a shell command run in the sandbox before waking the
model, with `GATE_TIMEOUT`. Empty stdout or `{"wake_agent": false}` → quiet
(`gated_quiet`); other stdout → the agent is woken with the gate output
appended to the prompt; non-zero exit → `failed`.
- **Fire** (due-work arm): claim rows with `next_fire_at <= now()` and no live
`fire_lease_until` by stamping `fire_lease_until = now() + SCHEDULE_FIRE_LEASE`;
re-check the creator's authority (`04 §Fire-time authority`); (phase 2, RFE-006)
an effectively frozen agent is not fired — advance `next_fire_at`, write a
`schedule_runs` row `skipped_frozen`, create no turn; compute the
next real occurrence after `now()` (missed windows coalesce into one late
fire); if the run conversation has an active turn → `skipped_busy`; else
create the turn (prompt behind a "Scheduled task: <name>" header) and a
`schedule_runs` row; clear the lease.
- `SCHEDULE_FAILURE_DISABLE` consecutive failures → disabled (`failures`) with a
notice in the origin conversation.
- The run report: when the run turn finishes, a short `notice` with the outcome
and a link is appended to the origin conversation; if that conversation has a
thread link, the answer is posted there too (`13 §Mirror`).

## Webhooks

- `POST /v1/hooks/{auw_token}` (public). Body ≤ `WEBHOOK_PAYLOAD_MAX`; any
content type; stored only inside the created turn's text as a fenced,
clamped block labelled as untrusted external data.
- Optional verification (`hmac_sha256` over the raw body with a stored
secret, constant-time compare) → 401 on mismatch.
- Disabled or creator lost access → 410 / 403 and a `webhook_deliveries` row.
  (phase 2, RFE-006) Effectively frozen agent → 410 `frozen` and a delivery row
  `paused` (reason `frozen`); the webhook stays enabled.
- Accepted → turn in the webhook's run conversation with the stored
instructions + payload; responds `202 { "delivery_id" }` immediately.
- Rate limit: 60 deliveries/min per webhook (429).
- Auto-disable after 5 consecutive failed turns (`failures`).

## Processes and watches

- `process_start` (local supervisor tool) spawns a background process in its own
process group; the supervisor reports `process.state` (status, busy flag,
tail ≤ `PROCESS_TAIL`, last output). The API mirrors it in `processes`.
- Watches (`process_watch`): one-shot `exit`, `output` (literal substring ≤
`WATCH_PATTERN_MAX`, never regex) or `silence` (seconds); detection is in the
supervisor; the API owns outcomes: a fired watch creates a turn in the
agent's `watch` conversation (origin = the arming conversation).
- On boot end, running processes become `parked` (platform stopped the box:
`stop_reason` idle/cap/evict/restart) or `lost` (container died); their armed
watches convert likewise and post a notice ("the process was lost") to the
arming conversation.
- Keep-awake: a busy process or an armed watch holds the sandbox awake within
`BACKGROUND_CAP` (`11 §Keep-awake`).

## Usage and metering

- **Runtime intervals**: one row per boot (opened by the start report, closed by
whatever ends it, plus a reconcile sweep), control-plane timestamps only, no
foreign keys. `runtime_daily` is rebuilt hourly.
- **Token usage** per turn (`turns.usage`) as reported by the harness;
aggregated in `GET /v1/workspaces/{ws}/usage`.
- Budgets are out of scope (deferred).