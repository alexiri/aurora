# 20 — Mattermost Connector

Phase: **1**. Binary/image: `aurora-connector-mattermost` (Rust, built on
`aurora-connector-sdk`). Implements connector contract v1 (`13`). Ports the
behaviour and lessons of the `onecli-mattermost-bridge` prototype (catch-up,
button signing as defence in depth, broadcast defusing, channel-setup UX);
none of the bridge's API-key custody, turn tailing or approval matching is
needed because the core does that.

**Supported Mattermost version: 11.7** (the production version; MM-003). The conformance
suite and the live scenario run against `mattermost-preview` 11.7.x pinned by digest; 12.x
is not a target (MM-001 findings were identical on 12.0.2).

## Manifest

- `package.id = "mattermost"`, contract 1.
- Capabilities: `threads`, `edits`, `reactions`, `buttons`, `menus`, `files`,
  `commands`, `history`, `presence_shared`, `presence_dedicated`,
  `email_verified`, `ephemeral` = true; `text_input`, `native_status`,
  `user_tokens` = false. Per-integration narrowing at `credentials.validate`:
  without a system-admin token → no `ephemeral`, no `presence_dedicated`;
  when the server hides email addresses from the bot → no `email_verified`.
  (phase 1, RFE-001) Even when `email_verified` is offered, the core uses it only
  if an org admin enabled `verified_email_autolink` on the integration (default
  off); otherwise the connector's `author.email` only pre-fills the sign-in link
  flow, and the setup guide says so.
- `public_callbacks: ["actions", "commands"]`.
- `limits`: `message_max_chars` 16 383 (server `MaxPostSize` read at validate),
  `writes_per_space_per_minute` 30.

## Integration setup fields

| Field | Type | Notes |
|-------|------|-------|
| `server_url` | text | https URL of the Mattermost server |
| `team` | text | team name or id |
| `bot_token` | secret | access token of a bot account (shared mode; required) |
| `admin_token` | secret, optional | system-admin personal access token: enables dedicated presences, ephemeral posts, slash-command registration |
| `shared_bot_display_name` | text, default `agents.default_name` (`Daneel`) | applied to the bot profile when the bot token allows |

`verified_email_autolink` (boolean, default `false`) is **not** a connector setup
field: it is a core-owned integration setting stored in
`channel_integrations.config` and edited by org admins (`13 §5.9`).

`credentials.validate`: `GET /api/v4/users/me` (bot) → bot identity;
`GET /api/v4/teams/name/{team}` (or by id) → `tenant_ref =
"<server host>/<team_id>"`, label "<Team> on <host>"; bot must be a team member
(else `invalid` with guidance); read `config/client` for `MaxFileSize`,
`MaxPostSize`, `ShowEmailAddress`; if `admin_token` works and is system admin,
register (or update) a team slash command `/aurora` pointing to
`<connector public URL>/mm/commands` and store its verification token in the
integration config.

## Transport

- One WebSocket (`/api/v4/websocket`) per bot token owned by this replica
  (shared bot + every dedicated bot of the integration), authenticated with the
  token; reconnect with jittered backoff; `seq` gaps trigger catch-up.
- **Catch-up**: after (re)connect, for DM channels of each bot and channels
  with an active binding, fetch posts since `now − catchup_window` (default
  10 min, max 24 h) via `GET /api/v4/channels/{id}/posts?since=` and emit them
  as events; core deduplication makes replays harmless.
- Events mapped:

| Mattermost | Neutral event | Notes |
|------------|---------------|-------|
| `posted` | `message.posted` | `event_id = "posted:<post_id>"`; ignore system posts (`type` set) and posts by any bot of this integration |
| `post_edited` / `post_deleted` | `message.edited` / `message.deleted` | |
| `user_added` (user = a bot) / `user_removed` | `bot.added` / `bot.removed` | `actor_ref` from the event when present |
| `reaction_added` | `reaction.added` | |
| `user_updated` with `delete_at > 0` for a bot, or 401 on the socket | `presence.removed` | |

- `message.posted` fields: space kind from channel type (`D` dm, `G`
  group_dm, `O` channel, `P` private); `thread_ref = root_id`, none for a
  top-level post (S-15);
  `bot_mentioned` when the text mentions `@<bot username>` or the channel is a
  DM with the bot; author email from `GET /api/v4/users/{id}` (cached 5 min per
  replica); `email_verified` only when the API states it for that user, which
  Mattermost 11.7 does not for other users, so it is normally absent (S-15); files from `file_ids`
  (metadata via `/api/v4/files/{id}/info`).

## Rendering

- Neutral markdown → Mattermost markdown (largely identical). Mentions:
  `<@user:ID>` → `@username` (lookup), `<#space:ID>` → `~channel-name`,
  `<@here>` → `@here` (only from the core). **Defuse** `@channel`, `@all`,
  `@here` occurring in text by wrapping them in inline code.
- Posts carry `props.aurora_op = <op_marker>`; when `maybe_sent` is true, the
  connector searches the last 50 posts of the target thread/channel for that
  prop before posting.
- Shared-bot attribution: the core's context line ("Research · via Daneel")
  is rendered as the first line in italics.
- Cards → message attachments: `header` → `title`, `text` → `text`, `fields` →
  `fields` (short when value ≤ 40 chars), `context` → `footer`, `link` →
  markdown link in text, `buttons` → `actions` of type `button` (style
  `primary`/`danger`/`default`), `select` → action of type `select` with
  `options`. Each action's `integration` = `{ url: "<public>/mm/actions",
  context: { token: <core action token>, sig: HMAC(callback key, token) } }`.
- Updates: `PUT /api/v4/posts/{id}/patch`; deletes: `DELETE /api/v4/posts/{id}`.
- Ephemeral: `POST /api/v4/posts/ephemeral` (admin token) else the core's DM
  fallback applies (capability narrowed).
- Reactions: neutral names map 1:1 to Mattermost emoji names (`eyes`,
  `white_check_mark`, `x`, `hourglass_flowing_sand`); unknown → `eyes`.

## Public callbacks (connector-owned)

- `POST /mm/actions`: Mattermost interactive-message callbacks. Mattermost does
  not sign callbacks, so: (1) verify the connector HMAC in `context.sig`;
  (2) emit `action.invoked { token, clicker_ref: user_id, value:
  selected_option }`; the core verifies its own token and authorizes the
  clicker; (3) respond `200 {}` immediately (the card is updated by a later
  `message.update` op). Operators MUST restrict this port to the Mattermost
  server's addresses (network policy / firewall).
- `POST /mm/commands`: slash command; verify the per-command token; emit
  `command.invoked` with `name` = first word and `args`; respond with an
  ephemeral "working…" acknowledgement; replies come as ops.
- In DMs `!<command> args` and in channels `@bot <command> args` (for
  `help|agents|use|link|unlink|bind|unbind|approve|deny`) are also mapped to
  `command.invoked` when slash commands are not registered.

## Operations

| Op | Mattermost API |
|----|----------------|
| `message.post` | `POST /api/v4/posts` (files: `POST /api/v4/files` first; DM targets: `POST /api/v4/channels/direct`) |
| `message.update` / `delete` | `PUT …/posts/{id}/patch` / `DELETE …/posts/{id}` |
| `reaction.add` / `remove` | `POST /api/v4/reactions` / `DELETE /api/v4/users/{bot}/posts/{post}/reactions/{emoji}` |
| `file.fetch` | `GET /api/v4/files/{id}` (streamed, size-capped) → `PUT /v1/connector/files/{op_id}` |
| `user.lookup` | `GET /api/v4/users/{id}` or `POST /api/v4/users/search` (team-scoped) |
| `spaces.list` / `space.members` | `GET /api/v4/users/{bot}/teams/{team}/channels`, `POST /api/v4/channels/search` / `GET /api/v4/channels/{id}/members` |
| `history.read` | `GET /api/v4/posts/{root}/thread` or `GET /api/v4/channels/{id}/posts` |
| `presence.provision` (admin) | `POST /api/v4/bots` (`username = <handle>` or `<handle>-agent` on collision), add to team, `POST /api/v4/users/{id}/tokens`, set image from `avatar_url` |
| `presence.deprovision` | revoke tokens, `POST /api/v4/bots/{id}/disable` — (phase 3, RFE-024) driven entirely by the op's own payload and `credentials` (`platform_user_ref`, `tenant_ref`, `config.server_url`, the admin token from `credentials.admin_token`), never by looking up the integration or presence rows, which may already be deleted; 404 on the bot → `not_found` (`dropped`) |
| `presence.rename` / `set_icon` | `PUT /api/v4/bots/{id}`, `POST /api/v4/users/{id}/image` |
| `credentials.validate` | as above |

Error mapping: 401 → `credential_dead`; 403 → `forbidden`; 404 → `not_found`;
429 → `rate_limited` (`retry_after` from `X-Ratelimit-Reset`); 413 →
`too_large`; 5xx/timeouts → `unavailable`.

## Managed (dashboard)

(phase 4, P4-MCON) Mattermost is a built-in connector type of `aurora-connector-host`: an instance
administrator adds it under Admin → Connectors → Add, with no token and no deployment change. The
host runs it in-process with the settings below, and the callbacks are served at
`<externalUrl>/connectors/mattermost/mm/actions` and `…/mm/commands`. The manifest then also
declares `instance_setup`: `public_url` (default `<server.external_url>/connectors/mattermost`),
`callback_hmac_key` (a `generated_secret`), `catchup_window_secs` (600) and `token_check_secs`
(30). The Mattermost server URL and tokens stay per integration, as below. The manifest carries the
official Mattermost icon (Simple Icons, `plugins/builtin/ICONS.md`) in both paths. Design:
`13 §3.1`; operator steps: `operator/plugins-and-connectors.md`. The standalone binary and its
configuration below stay the self-hosted path.

## Configuration

`AURORA_API_URL`, `AURORA_CONNECTOR_TOKEN`, `connector.listen`
(default `0.0.0.0:4110`), `connector.public_url` (reachable by Mattermost),
`connector.callback_hmac_key` (generated at install), `connector.catchup_window_secs`.
Stateless; any number of replicas (integration ownership via the core).

## Tests

- Connector conformance suite (`13 §7`) against a Mattermost server container
  (team, bot accounts and users seeded by a setup script) in CI.
- Unit tests for rendering (cards, mentions, defusing), event mapping, error
  mapping, catch-up deduplication.
