//! Gateway reads/writes: credential identity lookup and instance bookkeeping
//! (docs/06 §Credential authentication, docs/03 §gateway_instances).

use crate::DbError;
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CredentialIdentityRow {
    pub credential_id: Uuid,
    pub kind: String,
    pub agent_id: Uuid,
    pub workspace_id: Uuid,
    pub org_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    /// `gateway_credentials.trace_context` (FIX-146, D-232 (7)): the turn's W3C traceparent,
    /// copied at mint (the gateway role has no grant on `turns`).
    pub trace_context: Option<String>,
    pub context_label: Option<String>,
    /// `gateway_credentials.speaker_user_id` (RFE-005): hosted context credentials only.
    pub speaker_user_id: Option<Uuid>,
    /// `gateway_credentials.capabilities` (RFE-003).
    pub capabilities: Vec<String>,
    /// Seconds until `expires_at` (None = no expiry).
    pub expires_in_secs: Option<f64>,
    /// `agents.kind` (`byo` | `hosted`); BYO admission applies to `byo` only (RFE-025).
    pub agent_kind: String,
    pub org_slug: String,
    pub ws_slug: String,
    /// `workspaces.byo_posture` (RFE-025).
    pub ws_byo_posture: Option<String>,
    /// `organizations.governance -> 'byo'` (RFE-025).
    pub org_byo: Option<serde_json::Value>,
    /// `agents.byo_source_cidrs` / `byo_require_context` (RFE-025).
    pub agent_byo_source_cidrs: Option<Vec<String>>,
    pub agent_byo_require_context: bool,
    /// (RFE-006) Where the freeze that stops this agent is set (`org` | `workspace` | `agent`,
    /// broadest first); `None` when the agent is not effectively frozen.
    pub frozen_level: Option<String>,
}

const IDENTITY_SELECT: &str = "SELECT c.id AS credential_id, c.kind, c.agent_id, a.workspace_id, w.org_id, \
                c.conversation_id, c.turn_id, c.trace_context, c.context_label, c.speaker_user_id, c.capabilities, \
                EXTRACT(EPOCH FROM (c.expires_at - now()))::float8 AS expires_in_secs, \
                a.kind AS agent_kind, o.slug AS org_slug, w.slug AS ws_slug, \
                w.byo_posture AS ws_byo_posture, o.governance -> 'byo' AS org_byo, \
                a.byo_source_cidrs AS agent_byo_source_cidrs, \
                a.byo_require_context AS agent_byo_require_context, \
                CASE WHEN o.frozen_at IS NOT NULL THEN 'org' \
                     WHEN w.frozen_at IS NOT NULL THEN 'workspace' \
                     WHEN a.frozen_at IS NOT NULL THEN 'agent' END AS frozen_level \
         FROM gateway_credentials c \
         JOIN agents a ON a.id = c.agent_id \
         JOIN workspaces w ON w.id = a.workspace_id \
         JOIN organizations o ON o.id = w.org_id \
         WHERE c.revoked_at IS NULL \
           AND EXISTS (SELECT 1 FROM org_members m \
                       WHERE m.org_id = o.id AND m.role = 'owner' AND m.status = 'active')";

const NOT_EXPIRED: &str = " AND (c.expires_at IS NULL OR c.expires_at > now())";

/// Live credential by SHA-256 hash: not revoked, not expired, agent/workspace/org
/// present, and the org has at least one active owner.
pub async fn find_credential_identity<'e, E: PgExecutor<'e>>(
    exec: E,
    token_hash: &[u8],
) -> Result<Option<CredentialIdentityRow>, DbError> {
    Ok(
        sqlx::query_as::<_, CredentialIdentityRow>(sqlx::AssertSqlSafe(format!(
            "{IDENTITY_SELECT}{NOT_EXPIRED} AND c.token_hash = $1"
        )))
        .bind(token_hash)
        .fetch_optional(exec)
        .await?,
    )
}

/// (RFE-013) Like [`find_credential_identity`] but an expired credential is returned too
/// (`expires_in_secs <= 0`), so a caller can tell an expired agent token (`agent_token_expired`)
/// from an unknown or revoked one.
pub async fn find_credential_identity_incl_expired<'e, E: PgExecutor<'e>>(
    exec: E,
    token_hash: &[u8],
) -> Result<Option<CredentialIdentityRow>, DbError> {
    Ok(
        sqlx::query_as::<_, CredentialIdentityRow>(sqlx::AssertSqlSafe(format!(
            "{IDENTITY_SELECT} AND c.token_hash = $1"
        )))
        .bind(token_hash)
        .fetch_optional(exec)
        .await?,
    )
}

/// (FIX-050, C-630) Classification of a credential row that the identity lookup refused: is it
/// revoked, is it past `expires_at`, and does its organization have an active owner? `None` = no
/// such row at all (a plain unknown token). Only reached when
/// [`find_credential_identity_incl_expired`] found nothing, so a live, non-revoked row of an org
/// with an active owner never lands here; the expired fact disambiguates an expired token of an
/// ownerless org (`expired`, the token-level fact, outranks `no_active_owner` — restoring an
/// owner would not un-expire it).
#[derive(Debug, sqlx::FromRow)]
pub struct RefusedCredentialRow {
    pub kind: String,
    pub revoked: bool,
    pub expired: bool,
    pub has_owner: bool,
}

pub async fn find_refused_credential<'e, E: PgExecutor<'e>>(
    exec: E,
    token_hash: &[u8],
) -> Result<Option<RefusedCredentialRow>, DbError> {
    Ok(sqlx::query_as::<_, RefusedCredentialRow>(
        "SELECT c.kind, (c.revoked_at IS NOT NULL) AS revoked, \
                (c.expires_at IS NOT NULL AND c.expires_at <= now()) AS expired, \
                EXISTS (SELECT 1 FROM agents a \
                        JOIN workspaces w ON w.id = a.workspace_id \
                        JOIN org_members m ON m.org_id = w.org_id \
                             AND m.role = 'owner' AND m.status = 'active' \
                        WHERE a.id = c.agent_id) AS has_owner \
         FROM gateway_credentials c WHERE c.token_hash = $1",
    )
    .bind(token_hash)
    .fetch_optional(exec)
    .await?)
}

/// The same live-credential lookup by credential id (re-evaluation of live BYO tunnels).
pub async fn find_credential_identity_by_id<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<CredentialIdentityRow>, DbError> {
    Ok(
        sqlx::query_as::<_, CredentialIdentityRow>(sqlx::AssertSqlSafe(format!(
            "{IDENTITY_SELECT}{NOT_EXPIRED} AND c.id = $1"
        )))
        .bind(id)
        .fetch_optional(exec)
        .await?,
    )
}

/// (RFE-013, C-316) Records the use of credentials: `last_used_at = now()` for each id, only
/// where it is null or older than a minute. A churn column: `updated_at` is untouched.
pub async fn touch_last_used<'e, E: PgExecutor<'e>>(exec: E, ids: &[Uuid]) -> Result<u64, DbError> {
    if ids.is_empty() {
        return Ok(0);
    }
    Ok(sqlx::query(
        "UPDATE gateway_credentials SET last_used_at = now() \
         WHERE id = ANY($1) AND revoked_at IS NULL \
           AND (last_used_at IS NULL OR last_used_at < now() - interval '1 minute')",
    )
    .bind(ids)
    .execute(exec)
    .await?
    .rows_affected())
}

/// Whether the user holds `can_use` on the workspace: an active org member who is owner/admin
/// or holds `workspace_access`. Suspended members do not. The speaker of a turn is bound to
/// policy `user` identities only while this holds (RFE-005, 06 §Credential authentication).
pub async fn can_use<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM workspaces w \
         JOIN org_members m ON m.org_id = w.org_id AND m.status = 'active' \
         WHERE w.id = $1 AND m.user_id = $2 \
           AND (m.role IN ('owner', 'admin') \
                OR EXISTS (SELECT 1 FROM workspace_access x \
                           WHERE x.workspace_id = w.id AND x.user_id = m.user_id) \
                OR EXISTS (SELECT 1 FROM group_members gm \
                           JOIN groups g ON g.id = gm.group_id \
                           WHERE gm.org_id = w.org_id AND gm.user_id = m.user_id \
                             AND (g.org_role = 'admin' \
                                  OR EXISTS (SELECT 1 FROM group_workspace_access b \
                                             WHERE b.group_id = g.id AND b.workspace_id = w.id)))))",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_one(exec)
    .await?)
}

/// (P4-GRP-1, D-131, SEC-71) The ids of the groups **of `org`** that contain `user` (an active
/// member only): the speaker's principals beyond the agent id (`06 §Credential
/// authentication`). One indexed query per credential resolution. Callers pass it only for a
/// speaker that holds `can_use`; a request without a speaker has none.
///
/// Never truncates: a truncated set would make a **block** rule on a group past the cut not
/// match (fail open). More than `GROUPS_PER_USER` (100) rows (only possible if the cap was
/// bypassed) is an `Err`, and the caller must refuse the request.
pub async fn speaker_group_ids<'e, E: PgExecutor<'e>>(
    exec: E,
    org: Uuid,
    user: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT gm.group_id FROM group_members gm \
         JOIN org_members m ON m.org_id = gm.org_id AND m.user_id = gm.user_id \
              AND m.status = 'active' \
         WHERE gm.org_id = $1 AND gm.user_id = $2 \
         ORDER BY gm.group_id LIMIT 101",
    )
    .bind(org)
    .bind(user)
    .fetch_all(exec)
    .await?;
    if ids.len() > super::groups::GROUPS_PER_USER as usize {
        // The ids and names stay out of the log; the caller refuses the request.
        tracing::error!(
            user_id = %user, org_id = %org,
            "speaker is in more groups than the per-user limit: refusing to resolve a truncated principal set"
        );
        return Err(DbError::Other(sqlx::Error::Protocol(
            "speaker is in more than the per-user group limit".into(),
        )));
    }
    Ok(ids)
}

pub async fn register_instance<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    hostname: &str,
    version: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO gateway_instances (id, hostname, version, started_at, last_seen_at) \
         VALUES ($1, $2, $3, now(), now())",
    )
    .bind(id)
    .bind(hostname)
    .bind(version)
    .execute(exec)
    .await?;
    Ok(())
}

/// Churn column: does not touch `updated_at`.
pub async fn heartbeat_instance<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<(), DbError> {
    sqlx::query("UPDATE gateway_instances SET last_seen_at = now() WHERE id = $1")
        .bind(id)
        .execute(exec)
        .await?;
    Ok(())
}

pub async fn stop_instance<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE gateway_instances SET stopped_at = now(), updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(exec)
    .await?;
    Ok(())
}

// ---- policy / secrets reads for the MITM lane (docs/06 §MITM lane) ----

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PolicyRuleRow {
    pub logical_id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub identities: serde_json::Value,
    pub targets: serde_json::Value,
    pub action: String,
    pub require_approval: bool,
    /// (RFE-011a) hold deadline preference in seconds; only with `require_approval`.
    pub approval_timeout_secs: Option<i32>,
    pub rate_limit: Option<serde_json::Value>,
    pub conditions: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ScopePolicyRows {
    pub default_action: String,
    pub uncredentialed_default: String,
    pub uncredentialed_allowlist: serde_json::Value,
    /// (RFE-003) `enabled` | `allowlist_only` | `disabled`; always `enabled` at org scope.
    pub open_lane_posture: String,
    /// Hand rules by ascending priority.
    pub rules: Vec<PolicyRuleRow>,
    /// Derived rules (JSON per `derived_rules.rule`, with `logical_id` column), in order.
    pub derived: Vec<(Uuid, serde_json::Value)>,
}

impl Default for ScopePolicyRows {
    fn default() -> Self {
        Self {
            default_action: "none".into(),
            uncredentialed_default: "none".into(),
            uncredentialed_allowlist: serde_json::json!([]),
            open_lane_posture: "enabled".into(),
            rules: Vec::new(),
            derived: Vec::new(),
        }
    }
}

/// Published generation + rules + derived rules of one scope
/// (`workspace_id = None` = organization scope). No published generation = defaults.
pub async fn load_scope_policy(
    pool: &sqlx::PgPool,
    org_id: Uuid,
    workspace_id: Option<Uuid>,
) -> Result<ScopePolicyRows, DbError> {
    let gen_row: Option<(Uuid, String, String, serde_json::Value, String)> = sqlx::query_as(
        "SELECT id, default_action, uncredentialed_default, uncredentialed_allowlist, \
                open_lane_posture \
         FROM policy_generations \
         WHERE org_id = $1 AND workspace_id IS NOT DISTINCT FROM $2 AND status = 'published'",
    )
    .bind(org_id)
    .bind(workspace_id)
    .fetch_optional(pool)
    .await?;
    let mut out = ScopePolicyRows::default();
    if let Some((id, da, ud, al, olp)) = gen_row {
        out.default_action = da;
        out.uncredentialed_default = ud;
        out.uncredentialed_allowlist = al;
        out.open_lane_posture = olp;
        out.rules = sqlx::query_as::<_, PolicyRuleRow>(
            "SELECT logical_id, name, enabled, identities, targets, action, require_approval, \
                    approval_timeout_secs, rate_limit, conditions \
             FROM policy_rules WHERE generation_id = $1 ORDER BY priority",
        )
        .bind(id)
        .fetch_all(pool)
        .await?;
    }
    out.derived = sqlx::query_as::<_, (Uuid, serde_json::Value)>(
        "SELECT logical_id, rule FROM derived_rules \
         WHERE org_id = $1 AND workspace_id IS NOT DISTINCT FROM $2 \
         ORDER BY source_kind, source_id, ordinal",
    )
    .bind(org_id)
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;
    Ok(out)
}

/// Metadata of an active secret (never the plaintext).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SecretMetaRow {
    pub id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub kind: String,
    pub llm_provider: Option<String>,
    pub llm_auth_mode: Option<String>,
    pub llm_endpoint_id: Option<String>,
    pub host_pattern: String,
    pub path_pattern: Option<String>,
    pub injection: serde_json::Value,
    pub value_ref: String,
    pub value_version: i32,
    pub origin: Option<serde_json::Value>,
}

/// Active secrets of the organization (workspace NULL) and of the workspace.
pub async fn active_secrets(
    pool: &sqlx::PgPool,
    org_id: Uuid,
    workspace_id: Uuid,
) -> Result<Vec<SecretMetaRow>, DbError> {
    Ok(sqlx::query_as::<_, SecretMetaRow>(
        "SELECT id, workspace_id, name, kind, llm_provider, llm_auth_mode, llm_endpoint_id, \
                host_pattern, path_pattern, injection, value_ref, value_version, origin \
         FROM secrets \
         WHERE org_id = $1 AND status = 'active' AND (workspace_id IS NULL OR workspace_id = $2) \
         ORDER BY created_at, id",
    )
    .bind(org_id)
    .bind(workspace_id)
    .fetch_all(pool)
    .await?)
}

/// `(org_slug, workspace_slug)` for dashboard URLs.
pub async fn scope_slugs<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace_id: Uuid,
) -> Result<Option<(String, String)>, DbError> {
    Ok(sqlx::query_as::<_, (String, String)>(
        "SELECT o.slug, w.slug FROM workspaces w JOIN organizations o ON o.id = w.org_id \
         WHERE w.id = $1",
    )
    .bind(workspace_id)
    .fetch_optional(exec)
    .await?)
}

/// Churn column; does not touch `updated_at`.
pub async fn touch_secret_last_used<'e, E: PgExecutor<'e>>(
    exec: E,
    secret_id: Uuid,
) -> Result<(), DbError> {
    sqlx::query("UPDATE secrets SET last_used_at = now() WHERE id = $1")
        .bind(secret_id)
        .execute(exec)
        .await?;
    Ok(())
}

/// (P5-UX-14) Churn column of an app connection; does not touch `updated_at`.
pub async fn touch_connection_last_used<'e, E: PgExecutor<'e>>(
    exec: E,
    connection_id: Uuid,
) -> Result<(), DbError> {
    sqlx::query("UPDATE app_connections SET last_used_at = now() WHERE id = $1")
        .bind(connection_id)
        .execute(exec)
        .await?;
    Ok(())
}

/// (P5-UX-14, D-350, C-985) Records what real use showed about an account: `rejected` (an upstream
/// 401 on a request with its credential injected) or `ok` (a later 2xx or 3xx). Only on a change of
/// state: a row already holding `result` is not written. Informational — never touches `status`,
/// `updated_at` or anything an agent's access reads. Returns whether a row changed.
pub async fn record_account_check<'e, E: PgExecutor<'e>>(
    exec: E,
    connection: bool,
    id: Uuid,
    result: &str,
) -> Result<bool, DbError> {
    let sql = if connection {
        "UPDATE app_connections SET last_check_at = now(), last_check_result = $2 \
         WHERE id = $1 AND last_check_result IS DISTINCT FROM $2"
    } else {
        "UPDATE secrets SET last_check_at = now(), last_check_result = $2 \
         WHERE id = $1 AND last_check_result IS DISTINCT FROM $2"
    };
    Ok(sqlx::query(sql)
        .bind(id)
        .bind(result)
        .execute(exec)
        .await?
        .rows_affected()
        == 1)
}

#[derive(Debug, Clone)]
pub struct RequestLogRow {
    pub id: Uuid,
    pub ts: chrono::DateTime<chrono::Utc>,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub credential_kind: &'static str,
    pub lane: &'static str,
    pub method: String,
    pub host: String,
    pub path: String,
    pub app_id: Option<String>,
    pub tool_id: Option<String>,
    pub connection_id: Option<Uuid>,
    pub secret_id: Option<Uuid>,
    pub decision: &'static str,
    pub rule_logical_id: Option<Uuid>,
    pub rule_name: Option<String>,
    pub upstream_status: Option<i32>,
    pub status: i32,
    pub error_code: Option<String>,
    pub injected: i16,
    pub latency_ms: i32,
    pub bytes_up: i64,
    pub bytes_down: i64,
    /// (RFE-025) label of the BYO context credential that made the request.
    pub context_label: Option<String>,
    /// (RFE-005) the person whose turn made the request, when attributed.
    pub speaker_user_id: Option<Uuid>,
    /// (P4-011B, C-827) the approval the request was held, parked or resumed under.
    pub approval_id: Option<Uuid>,
    /// (P4-011B, C-827) a parked approval's grant was consumed by this request.
    pub resumed: bool,
    /// (P5-MODELS-2, C-880) the model an LLM request with an injected key asked for; audit only.
    pub llm_model: Option<String>,
}

/// Multi-row insert of a batch (callers keep batches <= 1000 rows).
pub async fn insert_request_log(
    pool: &sqlx::PgPool,
    rows: &[RequestLogRow],
) -> Result<(), DbError> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut qb = sqlx::QueryBuilder::<sqlx::Postgres>::new(
        "INSERT INTO request_log (id, ts, org_id, workspace_id, agent_id, conversation_id, turn_id, \
         credential_kind, lane, method, host, path, app_id, tool_id, connection_id, secret_id, \
         decision, rule_logical_id, rule_name, upstream_status, status, error_code, injected, \
         latency_ms, bytes_up, bytes_down, context_label, speaker_user_id, approval_id, resumed, \
         llm_model) ",
    );
    qb.push_values(rows, |mut b, r| {
        b.push_bind(r.id)
            .push_bind(r.ts)
            .push_bind(r.org_id)
            .push_bind(r.workspace_id)
            .push_bind(r.agent_id)
            .push_bind(r.conversation_id)
            .push_bind(r.turn_id)
            .push_bind(r.credential_kind)
            .push_bind(r.lane)
            .push_bind(&r.method)
            .push_bind(&r.host)
            .push_bind(&r.path)
            .push_bind(&r.app_id)
            .push_bind(&r.tool_id)
            .push_bind(r.connection_id)
            .push_bind(r.secret_id)
            .push_bind(r.decision)
            .push_bind(r.rule_logical_id)
            .push_bind(&r.rule_name)
            .push_bind(r.upstream_status)
            .push_bind(r.status)
            .push_bind(&r.error_code)
            .push_bind(r.injected)
            .push_bind(r.latency_ms)
            .push_bind(r.bytes_up)
            .push_bind(r.bytes_down)
            .push_bind(&r.context_label)
            .push_bind(r.speaker_user_id)
            .push_bind(r.approval_id)
            .push_bind(r.resumed)
            .push_bind(&r.llm_model);
    });
    qb.build().execute(pool).await?;
    Ok(())
}

/// A new `pending` approval hold (docs/03 §approvals). `deadline` is
/// `now() + timeout_secs` on the database clock.
#[derive(Debug, Clone)]
pub struct NewApproval {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub gateway_instance_id: Uuid,
    pub method: String,
    pub host: String,
    pub path: String,
    pub connection_id: Option<Uuid>,
    pub rule_logical_id: Option<Uuid>,
    pub rule_name: Option<String>,
    pub summary: serde_json::Value,
    pub batch_id: Option<String>,
    pub batch_label: Option<String>,
    pub batch_total: Option<i32>,
    /// (RFE-025) label of the BYO context credential that made the request.
    pub context_label: Option<String>,
    pub timeout_secs: f64,
}

pub async fn insert_approval<'e, E: PgExecutor<'e>>(
    exec: E,
    a: &NewApproval,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO approvals (id, org_id, workspace_id, agent_id, conversation_id, turn_id, \
         gateway_instance_id, method, host, path, connection_id, rule_logical_id, rule_name, \
         summary, batch_id, batch_label, batch_total, context_label, status, deadline) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$19,'pending', \
                 now() + make_interval(secs => $18))",
    )
    .bind(a.id)
    .bind(a.org_id)
    .bind(a.workspace_id)
    .bind(a.agent_id)
    .bind(a.conversation_id)
    .bind(a.turn_id)
    .bind(a.gateway_instance_id)
    .bind(&a.method)
    .bind(&a.host)
    .bind(&a.path)
    .bind(a.connection_id)
    .bind(a.rule_logical_id)
    .bind(&a.rule_name)
    .bind(&a.summary)
    .bind(&a.batch_id)
    .bind(&a.batch_label)
    .bind(a.batch_total)
    .bind(a.timeout_secs)
    .bind(&a.context_label)
    .execute(exec)
    .await?;
    Ok(())
}

/// Current status of the hold (`None` if the row is gone).
pub async fn get_approval_status<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<String>, DbError> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT status FROM approvals WHERE id = $1")
            .bind(id)
            .fetch_optional(exec)
            .await?,
    )
}

/// `pending` -> `expired`; true when this call made the transition.
pub async fn expire_approval<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE approvals SET status = 'expired', decided_at = now(), resolved_at = now(), \
         updated_at = now() \
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}

/// `pending` -> `abandoned` (client gone / gateway stopping); true on transition.
pub async fn abandon_approval<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE approvals SET status = 'abandoned', resolved_at = now(), updated_at = now() \
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}

/// Abandons every pending hold of this gateway instance; returns the count. Parked approvals
/// have no holder: SIGTERM never touches them (P4-011B, D-264).
pub async fn abandon_approvals<'e, E: PgExecutor<'e>>(
    exec: E,
    instance_id: Uuid,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "UPDATE approvals SET status = 'abandoned', resolved_at = now(), updated_at = now() \
         WHERE gateway_instance_id = $1 AND status = 'pending' AND NOT parked",
    )
    .bind(instance_id)
    .execute(exec)
    .await?
    .rows_affected())
}

// ---- parked approvals and grants (P4-011B-1, docs/07 §Park and resume, C-820, C-821) ----

/// A `ready` single-use grant of an agent's conversation: the row's salt and stored fingerprint,
/// for the gateway to recompute against the retried request.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GrantRow {
    pub id: Uuid,
    pub fp_salt: Vec<u8>,
    pub fingerprint: Vec<u8>,
}

/// The open grants (`resume_state = 'ready'`, window not over) of one agent and conversation.
pub async fn ready_grants<'e, E: PgExecutor<'e>>(
    exec: E,
    agent_id: Uuid,
    conversation_id: Uuid,
) -> Result<Vec<GrantRow>, DbError> {
    Ok(sqlx::query_as::<_, GrantRow>(
        "SELECT id, fp_salt, fingerprint FROM approvals \
         WHERE agent_id = $1 AND conversation_id = $2 AND resume_state = 'ready' \
           AND resume_expires_at > now() AND fp_salt IS NOT NULL AND fingerprint IS NOT NULL \
         ORDER BY resume_expires_at",
    )
    .bind(agent_id)
    .bind(conversation_id)
    .fetch_all(exec)
    .await?)
}

/// Consumes a grant: one compare-and-set, so concurrent retries of the same request cannot both
/// pass. True for exactly one caller.
pub async fn consume_grant<'e, E: PgExecutor<'e>>(exec: E, id: Uuid) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE approvals SET resume_state = 'consumed', consumed_at = now(), updated_at = now() \
         WHERE id = $1 AND resume_state = 'ready' AND resume_expires_at > now()",
    )
    .bind(id)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}

/// The limits a park is checked against (`approvals.park.*`, `PARK_PER_AGENT_HOUR`).
#[derive(Debug, Clone, Copy)]
pub struct ParkLimits {
    pub max_pending_per_agent: u32,
    pub per_agent_hour: u32,
    /// Seconds a denied fingerprint is answered without a new approval.
    pub deny_cooldown_secs: f64,
}

/// What a park attempt did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParkOutcome {
    /// A new `pending` parked approval was inserted; `expires_at` is its deadline.
    Parked {
        id: Uuid,
        expires_at: chrono::DateTime<chrono::Utc>,
    },
    /// An identical request is already pending: the same answer, nothing inserted.
    Existing {
        id: Uuid,
        expires_at: chrono::DateTime<chrono::Utc>,
    },
    /// The identical request was denied within the cooldown.
    Cooldown {
        id: Uuid,
        until: chrono::DateTime<chrono::Utc>,
    },
    /// A per-agent cap: `per` is `pending` or `hour`.
    Limit { limit: u32, per: &'static str },
}

/// Parks an approval in one transaction under a per-conversation advisory lock: an identical
/// pending request answers with its row, an identical denied one inside the cooldown with the
/// denial, the caps are checked, then the row is inserted with `parked = true` and no holder.
/// `matches(salt, fingerprint)` is the caller's constant-time comparison of a stored row with the
/// request being parked (the fingerprint is never computed in SQL).
pub async fn insert_parked(
    pool: &sqlx::PgPool,
    a: &NewApproval,
    fp_salt: &[u8],
    fingerprint: &[u8],
    limits: ParkLimits,
    matches: &(dyn Fn(&[u8], &[u8]) -> bool + Sync),
) -> Result<ParkOutcome, DbError> {
    let conversation = a.conversation_id;
    let mut tx = pool.begin().await?;
    // Serialises parks of one conversation so two identical requests cannot both insert.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
        .bind(format!("park:{}:{:?}", a.agent_id, conversation))
        .execute(&mut *tx)
        .await?;
    type Open = (Uuid, Vec<u8>, Vec<u8>, chrono::DateTime<chrono::Utc>);
    let pending: Vec<Open> = sqlx::query_as(
        "SELECT id, fp_salt, fingerprint, deadline FROM approvals \
         WHERE agent_id = $1 AND conversation_id IS NOT DISTINCT FROM $2 \
           AND parked AND status = 'pending'",
    )
    .bind(a.agent_id)
    .bind(conversation)
    .fetch_all(&mut *tx)
    .await?;
    for (id, salt, fp, deadline) in &pending {
        if matches(salt, fp) {
            tx.rollback().await?;
            return Ok(ParkOutcome::Existing {
                id: *id,
                expires_at: *deadline,
            });
        }
    }
    if limits.deny_cooldown_secs > 0.0 {
        let denied: Vec<Open> = sqlx::query_as(
            "SELECT id, fp_salt, fingerprint, \
                    decided_at + make_interval(secs => $4) \
             FROM approvals \
             WHERE workspace_id = $1 AND status = 'denied' AND parked AND agent_id = $2 \
               AND conversation_id IS NOT DISTINCT FROM $3 \
               AND decided_at + make_interval(secs => $4) > now() \
             ORDER BY decided_at DESC",
        )
        .bind(a.workspace_id)
        .bind(a.agent_id)
        .bind(conversation)
        .bind(limits.deny_cooldown_secs)
        .fetch_all(&mut *tx)
        .await?;
        for (id, salt, fp, until) in &denied {
            if matches(salt, fp) {
                tx.rollback().await?;
                return Ok(ParkOutcome::Cooldown {
                    id: *id,
                    until: *until,
                });
            }
        }
    }
    let pending_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM approvals WHERE agent_id = $1 AND parked AND status = 'pending'",
    )
    .bind(a.agent_id)
    .fetch_one(&mut *tx)
    .await?;
    if pending_count >= i64::from(limits.max_pending_per_agent) {
        tx.rollback().await?;
        return Ok(ParkOutcome::Limit {
            limit: limits.max_pending_per_agent,
            per: "pending",
        });
    }
    let hour_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM approvals \
         WHERE workspace_id = $1 AND agent_id = $2 AND parked \
           AND created_at > now() - interval '1 hour'",
    )
    .bind(a.workspace_id)
    .bind(a.agent_id)
    .fetch_one(&mut *tx)
    .await?;
    if hour_count >= i64::from(limits.per_agent_hour) {
        tx.rollback().await?;
        return Ok(ParkOutcome::Limit {
            limit: limits.per_agent_hour,
            per: "hour",
        });
    }
    let expires_at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "INSERT INTO approvals (id, org_id, workspace_id, agent_id, conversation_id, turn_id, \
         gateway_instance_id, method, host, path, connection_id, rule_logical_id, rule_name, \
         summary, batch_id, batch_label, batch_total, context_label, status, deadline, \
         parked, fp_salt, fingerprint) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$19,'pending', \
                 now() + make_interval(secs => $18), true, $20, $21) \
         RETURNING deadline",
    )
    .bind(a.id)
    .bind(a.org_id)
    .bind(a.workspace_id)
    .bind(a.agent_id)
    .bind(a.conversation_id)
    .bind(a.turn_id)
    .bind(a.gateway_instance_id)
    .bind(&a.method)
    .bind(&a.host)
    .bind(&a.path)
    .bind(a.connection_id)
    .bind(a.rule_logical_id)
    .bind(&a.rule_name)
    .bind(&a.summary)
    .bind(&a.batch_id)
    .bind(&a.batch_label)
    .bind(a.batch_total)
    .bind(a.timeout_secs)
    .bind(&a.context_label)
    .bind(fp_salt)
    .bind(fingerprint)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(ParkOutcome::Parked {
        id: a.id,
        expires_at,
    })
}

// ---- LLM governance (docs/08 §11) ----

/// `organizations.governance` (raw document; normalized by the caller). `None` = no such org.
pub async fn org_governance<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
) -> Result<Option<serde_json::Value>, DbError> {
    Ok(sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT governance FROM organizations WHERE id = $1",
    )
    .bind(org_id)
    .fetch_optional(exec)
    .await?)
}

/// Write back a verified/mismatched key origin (LLM secrets only).
pub async fn update_secret_origin<'e, E: PgExecutor<'e>>(
    exec: E,
    secret_id: Uuid,
    origin: &serde_json::Value,
) -> Result<(), DbError> {
    sqlx::query("UPDATE secrets SET origin = $2 WHERE id = $1 AND kind = 'llm'")
        .bind(secret_id)
        .bind(origin)
        .execute(exec)
        .await?;
    Ok(())
}

// ---- registry snapshot and app connections (docs/06 §Registry snapshot loading) ----

/// Header of a stored registry snapshot (activation rule inputs).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SnapshotHeader {
    pub contract: i32,
    pub activated_at: chrono::DateTime<chrono::Utc>,
    pub content_hash: String,
}

/// Snapshots with a contract in `min..=max` (the activation rule picks among them).
pub async fn snapshot_headers<'e, E: PgExecutor<'e>>(
    exec: E,
    min: i32,
    max: i32,
) -> Result<Vec<SnapshotHeader>, DbError> {
    Ok(sqlx::query_as::<_, SnapshotHeader>(
        "SELECT contract, activated_at, content_hash FROM registry_snapshots \
         WHERE contract BETWEEN $1 AND $2",
    )
    .bind(min)
    .bind(max)
    .fetch_all(exec)
    .await?)
}

pub async fn snapshot_body<'e, E: PgExecutor<'e>>(
    exec: E,
    content_hash: &str,
) -> Result<Option<serde_json::Value>, DbError> {
    Ok(sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT snapshot FROM registry_snapshots WHERE content_hash = $1",
    )
    .bind(content_hash)
    .fetch_optional(exec)
    .await?)
}

/// An app connection as the gateway needs it (never the credentials).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AppConnectionRow {
    pub id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub app_id: String,
    pub label: String,
    pub status: String,
    pub credentials_ref: String,
    pub credentials_version: i64,
    pub credentials_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub metadata: serde_json::Value,
    pub tenant_host: Option<String>,
    /// The registered app instance (D-74): the connection is injected only on its host.
    pub instance_id: Option<Uuid>,
    /// (FIX-123, D-214) The `connect[]` method the connection was made with; null = unknown.
    pub connect_method: Option<String>,
    /// (FIX-133, D-240) `read` | `write`: a `read` connection allows only the app's read tools.
    pub access: String,
    /// (FIX-136, D-241) The scopes the connection has (granted by OAuth, or declared for a pasted
    /// token); empty = unknown.
    pub scopes: Vec<String>,
}

/// An `active` app instance visible to an org, as the gateway needs it (D-74).
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct InstanceHostRow {
    pub id: Uuid,
    pub app_id: String,
    pub host: String,
    /// (FIX-145, D-231) The path the app is served under on `host`; null = the host root.
    pub base_path: Option<String>,
}

/// `active` instances visible to the organization (instance-wide and its own).
pub async fn visible_instances(
    pool: &sqlx::PgPool,
    org_id: Uuid,
) -> Result<Vec<InstanceHostRow>, DbError> {
    Ok(sqlx::query_as::<_, InstanceHostRow>(
        "SELECT id, app_id, host, base_path FROM app_instances \
         WHERE status = 'active' AND (org_id IS NULL OR org_id = $1) ORDER BY host, id",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?)
}

/// `active`, `disabled` and `reauth_required` connections of the organization (workspace
/// NULL) and of the workspace for the given apps (`disabled` ones only feed guidance;
/// `reauth_required` ones are refused with reconnect guidance).
pub async fn app_connections(
    pool: &sqlx::PgPool,
    org_id: Uuid,
    workspace_id: Uuid,
    app_ids: &[String],
) -> Result<Vec<AppConnectionRow>, DbError> {
    Ok(sqlx::query_as::<_, AppConnectionRow>(
        "SELECT id, workspace_id, app_id, label, status, credentials_ref, credentials_version, \
                credentials_expires_at, metadata, tenant_host, instance_id, connect_method, access, scopes \
         FROM app_connections \
         WHERE org_id = $1 AND (workspace_id IS NULL OR workspace_id = $2) \
           AND app_id = ANY($3) AND status IN ('active', 'disabled', 'reauth_required') \
         ORDER BY created_at, id",
    )
    .bind(org_id)
    .bind(workspace_id)
    .bind(app_ids)
    .fetch_all(pool)
    .await?)
}

/// One connection by id, if it belongs to the organization and is the workspace's or an
/// organization-level one (database lane login, P2-DBL-1). Any status is returned; the caller
/// refuses `disabled` and `reauth_required`.
pub async fn app_connection_by_id(
    pool: &sqlx::PgPool,
    org_id: Uuid,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<Option<AppConnectionRow>, DbError> {
    Ok(sqlx::query_as::<_, AppConnectionRow>(
        "SELECT id, workspace_id, app_id, label, status, credentials_ref, credentials_version, \
                credentials_expires_at, metadata, tenant_host, instance_id, connect_method, access, scopes \
         FROM app_connections \
         WHERE id = $1 AND org_id = $2 AND (workspace_id IS NULL OR workspace_id = $3)",
    )
    .bind(id)
    .bind(org_id)
    .bind(workspace_id)
    .fetch_optional(pool)
    .await?)
}

/// One connection by id **within the named organization**, whatever its scope or status (the
/// skill-library fetch route, P4-GSL, D-149): the caller refuses workspace-scoped and non-active
/// rows. A connection of another organization is `None`.
pub async fn app_connection_in_org(
    pool: &sqlx::PgPool,
    org_id: Uuid,
    id: Uuid,
) -> Result<Option<AppConnectionRow>, DbError> {
    Ok(sqlx::query_as::<_, AppConnectionRow>(
        "SELECT id, workspace_id, app_id, label, status, credentials_ref, credentials_version, \
                credentials_expires_at, metadata, tenant_host, instance_id, connect_method, access, scopes \
         FROM app_connections WHERE id = $1 AND org_id = $2",
    )
    .bind(id)
    .bind(org_id)
    .fetch_optional(pool)
    .await?)
}

/// `(restricted, available app ids)` for a workspace: `org_app_availability` rows with a
/// NULL workspace apply to the whole organization.
pub async fn app_availability(
    pool: &sqlx::PgPool,
    org_id: Uuid,
    workspace_id: Uuid,
) -> Result<(bool, Vec<String>), DbError> {
    let posture: Option<String> =
        sqlx::query_scalar("SELECT app_posture FROM organizations WHERE id = $1")
            .bind(org_id)
            .fetch_optional(pool)
            .await?;
    let restricted = posture.as_deref() == Some("restricted");
    if !restricted {
        return Ok((false, Vec::new()));
    }
    let ids = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT app_id FROM org_app_availability \
         WHERE org_id = $1 AND (workspace_id IS NULL OR workspace_id = $2)",
    )
    .bind(org_id)
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;
    Ok((true, ids))
}

// ---- token freshness (docs/08 §8) ----

/// Mutable state of a connection, re-read under the refresh lock.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConnectionState {
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub status: String,
    pub credentials_ref: String,
    pub credentials_version: i64,
    pub credentials_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub oauth_client_id: Option<Uuid>,
}

pub async fn connection_state<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<ConnectionState>, DbError> {
    Ok(sqlx::query_as::<_, ConnectionState>(
        "SELECT org_id, workspace_id, status, credentials_ref, credentials_version, \
                credentials_expires_at, oauth_client_id \
         FROM app_connections WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

/// Session-level advisory lock for one connection's refresh/mint; true when acquired.
/// Must be released on the same connection with [`refresh_unlock`].
pub async fn try_refresh_lock(
    conn: &mut sqlx::PgConnection,
    connection_id: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT pg_try_advisory_lock(hashtextextended('aurora:refresh:' || $1::text, 0))",
    )
    .bind(connection_id)
    .fetch_one(conn)
    .await?)
}

pub async fn refresh_unlock(
    conn: &mut sqlx::PgConnection,
    connection_id: Uuid,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT pg_advisory_unlock(hashtextextended('aurora:refresh:' || $1::text, 0))",
    )
    .bind(connection_id)
    .fetch_one(conn)
    .await?)
}

/// Compare-and-set persistence of refreshed credentials: succeeds only while the stored
/// `credentials_version` is still the one the refresh was based on.
pub async fn cas_connection_credentials<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    new_ref: &str,
    read_version: i64,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    credential_keys: &[String],
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE app_connections SET credentials_ref = $2, \
                credentials_version = credentials_version + 1, \
                credentials_expires_at = $4, credential_keys = $5, updated_at = now() \
         WHERE id = $1 AND credentials_version = $3 AND status = 'active'",
    )
    .bind(id)
    .bind(new_ref)
    .bind(read_version)
    .bind(expires_at)
    .bind(credential_keys)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}

/// `active` -> `reauth_required`; only a user reconnect clears it.
pub async fn mark_reauth_required<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
    reason: &str,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE app_connections SET status = 'reauth_required', status_reason = $2, \
                reauth_required_at = now(), updated_at = now() \
         WHERE id = $1 AND status = 'active'",
    )
    .bind(id)
    .bind(reason)
    .execute(exec)
    .await?
    .rows_affected()
        == 1)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OauthClientRow {
    pub org_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub fields: serde_json::Value,
    pub secret_ref: String,
    pub enabled: bool,
}

pub async fn oauth_client<'e, E: PgExecutor<'e>>(
    exec: E,
    id: Uuid,
) -> Result<Option<OauthClientRow>, DbError> {
    Ok(sqlx::query_as::<_, OauthClientRow>(
        "SELECT org_id, workspace_id, fields, secret_ref, enabled \
         FROM app_oauth_clients WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(exec)
    .await?)
}

/// `(org external_id, agent name, agent handle)` for template context variables.
pub async fn context_names<'e, E: PgExecutor<'e>>(
    exec: E,
    org_id: Uuid,
    agent_id: Uuid,
) -> Result<Option<(String, String, String)>, DbError> {
    Ok(sqlx::query_as::<_, (String, String, String)>(
        "SELECT o.external_id, a.name, a.handle FROM organizations o, agents a \
         WHERE o.id = $1 AND a.id = $2",
    )
    .bind(org_id)
    .bind(agent_id)
    .fetch_optional(exec)
    .await?)
}
