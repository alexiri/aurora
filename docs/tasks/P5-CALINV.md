# P5-CALINV: Calendar invitations — an agent puts an event in the person's calendar by sending them an invitation (owner, 2026-11-14, phase 5, D-274, C-844, SEC-108)

## Why
The owner wants agents to write to calendars without an OAuth app. For Outlook at CERN, getting one needs the Microsoft admins' consent; Graph has no other way in, and Exchange Online's basic auth is gone. ICS (P5-ICS) is read-only.

The agent therefore sends the **person** a standard calendar invitation: an iTIP `REQUEST` over email (iMIP, RFC 6047). Outlook, Gmail, Apple Calendar and Thunderbird all show it as a meeting invitation, and **Accept** puts it in the calendar.
- It needs no access to the calendar and no credential.
- It works with every provider.
- The person's click is the approval.

Aurora already has a mailer (`email.smtp_url`, `email.from`, SET-4), used for member invitations and approval emails.

## Scope

### 1. Agent tools (family **Calendar**, `12 §Phase 0 catalogue`)
The tools are available only while the instance mailer is configured (not log-only). Otherwise they are absent from the catalogue, not failing.
- `calendar_invite {title, start, end, timezone?, all_day?, location?, description?, repeat?, reminder_minutes?}` → `{ id, status: "sent" }`.
  - `start`/`end`: RFC 3339 local date-times plus `timezone` (an IANA name; default: the person's profile time zone, else the agent's `TZ`), or dates when `all_day`.
  - `end` must be after `start`, within 14 days of it, and the event within 5 years of now.
  - `repeat`: `{ freq: daily|weekly|monthly|yearly, interval?: 1–12, by_day?: [MO…SU], count?: 1–200, until?: date }`. `count` and `until` cannot both be set, and one of them is required; there is no endless series.
  - `reminder_minutes`: 0–10,080.
  - `title` ≤ 200, `location` ≤ 200, `description` ≤ 4,000 characters; plain text.
- `calendar_invite_update {id, …same fields…}`: sends a new `REQUEST` with the same `UID` and `SEQUENCE + 1`. The calendar updates the event, and the person may be asked to accept again (client-dependent).
- `calendar_invite_cancel {id}`: `METHOD:CANCEL`, same `UID`, `SEQUENCE + 1`. The calendar removes the event.
- `calendar_invite_list {}`: this agent's invitations to the current person (id, title, start, status), newest first, at most 50.

### 2. Who receives it (SEC-108)
The only recipient is **the person the turn runs for**:
- for a turn from a conversation, the person who sent the triggering message;
- for a scheduled turn, the schedule's creator;
- for a webhook or peer-agent turn, the tool refuses `no_person` (the lane names the existing field on the turn).

The recipient is always that person's **verified account email**. There is no `to`/`attendees` argument: the agent cannot invite anyone else, and the email never goes to an address the agent supplied. The person adds others themselves after accepting, if they want.

`update`/`cancel`/`list` act only on invitations the same agent sent to the same person.

### 3. The message
- **Envelope:** `From` is `email.from`, with display name `"<agent name> (Aurora)"`. `To` is the person. `Subject` is `"Invitation: <title> — <date, time, tz>"` (header-safe: CR/LF/NUL stripped, the title clipped).
- **Body:** a multipart body:
  - **`text/plain`**: a fixed template saying "**<agent name> proposes this event.** Accept it to add it to your calendar; Decline or ignore it otherwise." The fields are listed as plain text, and a footer names the Aurora instance and links to the agent's page. No HTML is built from agent text.
  - **`text/calendar; method=REQUEST; charset=utf-8`**: the iTIP part, inline, as calendar clients expect.
  - **The same calendar as an `invite.ics` attachment**, for clients that only import.
- **The calendar:**
  - It is written with a library, never by string concatenation: escaping of `,` `;` `\` and newlines, line folding, nothing an agent value can break out of.
  - `ORGANIZER;CN="<agent name> (Aurora)":mailto:<email.from>`.
  - `ATTENDEE;RSVP=FALSE;PARTSTAT=NEEDS-ACTION:mailto:<person>`.
  - `UID` = `<invitation id>@<instance host>`, plus `SEQUENCE`, `DTSTAMP` and a `VALARM` when asked.
  - Time zones: a single event uses UTC times. A repeating event uses `TZID` with a generated `VTIMEZONE` for its zone, so daylight-saving changes keep the local hour.
- **The description** carries "Proposed by <agent> in Aurora — <link to the conversation>" on its first line. The agent's description follows as plain text.

### 4. State (migration `20261201000230`)
- **Table `calendar_invitations`:** `id`, `org_id`, `workspace_id`, `agent_id`, `person_id`, `conversation_id?`, `uid`, `sequence`, `status` (`sent`|`cancelled`), `title`, `starts_at`, `ends_at`, `timezone`, `repeat` JSONB, `created_at`, `updated_at`.
- **Not stored:** neither the description nor the location; they are only in the sent email.
- **Retention:** rows follow the agent's retention (FIX-111) and are removed on agent, person or org deletion (FIX-097's completeness).
- **Audit:** every send, update and cancel is an audit event `calendar.invitation.{sent,updated,cancelled}` with ids only.

### 5. Limits and controls
- **Rate limit:** at most 20 sends (any of invite, update, cancel) per person per agent per hour and 60 per person per day across agents. Over the limit → `rate_limited` with the retry time.
- **Org switch:** an org setting `calendar_invitations.enabled` (default on) lets org admins turn the tools off. Where the org has a settings page, it shows there (SET-*).
- **Mailer failure:** → `mail_unavailable`, with no row written as sent.

### 6. Replies
The person's **Accept/Decline** goes to `email.from`, because the organizer is Aurora. `RSVP=FALSE` asks clients not to send one, and most still offer "Don't send a response". Aurora has no inbound mail, so replies are not read. Operator docs (`15`) say `email.from` should be an address that accepts or discards mail. Reading replies (to show "accepted" to the agent) is out of scope.

### 7. Docs
- `12`: the tool family.
- `10`: what the agent can and cannot do.
- `15`: `email.from` replies and the setting.
- `14`: the org setting, if added to a settings page.
- `05`: no HTTP route.
- `17`: SEC-108 (written by the coordinator).

## Live verification (owner-run, recorded in the report)
The dev stack's mailer is log-only, so the lane builds and tests against a captured SMTP (for example a local MailHog/smtp4dev container in the IT, or the existing test mailer capture).

Then, on an instance with a real mailer, **the owner** checks with their CERN Outlook (Exchange Online) and a Gmail address:
- the invitation shows as a meeting invite, not as junk;
- Accept adds it at the right local time, including a weekly repeat across a daylight-saving change;
- update moves it;
- cancel removes it;
- the agent's text cannot add lines to the event.

Where Exchange treats an external organizer specially (for example "tentative" placement off for external senders), the result goes in the report and in `docs/setup`-style help text.

## Acceptance criteria
- [ ] **Unit:**
  - calendar generation for single, all-day and repeating events;
  - escaping and folding, with a `title` containing `\r\nATTENDEE:mailto:x@y`, `;`, `,`, and 300 characters;
  - a `VTIMEZONE` for `Europe/Zurich` with a repeat across the October/March changes;
  - a `SEQUENCE` increment;
  - `CANCEL`;
  - argument validation (each bound).
- [ ] **API IT with captured SMTP:**
  - one message to the person's verified email, never another address;
  - `text/plain` with the fixed preface;
  - an inline `text/calendar; method=REQUEST` part and the attachment;
  - `update` keeps the `UID` and increments `SEQUENCE`;
  - `cancel` sends `METHOD:CANCEL`;
  - `list` shows only this agent's invitations to this person;
  - another agent's id → `not_found`.
- [ ] **IT, refusals:**
  - a webhook turn → `no_person`;
  - a person without a verified email → `no_verified_email`;
  - the mailer unset → tools absent from the catalogue;
  - the org switch off → absent;
  - the 21st send in an hour → `rate_limited`;
  - a mailer error → `mail_unavailable`, no row.
- [ ] **Deletion and retention:** an agent, person or org deletion removes the rows; retention prunes; audit events carry ids only.
- [ ] **Header injection:** a CR/LF in `title` never creates a header.
- [ ] `cargo fmt --check`, clippy, the api unit and IT suites for the touched modules; `extract-schema` and guards if the catalogue schema is generated.

## Ownership
- API `domains/tools/` (new `calendar.rs` + registration in `builtin.rs`/`registry.rs`).
- The mailer module (one new send function).
- `crates/aurora-db` (migration `20261201000230`, a repo module).
- Retention and deletion hooks (FIX-097/111 code paths).
- The org setting (if SET-* has an org settings model, use it; otherwise a column on the org settings row, in the same migration).
- The docs above.

Not the gateway, the sandbox image or web, except an optional settings toggle.

## Numbers
D-274, C-844, SEC-108, migration `20261201000230`.

## Completion report
**Status 2026-11-18 (lane `p5-agent`, worker 28): DONE, awaiting the coordinator's merge and the owner's live check.** Branch `alexiri/p5-agent`.

**Built.** Migration `20261201000230_calendar_invitations.sql` (`calendar_invitations`, plus the rate-limit ledger `calendar_invitation_sends`); repo `aurora-db::repo::calendar_invitations`; the four tools in `domains/tools/calendar.rs` with an in-house typed iCalendar writer `calendar_ics.rs`; the mailer's calendar part (`Mail.calendar`, `Mail.from_name`, `build_message`: multipart/mixed = plain + inline `text/calendar; method=…` + `invite.ics`); registry support (`needs_mailer`, `ToolError::Coded`, `catalogue(&AppState, …)`); the org switch (`orgs.settings.calendar_invitations.enabled`, PATCH `settings.calendar_invitations_enabled`); retention in the daily pass; audit `calendar.invitation.{sent,updated,cancelled}`; the docs (03, 02 limits rows, 04, 05, 10, 12, 15; 14 not changed, see below). Tests: 23 unit (`calendar_ics`, `calendar`, `mail`), 12 IT in `tests/it/calendar_invitations.rs`.

**Decisions and adaptations (read these).**
- **Crate vs writer: in-house writer, no new dependency** (no iCalendar crate in the workspace; so no `cargo deny` or `Cargo.lock` change). It is a typed writer (name, params, value; one `escape_text`, one `param_value`, closed formatters for dates, offsets and `mailto:`; folding once at 75 octets, never inside a UTF-8 sequence), not string concatenation; the injection corpus (CR/LF, `\nATTENDEE:`, `END:VEVENT`, `;`, `,`, `\`, NUL, 300 characters) is a unit test. `VTIMEZONE` is generated from `chrono-tz` offset changes (a base observance plus one component per distinct change with `RDATE`s, from the year before the start to 30 years after), tested for Europe/Zurich across the October and March changes. A series longer than 30 years (yearly, count above 30) falls back to the last observance in the client.
- **No profile time zone exists** for a person: the default zone is the **workspace's** (`workspaces.timezone`), as schedules do.
- **Tool error codes**: `ToolError` got a fourth variant `Coded(code, message)`; validation keeps the existing `invalid_arguments` (C-844 says `invalid_argument`; the message starts with the field). `rate_limited` carries `retry_after_secs=N` in its message (the wire error has no details).
- **Mailer flag**: `PlatformTool::needs_mailer()`; the live flag is `state.auth_live.smtp`, checked in `catalogue(state, agent)` (signature changed from `(pool, agent)`; six test call sites updated) and in `Registry::call`. `email.from` is read from the live settings snapshot at call time; unset → `mail_unavailable`.
- **The person**: the turn's `speaker_user_id`, else (door `schedule`) the schedule's `creator_user_id` of this agent's schedule; they must be an active org member and not disabled (else `no_person`), with `email_verified_at` set (else `no_verified_email`).
- **Rate limits** use the ledger table (a row per send, advisory lock per person, check + insert + send inside one transaction, so a mailer failure rolls the ledger entry back and uses no budget).
- **Update needs the full event again** (the description and location are not stored, so a partial update could not rebuild them). Updating or cancelling a cancelled invitation is refused.
- **All-day events**: `start`/`end` are dates and `end` is the **last day, inclusive** (`DTEND` is written exclusive); the rows hold those days at 00:00 UTC plus `all_day` (an added column, not in C-844's list). `by_day` is accepted for weekly repeats only; `until` must be within 10 years of the start (`count` is capped at 200 as specified). A timed local value may also be an RFC 3339 value with an offset (converted to the zone).
- **The link in the description and the footer is the agent's dashboard page** (`/o/<org>/w/<ws>/agents/<handle>`): the dashboard has no conversation URL to link.
- **Retention** (the brief says "the agent's retention (FIX-111)", which is the daily prune): single or cancelled rows 90 days after their end or last change, repeating rows 5 years after the last change, ledger rows after two days; constants in `limits.rs` with docs/02 rows. Deletion is by cascade (agent, person, organization; the org case is tested through the real `DELETE /v1/orgs/{org}`).
- **The org switch has no dashboard toggle** (the brief calls it optional): `OrgSettings` gained `calendar_invitations_enabled` (so `web/openapi.json` and `schema.d.ts` were regenerated; no UI change), and docs/14 is unchanged. A toggle belongs with the P5-IA org settings page.
- **P5-IA/SET-1..5 check**: `email.from`/`smtp_url` are SET-4 live settings (read through the live snapshot, not `state.config`); the tools doc is `12-supervisor-harness.md`; the audit-actions list is in `04`. FIX-160: no runtime SQL is built from strings (all statements are constants; the repo's `{COLS}` template goes through `sqlx::AssertSqlSafe`).

**Gates run.** Per task: `cargo fmt --all --check` ok; `cargo clippy -p aurora-api -p aurora-db -p aurora-core --all-targets --all-features -- -D warnings` ok. Batch: `cargo test -p aurora-core -p aurora-db` (79 + 8 + 43 passed, schema fixture IT included); `cargo nextest run -p aurora-api` (fixtures built first): 1642 tests, 1641 passed, 1 timed out under load (`permission_matrix::permission_matrix_routes_by_principal`, 600 s with the machine loaded), **rerun alone: passed (280 s)**; `cargo clippy --workspace --all-targets --all-features -- -D warnings` ok; `bash scripts/guards/run-all.sh` exit 0 (it first failed on the four new limits constants having no docs/02 row; rows added); `python3 scripts/extract-schema.py --check` up to date (fixture regenerated); `UPDATE_OPENAPI=1` + `pnpm -C web gen:api`; web `typecheck` ok, `lint` 0 errors (5 existing warnings in files not touched), `test` 2323 passed, `build` ok; Playwright with `AURORA_E2E_WEB_PORT=4183 AURORA_E2E_WEB_PROD_PORT=4184 AURORA_E2E_WEB_TRACING_PORT=4192`: `--project mocked --project tracing` 1267 passed (stage4-axe included), `e2e:prod` 11 passed; `bash scripts/e2e/upgrade.sh` 36 passed, 0 failed.

**Fail-before.** The feature is new (the tools did not exist), so no test could pass before it; I did not run a mutation check beyond that.

**Process slip.** One edit to `home.rs` (the `catalogue(&state, …)` argument) was made with `sed` instead of Edit, a one-line change; later fixed with Edit (clippy's needless borrow).

**Left for the owner's live check** (not done: the dev mailer is log-only; the IT uses the in-memory capturing mailer, and lettre's message building is unit-tested for the multipart layout and header injection): CERN Outlook (Exchange Online) and Gmail: shows as a meeting invitation not junk; Accept lands at the right local time including a weekly repeat across a daylight-saving change; update moves it; cancel removes it; an agent's text cannot add lines. Results go into `15 §Calendar invitations`. Note `Content-Type` is multipart/mixed as C-844 says; some clients prefer multipart/alternative, if Outlook shows only an attachment, switch the inline part there.

### Handoff notes of worker 27 (kept for the record; all of it is resolved above)
**Status 2026-11-17 (lane `p5-agent`, worker 27): NOT STARTED — handoff notes only** (the lane's context ran short after FIX-165 and FIX-162). Findings from reading master (`e307e7aa` on `alexiri/p5-agent`), so the next worker does not redo them:
- **Spec is already merged:** C-844 (`docs/API_CONTRACTS.md` ~1338), D-274, SEC-108 exist. Migration `20261201000230` is still unused (master has 231…250; 230 is a gap), as the brief says.
- **No iCalendar crate** is in the workspace deps or the cargo registry cache; `chrono-tz` 0.10 and `lettre` 0.11 are. "A library, never string concatenation" therefore means either adding one (a `Cargo.lock` change: ask the coordinator) or a small typed in-house writer (properties as name + params + value, escaping and 75-octet folding in one place, `VTIMEZONE` from `chrono-tz` transitions). The unit tests in the brief apply either way.
- **Mailer** (`domains/auth/mail.rs`): `Mail { to, subject, body, link }` is plain text only; `SmtpMailer::send` builds one plain body. Needs an optional calendar part (method + ics) and a multipart/mixed message (plain, `text/calendar; method=…`, `invite.ics` attachment); `MemoryMailer` already captures whole `Mail`s for the IT. "Mailer configured, not log-only" is the live `smtp` flag in `domains/settings/live.rs` (SET-4), not `config.email`.
- **Tools** (`domains/tools/`): `PlatformTool::available(&ToolContext)` is sync and `ToolContext` has only the pool, so the smtp flag cannot be read there; the async `enabled()` also takes `ToolContext`. Either add the flag to `ToolContext` or check it in `enabled()` through a state handle. `ToolError` has only `invalid_arguments | tool_unavailable | tool_failed`: C-844's codes (`no_person`, `no_verified_email`, `rate_limited`, `mail_unavailable`, `not_found`) go as a message prefix unless the enum is widened. Register in `builtin.rs`.
- **Eval read-only (C-868):** add `calendar_invite`, `calendar_invite_update`, `calendar_invite_cancel` to `WRITE_TOOLS` in `tools/registry.rs` (list is a read).
- **The person:** `turns::get(..).speaker_user_id` (as `tools/schedules.rs:124`). A schedule-fired turn has `speaker_user_id: None` (`schedules/fire.rs:106`) but its message door is `{"kind":"schedule","ref":<schedule id>}`: resolve the schedule's creator from that. Webhook and peer-agent turns → `no_person`. Verified email: check the users repo for the verified flag.
- **Org switch:** org settings are the JSONB `orgs.settings` read by `OrgSettings::from_json` (`tenancy/dto.rs`: `previews_enabled` is the pattern); C-912/P5-WEBHOOK-2 say webhooks' org switches are "stored and edited like `calendar_invitations.enabled`", so design it to be reused. The web org settings page was not checked against P5-IA.
- **Rate limits:** 20 per person per agent per hour and 60 per person per day: count `calendar_invitations` rows by `updated_at`/`created_at` or add a counter; cancelled sends count too (every send), so a row per send is not enough: count audit events or keep a small ledger.

## Suggested agent/model and reason
`rust-developer` (Sonnet). Calendar generation, mail and API tools; small, but the escaping and recipient rules are security-relevant.
