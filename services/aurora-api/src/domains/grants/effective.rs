//! Effective view (07 §Effective views), secrets part.
use super::dto::{Effective, EffectiveCredential};
use crate::app::AppState;
use crate::domains::policy::simulate::to_scope_policy;
use aurora_core::ApiError;
use aurora_db::repo::agents::AgentRow;
use aurora_db::repo::{gateway, policy};
use aurora_policy::eval::ScopePolicy;
use aurora_policy::facts::{Level, SecretKind};
use aurora_policy::rule::Action;
use aurora_policy::{Credential, RequestIdentity};

pub async fn compute(state: &AppState, agent: &AgentRow) -> Result<Effective, ApiError> {
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
    let credentials = secrets
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
    Ok(Effective {
        credentials,
        tools: Vec::new(),
    })
}
