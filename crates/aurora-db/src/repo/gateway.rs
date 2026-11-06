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
    pub context_label: Option<String>,
    /// Seconds until `expires_at` (None = no expiry).
    pub expires_in_secs: Option<f64>,
}

/// Live credential by SHA-256 hash: not revoked, not expired, agent/workspace/org
/// present, and the org has at least one active owner.
pub async fn find_credential_identity<'e, E: PgExecutor<'e>>(
    exec: E,
    token_hash: &[u8],
) -> Result<Option<CredentialIdentityRow>, DbError> {
    Ok(sqlx::query_as::<_, CredentialIdentityRow>(
        "SELECT c.id AS credential_id, c.kind, c.agent_id, a.workspace_id, w.org_id, \
                c.conversation_id, c.turn_id, c.context_label, \
                EXTRACT(EPOCH FROM (c.expires_at - now()))::float8 AS expires_in_secs \
         FROM gateway_credentials c \
         JOIN agents a ON a.id = c.agent_id \
         JOIN workspaces w ON w.id = a.workspace_id \
         JOIN organizations o ON o.id = w.org_id \
         WHERE c.token_hash = $1 \
           AND c.revoked_at IS NULL \
           AND (c.expires_at IS NULL OR c.expires_at > now()) \
           AND EXISTS (SELECT 1 FROM org_members m \
                       WHERE m.org_id = o.id AND m.role = 'owner' AND m.status = 'active')",
    )
    .bind(token_hash)
    .fetch_optional(exec)
    .await?)
}

/// Users with `can_use` on the workspace: active org members who are
/// owner/admin or hold `workspace_access`. Suspended members are excluded.
pub async fn principals<'e, E: PgExecutor<'e>>(
    exec: E,
    workspace_id: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT m.user_id FROM workspaces w \
         JOIN org_members m ON m.org_id = w.org_id AND m.status = 'active' \
         WHERE w.id = $1 \
           AND (m.role IN ('owner', 'admin') \
                OR EXISTS (SELECT 1 FROM workspace_access x \
                           WHERE x.workspace_id = w.id AND x.user_id = m.user_id)) \
         ORDER BY m.user_id",
    )
    .bind(workspace_id)
    .fetch_all(exec)
    .await?)
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
    pub rate_limit: Option<serde_json::Value>,
    pub conditions: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ScopePolicyRows {
    pub default_action: String,
    pub uncredentialed_default: String,
    pub uncredentialed_allowlist: serde_json::Value,
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
    let gen_row: Option<(Uuid, String, String, serde_json::Value)> = sqlx::query_as(
        "SELECT id, default_action, uncredentialed_default, uncredentialed_allowlist \
         FROM policy_generations \
         WHERE org_id = $1 AND workspace_id IS NOT DISTINCT FROM $2 AND status = 'published'",
    )
    .bind(org_id)
    .bind(workspace_id)
    .fetch_optional(pool)
    .await?;
    let mut out = ScopePolicyRows::default();
    if let Some((id, da, ud, al)) = gen_row {
        out.default_action = da;
        out.uncredentialed_default = ud;
        out.uncredentialed_allowlist = al;
        out.rules = sqlx::query_as::<_, PolicyRuleRow>(
            "SELECT logical_id, name, enabled, identities, targets, action, require_approval, \
                    rate_limit, conditions \
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
         latency_ms, bytes_up, bytes_down) ",
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
            .push_bind(r.bytes_down);
    });
    qb.build().execute(pool).await?;
    Ok(())
}
