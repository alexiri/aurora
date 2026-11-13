use super::compile::{compile_connection_grant, compile_secret_grant};
use super::dto::{
    AppRestrictions, BulkConnectionGrantItem, BulkConnectionGrantRequest, BulkSecretGrantItem,
    BulkStatus, ConnectionGrant, ConnectionGrantView, GRANT_BULK_MAX, GrantMode,
    PutConnectionGrant, SecretGrant, SecretGrantView,
};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::apps::service as apps;
use crate::http::auth::Caller;
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::agents::{self, AgentRow};
use aurora_db::repo::{connections, credentials, derived_rules, grants, secrets};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Agent visible to the caller (`can_use` on its workspace), else 404.
pub async fn visible_agent(
    caller: &Caller,
    state: &AppState,
    id: Uuid,
) -> Result<(AgentRow, aurora_db::authz::WorkspaceAuth), ApiError> {
    let row = agents::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Agent not found."))?;
    let auth = caller.workspace(state, row.workspace_id).await?;
    Ok((row, auth))
}

/// Where a grant write comes from (FIX-020, D-137, SEC-77). Both go through the same function
/// and the same checks; they differ only in what happens to a grant that already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// `PUT /agents/{agent}/grants/…`: idempotent / replacing, audited every time.
    Agent,
    /// Credential-side bulk routes: an existing grant is left unmodified and not audited;
    /// created grants are audited with `details.via = "credential"`.
    Credential,
}

impl Origin {
    fn replaces(self) -> bool {
        self == Self::Agent
    }
}

/// Result of one grant write inside a caller-owned transaction.
pub struct Written<T> {
    pub grant: T,
    /// `false` = the grant already existed and was left as it was (credential origin only).
    pub created: bool,
    /// The critical audit row to commit with the transaction (none for an untouched grant).
    pub event: Option<AuditEvent>,
}

fn with_via(origin: Origin, mut meta: serde_json::Value) -> serde_json::Value {
    if origin == Origin::Credential {
        meta["via"] = json!("credential");
    }
    meta
}

/// THE authorization path of a secret grant (SEC-77): the agent must be visible and
/// `can_own` (404 / 403), the secret active and in the agent's workspace or org (404), then
/// the grant, its derived rule and the gateway invalidation are written in `tx`. The caller
/// commits `tx` together with the returned audit event.
pub async fn grant_secret_in_tx(
    tx: &mut audit::Tx,
    state: &AppState,
    caller: &Caller,
    agent_id: Uuid,
    secret_id: Uuid,
    origin: Origin,
) -> Result<Written<SecretGrant>, ApiError> {
    let (agent, auth) = visible_agent(caller, state, agent_id).await?;
    crate::http::auth::require_own(&auth)?;
    write_secret_grant(
        tx,
        state,
        &agent,
        secret_id,
        origin,
        AuditActor::from_caller(caller),
        Some(caller.user_id),
    )
    .await
}

/// Starter grants (P4-ONB-1, D-140, SEC-79): the same write path as [`grant_secret_in_tx`]
/// with the system as actor, for the default agent of a personal workspace the sign-in
/// transaction has just created (no user holds `can_own` yet, and the rows are not committed,
/// so the caller's authorization check cannot apply; every other check does).
pub async fn grant_secret_as_system(
    tx: &mut audit::Tx,
    state: &AppState,
    agent: &AgentRow,
    secret_id: Uuid,
    actor: AuditActor,
) -> Result<Written<SecretGrant>, ApiError> {
    write_secret_grant(tx, state, agent, secret_id, Origin::Credential, actor, None).await
}

/// The secret must be active and in the agent's workspace or org (404), then the grant, its
/// derived rule and the gateway invalidation are written in `tx`.
async fn write_secret_grant(
    tx: &mut audit::Tx,
    state: &AppState,
    agent: &AgentRow,
    secret_id: Uuid,
    origin: Origin,
    actor: AuditActor,
    by: Option<Uuid>,
) -> Result<Written<SecretGrant>, ApiError> {
    let secret = secrets::get(&state.pool, secret_id)
        .await?
        .filter(|s| {
            s.org_id == agent.org_id
                && s.status == "active"
                && s.workspace_id.is_none_or(|w| w == agent.workspace_id)
        })
        .ok_or_else(|| ApiError::not_found("Secret not found."))?;
    let (g, created) = if origin.replaces() {
        let g = grants::upsert_secret_grant(&mut **tx, agent.workspace_id, agent.id, secret.id, by)
            .await?;
        (g, true)
    } else {
        match grants::insert_secret_grant_if_absent(
            &mut **tx,
            agent.workspace_id,
            agent.id,
            secret.id,
            by,
        )
        .await?
        {
            Some(g) => (g, true),
            None => {
                let g = grants::get_secret_grant(&mut **tx, agent.id, secret.id)
                    .await?
                    .ok_or_else(|| ApiError::not_found("Grant not found."))?;
                (g, false)
            }
        }
    };
    if !created {
        return Ok(Written {
            grant: g.into(),
            created,
            event: None,
        });
    }
    compile_secret_grant(
        tx,
        agent.org_id,
        agent.workspace_id,
        g.id,
        agent.id,
        secret.id,
        &secret.name,
    )
    .await?;
    credentials::notify_gateway_invalidate(&mut **tx, "agent", agent.id).await?;
    let ev = event_by(
        actor,
        "grant.create",
        agent,
        with_via(
            origin,
            json!({"secret_id": secret.id, "agent_handle": agent.handle}),
        ),
    );
    Ok(Written {
        grant: g.into(),
        created,
        event: Some(ev),
    })
}

/// Agent-side `PUT`: one grant in its own transaction, audited with it.
pub async fn grant_secret(
    state: &AppState,
    caller: &Caller,
    agent_id: Uuid,
    secret_id: Uuid,
) -> Result<SecretGrant, ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let w = grant_secret_in_tx(&mut tx, state, caller, agent_id, secret_id, Origin::Agent).await?;
    if let Some(ev) = &w.event {
        audit::commit_critical(tx, ev).await?;
    }
    Ok(w.grant)
}

/// Workspaces among `ids` the caller can own: credential-side lists never reveal agents of
/// any other workspace (SEC-77).
async fn owned_workspaces(
    state: &AppState,
    caller: &Caller,
    ids: Vec<Uuid>,
) -> Result<HashSet<Uuid>, ApiError> {
    let mut out = HashSet::new();
    let mut checked = HashSet::new();
    for ws in ids {
        if checked.insert(ws) && aurora_db::authz::can_own(&state.pool, caller.user_id, ws).await? {
            out.insert(ws);
        }
    }
    Ok(out)
}

/// Grants of a secret on agents of workspaces the caller can own (caller already authorized
/// to read the secret's metadata).
pub async fn list_secret_grants(
    state: &AppState,
    caller: &Caller,
    secret_id: Uuid,
) -> Result<Vec<SecretGrantView>, ApiError> {
    let rows = grants::list_for_secret(&state.pool, secret_id).await?;
    let ids = rows.iter().map(|r| r.workspace_id).collect();
    let own = owned_workspaces(state, caller, ids).await?;
    Ok(rows
        .into_iter()
        .filter(|r| own.contains(&r.workspace_id))
        .map(|r| SecretGrantView {
            grant: SecretGrant {
                id: r.id,
                agent_id: r.agent_id,
                secret_id: r.secret_id,
                created_at: r.created_at,
            },
            agent_name: r.agent_name,
            workspace_id: r.workspace_id,
            workspace_name: r.workspace_name,
        })
        .collect())
}

/// (P4-GRANTS, D-123) The manifest restrictions of an app in the active snapshot; both `false`
/// for an app without `grants` (or, never in practice, one the snapshot does not hold).
pub async fn app_restrictions(state: &AppState, app_id: &str) -> Result<AppRestrictions, ApiError> {
    Ok(apps::snapshot(state)
        .await?
        .and_then(|s| s.app(app_id).map(|a| a.grants.into()))
        .unwrap_or_default())
}

/// [`app_restrictions`] of each of the given connections.
pub async fn restrictions_by_connection(
    state: &AppState,
    connection_ids: &[Uuid],
) -> Result<HashMap<Uuid, AppRestrictions>, ApiError> {
    if connection_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let snap = apps::snapshot(state).await?;
    let rows = grants::connection_apps(&state.pool, connection_ids).await?;
    Ok(rows
        .into_iter()
        .map(|(id, app)| {
            let r = snap
                .as_ref()
                .and_then(|s| s.app(&app))
                .map(|a| AppRestrictions::from(a.grants))
                .unwrap_or_default();
            (id, r)
        })
        .collect())
}

/// Grants of a connection on agents of workspaces the caller can own.
pub async fn list_connection_grants(
    state: &AppState,
    caller: &Caller,
    connection_id: Uuid,
) -> Result<Vec<ConnectionGrantView>, ApiError> {
    let rows = grants::list_views_for_connection(&state.pool, connection_id).await?;
    let ids = rows.iter().map(|r| r.workspace_id).collect();
    let own = owned_workspaces(state, caller, ids).await?;
    let restr = restrictions_by_connection(state, &[connection_id])
        .await?
        .remove(&connection_id)
        .unwrap_or_default();
    Ok(rows
        .into_iter()
        .filter(|r| own.contains(&r.workspace_id))
        .map(|r| ConnectionGrantView {
            grant: ConnectionGrant::from(grants::ConnectionGrantRow {
                id: r.id,
                workspace_id: r.workspace_id,
                agent_id: r.agent_id,
                connection_id: r.connection_id,
                mode: r.mode,
                allow_tools: r.allow_tools,
                ask_tools: r.ask_tools,
                ask_timeout_secs: r.ask_timeout_secs,
                created_at: r.created_at,
            })
            .restricted(restr),
            agent_name: r.agent_name,
            workspace_id: r.workspace_id,
            workspace_name: r.workspace_name,
        })
        .collect())
}

fn bulk_check(agent_ids: &[Uuid]) -> Result<(), ApiError> {
    use aurora_core::FieldError;
    let bad = |m: &str| {
        ApiError::invalid_request("Invalid request.", vec![FieldError::new("/agent_ids", m)])
    };
    if agent_ids.is_empty() || agent_ids.len() > GRANT_BULK_MAX {
        return Err(bad(&format!("must hold 1 to {GRANT_BULK_MAX} agents")));
    }
    let mut seen = HashSet::new();
    if !agent_ids.iter().all(|a| seen.insert(*a)) {
        return Err(bad("must not contain duplicates"));
    }
    Ok(())
}

fn at_agent(e: ApiError, agent_id: Uuid) -> ApiError {
    let mut d = e.details().cloned().unwrap_or_else(|| json!({}));
    if let Some(o) = d.as_object_mut() {
        o.insert("agent_id".into(), json!(agent_id));
    }
    e.with_details(d)
}

/// Credential-side bulk secret grant: every agent runs [`grant_secret_in_tx`] in ONE
/// transaction; the first failure rolls everything back and is returned with `details.agent_id`.
pub async fn bulk_grant_secret(
    state: &AppState,
    caller: &Caller,
    secret_id: Uuid,
    agent_ids: &[Uuid],
) -> Result<Vec<BulkSecretGrantItem>, ApiError> {
    bulk_check(agent_ids)?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let mut items = Vec::new();
    let mut events = Vec::new();
    for &agent_id in agent_ids {
        let w = grant_secret_in_tx(
            &mut tx,
            state,
            caller,
            agent_id,
            secret_id,
            Origin::Credential,
        )
        .await
        .map_err(|e| at_agent(e, agent_id))?;
        events.extend(w.event);
        items.push(BulkSecretGrantItem {
            agent_id,
            grant: w.grant,
            status: status(w.created),
        });
    }
    audit::commit_critical_many(tx, &events).await?;
    Ok(items)
}

fn status(created: bool) -> BulkStatus {
    if created {
        BulkStatus::Created
    } else {
        BulkStatus::Existing
    }
}

/// Credential-side bulk connection grant (see [`bulk_grant_secret`]).
pub async fn bulk_grant_connection(
    state: &AppState,
    caller: &Caller,
    connection_id: Uuid,
    req: BulkConnectionGrantRequest,
) -> Result<Vec<BulkConnectionGrantItem>, ApiError> {
    bulk_check(&req.agent_ids)?;
    let one = PutConnectionGrant {
        mode: req.mode,
        allow_tools: req.allow_tools,
        ask_tools: req.ask_tools,
        ask_timeout_secs: req.ask_timeout_secs,
    };
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let mut items = Vec::new();
    let mut events = Vec::new();
    for &agent_id in &req.agent_ids {
        let w = grant_connection_in_tx(
            &mut tx,
            state,
            caller,
            agent_id,
            connection_id,
            one.clone(),
            Origin::Credential,
        )
        .await
        .map_err(|e| at_agent(e, agent_id))?;
        events.extend(w.event);
        items.push(BulkConnectionGrantItem {
            agent_id,
            grant: w.grant,
            status: status(w.created),
        });
    }
    audit::commit_critical_many(tx, &events).await?;
    Ok(items)
}

/// Security-critical audit event of a grant change (RFE-008): committed with the change.
fn event(caller: &Caller, action: &str, agent: &AgentRow, meta: serde_json::Value) -> AuditEvent {
    event_by(AuditActor::from_caller(caller), action, agent, meta)
}

fn event_by(
    actor: AuditActor,
    action: &str,
    agent: &AgentRow,
    meta: serde_json::Value,
) -> AuditEvent {
    AuditEvent::new(actor, action, Outcome::Success)
        .org(agent.org_id)
        .workspace(agent.workspace_id)
        .target("agent", agent.id)
        .metadata(meta)
}

/// Remove the grant and its derived rules; returns the grant id (404 when absent).
pub async fn revoke_secret(
    state: &AppState,
    caller: &Caller,
    agent: &AgentRow,
    secret_id: Uuid,
) -> Result<Uuid, ApiError> {
    let mut tx = audit::begin(state).await?;
    let id = grants::delete_secret_grant(&mut *tx, agent.id, secret_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Grant not found."))?;
    derived_rules::delete_for_source(&mut *tx, "grant", id).await?;
    credentials::notify_gateway_invalidate(&mut *tx, "agent", agent.id).await?;
    let ev = event(
        caller,
        "grant.delete",
        agent,
        json!({"secret_id": secret_id, "agent_handle": agent.handle}),
    );
    audit::commit_critical(tx, &ev).await?;
    Ok(id)
}

fn invalid(reason: &str, message: &str) -> ApiError {
    ApiError::unprocessable("grant_invalid", message).with_details(json!({"reason": reason}))
}

fn dedupe(v: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    v.into_iter().filter(|t| seen.insert(t.clone())).collect()
}

/// Agent-side `PUT`: create or replace the connection grant in its own transaction.
pub async fn grant_connection(
    state: &AppState,
    caller: &Caller,
    agent_id: Uuid,
    connection_id: Uuid,
    req: PutConnectionGrant,
) -> Result<ConnectionGrant, ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let w = grant_connection_in_tx(
        &mut tx,
        state,
        caller,
        agent_id,
        connection_id,
        req,
        Origin::Agent,
    )
    .await?;
    if let Some(ev) = &w.event {
        audit::commit_critical(tx, ev).await?;
    }
    Ok(w.grant)
}

/// THE authorization path of a connection grant (SEC-77; see [`grant_secret_in_tx`]): agent
/// visible and `can_own`, connection live in the agent's workspace or org, the grant body
/// valid (07 §Grants), then write, compile and invalidate in `tx`.
pub async fn grant_connection_in_tx(
    tx: &mut audit::Tx,
    state: &AppState,
    caller: &Caller,
    agent_id: Uuid,
    connection_id: Uuid,
    req: PutConnectionGrant,
    origin: Origin,
) -> Result<Written<ConnectionGrant>, ApiError> {
    let (agent, auth) = visible_agent(caller, state, agent_id).await?;
    crate::http::auth::require_own(&auth)?;
    write_connection_grant(
        tx,
        state,
        &agent,
        connection_id,
        req,
        origin,
        AuditActor::from_caller(caller),
        Some(caller.user_id),
    )
    .await
}

/// Starter grants (P4-ONB-1): a full-access connection grant written as the system; see
/// [`grant_secret_as_system`].
pub async fn grant_connection_as_system(
    tx: &mut audit::Tx,
    state: &AppState,
    agent: &AgentRow,
    connection_id: Uuid,
    actor: AuditActor,
) -> Result<Written<ConnectionGrant>, ApiError> {
    write_connection_grant(
        tx,
        state,
        agent,
        connection_id,
        PutConnectionGrant {
            mode: GrantMode::Full,
            allow_tools: Vec::new(),
            ask_tools: Vec::new(),
            ask_timeout_secs: None,
        },
        Origin::Credential,
        actor,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn write_connection_grant(
    tx: &mut audit::Tx,
    state: &AppState,
    agent: &AgentRow,
    connection_id: Uuid,
    req: PutConnectionGrant,
    origin: Origin,
    actor: AuditActor,
    by: Option<Uuid>,
) -> Result<Written<ConnectionGrant>, ApiError> {
    // Everything below runs on the transaction's own connection: a second pool connection taken
    // while this one is pinned could starve the pool under many concurrent writers.
    let conn = connections::get(&mut **tx, connection_id)
        .await?
        .filter(|c| {
            c.org_id == agent.org_id
                && c.status != "orphaned"
                && c.workspace_id.is_none_or(|w| w == agent.workspace_id)
        })
        .ok_or_else(|| ApiError::not_found("Connection not found."))?;
    // (P4-GRANTS, D-123, SEC-67) An app whose manifest says `custom_only` is never granted in
    // `full` mode, on any path: this function is the only writer of connection grants, so the
    // agent-side PUT and the credential-side bulk route both stop here.
    //
    // The activation of a plugin version (which converts `full` grants of a newly `custom_only`
    // app) holds the exclusive compile lock for its whole transaction. Taking the same lock in
    // shared mode here, *before* reading the restrictions, closes the read-then-write race: either
    // the activation committed first (this read, a fresh statement after the lock, sees the new
    // snapshot and refuses `full`), or it waits for this transaction and then converts the grant
    // written here. The lock is released with the transaction.
    sqlx::query(
        "SELECT pg_advisory_xact_lock_shared(hashtextextended('aurora:registry:compile', 0))",
    )
    .execute(&mut **tx)
    .await
    .map_err(DbError::from)?;
    let snap = crate::domains::plugins::registry::active_typed_on(tx).await?;
    let restrictions = snap
        .as_ref()
        .and_then(|s| s.app(&conn.app_id).map(|a| AppRestrictions::from(a.grants)))
        .unwrap_or_default();
    if restrictions.custom_only && req.mode == GrantMode::Full {
        return Err(ApiError::unprocessable(
            "full_grant_forbidden",
            "This app only allows custom access: choose the tools it may use instead of full access.",
        )
        .with_details(json!({"app_id": conn.app_id})));
    }
    let (allow, ask) = match req.mode {
        GrantMode::Full => (Vec::new(), Vec::new()),
        GrantMode::Custom => (dedupe(req.allow_tools), dedupe(req.ask_tools)),
    };
    // (RFE-011a) The ask timeout belongs to custom grants that have ask tools, within bounds.
    let ask_timeout = match req.ask_timeout_secs {
        None => None,
        Some(t) => {
            if req.mode != GrantMode::Custom || ask.is_empty() {
                return Err(invalid(
                    "ask_timeout_without_ask_tools",
                    "An ask timeout needs a custom grant with ask tools.",
                ));
            }
            if let Some(m) = crate::domains::policy::validate::timeout_range_error(
                t,
                state.config.approvals.max_timeout_secs,
            ) {
                return Err(crate::domains::policy::validate::timeout_out_of_range(
                    vec![aurora_core::FieldError::new("/ask_timeout_secs", m)],
                ));
            }
            // Range-checked above (60..=14400): the conversion cannot fail.
            i32::try_from(t).ok()
        }
    };
    if req.mode == GrantMode::Custom {
        if allow.is_empty() && ask.is_empty() {
            return Err(invalid(
                "empty_custom",
                "A custom grant needs at least one tool; detach the connection instead.",
            ));
        }
        if allow.iter().any(|t| ask.contains(t)) {
            return Err(invalid(
                "tool_in_both",
                "A tool cannot be both allowed and set to ask.",
            ));
        }
        let known: HashSet<String> = snap
            .as_ref()
            .and_then(|s| s.app(&conn.app_id))
            .map(|a| apps::tools(a).into_iter().map(|t| t.id).collect())
            .unwrap_or_default();
        if allow.iter().chain(&ask).any(|t| !known.contains(t)) {
            return Err(invalid(
                "unknown_tool",
                "A tool is not in the app's catalogue.",
            ));
        }
    }
    let (g, created) = if origin.replaces() {
        let g = grants::upsert_connection_grant(
            &mut **tx,
            agent.workspace_id,
            agent.id,
            conn.id,
            req.mode.as_str(),
            &allow,
            &ask,
            ask_timeout,
            by,
        )
        .await?;
        (g, true)
    } else {
        match grants::insert_connection_grant_if_absent(
            &mut **tx,
            agent.workspace_id,
            agent.id,
            conn.id,
            req.mode.as_str(),
            &allow,
            &ask,
            ask_timeout,
            by,
        )
        .await?
        {
            Some(g) => (g, true),
            None => {
                let g = grants::get_connection_grant(&mut **tx, agent.id, conn.id)
                    .await?
                    .ok_or_else(|| ApiError::not_found("Grant not found."))?;
                (g, false)
            }
        }
    };
    let grant = ConnectionGrant::from(g).restricted(restrictions);
    if !created {
        return Ok(Written {
            grant,
            created,
            event: None,
        });
    }
    compile_connection_grant(
        tx,
        agent.org_id,
        agent.workspace_id,
        grant.id,
        agent.id,
        conn.id,
        &conn.label,
        req.mode,
        &allow,
        &ask,
        ask_timeout,
    )
    .await?;
    credentials::notify_gateway_invalidate(&mut **tx, "agent", agent.id).await?;
    let ev = event_by(
        actor,
        "grant.create",
        agent,
        with_via(
            origin,
            json!({"connection_id": conn.id, "agent_handle": agent.handle,
                   "grant": {"mode": req.mode.as_str(), "allow_tools": allow, "ask_tools": ask,
                             "ask_timeout_secs": ask_timeout}}),
        ),
    );
    Ok(Written {
        grant,
        created,
        event: Some(ev),
    })
}

/// Remove the connection grant and its derived rules (404 when absent).
pub async fn revoke_connection(
    state: &AppState,
    caller: &Caller,
    agent: &AgentRow,
    connection_id: Uuid,
) -> Result<Uuid, ApiError> {
    let mut tx = audit::begin(state).await?;
    let id = grants::delete_connection_grant(&mut *tx, agent.id, connection_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Grant not found."))?;
    derived_rules::delete_for_source(&mut *tx, "grant", id).await?;
    credentials::notify_gateway_invalidate(&mut *tx, "agent", agent.id).await?;
    let ev = event(
        caller,
        "grant.delete",
        agent,
        json!({"connection_id": connection_id, "agent_handle": agent.handle, "grant": null}),
    );
    audit::commit_critical(tx, &ev).await?;
    Ok(id)
}
