# P5-ICS: Calendar link (ICS) — read-only access to any calendar through its secret subscription link, no OAuth app needed (owner, 2026-11-14, phase 5, D-273, C-843, SEC-107)

## Why
The owner (2026-11-14):

> "There's a simple way to export your Outlook calendar in read-only mode that gives you a ICS file … Perhaps this would be a simple way to give at least read-only access to the user's calendar, an addon … where the user puts in their secret ICS link."
>
> "This would be simpler than the full microsoft-365 app because it doesn't require an oauth client to be defined, which I can't get without full official permission from our microsoft admins."
>
> "We could in fact do the same thing for Google and other calendar providers! Maybe a generic ICS calendar app, then?"
>
> "Why do we have to hardcode each provider? Why not just have the user give us a name (`Private calendar`) and the full URL? Then the gateway could just swap the entire URL in the request … [otherwise] we need to add each possible URL source by hand, including self-hosted calendar servers, etc. It's just not scalable."

Today `microsoft-365` and `google-workspace` both need an OAuth client registered by the organization's administrators. A published calendar link (Outlook *Publish a calendar*, Google *Secret address in iCal format*, iCloud public calendar, Proton, Fastmail, Nextcloud, an on-premises Exchange…) needs nobody's permission. It is just a URL that works like a password.

## The idea: the URL is the credential and the destination
A new kind of connection credential, the **secret link**:
- **What the person enters:** a **name** ("Private calendar") and the **full URL**.
- **What the agent does:** it never sees the URL. It requests a synthetic address for the connection.
- **What the gateway does:** it sends the request to exactly the stored URL, then returns the answer.

The agent controls nothing of the destination: no host, path, query or headers. The person fixed it at connect, so nothing can be steered elsewhere. That is why no provider list is needed. SEC-04 ("a credential is injected only on hosts its package declares") exists to stop a credential being sent where the *agent* chooses. It does not apply to a destination the person typed, which SEC-107 governs instead.

This is the database lane's pattern (P2-DBL, D-80, D-89, C-231, C-233), applied to one HTTP `GET`. In that lane:
- the agent talks to `<connection_id>.db.aurora.internal`;
- the gateway connects to the stored server;
- the workspace sync tells the supervisor which connections the agent may use, and the supervisor writes the agent a list.

The mechanism is generic (`secret_link`), so a later package (private RSS/Atom feeds, other "secret URL" exports) reuses it with no core change. This brief ships only the calendar package.

## Scope

### 1. Package `ics-calendar` (built-in)
- **App:** one app, **Calendar link (ICS)**, category `productivity`, neutral calendar/monogram icon (D-99; it spans vendors).
- **Connect method:** one `fields` method:
  - `name`: text, required, ≤ 60 characters, `store: metadata`; it becomes the connection label;
  - `url`: `secret`, required, `store: credentials`.
- **Manifest section `secret_link: { credential: url, expect_prefix: "BEGIN:VCALENDAR" }`.**
- **Tool:** one read tool, `read_calendar`, `GET` on `*.link.aurora.internal` path `/`. No write tools.
- **`docs/setup.md`:** where each common provider keeps its link. This is guidance only, with no code per provider:
  - Outlook on the web / new Outlook: *Settings → Calendar → Shared calendars → Publish a calendar*, copy the **ICS** link;
  - Google: *Settings → the calendar → Integrate calendar → Secret address in iCal format*;
  - iCloud public calendar, Proton, Fastmail, Nextcloud, Exchange on-premises OWA.

  For each it says:
  - the link is a password, so publish the least detail you need (busy only, titles and locations, or all details);
  - revoking is *Unpublish*/*Reset* at the provider, then reconnect with the new link;
  - changes arrive with the provider's own delay;
  - an Outlook organization may have turned publishing off, in which case only `microsoft-365` with admin consent reaches the calendar;
  - a server on a private network works only where the instance admin allows that range (`gateway.allowed_private`).
- **Agent snippet** (≤ 2,000 chars):
  - the calendars are listed in `~/.aurora/links.json` (`name`, `app`, `url`);
  - `curl` the listed `url` to a file;
  - expand recurrences with a pinned `uv run --with icalendar==<pin> --with recurring-ical-events==<pin> python - …` recipe that prints `start–end | title | location` between two dates in the person's time zone;
  - "Busy" entries without titles mean a busy-only publication;
  - `link_not_found` means the link was unpublished or reset, so ask the person to reconnect with the new link.

  The lane checks that PyPI is reachable from a default sandbox on the dev stack. If it is not, it stops and reports: preinstalling the two libraries in `images/sandbox/Containerfile` is the coordinator's call.

### 2. Core: the secret-link lane (C-843, SEC-107)
**Registry (manifest contract).** An app may declare `secret_link { credential, expect_prefix? }`:
- `credential` names a declared `secret: true` credential;
- the app has no `hosts[]` entry;
- its tools must be `GET` on host `*.link.aurora.internal`;
- the linter accepts that host only with the section (`tool_host_uncovered` otherwise), as C-233 does for `*.db.aurora.internal`;
- `aurora plugin test` fixtures may send `GET https://<uuid>.link.aurora.internal/` answered from a recorded upstream.

**API connect.**
- Validate the URL: `https` (a `webcal://` is normalized to `https://`), a DNS host (no userinfo, no IP literal unless covered by `allowed_private`), ≤ 2,048 characters.
- Fetch it once **through the gateway's destination guard**: 10 s, the first 256 KiB, redirects as below.
- Check `expect_prefix`.
- Store `url` as the credential, and the URL's **host** as metadata `host` (shown on the connection card; not secret).
- Errors are fixed codes and never echo the URL:
  - `link_not_https`;
  - `link_invalid`;
  - `link_destination_refused` (guard or org egress policy);
  - `link_not_found` (404 or 410);
  - `link_unreachable` (timeout, DNS, TLS);
  - `link_unexpected_content`.
- Reconnect replaces the URL. The URL is never returned by any read endpoint.

**Workspace sync and supervisor.**
- The home sync's `render_inputs` gains `links: [{ name, app_id, connection_id }]`: one entry per active connection of an app with `secret_link` that the agent holds a grant on (the same set as `apps[].accounts`, as C-231 does for databases).
- The supervisor writes `~/.aurora/links.json` (`0600`): `[{ name, app, url: "https://<connection_id>.link.aurora.internal/" }]`. It removes the file when the list is empty.
- No secret is in the sandbox.

**Gateway.** A request to `<connection_id>.link.aurora.internal` (TLS leaf from the gateway CA, as for `*.db`):
- **Before anything is fetched:**
  - The connection must exist and be of a `secret_link` app.
  - It must be granted to the calling agent. Policy, grants, approvals, Read-only (FIX-133) and the request log apply as to any tool request, with the tool taken from the manifest.
  - Methods: `GET` and `HEAD` only (else 405). The agent's path and query are ignored.
  - Forwarded agent headers are an allowlist: `Accept`, `If-None-Match`, `If-Modified-Since`, `Range`. Never cookies, `Authorization` or proxy headers.
- **The upstream request:**
  - It goes to the stored URL exactly: scheme, host, port, path, query.
  - The **destination guard** applies on every request (resolved address; private ranges only under `allowed_private`; cloud metadata, link-local and Aurora's own addresses never).
  - The **org egress policy** is also evaluated on the real host, so a host the org blocks stays blocked through a link.
- **Redirects** are followed by the gateway: at most 3, `https` only, guard and policy on every hop. A `3xx` and its `Location` never reach the agent.
- **The response:**
  - It is streamed, capped at `SECRET_LINK_RESPONSE_MAX` (new limit, 20 MiB, in `limits.rs`).
  - `Set-Cookie` and `Location` are dropped.
  - Upstream 404/410 become 404 `link_not_found`; guard or policy refusals become 403 `link_destination_refused`.
  - The echo check (FIX-104) uses the stored URL and its path-with-query, raw and percent-encoded, as needles; a match gives 502 `upstream_echoed_credential`.
- **Logs and traces:** the request log, approval summaries, traces (SEC-88) and every error carry the synthetic host, the connection id and the real **host** only. The real path and query appear nowhere.

**Dashboard.**
- The `fields` form already renders `name` and the `secret` URL.
- The connection card shows name, host, Read only and Edit, as for other connections. It never shows the URL.
- No new screen.

### 3. Docs
- `09` §3.4: the `secret_link` section.
- `06`: a **Secret-link lane** section next to the database lane.
- `12`: `links.json` beside the pg files.
- `17`: SEC-107 (written by the coordinator, below); keep it in step.
- The built-in catalogue list.

## Security (SEC-107, new; written into `17` with this brief)
A secret-link credential is a destination fixed by the person:
- the gateway sends a request only to the stored URL (`https`);
- the agent controls no part of the destination and only allowlisted headers;
- the destination guard and the org egress policy apply to the stored host and every redirect hop;
- the URL never reaches the sandbox, a log, a trace, an approval, an error message or any API read after connect (its host is shown, nothing else).

## Acceptance criteria
- [ ] **Registry:** the linter accepts the package. It refuses `secret_link` on a non-secret credential, a `secret_link` app with `hosts[]`, a non-`GET` tool, and `*.link.aurora.internal` without the section. Fixtures: placeholder `GET` → stored URL; `POST` → 405.
- [ ] **API ITs:** connect success (label = name, metadata `host`). Every error code, including a private address refused without `allowed_private` and accepted with it, and `webcal://` normalized. Neither the URL nor its path appears in any response, log line or audit row (canary). Reconnect replaces the URL.
- [ ] **Gateway ITs:**
  - an ungranted agent → refused;
  - a read-only connection works;
  - `HEAD` works and `POST` → 405;
  - the agent's path, query and a `Cookie`/`Authorization` header are not forwarded, and `If-None-Match` is;
  - a redirect is followed (≤ 3) and never shown;
  - a redirect to `http` or to a private address is refused;
  - an org block rule on the real host blocks;
  - a 404 → `link_not_found`;
  - an echoed URL → 502;
  - a body over the cap is cut with an error;
  - the request log row and the spans contain the host but not a canary token in the path or query.
- [ ] **Supervisor test:** `links.json` content, `0600`, removed when the list empties. A duplicated name keeps both entries; `connection_id` distinguishes them.
- [ ] **End-to-end on the dev stack:** an agent lists `links.json`, reads a recorded ICS through the link and expands a weekly series with an exception and a moved instance correctly in the person's time zone.
- [ ] **Live check by the lane** with the owner's test link: one Google secret iCal address, which is enough because nothing depends on the provider.
  - The link is at `~/.config/aurora-dev/p5-ics-test-link` (`0600`, outside the repo). Never copy it into the repo, fixtures, logs or a report.
  - Connect it, read it through the gateway, check the request log and spans for the canary (its token), and report the results.
  - Revocation (`link_not_found` after *Reset*) is tested with a recorded 404, unless the owner resets the link themselves.
- [ ] `cargo fmt --check`, clippy, the touched crates' tests; `pnpm -C web test` if any web file changes (none is expected).

## Coordinator check of the owner's link (2026-11-14; no content recorded)
- **The response:**
  - `GET` → `200`, no redirect, `text/calendar; charset=utf-8`, 123 KB, 0.5 s;
  - 327 `VEVENT`s from 2011 to 2027, one `VTIMEZONE` (`X-WR-TIMEZONE: Europe/Paris`);
  - one recurring series (`FREQ=WEEKLY;COUNT=5` with 3 `EXDATE`s; the other two `RRULE` lines are the time zone's);
  - Google sends `Set-Cookie` (dropped by the gateway) and `Cache-Control: no-store` with no `ETag`/`Last-Modified`, so conditional requests gain nothing with Google.
- **The echo check is confirmed:**
  - the token appears nowhere in the body;
  - the calendar id (path segment 3) appears **23 times** (`ORGANIZER`/`ATTENDEE`);
  - so the needles must be the whole URL and path-with-query, never single path segments, as C-843 says.
- **The recipe works:** `uv run --no-project --with icalendar==7.3.0 --with recurring-ical-events==3.8.2` on Python 3.13 expands the weekly series to its 2 remaining instances (5 − 3 cancelled), and `between()` over a window works. The sandbox base is AlmaLinux 10 (`python3` 3.12): the lane confirms these pins run there, or picks the newest that do.

## Ownership and sequencing
- **Owns:**
  - `plugins/builtin/ics-calendar/**`;
  - `crates/aurora-registry` (model, linter, plugin test);
  - `services/aurora-gateway` (new `link` module and its dispatch from the decision path);
  - the API connect path for `secret_link` and the home-sync `links` input;
  - `services/aurora-supervisor` (`links.json`);
  - `crates/aurora-core/src/limits.rs` (one constant);
  - the docs above.
- The registry manifest and the gateway's per-app decision are `p5-apps` hot files (PHASE5-PLAN): run it in that lane after its queue, or when the Orchestrator frees those files.
- No migration: the credential and metadata live in existing columns. If the home sync needs a schema change, report it first.

## Numbers
D-273 (rewritten for this design), C-843, SEC-107. No migration.

## Suggested agent/model and reason
`rust-developer` (Sonnet). It spans the registry, gateway, API and supervisor, like P2-DBL-2. The security tests matter more than the package.
