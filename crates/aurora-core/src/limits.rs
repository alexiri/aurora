//! Limits registry (`02 §Limits registry`). Dependency-free (std only).

use std::time::Duration;

const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;

pub const GATEWAY_RESOLVE_CACHE_TTL: Duration = Duration::from_secs(60);
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
pub const TURN_STALL: Duration = Duration::from_secs(600);
pub const TURN_PROGRESS_INTERVAL: Duration = Duration::from_secs(60);
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
pub const ATTACH_IN_FILE_MAX: u64 = 10 * MIB as u64;
/// per message.
pub const ATTACH_IN_COUNT: u32 = 10;
pub const ATTACH_OUT_FILE_MAX: u64 = 25 * MIB as u64;
/// per turn.
pub const ATTACH_OUT_COUNT: u32 = 10;
pub const ATTACH_OUT_DAILY_PER_CONVERSATION: u64 = 200 * MIB as u64;
pub const ATTACH_CAPTION_MAX: usize = 500;
pub const ATTACH_PENDING_TTL: Duration = Duration::from_secs(86400);
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
pub const PACKAGE_ICON_MAX: usize = 256 * KIB;
pub const PACKAGE_SETUP_GUIDE_MAX: usize = 64 * KIB;
pub const AGENT_SNIPPET_MAX: usize = 2_000;
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
/// idle timeout of an open terminal session (a WebSocket ping counts).
pub const TERMINAL_IDLE_TIMEOUT: Duration = Duration::from_secs(600);
/// total life of one terminal session.
pub const TERMINAL_MAX_LIFETIME: Duration = Duration::from_secs(4 * 3600);
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
/// revocation cache: freeze/disable closes sessions within this (mirroring previews).
pub const TERMINAL_AUTH_CACHE: Duration = Duration::from_secs(15);
/// the `pty.open` round trip to the supervisor (an older supervisor answers nothing → 4502).
pub const TERMINAL_PTY_OPEN_TIMEOUT: Duration = Duration::from_secs(5);

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
        assert_eq!(TERMINAL_IDLE_TIMEOUT, Duration::from_secs(600));
        assert_eq!(TERMINAL_MAX_LIFETIME, Duration::from_secs(14_400));
        assert_eq!(TERMINAL_AWAKE_GRACE, Duration::from_secs(120));
        assert_eq!(TERMINAL_AWAKE_CAP, Duration::from_secs(4 * 3600));
        assert_eq!(TERMINAL_WAKE_WINDOW, Duration::from_secs(60));
        assert_eq!(TERMINAL_WAKE_WAIT, Duration::from_secs(45));
        assert_eq!(TERMINAL_WAKES_PER_AGENT_HOUR, 12);
        assert_eq!(TERMINAL_AUTH_CACHE, Duration::from_secs(15));
        assert_eq!(TERMINAL_PTY_OPEN_TIMEOUT, Duration::from_secs(5));
        const { assert!(CONDITION_BODY_BUFFER_MIN <= CONDITION_BODY_BUFFER) };
        const { assert!(CONDITION_BODY_BUFFER <= CONDITION_BODY_BUFFER_MAX) };
    }
}
