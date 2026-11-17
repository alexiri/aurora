use super::compile::{compile_connection_grant, compile_secret_grant};
use super::dto::{
    AppRestrictions, BulkConnectionGrantItem, BulkConnectionGrantRequest, BulkSecretGrantItem,
    BulkStatus, ConnectionGrant, ConnectionGrantView, GRANT_BULK_MAX, GrantMode, GrantedApp,
    PutConnectionAccess, PutConnectionGrant, SecretGrant, SecretGrantView,
};
use super::levels;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::apps::service as apps;
use crate::http::auth::Caller;
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::agents::{self, AgentRow};
use aurora_db::repo::{approvals, connections, credentials, derived_rules, grants, secrets};
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
    /// (P5-UX-2, C-957) The connect journey gave the access together with the new account: the
    /// grant is new by construction; audited with `details.via = "connect_journey"`.
    ConnectJourney,
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
    match origin {
        Origin::Credential => meta["via"] = json!("credential"),
        Origin::ConnectJourney => meta["via"] = json!("connect_journey"),
        Origin::Agent => {}
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
    agent: &AgentRow,
    secret_id: Uuid,
    actor: AuditActor,
) -> Result<Written<SecretGrant>, ApiError> {
    write_secret_grant(tx, agent, secret_id, Origin::Credential, actor, None).await
}

/// The secret must be active and in the agent's workspace or org (404), then the grant, its
/// derived rule and the gateway invalidation are written in `tx`.
async fn write_secret_grant(
    tx: &mut audit::Tx,
    agent: &AgentRow,
    secret_id: Uuid,
    origin: Origin,
    actor: AuditActor,
    by: Option<Uuid>,
) -> Result<Written<SecretGrant>, ApiError> {
    // Read on the transaction's own connection: a secret created in this very transaction
    // (P5-UX-2, C-988) is not visible to another one until it commits.
    let secret = secrets::get(&mut **tx, secret_id)
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

/// (FIX-084, SEC-67) An app the active snapshot does not hold (its package is deactivated) is
/// treated as `custom_only` with `block_untooled` by every grant writer and evaluator: with no
/// manifest to say otherwise, the restrictive reading is the safe one.
pub const ABSENT_APP: aurora_registry::manifest::Grants = aurora_registry::manifest::Grants {
    custom_only: true,
    block_untooled: true,
};

/// (P4-GRANTS, D-123) The manifest restrictions of an app in the active snapshot; both `false`
/// for an app without `grants`, both `true` for one the snapshot does not hold ([`ABSENT_APP`]).
pub async fn app_restrictions(state: &AppState, app_id: &str) -> Result<AppRestrictions, ApiError> {
    Ok(apps::snapshot(state)
        .await?
        .and_then(|s| s.app(app_id).map(|a| a.grants))
        .unwrap_or(ABSENT_APP)
        .into())
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
                .map_or(ABSENT_APP, |a| a.grants);
            (id, AppRestrictions::from(r))
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

pub(crate) fn bulk_check(agent_ids: &[Uuid]) -> Result<(), ApiError> {
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

/// The checks a grant that travels with a **new** account makes before any write or redirect
/// (C-957, C-988, SEC-130), as the bulk routes make them (C-504): 1 to `GRANT_BULK_MAX` distinct
/// agents, each visible to the caller, owned by the caller (`can_own`) and in `workspace`. The
/// first failure is returned with `details.agent_id`.
pub async fn check_agents_for_new_account(
    state: &AppState,
    caller: &Caller,
    workspace: Uuid,
    agent_ids: &[Uuid],
) -> Result<(), ApiError> {
    bulk_check(agent_ids)?;
    for &id in agent_ids {
        let (agent, auth) = visible_agent(caller, state, id)
            .await
            .map_err(|e| at_agent(e, id))?;
        crate::http::auth::require_own(&auth).map_err(|e| at_agent(e, id))?;
        if agent.workspace_id != workspace {
            return Err(at_agent(ApiError::not_found("Agent not found."), id));
        }
    }
    Ok(())
}

pub(crate) fn at_agent(e: ApiError, agent_id: Uuid) -> ApiError {
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
    // (FIX-084) Held requests of this agent that would use a secret end with the grant.
    approvals::abandon_pending_depending_on(
        &mut tx,
        approvals::Dependency::AgentSecrets { agent_id: agent.id },
    )
    .await?;
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

/// (P5-UX-4, C-980) `PUT /agents/{agent}/grants/connections/{conn}` with a body that is either a
/// `level` or a `mode` with tool lists. A level is expanded here, inside the transaction, from
/// the app's tools in the snapshot; everything after that is the one writer of connection
/// grants, so a level obeys the same rules as a custom grant (limit, `custom_only`, access).
pub async fn grant_connection_access(
    state: &AppState,
    caller: &Caller,
    agent_id: Uuid,
    connection_id: Uuid,
    body: PutConnectionAccess,
) -> Result<ConnectionGrant, ApiError> {
    // Authorization first: nothing about the connection is answered to a caller who may not
    // own the agent's workspace.
    let (_, auth) = visible_agent(caller, state, agent_id).await?;
    crate::http::auth::require_own(&auth)?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let req = match (body.level, body.mode) {
        (Some(level), None) => {
            if !body.allow_tools.is_empty()
                || !body.ask_tools.is_empty()
                || body.ask_timeout_secs.is_some()
            {
                return Err(invalid(
                    "level_with_tools",
                    "A level carries no tool list: choose a level, or custom with tools.",
                ));
            }
            let conn = connections::get(&mut *tx, connection_id)
                .await?
                .ok_or_else(|| ApiError::not_found("Connection not found."))?;
            let snap = crate::domains::plugins::registry::active_typed_on(&mut tx).await?;
            let tools = snap
                .as_ref()
                .and_then(|s| s.app(&conn.app_id))
                .map(apps::tools)
                .unwrap_or_default();
            levels::expand(level, &tools)
        }
        (None, Some(mode)) => PutConnectionGrant {
            mode,
            allow_tools: body.allow_tools,
            ask_tools: body.ask_tools,
            ask_timeout_secs: body.ask_timeout_secs,
        },
        _ => {
            return Err(invalid(
                "level_or_mode",
                "Send a level, or a mode with its tools.",
            ));
        }
    };
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

/// (P5-UX-4, C-980) A workspace's account has no access setting of its own: the connection's
/// `access` follows every agent's saved choice (`write` when any grant is `full` or allows or
/// asks a write tool, else `read`). Called in the transaction of every grant change and removal.
async fn sync_connection_access(
    tx: &mut audit::Tx,
    connection_id: Uuid,
    app_id: &str,
    current: &str,
) -> Result<(), ApiError> {
    let snap = crate::domains::plugins::registry::active_typed_on(tx).await?;
    let tools = snap
        .as_ref()
        .and_then(|s| s.app(app_id))
        .map(apps::tools)
        .unwrap_or_default();
    let rows = grants::list_for_connection(&mut **tx, connection_id).await?;
    let want = if rows.iter().any(|g| {
        levels::access_for_saved(g.mode == "full", &g.allow_tools, &g.ask_tools, &tools) == "write"
    }) {
        "write"
    } else {
        "read"
    };
    if want != current {
        connections::update(&mut **tx, connection_id, None, None, Some(want), None).await?;
    }
    Ok(())
}

/// (FIX-154, D-306(c), C-896) `PUT /agents/{agent}/grants/apps/{app}`: give an agent a
/// credential-free app in one step. Checks before any write (404 `app_not_found`, 403
/// `app_unavailable`, 422 `app_not_credential_free`), then in ONE transaction, serialized per
/// (workspace, app): reuse the workspace's oldest active connection of the app or create one, and
/// upsert the grant exactly as `PUT …/grants/connections/{conn}` does.
pub async fn grant_app(
    state: &AppState,
    caller: &Caller,
    agent_id: Uuid,
    app_id: &str,
    req: PutConnectionGrant,
) -> Result<GrantedApp, ApiError> {
    let (agent, auth) = visible_agent(caller, state, agent_id).await?;
    crate::http::auth::require_own(&auth)?;
    let snap = crate::domains::plugins::registry::active_typed(state)
        .await?
        .ok_or_else(app_not_found)?;
    let (app, pkg) = apps::find_app(&snap, app_id).map_err(|_| app_not_found())?;
    if !crate::domains::connect::service::available_in_workspace(
        state,
        agent.org_id,
        agent.workspace_id,
        app_id,
    )
    .await?
    {
        return Err(ApiError::new(
            http::StatusCode::FORBIDDEN,
            "app_unavailable",
            "This app is not available here.",
        ));
    }
    if !aurora_registry::credential_free::entry_is_credential_free(app) {
        return Err(ApiError::unprocessable(
            "app_not_credential_free",
            "This app needs a sign-in or settings: connect it, then give the agent access.",
        )
        .with_details(json!({"app_id": app_id})));
    }
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("aurora:grant-app:{}:{app_id}", agent.workspace_id))
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
    let mut events = Vec::new();
    let (row, sealed) = crate::domains::connect::service::ensure_credential_free_in_tx(
        state,
        &mut tx,
        caller,
        &snap,
        app,
        pkg,
        agent.org_id,
        agent.workspace_id,
        agent.id,
        &mut events,
    )
    .await?;
    let created = sealed.is_some();
    let w = match write_connection_grant(
        &mut tx,
        state,
        &agent,
        row.id,
        req,
        Origin::Agent,
        AuditActor::from_caller(caller),
        Some(caller.user_id),
    )
    .await
    {
        Ok(w) => w,
        Err(e) => {
            // Rolled back with the transaction: the sealed (empty) credentials go too.
            drop(tx);
            if let Some(s) = sealed {
                s.discard(state).await;
            }
            return Err(e);
        }
    };
    events.extend(w.event);
    if let Err(e) = audit::commit_critical_many(tx, &events).await {
        if let Some(s) = sealed {
            s.discard(state).await;
        }
        return Err(e);
    }
    Ok(GrantedApp {
        connection: crate::domains::connections::dto::Connection::new(row, Some(&snap)),
        grant: w.grant,
        created,
    })
}

fn app_not_found() -> ApiError {
    ApiError::new(
        http::StatusCode::NOT_FOUND,
        "app_not_found",
        "App not found.",
    )
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
    // (FIX-084, SEC-67) An app absent from the snapshot is `custom_only`, never "no restrictions".
    let restrictions = AppRestrictions::from(
        snap.as_ref()
            .and_then(|s| s.app(&conn.app_id))
            .map_or(ABSENT_APP, |a| a.grants),
    );
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
                state.ops().approvals.max_timeout_secs,
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
    let app_tools: Vec<crate::domains::apps::dto::Tool> = snap
        .as_ref()
        .and_then(|s| s.app(&conn.app_id))
        .map(apps::tools)
        .unwrap_or_default();
    // (P5-UX-4, C-980, SEC-131) An organization's account has a limit; no grant above it is
    // written, whichever path asked (levels, custom lists, the bulk route, the journey). The
    // starter grants (`by == None`, written as the system) are the platform's own and unchecked.
    if conn.workspace_id.is_none() && by.is_some() {
        let limit = levels::limit_from_access(&conn.access);
        if levels::exceeds_limit(limit, req.mode == GrantMode::Full, &allow, &ask, &app_tools) {
            return Err(ApiError::unprocessable(
                "above_account_limit",
                "This account's limit does not allow that choice.",
            )
            .with_details({
                let mut d = crate::http::who_can::stub(
                    crate::http::who_can::WhoCanRole::OrgAdmin,
                    Some(conn.org_id),
                    None,
                );
                d["limit"] = json!(limit.as_str());
                d
            }));
        }
    }
    // (FIX-084) The previous grant, to tell a narrowing replace from a widening one.
    let previous = if origin.replaces() {
        grants::get_connection_grant(&mut **tx, agent.id, conn.id).await?
    } else {
        None
    };
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
    if let Some(p) = &previous
        && narrows(p, req.mode, &allow, &ask)
    {
        // Held requests of this agent that would use the connection were approved or are waiting
        // under the wider grant; they do not survive its narrowing.
        approvals::abandon_pending_depending_on(
            tx,
            approvals::Dependency::Connection {
                connection_id: conn.id,
                agent_id: Some(agent.id),
            },
        )
        .await?;
    }
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
    if conn.workspace_id.is_some() {
        sync_connection_access(tx, conn.id, &conn.app_id, &conn.access).await?;
    }
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

/// Whether replacing `old` by (`mode`, `allow`, `ask`) removes access: `full` becoming `custom`,
/// or a custom grant losing a tool (moving a tool between allow and ask keeps it granted).
fn narrows(
    old: &grants::ConnectionGrantRow,
    mode: GrantMode,
    allow: &[String],
    ask: &[String],
) -> bool {
    match (old.mode.as_str(), mode) {
        ("full", GrantMode::Full) => false,
        ("full", GrantMode::Custom) => true,
        (_, GrantMode::Full) => false,
        (_, GrantMode::Custom) => old
            .allow_tools
            .iter()
            .chain(&old.ask_tools)
            .any(|t| !allow.contains(t) && !ask.contains(t)),
    }
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
    // (P5-UX-4, C-980) Removing an agent's access recomputes the workspace account's `access`.
    if let Some(conn) = connections::get(&mut *tx, connection_id).await?
        && conn.workspace_id.is_some()
    {
        sync_connection_access(&mut tx, conn.id, &conn.app_id, &conn.access).await?;
    }
    // (FIX-084) Held requests of this agent that would use the connection end with the grant.
    approvals::abandon_pending_depending_on(
        &mut tx,
        approvals::Dependency::Connection {
            connection_id,
            agent_id: Some(agent.id),
        },
    )
    .await?;
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
