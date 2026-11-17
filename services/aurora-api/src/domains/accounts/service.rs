//! (P5-UX-4, C-917) One list of what agents can use. A list is a constant number of queries
//! (connections and secrets of both scopes, every grant of the scope joined with agents and
//! workspaces, the registry snapshot), whatever the number of accounts.

use super::dto::{
    Account, AccountAgent, AccountApp, AccountCan, AccountKind, AccountRef, AccountRefType,
    AccountWorkspace, AccountsQuery, AgentAccess, AgentAccessLevel, AlwaysAllowed, CustomChoice,
    HealthState, PersonName,
};
use super::health;
use crate::app::AppState;
use crate::domains::apps::dto::{AppFamily, Tool};
use crate::domains::apps::service as apps;
use crate::domains::grants::levels::{self, Choice, Level};
use crate::domains::grants::service as grants_service;
use crate::domains::plugins::registry;
use crate::http::auth::Caller;
use crate::http::pagination::Page;
use crate::http::who_can::{WhoCan, WhoCanRole};
use aurora_core::{ApiError, FieldError};
use aurora_db::DbError;
use aurora_db::repo::grants::{self, AccountGrantRow};
use aurora_db::repo::{connections, contacts, secrets};
use aurora_registry::snapshot::Snapshot;
use std::collections::HashMap;
use uuid::Uuid;

/// More accounts than one scope can hold in practice; the lists are not paged.
const SCOPE_MAX: i64 = 1000;

fn bad(field: &str, msg: &str) -> ApiError {
    ApiError::invalid_request("Invalid request.", vec![FieldError::new(field, msg)])
}

struct Scope {
    org: Uuid,
    workspace: Option<Uuid>,
    can: Can,
}

/// What the caller may do to each kind of account in this list.
#[derive(Clone, Copy)]
struct Can {
    /// Owns the workspace (workspace list), or any workspace of the organization (org list).
    owns: bool,
    /// Organization owner or admin.
    org_admin: bool,
}

/// `GET /workspaces/{ws}/accounts`.
pub async fn list_workspace(
    state: &AppState,
    caller: &Caller,
    ws: Uuid,
    q: &AccountsQuery,
) -> Result<Page<Account>, ApiError> {
    let auth = caller.workspace(state, ws).await?;
    check_query(q)?;
    let agent = match q.agent_id {
        Some(id) => {
            let (a, _) = grants_service::visible_agent(caller, state, id).await?;
            if a.workspace_id != ws {
                return Err(ApiError::not_found("Agent not found."));
            }
            Some(a)
        }
        None => None,
    };
    let scope = Scope {
        org: auth.org_id,
        workspace: Some(ws),
        can: Can {
            owns: auth.can_own(),
            org_admin: auth.org_role.is_admin(),
        },
    };
    list(state, caller, &scope, q, agent.as_ref()).await
}

/// `GET /orgs/{org}/accounts` (organization owners and admins).
pub async fn list_org(
    state: &AppState,
    caller: &Caller,
    org: Uuid,
    q: &AccountsQuery,
) -> Result<Page<Account>, ApiError> {
    let auth = caller.org(state, org).await?;
    auth.require_admin()?;
    check_query(q)?;
    let owns = aurora_db::authz::owns_any_workspace(&state.pool, caller.user_id, org).await?;
    let scope = Scope {
        org,
        workspace: None,
        can: Can {
            owns,
            org_admin: true,
        },
    };
    list(state, caller, &scope, q, None).await
}

fn check_query(q: &AccountsQuery) -> Result<(), ApiError> {
    if q.kind
        .as_deref()
        .is_some_and(|k| !["app", "website", "ai_key"].contains(&k))
    {
        return Err(bad("/kind", "must be app, website or ai_key"));
    }
    Ok(())
}

/// One account before the agents are attached.
struct Row {
    account: Account,
    tools: Vec<Tool>,
    limit: Option<Level>,
}

async fn list(
    state: &AppState,
    caller: &Caller,
    scope: &Scope,
    q: &AccountsQuery,
    for_agent: Option<&aurora_db::repo::agents::AgentRow>,
) -> Result<Page<Account>, ApiError> {
    let snap = registry::active_typed(state).await?;
    let want = |k: AccountKind| {
        q.kind.as_deref().is_none_or(|w| {
            w == match k {
                AccountKind::App => "app",
                AccountKind::Website => "website",
                AccountKind::AiKey => "ai_key",
            }
        })
    };

    // The scopes read: the workspace's own and the organization's (a workspace list), or only
    // the organization's.
    let mut scopes: Vec<Option<Uuid>> = Vec::new();
    if let Some(w) = scope.workspace {
        scopes.push(Some(w));
    }
    scopes.push(None);

    let managed_by = org_managed_by(state, scope, caller).await?;
    let mut rows: Vec<Row> = Vec::new();
    for ws in scopes {
        if want(AccountKind::App) {
            for c in connections::list_scope(
                &state.pool,
                scope.org,
                ws,
                q.app_id.as_deref(),
                None,
                None,
                SCOPE_MAX,
            )
            .await?
            .into_iter()
            .filter(|c| c.status != "orphaned")
            {
                rows.push(connection_row(&c, snap.as_ref(), scope, &managed_by));
            }
        }
        if q.app_id.is_none() && (want(AccountKind::Website) || want(AccountKind::AiKey)) {
            for s in secrets::list_scope(
                &state.pool,
                scope.org,
                ws,
                None,
                Some("active"),
                None,
                SCOPE_MAX,
            )
            .await?
            {
                let kind = if s.kind == "llm" {
                    AccountKind::AiKey
                } else {
                    AccountKind::Website
                };
                if want(kind) {
                    rows.push(secret_row(&s, kind, scope, &managed_by));
                }
            }
        }
    }

    // Every grant of the scope in one query, bucketed by account.
    let grants = grants::list_account_grants(&state.pool, scope.org, scope.workspace).await?;
    let mut by_account: HashMap<Uuid, Vec<&AccountGrantRow>> = HashMap::new();
    for g in &grants {
        if let Some(id) = g.connection_id.or(g.secret_id) {
            by_account.entry(id).or_default().push(g);
        }
    }

    let model = match for_agent {
        Some(a) if scope.can.owns => model_line(state, a).await,
        _ => None,
    };

    let mut items = Vec::with_capacity(rows.len());
    for mut r in rows {
        let id = r.account.reference.id;
        for g in by_account.get(&id).map_or(&[][..], Vec::as_slice) {
            let mut access = agent_access(g, &r);
            if r.account.kind == AccountKind::AiKey
                && for_agent.is_some_and(|a| a.id == g.agent_id)
                && let Some((key, m)) = &model
                && *key == id
            {
                access.model = Some(m.clone());
            }
            r.account.agents.push(access);
        }
        if let Some(a) = for_agent {
            r.account.given = Some(r.account.agents.iter().any(|x| x.agent.id == a.id));
        }
        // The host an unreachable account could not be reached at (its health line names it).
        if r.account.health.state == HealthState::Unreachable
            && r.account.reference.kind == AccountRefType::Connection
        {
            r.account.health.detail = super::test::unreachable_host(state, id).await;
        }
        items.push(r.account);
    }
    if for_agent.is_some() {
        // Given ones first; the order inside each group stays as listed.
        items.sort_by_key(|a| !a.given.unwrap_or(false));
    }
    Ok(Page {
        items,
        next_cursor: None,
    })
}

/// The model the agent's next turn uses and the key it goes through, for the AI-key row.
async fn model_line(
    state: &AppState,
    a: &aurora_db::repo::agents::AgentRow,
) -> Option<(Uuid, String)> {
    if a.kind != "hosted" {
        return None;
    }
    let harness = a
        .harness
        .clone()
        .unwrap_or_else(|| state.ops().hosted.default_harness.clone());
    let llm = state.llm();
    let mut conn = state.pool.acquire().await.map_err(DbError::from).ok()?;
    let r = crate::domains::hosted::model::readiness(
        state,
        &llm,
        &mut conn,
        a.id,
        a.org_id,
        a.workspace_id,
        &harness,
    )
    .await
    .ok()?;
    let d = r.default.as_ref()?;
    Some((r.keys.get(d.key)?.secret.id, d.model.clone()))
}

async fn org_managed_by(
    state: &AppState,
    scope: &Scope,
    _caller: &Caller,
) -> Result<Option<WhoCan>, ApiError> {
    // Built once per list: every organization account of it has the same managers.
    let path = if scope.can.org_admin {
        let (org_slug, _) = contacts::slugs(&state.pool, scope.org, None)
            .await?
            .ok_or_else(|| ApiError::not_found("Organization not found."))?;
        Some(aurora_core::dash_paths::org_apps(&org_slug, "accounts"))
    } else {
        None
    };
    Ok(Some(
        WhoCan::build(
            state,
            WhoCanRole::OrgAdmin,
            scope.org,
            None,
            "manage_account",
            path,
        )
        .await,
    ))
}

fn connection_row(
    c: &connections::ConnectionRow,
    snap: Option<&Snapshot>,
    scope: &Scope,
    managed_by: &Option<WhoCan>,
) -> Row {
    let entry = snap.and_then(|s| s.app(&c.app_id));
    let tools = entry.map(apps::tools).unwrap_or_default();
    let custom_only = entry.is_some_and(|a| a.grants.custom_only);
    let org_account = c.workspace_id.is_none();
    let limit = (org_account && tools.iter().any(|t| t.group != "read"))
        .then(|| levels::limit_from_access(&c.access));
    let health = health::health(
        Some(c.status.as_str()),
        c.last_check_result.as_deref(),
        c.last_check_at,
        c.last_used_at,
        health::refused_word(entry),
    );
    let account = Account {
        reference: AccountRef {
            kind: AccountRefType::Connection,
            id: c.id,
        },
        kind: AccountKind::App,
        name: c.label.clone(),
        app: Some(AccountApp {
            app_id: c.app_id.clone(),
            name: entry.map_or_else(|| c.app_id.clone(), |a| a.about.name.clone()),
            family: entry.and_then(|a| {
                a.about.family.as_ref().map(|f| AppFamily {
                    id: f.id.clone(),
                    name: f.name.clone(),
                    variant_label: f.variant_label.clone(),
                    instance_bound: a.instances.is_some(),
                })
            }),
            custom_only,
        }),
        scope: if org_account {
            "organization"
        } else {
            "workspace"
        }
        .to_owned(),
        limit,
        levels: levels::levels_for(&tools, custom_only, limit),
        health,
        status_reason: (c.status == "disabled")
            .then(|| c.status_reason.clone())
            .flatten(),
        connected_by: c.connected_by_name.clone().map(|name| PersonName { name }),
        agents: Vec::new(),
        managed_by: org_account.then(|| managed_by.clone()).flatten(),
        can: can_of(scope, org_account),
        given: None,
    };
    Row {
        account,
        tools,
        limit,
    }
}

fn secret_row(
    s: &secrets::SecretRow,
    kind: AccountKind,
    scope: &Scope,
    managed_by: &Option<WhoCan>,
) -> Row {
    let org_account = s.workspace_id.is_none();
    Row {
        account: Account {
            reference: AccountRef {
                kind: AccountRefType::Secret,
                id: s.id,
            },
            kind,
            name: s.name.clone(),
            app: None,
            scope: if org_account {
                "organization"
            } else {
                "workspace"
            }
            .to_owned(),
            limit: None,
            levels: Vec::new(),
            health: health::health(
                None,
                s.last_check_result.as_deref(),
                s.last_check_at,
                s.last_used_at,
                if kind == AccountKind::AiKey {
                    "key"
                } else {
                    "token"
                },
            ),
            status_reason: None,
            connected_by: s.created_by_name.clone().map(|name| PersonName { name }),
            agents: Vec::new(),
            managed_by: org_account.then(|| managed_by.clone()).flatten(),
            can: can_of(scope, org_account),
            given: None,
        },
        tools: Vec::new(),
        limit: None,
    }
}

fn can_of(scope: &Scope, org_account: bool) -> AccountCan {
    if org_account {
        // Giving access and choosing within the limit belong to workspace owners; the account
        // itself to organization owners and admins.
        AccountCan {
            give_access: scope.can.owns,
            change_access: scope.can.owns,
            manage: scope.can.org_admin,
        }
    } else {
        AccountCan {
            give_access: scope.can.owns,
            change_access: scope.can.owns,
            manage: scope.can.owns,
        }
    }
}

/// One agent's saved choice, read as it takes effect.
fn agent_access(g: &AccountGrantRow, r: &Row) -> AgentAccess {
    let mut a = AgentAccess {
        agent: AccountAgent {
            id: g.agent_id,
            name: g.agent_name.clone(),
            handle: g.agent_handle.clone(),
        },
        workspace: AccountWorkspace {
            id: g.workspace_id,
            name: g.workspace_name.clone(),
        },
        level: AgentAccessLevel::Use,
        always_allowed: None,
        custom: None,
        model: None,
    };
    if r.account.levels.is_empty() || g.connection_id.is_none() {
        return a;
    }
    let full = g.mode.as_deref() == Some("full");
    // A saved choice above the limit (data from before the limit) reads as the limit.
    if let Some(limit) = r.limit
        && levels::exceeds_limit(limit, full, &g.allow_tools, &g.ask_tools, &r.tools)
    {
        a.level = limit.into();
        return a;
    }
    let c = levels::classify(full, &g.allow_tools, &g.ask_tools, &r.tools);
    match c.choice {
        Choice::Level(l) => {
            a.level = l.into();
            if l == Level::Ask && !c.always_allowed.is_empty() {
                a.always_allowed = Some(
                    c.always_allowed
                        .iter()
                        .map(|id| AlwaysAllowed {
                            tool_id: id.clone(),
                            name: r
                                .tools
                                .iter()
                                .find(|t| &t.id == id)
                                .map_or_else(|| id.clone(), |t| t.name.clone()),
                        })
                        .collect(),
                );
            }
        }
        Choice::Custom { base, differs } => {
            a.level = AgentAccessLevel::Custom;
            a.custom = Some(CustomChoice {
                base,
                differs: u32::try_from(differs).unwrap_or(u32::MAX),
            });
        }
    }
    a
}
