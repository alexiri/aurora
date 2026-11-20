# P5-REFINE-4: Runner and computer backend: the stream-port bug, sessions that keep the computer awake, readiness in words, Sleep now (owner, 2026-11-20)

## Goal

UX review 5 found one real bug and four gaps in what the platform tells people about their agent's computer. This brief is the
**Rust side only**. It adds the web `lib/api` client and types for the new fields and no UI (P5-REFINE-11 and -15 build the screens).

1. **The bug (review "The bug found on the way").** Turning the terminal on with `helm upgrade` leaves Terminal, Files and SSH dead
   until the runner restarts. The runner decides **once**, from the answer to its **first** registration, whether to open its
   stream port (:4108). The new runner registered with the old API pod, got "off", and never asked again. Terminal looped
   "Reconnecting… (attempt 4 of 4)", Files said "The computer restarted; retrying…" forever, SSH answered
   `exec request failed on channel 0`. Owner decision **D-416**: the runner re-decides.
2. **An open terminal does not keep the computer awake (computer-1).** With a command running in an open terminal the 30-second idle
   sleep stopped the computer and killed the command; the host name changed in the same scrollback. Administration says an open
   terminal keeps the computer awake for up to 4 hours. Today the runner stamps keep-awake only when a session **carries bytes**
   (`stream_listener.rs` `supervisor_terminal`), so a quiet command does not count. **D-416**: an open terminal or SSH session counts
   as activity, as the setting promises; if a restart still happens the terminal says so (the dashboard prints it; the API gives it
   the facts).
3. **The header cannot tell "ready" from "running" (D-408, chat-5 and the status rail).** The agent header must say "Starting its
   computer…", **Ready only when it answers**, "Asleep", "Working". Today `state = running` is true before the supervisor connected
   and the harness reported ready, and the Computer page's start log stays on `waiting_supervisor` after the computer woke
   (computer-7).
4. **A member sees the engine (computer-7, D-412).** Computer › Technical details shows the image name, the runner's cluster address
   and IP, `waiting_supervisor` and "home volume" to a person who only uses the agent. **D-412**: people see the computer's phases
   in words (Off, Starting, On, Stopping, Failed), the raw phase as a secondary line at Expert, and the sandbox image and runner
   address are **instance-admin only**.
5. **No Sleep now, no "sleeps in" (computer-13).** An awake computer offers only Restart.
6. **Two Files facts (computer-3, computer-11).** Move to… needs no new route (the API's rename already moves). Text files over 1 MiB
   show "No preview for this file"; the page should show the first part.

Decisions that bind this brief: **D-405** (this set, precedence), **D-408** (the header follows the computer's readiness), **D-412**
(phases in words; image and runner address for instance admins only), **D-416** (runner re-decides; open sessions keep the computer
awake; a restart prints a notice).

## Linked requirements

- **The review:** <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>. Workers cannot open artifacts; read the local copy
  `/home/alex/orca/workspaces/aurora/design-ux-review-5/` (written `UXR5/` below). `UXR5/findings.md` is the authoritative text;
  `UXR5/priorities-themes.md` has "The bug found on the way".
- **Findings** (each: artifact anchor `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#f-<key>`; After renders
  `UXR5/after/<key>.png` and `UXR5/after/<key>-lt.png`; markup `UXR5/after/src/<key>.html`; Before crop(s)
  `UXR5/before/<key>-b1.png`; main screenshots `UXR5/screens/<id>.png`):
  - `computer-1` (the open terminal keeps the computer awake; the restart notice) · After `computer-1.png`, `computer-1-lt.png`,
    `src/computer-1.html` · Before `computer-1-b1.png`, `computer-1-b2.png` · Screens `C134-terminal-longrun.png`,
    `C135-terminal-interrupted.png`, `C136-terminal-after-wake.png`, `A201-agents-computers-after-terminal.png`.
  - `computer-2` (the bug's face: stop after the last retry; the dashboard side is P5-REFINE-15, the runner side is here) · After
    `computer-2.png`, `computer-2-lt.png` · Before `computer-2-b1.png`, `computer-2-b2.png` · Screens `C103-terminal-failed.png`,
    `C106-terminal-after-wait.png`, `C111-files-after.png`.
  - `computer-6` (SSH: `aurora ssh daneel`, certificate expiry; the designer's note on handles) · After `computer-6.png`,
    `computer-6-lt.png` · Before `computer-6-b1.png`, `computer-6-b2.png` · Screens `C112-ssh.png`, `C117-ssh-cert.png`,
    `C161-ssh-active.png`.
  - `computer-7` (a member sees "Started 23:55 · on"; the engine's details are admin only and current) · After `computer-7.png`,
    `computer-7-lt.png` · Before `computer-7-b1.png` · Screens `C050-computer-technical.png`, `C051-computer-technical-full.png`.
  - `computer-13` (Sleep now, an idle countdown, a waking spinner) · After `computer-13.png`, `computer-13-lt.png` · Before
    `computer-13-b1.png`, `computer-13-b2.png` · Screens `C048-computer-waking.png`, `C049-computer-awake.png`.
  - `computer-3` (Move to…: **no API change**, see Scope 6) · After `computer-3.png`, `computer-3-lt.png` · Before `computer-3-b1.png`,
    `computer-3-b2.png` · Screens `C150-row-menu.png`, `C151-rename.png`, `C153-move-by-rename.png`.
  - `computer-11` (first part of a big text file) · After `computer-11.png`, `computer-11-lt.png` · Before `computer-11-b1.png`,
    `computer-11-b2.png` · Screens `C149-preview-big.png`, `C147-preview-csv.png`.
  - Header and status wording for D-408: `chat-5` (After `chat-5.png`; Screens `B033-agent-actions-menu.png`) and `chat-4` (After `chat-4.png`:
    "the header goes back to Ready at once").
- **The bug:** `UXR5/priorities-themes.md` "The bug found on the way" and the artifact section
  <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#bug>. Layer-above rec 3 (the status rail) and rec 11 (Computer: Files first,
  phases in words, Technical details admin-only): `UXR5/layer-above.md`, anchor `…#above`.
- **Decisions:** D-405, D-408, D-412, D-416; amended: D-178 Q-R5-5 (who sees the start detail), C-583, C-611, C-1046.
- **Contracts:** **C-1057**, **C-1058**, **C-1059**, **C-1060**. The drafts are in
  `docs/API_CONTRACTS.md` › “Phase 5 — P5-REFINE” (merged with the briefs, 2026-11-20)
  (section P5-REFINE-4). **The four entries are already in `docs/API_CONTRACTS.md`; if the code needs a different shape, correct the entry there first** (create the heading
  "Phase 5 — P5-REFINE" if it is not there yet, otherwise add only your entries).
- **Security:** **SEC-141** (the draft row is in the same file under "Security"; add it to `docs/17-security.md`, and the coverage
  row to `docs/tests/security-coverage.md`).
- **Normative docs to read:** `05` (rows for `GET /v1/agents/{agent}/sandbox`, `…/sandbox/wake`, `…/restart`, the runner plane),
  `11 §Web terminal` and `§Stop` (keep-awake reasons), `12 §Terminal`, `15` (runner config), `16 §W17–W20` (stream and PTY frames),
  `docs/04-*` §SSH access.

## Wave and order

- Wave 1, **lane E (Rust)**: **4** → 7 → 9. Starts immediately, in parallel with 1, 2, 3 and 5.
- No migration. No dependency on other briefs. P5-REFINE-11 (header) and P5-REFINE-15 (Computer, terminal, SSH) wait for this.
- Disjointness (lane E owns these while this brief runs): `services/aurora-runner/src/{runner.rs,api.rs,stream_listener.rs,channel/mod.rs}`,
  `crates/aurora-protocol/src/runner.rs`, `services/aurora-api/src/domains/runner_plane/{routes,service,work}.rs`,
  `services/aurora-api/src/domains/hosted/{routes,dto}.rs`, `services/aurora-api/src/domains/terminal/mod.rs`,
  `services/aurora-api/src/domains/files/{routes,service,dto}.rs`, `crates/aurora-db/src/repo/{sandboxes,hosted}.rs`, `web/src/lib/api/conversations.ts`.
  `crates/aurora-protocol` is shared with the supervisor and the runner: keep every change additive (`#[serde(default)]`).
- `web/openapi.json` and `web/src/lib/api/schema.d.ts` are regenerated in this change (`pnpm gen:api`).

## Context

Paths from the repository root; line numbers are master `c0fe7d10`.

### The runner and the stream port (Scope 1)

- `services/aurora-runner/src/runner.rs`
  - `run` :129; builds `RegisterRequest` and calls `api.register` **once** in a retry loop :165; then
    `self.hub.set_previews_enabled(cfg.previews || cfg.terminal || cfg.ssh_enabled)` :184 and `set_ssh_enabled(cfg.ssh_enabled)` :185.
    **These are the only two places the flags are ever set.**
  - the work poll loop :251–287 (`api.work(&req)` :259; `Ok(resp)` :266 submits `resp.items`).
  - `s.terminal` (the runner's own wiring, `cfg.terminal.enabled || cfg.terminal.available`) feeds `caps.terminal` :104–108; `caps.ssh = true` :111.
- `services/aurora-runner/src/channel/mod.rs`: `set_previews_enabled` :491 (comment: "Never un-flipped"), `previews_wait` :497 (waits for
  `previews_on` or stop), `set_ssh_enabled` :529, `ssh_enabled()` :534, the `ChannelHub` impl :635–641.
- `services/aurora-runner/src/stream_listener.rs`: `serve` :750; it calls `state.server.previews_wait(&mut stop).await` :757 and only
  then accepts on :4108. So a runner whose flag was off at registration never opens the port.
- `services/aurora-runner/src/api.rs`: `register` :175, `work` :184 (the poll; `WorkResponse` is read in `once`).
- `crates/aurora-protocol/src/runner.rs`: `RunnerConfig` :227 (`previews`, `terminal`, `ssh_enabled`, all `#[serde(default, skip_serializing_if)]`),
  `RegisterResponse` :250, `WorkResponse` :287 (`items`, `reconciler`).
- `services/aurora-api/src/domains/runner_plane/service.rs`: `register` :19; the three flags are computed at :73–86 from
  `s.config.previews.enabled`, `s.ops().terminal` (`enabled || available`, and `req.capabilities.terminal`) and `s.config.ssh.enabled && req.capabilities.ssh`.
- `services/aurora-api/src/domains/runner_plane/work.rs`: `poll` :56 returns `Ok(WorkResponse { items, reconciler })` :86; `caps_of(runner)` :89 gives the capabilities
  the runner registered (so the API knows what it advertised); `compute` :115.
- Tests that exist: `services/aurora-api/tests/it/runner_plane.rs`, `runner_rig.rs`; runner `services/aurora-runner/tests/it/runner.rs` (wiremock rig: `register_response`
  :69, `mount_full` :147, `registers_executes_start_and_stop_reports_and_retires` :252).

### Keep-awake by open sessions (Scope 2)

- `services/aurora-runner/src/stream_listener.rs`
  - `supervisor_terminal` :357 delivers PTY bytes and stamps through `stamp_awake` only when the chunk was queued; `supervisor_stderr` :377 does the same for SSH;
    `stamp_awake` :400, `stamp_due` :412 (one stamp per grace per `(sandbox, ssh)`, map `last_stamp` :151/:227).
  - legs are registered in `register_route` :578 and released in `release_pty_as` :703; the terminal leg opens in `open_supervisor_pty` :1206.
- `services/aurora-runner/src/api.rs` `keep_awake` :290 posts `/runner/terminal/keep-awake` (`{ sandbox_id, kind? }`).
- `services/aurora-api/src/domains/terminal/mod.rs` `keep_awake` :30 → `crates/aurora-db/src/repo/sandboxes.rs` `keep_terminal_awake` :274 / `keep_ssh_awake` :295
  (`terminal_active_until = now() + grace`, cap clock `terminal_awake_since`).
- The stop arm: `claim_stops` :125; `TERMINAL_KEEP` :180 (`terminal_active_until > now()` and under the cap); `SSH_KEEP`.
- `crates/aurora-core/src/limits.rs`: `TERMINAL_AWAKE_GRACE` :608 (120 s), `TERMINAL_AWAKE_CAP` (4 h), `SSH_AWAKE_GRACE`.

### The computer view (Scopes 3 and 4)

- `services/aurora-api/src/domains/hosted/dto.rs`: `SandboxView` :46 (state, `boot_id`, `runner_id`, `image`, `started_at`, `keep_awake_reasons`, failure,
  `start_attempt`, `retryable`, `start_log`, processes, ports, `preview_in_use`, `terminal_in_use`, `restart_to_update`, `idle_stop_after_secs`),
  `SandboxStartAttempt`, `StartLogEntry`.
- `services/aurora-api/src/domains/hosted/routes.rs`: `can_see_start_detail` :96 (instance admin **or workspace owner** today), `reduce_view` :104 (drops
  image, runner_id, attempt detail/events, log detail, swaps the failure message for `readiness::member_hint`), `view` :127, `view_full` :132
  (reasons built :161–178; `image` from the runner's advertised capabilities), `get_sandbox` route, `wake_sandbox` :407, `retry_sandbox`, `restart` :528.
- `services/aurora-api/src/domains/hosted/readiness.rs`: `member_hint` :39 (the member sentences by failure code), `unreachable_member_message` :69.
- `crates/aurora-db/src/repo/hosted.rs`: `SandboxViewRow` :739 (reads `start_phase`, `start_detail`, `running_at`…; **add `last_engaged_at`, `last_active_at`**),
  `record_harness` :40 (sets `start_phase = 'ready'`), `request_restart` :725.
- `crates/aurora-db/src/repo/sandboxes.rs`: `PHASE_WAITING_SUPERVISOR` :537; the **`running` report** :1031–1037 writes `start_phase = 'waiting_supervisor'`
  unconditionally; the supervisor-connected update :1300–1305 writes `booting` unless already `ready`. If the supervisor connects first, the later `running` report
  overwrites `booting`/`ready` with `waiting_supervisor`: that is the stale "waiting" of computer-7.
- Web: `web/src/lib/api/conversations.ts` `SandboxView` :34 (a hand-written type; `chatApi.sandbox` :111, `restart`, `retry`, `wake` :116),
  `web/src/components/agents/computer-tab.tsx` (Technical details :324, `Image: {v.image}` :327), `web/src/components/agents/agent-state.tsx`
  (`AgentState`; today the header's word comes from the model check and the conversations only).

### Sleep now and Files (Scope 5, 6)

- `claim_stops` :145 `WHEN s.stop_requested_at IS NOT NULL THEN coalesce(s.stop_reason, 'idle')`: the column exists, is cleared at every start (:457, :890) and
  **nothing sets it today**. `stop_reason` accepts `'idle'` (CHECK in `20261113000006_freeze.sql`). The stop order is **before** the active-turn check, so Sleep now must refuse
  while work runs.
- `services/aurora-api/src/domains/files/routes.rs`: `file_text` :65 (`GET /files/text`), `file_ops` :213; `files/service.rs`: `text` :753
  (`FILES_EDIT_MAX` :562 → `422 file_not_editable` reason `too_large` :770), `rename` :1208 (**already takes a full destination path and moves across folders**; it refuses
  moving a folder into itself).
- SSH: `services/aurora-api/src/domains/ssh/service.rs` `mint_certificate` :186; `SshCertificate` has `valid_before`, `username`, `host`, `port`
  (`ssh/dto.rs`); the CLI `services/aurora-cli/src/ssh.rs` resolves a handle already (`resolve_agent` :143); the terminator requires a UUID user name
  (`services/aurora-terminator/src/cert.rs` :93–95).

## Scope

1. **The runner re-decides at every poll (C-1057; the bug).** Findings: `computer-2` (and the bug section). Anchor
   `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#bug`.
   - API: `WorkResponse` gains `streams: Option<StreamsConfig { previews, terminal, ssh_enabled }>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`).
     `work.rs poll` fills it on **every** call from the same rules as `register` (factor the three expressions of `service.rs` :73–86 into one function that both call,
     so registration and the poll can never disagree), using `caps_of(runner)` for what the runner advertised.
   - Runner: after each successful poll (`runner.rs` :266) apply `resp.streams` when present: `hub.set_previews_enabled(p || t || s)` and `hub.set_ssh_enabled(s)`.
     `previews_wait` already wakes when the flag flips on, so the :4108 listener opens the first time it is on. Update the stale comments ("Never un-flipped", "Called
     once the runner has registered") and `16 §W18`/`15` in one sentence each.
   - Never open the port earlier than today: a runner that gets no `streams` (older API) behaves exactly as now.
   - The dashboard half of the bug (a terminal that loops "Reconnecting…" forever) is P5-REFINE-15: it stops after the last retry and says what failed. The API cannot know
     whether a runner's port is open, so **do not add a probe here**. In the completion report say whether an existing close code (4502, 4503, 4504 of the terminal leg) is enough
     for the dashboard to say "Couldn't reach the computer".
2. **An open terminal or SSH session keeps the computer awake (C-1058; computer-1).** Anchor `…#f-computer-1`.
   - In `stream_listener.rs`, start a **heartbeat task per open terminal or SSH leg** when it is registered (`register_route` :578 for legs with `terminal` or `ssh`
     set) that calls `stamp_awake(sandbox, ssh)` every `TERMINAL_AWAKE_GRACE / 2` (60 s; use the matching `SSH_AWAKE_GRACE` for SSH) and ends when the leg ends
     (`release_pty_as` :703, every close reason, and when the channel dies). Reuse `stamp_due` so a byte-triggered stamp and the heartbeat never double-post. A leg that is
     idle for hours past the cap simply stops mattering: the API's cap clock (`terminal.awake_cap_secs`) ends the reason; do not add a runner-side cap.
   - Files legs keep their own stamp (`files_active_until`, API-side, C-900): unchanged.
   - Restart notice: no API field (C-1058). Make sure `SandboxView.boot_id` is present for every caller (it is; `reduce_view` does not touch it) and that a terminal
     reconnecting after a restart gets a **new** `boot_id`. Add a test of that.
3. **The computer in words (C-1059 first half; D-408, D-412).** Findings `computer-7`, `computer-13`, header words of `chat-4`/`chat-5`. Anchors `…#f-computer-7`, `…#f-computer-13`.
   - Add `computer: ComputerView { word, ready, working, asleep, phase, sleeps_in_secs, started_at }` to `SandboxView` exactly as C-1059 says. Build it in
     `view_full` from the row (`state`, `start_phase`, `keep_awake_reasons`, `failure_code`, `last_engaged_at`/`last_active_at`, `idle_stop_after_secs`). `word` and
     `ready` are one small pure function with a unit test table (every `state` × `start_phase` × failure).
   - `sleeps_in_secs`: `None` unless `word = on`, no keep-awake reasons and idle stop is on; else `max(0, idle_stop_secs − quiet)` where quiet is
     `now − coalesce(last_engaged_at, last_active_at, created_at)` (the same clock as `ENGAGED_AGE` in `sandboxes.rs` :23). It is a **hint**: the stop arm decides.
   - **Fix the stale phase.** In the `running` report (`sandboxes.rs` :1031–1037) write `waiting_supervisor` only when the current `start_phase` is not `booting` or
     `ready` (a `CASE`, as :1305 already does the other way). Test: supervisor connects, then the `running` report is applied; the phase stays `booting`/`ready`.
4. **Who sees the engine (C-1059 second half; SEC-141; D-412).** Finding `computer-7`.
   - `can_see_start_detail` (:96) becomes **instance admin only**; `reduce_view` (:104) is the reduced shape for everyone else, including workspace owners: `image`,
     `runner_id`, `start_attempt.detail`/`events` and every `start_log[].detail` are removed; the failure's `failure_message` becomes the member hint. The `computer`
     object, `boot_id`, the phases and times stay. The existing callers of `view` keep working; the retry, wake and restart answers use the same function.
   - Update every existing test that expected a workspace owner to see the detail (search `services/aurora-api/tests/it` for `start_attempt`, `image`, `start_log`) and add
     the three-role test of SEC-141. This **amends D-178 Q-R5-5**; the entry in `docs/90-decisions.md` is the Orchestrator's job, so say so in the completion report.
   - Web client: extend `SandboxView` in `web/src/lib/api/conversations.ts` with `computer` and the new optional nullability of `image`/`runner_id` (they are already `| null`),
     and add `sleep: (agent) => call<SandboxView>("POST", `/v1/agents/${agent}/sandbox/sleep`)`. No component changes.
5. **Sleep now (C-1060; computer-13).** Anchor `…#f-computer-13`.
   - New route `POST /v1/agents/{agent}/sandbox/sleep` in `hosted/routes.rs` next to `wake_sandbox` (:407), with the `utoipa` annotation and `routes!` entry. Logic exactly as
     C-1060: a repository function `request_sleep(conn, agent) -> SleepOutcome { Requested, AlreadyStopped, Busy(reasons), Failed }` in `repo/hosted.rs` that, in one statement under the
     sandbox row lock, sets `stop_requested_at = now(), stop_reason = 'idle'` when `state = 'running'` and no `turn`/`process`/`watch` reason holds. Re-use the reason
     computation of `view_full` (do not copy the SQL: query once through a shared helper). Audit `agent.sandbox_sleep`. Register it in
     `services/aurora-api/tests/it/permission_matrix.rs` (can-use callers only; 404 for others).
   - Race: a message that arrives between the check and the stop makes the start arm bring the computer back; that is acceptable and documented.
6. **Two Files facts (C-1060; computer-3, computer-11).** Anchors `…#f-computer-3`, `…#f-computer-11`.
   - **Move needs no code.** Add a test in `files/service.rs`'s test module or `tests/it` that `FileOp::Rename { path: "/workspace/a.csv", to: "/workspace/docs/a.csv" }`
     moves across folders and that moving a folder into itself is `422 invalid_path`. Write the sentence in `05`: "the dashboard's Move to… is `rename` with a full destination".
   - **Big text preview.** `GET …/files/text?path=&preview=1`: `PathQuery` gains `preview: Option<bool>`; with it, a regular UTF-8 text file over `FILES_EDIT_MAX` returns the
     first 256 KiB cut at the last line end, `truncated: true`, `size` the real size, `etag: null`, audited `agent.files.download` with `via: "preview"`. Without `preview` nothing changes.
     `FileText` gains `truncated: bool` (default false) and `etag` becomes nullable **only** in this mode (keep the field a string in the schema for existing callers: use
     `Option<String>` with `skip_serializing_if` only if the generated client types stay compatible; otherwise a separate `FileTextPreview` schema). Binary/not-UTF-8 stay `422 file_not_editable`.
7. **SSH: verify, change nothing (computer-6).** Anchor `…#f-computer-6`. Confirm and write in the completion report: `valid_before` and `username` are returned today; `aurora ssh <handle>`
   works; the terminator requires a UUID user name. The designer's note says to keep the id behind "Copy command" until the terminator maps handles; **do not** change the terminator here.

## Out of scope

- Any UI (header, Computer tab, terminal notice, Sleep now button, countdown, CSV table, Move to… menu, drag and drop, terminal key row): P5-REFINE-11 and -15.
- Mapping SSH handles in the terminator (needs a terminator and CA change; open question for the owner).
- The dashboard's retry limits and "Couldn't reach the computer. Try again · Report it" (computer-2, P5-REFINE-15).
- Usage "Awake time 0 min" (the rollup lag is documented, P5-REFINE-16 words it).
- The agent timeline and its live stream (P5-REFINE-9).

## Constraints / file ownership

- Everything in "Wave and order" above. Keep `aurora-protocol` changes additive.
- Never touch `localhost:4000`, the aurora-dev cluster or its database (memory: no aurora-dev cluster). Tests use throwaway Postgres.
- Heavy commands only through `/home/alex/orca/workspaces/aurora/heavy.sh <lane> <command>` (one at a time).
- Lanes run only fast targeted tests; GitHub CI is the full gate.

## Acceptance criteria

1. **Bug regression test (runner).** A mocked API answers the first `register` with `terminal: false, previews: false, ssh_enabled: false` and a later `work` poll with
   `streams { terminal: true }`; the runner's stream listener **opens :4108 within one poll** and accepts a connection, with no restart. The reverse (`ssh_enabled` goes
   false) stops redeeming `auz_` grants. An API that omits `streams` leaves today's behaviour. (`services/aurora-runner/tests/it/` and a unit test of the apply function.)
2. **Bug regression test (API).** `poll` returns `streams` computed from the live configuration (switch `terminal.enabled` in the test settings between two polls and see it change);
   `register` and `poll` use the same function (a test asserts equal output for the same inputs).
3. **Keep-awake.** With a terminal leg open and **no bytes** for longer than `TERMINAL_AWAKE_GRACE`, the runner posts at least two keep-awake stamps and the API's `claim_stops` does not
   stop the sandbox (an integration test with a short grace override); closing the leg stops the stamps; SSH likewise. `keep_awake_reasons` contains `terminal` while open.
4. **Computer view.** The pure function test table passes. For a member, a workspace owner and an instance admin, `GET …/sandbox` shows the `computer` object to all three and the image,
   runner id, attempt detail/events and log details **only** to the instance admin; the owner keeps the member hint and the phases (SEC-141 test). A supervisor that connects before the
   `running` report leaves `start_phase` at `booting`/`ready`.
5. **Sleep now.** 202 on a running idle computer (and the stop arm then stops it with reason `idle`); 200 when already stopped or stopping; 409 `sandbox_busy` with a running turn, a busy
   process and an armed watch; 409 `not_hosted`; 404 for a caller without access; one `agent.sandbox_sleep` audit row; the permission matrix row.
6. **Files.** Rename-as-move test passes; `preview=1` returns the first 256 KiB with `truncated: true` and no etag for a 2 MiB text file, `422 file_not_editable (binary)` for a binary one, and the plain
   route still refuses 2 MiB with `too_large`.
7. **Contracts and docs.** C-1057..C-1060 are in `docs/API_CONTRACTS.md` **before** the code commits; SEC-141 in `docs/17-security.md` and `docs/tests/security-coverage.md`; the `05` rows, `11`, `15`, `16 §W18`
   updated; `web/openapi.json`, `web/src/lib/api/schema.d.ts` regenerated; the web `SandboxView` type and `sleep` call added.
8. **Gates (targeted):** `cargo fmt --check`; `cargo clippy -p aurora-runner -p aurora-api -p aurora-db -p aurora-protocol --all-targets -- -D warnings`; `cargo test -p aurora-runner` (the new tests),
   `cargo test -p aurora-api --test it -- runner_plane sandbox terminal ssh files hosted permission_matrix` (names as they exist); `cargo test -p aurora-protocol`; `scripts/guards/run-all.sh`;
   `python3 scripts/extract-schema.py --check` (no schema change, so it must still pass); OpenAPI check (`openapi_matches_checked_in`) and `pnpm gen:api` with no diff; web `pnpm tsc --noEmit` for the type change.
   Report the counts. Never claim a check that did not run.

## Tests

- **Runner (`services/aurora-runner/tests/it/runner.rs`, wiremock):** the bug (AC 1); an old API with no `streams`; ssh off again.
- **Runner unit:** the heartbeat starts on `register_route` for terminal/ssh legs only, stops on release, never double-stamps with the byte path (fake clock via `tokio::time::pause`).
- **API (`services/aurora-api/tests/it/`):** `runner_plane.rs` (poll `streams`), `sandbox_wake.rs`/`terminal.rs` (keep-awake keeps the sandbox, restart changes `boot_id`), a new `computer_view.rs`
  (three roles, phases, sleeps_in, stale phase), a sleep test (all outcomes), `files` (rename-as-move, preview), `permission_matrix.rs`.
- **Unit:** `computer.word`/`ready` table; `sleeps_in_secs` arithmetic; `request_sleep`.
- **Do not** run perf budgets, the full workspace test or any build outside `heavy.sh`.

## Docs to update

- `docs/API_CONTRACTS.md` (C-1057..C-1060, before coding), `docs/17-security.md` (SEC-141), `docs/tests/security-coverage.md`.
- `docs/05-control-plane-api.md`: the runner `work` row (`streams`), `GET /v1/agents/{agent}/sandbox` (computer, who sees what), `POST …/sandbox/sleep`, `GET …/files/text` (`preview`),
  the files `rename` row (moves).
- `docs/11-*` §Web terminal / §Stop (open sessions keep the computer awake; Sleep now), `docs/15-*` and `docs/16-*` §W18 (the listener's flags are re-read at every poll).
- `docs/14-web-dashboard.md` §Computer: one sentence that the engine details are instance-admin only (the UI brief writes the rest).
- `docs/PROGRESS.md` and `docs/FEATURES.md` rows for P5-REFINE-4 (status, verification); `docs/90-decisions.md`: tell the Orchestrator that D-178 Q-R5-5 is amended by D-412.

## Suggested agent/model and reason

Sonnet (`rust-developer`). It spans the runner, the runner plane, the hosted domain and two crates, and has security weight (SEC-141). Scopes 1 and 2 are the delicate ones (async tasks in the stream
listener); Scopes 5–6 are mechanical. Not Haiku.

## Completion report

(Fill in; P5-REFINE-20 reads it.)

- **Items done** (Scope 1–7, each with its commit):
- **Contracts C-1057..C-1060** (the commit that added them to `docs/API_CONTRACTS.md`), **SEC-141** (docs/17 and the coverage row):
- **The bug:** the regression test names and the proof it fails without the fix:
- **Keep-awake:** how the heartbeat is tied to the leg's life, the test that proves a quiet terminal stays awake, any cap behaviour found:
- **Computer view:** the `word`/`ready` table, the stale-phase fix and its test; the role matrix result (member / owner / instance admin):
- **Sleep now:** outcomes tested; the race noted:
- **Files:** rename-as-move test; preview shape chosen (`FileText.truncated` or a separate schema) and why:
- **SSH check (Scope 7):** what was verified; the open question for the owner (handle as the SSH user):
- **Deviations and why:**
- **Screenshots compared:** none for this brief (no UI); say so. The UI briefs compare `computer-1`, `computer-7`, `computer-13`.
- **Tests run** (commands, counts, anything not run and why):
- **Open questions** (for the owner or the Orchestrator):
