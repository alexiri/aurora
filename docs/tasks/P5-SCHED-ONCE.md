# P5-SCHED-ONCE: One-time schedules ("run at"), reminders that reach the person, a yearly preset (owner, 2026-11-20)

## Goal

Owner, 2026-11-20 (D-421, D-422). Every schedule today repeats: `cron` is a 5-field expression
(`services/aurora-api/src/domains/schedules/cron.rs:23`, `croner` 4.0.3, which could parse 7 fields but we
refuse them). People want:

- **"27 November 2027 at 10"**: a schedule that runs **once**.
- **"Every year on 27 November at 10"**: already possible as `0 10 27 11 *`, but only by typing cron. The
  P5-REFINE-14 presets have no yearly option.
- **"Remind me to do X tomorrow morning"**, said to the agent. Agents already have `schedule_create`
  (`domains/tools/schedules.rs`), but only with a cron. A reminder becomes `0 9 21 11 *` and fires again
  every 21 November. The run happens in a separate run conversation "nobody reads". For a person who only
  uses the web chat, the answer shows up only as a "Scheduled task … finished" notice in the chat where
  they asked, with no notification.

The owner chose an explicit **run at** time over 7-field cron. A year-pinned cron is still a one-time
schedule, and the engine needs one-time rules anyway: what happens after the last fire, and what happens
when the agent is paused or busy at that moment (today both skip to "the next occurrence", which would
silently lose a reminder). A model also writes `2026-11-21T09:00` far more reliably than
`0 0 9 21 11 * 2026`. Cron stays 5 fields.

## Decisions that bind this brief

- **D-421** One-time schedules: `run_at` instead of `cron` (exactly one of them); run once, then **Done**,
  removed 30 days later; late rather than lost when paused or busy; future and at most 5 years ahead; no
  5-minute minimum; Done ones don't count toward the 50-per-agent limit; the agent tools take `run_at`, and
  the tools' time zone defaults to the person's.
- **D-422** A one-time schedule created in a person's own chat **runs in that chat**, so the reminder
  appears where they asked and the agent has the conversation's history. That person gets a Web Push
  when the answer is ready. Every other run keeps its own run thread (P5-REFINE-7).
- **SEC-144** A schedule run is created in a direct conversation only when its owner is the schedule's
  current creator.
- Contracts **C-1092..C-1095**. Migration **`20261201000266`**.
- Binding from earlier briefs: D-402 (turn context, C-1052..C-1055), D-407 (words), D-409 and C-1074/C-1077
  (one thread per run, `next_runs`, preview, `RunResult`), RFE-021 (anchor, reassignment), C-1023 (push
  claim pattern), FIX-192 (chat-platform origins).

## Order and lanes

- **Part A** (engine, API, tools) and **Part B** (where a one-time run happens, push) are Rust, in one
  lane. They edit `domains/schedules/**`, `repo/schedules.rs` and the conversation turn creation, which
  **P5-REFINE-7 rewrites** (per-run threads, busy rule, `next_runs`, preview). They start **after
  P5-REFINE-7 has merged**: lane E after -7. The Orchestrator places it relative to P5-REFINE-9.
- **Part C** (web) starts **after P5-REFINE-14** (the Automations page and schedule presets) and
  **P5-REFINE-11** (the transcript's schedule message renderer) have merged, and after Parts A and B.
- Size: A M, B M, C S–M. Commit A+B, then C. Tests move with the code in the same commit.

## Read first

- `docs/10-agents-conversations.md` §Schedules (:895), §Turn context (:217).
- `docs/12-supervisor-harness.md` :808 (the schedule tools row).
- `docs/05-control-plane-api.md`: the schedule rows and §Web Push (:711); the push delivery row (:370).
- `docs/API_CONTRACTS.md`: C-1074, C-1077 (P5-REFINE-7), C-1023 (the admin-attention push you copy),
  C-1052..C-1055, and this brief's **C-1092..C-1095**.
- `docs/tasks/P5-REFINE-7.md` items 2 and 5, `docs/tasks/P5-REFINE-14.md` items 2 and 3.
- Code (line numbers before P5-REFINE-7, which moves them):
  - `domains/schedules/{service,fire,cron,dto,routes}.rs`: `fire_due` :158 (the frozen branch :147, the
    coalesced check :188), `start_run` :66, `on_turn_finished`.
  - `domains/automations/service.rs:121`: the reassign `re_enable` path calls `cron::validate`.
  - `domains/hosted/home.rs:110`: `schedules_summary`.
  - `domains/tools/schedules.rs`.
  - `domains/tools/calendar.rs:190–220`: local date-time parsing and the DST-gap rule to copy.
  - `jobs.rs` `PruneReport` (the daily retention pass).
  - `crates/aurora-db/src/repo/schedules.rs` (`COLS`, `insert`, `update`, `record_fire`, `claim_due`,
    `count_for_agent`).

## Part A: one-time schedules (D-421, C-1092, C-1094)

1. **Migration `20261201000266_schedule_run_at.sql`** (C-1092):
   - `schedules.cron` drops NOT NULL.
   - New columns `run_at timestamptz NULL` and `completed_at timestamptz NULL`.
   - `CHECK ((cron IS NULL) <> (run_at IS NULL))` and `CHECK (completed_at IS NULL OR run_at IS NOT NULL)`.
   - `last_outcome` CHECK also allows `waiting_paused`.
   - `schedule_runs.answer_pushed_at timestamptz NULL` (Part B).
   - A partial index on `schedules (completed_at) WHERE completed_at IS NOT NULL`.
   - `eval_schedules` is not touched.
   - Update `docs/03` §schedules / §schedule_runs in the same commit; `extract-schema.py --check` must pass.
2. **Parsing (`schedules/once.rs`, new).**
   - `run_at` is a **local date-time without an offset**, `YYYY-MM-DDTHH:MM` or `YYYY-MM-DDTHH:MM:SS`,
     read in `timezone`.
   - An offset or `Z` → 422 at `/run_at`: "a local date and time such as 2026-11-21T09:00, without an
     offset; set timezone".
   - An ambiguous local time (clocks go back) takes the **earlier** instant. A time inside a DST gap → 422
     "does not exist in that time zone (a daylight-saving gap)". Copy the rules of
     `tools/calendar.rs`; don't edit that file.
   - Bounds:
     - not earlier than the **start of the current minute** ("in the past" otherwise; a time in the
       current minute fires at once);
     - at most `SCHEDULE_ONCE_MAX_AHEAD` (5 years, `aurora_core::limits`) ahead.
   - No minimum interval.
3. **Create and update (`service.rs`, `routes.rs`, `dto.rs`).**
   - `CreateScheduleRequest` and `UpdateScheduleRequest` take `run_at?: string`. **Exactly one** of `cron`
     / `run_at` on create; a patch may carry at most one of them.
     - Both on create → 422 at `/run_at` "set either cron or run_at, not both"; neither → 422 at `/cron`.
   - `next_fire_at` is the instant.
   - A patch with `run_at` makes the schedule one-time (`cron` NULL, `completed_at` NULL, re-armed if
     enabled). A patch with `cron` makes it recurring (`run_at` NULL, `completed_at` NULL).
   - A patch of `timezone` alone on a one-time schedule **keeps the local wall time** and recomputes the
     instant, re-validated: 9:00 stays 9:00 in the new zone.
   - `enabled` on a **Done** schedule → 409 `schedule_completed` ("This schedule has already run. Set a new
     time to run it again."). A new future `run_at` re-arms it.
   - Resuming a pending one-time schedule whose time has passed (e.g. after an `authorization` disable and a
     reassignment) arms it for **now**.
     - The same rule applies in `automations/service.rs` `reassign_schedule` (`re_enable`). Done stays
       Done.
   - The per-agent limit (`PER_AGENT_MAX`, 50) counts only rows with `completed_at IS NULL`.
   - `Schedule` DTO:
     - `cron: string | null`;
     - `run_at: string | null` (the local wall time in `timezone`, same format as the request, so it
       round-trips);
     - `completed_at: string | null` (RFC 3339 UTC);
     - `next_runs` (C-1077) is `[instant]` for a pending enabled one-time schedule, `[]` when Done or
       paused.
   - Preview (C-1077, `POST …/schedules/preview`) also takes `{ run_at, timezone? }` (exactly one of `cron`
     / `run_at`). It returns `next: [instant]`, `interval_secs: null`, `too_frequent: false`, or the field
     errors above as `invalid_run_at`.
4. **Firing (`fire.rs`), a one-time row only.** The recurring path is unchanged.
   - **Agent effectively paused:**
     - no turn; the schedule is neither disabled nor completed;
     - a `schedule_runs` row `waiting_paused` is written **once** (not again while
       `last_outcome = 'waiting_paused'`) and `last_outcome = 'waiting_paused'`;
     - `next_fire_at = now + SCHEDULE_ONCE_RECHECK` (60 s), so it fires within a minute of Resume.
     - Add `waiting_paused` to C-1077's `RunResult` table as `skipped` with `reason: "paused"`.
   - **Fire-time authority fails:** exactly as recurring (disabled `authorization`, origin notice). Not
     completed.
   - **Otherwise it fires and is never `skipped_busy`.** Where the turn goes is Part B. A run thread
     (C-1074) has no busy conflict for a single run.
     - In the same transaction: `next_fire_at = NULL`, `completed_at = now`, `last_fire_at = now`,
       `last_outcome = 'ok'`.
     - A gate (`gate_command`) works as for recurring runs; a quiet or failed gate still completes the
       schedule.
     - `consecutive_failures` and `SCHEDULE_FAILURE_DISABLE` never disable a one-time schedule. The
       failure notice is still posted.
   - No coalesced row.
   - **Run now** on a one-time schedule is a manual run in its own run thread. It changes neither
     `run_at` nor `completed_at`.
5. **Turn context (`10 §Turn context`, C-1093).** For a one-time run the `schedule` sentence says:
   - that it is a **one-time** schedule due at `<local time> (<zone>)` and will not run again;
   - when it fires more than 5 minutes after `run_at`: "running late: <agent> was paused at the time";
   - who reads it and where the answer goes (Part B).

   Values stay server data, one line, clamped.
6. **Agent home summary (`hosted/home.rs`).**
   - A pending one-time schedule is listed as ``name: once at `2026-11-21 09:00` (Europe/Zurich)``.
   - Done ones are left out (the repository query filters them).
   - The sanitising rules (FIX-110) stay.
7. **Retention.** The daily `retention_prune` pass deletes Done schedules with
   `completed_at < now − SCHEDULE_DONE_RETENTION` (30 days), batched (`drain`).
   - It deletes through the **same path as `service::delete`**, so P5-REFINE-7's "thread of a deleted
     automation" marking happens.
   - `PruneReport.schedules_done`.
8. **Agent tools (C-1094, `tools/schedules.rs`).**
   - `schedule_create {name, prompt, cron?, run_at?, timezone?, gate_command?, gate_description?}`:
     - exactly one of `cron` / `run_at`, with the API's messages;
     - `schedule_update` takes `run_at` too;
     - `view()` adds `run_at` (local) and `completed_at`; `cron` may be null.
   - **Time zone default for both tools:** the speaker's own zone (`users.timezone`, C-1054), else the
     workspace's. It is the same zone as "Now" in the turn context, so "tomorrow at 9" means the person's
     9:00. This changes the default for cron schedules created by the agent; say so in the completion
     report.
   - Description of `schedule_create` (keep it this short):

     > Wake yourself later with `prompt`. For a reminder or anything that happens once, set `run_at` to a
     > local date-time such as 2026-11-21T09:00 (in `timezone`, default: the person's, the zone of Now
     > above); it runs once. For repeating work set `cron` (5 fields, e.g. `0 10 27 11 *` = every year
     > on 27 November at 10:00; at most every 5 minutes). A one-time schedule made in a person's own chat
     > runs in that chat, so your answer reaches them there. Optional gate_command runs first; empty
     > output or {"wake_agent": false} skips the run.

   - Creating still needs a person speaking. A schedule run has no speaker, so it cannot schedule itself
     again. Keep the existing test.

## Part B: where a one-time run happens, and the push (D-422, C-1093, C-1095, SEC-144)

1. **Target conversation.** A one-time run is created **in the schedule's origin conversation** when all
   of these hold at fire time:
   - `origin_conversation_id` is set and the conversation exists;
   - it belongs to the same agent;
   - it is not archived (P5-REFINE-7);
   - its `direct_user_id` **equals the schedule's current `creator_user_id`** (SEC-144: after a
     reassignment to someone else, the run never lands in the old owner's chat).

   Otherwise the run gets its own run thread exactly like a recurring run, and FIX-192 keeps posting to a
   chat-platform origin thread.
2. **In the origin.**
   - The run turn uses the schedule door (`{kind:"schedule", ref}`, system speaker "Schedule", the
     "Scheduled task: <name>" header, P5-REFINE-7's `display` payload).
   - If a turn is active there it **queues behind it** (follow-up), never skipped.
   - `schedule_runs.turn_id` is that turn, and C-1077's `ScheduleRun.conversation_id` is the origin.
   - The "Scheduled task … finished." `schedule_run` notice is **not** appended for a run that happened in
     the origin (the answer is already there). Failure notices stay.
   - The turn context's `schedule` sentence for this case: "<creator> set this one-time schedule in this
     conversation and reads it; your answer is shown here" (instead of "nobody reads the run").
3. **Push (C-1095).** When such a run's turn ends `done`, the creator's opted-in devices get **one** Web
   Push. Copy C-1023's loop and claim:
   - the claim is `schedule_runs.answer_pushed_at`;
   - a run older than 1 h is never pushed;
   - payload `{ "v": 1, "type": "schedule_answer", "url": "<the conversation's dashboard path>" }`, plus
     `title` (the schedule name, `NAME`-capped) only on a device with `show_details`;
   - never the answer text;
   - `Urgency: normal`, `TTL` = `PUSH_TTL_MAX`, `Topic` per C-1023.

   No push for recurring runs or for run-thread runs. It is sent even if the person has the chat open
   (simpler; acceptable).

## Part C: web (after P5-REFINE-14 and -11)

1. **Presets.** In P5-REFINE-14's **When** row, add **Every year** and **Once**:
   `Every 5 minutes · Hourly · Every day · Weekdays · Weekly · Every year · Once · Custom`.
   - **Every year:** day + month + time → cron `M H D Mo *`.
   - **Once:** date (default tomorrow) + time (default 09:00), sent as `run_at`.
   - Both use the same time-zone select.
   - Summary lines:
     - "Every year on 27 November at 10:00 · next: 27 Nov 2027 (Zurich time)";
     - "Once, on Sat 21 Nov at 09:00 (Zurich time)".
   - A past time shows "Pick a time in the future" as you type (preview).
   - Editing an existing yearly cron opens on Every year; a `run_at` schedule opens on Once.
2. **States on the row (D-407 words).**
   - Pending: "Once · Sat 21 Nov at 09:00 (Zurich time)", badge **On** / **Paused** as P5-REFINE-14.
   - Waiting: "Waiting: <Agent> is paused. It runs when <Agent> is resumed."
   - Done: badge **Done**, "Ran on Sat 21 Nov at 09:00 · removed on 21 Dec". Actions: **Run again**
     (opens the form on Once with a new date) and Delete. No Pause/Resume. **Run now** stays.
   - Done schedules count in no "On" count or badge.
   - Add the words to P5-REFINE-1's glossary / words test if it checks them.
3. **Transcript.** The run in the person's chat renders with P5-REFINE-11's schedule message renderer. Check
   it at 390 px in both themes.
4. **Service worker (C-835).** Handle `schedule_answer`:
   - "Aurora — a scheduled task has an answer for you" / "Open the conversation to read it.";
   - with `title`: "Aurora — <title>";
   - `tag` `aurora-schedule`; it may open exactly the conversation path it was given.
5. `docs/14` §Visual design applies: tokens, shared primitives, both themes, no horizontal scroll at 390 px.

## Acceptance

1. **Create and validate (API IT `tests/it/schedules.rs`):**
   - `run_at` tomorrow 09:00 Europe/Zurich → 201, `cron: null`, `next_runs` = that instant.
   - Both fields / neither / an offset / a past time / 6 years ahead / 02:30 on the spring-forward day →
     422 with the field and message.
   - 01:30 on the fall-back day → the earlier instant.
   - The 50 limit ignores Done rows.
   - A `timezone`-only patch keeps 09:00 local.
   - `enabled` on Done → 409 `schedule_completed`; a new `run_at` re-arms it.
   - `cron` ↔ `run_at` switches both ways.
   - Preview with `run_at`.
2. **Fire:**
   - due → one run, the schedule is Done in the same transaction, never fired again;
   - paused agent → one `waiting_paused` row across several ticks, then fires within 60 s of Resume, and
     the turn context says it is late;
   - unauthorized → disabled `authorization`, not Done; after a reassignment and resume it fires at once;
   - a quiet gate completes it;
   - Run now leaves `run_at` and `completed_at` alone;
   - recurring schedules behave exactly as before (existing tests unchanged).
3. **Where it runs (SEC-144):**
   - created by the tool in the creator's direct chat → the run turn is in that chat, queued behind an
     active turn, with no "finished" notice;
   - reassigned to another member → its own run thread, nothing in the old owner's chat;
   - origin archived or deleted → run thread;
   - created in a group/space conversation → run thread + FIX-192 mirror unchanged
     (`schedule_run_answer_is_posted_to_the_origin_thread` passes).
4. **Push:** one push per opted-in device of the creator for an origin run that ended `done`, none on a
   replay, none for a run thread or a recurring run, never the answer text; `title` only with `show_details`.
5. **Tools:**
   - `schedule_create` with `run_at` and no `timezone` uses the speaker's `users.timezone`, else the
     workspace's;
   - refused without a speaker (a schedule turn);
   - `view` carries `run_at` and `completed_at`.
6. **Retention:** a Done schedule older than 30 days is removed by the pass and its threads are marked as
   from a deleted automation; younger ones and pending ones stay.
7. **Web (vitest; Playwright mocked spec for the Automations page):**
   - Every year and Once build the right request;
   - the summaries and the Done / Waiting rows render;
   - Run again;
   - no horizontal scroll at 390 px;
   - the service worker test for `schedule_answer`.
8. **Docs in the same commits:**
   - `03` (columns), `05` (schedule rows, preview, push loop row);
   - `10` §Schedules (one-time rules, where a one-time run happens) and §Turn context;
   - `12` :808 (tool signatures);
   - `07` §Web Push (the new type);
   - `14` (presets, states);
   - `15` (the three limits);
   - `docs/tests/security-coverage.md` (SEC-144 row with its proving test);
   - `web/openapi.json` and `schema.d.ts` regenerated; `extract-schema.py --check` and
     `scripts/guards/run-all.sh` pass.
9. **Gates (targeted, through `heavy.sh`; CI is the full gate):**
   - `cargo fmt --check`;
   - `cargo clippy -p aurora-api -p aurora-db -p aurora-core --all-targets -- -D warnings`;
   - `cargo test -p aurora-api --test it -- schedules automations channels_inbox turn_person push
     retention tools_schedules permission_matrix` (names as they exist);
   - unit tests of `once.rs`;
   - web: vitest on the touched files, `tsc`, lint.

## Out of scope

- 7-field cron, seconds, an end date for recurring schedules, "snooze" buttons. Snoozing is the person
  replying in the chat, which is a speaker turn and may create a new one-time schedule.
- Moving recurring runs into the origin chat.
- Push for recurring runs.
- E-mail reminders.
- Eval schedules.

## Completion report

Files changed, test commands and results, and anything left undecided. Always name:

- the tools' time-zone default change;
- whether the push loop shares C-1023's sweeper or runs its own;
- the conversation path format used in `url`;
- any P5-REFINE-7 / -11 / -14 code this brief had to bend.

### Parts A and B (lane E, `p5-platform`, 2026-11-20)

**Done:** Parts A and B. **Waiting:** Part C (web: presets, row states, transcript check, service
worker `schedule_answer`), lane B after P5-REFINE-11 and -14. The service worker does not handle
`schedule_answer` yet; until Part C ships, such a push shows the worker's generic fallback, if it
has one.

**SEC-144: who can schedule what, for whom.**
- *Who creates a reminder for whom.* `schedule_create` needs a person speaking in the current turn
  (`turns.speaker_user_id`); that person becomes `creator_user_id` and the turn's conversation
  becomes `origin_conversation_id`. An agent cannot name another creator or another conversation
  (no such arguments; `deny_unknown_fields`), and `anchor` is always `creator`. Through the API the
  caller is the creator and there is no origin.
- *Why a run cannot reach another person.* At fire time (`repo::schedules::origin_for_run`) a
  one-time run goes into the origin only when it is a `direct` conversation of the same agent, not
  archived, whose `direct_user_id` equals the schedule's **current** `creator_user_id`. Anything else
  (reassigned, group or space conversation, archived, deleted, API-created) gets its own run thread.
  The fire-time authority check (`automation_authorized`) still runs first. A schedule turn has no
  speaker, so it can neither create a schedule nor make a run schedule itself again (tested). Origin
  notices of every schedule (finished, failed, disabled) now skip a direct chat whose person is not
  the current creator, so after a reassignment nothing is written into the old owner's chat (a
  creator-less schedule keeps its notices; a deleted creator's chats are deleted with them).
- *Push audience.* The claim query (`claim_answer_push`) re-checks that the run's turn is in the
  schedule's origin, a `direct` conversation whose `direct_user_id` is the schedule's current
  creator, and pushes only to that person's deliverable devices (live session, servable key).
- *Bounds of `run_at`.* Refused before the start of the current minute and more than
  `SCHEDULE_ONCE_MAX_AHEAD` (1 827 days) ahead; an offset or `Z` is refused; a daylight-saving gap is
  refused; an ambiguous time is the earlier instant. A Done schedule never fires again
  (`next_fire_at` null) until a new future `run_at`; a paused agent's run waits with a 60 s recheck
  and writes one `waiting_paused` row, not one per tick.
- *Lock screen.* The payload is `{v, type: "schedule_answer", url}` plus `title` (the schedule name,
  one line, capped at 80 characters) only on a device with `show_details`. Never the prompt or the
  answer (the IT puts a canary in the prompt and checks both payloads).

**Brief questions.**
- *Time-zone default of the tools:* changed for **both** kinds. `schedule_create` without `timezone`
  now uses the speaker's `users.timezone` (if a valid IANA name), else the workspace's; before, a
  cron made by the agent used the workspace's. `schedule_update` reads a new `cron` / `run_at` without
  `timezone` in the speaker's zone too (with no speaker the schedule keeps its zone).
- *Push loop:* it shares the Web Push loop (`push::deliver::spawn`, every 10 s and on approval
  events), as its own sweep `sweep_schedule_answers` next to `sweep_attention` (C-1023) and
  `sweep_app_setup`; claim column `schedule_runs.answer_pushed_at`, driven by the new
  `schedules_completed_idx` (only schedules Done within the last hour are read).
- *`url`:* `/o/<org slug>/w/<workspace slug>/agents/<agent handle>?c=<conversation id>` (the agent
  page's deep link, D-102).
- *P5-REFINE-7 code bent:*
  - `fire::start_run` takes `busy_rule`; a one-time fire passes `false` (never `skipped_busy`); Run
    now and recurring fires keep the busy rule.
  - A follow-up's waiting: a schedule run queued behind a person's active turn is a `joining`
    follow-up. Arm 7 (`repo::hosted::steer_candidates`) now skips follow-ups with a `schedule` door,
    so it is never steered into the person's turn. When that turn ends, `promote_one` re-points
    `schedule_runs.turn_id` at the promoted turn (`fire::on_followup_promoted`) and gives its
    `turn.queued` event the schedule's `display`.
  - `on_turn_finished` and `gate_result` no longer write a one-time schedule's row (failure count,
    `last_outcome`): it is already Done, and a run in the origin holds that conversation's lock while
    a fire takes the schedule row first, so writing it would invert the lock order. Failure notices
    are still posted.
  - Origin notices skip a direct chat that is not the current creator's (above): this also applies
    to recurring schedules.

**Decisions taken here (please confirm in review).**
- Field errors of create/update are **400 `invalid_request`** at `/run_at` (or `/cron`), like every
  other schedule field error (`cron`, `timezone`); the brief said 422. The preview keeps its 422
  codes (`invalid_run_at`, `invalid_cron` when neither is given). Recorded in `05` and C-1092.
- Done schedules keep `enabled = true`; Done is `completed_at`. `enabled` (true or false) on a Done
  one is 409 `schedule_completed`. Offboarding lists and the group impact count leave Done ones out.
- A `timezone`-only patch on a **Done** schedule moves the stored instant to the same wall time
  without the past check and without re-arming it.
- `run_conversation_id` of an origin run is the origin conversation.

**Files.** Migration `crates/aurora-db/migrations/20261201000266_schedule_run_at.sql`;
`crates/aurora-core/src/limits.rs` (`SCHEDULE_ONCE_MAX_AHEAD`, `SCHEDULE_ONCE_RECHECK`,
`SCHEDULE_DONE_RETENTION`); `crates/aurora-db/src/repo/{schedules,hosted,automations,groups}.rs`;
`services/aurora-api/src/domains/schedules/{once (new),service,fire,dto,routes,results,mod}.rs`,
`automations/service.rs`, `conversations/service.rs`, `hosted/home.rs`,
`people/turn_context.rs`, `push/deliver.rs`, `tools/schedules.rs`, `jobs.rs`; docs `02`, `03`,
`05`, `07`, `10`, `12`, `API_CONTRACTS` (C-1092 error status), `tests/security-coverage.md`
(SEC-144 row), `FEATURES`, `PROGRESS`; regenerated `web/openapi.json`, `web/src/lib/api/schema.d.ts`,
`crates/aurora-db/tests/fixtures/spec_schema.json`. `14` (presets, states) is Part C's.

**Tests.** New `services/aurora-api/tests/it/schedules_once.rs` (6 ITs:
`one_time_create_validate_patch_and_preview`,
`a_one_time_schedule_fires_once_waits_while_paused_and_resumes_late`,
`a_quiet_gate_completes_and_a_failure_never_disables`,
`a_reminder_runs_in_the_creators_chat_queued_and_pushed`,
`after_reassignment_or_archive_the_run_gets_its_own_thread`,
`retention_removes_done_schedules_after_thirty_days`); unit tests `schedules::once` (4), `results`
(the `waiting_paused` row), `push::deliver::schedule_answer_payload_is_fixed_and_names_only_on_request`,
`hosted::home::a_one_time_schedule_reads_once_at_its_local_time`. Changed: `tests/it/schedules.rs`
(`row.cron` is an `Option`), `tests/it/main.rs`. No test removed or loosened; no web test touched (no
web source changed besides the generated client; the existing schedule mocks still type-check).

**Commands (all through `heavy.sh p5-platform`).**
- `cargo test -p aurora-api --lib -- schedules:: push::deliver hosted::home`: 26 passed.
- `cargo test -p aurora-api --test it -- schedules_once:: schedules:: turn_person:: automation_continuity:: channels_inbox::schedule_run_answer_is_posted_to_the_origin_thread web_push:: admin_attention:: freeze:: tools:: permission_matrix::`:
  83 passed, 1 failed (the FIX-192 test, whose fixture schedule has no creator: the first notice
  guard skipped it; narrowed to "a creator is set and differs"), then
  `-- schedules_once:: channels_inbox::schedule_run`: 7 passed.
- `UPDATE_OPENAPI=1 cargo test -p aurora-api --test it -- openapi_matches_checked_in`: passed;
  `pnpm gen:api`; `pnpm typecheck`: clean.
- `cargo fmt --all --check`: clean. `cargo clippy -p aurora-api -p aurora-db -p aurora-core
  --all-targets -- -D warnings`: clean (after naming one tuple type).
- `python3 scripts/extract-schema.py --check`: up to date; `bash scripts/guards/run-all.sh`: all
  guards passed.
- Playwright production specs: none affected (no UI change).
