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
  - `curl` the listed `url` to a file **once** per task and work on that file: re-downloading within 5 minutes returns the gateway's cached copy anyway;
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
  - Only the agent's `Accept` header is forwarded upstream. Never cookies, `Authorization`, proxy headers, `Range` or conditional headers.
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
- **Cache** (owner, 2026-11-14: "cache the results for a bit … so that we don't hit the server if the agent keeps downloading it"):
  - **What is kept:** a complete `200` body, in the replica's memory, for `SECRET_LINK_CACHE_TTL` = 300 s. It is keyed by `(connection_id, credential version)`; the lane uses whatever already changes on reconnect, such as the credential's `updated_at` or a secret version.
  - **Bound:** `SECRET_LINK_CACHE_MAX_BYTES` = 64 MiB per replica, least recently used evicted. A body over the response cap is never cached.
  - **Order:** admission (grant, policy, approvals, Read only) runs **before** the lookup, every time.
  - **Logging:** a hit is a request-log row with `cache: hit`, and the span says so.
  - **Upstream headers:** the upstream's `Cache-Control: no-store` (Google sends it) is overridden, because the cache is private to the connection's admitted callers.
  - **Not cached:** errors, redirects and cut bodies.
  - **Dropped on:** reconnect, disable or delete of the connection, and credential revocation, by subscribing to the gateway's existing connection-change signal (the lane names it).
  - **What the agent gets:** `ETag` (a hash of the body) and `Age`. `If-None-Match` matching the cached `ETag` gets `304` with no upstream call.
  - **No disk and no shared store:** a multi-replica gateway just fetches once per replica.

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
  - the agent's path, query, `Cookie`, `Authorization`, `Range` and `If-None-Match` headers are not forwarded upstream;
  - cache:
    - two `GET`s within the TTL make one upstream call, and the second is logged `cache: hit` with `Age`;
    - after the TTL, a new upstream call;
    - `If-None-Match` with the gateway `ETag` → `304`, with no upstream call;
    - an ungranted agent is refused while the body is cached;
    - reconnect (new URL) and delete each drop the entry, so the next request goes to the new URL or is refused;
    - an error or a cut body is not cached;
    - filling past `SECRET_LINK_CACHE_MAX_BYTES` evicts the least recently used entry;
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
  - `crates/aurora-core/src/limits.rs` (three constants: response cap, cache TTL, cache size);
  - the docs above.
- The registry manifest and the gateway's per-app decision are `p5-apps` hot files (PHASE5-PLAN): run it in that lane after its queue, or when the Orchestrator frees those files.
- No migration: the credential and metadata live in existing columns. If the home sync needs a schema change, report it first.

## Numbers
D-273 (rewritten for this design), C-843, SEC-107. No migration.

## Suggested agent/model and reason
`rust-developer` (Sonnet). It spans the registry, gateway, API and supervisor, like P2-DBL-2. The security tests matter more than the package.

## Completion report (lane `p5-agent`, worker 29, 2026-11-18) — PARTIAL: part 1 of 5 done

**Done and committed (22db5880): registry.** `App.secret_link: Option<SecretLink { credential, expect_prefix }>` (model, copied into the snapshot `AppEntry` with `skip_serializing_if`, so the 65 golden snapshot hashes are unchanged); constants `SECRET_LINK_HOST_SUFFIX`, `SECRET_LINK_TOOL_HOST`, `SECRET_LINK_PREFIX_MAX`; `SecretLink::host_of` / `id_label` (string-based: the registry has no `uuid` dependency, the gateway parses). Linter codes: `secret_link_credential` (must be a declared, non-optional `secret: true`), `secret_link_hosts`, `secret_link_tool` (write group, wildcard, a method other than only `GET`, a host other than `*.link.aurora.internal`, a path other than `/`), `secret_link_prefix`, `secret_link_exclusive` (database/ldap/instances/token), `secret_link_host_reserved` (no `hosts[]` entry may claim the suffix, section or not); `tool_host_uncovered` for the host without the section. Tests: `crates/aurora-registry/tests/it/secret_link.rs` (8; new by construction, the code did not exist). Gate: per task — `cargo fmt`, `cargo clippy -p aurora-registry --all-targets --all-features -D warnings` clean, `cargo test -p aurora-registry` 22 + 128 green. Registry files touched: `manifest/model.rs`, `snapshot.rs`, `validate/mod.rs`, `tests/it/{main,secret_link}.rs` (all additive).

**Adapted to master** (brief of 2026-11-14): file references checked — `crates/aurora-registry` model/linter/snapshot as the brief says; the "plugin test" runner is `aurora-gateway check-snapshot` (`services/aurora-gateway/src/check_snapshot.rs`, db hosts special-cased near line 581), not the CLI crate, so the fixture support for `GET https://<uuid>.link.aurora.internal/` belongs there; the gateway has a `links.rs` (dashboard links, FIX-153), so the new module must not be called `links`; the D-251 LDAP lane (`PreparedRequest.ldap`, `proxy/mod.rs::ldap_answer`, `decide.rs::handle` guard) is the closest pattern and fits better than the db lane (which has its own wire); P5-UX/P5-IA: the connect form is the journey's sign-in step and an account is a row, so the `fields` method needs no new screen.

**Design for the remaining parts (found while reading; not yet built):**
- *Gateway.* Mirror D-251: a new field `PreparedRequest.link: Option<LinkFetch>` (stored URL, connection id, credential version, host, `Accept`), taken in `proxy/mod.rs::process_request` next to `prepared.ldap.take()` and answered by `link_answer`, which loops `Upstream::send(PreparedRequest)` (already guarded dialling through `Guard`) for ≤ 3 `https` hops. In `decide.rs::handle_with`, right after `resolver.resolve`, branch when `SecretLink::id_label(&host)` parses as a connection id in `rv.conns`: grant via `selected_candidates`, tool via `apps::identify_tool_in(.., host, 443, "/", None, method, ..)` (as `DbSession::authorize_with`), `evaluate` + `rate.settle` + the approval hold, and the `Released` second pass through `handle_with` as the HTTP path does; then `plaintext.get_connection(conn)` for the URL, only after admission. Every other request on a `*.link.aurora.internal` host with no winning connection must be refused in `RequestHandler::handle` (as the LDAP guard at the end of that function does), never forwarded. Per-hop org policy: a new `PolicyHandler` method that `evaluate`s a plain `GET https://<real host>/` (a `NeedsApproval` verdict is a refusal, a hold is impossible mid-fetch); do **not** reuse `open_lane_check` (it applies the workspace open-lane posture, which would refuse a disabled open lane). Echo needles: `inject::url_needles([url bytes], path, query)`. The log row's `path` must be the synthetic path `/` (never the real one): `Ctx.host/path` are the synthetic ones already.
- *Cache* (`SECRET_LINK_*` constants go in `aurora-core/src/limits.rs`; add them to `scripts/guards/limits-gaps.allow` if the limits guard asks). Key `(connection_id, credential updated_at)`; drop on the gateway's connection-change signal (name it after reading `invalidation.rs`).
- *API connect, home sync (`links`), supervisor (`links.json`), package `plugins/builtin/ics-calendar`, docs (09 §3.4, 06, 12, built-in catalogue)*: untouched.

**Not run:** the live check (needs the owner's link), the PyPI reachability check from a sandbox, the pins on AlmaLinux 10. The gateway, API, supervisor, e2e and batch gates are not run (nothing there changed yet). Deviation: one `sed -i` was attempted on `snapshot.rs` (an import line) and was blocked by the harness before it ran; the line was then edited with Edit. No other file was written with a shell tool.

## Completion report, parts 2–5 (lane `p5-agent`, worker 30, 2026-11-18) — code done, batch gate NOT yet run

Commits: `6f1171a2` (2/5 gateway), `762c91ad` (3/5 API connect), `6ef1bf59` (4/5 home sync + supervisor), `a4d1a4d6` (5/5 package + docs).

**Adapted to master.** The secret-link app has no `hosts[]`, so it never matches `identify_request`; the decision is its own path (`decide/link.rs`, a child module of `decide`) taken in `handle_with` right after the `Ctx`, the DB lane's admission (`db_connection` by id, grant, tool via `identify_tool_in`, evaluate/settle/hold/`verdict_error`) with the LDAP lane's hand-off (`PreparedRequest.link`, answered in `proxy/mod.rs`, plus the same "never forward on this host" guard in `handle`). The connection is not in `rv.conns` (the host is reserved), hence `db_connection`. The stored credentials are the JSON object of the secret fields; the URL is `credentials[secret_link.credential]`. The connect `name` field is metadata `name` (declared) and becomes the label. `http` (not `url`) is used for URL parsing in the gateway so `Cargo.lock` is unchanged; the supervisor does not depend on the registry crate (a local copy of the host suffix).

**Deviations / decisions to confirm.**
- *Org egress policy at connect:* the API has no org-rule evaluator, so connect applies the destination guard only (`allowed_private` included); the org and workspace egress policy is applied by the gateway on every read, on the real host of every hop (`seclink::Hops`, tested). A host the org blocks therefore connects and then reads as 403 `link_destination_refused`.
- *Request-log "cache: hit":* the row has no such column and no migration was allowed; a hit is `error_code = "cache_hit"` (status 200). A column would be a migration (ask).
- *Streaming:* the body is read whole (≤ 20 MiB) before it is answered, because the cache and the echo check need it complete; the cap is enforced while reading.
- *Real host in the log row/span:* not recorded (the row has the synthetic host, connection id, app, tool; the real host is the connection's metadata `host`). Spans carry no path or query at all (`server.address` is the synthetic host).
- The approval hold path (`NeedsApproval` on a link) is implemented like the DB lane's (summary: connection + real host, no URL) but has **no IT** (the apps harness has no hold store); the second pass re-reads grant and policy.
- A HEAD is served from the same cache and answered without a body; upstream is always `GET`.

**Where the URL is stored, decrypted or could be echoed, and the test that proves it never leaks (canaries `PATHCANARY…`/`QUERYCANARY…` in the stored URL's path and query).**
1. *API request body (connect):* in memory only for the request; refusals use fixed messages (`link_*`); proved by `connect::link_connect::{every_refusal_is_a_fixed_code_that_never_echoes_the_url, the_servers_answer_decides_…}` (canary in no response, no `/connections` list, no `audit_log` row, no `app_connections` row).
2. *`app_connections.credentials_ref`:* sealed with the vault (`ConnectionCredentials`); the row's label/metadata hold only the name and the host (`connect_stores_the_url_as_the_credential_and_its_host_as_metadata`, `reconnect_replaces_the_url`: sealed value and credential version change).
3. *Probe fetch at connect:* guarded client; the mock server received exactly the stored path and query once; errors map to fixed codes.
4. *Gateway decrypt:* only after full admission, in `link_request`, into `LinkFetch.url` (a manual `Debug` that prints only the connection id); never put in `Ctx`, `Hook`, an approval summary or an `ApiError`. Tests: `apps::link_lane::{the_stored_url_is_fetched_exactly_and_the_agent_steers_nothing, the_canaries_appear_in_no_response_error_or_log_row}` (response heads/bodies of success, 404, 500, echo, redirect and private-address cases and every request-log row).
5. *Upstream response echo:* needles = whole URL and path-with-query, raw and percent-encoded, over headers and the whole body → 502 (`an_echoed_url_is_withheld`; unit `needles_are_the_whole_url_and_path_with_query_not_segments` proves a calendar-id-like segment is not a needle).
6. *Gateway cache:* holds the body only, keyed by `(connection id, credential version)`, memory only; never the URL.
7. *Home sync / supervisor / sandbox:* `render_inputs.links` carries `{name, app_id, connection_id}` only (`render_inputs_list_the_granted_secret_link_connections_by_name_and_id` asserts the 3 keys); `links.json` is the synthetic address, 0600 (`links_in_render_inputs_become_links_json_…`, unit tests in `links.rs`).
8. *Agent request headers:* `Cookie`, `Authorization`, `Range`, `If-Modified-Since`, `X-Forwarded-For`, `Proxy-Authorization`, `If-None-Match` are not forwarded (asserted at the fake upstream); only `Accept`.

**Tests added (all new by construction; the code did not exist, so no fail-before is possible).** Gateway: `tests/it/link_lane.rs` (12: exact fetch + header allow-list + log row; HEAD/405/read-only; unknown/ungranted/foreign host; cache TTL/ETag/304/ungranted-while-cached; invalidation + credential version + delete; errors and cut bodies not cached; 404/410; ≤ 3 redirects hidden; http/private redirect refused; org block on the real host (upstream count 0); echo; canaries) + 3 unit tests in `seclink.rs` (redirect targets, needles, cache TTL/LRU/size/version). The harness (`tests/it/apps.rs`) gained `Src::connection`, `Rows.org`, a capturing log writer and the cache/resolver handles. API: `tests/it/link_connect.rs` (6) and 2 unit tests; `home_render_inputs` (+1, +1 assertion). Supervisor: 5 unit tests + 1 IT (`home.rs`). Package: `SKIP_CARGO=1 bash scripts/plugins/check-builtin.sh ics-calendar` → ok (5 fixtures incl. 2 negatives; uses the new `link_case` in `check_snapshot.rs`).

**Per-task gates run:** `cargo fmt --all --check`; clippy `-D warnings --all-targets --all-features` on `aurora-gateway`, `aurora-core`, `aurora-api`, `aurora-supervisor` (clean); `cargo test -p aurora-gateway --lib` (82) and `--test it -- apps::` (57, includes the 12 new); `-p aurora-api --test it -- link_connect` (6) and `-- home_render_inputs` (5); `-p aurora-api --lib -- connect::service::link` (2); `-p aurora-supervisor --lib -- links` (5), `--test it -- home::` (18); `scripts/guards/limits-readers.sh` (exit 0); `python3 scripts/extract-schema.py --check` (up to date).

**Not run (batch gate and live checks):** the full suites of `aurora-registry`, `aurora-gateway`, `aurora-core`, `aurora-api`, `aurora-supervisor`; `scripts/guards/run-all.sh`; OpenAPI drift/`gen:api` (no route changed); `cargo deny` (`Cargo.lock` untouched); `scripts/e2e/gateway.sh`; `hosted.sh`; the web suites (no `web/` change); the dev-stack end-to-end (agent lists `links.json`, reads a recorded ICS, expands a weekly series with exception and moved instance); the **live check with the owner's Google link — not run, needs the owner's link**; PyPI reachability from a default sandbox and the recipe pins (`icalendar==7.3.0`, `recurring-ical-events==3.8.2`) on AlmaLinux 10's Python 3.12 — not run; shellcheck not installed.

## Close-out (lane `p5-agent`, worker 31, 2026-11-18)

- **Approval hold IT added** (`link_lane.rs`: `an_approval_hold_names_the_connection_and_host_never_the_url_and_fetches_only_once_approved`, `a_denied_link_approval_fetches_nothing_and_leaks_nothing`). The harness got a real hold store (`start_in(.., with_holds)`, a `Holds` on the test database; `lane_with_approval`). Proved: held with zero upstream calls; the approval row/summary has the connection name and `localhost`, canary-free; after approve exactly one fetch; a grant withdrawn while held is refused on the second pass (no fetch); deny → 403 `manual_approval_denied`, no fetch, no canary. Both pass; new by construction (no fail-before possible).
- **`error_code = "cache_hit"` dropped.** Readers checked: `web/src/components/agents/status-rail/status-data.ts` (`decisionBadge`/`hasProblem`: any `error_code` reads **Failed**), the activity route DTO (`activity/routes.rs`, passes it through to the activity view). A hit would have shown as a failure, so it is gone: a hit is an ordinary 200 row with `error_code` null and no `upstream_status` (no upstream call; a miss has `upstream_status 200`). IT asserts both rows. No migration needed. Docs 06 and 17 (SEC-107) updated.
- **Egress rule documented** in `docs/09` (secret_link) and `docs/17` SEC-107: connect applies the destination guard only; org/workspace egress policy on every read, real host of every hop.

**Batch gate (worker 31, HEAD after the clippy-allow commit; box shared with the p5-apps lane, load 5–26):**
- `cargo test -p` full: `aurora-registry` 22 + 128 + 0; `aurora-core` 78; `aurora-supervisor` 118 + 2 + 4 + 137; `aurora-gateway` 82 lib + 355 it (all targets exit 0). `aurora-api` (after `scripts/fixtures/build.sh`, 67 ok, 0 failed): lib 182, `it` 1479 passed / 1 failed under load (`evals_judge::a_judge_that_cannot_be_used_is_an_error_and_no_request_is_sent`, "an active eval turn: RowNotFound") → rerun alone `-- evals_judge` 7 passed; the other 5 test targets and the doc target green. The failure is unrelated to this lane's files (eval turns).
- `SKIP_CARGO=1 bash scripts/plugins/check-builtin.sh` exit 0 (ics-calendar ok); `cargo clippy --workspace --all-targets --all-features --locked -D warnings` exit 0; `scripts/guards/run-all.sh` exit 0 ("all guards passed"); `extract-schema.py --check` up to date; OpenAPI drift IT 3 passed, `pnpm -C web gen:api` no diff; `scripts/e2e/gateway.sh` exit 0 (22, 4, … all ok; no `hooks h2` failure this run); `AURORA_TEST_PODMAN=1 AURORA_E2EH_PREFIX=p5agent-e2eh … hosted.sh` exit 0, 19 passed, `s8d` skipped (needs `AURORA_E2EH_FULL_IMAGE`).
- **Recipe pins checked live from this box (not from a sandbox):** `uv run --no-project --with icalendar==7.3.0 --with recurring-ical-events==3.8.2` resolves from PyPI; a recorded ICS (weekly ×5 in Europe/Zurich with an `EXDATE` and a moved `RECURRENCE-ID` instance) printed in America/New_York: the excluded week gone, the moved instance at its new time and title, the rest at 03:00 NY (= 09:00 Zurich).
- **Not run:** the dev-stack end-to-end *through a sandbox* (agent lists `links.json`, `curl`s the link through the gateway and expands it): the pieces are proved separately (supervisor IT writes `links.json`; gateway IT fetches the recorded ICS; the recipe above) but no test chains them in a sandbox, and no hosted e2e test for the link lane exists (a `tests/e2e-hosted` test with a fake HTTPS server under a trusted private CA would be a new task); PyPI reachability *from a default sandbox* and the pins on AlmaLinux 10's Python 3.12; the live check with the owner's Google link (needs the owner's link); shellcheck (not installed); `cargo deny` (`Cargo.lock` untouched).

**Registry files touched in parts 2–5:** none (part 1's files only). `plugins/builtin/ics-calendar/**` is new content (no overlap with P5-GUIDE-4's package edits).
