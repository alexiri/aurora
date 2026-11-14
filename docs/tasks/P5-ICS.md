# P5-ICS: Calendar link (ICS) — read-only access to a calendar through its secret subscription link, no OAuth app needed (owner, 2026-11-14, phase 5, D-273)

## Why
The owner (2026-11-14):

> "There's a simple way to export your Outlook calendar in read-only mode that gives you a ICS file … Perhaps this would be a simple way to give at least read-only access to the user's calendar, an addon … where the user puts in their secret ICS link."
>
> "This would be simpler than the full microsoft-365 app because it doesn't require an oauth client to be defined, which I can't get without full official permission from our microsoft admins."
>
> "We could in fact do the same thing for Google and other calendar providers! Maybe a generic ICS calendar app, then?"

Today `microsoft-365` and `google-workspace` both need an OAuth client registered by the organization's administrators. A published calendar link needs nobody's permission. The person publishes the calendar in their own calendar settings, then pastes the link once.

## What it is
A new built-in package **`ics-calendar`** with one app, **Calendar link (ICS)**, category `productivity`.
- **Icon:** a neutral monogram/calendar tile per `plugins/builtin/ICONS.md` (D-99): it spans vendors, so no vendor's brand.
- **Description:** "Read a calendar you publish as a private subscription link (Outlook, Google Calendar). Read-only; no admin-registered sign-in app needed."

### Providers in this build
Each provider is one **connect method** (the "How do you want to connect?" cards, FIX-123), so each card and the setup guide can say exactly where that provider keeps the link.

| Method id | Card label | Host | Link the person pastes (`webcal://` accepted, normalized to `https://`) | Stored |
|---|---|---|---|---|
| `outlook` | Microsoft Outlook (work or school) | `outlook.office365.com` | `/owa/calendar/<mailbox>/<token>/<file>.ics` | credential `token`; metadata `mailbox`, `file` |
| `outlook_com` | Outlook.com (personal) | `outlook.live.com` | `/owa/calendar/<mailbox>/<token>/<file>.ics` | same |
| `google` | Google Calendar | `calendar.google.com` | `/calendar/ical/<calendar id>/private-<token>/basic.ics` | credential `token` (the part after `private-`); metadata `calendar_id` |

- **Link formats are from public knowledge, not verified yet.** Microsoft's page (support article "Share an Outlook calendar as view-only with others", read 2026-11-14) gives the steps but no link format, host, refresh delay or secrecy statement. Before writing the patterns, the lane verifies each format against a real published link: the owner provides a throwaway calendar link per provider, used locally and never committed.
- **Unconfirmed variants are not guessed.** Where a variant can't be confirmed (another Outlook host, another final file name), it is refused with a message naming the expected shape.

**Adding a provider later** is a package update, with no core change:
- a `hosts[]` entry;
- a method;
- its pattern in the hook;
- one tool;
- a setup-guide section;
- one line in the agent snippet;
- fixtures.

Candidates, each needing a verified link format: iCloud public calendars (`p<NN>-caldav.icloud.com/published/2/<token>`), Proton (secrets in the query string, served by the `query` mechanism), Fastmail. Self-hosted servers (Exchange on-premises OWA, Nextcloud) would use instance-bound hosts (D-74) and are out of scope here. **Arbitrary hosts are impossible by design:** a package injects only into hosts it declares (SEC-04).

## Design (D-273)
1. **The secret is the link's path, and the agent never sees it.**
   - **What the agent requests:** a fixed placeholder path on the provider's host:
     - `https://outlook.office365.com/owa/calendar/aurora/calendar.ics`;
     - `https://outlook.live.com/owa/calendar/aurora/calendar.ics`;
     - `https://calendar.google.com/calendar/ical/aurora/basic.ics`.
   - **What the gateway does:** each host entry matches only that placeholder path (`path_prefix` as narrow as the linter allows). One `path_regex` inject rebuilds the real path, for example `^/owa/calendar/aurora/calendar\.ics$` → `/owa/calendar/{metadata.mailbox}/{credentials.token}/{metadata.file}`, and `^/calendar/ical/aurora/basic\.ics$` → `/calendar/ical/{metadata.calendar_id}/private-{credentials.token}/basic.ics`. Each reference is percent-encoded per segment, as the gateway already does (`mechanisms/template.rs`).
   - **Why the mailbox and calendar id are metadata:** only the token grants access. The mailbox or calendar id can be the person's email address, which the ICS body repeats in `ORGANIZER`/`ATTENDEE` lines. As a credential it would trip FIX-104's echo check (`upstream_echoed_credential`) on every read.
   - **Echo check on the token:** it stays on, and the token must not appear in the response.
2. **Each connection is injected only on its own provider's host.**
   - Every host entry carries a `tenant` guard on `connection.tenant_host` with a zone equal to that exact host, and the hook sets `tenant-host` at connect.
   - So a request to another provider's placeholder is refused 421 `host_mismatch` (SEC-04(c)). A Google token never goes to Microsoft or the reverse.
3. **Connect is a `hook` method per provider** (`authorize: false`, one `secret` field "Calendar link"). `connect.exchange` (§8.2, WIT `connected` record) does the work.
   - **Parse and check:** it parses the pasted URL and checks the scheme (`https`/`webcal`), the exact host for the chosen method and the path pattern.
   - **Fetch once:** it fetches the link once through its declared egress: the three hosts, 10 s, the first 256 KiB.
   - **Validate:** it requires `BEGIN:VCALENDAR`.
   - **Output:** `credentials {token}`, metadata, `tenant-host` and a label `"<X-WR-CALNAME or 'Calendar'> (<provider>)"` (sanitized, no part of the link).
   - **Errors** are fixed codes with fixed copy, and never include the URL: `ics_link_not_recognized` (shape), `ics_link_wrong_provider`, `ics_link_not_found` (404/410: not published or unpublished), `ics_link_unreachable`, `ics_not_a_calendar`.
   - **Busy-only calendars:** a calendar published "busy only" connects fine. The label gets no warning, but the setup guide explains it.
4. **Read only by construction.** `tools.read` has one tool per method (`read_calendar_outlook`, `read_calendar_outlook_com`, `read_calendar_google`: `GET` on the placeholder path, `connect_methods: [<method>]`), and there are no write tools. `token: static`. Grants, approvals, Give agents access and the request log work as for any app.
5. **The agent snippet** (≤ `AGENT_SNIPPET_MAX` 2,000 chars, written in the package) says:
   - which placeholder URL belongs to which provider;
   - that the connection label names the provider;
   - to use `x-aurora-connection-id` when several calendar connections exist (gateway 409 `multiple_connections` lists them);
   - to fetch to a file and expand recurrences with a pinned recipe:
     ```
     uv run --with icalendar==<pin> --with recurring-ical-events==<pin> python - <<'PY' …
     ```
     It prints events between two dates in the person's time zone, as `start–end | title | location`.
   - that raw `VEVENT` reading is a fallback only for non-recurring events;
   - that "Busy" entries without titles mean a busy-only publication;
   - that 404 means the link was unpublished or reset, so the agent should ask the person to reconnect with the new link.

   The lane pins versions that work on the sandbox's `python3`. It checks that PyPI is reachable from a default sandbox on the dev stack; if it is not, it stops and reports. The fallback is preinstalling the two libraries in `images/sandbox/Containerfile`, which is an ops-owned file, so that is the coordinator's call.
6. **Setup guide** (`docs/setup.md`, one section per method):
   - **Outlook (both):** Outlook on the web or new Outlook → Calendar → Settings → Calendar → Shared calendars → *Publish a calendar* → choose the calendar and how much to share → Publish → copy the **ICS** link (not the HTML one).
   - **Google:** Settings → the calendar → Integrate calendar → *Secret address in iCal format*.
   - **For each, say:**
     - The link is a password. Anyone who has it can read the calendar at the level chosen.
     - Choose the least detail that serves the person: busy only, titles and locations, or all details.
     - How to revoke: Unpublish in Outlook (publishing again makes a new link); *Reset* in Google. Then reconnect in Aurora with the new link.
     - Changes reach the link with the provider's delay. The lane measures what it can and states it, or says the provider doesn't document it.
     - If the *Publish a calendar* section is missing, the organization turned publishing off. Then only the `microsoft-365` app (with admin consent) can reach the calendar.

## Security (existing invariants; no new SEC)
- **Where the link lives:**
  - It is a `secret` field: never logged, cleared from form state after submit.
  - It is stored only as the credential `token` plus metadata.
  - It is never shown again: the connection card shows the label, provider and host.
- **Request log, approval summaries, traces (SEC-88) and gateway errors** carry the **placeholder** path, never the rewritten one. This must be proven by tests, not assumed.
- **The echo check covers the token** (FIX-104, SEC-01).
- **Injection** happens only on the declared host and placeholder path, within the connection's tenant host (SEC-04), and an agent-supplied path is overwritten, never merged (SEC-05).
- **Probe:** the hook's fetch goes through the destination guard and its declared egress only.

## Core gaps to check first (stop and report, never extend the core silently)
1. A `hook` method with `authorize: false` and form fields calls `connect.exchange` with `form(...)` on submit, and honours `tenant-host` and `label`.
2. The `tenant` guard accepts an exact host as its zone, so each host entry needs no wildcard.
3. A `path_regex` replacement may reference `metadata.*` and `credentials.*` together, and the linter accepts the narrow `path_prefix`.
4. `exec.string_values()` (the echo needles, `decide.rs` ~1259) covers credentials only, not metadata. If it covers metadata, the Outlook/Google bodies trip it, so report.
5. Outlook accepts `%40` for `@` in the mailbox segment (the template percent-encodes it), checked with the real link.
6. `tools[].connect_methods` works with `hook` methods.

If any one fails, the fix is a small core change, and it goes to the coordinator with a proposed decision before any code.

## Ownership
- **Owns:** `plugins/builtin/ics-calendar/**` (manifest, `docs/setup.md`, `assets/`, `hooks/` guest crate with its own `Cargo.lock`, `tests/*.yaml`).
- **Built-in list:** the place the built-in packages are listed (the lane finds it: packaging, `check-builtin.sh`, any snapshot or catalogue test fixtures).
- **Docs:** `docs/09` §10 / the built-in catalogue list only.
- **Touches nothing else** unless a core gap above is decided.

## Acceptance criteria
- [ ] `aurora plugin test plugins/builtin/ics-calendar` and `scripts/check-builtin.sh` pass. Fixtures:
  - placeholder → real path for each provider, with `%40` in the mailbox;
  - a request on another path of the host is not injected;
  - a cross-provider request is 421 `host_mismatch`;
  - an agent-supplied path segment is overwritten;
  - a response containing the token is 502 `upstream_echoed_credential`;
  - a response containing the mailbox/email is **not**.
- [ ] Hook fixtures (`http_cassette`):
  - each provider's valid link → expected credentials, metadata, tenant host and label;
  - `webcal://` normalized;
  - every error code: wrong host, wrong shape, 404, not a calendar;
  - an egress-denied case.
- [ ] Gateway or API IT on the dev stack: connect with a recorded link. A `GET` on the placeholder returns the ICS. The request log row, the trace spans and an approval-held summary show the placeholder path only (a canary token appears nowhere).
- [ ] Live check by the lane against the owner's throwaway links (not committed): each provider connects, reads, and after unpublish/reset answers 404 with the agent-facing message. Results go in the report, not in fixtures.
- [ ] Agent snippet ≤ 2,000 chars. The recurrence recipe, run in a default sandbox on a recorded Outlook ICS with a weekly series, an exception and a moved instance, lists the right instances in the person's time zone.
- [ ] The dashboard shows three method cards with their descriptions. The Give agents access step offers only the method's read tool.
- [ ] `cargo fmt --check`, clippy and the hook crate build; the registry linter. Web needs no change: if it seems to, report why.

## Dependencies
- Blocked by: — (D-273 written). The connect flow is the one FIX-149 turns into a dialog; this brief needs no change there.
- Numbers: D-273. No contract, no migration, no SEC.

## Suggested agent/model and reason
`rust-developer` (Sonnet): a WASM hook guest, manifest, fixtures and gateway ITs. The package directory is new and disjoint, so it can run in any free slot. It belongs with the apps work (`p5-apps` queue) unless a core gap needs the gateway, in which case it waits for that lane's slot.
