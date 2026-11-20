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
Lane `p5-agent` (branch `alexiri/p5-agent`), 2026-11-20.

**What was built**
- Contract first (commit `03280e78`): C-1052..C-1055 in `API_CONTRACTS` and the `/v1/me` row of `05`.
- `domains/people/turn_person.rs` (C-1052): one function for the turn's person, `{user_id,
  display_name, email, email_verified, timezone}`. Person speaker only on doors `web`/`api`/`thread`
  (`speaker_kind = person`); on a `schedule` turn with no speaker, the creator of the schedule named
  by `door.ref` when it belongs to the turn's agent; every other door has none. Then: not disabled,
  active org member, `authz::can_use` (the `effective_workspace_access` view, so SEC-138 counts).
- `domains/people/turn_context.rs` (C-1053): the block, called from arm 6 in
  `domains/hosted/dispatch.rs` after `delivered_text_of` and put in front of it; never stored. Eval
  turns get none; `TurnMessage` is unchanged.
- `domains/tools/calendar.rs`: `person_of` now calls the shared function and keeps `no_person` /
  `no_verified_email`. **Calendar is stricter**: an org member without `can_use` on the agent's
  workspace (or suspended there) is now `no_person` (intended; the calendar IT world binds its people
  to the workspace and a new assertion pins the refusal).
- `send_message` `to: "me"` (C-1055) in `domains/channels/messaging.rs::send_to_me`, before any
  other resolution; `Fail::Coded` carries `no_person` / `not_linked` to the tool error code. The
  tool description and the `to` schema text mention `me` (`domains/tools/messaging.rs`; the
  description lives there, not in `registry.rs`).
- Handle cache: `lookup_and_pin` (behind `find_recipient` and the exact-handle lookups of
  `send_message`/`read_conversation`) writes each non-bot `PlatformUser.handle` to its link
  (`user_links::set_handle`, only on a change).
- `PATCH`/`GET /v1/me` `timezone` (C-1054): `null` clears, absent leaves, not a `chrono-tz` name →
  422 `value_invalid` with nothing changed.
- Migration `20261201000259_person_timezone_link_handle.sql`: `users.timezone`, `user_links.handle`,
  both nullable, no default, no grant (the gateway reads `gateway_users`, which has neither).
- Web: Account › Profile time zone `Select` (the workspace settings' zone list, "Not set" → `null`);
  `useTimezoneAutofill()` in `AppShell` (`web/src/lib/timezone.ts`): one PATCH per browser session
  when `GET /v1/me` says `timezone: null` and the browser's zone is a valid IANA name.
- Docs: `03` (both columns), `04 §The turn's person` (SEC-139), `05`, `10 §Turn context` (+ the
  Schedules pointer), `12` (`to` forms), `13 §5.10` (`me`, handle cache), `14` (Profile time zone),
  `17` (SEC-139 row), `API_CONTRACTS`.

**Security (for the Orchestrator's review)**
- *The block can name no one but the turn's person.* The only identity line, `Working for`, is
  written from the `TurnPerson` that `turn_person::of_turn` returns, which is the turn row's own
  `speaker_user_id` (person speaker, person doors only) or the creator of the schedule the turn's
  `door.ref` names, re-checked as belonging to the turn's agent, and only while active, an org
  member and `can_use`. Nothing comes from the message text or anything the agent wrote; other
  values (schedule, webhook, space label, platform name) are names of things, not people, except the
  unlinked thread author's own `speaker_label`, which is shown as `Speaker:` with "(no Aurora
  account linked)" and is never a person. Every value passes `sanitize::one_line` with `<`/`>`
  removed and a 60/100-character clamp, so a display name or schedule name cannot close the tag or
  add a second `Working for` line (IT `values_cannot_forge_lines_and_the_block_is_bounded`).
- *No e-mail appears in a group space.* The e-mail is added only when `email_verified` **and**
  `group` is false, where `group` is true if the conversation's `kind` is `group` **or** the door's
  thread link (else the conversation's latest link) has `kind = group`. A web turn inside a group
  thread conversation is covered by the conversation kind (IT `thread_group_has_no_email`).
- *`me` can reach no one else.* `send_to_me` takes no recipient from the model: the target is
  always the `user_links` row of `(the turn's person, the resolved integration)`, i.e. that
  person's own platform user, sent as `Target::User` (their DM) or, when the current link is a
  `direct` link of a conversation whose `direct_user_id` is the person, that thread. The turn is
  looked up by the tool context's `turn_id` and must belong to the tool context's conversation;
  with no person → `no_person`, no link → `not_linked`; only `proactive_messaging = off` refuses
  outside the own thread. Every other `to` keeps the reach check and approval (the code path is
  unchanged after the `me` branch).

**Finding: where a schedule's final answer goes.** The brief says "posted to … when the origin
conversation has a thread link". In the code the run turn lives in the schedule's own run
conversation (`schedules/fire.rs::start_run`), and the doorless-turn mirror posts to the latest
thread link **of that conversation** (`channels/mirror.rs`), which a run conversation never has; the
origin gets only the `schedule_run` notice. So the answer is in practice never posted to chat, which
is the owner's lost wake-up call. The block therefore uses the mirror's own rule (the run
conversation's link) so it never tells the agent a false destination: today it says "shown only in
the dashboard", and the `me` sentence tells it how to reach the person (IT
`schedule_with_and_without_a_thread_link` pins both, including an origin with a link). `10
§Schedules` "the answer is posted there too" does not match the code; posting run answers to the
origin's thread would be a separate change (not done here).

**Handle cache, how often it fills.** Only when an agent's `user.lookup` happens to return the
person: `find_recipient`, or `send_message`/`read_conversation` with a handle or name. An agent
rarely looks up the person it is working for, so for a linked Mattermost user the cache is usually
empty until then. Proposal (follow-up): when a link is created (verified-e-mail autolink in
`inbox/speaker.rs`, sign-in link, admin link), queue one `user.lookup { user_ref }` op (the op
already supports `user_ref`) and write the handle from its result; that is one cheap op per link
for its lifetime, no platform call at dispatch.

**Other behaviour changes from the shared rule.** Eval turns have no person (C-1052), so
`calendar_invite_list` on an eval turn now answers `no_person` instead of listing the run starter's
invitations (the three write tools were already `eval_read_only`); `calendar_invitations::
eval_turns_cannot_send_invitations` asserts it. The fake harness echoes the whole delivered text, so
an echo now includes the block; no IT, script or Playwright spec asserts an echo (checked); the ITs
that compared `input.text` exactly (`hosted`, `chat_reply`) use the new `common::delivered_text`,
which also proves the block is there. The real-stack Playwright specs see one extra
`PATCH /v1/me { timezone }` per fresh session (harmless; nothing asserts on it).

**Verification** (all through `heavy.sh p5-agent`; ITs against a throwaway `postgres:16` container
on `127.0.0.1:55499` via `AURORA_TEST_DATABASE_URL`, never aurora-dev-pg; the container was removed
afterwards)
- `cargo test -p aurora-api --test it -- turn_person:: calendar_invitations:: chat_reply::
  hosted::end_to_end_turn_with_steer_credential_and_replay
  hosted::followup_is_promoted_when_the_turn_ends_unjoined`: first run 25 passed, 5 failed (four
  test-setup errors in the new file: duplicate thread link, one active turn per conversation, an
  81+-character display name; and the eval `calendar_invite_list` change above). After the fixes:
  `turn_person:: calendar_invitations::eval_turns_cannot_send_invitations
  foundation::openapi_matches_checked_in channels_messaging::` (with `UPDATE_OPENAPI=1`) **34
  passed, 0 failed**. The other calendar, `chat_reply` and `hosted` tests passed in the first run.
- `cargo test -p aurora-db --test it -- schema_matches_spec gateway_grants membership_queries`: 7
  passed.
- `cargo fmt --all --check`: clean (after `cargo fmt -p aurora-api -p aurora-db`).
- `cargo clippy -p aurora-api -p aurora-db --all-targets -- -D warnings`: clean.
- `python3 scripts/extract-schema.py --check`: up to date (fixture regenerated);
  `scripts/guards/run-all.sh`: all guards passed (after rewording one comment the
  no-integration-names guard caught).
- Web: `pnpm gen:api` (schema from the updated `openapi.json`), `pnpm typecheck`: pass; `eslint` on
  the five touched web files: clean; `vitest run tests/person-timezone.test.tsx tests/ssh.test.tsx`:
  2 files, 20 tests passed (the profile field saves and clears; the fill sends exactly one PATCH
  per session when null, none when set, none for an invalid browser zone).
- Not run (CI is the gate): full suites, Playwright.
