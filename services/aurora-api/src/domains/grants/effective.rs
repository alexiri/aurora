//! Effective view (07 §Effective views): credential selection and per-tool decisions.
use super::dto::{Effective, EffectiveCredential, EffectiveTool};
use crate::app::AppState;
use crate::domains::apps::service as apps;
use crate::domains::policy::simulate::to_scope_policy;
use aurora_core::ApiError;
use aurora_db::repo::agents::AgentRow;
use aurora_db::repo::{gateway, grants, policy};
use aurora_policy::RequestFacts;
use aurora_policy::eval::{MemoryCounter, ScopePolicy, Verdict, evaluate};
use aurora_policy::facts::{Level, SecretKind};
use aurora_policy::rule::Action;
use aurora_policy::{Credential, RequestIdentity};
use std::collections::{BTreeSet, HashSet};

pub async fn compute(state: &AppState, agent: &AgentRow) -> Result<Effective, ApiError> {
    Ok(compute_with_apps(state, agent).await?.0)
}

/// The effective view plus the ids of the apps with at least one selected (granted) connection.
pub async fn compute_with_apps(
    state: &AppState,
    agent: &AgentRow,
) -> Result<(Effective, BTreeSet<String>), ApiError> {
    let mut granted_apps = BTreeSet::new();
    let org: ScopePolicy =
        to_scope_policy(policy::load_scope_rows(&state.pool, agent.org_id, None, false).await?)?;
    let ws: ScopePolicy = to_scope_policy(
        policy::load_scope_rows(&state.pool, agent.org_id, Some(agent.workspace_id), false).await?,
    )?;
    let identity = RequestIdentity {
        agent_id: agent.id,
        principals: gateway::principals(&state.pool, agent.workspace_id).await?,
    };
    let rules: Vec<_> = org
        .ordered_rules
        .iter()
        .chain(&ws.ordered_rules)
        .filter(|r| r.enabled && r.action == Action::Allow && r.identity_matches(&identity))
        .collect();
    let secrets = gateway::active_secrets(&state.pool, agent.org_id, agent.workspace_id).await?;
    let mut credentials: Vec<EffectiveCredential> = secrets
        .into_iter()
        .map(|s| {
            let level = if s.workspace_id.is_some() {
                Level::Workspace
            } else {
                Level::Organization
            };
            let cred = Credential::Secret {
                id: s.id,
                kind: if s.kind == "llm" {
                    SecretKind::Llm
                } else {
                    SecretKind::Generic
                },
                scope: level,
            };
            let selected_by: Vec<_> = rules
                .iter()
                .filter(|r| r.targets.iter().any(|t| t.selects(&cred)))
                .map(|r| r.logical_id)
                .collect();
            EffectiveCredential {
                kind: "secret".into(),
                id: s.id,
                name: s.name,
                scope: match level {
                    Level::Workspace => "workspace",
                    Level::Organization => "organization",
                }
                .into(),
                selected: !selected_by.is_empty(),
                selected_by,
            }
        })
        .collect();

    // Connections of the agent's workspace and organization, for apps in the catalogue.
    let snap = apps::snapshot(state).await?;
    let mut tools = Vec::new();
    if let Some(snap) = &snap {
        let app_ids: Vec<String> = snap.apps.iter().map(|a| a.app_id.clone()).collect();
        let conns =
            gateway::app_connections(&state.pool, agent.org_id, agent.workspace_id, &app_ids)
                .await?;
        let grant_rows = grants::list_connection_grants(&state.pool, agent.id).await?;
        let counter = MemoryCounter::new();
        let empty = ScopePolicy::default();
        for c in conns {
            let level = if c.workspace_id.is_some() {
                Level::Workspace
            } else {
                Level::Organization
            };
            let cred = Credential::Connection {
                id: c.id,
                app_id: c.app_id.clone(),
                scope: level,
            };
            let selected_by: Vec<_> = rules
                .iter()
                .filter(|r| r.targets.iter().any(|t| t.selects(&cred)))
                .map(|r| r.logical_id)
                .collect();
            let selected = !selected_by.is_empty();
            if selected {
                granted_apps.insert(c.app_id.clone());
            }
            credentials.push(EffectiveCredential {
                kind: "connection".into(),
                id: c.id,
                name: c.label.clone(),
                scope: match level {
                    Level::Workspace => "workspace",
                    Level::Organization => "organization",
                }
                .into(),
                selected,
                selected_by,
            });
            let Some(app) = snap.app(&c.app_id) else {
                continue;
            };
            let catalogue = apps::tools(app);
            if selected {
                for t in &catalogue {
                    let facts = RequestFacts {
                        host: example(&t.host),
                        port: 443,
                        path: example_path(&t.path),
                        query: None,
                        method: t.methods.first().map_or("GET", String::as_str).to_owned(),
                        body_prefix: Vec::new(),
                        truncated: false,
                        app_id: Some(c.app_id.clone()),
                        tool_id: Some(t.id.clone()),
                        tool_tags: aurora_policy::facts::ToolTags {
                            graphql: t.graphql.is_some(),
                            git: t.git.is_some(),
                        },
                        winning_credential: Some(cred.clone()),
                        is_llm_host: false,
                        identity: identity.clone(),
                    };
                    let d = evaluate(&org, &ws, &facts, &counter);
                    let org_only = evaluate(&org, &empty, &facts, &counter);
                    tools.push(EffectiveTool {
                        connection_id: c.id,
                        app_id: c.app_id.clone(),
                        tool_id: t.id.clone(),
                        decision: verdict_str(d.verdict).into(),
                        inert: false,
                        org_ceiling: org_only
                            .trace
                            .organization
                            .verdict
                            .map(|v| verdict_str(v).into()),
                    });
                }
            }
            // Granted tools that left the catalogue match nothing.
            let known: HashSet<&str> = catalogue.iter().map(|t| t.id.as_str()).collect();
            for g in grant_rows.iter().filter(|g| g.connection_id == c.id) {
                for id in g.allow_tools.iter().chain(&g.ask_tools) {
                    if !known.contains(id.as_str()) {
                        tools.push(EffectiveTool {
                            connection_id: c.id,
                            app_id: c.app_id.clone(),
                            tool_id: id.clone(),
                            decision: "inert".into(),
                            inert: true,
                            org_ceiling: None,
                        });
                    }
                }
            }
        }
    }
    Ok((Effective { credentials, tools }, granted_apps))
}

fn verdict_str(v: Verdict) -> &'static str {
    match v {
        Verdict::Allow => "allow",
        Verdict::NeedsApproval => "needs_approval",
        Verdict::RateLimited => "rate_limited",
        Verdict::Block { .. } => "block",
    }
}

/// A concrete host matching a tool's host pattern.
fn example(pattern: &str) -> String {
    pattern.replace('*', "x")
}

/// A concrete path matching a tool's path pattern.
fn example_path(pattern: &str) -> String {
    let p = pattern.replace('*', "x");
    if p.starts_with('/') {
        p
    } else {
        format!("/{p}")
    }
}
