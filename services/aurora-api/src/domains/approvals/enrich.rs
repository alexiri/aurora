//! (P5-UX-16, C-982) The parts of an [`Approval`] that need more than its row: the app and account
//! of the sentence, what asked the question, and whether the caller may choose Always allow.

use super::dto::{AccountRefDto, Approval, AskedBy, SentenceAccount, SentenceApp};
use crate::app::AppState;
use crate::domains::apps::service as apps;
use crate::domains::grants::levels::{self, Level};
use aurora_db::authz;
use aurora_db::repo::approvals::{self, RuleOrigin};
use aurora_db::repo::connections;
use std::collections::HashMap;
use uuid::Uuid;

/// Lowercase the first letter of a tool's name so it completes "<agent> wants to …".
fn as_action(name: &str) -> String {
    let mut c = name.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Fills `sentence.app`, `sentence.account`, `asked_by` and `can_always_allow`. `chooser` is the
/// person who may choose Always allow: a dashboard session; `None` for keys and chat platforms
/// (SEC-131). A lookup that fails leaves the plain sentence: the card still works.
pub async fn enrich(s: &AppState, chooser: Option<Uuid>, items: &mut [Approval]) {
    let snap = crate::domains::plugins::registry::active_typed(s)
        .await
        .ok()
        .flatten();
    let mut conns: HashMap<Uuid, Option<connections::ConnectionRow>> = HashMap::new();
    let mut owns: HashMap<Uuid, bool> = HashMap::new();
    for a in items.iter_mut() {
        let conn = match a.connection_id {
            None => None,
            Some(id) => {
                if let std::collections::hash_map::Entry::Vacant(e) = conns.entry(id) {
                    e.insert(connections::get(&s.pool, id).await.ok().flatten());
                }
                conns.get(&id).and_then(Clone::clone)
            }
        };
        let app_id = a
            .app_id
            .clone()
            .or_else(|| conn.as_ref().map(|c| c.app_id.clone()));
        let entry = app_id
            .as_deref()
            .and_then(|id| snap.as_ref().and_then(|sn| sn.app(id)));
        if let (Some(id), Some(e)) = (&app_id, entry) {
            a.sentence.app = Some(SentenceApp {
                app_id: id.clone(),
                name: e.about.name.clone(),
            });
        }
        if let Some(c) = &conn {
            a.sentence.account = Some(SentenceAccount {
                reference: AccountRefDto {
                    kind: "connection".to_owned(),
                    id: c.id,
                },
                name: c.label.clone(),
            });
        }
        if a.summary.source == "generic"
            && let (Some(e), Some(tool)) = (entry, a.tool_id.as_deref())
            && let Some(t) = apps::tools(e).into_iter().find(|t| t.id == tool)
        {
            a.sentence.action = as_action(&t.name);
        }
        let Some(logical) = a.rule_logical_id else {
            continue;
        };
        let Ok(mut c) = s.pool.acquire().await else {
            continue;
        };
        let Ok(Some(origin)) = approvals::rule_origin(&mut c, logical).await else {
            continue;
        };
        drop(c);
        match origin {
            RuleOrigin::Rule { name, organization } => {
                a.asked_by = Some(AskedBy::Rule {
                    rule_id: logical,
                    rule_name: name,
                    scope: if organization {
                        "organization"
                    } else {
                        "workspace"
                    }
                    .to_owned(),
                });
            }
            RuleOrigin::Access {
                agent_id,
                connection_id,
                mode,
                ask_tools,
                ..
            } => {
                a.asked_by = Some(AskedBy::Access {
                    agent_id,
                    connection_id,
                    tool_id: a.tool_id.clone(),
                });
                let (Some(user), Some(tool)) = (chooser, a.tool_id.as_deref()) else {
                    continue;
                };
                // The same checks `decide` makes before it writes anything (SEC-131).
                let in_ask = a.status == "pending"
                    && mode == "custom"
                    && a.agent.as_ref().is_some_and(|g| g.id == agent_id)
                    && a.connection_id == Some(connection_id)
                    && ask_tools.iter().any(|t| t == tool);
                let limit_ok = conn.as_ref().is_some_and(|c| {
                    c.workspace_id.is_some() || levels::limit_from_access(&c.access) == Level::Act
                });
                if !(in_ask && limit_ok) {
                    continue;
                }
                let ws = a.workspace_id;
                let can = match owns.get(&ws) {
                    Some(v) => *v,
                    None => {
                        let v = authz::can_own(&s.pool, user, ws).await.unwrap_or(false);
                        owns.insert(ws, v);
                        v
                    }
                };
                a.can_always_allow = can;
            }
        }
    }
}
