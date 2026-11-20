//! Limits registry (`02 §Limits registry`). Dependency-free (std only).

use std::time::Duration;

const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;

pub const GATEWAY_RESOLVE_CACHE_TTL: Duration = Duration::from_secs(60);
/// (FIX-180, D-395) A proxied request whose body delivers no byte for this long is answered `408
/// request_body_timeout` and its connection closed. An idle limit, not a total one.
pub const GATEWAY_BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);
/// (RFE-011a) lower bound of a per-rule / per-grant `approval_timeout_secs`.
pub const APPROVAL_TIMEOUT_MIN: Duration = Duration::from_secs(60);
/// (RFE-011a) default `approvals.max_timeout_secs`.
pub const APPROVAL_TIMEOUT_MAX: Duration = Duration::from_secs(3600);
/// (RFE-011a) hard upper bound the instance maximum may be configured to.
pub const APPROVAL_TIMEOUT_MAX_CONFIGURABLE: Duration = Duration::from_secs(14_400);
/// (RFE-011a) lower bound of the instance maximum.
pub const APPROVAL_TIMEOUT_MAX_FLOOR: Duration = Duration::from_secs(300);
/// (RFE-011a) a pending approval with no live delivery surface is e-mailed after this.
pub const APPROVAL_NOTIFY_DELAY: Duration = Duration::from_secs(20);
/// (RFE-011a) an `approval_watchers` row is live when seen within this.
pub const APPROVAL_WATCHER_TTL: Duration = Duration::from_secs(45);
/// (RFE-011a) e-mails per user per hour; the rest fold into a digest.
pub const APPROVAL_EMAIL_PER_USER_HOUR: u32 = 20;
/// (P4-011B, RFE-011b) default `approvals.park.threshold_secs`: effective timeouts above it park.
pub const PARK_THRESHOLD: Duration = Duration::from_secs(900);
/// (P4-011B) default `approvals.park.max_timeout_secs`: the longest deadline of a parked approval.
pub const PARK_MAX_TIMEOUT: Duration = Duration::from_secs(86_400);
/// (P4-011B) default `approvals.park.deny_cooldown_secs`: a denied fingerprint repeated within it
/// is answered `manual_approval_denied` without a new approval.
pub const PARK_DENY_COOLDOWN: Duration = Duration::from_secs(900);
/// (P4-011B) default `approvals.park.max_body_bytes`: the largest request body that may be parked.
pub const PARK_BODY_MAX: usize = 256 * KIB;
/// (P4-011B) the whole request body must be read within this to be parked (else it is held).
pub const PARK_BODY_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// (P4-011B) default `approvals.park.max_pending_per_agent`: parked pending approvals per agent.
pub const PARK_PENDING_PER_AGENT: u32 = 20;
/// (P4-011B) new parked approvals per agent per hour (not configurable).
pub const PARK_PER_AGENT_HOUR: u32 = 60;
/// (P4-011B-2) default `approvals.park.resume_window_secs`: how long an approved parked approval
/// can be consumed by the identical repeat.
pub const PARK_RESUME_WINDOW: Duration = Duration::from_secs(1800);
/// (P4-011B-2, D-264(e), C-824) approvals listed in one resume follow-up, then "…and N more".
pub const PARK_FOLLOWUP_LINES_MAX: usize = 10;
/// (P4-011B-3, D-134) grace before a push for a new approval (a decision or a live tab first wins).
pub const APPROVAL_PUSH_DELAY: Duration = Duration::from_secs(5);
/// (P4-011B-3) pushes per user per hour; the next one becomes a digest, the rest fold silently.
pub const PUSH_PER_USER_HOUR: u32 = 30;
/// (P4-011B-3) Web Push registrations per user (409 `push_subscription_limit_reached`).
pub const PUSH_SUBSCRIPTIONS_PER_USER: u32 = 10;
/// (P4-011B-3) test pushes per user per hour (429 `push_test_rate_limited`).
pub const PUSH_TEST_PER_HOUR: u32 = 3;
/// (P4-011B-3) the longest `TTL` sent to a push service.
pub const PUSH_TTL_MAX: Duration = Duration::from_secs(86_400);
/// (P4-011B-3, C-832) the largest plaintext payload before encryption.
pub const PUSH_PAYLOAD_MAX: usize = KIB;
/// (P4-011B-3) how long a retired VAPID key stays for the subscriptions that reference it.
pub const PUSH_VAPID_RETIRE: Duration = Duration::from_secs(30 * 86_400);
/// (P4-011B-3, C-834) tries for a 429/5xx answer within 60 s.
pub const PUSH_RETRY_MAX: u32 = 3;
/// (P4-011B-3, C-834) consecutive failures that disable a subscription.
pub const PUSH_DISABLE_AFTER_FAILURES: i32 = 5;
/// (P4-011B-3, C-831) the longest accepted subscription endpoint URL, in characters.
pub const PUSH_ENDPOINT_MAX: usize = 2048;
/// (P4-011B-3, C-833) lifetime of a VAPID JWT (`exp`).
pub const PUSH_VAPID_JWT_TTL: Duration = Duration::from_secs(3600);
/// (RFE-011a) The effective hold timeout in seconds: the rule-level minimum clamped to
/// `[min, max]`, else `default` (`approvals.timeout_secs`). One definition for the gateway hold
/// and for the simulate / effective views, so they cannot disagree.
pub fn effective_approval_timeout_secs(rule: Option<u32>, default: u64, min: u64, max: u64) -> u64 {
    match rule {
        Some(s) => u64::from(s).max(min).min(max.max(min)),
        None => default,
    }
}
pub const APPROVAL_BODY_PEEK: usize = 16 * KIB;
/// default; allowed range is `_MIN..=_MAX`.
pub const CONDITION_BODY_BUFFER: usize = 256 * KIB;
pub const CONDITION_BODY_BUFFER_MIN: usize = 4 * KIB;
pub const CONDITION_BODY_BUFFER_MAX: usize = 8 * MIB;
pub const UPSTREAM_HEADER_TIMEOUT: Duration = Duration::from_secs(300);
pub const WEBSOCKET_IDLE_TIMEOUT: Duration = Duration::from_secs(600);
/// (FIX-106) Inbound tunnel bounds (`06` §Tunnel limits): the TLS handshake inside a `CONNECT`
/// tunnel (and the dial of an open-lane tunnel) must finish in this time.
pub const TUNNEL_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// (FIX-106) A request head inside a tunnel must arrive in this time.
pub const TUNNEL_HEADER_TIMEOUT: Duration = Duration::from_secs(30);
/// (FIX-106) Every tunnel kind ends after this long without bytes in either direction (and, for
/// a MITM tunnel, without a request in flight): the WebSocket idle value.
pub const TUNNEL_IDLE_TIMEOUT: Duration = WEBSOCKET_IDLE_TIMEOUT;
/// (FIX-106) Concurrent tunnels of one credential; the next `CONNECT` is answered 429.
pub const TUNNEL_PER_CREDENTIAL_MAX: usize = 64;
/// (FIX-106) Concurrent tunnels of the whole gateway.
pub const TUNNEL_TOTAL_MAX: usize = 4096;
/// flush interval; also flushes at `REQUEST_LOG_FLUSH_ROWS` rows.
pub const REQUEST_LOG_FLUSH: Duration = Duration::from_secs(5);
pub const REQUEST_LOG_FLUSH_ROWS: u32 = 500;
pub const BATCH_HEADER_MAX: usize = 128;
pub const CONNECTION_CHOICE_MAX: u32 = 20;
pub const REFRESH_MARGIN: Duration = Duration::from_secs(60);
pub const LLM_ORIGIN_RECHECK: Duration = Duration::from_secs(3600);
pub const LLM_ORIGIN_CHECK_TIMEOUT: Duration = Duration::from_secs(10);
/// (P4-LLM-2, D-261(e)) One mint call (Google token, Entra token, STS session).
pub const LLM_TOKEN_MINT_TIMEOUT: Duration = Duration::from_secs(10);
/// A minted credential is reused until this long before it expires ...
pub const LLM_TOKEN_SKEW: Duration = Duration::from_secs(300);
/// (the 55-minute cap of the same row is `LLM_TOKEN_MAX_AGE` in the gateway's `llm_mint.rs`)
/// After a failed mint the same `(secret, version, mode, region)` answers 502 without a new upstream call for this long.
pub const LLM_TOKEN_MINT_FAILURE_HOLD: Duration = Duration::from_secs(5);
/// (P4-LLM-4, D-268(c)) Request body the gateway buffers to SigV4-sign a Bedrock request; larger → 413
/// `llm_request_too_large`.
pub const LLM_SIGN_BODY_MAX: usize = 8 * 1024 * 1024;
/// (P5-ICS, D-273, C-843) Largest body the gateway reads from a secret link; a larger one is cut
/// with 502 `link_response_too_large` and never cached.
pub const SECRET_LINK_RESPONSE_MAX: usize = 20 * 1024 * 1024;
/// (P5-ICS, D-273(g)) How long a complete `200` body of a secret link is answered from memory.
pub const SECRET_LINK_CACHE_TTL: Duration = Duration::from_secs(300);
/// (P5-ICS) Bytes of secret-link bodies one gateway replica keeps (least recently used evicted).
pub const SECRET_LINK_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;
/// (P5-ICS, C-843) Redirect hops the gateway follows for a secret link.
pub const SECRET_LINK_REDIRECTS_MAX: usize = 3;
/// (P5-WEBHOOK, D-320, C-909, C-912) Largest stored outgoing-webhook configuration (the JSON of
/// `app_connections.config`).
pub const WEBHOOK_CONFIG_MAX: usize = 32 * 1024;
/// (C-909) Largest body template.
pub const WEBHOOK_TEMPLATE_MAX: usize = 16 * 1024;
/// (C-909) Most slots one webhook declares.
pub const WEBHOOK_SLOTS_MAX: usize = 20;
/// (C-910) Largest slot-values body an agent sends (413 `values_too_large`).
pub const WEBHOOK_VALUES_MAX: usize = 64 * 1024;
/// (C-910) Largest rendered body (413 `rendered_too_large`).
pub const WEBHOOK_BODY_MAX: usize = 256 * 1024;
/// (C-910) Most of the receiver's answer the agent gets (`X-Aurora-Truncated: 1` beyond).
pub const WEBHOOK_RESPONSE_MAX: usize = 64 * 1024;
/// (C-910) Time-out of one outgoing-webhook request; there is no retry.
pub const WEBHOOK_TIMEOUT: Duration = Duration::from_secs(15);
/// (P4-LLM-4, D-261) `aws_assume_role` `session_duration_secs`, in seconds (default 3 600).
pub const LLM_ASSUME_ROLE_DURATION: std::ops::RangeInclusive<u64> = 900..=43_200;
/// Minted entries per gateway replica (least recently used evicted).
pub const LLM_MINT_CACHE_MAX: usize = 10_000;
/// entries per org / workspace.
pub const EGRESS_ALLOWLIST_MAX: u32 = 500;
/// (RFE-025) CIDRs per BYO source list (agent / org / instance).
pub const BYO_CIDR_MAX: usize = 32;
/// (phase 3, RFE-013) bounds of `governance.agent_tokens.max_age_days`.
pub const AGENT_TOKEN_MAX_AGE_MIN: i64 = 1;
pub const AGENT_TOKEN_MAX_AGE_MAX: i64 = 3650;
/// (RFE-013) minimum remaining life (days) of an existing token when a maximum age is introduced
/// or tightened.
pub const AGENT_TOKEN_RETROFIT_GRACE_DAYS: i64 = 7;
/// (RFE-013) a token unused (or never used, counted from creation) for this long is stale.
pub const AGENT_TOKEN_STALE_DAYS: i64 = 30;
/// (RFE-013) a token expiring within this many days is `expiring`.
pub const AGENT_TOKEN_EXPIRING_DAYS: i64 = 14;
/// (RFE-025) one log line and one request-log row per credential per interval for BYO refusals.
pub const BYO_REFUSAL_LOG_INTERVAL: Duration = Duration::from_secs(60);
/// (RFE-006) one "paused by an administrator" reply per (space/thread, person) per interval.
pub const FREEZE_REPLY_INTERVAL: Duration = Duration::from_secs(600);
pub const REFRESH_LOCK_WAIT: Duration = Duration::from_secs(10);
/// Entries per signing-key / internal-secret ring (`08 §4`, RFE-007).
pub const KEYRING_MAX: usize = 4;
/// Header naming the internal-secret ring entry a caller presents (`08 §4`, RFE-007). Not secret.
pub const KEY_ID_HEADER: &str = "x-aurora-key-id";
pub const OAUTH_STATE_TTL: Duration = Duration::from_secs(600);
pub const IDLE_STOP: Duration = Duration::from_secs(1800);
pub const BACKGROUND_CAP: Duration = Duration::from_secs(43200);
pub const TURN_CEILING: Duration = Duration::from_secs(21600);
pub const TURN_CEILING_WARNING: Duration = Duration::from_secs(900);
/// (FIX-158, D-319) seconds between a chat connector's typing-indicator refreshes for a running turn;
/// clients drop the indicator after about 5 s. A refresher without its `status.clear` stops after
/// `TURN_CEILING_WARNING`.
pub const TYPING_REFRESH_SECS: u64 = 4;
/// (FIX-158, D-319) an open chat receipt (👀 or "Working…") whose conversation has had no active
/// turn and no pending follow-on work for this long is cleared by the channel-op sweep.
pub const RECEIPT_ORPHAN_SECS: u64 = 600;
/// (P4-NARR, D-280) at most one narration-card update per turn per this interval.
pub const NARR_MIN_INTERVAL: Duration = Duration::from_secs(5);
/// (P4-NARR, D-280) narration writes (posts, updates, collapses) per space per minute.
pub const NARR_SPACE_UPDATES_PER_MINUTE: usize = 10;
/// (P4-NARR, D-280) narration cards open at a time in one space; a further turn gets none.
pub const NARR_CARDS_PER_SPACE: usize = 3;
/// (P4-NARR, D-280) steps listed per card, then "N earlier".
pub const NARR_MAX_STEPS: usize = 12;
/// (P4-NARR, D-280) characters per step name and per detail.
pub const NARR_STEP_CHARS: usize = 120;
pub const TURN_STALL: Duration = Duration::from_secs(600);
pub const STALE_DISPATCH: Duration = Duration::from_secs(90);
pub const TOOL_CALL_TIMEOUT: Duration = Duration::from_secs(25);
pub const TOOL_CONSECUTIVE_FAILURES: u32 = 2;
pub const WS_FRAME_MAX: usize = 256 * KIB;
pub const HOME_SYNC_PART_MAX: usize = 200 * KIB;
pub const CONTEXT_CREDENTIAL_GRACE: Duration = Duration::from_secs(300);
pub const TURN_TEXT_MAX: usize = 100_000;
pub const MEMORY_MAX_PER_AGENT: u32 = 100;
pub const MEMORY_REVISIONS_KEPT: u32 = 50;
pub const MEMORY_TOOL_CONTENT_MAX: usize = 12_000;
pub const MEMORY_CONTENT_MAX: usize = 100_000;
/// Decision S-13: a memory's content, serialized as a JSON string, must also fit one home-sync
/// part with its frontmatter and path (`HOME_SYNC_PART_MAX`); 100 000 multi-byte characters
/// can exceed it.
pub const MEMORY_CONTENT_JSON_MAX: usize = 180 * KIB;
pub const MEMORY_INDEX_MAX: usize = 6_000;
/// snippet count (each `MEMORY_SNIPPET_CHARS` chars, score >= `MEMORY_SNIPPET_MIN_SCORE`).
pub const MEMORY_SNIPPETS: u32 = 3;
pub const MEMORY_SNIPPET_CHARS: usize = 800;
pub const MEMORY_SNIPPET_MIN_SCORE: f32 = 0.1;
pub const MEMORY_SEARCH_HITS: u32 = 8;
pub const SKILL_NAME_MAX: usize = 64;
pub const SKILL_DESCRIPTION_MAX: usize = 500;
pub const SKILL_BODY_MAX: usize = 24_000;
pub const SKILL_TOTAL_MAX: usize = 32_000;
/// (FIX-110, docs/02 §Limits) Own (non-library) skills per agent and files per skill: a looping
/// `skill_create` cannot grow what every home sync projects without bound.
pub const SKILL_PER_AGENT_MAX: i64 = 50;
pub const SKILL_FILES_MAX: i64 = 50;
/// (FIX-110) Queued peer tasks per ordered (sender, target) pair.
pub const PEER_TASK_QUEUE_MAX: i64 = 10;
// Git skill library (phase 4, P4-GSL, D-148, D-149; docs/02 §Limits).
pub const LIBRARIES_PER_ORG: usize = 10;
/// Library-sourced bytes (body + files, binary as base64 length) and skill counts per tier.
pub const LIBRARY_BYTES_PER_AGENT: usize = MIB;
pub const LIBRARY_SKILLS_PER_AGENT: usize = 25;
pub const LIBRARY_BYTES_PER_WORKSPACE: usize = 2 * MIB;
pub const LIBRARY_SKILLS_PER_WORKSPACE: usize = 50;
pub const LIBRARY_BYTES_PER_ORG: usize = 4 * MIB;
pub const LIBRARY_SKILLS_PER_ORG: usize = 100;
pub const LIBRARY_FILE_MAX: usize = 64 * KIB;
pub const LIBRARY_TREE_MAX: usize = 20_000;
pub const LIBRARY_BROWSE_MAX: usize = 200;
/// (P5-IA-1, C-987) One-time notice keys a person can have dismissed.
pub const ONBOARDING_NOTICES_MAX: usize = 50;
/// Open proposed updates per organization.
pub const LIBRARY_PENDING_BYTES_PER_ORG: usize = 4 * MIB;
pub const LIBRARY_REFRESH_INTERVAL: Duration = Duration::from_secs(15 * 60);
pub const LIBRARY_REFRESH_INTERVAL_MIN: Duration = Duration::from_secs(5 * 60);
pub const LIBRARY_LEASE: Duration = Duration::from_secs(5 * 60);
/// overall bound of one library's refresh pass: safely below `LIBRARY_LEASE` so the outcome is
/// recorded (and the lease released) before another replica can claim the library.
pub const LIBRARY_REFRESH_BUDGET: Duration = Duration::from_secs(5 * 60 - 30);
pub const LIBRARY_REFRESH_CONCURRENCY: usize = 4;
pub const LIBRARY_CHECK_NOW_INTERVAL: Duration = Duration::from_secs(60);
pub const LIBRARY_BROWSE_CACHE: Duration = Duration::from_secs(5 * 60);
pub const LIBRARY_FETCH_TIMEOUT: Duration = Duration::from_secs(20);
// Addon sources (phase 5, P5-ASRC, D-250; docs/02 §Limits, docs/22).
/// The `aurora-addons.yaml` manifest (download cap; a bigger body is refused before parsing).
pub const ADDON_MANIFEST_MAX: usize = MIB;
/// Addon entries per manifest and versions per addon.
pub const ADDON_ADDONS_MAX: usize = 100;
pub const ADDON_VERSIONS_MAX: usize = 50;
/// The per-source lease of the poll job and the manual "Check now".
pub const ADDON_LEASE: Duration = Duration::from_secs(5 * 60);
/// Sources checked at once per replica.
pub const ADDON_CHECK_CONCURRENCY: usize = 2;
/// Manifest and package fetch timeouts.
pub const ADDON_FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Backoff until a source is re-checked after a failed poll.
pub const ADDON_FAILURE_BACKOFF: Duration = Duration::from_secs(3600);
// Instance settings (phase 5, SET-1, D-236; docs/02 §Limits).
/// Stored JSON per settings section (413 `settings_too_large`).
pub const SETTINGS_SECTION_BYTES_MAX: usize = 128 * 1024;
/// A settings document per import request (413).
pub const SETTINGS_IMPORT_BYTES_MAX: usize = 512 * 1024;
/// Backstop poll of the section generations next to `aurora_settings`.
pub const SETTINGS_RELOAD_POLL: Duration = Duration::from_secs(30);
/// Longest before/after value kept in an `instance_settings.*` audit row.
pub const SETTINGS_AUDIT_VALUE_MAX: usize = 1024;
/// Affected organizations a settings preview lists.
pub const SETTINGS_PREVIEW_ORGS_MAX: usize = 200;
// Secret stores in the dashboard (FIX-124).
/// A store's health and the value counts on Admin › Secret stores are probed at most this often.
pub const SECRET_STORE_HEALTH_INTERVAL: Duration = Duration::from_secs(30);
/// Longest a store probe (health, Test connection) may take.
pub const SECRET_STORE_PROBE_TIMEOUT: Duration = Duration::from_secs(8);
/// Wait after a dashboard migration's commits before the old values are discarded.
pub const SECRET_MIGRATION_DISCARD_GRACE: Duration = Duration::from_secs(2);
pub const ATTACH_IN_FILE_MAX: u64 = 10 * MIB as u64;
/// per message.
pub const ATTACH_IN_COUNT: u32 = 10;
pub const ATTACH_OUT_FILE_MAX: u64 = 25 * MIB as u64;
/// per turn.
pub const ATTACH_OUT_COUNT: u32 = 10;
pub const ATTACH_OUT_DAILY_PER_CONVERSATION: u64 = 200 * MIB as u64;
pub const ATTACH_CAPTION_MAX: usize = 500;
pub const ATTACH_PENDING_TTL: Duration = Duration::from_secs(86400);
/// (phase 5, P5-UX-17, D-348) how long an agent's open request for access stays open.
pub const ACCESS_REQUEST_TTL: Duration = Duration::from_secs(14 * 86400);
/// (phase 2, RFE-010) bounds of an org's conversation retention window, in days.
pub const RETENTION_MIN_DAYS: i64 = 7;
pub const RETENTION_MAX_DAYS: i64 = 3650;
/// conversations deleted per retention-sweep transaction.
pub const RETENTION_SWEEP_BATCH: i64 = 200;
/// conversations deleted per organization and sweep run.
pub const RETENTION_SWEEP_PER_ORG: i64 = 5_000;
/// (phase 2, RFE-010) one conversation export: bytes and events.
pub const EXPORT_MAX_BYTES: u64 = 256 * MIB as u64;
/// `STORAGE_SNAPSHOT_MAX_AGE` (FIX-126): a Storage page is served from a snapshot at most this
/// old; an older (or missing) one is recomputed once, under a lock, by the next read. The daily
/// attachment sweep and the refresh buttons recompute it too.
pub const STORAGE_SNAPSHOT_MAX_AGE: Duration = Duration::from_secs(3600);
pub const EXPORT_MAX_EVENTS: i64 = 200_000;
pub const SCHEDULE_FIRE_LEASE: Duration = Duration::from_secs(300);
/// consecutive failures.
pub const SCHEDULE_FAILURE_DISABLE: u32 = 5;
/// (P5-SCHED-ONCE, D-421, C-1092) How far ahead a one-time schedule's `run_at` may be: 5 years
/// (1 827 days, two of them leap days).
pub const SCHEDULE_ONCE_MAX_AHEAD: Duration = Duration::from_secs(1827 * 24 * 3600);
/// (P5-SCHED-ONCE) A due one-time schedule of a paused agent is checked again this often.
pub const SCHEDULE_ONCE_RECHECK: Duration = Duration::from_secs(60);
/// (P5-SCHED-ONCE) A Done one-time schedule is deleted this long after it ran.
pub const SCHEDULE_DONE_RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);
/// (P5-REFINE-7, C-1073) Direct conversations (active and archived) per person and agent.
pub const DIRECT_CONVERSATIONS_PER_PERSON_AGENT: i64 = 500;
/// (P5-REFINE-7, C-1074) Run threads kept per schedule or webhook; older ones are deleted by
/// the nightly conversation sweep.
pub const AUTOMATION_THREADS_KEPT: i64 = 200;
pub const GATE_TIMEOUT: Duration = Duration::from_secs(30);
pub const WEBHOOK_PAYLOAD_MAX: usize = MIB;
pub const PROCESS_TAIL: usize = 4 * KIB;
pub const WATCH_PATTERN_MAX: usize = 256;
pub const RUNNER_POLL_WAIT: Duration = Duration::from_secs(25);
pub const REPLICA_ONLINE_THRESHOLD: Duration = Duration::from_secs(90);
/// (FIX-025, D-158) A sandbox the runner reported running whose supervisor has not connected
/// within this fails with `supervisor_unreachable`.
pub const RUNNER_SUPERVISOR_HELLO: Duration = Duration::from_secs(60);
/// (FIX-025) How often each API replica runs the sandbox readiness sweep.
pub const SANDBOX_READINESS_SWEEP: Duration = Duration::from_secs(10);
/// (FIX-029, D-160, C-612) Entries and bytes of a boot's start-log ring
/// (`sandboxes.start_log`): at most 30 entries totalling 8 KiB, oldest dropped; each
/// entry's `detail` is at most 300 chars after sanitising.
pub const SANDBOX_START_LOG_MAX: usize = 30;
/// (FIX-029, D-160, C-612) Total serialized size of one boot's start log.
pub const SANDBOX_START_LOG_BYTES: usize = 8 * KIB;
/// (FIX-029, D-160, C-612) Maximum length of one start-log entry's `detail`.
pub const SANDBOX_START_LOG_DETAIL: usize = 300;
/// (P4-MCON) A connector host not seen for this long leaves the catalogue and `host_running`.
pub const CONNECTOR_HOST_STALE: Duration = Duration::from_secs(90);
/// (P4-MCON) Connector host rows older than this are pruned.
pub const CONNECTOR_HOST_RETAIN: Duration = Duration::from_secs(24 * 3600);
pub const RECONCILE_INTERVAL: Duration = Duration::from_secs(60);
pub const ORPHAN_GRACE: Duration = Duration::from_secs(3600);
pub const SANDBOX_DEFAULT_MEMORY_MIB: u32 = 2048;
pub const SANDBOX_DEFAULT_CPUS: u32 = 1;
pub const SANDBOX_DEFAULT_PIDS: u32 = 512;
pub const SANDBOX_DEFAULT_HOME_MIB: u32 = 10_240;
pub const RUNNER_MAX_SANDBOXES: u32 = 4;
pub const CONNECTOR_OPS_WAIT: Duration = Duration::from_secs(25);
pub const INBOX_LEASE: Duration = Duration::from_secs(60);
pub const OP_LEASE: Duration = Duration::from_secs(60);
/// backoff grows exponentially from `OP_BACKOFF_INITIAL` to `OP_BACKOFF_MAX`.
pub const OP_MAX_ATTEMPTS: u32 = 10;
pub const OP_BACKOFF_INITIAL: Duration = Duration::from_secs(2);
pub const OP_BACKOFF_MAX: Duration = Duration::from_secs(600);
/// (RFE-024) A self-contained `presence.deprovision` op is retried for this long from its
/// creation (instead of `OP_MAX_ATTEMPTS`), then `failed`.
pub const DEPROVISION_RETRY_WINDOW: Duration = Duration::from_secs(24 * 3600);
pub const CHANNEL_LEDGER_RETENTION: Duration = Duration::from_secs(604800);
/// (FIX-111) Webhook delivery rows are kept this long, and at most
/// [`WEBHOOK_DELIVERY_MAX_PER_WEBHOOK`] per webhook (rejected requests write rows too).
pub const WEBHOOK_DELIVERY_RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);
pub const WEBHOOK_DELIVERY_MAX_PER_WEBHOOK: i64 = 10_000;
/// (phase 5, P5-CALINV, D-274, SEC-108) Calendar invitations: sends (invite, update or cancel) per
/// person per agent per hour and per person per day across agents.
pub const CALENDAR_SENDS_PER_AGENT_HOUR: i64 = 20;
pub const CALENDAR_SENDS_PER_PERSON_DAY: i64 = 60;
/// A single or cancelled invitation row is pruned this long after its event ended (or its last
/// change); a repeating one this long after its last change.
pub const CALENDAR_INVITATION_RETENTION: Duration = Duration::from_secs(90 * 24 * 3600);
pub const CALENDAR_REPEATING_RETENTION: Duration = Duration::from_secs(5 * 365 * 24 * 3600);
/// (phase 5, P5-UX-5, D-325, SEC-128) An unused pending workspace access row expires this long after it was added.
pub const PENDING_ACCESS_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
/// (phase 5, P5-UX-5, D-325) Pending access rows per workspace; one more is refused (422 `pending_access_limit`).
pub const PENDING_ACCESS_MAX: i64 = 50;
/// (FIX-111) Revoked or expired sessions and expired context credentials are removed by the
/// daily retention pass this long after expiry or revocation.
pub const SESSION_PRUNE_GRACE: Duration = Duration::from_secs(7 * 24 * 3600);
/// (FIX-111) Rows removed per statement by the daily retention pass (`SKIP LOCKED` batches:
/// no long lock, no unbounded transaction).
pub const RETENTION_PRUNE_BATCH: i64 = 1_000;
/// (FIX-111) Bounds of `expires_in` (seconds) on a reach-grant approval: 1 minute to 365 days.
pub const REACH_EXPIRES_IN_MIN_SECS: u32 = 60;
pub const REACH_EXPIRES_IN_MAX_SECS: u32 = 365 * 24 * 3600;
/// fallback TTL when there is no approval deadline.
pub const ACTION_TOKEN_TTL: Duration = Duration::from_secs(86400);
/// consecutive bot-authored turns.
pub const APP_TURN_CAP: u32 = 5;
/// messages.
pub const PEER_TASK_BUDGET: u32 = 20;
pub const PEER_TASK_TTL: Duration = Duration::from_secs(86400);
/// pending action approvals expire after this (docs/03 `action_approvals.expires_at`: 24 h).
pub const ACTION_APPROVAL_TTL: Duration = Duration::from_secs(86400);
pub const SESSION_IDLE: Duration = Duration::from_secs(1209600);
pub const SESSION_ABSOLUTE: Duration = Duration::from_secs(2592000);
pub const HOOK_MEMORY: usize = 64 * MIB;
pub const HOOK_OUTPUT: usize = 256 * KIB;
/// connect/mint hooks; `approval.summarize` uses `HOOK_WALL_SUMMARIZE`.
pub const HOOK_WALL: Duration = Duration::from_secs(15);
pub const HOOK_WALL_SUMMARIZE: Duration = Duration::from_secs(1);
/// max fetch calls per hook (response/timeout limits below).
pub const HOOK_FETCH_MAX: u32 = 16;
pub const HOOK_FETCH_RESPONSE_MAX: usize = MIB;
pub const HOOK_FETCH_TIMEOUT: Duration = Duration::from_secs(10);
pub const SUMMARY_TITLE_MAX: usize = 120;
/// max summary fields.
pub const SUMMARY_FIELDS_MAX: u32 = 12;
pub const SUMMARY_VALUE_MAX: usize = 200;
pub const SUMMARY_TEXT_MAX: usize = 1_000;
pub const PACKAGE_MAX: usize = 20 * MIB;
/// (FIX-107) `plugin.yaml` of a package (the largest built-in is ~23 KiB).
pub const PACKAGE_MANIFEST_MAX: usize = 256 * KIB;
pub const PACKAGE_ICON_MAX: usize = 256 * KIB;
pub const PACKAGE_SETUP_GUIDE_MAX: usize = 64 * KIB;
pub const AGENT_SNIPPET_MAX: usize = 2_000;
/// (phase 5, P5-GUIDE-1, `docs/24 §8`) `agent.summary`, one line.
pub const AGENT_SUMMARY_MAX: usize = 160;
/// Guide files per app.
pub const AGENT_GUIDE_FILES_MAX: usize = 24;
/// One guide file (UTF-8 bytes).
pub const AGENT_GUIDE_FILE_MAX: usize = 64 * KIB;
/// All guide files of one app.
pub const AGENT_GUIDE_APP_MAX: usize = 256 * KIB;
/// Reference files and recipes projected into one agent's home (P5-GUIDE-2).
pub const AGENT_GUIDE_SYNC_MAX: usize = 2 * 1024 * KIB;
/// (P5-GUIDE-6, `docs/24 §8`) Operations in one operations file.
pub const AGENT_OPERATIONS_MAX: usize = 2_000;
/// One operations file (UTF-8 bytes).
pub const AGENT_OPERATIONS_FILE_MAX: usize = 512 * KIB;
/// The rendered `api.json` of one app and agent.
pub const AGENT_API_JSON_MAX: usize = 512 * KIB;
/// `aurora-agent` stdout preview, in bytes.
pub const AGENT_CLI_PREVIEW_MAX: usize = 2_000;
/// (P5-GUIDE-7, D-297) Platform tools the MCP bridge lists in `on_demand` mode: the registry's
/// core tools plus the supervisor's local core tools. A bigger set is a spec change.
pub const PLATFORM_TOOLS_CORE_MAX: usize = 8;
/// Pages `aurora-agent call --all` follows by default.
pub const AGENT_CLI_PAGES_DEFAULT: u32 = 20;
/// Most pages `--max-pages` may ask for.
pub const AGENT_CLI_PAGES_MAX: u32 = 200;
/// Most bytes `aurora-agent call` saves or prints for one response.
pub const AGENT_CLI_SAVE_MAX: usize = 64 * 1024 * KIB;
pub const AVATAR_MAX: usize = 512 * KIB;
pub const SSE_KEEPALIVE: Duration = Duration::from_secs(15);
pub const LONG_POLL_MAX: Duration = Duration::from_secs(30);

// SCIM provisioning (phase 4, P4-GRP-2, D-132; docs/02 §Limits, docs/04 §SCIM).
/// `SCIM_RATE`: requests per minute per token (burst is a fifth of it); `scim.rate_per_minute`.
pub const SCIM_RATE: u32 = 600;
/// `SCIM_BODY_MAX`.
pub const SCIM_BODY_MAX: usize = MIB;
/// `SCIM_MAX_PATCH_OPS`.
pub const SCIM_MAX_PATCH_OPS: usize = 100;
/// `SCIM_MAX_MEMBERS_PER_REQUEST`.
pub const SCIM_MAX_MEMBERS_PER_REQUEST: usize = 1_000;
/// `SCIM_PAGE_MAX`.
pub const SCIM_PAGE_MAX: usize = 100;
/// `SCIM_PAGE_DEFAULT`.
pub const SCIM_PAGE_DEFAULT: usize = 50;
/// `SCIM_TOKENS_PER_ORG`: active (unrevoked, unexpired) tokens per org.
pub const SCIM_TOKENS_PER_ORG: usize = 5;
/// `SCIM_TOKEN_DEFAULT_DAYS` (the dashboard's offered default; never above the maximum).
pub const SCIM_TOKEN_DEFAULT_DAYS: u32 = 365;
/// `SCIM_TOKEN_MAX_DAYS`: longest token lifetime (`scim.token_max_days`; no non-expiring token).
pub const SCIM_TOKEN_MAX_DAYS: u32 = 730;
/// `SCIM_ERROR_LOG`: last N sanitized errors kept per org (Directory sync page).
pub const SCIM_ERROR_LOG: usize = 20;

// Port previews (phase 4, P4-PRV-1, D-144, D-145; docs/02 §Limits, docs/11 §Port previews).
/// shares per hosted agent; beyond it 409 `preview_limit`.
pub const PREVIEW_SHARES_PER_AGENT: u32 = 10;
/// shares per organization; beyond it 409 `preview_limit`.
pub const PREVIEW_SHARES_PER_ORG: u32 = 200;
/// max length of a share's start command (one line).
pub const PREVIEW_START_COMMAND_MAX: usize = 1024;
/// max length of a share's `cwd` (relative to `/workspace`, no `..`).
pub const PREVIEW_CWD_MAX: usize = 200;
/// a preview wake demand stays due work for this long (the start arm).
pub const PREVIEW_WAKE_WINDOW: Duration = Duration::from_secs(60);
/// the relay waits this long for a woken port, then answers its 503 page.
pub const PREVIEW_WAKE_WAIT: Duration = Duration::from_secs(45);
/// a request to a running sandbox whose port is not listening (no start command) waits this long.
pub const PREVIEW_LISTEN_WAIT: Duration = Duration::from_secs(5);
/// wakes per share per hour; beyond it 429 `preview_wake_limited`.
pub const PREVIEW_WAKES_PER_SHARE_HOUR: u32 = 12;
/// launches of a share's start command per share per `PREVIEW_START_WINDOW`.
pub const PREVIEW_START_ATTEMPTS: u32 = 3;
pub const PREVIEW_START_WINDOW: Duration = Duration::from_secs(600);
/// each relay heartbeat of an open/recent stream refreshes `preview_active_until` by this much.
pub const PREVIEW_AWAKE_GRACE: Duration = Duration::from_secs(120);
/// the keep-awake reason `preview` ends this long after the boot's first heartbeat (`preview_awake_since`).
pub const PREVIEW_AWAKE_CAP: Duration = Duration::from_secs(4 * 3600);
/// the label of a share hostname: `p` + this many base32 chars (120 bits of CSPRNG).
pub const PREVIEW_LABEL_CHARS: usize = 24;

// Port preview access and byte streams (phase 4, P4-PRV-2, D-146, D-147; docs/02 §Limits,
// docs/04 §Preview access, docs/16 §W17/§W18).
/// absolute life of a preview session (never past the dashboard session's own expiry).
pub const PREVIEW_SESSION_TTL: Duration = Duration::from_secs(8 * 3600);
/// a preview session unused for this long is dead.
pub const PREVIEW_SESSION_IDLE: Duration = Duration::from_secs(3600);
/// single-use browser authorization grant (sealed, docs/04 §Preview access).
pub const PREVIEW_GRANT_TTL: Duration = Duration::from_secs(60);
/// single-use stream grant (`aux_`, redeemed by the runner at the API).
pub const PREVIEW_STREAM_GRANT_TTL: Duration = Duration::from_secs(30);
/// relay cache of a resolve: 15 s positive, 5 s negative; also the open-stream
/// revalidation interval (worst-case revocation latency, SEC-81).
pub const PREVIEW_AUTH_CACHE: Duration = Duration::from_secs(15);
pub const PREVIEW_AUTH_CACHE_NEG: Duration = Duration::from_secs(5);
/// concurrent streams per share / per sandbox / per user. Per share and per user are counted per
/// relay replica and enforced in `aurora-relay` (`stream::Streams::reserve`, FIX-072); per
/// sandbox is enforced in the supervisor.
pub const PREVIEW_STREAMS_PER_SHARE: u32 = 32;
pub const PREVIEW_STREAMS_PER_SANDBOX: u32 = 64;
pub const PREVIEW_STREAMS_PER_USER: u32 = 64;
/// (phase 5, P5-REFINE-9, C-1084) open agent-timeline streams per person (per API replica);
/// 429 `rate_limited` beyond.
pub const TIMELINE_STREAMS_PER_USER: u32 = 5;
/// credit window per direction per stream; a chunk never exceeds `PREVIEW_CHUNK`.
pub const PREVIEW_STREAM_WINDOW: usize = 128 * KIB;
pub const PREVIEW_CHUNK: usize = 16 * KIB;
/// idle timeouts of a leg-1 stream (plain HTTP vs upgraded) and its total lifetime.
pub const PREVIEW_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
pub const PREVIEW_WS_IDLE_TIMEOUT: Duration = Duration::from_secs(600);
pub const PREVIEW_STREAM_LIFETIME: Duration = Duration::from_secs(4 * 3600);
/// the runner's stream-open round trip (grant redemption + `stream.open`).
pub const PREVIEW_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// the supervisor's loopback connect timeout (`docs/16 §W18`).
pub const PREVIEW_SUPERVISOR_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// upstream response headers must arrive within this.
pub const PREVIEW_HEADER_TIMEOUT: Duration = Duration::from_secs(60);
/// request bodies proxied to a preview app, at most.
pub const PREVIEW_REQUEST_BODY_MAX: usize = 100 * MIB;
/// relayed bytes per share per UTC day; beyond it 429 `preview_quota`.
pub const PREVIEW_BYTES_PER_SHARE_DAY: u64 = 5 * 1024 * 1024 * 1024;
/// relay-plane requests/s per relay replica (burst is double).
pub const PREVIEW_API_RATE: u32 = 100;
/// grant redemptions per minute per client address (and per share) at the relay.
pub const PREVIEW_GRANT_RATE: u32 = 30;

// Web terminal (phase 4, P4-TERM, D-161, SEC-87; docs/02 §Limits, docs/16 §W19,
// docs/11 §Web terminal).
/// single-use terminal stream grant (`auy_`, redeemed by the API at the runner).
pub const TERMINAL_GRANT_TTL: Duration = Duration::from_secs(30);
/// max bytes of one terminal data chunk (tag `0x02`).
pub const TERMINAL_CHUNK: usize = 16 * KIB;
/// credit window per direction per PTY; a chunk never exceeds `TERMINAL_CHUNK`.
pub const TERMINAL_STREAM_WINDOW: usize = 128 * KIB;
/// concurrent terminal sessions per sandbox / per agent / per user (per API replica).
pub const TERMINALS_PER_SANDBOX: u32 = 2;
pub const TERMINALS_PER_AGENT: u32 = 4;
pub const TERMINALS_PER_USER: u32 = 2;
/// (phase 5, P5-FILES-1, D-316) concurrent `exec.*` children per sandbox at the supervisor
/// (`SSH_CHANNELS_PER_SANDBOX` 8 + `FILE_SESSIONS_PER_SANDBOX` 4; the per-kind caps are the
/// runner's), separate from `TERMINALS_PER_SANDBOX`.
pub const EXECS_PER_SANDBOX: u32 = 12;
/// (phase 5, P5-FILES-1) live `auf_` files legs per sandbox at the runner (close 4409).
pub const FILE_SESSIONS_PER_SANDBOX: u32 = 4;
/// (phase 5, P4-SSH-2, D-239) live `auz_` SSH channels per sandbox at the runner (close 4409).
pub const SSH_CHANNELS_PER_SANDBOX: u32 = 8;
/// (16 §W20) the longest `exec` command line (no NUL) and the most `env` variables a channel
/// passes (each ≤ 256 bytes); checked by the terminator, the runner and the supervisor.
pub const SSH_EXEC_COMMAND_MAX: usize = 8 * KIB;
pub const SSH_ENV_VARS_MAX: usize = 16;
/// (phase 5, P5-FILES-1, C-901) single-use `auf_` files grant lifetime.
pub const FILES_GRANT_TTL: Duration = Duration::from_secs(30);
/// (C-901) a pooled SFTP session without a request closes after this.
pub const FILES_SESSION_IDLE: Duration = Duration::from_secs(60);
/// (C-901) a pooled SFTP session is reopened transparently after this.
pub const FILES_SESSION_LIFETIME: Duration = Duration::from_secs(3600);
/// (D-316(e)) entries per listing (`truncated` beyond).
pub const FILES_LIST_MAX: usize = 5_000;
/// (D-316(e)) entries per recursive delete (422 `too_many_entries`, nothing deleted).
pub const FILES_DELETE_MAX: usize = 10_000;
/// (D-316(e)) bytes per uploaded file (413 `file_too_large`).
pub const FILES_UPLOAD_MAX: u64 = 512 * 1024 * 1024;
/// (D-316(e)) bytes of UTF-8 text opened in the editor.
pub const FILES_EDIT_MAX: u64 = 1024 * 1024;
/// (C-899) bytes per request path (255 per component).
pub const FILES_PATH_MAX: usize = 4_096;
/// (P5-MODELS-1, D-303, SEC-122) characters per model id (default, role, `model_info` key).
pub const MODEL_ID_MAX: usize = 200;
/// (P5-MODELS-1, D-303) `model_info` entries per LLM endpoint.
pub const MODEL_INFO_MAX: usize = 64;
/// (P5-MODELS-2, D-304, C-879) `by_model` entries per turn; the rest are folded into the default's entry.
pub const USAGE_BY_MODEL_MAX: usize = 16;
/// (P5-USAGE-1, C-1000) Days a usage query may span (from..=to inclusive); a longer range is 422
/// `usage_range_invalid` (`too_long`).
pub const USAGE_RANGE_MAX_DAYS: i64 = 366;
/// (P5-USAGE-1, C-1003, C-1006) Workspaces listed individually by the instance and organization usage
/// routes; the rest are summed in `other_workspaces`.
pub const USAGE_TOP_MAX: usize = 50;
/// (C-900) concurrent content transfers per person per API replica (429 `too_many_transfers`).
pub const FILE_TRANSFERS_PER_USER: u32 = 4;
/// (C-900) a transfer with no progress this long is aborted.
pub const FILES_TRANSFER_IDLE: Duration = Duration::from_secs(60);
/// (C-900) the Files keep-awake grace after the last request.
pub const FILES_AWAKE_GRACE: Duration = Duration::from_secs(120);
/// (C-900) per-boot cap on Files-held wakefulness.
pub const FILES_AWAKE_CAP: Duration = Duration::from_secs(4 * 3600);
/// idle timeout of an open terminal session (a WebSocket ping counts).
pub const TERMINAL_IDLE_TIMEOUT: Duration = Duration::from_secs(600);
/// total life of one terminal session.
pub const TERMINAL_MAX_LIFETIME: Duration = Duration::from_secs(4 * 3600);
/// (FIX-146, D-232 (g)) cap on the parameterised SQL in a database span's `db.query.text`.
pub const DB_SPAN_STATEMENT_MAX: usize = 4 * KIB;
/// (FIX-146 Part D, C-732 (6)) request body of `POST /v1/telemetry/traces` (413 above it).
pub const TELEMETRY_INGEST_BODY_MAX: usize = 256 * KIB;
/// (FIX-146 Part D) spans per `POST /v1/telemetry/traces` request (400 above it).
pub const TELEMETRY_INGEST_SPANS_MAX: usize = 512;
/// (FIX-146 Part D) attributes per browser span (400 above it).
pub const TELEMETRY_SPAN_ATTRIBUTES_MAX: usize = 32;
/// (FIX-146 Part D) events per browser span (400 above it; events are never exported).
pub const TELEMETRY_SPAN_EVENTS_MAX: usize = 8;
/// (FIX-146 Part D) kept bytes of a browser span's `exception.stacktrace`.
pub const TELEMETRY_STACKTRACE_MAX: usize = 4 * KIB;
/// (FIX-146 Part D) browser span timestamps older than this are clamped up to `now - this`.
pub const TELEMETRY_CLOCK_PAST: Duration = Duration::from_secs(15 * 60);
/// (FIX-146 Part D) browser span timestamps later than `now + this` are clamped down.
pub const TELEMETRY_CLOCK_FUTURE: Duration = Duration::from_secs(60);
/// (FIX-146 Part D) longest browser span duration kept (an end later than `start + this` is clamped).
pub const TELEMETRY_SPAN_DURATION_MAX: Duration = Duration::from_secs(15 * 60);
/// each keep-awake stamp of a byte-carrying session refreshes `terminal_active_until` by this much.
pub const TERMINAL_AWAKE_GRACE: Duration = Duration::from_secs(120);
/// the keep-awake reason `terminal` ends this long after the boot's first session byte
/// (`terminal_awake_since`).
pub const TERMINAL_AWAKE_CAP: Duration = Duration::from_secs(4 * 3600);
/// a terminal wake demand stays due work for this long (the start arm).
pub const TERMINAL_WAKE_WINDOW: Duration = Duration::from_secs(60);
/// how long the WebSocket upgrade may wait for a waking sandbox.
pub const TERMINAL_WAKE_WAIT: Duration = Duration::from_secs(45);
/// wakes per agent per hour; beyond it 429.
pub const TERMINAL_WAKES_PER_AGENT_HOUR: u32 = 12;
/// the `pty.open` round trip to the supervisor (an older supervisor answers nothing → 4502).
pub const TERMINAL_PTY_OPEN_TIMEOUT: Duration = Duration::from_secs(5);
/// (phase 5, P4-SSH-1, D-239(b), C-790) registered SSH public keys per person (409
/// `ssh_key_limit`).
pub const SSH_KEYS_PER_USER: usize = 10;
/// (phase 5, P4-UTOK, D-284(c), C-853) chat-platform tokens a person keeps (`active` or
/// `invalid`; 409 `user_token_limit`).
pub const USER_TOKENS_PER_PERSON: usize = 5;
/// (P4-UTOK, D-284(c), C-856) pending `send_as_me` approvals per token (the sixth is refused
/// `pending_limit`).
pub const USER_TOKEN_PENDING_MAX: usize = 5;
/// (phase 5, P4-EVAL-1, D-293(c), C-861, C-872) eval questions per agent (422 `eval_question_limit`).
pub const EVAL_QUESTIONS_PER_AGENT: usize = 100;
/// (P4-EVAL-1, C-861) characters of an eval question (422 `invalid`).
pub const EVAL_QUESTION_CHARS: usize = 2000;
/// (P4-EVAL-1, C-861) characters of an expected text answer (422 `invalid`).
pub const EVAL_EXPECTED_TEXT_CHARS: usize = 1000;
/// (P4-EVAL-1, C-861) characters of judged criteria (422 `invalid`).
pub const EVAL_CRITERIA_CHARS: usize = 1000;
/// (P4-EVAL-2, C-867) characters of an answer kept on an eval result (sanitised).
pub const EVAL_ANSWER_CHARS: usize = 4000;
/// (P4-EVAL-2, D-290, C-865) per eval question: the turn is aborted and the question is `error`
/// (`timeout`) after this long.
pub const EVAL_TURN_TIMEOUT: Duration = Duration::from_secs(900);
/// (P4-EVAL-2, D-290(d), C-864) active (`queued` or `running`) eval runs per agent (409
/// `eval_run_active`).
pub const EVAL_RUNS_ACTIVE_PER_AGENT: usize = 1;
/// (P4-EVAL-2, D-293(c), C-864) eval run starts per agent per UTC day, manual and scheduled
/// together (429 `eval_runs_limited`).
pub const EVAL_RUNS_PER_AGENT_DAY: usize = 20;
/// (P4-EVAL-2, D-293(b), C-870) the smallest gap between eval schedule occurrences.
pub const EVAL_SCHEDULE_MIN_INTERVAL: Duration = Duration::from_secs(3600);
/// (P4-EVAL-2, C-870) eval schedules per agent (422 `eval_schedule_limit`).
pub const EVAL_SCHEDULES_PER_AGENT: usize = 5;
/// (P4-EVAL-2, D-294, C-872) a finished run, its results and its eval conversations are deleted
/// this long after it finishes.
pub const EVAL_RUN_RETENTION: Duration = Duration::from_secs(90 * 24 * 3600);
/// (P4-EVAL-2, D-293(d), C-871) percentage points of pass rate below the previous finished run
/// of the same question-set version at which a regression is recorded.
pub const EVAL_REGRESSION_DROP: f64 = 10.0;
/// (P4-EVAL-3, D-292, C-869) one judge call (the request through `PlatformLlm`) is abandoned after
/// this long: `judge_timeout`, an `error` for that question.
pub const EVAL_JUDGE_TIMEOUT: Duration = Duration::from_secs(60);
/// (P4-EVAL-3, C-869) `max_tokens` of a judge request.
pub const EVAL_JUDGE_MAX_TOKENS: u32 = 512;
/// (P4-EVAL-3, SEC-115) bytes of the question's own answer sent to the judge; the rest is cut.
pub const EVAL_JUDGE_INPUT_MAX: usize = 8 * 1024;
/// (P4-EVAL-3, C-869) characters of the judge's rationale kept (one line, sanitised); longer is
/// cut, never a failure.
pub const EVAL_RATIONALE_CHARS: usize = 300;
/// (P4-SSH-1, C-791) default validity of a minted SSH user certificate; `ssh.cert_ttl_secs`
/// (60 … 86 400 s) overrides it. `valid_after` is backdated 60 s for clock skew.
pub const SSH_CERT_TTL: Duration = Duration::from_secs(900);
/// (P4-SSH-1, D-239(c)) certificate mints per person per agent per clock hour (429
/// `ssh_mint_rate_limited`); counted exactly across replicas in `ssh_mint_counters`.
pub const SSH_MINTS_PER_USER_AGENT_HOUR: u32 = 30;
/// (phase 5, P4-SSH-2, D-239, C-794) an SSH session is alive while its lease is current; every
/// heartbeat that keeps it renews it to `now() + SSH_SESSION_LEASE`.
pub const SSH_SESSION_LEASE: Duration = Duration::from_secs(90);
/// (C-794) the terminator's heartbeat period (`TerminatorConfig.heartbeat_secs`).
pub const SSH_HEARTBEAT: Duration = Duration::from_secs(30);
/// (C-794) open SSH sessions per person / per agent, counted over open lease-current rows
/// (global); `ssh.sessions_per_user` / `ssh.sessions_per_agent` may only lower them.
pub const SSH_SESSIONS_PER_USER: u32 = 4;
pub const SSH_SESSIONS_PER_AGENT: u32 = 8;
/// (C-794, 16 §W20) concurrent session channels per SSH connection (the terminator enforces it;
/// the API reports it in `TerminatorConfig`).
pub const SSH_CHANNELS_PER_SESSION: u32 = 4;
/// (C-794) single-use `auz_` channel grant lifetime.
pub const SSH_GRANT_TTL: Duration = Duration::from_secs(30);
/// (C-794) default idle timeout and maximum lifetime of one SSH connection;
/// `ssh.idle_timeout_secs` may only lower the idle value.
pub const SSH_IDLE_TIMEOUT: Duration = Duration::from_secs(600);
pub const SSH_MAX_LIFETIME: Duration = Duration::from_secs(4 * 3600);
/// (C-796, D-239(h)) SSH keep-awake: grace after the last channel byte and the cap per boot
/// (`ssh.awake_cap_secs`, at most `BACKGROUND_CAP`).
pub const SSH_AWAKE_GRACE: Duration = Duration::from_secs(120);
pub const SSH_AWAKE_CAP: Duration = Duration::from_secs(4 * 3600);
/// (C-794) how long a channel waits for a waking sandbox (`ssh.wake_wait_secs` may only lower it).
pub const SSH_WAKE_WAIT: Duration = Duration::from_secs(90);
/// (C-794) one long-poll of `POST /v1/terminator/sessions/{id}/channels` (`wait_secs` ≤ this).
pub const SSH_CHANNEL_POLL_MAX: Duration = Duration::from_secs(45);
/// (C-794) terminator-plane requests per second per replica, burst `2 ×` it.
pub const SSH_API_RATE: u32 = 50;
/// (phase 5, P4-SSH-2, 16 §W20, D-356) the terminator's authentication limits: real attempts per
/// connection before it ends (the `none` probe is not one; russh's own backstop is one more),
/// the delay of every refusal, and the time from accept to authenticated.
pub const SSH_AUTH_ATTEMPTS: usize = 6;
pub const SSH_AUTH_FAILURE_DELAY: Duration = Duration::from_secs(1);
pub const SSH_AUTH_TIMEOUT: Duration = Duration::from_secs(20);
/// (16 §W20) per replica: unauthenticated connections in total / per client address, and the
/// per-address token bucket (connections per minute, burst); `[terminator]` may raise them.
pub const SSH_PREAUTH_MAX: u32 = 256;
pub const SSH_PREAUTH_PER_IP: u32 = 8;
pub const SSH_CONNECT_RATE: u32 = 30;
pub const SSH_CONNECT_BURST: u32 = 10;
/// (16 §W20) consecutive failed heartbeats after which the terminator closes every session and
/// refuses new authentications until one succeeds.
pub const SSH_HEARTBEAT_FAILURES: u32 = 3;
/// (03 §ssh_sessions) closed session rows are kept this long; grants are pruned after 24 h.
pub const SSH_SESSION_RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spot_values() {
        assert_eq!(APPROVAL_TIMEOUT, Duration::from_secs(300));
        assert_eq!(SESSION_ABSOLUTE, Duration::from_secs(2_592_000));
        assert_eq!(ATTACH_OUT_DAILY_PER_CONVERSATION, 209_715_200);
        assert_eq!(CONDITION_BODY_BUFFER, 262_144);
        assert_eq!(SKILL_TOTAL_MAX, 32_000);
        assert_eq!(WEBHOOK_PAYLOAD_MAX, 1_048_576);
        assert_eq!(SCIM_RATE, 600);
        assert_eq!(SCIM_BODY_MAX, 1_048_576);
        assert_eq!(SCIM_PAGE_MAX, 100);
        assert_eq!(SCIM_PAGE_DEFAULT, 50);
        assert_eq!(SCIM_TOKEN_MAX_DAYS, 730);
        assert_eq!(PREVIEW_SHARES_PER_AGENT, 10);
        assert_eq!(PREVIEW_SHARES_PER_ORG, 200);
        assert_eq!(PREVIEW_WAKE_WINDOW, Duration::from_secs(60));
        assert_eq!(PREVIEW_AWAKE_CAP, Duration::from_secs(4 * 3600));
        assert_eq!(PREVIEW_SESSION_TTL, Duration::from_secs(28_800));
        assert_eq!(PREVIEW_SESSION_IDLE, Duration::from_secs(3_600));
        assert_eq!(PREVIEW_GRANT_TTL, Duration::from_secs(60));
        assert_eq!(PREVIEW_STREAM_GRANT_TTL, Duration::from_secs(30));
        assert_eq!(PREVIEW_AUTH_CACHE, Duration::from_secs(15));
        assert_eq!(PREVIEW_STREAMS_PER_SHARE, 32);
        assert_eq!(PREVIEW_STREAMS_PER_SANDBOX, 64);
        assert_eq!(PREVIEW_STREAM_WINDOW, 131_072);
        assert_eq!(PREVIEW_CHUNK, 16_384);
        assert_eq!(PREVIEW_IDLE_TIMEOUT, Duration::from_secs(120));
        assert_eq!(PREVIEW_WS_IDLE_TIMEOUT, Duration::from_secs(600));
        assert_eq!(PREVIEW_STREAM_LIFETIME, Duration::from_secs(14_400));
        assert_eq!(PREVIEW_REQUEST_BODY_MAX, 104_857_600);
        assert_eq!(PREVIEW_BYTES_PER_SHARE_DAY, 5 * 1024 * 1024 * 1024);
        assert_eq!(PREVIEW_API_RATE, 100);
        assert_eq!(TERMINAL_GRANT_TTL, Duration::from_secs(30));
        assert_eq!(TERMINAL_CHUNK, 16_384);
        assert_eq!(TERMINAL_STREAM_WINDOW, 131_072);
        assert_eq!(TERMINALS_PER_SANDBOX, 2);
        assert_eq!(TERMINALS_PER_AGENT, 4);
        assert_eq!(TERMINALS_PER_USER, 2);
        assert_eq!(EXECS_PER_SANDBOX, 12);
        assert_eq!(FILE_SESSIONS_PER_SANDBOX, 4);
        assert_eq!(FILES_GRANT_TTL, Duration::from_secs(30));
        assert_eq!(FILES_SESSION_IDLE, Duration::from_secs(60));
        assert_eq!(FILES_SESSION_LIFETIME, Duration::from_secs(3600));
        assert_eq!(FILES_LIST_MAX, 5_000);
        assert_eq!(FILES_DELETE_MAX, 10_000);
        assert_eq!(FILES_UPLOAD_MAX, 512 * 1024 * 1024);
        assert_eq!(FILES_EDIT_MAX, 1024 * 1024);
        assert_eq!(FILES_PATH_MAX, 4_096);
        assert_eq!(MODEL_ID_MAX, 200);
        assert_eq!(MODEL_INFO_MAX, 64);
        assert_eq!(USAGE_BY_MODEL_MAX, 16);
        assert_eq!(USAGE_RANGE_MAX_DAYS, 366);
        assert_eq!(USAGE_TOP_MAX, 50);
        assert_eq!(FILE_TRANSFERS_PER_USER, 4);
        assert_eq!(FILES_TRANSFER_IDLE, Duration::from_secs(60));
        assert_eq!(FILES_AWAKE_GRACE, Duration::from_secs(120));
        assert_eq!(FILES_AWAKE_CAP, Duration::from_secs(4 * 3600));
        assert_eq!(TERMINAL_IDLE_TIMEOUT, Duration::from_secs(600));
        assert_eq!(TERMINAL_MAX_LIFETIME, Duration::from_secs(14_400));
        assert_eq!(TERMINAL_AWAKE_GRACE, Duration::from_secs(120));
        assert_eq!(TERMINAL_AWAKE_CAP, Duration::from_secs(4 * 3600));
        assert_eq!(TERMINAL_WAKE_WINDOW, Duration::from_secs(60));
        assert_eq!(TERMINAL_WAKE_WAIT, Duration::from_secs(45));
        assert_eq!(TERMINAL_WAKES_PER_AGENT_HOUR, 12);
        assert_eq!(TERMINAL_PTY_OPEN_TIMEOUT, Duration::from_secs(5));
        assert_eq!(SSH_KEYS_PER_USER, 10);
        assert_eq!(USER_TOKENS_PER_PERSON, 5);
        assert_eq!(USER_TOKEN_PENDING_MAX, 5);
        assert_eq!(EVAL_QUESTIONS_PER_AGENT, 100);
        assert_eq!(EVAL_QUESTION_CHARS, 2000);
        assert_eq!(EVAL_EXPECTED_TEXT_CHARS, 1000);
        assert_eq!(EVAL_CRITERIA_CHARS, 1000);
        assert_eq!(EVAL_ANSWER_CHARS, 4000);
        assert_eq!(EVAL_TURN_TIMEOUT, Duration::from_secs(900));
        assert!(EVAL_TURN_TIMEOUT <= TURN_CEILING);
        assert_eq!(EVAL_RUNS_ACTIVE_PER_AGENT, 1);
        assert_eq!(EVAL_RUNS_PER_AGENT_DAY, 20);
        assert_eq!(EVAL_SCHEDULE_MIN_INTERVAL, Duration::from_secs(3600));
        assert_eq!(EVAL_SCHEDULES_PER_AGENT, 5);
        assert_eq!(EVAL_RUN_RETENTION, Duration::from_secs(90 * 24 * 3600));
        assert!((EVAL_REGRESSION_DROP - 10.0).abs() < f64::EPSILON);
        assert_eq!(EVAL_JUDGE_TIMEOUT, Duration::from_secs(60));
        assert_eq!(EVAL_JUDGE_MAX_TOKENS, 512);
        assert_eq!(EVAL_JUDGE_INPUT_MAX, 8192);
        assert_eq!(EVAL_RATIONALE_CHARS, 300);
        assert_eq!(SSH_CERT_TTL, Duration::from_secs(900));
        assert_eq!(SSH_MINTS_PER_USER_AGENT_HOUR, 30);
        const { assert!(CONDITION_BODY_BUFFER_MIN <= CONDITION_BODY_BUFFER) };
        const { assert!(CONDITION_BODY_BUFFER <= CONDITION_BODY_BUFFER_MAX) };
    }
}
