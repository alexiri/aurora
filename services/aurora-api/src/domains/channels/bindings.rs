//! Space bindings of shared bots (docs/13 §5.3): dashboard listing, edit and removal by
//! workspace owners (`can_own`).

use super::dto::*;
use super::inbox::send::{self, Ctx};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::connectors::ops::{self, CORRELATED_WAIT, CorrelatedError, NewOp};
use crate::domains::conversations::service::load_agent;
use crate::http::auth::{Caller, require_own};
use aurora_core::{ApiError, FieldError, sanitize};
use aurora_db::repo::agents::AgentRow;
use aurora_db::repo::channel_integrations::{self as integrations_repo, IntegrationRow};
use aurora_db::repo::space_bindings::{self as repo, BindingRow};
use aurora_db::repo::{channel_presences as presences, connectors, user_links};
use aurora_protocol::connector::{
    Sender, Space, SpaceKind, SpaceMembersOp, SpacesListOp, SpacesResult, UsersResult,
};
use serde_json::json;
use uuid::Uuid;

fn view(r: &BindingRow) -> SpaceBinding {
    SpaceBinding {
        id: r.id,
        integration_id: r.integration_id,
        space_ref: r.space_ref.clone(),
        space_label: r.space_label.clone(),
        agent_id: r.agent_id,
        listen: Listen::parse(&r.listen),
        followups_need_mention: r.followups_need_mention,
        active: r.active,
        bound_by: r.bound_by_user_id.map(|id| UserRef {
            id,
            display_name: r.bound_by_name.clone().unwrap_or_default(),
        }),
        created_at: r.created_at,
    }
}

async fn owned_agent(s: &AppState, c: &Caller, agent: Uuid) -> Result<AgentRow, ApiError> {
    let a = load_agent(s, c, agent).await?;
    require_own(&c.workspace(s, a.workspace_id).await?)?;
    Ok(a)
}

pub async fn list(s: &AppState, c: &Caller, agent: Uuid) -> Result<Vec<SpaceBinding>, ApiError> {
    owned_agent(s, c, agent).await?;
    Ok(repo::list_for_agent(&s.pool, agent)
        .await?
        .iter()
        .map(view)
        .collect())
}

async fn load(s: &AppState, c: &Caller, id: Uuid) -> Result<(BindingRow, AgentRow), ApiError> {
    let not_found = || ApiError::not_found("Binding not found.");
    let row = repo::get(&s.pool, id).await?.ok_or_else(not_found)?;
    if !row.active {
        return Err(not_found());
    }
    // Bindings the caller cannot see are indistinguishable from missing ones.
    let agent = load_agent(s, c, row.agent_id).await.map_err(|e| {
        if e.status() == http::StatusCode::NOT_FOUND {
            not_found()
        } else {
            e
        }
    })?;
    require_own(&c.workspace(s, agent.workspace_id).await?)?;
    Ok((row, agent))
}

pub async fn patch(
    s: &AppState,
    c: &Caller,
    id: Uuid,
    req: PatchBindingRequest,
) -> Result<SpaceBinding, ApiError> {
    let (_, agent) = load(s, c, id).await?;
    repo::update(
        &s.pool,
        id,
        req.listen.map(Listen::as_str),
        req.followups_need_mention,
    )
    .await?;
    audit::record(
        s,
        AuditEvent::new(
            AuditActor::from_caller(c),
            "space_binding.update",
            Outcome::Success,
        )
        .org(agent.org_id)
        .workspace(agent.workspace_id)
        .target("space_binding", id)
        .metadata(json!({
            "listen": req.listen.map(Listen::as_str),
            "followups_need_mention": req.followups_need_mention,
        })),
    );
    let row = repo::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Binding not found."))?;
    Ok(view(&row))
}

pub async fn delete(s: &AppState, c: &Caller, id: Uuid) -> Result<(), ApiError> {
    let (row, agent) = load(s, c, id).await?;
    repo::delete(&s.pool, id).await?;
    audit::record(
        s,
        AuditEvent::new(
            AuditActor::from_caller(c),
            "space_binding.delete",
            Outcome::Success,
        )
        .org(agent.org_id)
        .workspace(agent.workspace_id)
        .target("space_binding", id)
        .metadata(json!({"integration_id": row.integration_id, "space_ref": row.space_ref})),
    );
    Ok(())
}

// ----------------------------------------------------- dashboard creation (FIX-127, C-713/C-714)

const SPACES_SCAN: u32 = 200;

/// One correlated op against the integration (no presence: the shared bot's own view).
async fn call(
    s: &AppState,
    integration: Uuid,
    kind: &str,
    payload: serde_json::Value,
) -> Result<serde_json::Value, CorrelatedError> {
    ops::call_correlated(
        s,
        NewOp {
            kind: kind.to_owned(),
            connector_id: None,
            integration_id: Some(integration),
            presence_id: None,
            payload,
            dedupe_key: None,
        },
        CORRELATED_WAIT,
    )
    .await
}

fn unavailable() -> ApiError {
    ApiError::unavailable(
        "connector_unavailable",
        "The connector did not answer in time.",
    )
}

/// The active integration the person may use for binding, plus their linked chat account.
/// An active integration of an org the caller belongs to; anything else is a 404.
async fn visible_integration(
    s: &AppState,
    c: &Caller,
    integration: Uuid,
) -> Result<IntegrationRow, ApiError> {
    let not_found = integration_not_found;
    let integ = integrations_repo::get(&s.pool, integration)
        .await?
        .filter(|i| i.status == "active")
        .ok_or_else(not_found)?;
    // Another org's integration is indistinguishable from a missing one.
    if !c.org(s, integ.org_id).await.is_ok() {
        return Err(not_found());
    }
    Ok(integ)
}

async fn linked_account(
    s: &AppState,
    c: &Caller,
    integ: &IntegrationRow,
) -> Result<String, ApiError> {
    let link = user_links::list_for_user_in(&s.pool, c.user_id, integ.id)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| {
            ApiError::conflict(
                "link_required",
                "Link your chat account first (Account › Channels).",
            )
        })?;
    Ok(link.platform_user_ref)
}

fn members_of(v: serde_json::Value) -> Result<Vec<String>, ApiError> {
    let r: UsersResult = serde_json::from_value(v).map_err(|_| {
        ApiError::bad_gateway(
            "connector_error",
            "The connector returned an invalid result.",
        )
    })?;
    Ok(r.users.into_iter().map(|u| u.reference).collect())
}

/// The bot's spaces (the platform lists only spaces the bot is in), optionally filtered.
async fn bot_spaces(
    s: &AppState,
    integration: Uuid,
    query: Option<String>,
    limit: u32,
) -> Result<Vec<Space>, ApiError> {
    let v = call(
        s,
        integration,
        "spaces.list",
        json!(SpacesListOp { query, limit }),
    )
    .await
    .map_err(|e| match e {
        CorrelatedError::Unavailable => unavailable(),
        CorrelatedError::Failed { .. } => {
            ApiError::bad_gateway("connector_error", "The platform could not list its spaces.")
        }
    })?;
    Ok(serde_json::from_value::<SpacesResult>(v)
        .map_err(|_| {
            ApiError::bad_gateway(
                "connector_error",
                "The connector returned an invalid result.",
            )
        })?
        .spaces)
}

fn bindable(kind: &SpaceKind) -> bool {
    !matches!(kind, SpaceKind::Dm | SpaceKind::GroupDm)
}

/// `GET /v1/integrations/{id}/spaces?mine=true` (C-714).
pub async fn my_spaces(
    s: &AppState,
    c: &Caller,
    integration: Uuid,
    q: MySpacesQuery,
) -> Result<MySpacesList, ApiError> {
    if q.mine != Some(true) {
        return Err(ApiError::invalid_request(
            "Invalid request.",
            vec![FieldError::new("/mine", "must be true")],
        ));
    }
    // The role first: someone who cannot bind is told so, not asked to link an account.
    let integ = visible_integration(s, c, integration).await?;
    if !aurora_db::authz::owns_any_workspace(&s.pool, c.user_id, integ.org_id).await? {
        return Err(ApiError::forbidden("Workspace owner role required."));
    }
    let me = linked_account(s, c, &integ).await?;
    let limit = q.limit.unwrap_or(25).clamp(1, 50);
    let query = q
        .query
        .map(|t| sanitize::one_line(&t, 100))
        .filter(|t| !t.is_empty());
    let spaces: Vec<Space> = bot_spaces(s, integration, query, limit)
        .await?
        .into_iter()
        .filter(|sp| bindable(&sp.kind))
        .collect();
    let checks = spaces.iter().map(|sp| {
        call(
            s,
            integration,
            "space.members",
            json!(SpaceMembersOp {
                space_ref: sp.reference.clone()
            }),
        )
    });
    let results = futures::future::join_all(checks).await;
    if results
        .iter()
        .all(|r| matches!(r, Err(CorrelatedError::Unavailable)))
        && !results.is_empty()
    {
        return Err(unavailable());
    }
    let mut items = Vec::new();
    for (sp, r) in spaces.into_iter().zip(results) {
        let Ok(v) = r else { continue };
        if members_of(v)?.iter().any(|m| m == &me) {
            items.push(MySpace {
                kind: if matches!(sp.kind, SpaceKind::Private) {
                    "private"
                } else {
                    "channel"
                }
                .to_owned(),
                label: sanitize::label(&sp.label),
                space_ref: sp.reference,
                bot_present: true,
            });
        }
    }
    Ok(MySpacesList { items })
}

/// `POST /v1/agents/{agent}/space-bindings` (C-713).
pub async fn create(
    s: &AppState,
    c: &Caller,
    agent: Uuid,
    req: CreateBindingRequest,
) -> Result<SpaceBinding, ApiError> {
    let agent = owned_agent(s, c, agent).await?;
    let integ = visible_integration(s, c, req.integration_id).await?;
    if integ.org_id != agent.org_id {
        return Err(integration_not_found());
    }
    let me = linked_account(s, c, &integ).await?;
    let shared = presences::list_for_agent(&s.pool, agent.id)
        .await?
        .into_iter()
        .any(|p| p.integration_id == integ.id && p.mode == "shared" && p.status == "active");
    if !shared {
        return Err(ApiError::unprocessable(
            "no_shared_presence",
            "Attach this agent to the integration's shared bot first.",
        ));
    }
    let bot_ref = integ
        .shared_bot
        .as_ref()
        .and_then(|b| b.get("platform_user_ref"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let handle = integ
        .shared_bot
        .as_ref()
        .and_then(|b| b.get("handle"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("the bot")
        .to_owned();
    let invite = || {
        let hint = format!("Invite @{handle} to the channel, then try again.");
        ApiError::conflict("bot_not_in_space", hint.clone()).with_details(json!({"hint": hint}))
    };

    // Live checks through the connector, no cached ledger.
    let space_ref = req.space_ref.trim().to_owned();
    if space_ref.is_empty() || space_ref.len() > 256 {
        return Err(ApiError::invalid_request(
            "Invalid request.",
            vec![FieldError::new("/space_ref", "invalid")],
        ));
    }
    let listed = bot_spaces(s, integ.id, None, SPACES_SCAN)
        .await?
        .into_iter()
        .find(|sp| sp.reference == space_ref);
    if let Some(sp) = &listed
        && !bindable(&sp.kind)
    {
        return Err(not_bindable());
    }
    let members = call(
        s,
        integ.id,
        "space.members",
        json!(SpaceMembersOp {
            space_ref: space_ref.clone()
        }),
    )
    .await;
    let members = match members {
        Ok(v) => members_of(v)?,
        Err(CorrelatedError::Unavailable) => return Err(unavailable()),
        Err(CorrelatedError::Failed { code, .. }) if code == "forbidden" || code == "not_found" => {
            return Err(invite());
        }
        Err(CorrelatedError::Failed { .. }) => {
            return Err(ApiError::bad_gateway(
                "connector_error",
                "The platform could not list the space's members.",
            ));
        }
    };
    let Some(sp) = listed else {
        // Not among the bot's channels: a direct conversation the bot is in, or not the bot's.
        return Err(if members.iter().any(|m| m == &bot_ref) {
            not_bindable()
        } else {
            invite()
        });
    };
    if !members.iter().any(|m| m == &me) {
        return Err(ApiError::new(
            http::StatusCode::FORBIDDEN,
            "not_a_member",
            "You are not a member of that space.",
        ));
    }
    let id = Uuid::now_v7();
    let label = sanitize::label(&sp.label);
    let listen = req.listen.unwrap_or(Listen::Mentions);
    let res = repo::insert(
        &s.pool,
        &repo::NewBinding {
            id,
            integration_id: integ.id,
            space_ref: &space_ref,
            space_label: &label,
            agent_id: agent.id,
            listen: listen.as_str(),
            bound_by_user_id: Some(c.user_id),
        },
    )
    .await;
    match res {
        Ok(()) => {}
        Err(aurora_db::DbError::UniqueViolation { .. }) => {
            let same = repo::active_binding(&s.pool, integ.id, &space_ref)
                .await?
                .is_some_and(|b| b.agent_id == agent.id);
            return Err(if same {
                ApiError::conflict(
                    "already_bound",
                    "This agent is already bound to that space.",
                )
            } else {
                ApiError::conflict(
                    "space_already_bound",
                    "That space already has an agent. Unbind it first.",
                )
            });
        }
        Err(e) => return Err(e.into()),
    }
    audit::record(
        s,
        AuditEvent::new(
            AuditActor::from_caller(c),
            "space_binding.create",
            Outcome::Success,
        )
        .org(agent.org_id)
        .workspace(agent.workspace_id)
        .target("space_binding", agent.id)
        .metadata(json!({"integration_id": integ.id, "space_ref": space_ref, "user_id": c.user_id, "via": "dashboard"})),
    );
    let manifest = connectors::get(&s.pool, &integ.connector_id)
        .await?
        .and_then(|r| r.manifest);
    let ctx = Ctx::new(integ, manifest);
    let mut conn = s
        .pool
        .acquire()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if let Err(e) = send::say(
        &mut conn,
        &ctx,
        &Sender::shared(),
        &space_ref,
        "",
        &format!(
            "{} is now answering in this space. Mention me to talk.",
            sanitize::label(&agent.name)
        ),
        format!("bound:{id}"),
    )
    .await
    {
        tracing::warn!(error = %e, "binding announcement not queued");
    }
    let row = repo::get(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::internal("Binding vanished."))?;
    Ok(view(&row))
}

fn integration_not_found() -> ApiError {
    ApiError::new(
        http::StatusCode::NOT_FOUND,
        "integration_not_found",
        "Integration not found.",
    )
}

fn not_bindable() -> ApiError {
    ApiError::unprocessable(
        "space_not_bindable",
        "Direct conversations cannot be bound to an agent.",
    )
}
