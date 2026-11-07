# Hosted agents: test coverage audit (TEST-014)

Maps each bullet of `docs/18-testing.md` §Hosted agents to the test that proves it.
Paths are relative to the repo root; `api` = `services/aurora-api/tests`, `sup` =
`services/aurora-supervisor/tests`, `run` = `services/aurora-runner/tests`, `e2e` = `tests/e2e-hosted/tests`.
**New** marks tests added by TEST-014.

## Due-work engine (fake runner backend)

| Item | Test(s) |
|---|---|
| Arm 1 ownership takeover / replica death | `api/runner_plane.rs::ownership_fencing_takeover_and_duplicates` |
| Arm 2 stop: idle park, `stopped` report revokes boot token | `api/runner_plane.rs::idle_stop_then_stopped_report_revokes_boot_token` |
| Arm 2 stop: restart request | `api/runner_plane.rs::restart_request_stops_running_sandbox` |
| Arm 2 stop: background keep-awake and cap expiry | **New** `api/runner_plane.rs::background_process_holds_awake_until_the_cap` |
| Arm 2 stop: eviction over the held-awake ceiling | **New** `api/runner_plane.rs::eviction_stops_the_longest_idle_when_a_start_is_waiting` |
| Arm 3 start: boot document, tokens, backoff after failure | `api/runner_plane.rs::start_item_has_valid_boot_document_and_tokens_match_db`, `failed_start_backs_off` |
| Arm 4 abort / send-now | `api/hosted.rs::abort_running_turn_is_delivered_and_ends_aborted`, `api/conversations.rs::abort_queued_send_now_and_finish_fencing` |
| Arm 5 home sync | `api/skills.rs::projection_entries_in_home_sync`, `api/hosted.rs::end_to_end_turn_with_steer_credential_and_replay` |
| Arm 6 dispatch (credential minted, no model key) | `api/hosted.rs::end_to_end_turn_with_steer_credential_and_replay`, `no_model_key_fails_before_any_sandbox_start` |
| Arm 7 steer | `api/hosted.rs::end_to_end_turn_with_steer_credential_and_replay`, `sup/turns.rs::steer_delivered_missed_and_refused` |
| Arm 8 stale recovery | `api/hosted.rs::never_started_is_revived_once_then_failed`, `stalled_turn_fails_and_ceiling_warns_once_then_times_out` |
| Arm 9 schedules | `api/schedules.rs::missed_fires_coalesce_and_pollers_fire_once` |
| Arm 10/11 boot end: processes parked/lost, watches converted with notice, runtime intervals closed | **New** `api/runner_plane.rs::boot_end_parks_or_loses_processes_and_converts_watches`; intervals in `idle_stop_then_stopped_report_revokes_boot_token` |
| Claim races with concurrent pollers | `api/runner_plane.rs::concurrent_pollers_never_receive_the_same_start`, `channel_claim_is_single_use_and_hold_is_fenced`, `api/schedules.rs::missed_fires_coalesce_and_pollers_fire_once`, `api/conversations.rs::concurrent_messages_make_one_turn_and_one_followup` |
| Ownership fencing, duplicate/replayed reports | `api/runner_plane.rs::ownership_fencing_takeover_and_duplicates`, `replayed_reports_through_a_second_app_state_have_no_double_effects`; runner side `run/runner.rs::duplicate_and_fenced_results_are_not_retried` |
| Channel replica takeover | `run/channel.rs::session_reconnect_is_accepted_by_another_replica`, `fast_reconnect_is_never_followed_by_a_stale_release` |
| Long-poll wake | `api/runner_plane.rs::long_poll_wakes_on_work_notify` |

## Turn lifecycle

| Item | Test(s) |
|---|---|
| queued → dispatched → running → done | `api/hosted.rs::end_to_end_turn_with_steer_credential_and_replay`, `e2e/hosted.rs::s1_chat_with_daneel_fake_harness` |
| Follow-up joined / promoted | `api/hosted.rs::followup_is_promoted_when_the_turn_ends_unjoined`, `api/conversations.rs::finish_promotes_followups_and_replays_are_noops`, `e2e/hosted.rs::s6_followup_joins_or_is_promoted` |
| Abort | `api/hosted.rs::abort_running_turn_is_delivered_and_ends_aborted`, `sup/turns.rs::abort_ends_with_aborted_result` |
| Send-now | `api/conversations.rs::abort_queued_send_now_and_finish_fencing` |
| Never-started revival | `api/hosted.rs::never_started_is_revived_once_then_failed`, `e2e/hosted.rs::s7_never_started_revived_once_then_failed` |
| Stall; ceiling warning and failure | `api/hosted.rs::stalled_turn_fails_and_ceiling_warns_once_then_times_out` |
| Restart | `api/hosted.rs::sandbox_restart_and_models_routes`, `e2e/hosted.rs::s5_restart_fails_active_turn_and_boots_anew` |
| Supervisor reconnect/replay without duplicates | `sup/turns.rs::reconnect_replays_without_duplicates`, `replays_frames_unacked_by_runner` |

## Home sync

| Item | Test(s) |
|---|---|
| Parts packing at worst-case encodings (controls x6, CJK x3), every entry once, part indices/count, manifest | **New** `api/skills.rs::home_sync_parts_pack_within_the_wire_max_at_worst_case_encodings` |
| Crash mid-generation | `sup/home.rs::multi_part_generation_is_applied_only_at_commit`, `interrupted_generation_keeps_previous_state_and_is_replaced`, `replayed_part_is_idempotent_and_missing_manifest_is_not_acked` |
| Manifest pruning | `sup/home.rs::prune_removes_only_managed_paths`, `unmodified_projection_pruned_and_unmanaged_paths_refused` |
| Memory harvest round-trip with checksum verification | `sup/home.rs::harvest_detects_edits_and_ignores_unchanged` (sealed checksum), `api/memory.rs::harvest_memory_writes` (revision semantics), `run/channel.rs::memory_write_and_file_upload_round_trip` (relay; corrupt checksum refused), `sup/home.rs::outbound_file_is_split_with_correct_sha256` |

## Memory search languages

| Item | Test(s) |
|---|---|
| `english`, `german`, `simple` search | `api/memory.rs::search_ranking_and_languages` |
| Language change re-render | `api/memory.rs::language_change_bumps_generation` |

## Schedules

| Item | Test(s) |
|---|---|
| Coalescing missed fires | `api/schedules.rs::missed_fires_coalesce_and_pollers_fire_once` |
| Gate quiet / wake / fail; failure disable | `api/schedules.rs::gates_quiet_wake_fail_and_failure_disable` |
| Authorization disable | `api/schedules.rs::creator_losing_access_disables_with_authorization_and_notice` |

## Security probes in a real sandbox

SEC-01/02/09/26/34: `e2e/hosted.rs::s8_security_probes`, `s8b_gateway_and_runner_ops_ports_unreachable_from_sandbox`,
`e2e/k8s.rs::k4_sec02_sec09_probes_from_sandbox_pod` (TEST-013 / OPS-011). Runner-side hardening: `run/podman.rs::sandbox_lifecycle_and_hardening`.
Boot-secret confinement in the supervisor: `sup/spawn_env.rs::children_never_receive_boot`, `sup/boot_proxy.rs::boot_files_are_confined_to_the_home`.

## Defects found and fixed

1. **Armed watches were not converted when a boot ended** (docs/10 §Processes and watches: "their armed
   watches convert likewise and post a notice"). Only `processes` rows moved to `parked`/`lost`; the watch stayed
   `armed`, which also kept the sandbox "held awake" (`BACKGROUND` keep-awake query). Fix: `end_boot` in
   `crates/aurora-db/src/repo/sandboxes.rs` now sets armed watches to the same status and appends a `notice`
   event to the arming conversation. Proven by `boot_end_parks_or_loses_processes_and_converts_watches`.

## Known risk, not fixed (contract territory)

A single home-sync entry larger than `PACK_MAX` (about 198 KB serialized) is skipped with a warning in
`hosted/home.rs::pack`. In-spec skills (32 000 chars total, even all controls x6 = 192 KB) and memories up to
`MEMORY_TOOL_CONTENT_MAX` fit, but a dashboard-written memory near `MEMORY_CONTENT_MAX` (100 000 chars) in CJK
(300 KB) or controls would silently not reach the sandbox. Fixing it needs a wire decision (split-file parts or a
smaller content cap), so it is reported rather than invented here.
