//! Effective view (07 §Effective views): credential selection and per-tool decisions.
use super::dto::{
    AccountRef, AppAccess, AppAccessAccount, AppAccessTool, Effective, EffectiveCredential,
    EffectiveTool, ToolReason,
};
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
use uuid::Uuid;

/// (phase 2, RFE-005) The effective view for a given speaker (`None` = speakerless traffic).
pub async fn compute_for(
    state: &AppState,
    agent: &AgentRow,
    speaker: Option<Uuid>,
) -> Result<Effective, ApiError> {
    Ok(compute_inner(state, agent, speaker).await?.0)
}

/// The effective view plus the ids of the apps with at least one selected (granted) connection.
/// Speakerless: this is what the boot token gets, so `user` rules never select here (RFE-005).
pub async fn compute_with_apps(
    state: &AppState,
    agent: &AgentRow,
) -> Result<(Effective, BTreeSet<String>), ApiError> {
    compute_inner(state, agent, None).await
}

async fn compute_inner(
    state: &AppState,
    agent: &AgentRow,
    speaker: Option<Uuid>,
) -> Result<(Effective, BTreeSet<String>), ApiError> {
    let mut granted_apps = BTreeSet::new();
    let org: ScopePolicy =
        to_scope_policy(policy::load_scope_rows(&state.pool, agent.org_id, None, false).await?)?;
    let ws: ScopePolicy = to_scope_policy(
        policy::load_scope_rows(&state.pool, agent.org_id, Some(agent.workspace_id), false).await?,
    )?;
    // (P4-GRP-1, D-131) The speaker's current groups are resolved from the database and passed
    // as principals; no speaker, no groups.
    let speaker_group_ids = match speaker {
        Some(u) => gateway::speaker_group_ids(&state.pool, agent.org_id, u).await?,
        None => Vec::new(),
    };
    let identity = RequestIdentity {
        agent_id: agent.id,
        speaker_user_id: speaker,
        speaker_group_ids,
    };
    let allow: Vec<_> = org
        .ordered_rules
        .iter()
        .chain(&ws.ordered_rules)
        .filter(|r| r.enabled && r.action == Action::Allow)
        .collect();
    let rules: Vec<_> = allow
        .iter()
        .copied()
        .filter(|r| r.identity_matches(&identity))
        .collect();
    // Rules whose identities name a person: they make a credential's selection speaker-dependent.
    let user_rules: Vec<_> = allow
        .iter()
        .copied()
        .filter(|r| {
            r.identities.iter().any(|i| {
                matches!(
                    i,
                    aurora_policy::rule::Identity::User { .. }
                        | aurora_policy::rule::Identity::Group { .. }
                )
            })
        })
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
                depends_on_speaker: user_rules
                    .iter()
                    .any(|r| r.targets.iter().any(|t| t.selects(&cred))),
                status: None,
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
            let (cred, level) = connection_credential(&c, snap);
            let selected_by: Vec<_> = rules
                .iter()
                .filter(|r| r.targets.iter().any(|t| t.selects(&cred)))
                .map(|r| r.logical_id)
                .collect();
            let selected = !selected_by.is_empty();
            // FIX-014: only an active connection feeds a boot (env, stubs, snippets). The others
            // stay listed with their status; the gateway refuses them with the right reason.
            let usable = c.status == "active";
            if selected && usable {
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
                depends_on_speaker: user_rules
                    .iter()
                    .any(|r| r.targets.iter().any(|t| t.selects(&cred))),
                status: Some(c.status.clone()),
            });
            let Some(app) = snap.app(&c.app_id) else {
                continue;
            };
            let catalogue = apps::tools(app);
            if selected && usable {
                for t in &catalogue {
                    // (P5-UX-10, C-934) One function for this view and for "What exactly?".
                    let e = tool_eval(&org, &ws, &identity, &counter, &cred, &c, t);
                    let d = e.verdict;
                    let org_only = e.decision.as_ref().and_then(|_| {
                        evaluate(&org, &empty, &e.facts, &counter)
                            .trace
                            .organization
                            .verdict
                    });
                    let timeout = e.decision.as_ref().and_then(|x| x.approval_timeout_secs);
                    tools.push(EffectiveTool {
                        connection_id: c.id,
                        app_id: c.app_id.clone(),
                        tool_id: t.id.clone(),
                        decision: verdict_str(d).into(),
                        inert: false,
                        org_ceiling: org_only.map(|v| verdict_str(v).into()),
                        approval_timeout_secs: (d == Verdict::NeedsApproval).then(|| {
                            crate::domains::policy::simulate::effective_timeout_secs(
                                &state.ops().approvals,
                                timeout,
                            )
                        }),
                        approval_parks: crate::domains::policy::simulate::timeout_flags(
                            &state.ops().approvals,
                            timeout,
                            d == Verdict::NeedsApproval,
                        )
                        .0,
                        approval_timeout_clamped: crate::domains::policy::simulate::timeout_flags(
                            &state.ops().approvals,
                            timeout,
                            d == Verdict::NeedsApproval,
                        )
                        .1,
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
                            approval_timeout_secs: None,
                            approval_parks: false,
                            approval_timeout_clamped: false,
                        });
                    }
                }
            }
        }
    }
    let speaker_groups = aurora_db::repo::groups::names(&state.pool, &identity.speaker_group_ids)
        .await?
        .into_iter()
        .map(|(id, name, _, _)| super::dto::SpeakerGroup { id, name })
        .collect();
    Ok((
        Effective {
            credentials,
            tools,
            speaker_groups,
        },
        granted_apps,
    ))
}

/// The credential a connection would win with, and its level.
fn connection_credential(
    c: &gateway::AppConnectionRow,
    snap: &aurora_registry::snapshot::Snapshot,
) -> (Credential, Level) {
    let level = if c.workspace_id.is_some() {
        Level::Workspace
    } else {
        Level::Organization
    };
    // (FIX-084, SEC-67) An app absent from the snapshot is `custom_only` + `block_untooled`.
    let restrictions = snap
        .app(&c.app_id)
        .map_or(super::service::ABSENT_APP, |a| a.grants);
    let cred = Credential::Connection {
        id: c.id,
        app_id: c.app_id.clone(),
        scope: level,
        custom_only: restrictions.custom_only,
        block_untooled: restrictions.block_untooled,
        read_only: c.access != "write",
    };
    (cred, level)
}

/// (P5-UX-10, C-934) What the gateway would decide for one catalogue tool of one connection:
/// one synthetic request through the shared facts and the evaluator. `decision` is absent for a
/// tool the app does not offer for the connection's way of connecting (refused before any rule,
/// like an untooled request).
pub(crate) struct ToolEval {
    pub verdict: Verdict,
    pub decision: Option<aurora_policy::eval::Decision>,
    pub facts: RequestFacts,
}

pub(crate) fn tool_eval(
    org: &ScopePolicy,
    ws: &ScopePolicy,
    identity: &RequestIdentity,
    counter: &MemoryCounter,
    cred: &Credential,
    c: &gateway::AppConnectionRow,
    t: &crate::domains::apps::dto::Tool,
) -> ToolEval {
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
        // (FIX-159, FIX-136, D-241) The gateway's tags: the group, the kind and
        // whether the token cannot serve the tool (shown blocked).
        tool_tags: aurora_registry::identify::tags(
            t.group == "write",
            t.graphql.is_some(),
            t.git.is_some(),
            aurora_registry::identify::scope_missing(&t.scopes, &c.scopes),
        ),
        winning_credential: Some(cred.clone()),
        is_llm_host: false,
        identity: identity.clone(),
        connect_time: false,
        host_apps: Vec::new(),
    };
    // (FIX-123) A tool restricted to other connect methods is not the account's tool.
    let not_offered = !t.connect_methods.is_empty()
        && c.connect_method
            .as_deref()
            .is_some_and(|m| !t.connect_methods.iter().any(|x| x == m));
    if not_offered {
        return ToolEval {
            verdict: Verdict::Block {
                reason: aurora_policy::eval::BlockReason::AppUntooledBlocked,
            },
            decision: None,
            facts,
        };
    }
    let d = evaluate(org, ws, &facts, counter);
    ToolEval {
        verdict: d.verdict,
        decision: Some(d),
        facts,
    }
}

/// What the agent's own saved access says of a tool: the thing that is not a "reason".
fn own_choice(grant: &grants::ConnectionGrantRow, tool_id: &str) -> &'static str {
    if grant.mode == "full" || grant.allow_tools.iter().any(|x| x == tool_id) {
        "allow"
    } else if grant.ask_tools.iter().any(|x| x == tool_id) {
        "ask"
    } else {
        "block"
    }
}

fn decision_str(v: Verdict) -> &'static str {
    match v {
        Verdict::Allow | Verdict::RateLimited => "allow",
        Verdict::NeedsApproval => "ask",
        Verdict::Block { .. } => "block",
    }
}

/// (C-934) The deciding layer, only when it is not the agent's own access.
fn reason_for(
    e: &ToolEval,
    own: &str,
    derived: &HashSet<Uuid>,
    org_account: bool,
    org_name: &str,
) -> Option<ToolReason> {
    use aurora_policy::eval::{ScopeKind, Via};
    if decision_str(e.verdict) == own {
        return None;
    }
    let package = || ToolReason {
        layer: "package".into(),
        label: "The app does not offer this for this account.".into(),
        link: None,
    };
    let Some(d) = &e.decision else {
        return Some(package());
    };
    let scope = d.trace.decided_by?;
    let (trace, matched) = match scope {
        ScopeKind::Organization => (&d.trace.organization, d.org_matched.as_ref()),
        ScopeKind::Workspace => (&d.trace.workspace, d.workspace_matched.as_ref()),
    };
    let org_layer = scope == ScopeKind::Organization;
    match trace.via {
        Via::ReadOnlyBlocked => org_account.then(|| ToolReason {
            layer: "limit".into(),
            label: format!("Limited by {org_name}"),
            link: None,
        }),
        Via::AppUntooledBlocked | Via::ToolScopeBlocked => Some(package()),
        Via::Rule => {
            let m = matched?;
            if derived.contains(&m.logical_id) {
                // Written by a grant or the platform, not by a person.
                org_layer.then(|| ToolReason {
                    layer: "organization".into(),
                    label: format!("Set by {org_name}"),
                    link: None,
                })
            } else {
                Some(ToolReason {
                    layer: if org_layer {
                        "org_rule"
                    } else {
                        "workspace_rule"
                    }
                    .into(),
                    label: m.name.clone(),
                    link: Some("internet_access".into()),
                })
            }
        }
        Via::Default | Via::Uncredentialed | Via::UncredentialedAllowlist => {
            if org_layer {
                Some(ToolReason {
                    layer: "organization".into(),
                    label: format!("Set by {org_name}"),
                    link: None,
                })
            } else {
                Some(ToolReason {
                    layer: "workspace_rule".into(),
                    label: "The workspace's default rule".into(),
                    link: Some("internet_access".into()),
                })
            }
        }
        Via::DefaultSkippedLlmHost | Via::Nothing => None,
    }
}

/// (P5-UX-10, C-934) What actually happens for each tool of an app, for each account of it the
/// agent has been given; computed like the gateway, with a reason only when something other
/// than the agent's own access decided.
pub async fn app_access(
    state: &AppState,
    agent: &AgentRow,
    app_id: &str,
    speaker: Option<Uuid>,
) -> Result<AppAccess, ApiError> {
    let snap = apps::snapshot(state)
        .await?
        .ok_or_else(|| ApiError::not_found("App not found."))?;
    let app = snap
        .app(app_id)
        .ok_or_else(|| ApiError::not_found("App not found."))?;
    let rows = policy::load_scope_rows(&state.pool, agent.org_id, None, false).await?;
    let wrows =
        policy::load_scope_rows(&state.pool, agent.org_id, Some(agent.workspace_id), false).await?;
    let derived: HashSet<Uuid> = rows
        .derived
        .iter()
        .chain(&wrows.derived)
        .map(|(id, _)| *id)
        .collect();
    let org = to_scope_policy(rows)?;
    let ws = to_scope_policy(wrows)?;
    let speaker_group_ids = match speaker {
        Some(u) => gateway::speaker_group_ids(&state.pool, agent.org_id, u).await?,
        None => Vec::new(),
    };
    let identity = RequestIdentity {
        agent_id: agent.id,
        speaker_user_id: speaker,
        speaker_group_ids,
    };
    let conns = gateway::app_connections(
        &state.pool,
        agent.org_id,
        agent.workspace_id,
        &[app_id.to_owned()],
    )
    .await?;
    let grant_rows = grants::list_connection_grants(&state.pool, agent.id).await?;
    let org_name = aurora_db::repo::orgs::name_of(&state.pool, agent.org_id)
        .await?
        .unwrap_or_default();
    let catalogue = apps::tools(app);
    let counter = MemoryCounter::new();
    let mut accounts = Vec::new();
    for c in conns {
        let Some(grant) = grant_rows.iter().find(|g| g.connection_id == c.id) else {
            continue;
        };
        let (cred, level) = connection_credential(&c, &snap);
        let org_account = level == Level::Organization;
        let tools = catalogue
            .iter()
            .map(|t| {
                let e = tool_eval(&org, &ws, &identity, &counter, &cred, &c, t);
                let own = own_choice(grant, &t.id);
                AppAccessTool {
                    tool_id: t.id.clone(),
                    name: t.name.clone(),
                    description: t.description.clone(),
                    access: if t.group == "write" { "write" } else { "read" }.into(),
                    decision: decision_str(e.verdict).into(),
                    own: own.into(),
                    reason: reason_for(&e, own, &derived, org_account, &org_name),
                }
            })
            .collect();
        accounts.push(AppAccessAccount {
            r#ref: AccountRef {
                r#type: "connection".into(),
                id: c.id,
            },
            name: c.label.clone(),
            tools,
        });
    }
    Ok(AppAccess { accounts })
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
