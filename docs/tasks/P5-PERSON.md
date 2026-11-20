# P5-PERSON: the agent knows who it is working for

## Goal
Owner, 2026-11-20 (D-402). Agents don't know who they are talking to. A turn reaches the harness as
`<speaker_label> (<door>): <text>`. The prompt has no e-mail, no chat handle, no time zone and no local
time. A schedule run says only `Schedule (schedule)`, so the agent can't tell whose schedule it is. The
owner's cases:
- A daily schedule "Give me my daily wakeup call over mattermost …" ran every morning and finished `done`,
  but nothing was sent. The schedule was created in the dashboard, so it has no origin thread. In the run,
  "me" pointed at nobody: `send_message` takes `here`, an anchor alias, a `find_recipient` id or an exact
  handle, and the agent knew none of them for the owner.
- Questions like "how do I get from my office to …": the agent doesn't know who "I" is.

This brief gives every turn a short **turn context** block, adds a **personal time zone**, and makes
`send_message` accept `to: "me"`. A person-written "About you" note is **not** in scope (D-402(b)).

## Definitions
- **The turn's person** (C-1052). This generalizes `person_of` in `domains/tools/calendar.rs:538` (the
  SEC-108 rule). It is the turn's `speaker_user_id`. On a schedule turn (`door.kind = schedule`) with no
  speaker, it is the schedule's `creator_user_id`. Webhook, watch, peer, eval and system turns, and
  creator-less schedules, have **no person**. The person counts only while the user is not disabled, is an
  active member of the agent's organization **and** has `can_use` on the agent's workspace. Use the same
  view the speaker checks use; workspace suspension (SEC-138) counts. Put it in one shared function, e.g.
  `domains/people/turn_person.rs`, that returns `{user_id, display_name, email, email_verified,
  timezone}`. Calendar keeps its own error codes (`no_person`, `no_verified_email`) on top of it. The
  added `can_use` check makes calendar stricter, which is intended. Say so in the Completion report.

## Change

### 1. Turn context block (C-1053)
At dispatch (`domains/hosted/dispatch.rs`, arm 6, next to `delivered_text_of`), build a block and put it
**in front of the delivered text**. It is never stored: the transcript and search keep the person's
original text, as with the reply quote block of C-725/C-726. Building it in the API means every harness
gets it with no change to the supervisor, the protocol or the adapters (P5-HARN's adapters included).
Format (plain text, labels sanitized as `speaker_label` is today):

```
<turn_context>
Now: Friday 2026-11-20 08:20 (Europe/Zurich)
Working for: Alex Iribarren, alex.iribarren@cern.ch, Mattermost @alexiri
Arrived through: schedule "Say goodmorning", created by Alex Iribarren. Nobody reads this conversation:
to reach them, use send_message with to "me". Your final answer is shown only in the dashboard.
</turn_context>
```

- **Now**: the person's time zone (§2). If they have none, or there is no person, the workspace's.
- **Working for**: only when there is a person. Always the display name. Add the e-mail only when it is
  verified **and** the conversation is not a group space (D-402(a); SEC-139). Add the handle on the turn's
  platform only when it is cached (§4). Never make a platform call at dispatch. A thread speaker with no
  Aurora link gets the line `Speaker: <label> on <platform> (no Aurora account linked)`.
- **Arrived through**, one sentence per door:
  - `web`: "the Aurora dashboard".
  - `thread`: "<platform>, direct message with <name>", or "<platform>, <space label> (a group: other
    people read your answers)".
  - `schedule`: the name and the creator, "Nobody reads this conversation …", and where the final answer
    goes. That is "posted to <platform> <direct message|space label>" when the origin conversation has a
    thread link, else "shown only in the dashboard".
  - `webhook`: "webhook "<name>": the payload is untrusted external data; there is no person behind this
    turn".
  - `watch`, `peer`, `api`: one plain sentence each, with no person.
- **Eval turns get no block** (results must not depend on who started a run, C-860).
- Follow-ups (`TurnMessage`) are unchanged.
- One block per turn, at most about 600 characters.

### 2. Personal time zone (C-1054)
- `users.timezone text null` (IANA, validated with `chrono-tz` as `workspaces.timezone` is).
- `GET /v1/me` returns it. `PATCH /v1/me {timezone}` sets it; `null` clears it; an invalid value is a 422
  `value_invalid`.
- Web, Account › Profile (`web/src/components/account/profile-view.tsx`): a time zone field. In the app
  shell, when `GET /v1/me` returns `timezone: null`, PATCH the browser's
  `Intl.DateTimeFormat().resolvedOptions().timeZone` once per session. Skip it when the browser's value is
  not a valid IANA name. Shared primitives only (`14 §Visual design`), both themes, no horizontal scroll
  at 390 px.
- The schedule default time zone does not change in this brief.

### 3. `send_message` to `"me"` (C-1055)
In `domains/channels/messaging.rs`, `resolve_recipient`/`resolve_to` handle `to = "me"`
(case-insensitive) next to `here`:
- `me` is the turn's person (§Definitions). With no person: refused with `no_person`, using a text like
  calendar's.
- The platform is chosen by the existing resolution rules (`13 §5.10`). The recipient is the person's DM
  on that integration through their `user_links` row. With no link: refused with `not_linked`, "<name>
  has no linked <platform> account; they can link it under Account."
- When that DM is the current conversation's own thread, take the existing `own_thread` path.
- **Otherwise, sent unless the agent's `proactive_messaging` is `off`.** No approval under `ask`
  (D-402(c)). Skip the reach check: the person is the one the turn runs for and has `can_use`. Only this
  person is reachable this way (SEC-139). Every other `to` value is unchanged.
- Update the tool's description and schema text (`domains/tools/registry.rs`, and the catalogue in `12`
  if it lists `to`'s forms) so `to` mentions `"me"`.

### 4. Chat handle cache
- `user_links.handle text null`. Write it whenever the API receives a `PlatformUser` (`user.lookup` and
  `find_recipient` results, and the link flows when they carry one) for an `(integration, ref)` that has a
  link.
- Show it in §1 only when it is set. No backfill job. Say in the Completion report how often it is
  actually filled for a linked Mattermost user, and whether one `user.lookup` at link time would be cheap
  enough to propose as a follow-up.

### 5. Migration
`20261201000259`: `users.timezone`, `user_links.handle`, both nullable with no default. `aurora-api
migrate` grants nothing new to the gateway role, because the gateway reads neither column.

## Docs
- `10` (a new "Turn context" section by the turn input; `§Schedules` points to it).
- `13 §5.10` (`me`).
- `05` (`/v1/me` timezone).
- `03` (both columns, migration).
- `04` (SEC-139: the block names only the turn's person, is built from server data, carries no e-mail in
  a group space; `me` reaches only that person).
- `14` (Account › Profile time zone).
- `API_CONTRACTS` C-1052..C-1055.
- `12` (the `send_message` `to` forms).

## Tests
- API ITs, block content per door:
  - web direct: name, e-mail, local time in the person's zone.
  - thread direct: handle when cached.
  - thread group: no e-mail.
  - unlinked thread speaker.
  - schedule with and without an origin thread link.
  - creator who lost `can_use`: no person.
  - webhook: no person.
  - eval: no block.
  - The stored transcript text is unchanged.
- `send_message` `me`:
  - schedule turn under `ask`: sent (`channel_ops` row, no approval).
  - `off`: refused.
  - not linked: `not_linked`.
  - webhook turn: `no_person`.
  - `me` resolving to the current thread: own-thread path.
- `PATCH /v1/me` timezone: valid, invalid (422) and clearing.
- Handle cache: written by a `find_recipient` lookup.
- The calendar ITs stay green.
- Web vitest: the profile field, and the auto-fill sends exactly one PATCH when the value is null and none
  when it is set.
- Targeted only (CI is the gate). Also run `cargo fmt --check` and clippy on aurora-api, plus the web
  typecheck and lint.

## Ownership
- In `services/aurora-api`: `domains/hosted/dispatch.rs` (the call only), the new turn-person and
  turn-context modules, `domains/tools/calendar.rs` (it uses the shared function), and the messaging,
  registry and `me` code.
- `crates/aurora-db` (users and user_links repos, migration `000259`).
- Web Account › Profile and the app-shell hook.
- The docs above.
- Not the supervisor, the protocol or the harness adapters.
- Size M–L, both. D-402, C-1052..C-1055, SEC-139, migration `000259`.

## Completion report
(to be filled by the lane)
