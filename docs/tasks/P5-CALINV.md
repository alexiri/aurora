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
