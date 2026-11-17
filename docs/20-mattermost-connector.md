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
- Typing indicator (phase 5, FIX-158, D-319): `native_status` is declared. `status.set` starts one refresher per (sender, channel, thread) that calls `POST /api/v4/users/{user_id}/typing` `{channel_id, parent_id}` with the sender's (bot or presence) token every `TYPING_REFRESH_SECS` (4 s; clients drop it after ~5 s); `status.clear` stops it; it stops by itself after `TURN_CEILING_WARNING` and quietly on 403/404. Two sets are one refresher. A connector restart drops the refreshers (the next turn sets it again). Typing calls are not posts and do not count toward the write limits.
- Capabilities: `threads`, `edits`, `reactions`, `buttons`, `menus`, `files`,
  `commands`, `history`, `presence_shared`, `presence_dedicated`,
  `email_verified`, `ephemeral` = true; `text_input`, `native_status`,
  `user_tokens` = true (phase 5, P4-UTOK, D-281: per-person personal access tokens for `send_as_me`, `13 §5.11`; read-as-user stays hidden, Q-UTOK-1). Per-integration narrowing at `credentials.validate`:
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

(FIX-127, C-715) Labels and flags in the manifest: `server_url` is "Your Mattermost server";
`team` is "Team" and declares `choices: "from_validate"`; `public_url` (instance setup) is "Address
Mattermost uses to reach Aurora" with the help "Leave empty unless Mattermost reaches Aurora under
another name", and it, `catchup_window_secs` and `token_check_secs` are `advanced: true`.

`credentials.validate` with `mode: "probe"` (C-710): `GET /api/v4/users/me` (bot token) then
`GET /api/v4/users/me/teams` → `choices.team = [{ value: <team URL name>, label: <display name> }]` (create resolves a name or an
id; the dashboard builds the "Open a DM with @bot" link `https://<server>/<name>/messages/@<bot>`
from the value),
`tenant_label` "@<bot> on <host>"; `warnings` carries "no admin token: replies to individuals
arrive as DMs, and `/aurora` is not registered" when no `admin_token` is given, and the check of
the admin token when one is. Probe never registers the slash command, edits the bot profile or
reads `config/client`. A `team` that is set (name or id) is still resolved as below and its
failure is a probe error; an unset `team` is not.

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
  10 min, max 24 h) and emit them oldest first as events; core deduplication
  makes replays harmless. The read pages newest-first through
  `GET /api/v4/channels/{id}/posts?page=&per_page=200` (not `?since=`: Mattermost
  answers that with one capped batch and no way to continue, so a longer gap lost
  its oldest messages) until a page is short or reaches back past the window.
  The read is capped at 50 pages (10 000 posts) per channel; past the cap the
  oldest posts of the gap are not replayed and a warning names the channel.
- **Event ordering**: socket events of one channel are handled one after another
  in the order they arrived (a queue per channel; channels run in parallel).
  Handling an event awaits channel, user and file lookups, and without the queue a
  post deleted within that latency reached the core as "deleted" (an unknown
  message) followed by "posted" with the original text. Events that name no
  channel share one queue.
- **Platform URL** (`server_url` is supplied by the tenant): the connector connects
  only to public addresses. A literal loopback, private (RFC 1918, ULA),
  link-local (including the cloud metadata address), CGNAT, unspecified or
  multicast address, `localhost`, and a name that resolves only to such addresses
  are refused when the integration is validated and again when it is opened; the
  operator whose Mattermost lives on a private network sets
  `AURORA_MM_ALLOW_PRIVATE_NETWORKS=true`. What the target answers is never
  echoed into validation errors (they say "mattermost answered <status>"; the
  detail is in the connector's debug log). The REST client and the WebSocket both
  trust the PEM bundle named by `AURORA_TLS_EXTRA_CA_BUNDLE` (the file mounted as
  `tls.extra_ca_bundle`) in addition to their normal roots.
- **Lease**: a replica that cannot reach the core for longer than the `lease_secs`
  of its last configuration closes its integrations (sockets included) and opens
  them again when the core answers: it assumes another replica owns them now.
- A WebSocket authentication reply without an error code is a platform fault, not
  a revoked token (`presence.removed` follows an explicit 401 only); a reaction in
  another team's channel is ignored like a post is.
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

- Neutral markdown → Mattermost markdown per the conversion table below
  (`13 §4.6`). Mentions:
  `<@user:ID>` → `@username` (lookup), `<#space:ID>` → `~channel-name`,
  `<@here>` → `@here` (only from the core: platform voice, or a standalone
  `@here` in an agent's message body, `13 §4.3`). **Defuse** (phase 5, D-271)
  every ping that notifies more than one person — `@channel`, `@all`, `@here`
  (the core's `<@here>` aside) and **group mentions** — in every spelling the
  server reads as one (mentions table below), inside formatted text too, by
  wrapping the word in inline code; code spans, fenced blocks, indented code and
  HTML code are never altered and never notify. One user's `@username` passes.
  Implemented by `render::defuse` (a parser-based pass) and `mentions::resolve`
  (which `@name`s are users and which are groups: `POST /users/usernames`, then
  `GET /groups?q=` — cached a minute; a server without the groups feature answers
  `501` = no groups, cached ten minutes; any other failure, or more than 25 names
  in a message, sets `Lookups::closed` and **every `@name` that is not a known
  user is defused like a group** — fail closed). Where a backtick cannot be
  inserted — a link destination (`@` → `%40`), after a backslash, next to or
  among stray backticks — a zero-width space is written after the `@`.
- (phase 5, FIX-142, D-227(c)) **Conversion table.** Native constructs pass
  through byte for byte; the rest is converted as below; nothing is dropped.
  Implemented by `connectors/mattermost/src/convert.rs` (a pulldown-cmark offset
  pass that edits only the ranges below, in `render_text` before the mention and
  broadcast pass). **Verified (FIX-142 step 4, 2026-11-14)** against Mattermost 11.7.15
  (the pinned `mattermost-preview` image of `scripts/mattermost/setup.sh`): one post per
  construct was created through `render_text`, read back through the REST API (the stored
  message equals the converted text byte for byte), and its **rendered DOM** read in a headless
  browser logged in as the admin. Every row below is confirmed as stated. Notes from that run:
  the task list renders as real disabled checkboxes (`<input type="checkbox">`), so the
  `☐` / `☑` fallback is **not needed and not implemented**; display math as a ```` ```latex ````
  block renders with KaTeX on this server (`EnableLatex` and `EnableInlineLatex` are `true`
  by default in 11.7.15); `mermaid` shows as a plain code block; an alert shows as a quote
  with the emoji and a bold label; Mattermost joins an ordered list that follows a bullet
  list after a blank line into the bullet list (its own behaviour, not converted).

  | Construct | Mattermost | Conversion |
  |---|---|---|
  | Headings, emphasis, strong, strikethrough, inline code, block quotes, rules, nested/ordered lists, hard breaks, autolinks, emoji shortcodes | native | — |
  | Tables (alignment, inline formatting in cells) | native | — (a table longer than one post: the core splits it by rows, header row repeated in each post, `13 §5.2`) |
  | Fenced code with a language | native, highlighted | — |
  | Task lists | native (`- [ ]` / `- [x]`, passed through; renders checkboxes — verified) | — |
  | Footnotes | conversion | `[1]` at the reference, a "Notes" list at the end of the message |
  | Alerts (`> [!NOTE]` …) | conversion | block quote starting with `ℹ️ **Note**` / `💡 **Tip**` / `❗ **Important**` / `⚠️ **Warning**` / `🛑 **Caution**` |
  | Math `$$…$$`, ```` ```math ````, inline `$…$` | conversion (```` ```latex ```` blocks render when the server enables LaTeX) | ```` ```latex ```` block (renders when enabled, readable code otherwise); inline `$…$` left as is (renders when inline LaTeX is enabled) |
  | Mermaid | conversion | ```` ```mermaid ```` code block |
  | Allowlisted HTML (Mattermost renders no HTML) | conversion | `b`/`strong` → `**`, `i`/`em` → `_`, `s`/`del` → `~~`, `code`/`kbd` → `` ` ``, `br` → hard break, `hr` → `---`, `details`/`summary` → bold summary line + content, `sub`/`sup`/`ins`/`mark`/`small`/`span`/`div`/`p` → their content, HTML lists/tables → Markdown lists/tables |
  | Link previews | the **server** fetches the first bare URL of a post (`EnableLinkPreviews`), unprompted | no per-post switch exists (post `props` `preview_removed`, `link_previews`, `disable_link_preview`, `previews_removed` change nothing; the REST API has none). Measured: only a **bare** URL or `<url>` is fetched (a listener received `GET /?d=… ` from `Mattermost-Bot/1.1` during post creation); `[label](url)` (also `[url](url)`) and code spans are never fetched. **Closed by D-270**: the core writes every bare URL and autolink of an agent's message as `[url](url)` (`13 §4.3` exception 5), the connector re-checks that none is left (the plain-text fallback writes `https:\//…`), so agent posts show no preview cards; the operator note about `EnableLinkPreviews` stays as defence in depth |

  **Which URLs the server previews** (measured on 11.7.15 through the post's `metadata.embeds`,
  `connectors/mattermost/tests/mentions_e2e.rs::agent_urls_are_never_previewed_by_the_server`):
  `http://…` and `https://…` (any letter case), **`www.…`** (lower case; the server makes the
  embed with `http://` in front) and **`ftp://…`**. Not previewed: `ftps://`, `sftp://`, `file://`,
  `ws://`, `ssh://`, `mailto:`, `tel:`, `data:`, `javascript:`, a bare `example.org/path`, `//host/p`,
  `http:host/p`, `Www.…`/`WWW.…`, an e-mail address, a `[label](url)` link, a code span. (The local
  server could not be made to reach a `www.` host — its SSRF guard allows only the listed
  loopback names — so the fetch itself was observed for the `http://127.0.0.1` form, and the embed
  is the evidence for the others.) So the core rewrites a `www.` URL to `[www.x/…](https://www.x/…)`
  and writes `ftp://` as `ftp:\//` (it is no link scheme, rule 2); the plain-text fallback breaks all
  three (`https:\//`, `ftp:\//`, `www\.`). **Look-alike spellings of a ping word** do not notify (measured:
  `@ｃhannel` full-width c, `@ᴄhannel` small-cap c, Cyrillic `@сhannel`, `@channeI` capital i,
  `@c‍hannel` with a zero-width joiner, `@c­hannel` with a soft hyphen, `＠channel` full-width at,
  `@ａll`, `@ｈere`, `@ℂhannel`), so nothing is normalised for them.

  **Mentions table** (phase 5, D-271) — measured on 11.7.15 with two channel members, Alice
  online and Bob offline, through each member's `mention_count`
  (`connectors/mattermost/tests/mentions_e2e.rs`, run by `scripts/conformance.sh --mattermost`):

  | What the agent wrote | Notifies on the server? | What Aurora posts |
  |---|---|---|
  | `@channel`, `@all` — any letter case (`@Channel`, `@ALL`, `@cHaNnEl`) | everyone in the channel | the word in inline code |
  | the same followed by `. , : ! ? ; ) - _ ’ … — –` or `..`, `.-` | yes | in code, the punctuation after it |
  | the same after `( [ " ' * _ ~ # > - . / & = + $ % ^ \| { < :` or `\` | yes | in code (after a backslash: `@` + zero-width space) |
  | the same inside `**…**`, `_…_`, `~~…~~`, headings, quotes, list items, table cells, link labels, raw text of a destination | yes | in code (destination: `%40`) |
  | `@channelx`, `@channel.foo`, `@channel-foo`, `@channel_foo`, `@channel1`, `x@channel`, `1@all`, `@@channel`, `foo@all.example.com` | no — a longer name, or after an ASCII letter/digit | unchanged |
  | `＠channel`, `@ channel`, `@​channel`, `%40channel`, `&#64;channel` | no | unchanged |
  | `@here` (any case, any punctuation as above) | online members only | the core's `<@here>` only for the exact lowercase standalone spelling outside code and link labels (`@here`); every other spelling in code |
  | `@name` of a group (custom / LDAP) | every member | in code (resolved through the API; unresolved → treated as a group when the lookup failed) |
  | `@alice`, `@Alice.`, `**@alice**`, `[@alice](url)` | Alice | unchanged (D-226(b)) |
  | anything inside `` `…` ``, ```` ``` ````, indented code, `<code>`/`<pre>`/`<kbd>` | no (the server strips code) | unchanged (HTML code becomes a code span) |
  | `<code>@channel</code>` as raw text | yes — the server does not know HTML | converted to a code span first |

  Custom groups need a Mattermost licence: the unlicensed dev image answers `501`, so no real
  group can be created there; the group path is proven by unit tests with a mocked groups API
  (`mentions::tests`, `render::tests::group_mentions_…`) and, on a licensed server, by the same
  e2e (it creates a group and posts its mention).

  LaTeX is enabled in 11.7.15 by default (`EnableLatex`, `EnableInlineLatex`); an admin can
  turn it off, and ```` ```latex ```` blocks and `$…$` then show as readable code/text. Math is
  never an image carrier: the core writes a space into `![` and `](` inside math (`$! [x] (u)$`),
  which LaTeX ignores.
  **Mattermost-only image forms** (verified): `![x](url =10x10)`, `![x](https://h/a b)` and a
  reference definition directly after a paragraph line followed by `![x][r]` are images to
  Mattermost although CommonMark reads them as text; the core therefore breaks every `![`
  outside code (`!\[`), and `render_text` re-checks its converted output and posts escaped plain
  text if an image, a link outside http/https/mailto or an image opener is found.
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
  server's addresses (network policy / firewall). The integration is the one
  named in the signed context (`iid`); a bad or missing signature, an unknown
  integration or a missing clicker is refused 403 and nothing reaches the core.
  **The clicker's identity is the `user_id` the Mattermost server sends;
  Aurora trusts the Mattermost server for it.** That is sound only because the
  server makes this callback itself and the action context (token and
  signature) is never delivered to Mattermost clients; the conformance suite
  checks this against the supported server versions (a non-admin member's
  fetch of an approval post carries no integration context).
- `POST /mm/commands`: slash command; the integration is the one **whose
  command token matches** the request's token among those serving the request's
  team (never "the first integration in the team"); an unknown team, an
  unknown token and a wrong token are all refused 403 alike. Emit
  `command.invoked` with `name` = first word and `args`; respond with an
  ephemeral "working…" acknowledgement; replies come as ops. A team has one
  command per trigger, so when several integrations serve a team the one with
  the smallest id keeps `/aurora` and the others register `/aurora-<id suffix>`.
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
| `user_token.verify` (phase 5, P4-UTOK, C-854) | `GET /api/v4/users/me` with the person's token only (`user_credential`); the result must be the person's own platform user (the core compares it with their link). `401` → `credential_dead` |
| `message.post` with `sender.user_token` (phase 5, P4-UTOK, C-857) | `POST /api/v4/posts` with the person's token (`user_credential`), the post ending with the footer below. No other endpoint is called with a person's token: no user, token, team or channel-membership write, no reads in phase 5 |
| `credentials.validate` | as above |

Error mapping: 401 → `credential_dead`; 403 → `forbidden`; 404 → `not_found`;
429 → `rate_limited` (`retry_after` from `X-Ratelimit-Reset`); 413 →
`too_large`; 5xx/timeouts → `unavailable`.

### Narration cards and user-token posts (phase 5, P4-NARR, P4-UTOK; D-279, D-280, D-281, C-847..C-851, C-857)

- **Narration card** (`13 §5.12`): a post whose body is a plain list, with a header line
  `Working on it` or `Done` or `Stopped`, and one line per listed step as `- <name>: running|done|failed`.
  `<name>` goes in a code span (never Markdown, never a ping; `detail` text appears only in a `direct`
  conversation, `10 §Live narration cards`); a bare URL
  inside a label is wrapped in a code span too, so Mattermost never fetches it (D-270). The `N earlier` line
  is last when steps were cut. Words only, no emoji. A collapsed card (D-286) is the single line
  `Done: 4 steps, 42 s`, `Failed after 3 steps (1 failed), 1 min 5 s` or `Stopped after 2 steps, 12 s`,
  with no list. Edits, including the collapse, go through `PUT …/posts/{id}/patch`; a card is never deleted. The card is built within `message_max_chars` by construction (`NARR_MAX_STEPS`
  × `NARR_STEP_CHARS` < 16 383).
- **Footer of a user-token post** (D-283(c)): the last line of the post, italic, platform-authored:
  `_Sent by <agent name> for <owner label>_`. The agent and owner labels are sanitized and escaped; the footer
  is added by the connector at send time, not by the model; the owner approves the text without it, and the
  approval summary (`13 §5.11`) does not count it.
- **Tokens.** A person's token is used only for the calls of the Operations table above. Its value is never
  written to a log, a metric label or an error message; a `401` answers `credential_dead` with no detail.

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
  mapping, catch-up deduplication. (phase 5, FIX-142) One rendering test per
  conversion-table row: native constructs byte-identical, each conversion as
  stated; rendered `<@here>` → live `@here`; literal `@channel`/`@all`/`@here`
  defused.
