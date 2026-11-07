# aurora-connector-mattermost

The Mattermost connector (connector contract v1, `docs/20`). **Shared mode** (MM-001): one bot
per integration, bindings and DM defaults handled by the core. Dedicated bots (`presence.*`,
`presence_dedicated`) are MM-002; this manifest declares `presence_dedicated: false`.

## Run

```
aurora-connector-mattermost serve
```

| Setting | Flag / env | Default |
|---------|-----------|---------|
| Aurora API | `--api-url` / `AURORA_API_URL` (alias `AURORA_CORE_URL`) | `http://127.0.0.1:4100` |
| Connector token | `AURORA_CONNECTOR_TOKEN` (`aun_…`) | required |
| `connector.listen` | `--listen` / `AURORA_CONNECTOR_LISTEN` | `0.0.0.0:4110` |
| `connector.public_url` | `--public-url` / `AURORA_CONNECTOR_PUBLIC_URL` | none: buttons, menus and `/aurora` are off |
| `connector.callback_hmac_key` | `--callback-hmac-key` / `AURORA_CONNECTOR_CALLBACK_HMAC_KEY` (≥ 32 bytes) | random per process (single replica only) |
| `connector.catchup_window_secs` | `--catchup-window-secs` / `AURORA_CONNECTOR_CATCHUP_WINDOW_SECS` | 600 (max 86 400) |
| health / metrics port | `--ops-port` / `AURORA_CONNECTOR_OPS_PORT` | 4191 |

Test profile only: `AURORA_MM_TEST_CONTROL=true` mounts `/mm/test/*` (re-run catch-up, drop
`message.post` results); `AURORA_MM_CAPABILITIES=a,b` declares only those capabilities.

## Decisions worth knowing

- **Events** follow `docs/20`, with these choices where the text or the platform leaves room:
  - `thread_ref` is the post's `root_id`, absent for a top-level post (the conformance suite
    requires `None` for top-level messages; replies to a post carry its root).
  - `email_verified` is `Some(true)` **only when the Mattermost API says so**. Mattermost omits the flag
    when false, and Mattermost 12.0.2 does not return it for any user other than the caller
    (verified against `mattermost/mattermost-preview` 12.0.2: it is absent even for a system admin).
    Absence means *unknown*, never *verified*, so on such servers the core never auto-links
    (RFE-001) and `author.email` only pre-fills the sign-in link flow. `email` is whatever
    Mattermost shows the bot.
  - Plain-text commands (`!help` in DMs, `@bot help` in channels) become `command.invoked`
    instead of `message.posted`. `link` is forwarded like every other command; the connector never
    calls `POST /link-start` in shared mode (nothing in Mattermost asks for it).
  - A person cannot forge neutral tokens: `<@…>` / `<#…>` in message text are broken with a zero-width
    space before the connector adds its own mention tokens.
  - `bot.added`'s `actor_ref` comes from the `system_add_to_channel` post nearest the `user_added` event.
- **Slash command** `/aurora` is registered when an admin token is present and `public_url` is set
  (at integration open, not at `credentials.validate`, which has no integration yet). Its verification
  token is written back with `POST /credentials` (compare-and-set) as `command_token` in the integration's
  credentials; the contract has no write path for `config`.
- **Interactive callbacks** are accepted only with a valid HMAC over the action token
  (`context.sig`), then forwarded as `action.invoked`; the core verifies its own token. The signed
  context also carries the integration id (`iid`, unsigned: the core checks the token's integration).
- **Ops without an open integration** wait up to 10 s for the config poll to hand it over (ops can be
  claimed in the same instant), then fail `unavailable`.

## Conformance

`bash scripts/conformance.sh --mattermost` builds this crate, starts the pinned
`mattermost/mattermost-preview` container (`scripts/mattermost/setup.sh`), seeds a team, users and
a bot, and runs all twelve scenarios of `tests/conformance/connector` through the Mattermost driver
(`src/mattermost_driver.rs`), one at a time. Each scenario gets its own users, channels and bot.

Mattermost chooses its own ids, so the scenarios' `U1` / `C1` / `B1` / `F1` are aliases registered
with the fake core (`FakeCore::set_alias`). Mattermost strips the `integration` block (action tokens)
from every API answer, so the driver reads it from the server's database via `podman exec … psql`.

### Exclusions (steps skipped through `PlatformDriver::quirks()`)

All twelve scenarios run; these steps inside them are skipped on Mattermost:

| Scenario | Skipped step | Why |
|----------|--------------|-----|
| s02 | `author.email_verified == Some(true)` (asserted to be `None`) | Mattermost 12 does not expose other users' flag over REST |
| s05 | `author.email == None` for a user without an email | every Mattermost account has an email |
| s05 | `request_link` → `POST /link-start` | no person-initiated action triggers it in shared mode; the private delivery of a sign-in link (ephemeral post) is still asserted with a placeholder URL |
| s08 (a) | `presence.removed` for a dedicated presence (`B2`, presence id) | dedicated mode is MM-002; part (b), dead credentials → `credential_dead`, runs |

s04 additionally waits for the `bot.added` event whose actor is `U1`: the bot was put into the
channel by the harness first, which Mattermost reports like any other add.

### Mattermost test image

`mattermost/mattermost-preview`, pinned by index digest in `scripts/mattermost/setup.sh`
(`sha256:71fe9543…b233`, Mattermost 12.0.2, ~1.7 GB uncompressed, bundles PostgreSQL 14;
the compiled Team Edition is MIT-licensed). It is for CI and development only and is never
shipped with Aurora. Resources are named `aurora-mm-*`; `scripts/mattermost/setup.sh down`
removes the container (`conformance.sh --mattermost` does it unless `AURORA_MM_KEEP=1`).
