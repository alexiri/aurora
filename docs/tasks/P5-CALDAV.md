# P5-CALDAV: CalDAV calendar — read and write any CalDAV server with the person's own app password (owner, 2026-11-14, phase 5, D-275, C-845, SEC-109)

## Why
The owner (2026-11-14), after P5-ICS (read-only links) and P5-CALINV (invitations):

> "do Invitations and CalDAV app. Letting a person point their own password at a server they choose doesn't seem like a security issue, so I think it's fine. I'm assuming the agent will still not see either piece of information due to a trick like with ICS?"

CalDAV (RFC 4791) gives real read **and** write: events in a date range, server-side recurrence expansion, and create/change/delete of single events with ETags so a concurrent edit is never overwritten. Most servers sign in with a username and an **app password**, with no OAuth and no admin. That covers iCloud, Fastmail, Nextcloud, mailbox.org, Posteo, Zimbra, SOGo, Radicale, Baïkal and Synology.

**Not covered:**
- Microsoft 365 / Exchange Online have no CalDAV. Use ICS to read and invitations to write.
- Google's CalDAV requires OAuth, so the existing `google-workspace` app serves it.

## What the agent sees (the owner's question)
- **The password never reaches the sandbox.** The gateway adds `Authorization: Basic` on the way out, as for every other app.
- **The server address is visible to the agent, on purpose.** For ICS the URL is the secret. For CalDAV only the password is. CalDAV is many requests whose answers link to other paths and sometimes other hosts (iCloud moves an account to a `pNN-caldav.icloud.com` partition). Hiding the address would mean rewriting every response body, which is fragile and protects nothing.
- **What the agent is given:** its calendar accounts are listed in `~/.aurora/servers.json` (`[{ name, app, url }]`, entries with `app: caldav`, `url` = the discovered calendar home).

## Scope

### 1. Core: person-pinned servers (D-275, C-845, SEC-109)
A new manifest section `own_server: { url_field: <metadata field>, hosts_field: <metadata field> }`. The connection, not the package, names the server.
- **At connect, through the destination guard and the org egress policy:**
  - The URL must be `https` with a DNS host (no userinfo, no IP literal unless covered by `gateway.allowed_private`).
  - **Discovery:** `/.well-known/caldav` (following ≤ 3 redirects) → `current-user-principal` → `calendar-home-set`, authenticated with the entered credentials.
  - **What is stored:**
    - `url` = the calendar home, as metadata;
    - `hosts` = the exact `host:port` origins discovery used (≤ 4; iCloud's partition host included), as metadata;
    - the username and password as credentials (password `secret`).
  - **The label** is `<username> at <host>`, or the person's name for the account. A `name` field (text, optional) overrides it.
- **Injection:** only on requests whose origin is exactly one of the connection's `hosts`, over TLS to that port (SEC-04's TLS rule).
- **Never on these hosts:** a host held by a static package claim, a registered instance host, a governed LLM endpoint or any Aurora address. Connect refuses those with `server_host_claimed` before any credential is sent. A request elsewhere gets no credential (an agent following a redirect off the pinned hosts sends nothing).
- **Choosing the connection:** several connections of the app on one host follow the usual rule (`x-aurora-connection-id`, 409 `multiple_connections`).
- **Moved account:** if the server moves the account to a host not in `hosts`, requests there are uncredentialed and the server answers 401. The agent snippet says to ask the person to reconnect, which runs discovery again.
- **Owner decision (D-275):** a person may point their own credential at a server they choose. This is the second exception to SEC-04's declared-host rule, after SEC-107. It is safe because the credential is the person's own, it goes nowhere they did not name, and it can never land on a host another app or instance owns. SEC-59's admin-only host registration still governs instance hosts, which serve *everyone's* connections.

### 2. Registry (manifest contract)
- **`own_server`:** an app with `own_server` has no `hosts[]` entry and `inject` comes from the section:
  ```yaml
  own_server: { url_field: url, hosts_field: hosts, inject: [ { basic: { user: "{credentials.username}", password: "{credentials.password}" } } ] }
  ```
- **Tool host:** its tools use the host placeholder `"{connection}"` (any of the connection's pinned hosts) with `path: "/**"`.
- **Linter:**
  - refuses `{connection}` without the section;
  - refuses the section together with `hosts[]` or `instances`;
  - refuses `inject` stages other than `basic`, `bearer` or `header`;
  - refuses `rewrite_host`/`tenant`.
- **`check-builtin.sh`:** exempts `own_server` read tools from the GET-only read rule (they are `PROPFIND`/`REPORT`), as C-233(c) does for databases.
- **HTTP methods:** confirm that the WebDAV/CalDAV methods (`PROPFIND`, `REPORT`, `MKCALENDAR`, `PROPPATCH`, `MOVE`, `COPY`, `OPTIONS`) load and classify. The manifest model keeps `methods` as strings; check the gateway's classifier and the linter.

### 3. Package `caldav` (built-in)
- **App:** one app, **CalDAV calendar**, category `productivity`, neutral icon (D-99).
- **Connect method (`fields`):**
  - `name` (text, optional, metadata);
  - `url` (text, required, metadata: the server or account URL as the provider documents it);
  - `username` (text, required, credentials);
  - `password` (`secret`, required, credentials, `help` "Use an app-specific password").
- **Tools `read`:**
  - `discover_calendars`: `PROPFIND` on `/**`;
  - `query_events`: `REPORT` on `/**`;
  - `get_event`: `GET`/`HEAD` on `/**`;
  - `options`: `OPTIONS`.
- **Tools `write`:**
  - `put_event`: `PUT` on `/**`. It covers create and change; the agent sends `If-None-Match: *` to create and `If-Match: <etag>` to change.
  - `delete_event`: `DELETE` on `/**`.
  - `create_calendar`: `MKCALENDAR`.
  - `change_calendar`: `PROPPATCH`.
  - `move_event`: `MOVE`/`COPY`.
  - `send_scheduling_message`: `POST` on `/**`, a server's scheduling outbox. **Default grant: Ask.** It emails the attendees.
- **Approval summaries** need a new decoder `ical` (`09 §5`, decoders). For `PUT`, the summary shows SUMMARY, DTSTART–DTEND with time zone, whether it repeats, LOCATION and **every ATTENDEE**. A server with automatic scheduling (iCloud, Nextcloud) emails the attendees on `PUT`, and the person deciding must see who would be invited. `DELETE` shows the path. Bodies over the summary budget say so.
- **Connect errors** are fixed codes and never echo the password:
  - `caldav_not_https`;
  - `caldav_destination_refused`;
  - `caldav_unreachable`;
  - `caldav_unauthorized` (401/403: wrong user or password, or app passwords required);
  - `caldav_not_caldav` (no principal or calendar home);
  - `server_host_claimed`;
  - `caldav_needs_oauth`: a Google CalDAV host, which points to the Google Calendar app;
  - `caldav_unsupported`: an Exchange/Outlook host, which points to ICS and invitations.
- **`docs/setup.md`:**
  - per common provider, the URL to enter and where to make an app password: iCloud (`https://caldav.icloud.com`, appleid.apple.com → App-Specific Passwords), Fastmail, Nextcloud, mailbox.org, Posteo, generic;
  - why Google and Outlook are not here;
  - that a write which invites attendees emails them;
  - a server on a private network needs the instance admin's `allowed_private`.
- **Agent snippet** (≤ 2,000 chars):
  - accounts are the `app: caldav` entries of `~/.aurora/servers.json`;
  - use the Python `caldav` library pinned via `uv run --with caldav==<pin>`, through the normal proxy (no credentials needed in code; pass any placeholder user, since the gateway replaces `Authorization`);
  - list calendars;
  - fetch events in a range with server-side expansion where supported, else expand with `recurring-ical-events` as in P5-ICS;
  - create with `If-None-Match: *`, change with `If-Match`, and on 412 re-read and retry;
  - adding attendees invites them, so ask the person first;
  - 401 means ask the person to reconnect.

### 4. Home sync and supervisor
- **Home sync:** `render_inputs.servers: [{ name, app_id, connection_id, url }]` for active connections of any `own_server` app that the agent holds a grant on (the `apps[].accounts` set, as C-231). It is generic, so a later own-server app reuses it.
- **Supervisor:** writes `~/.aurora/servers.json` (`0600`, removed when empty): `[{ name, app, url }]`.
- No secret is in the sandbox.

### 5. Docs
- `09`: the `own_server` section and the `ical` decoder.
- `06`: the pinned-server injection path.
- `12`: `servers.json`.
- `17`: SEC-109 (written by the coordinator).
- The built-in catalogue list.

## Test server
**Radicale** in a container for the ITs and fixtures. It is pinned, local and covered by `allowed_private` in the test config. It supports discovery, `REPORT` with `expand` and ETags.

Live checks against iCloud or Fastmail need an account the owner provides as a throwaway app password, kept outside the repo like P5-ICS's link. They are optional for Done: the report lists what was and was not run live.

## Acceptance criteria
- [ ] **Registry:** the package loads. Each linter refusal above has a fixture. `check-builtin.sh` passes with the exemption. The WebDAV methods classify to the right read/write tools (fixture per method).
- [ ] **API ITs (Radicale):**
  - connect discovers the home and stores `url`/`hosts`;
  - the label is right;
  - each error code (wrong password → `caldav_unauthorized`; plain `http` → `caldav_not_https`; private address without `allowed_private` → `caldav_destination_refused`; a host claimed by a built-in package → `server_host_claimed`, with no request sent; a Google CalDAV URL → `caldav_needs_oauth`);
  - the password appears in no response, log or audit row (canary).
- [ ] **Gateway ITs:**
  - `Basic` is injected on a pinned host only;
  - nothing is injected on another host, on the same host over `http`, or on another port;
  - a static-claim host is never injected even if it equals a stored host (a snapshot change after connect);
  - read-only connection: `PROPFIND`/`REPORT` pass, while `PUT`/`DELETE`/`MKCALENDAR` are refused;
  - an approval on `PUT` shows title, time and attendees from the `ical` decoder;
  - `send_scheduling_message` defaults to Ask.
- [ ] **Supervisor:** `servers.json` content, `0600`, removed when the list empties.
- [ ] **End to end (dev stack, Radicale):**
  - the agent lists calendars;
  - reads a week with a repeating event expanded;
  - creates an event (`If-None-Match: *`);
  - changes it (`If-Match`; a stale ETag → 412, re-read, retry);
  - deletes it.
- [ ] `cargo fmt --check`, clippy, the touched crates' tests; the package's `aurora plugin test`.

## Ownership and sequencing
- **Owns:**
  - `crates/aurora-registry` (model, linter, the `ical` decoder);
  - the gateway's injection decision for `own_server` and the decoder;
  - the API connect path for `own_server` (discovery) and the home-sync `caldav` input;
  - `services/aurora-supervisor` (`servers.json`);
  - `plugins/builtin/caldav/**`;
  - the Radicale test container;
  - the docs above.
- It shares the registry, the gateway decision path and home sync with **P5-ICS**. Run it **after P5-ICS in the same lane** (`p5-apps` files), reusing P5-ICS's patterns: the synthetic-host dispatch is not used here, but the home-sync list, the supervisor file and the destination-guard connect fetch are.

## Numbers
D-275, C-845, SEC-109. No migration (credentials and metadata live in existing columns).

## Suggested agent/model and reason
`rust-developer` (Sonnet). It spans the registry, gateway, API, supervisor and a package, like P2-DBL-2. The injection-scope tests are the core of it.
