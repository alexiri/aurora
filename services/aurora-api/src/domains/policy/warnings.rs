//! Authoring warnings (07 §Authoring): `shadowed`, `inert`, `overridden_by_org`.
//! Computed on the scope's draft (what the author is editing); `overridden_by_org`
//! compares against the organization's published rules.
use super::dto::*;
use super::validate::Scope;
use crate::app::AppState;
use aurora_core::ApiError;
use aurora_db::repo::policy as repo;
use aurora_policy::HostPattern;
use std::collections::HashMap;
use uuid::Uuid;

fn identities_cover(a: &[Identity], b: &[Identity]) -> bool {
    a.is_empty() || (!b.is_empty() && b.iter().all(|x| a.contains(x)))
}

fn tools_cover(a: &Option<Vec<String>>, b: &Option<Vec<String>>) -> bool {
    match (a, b) {
        (None, _) => true,
        (Some(a), _) if a.is_empty() => true,
        (Some(_), None) => false,
        (Some(_), Some(b)) if b.is_empty() => false,
        (Some(a), Some(b)) => b.iter().all(|t| a.contains(t)),
    }
}

fn scope_covers(a: ScopeSel, b: ScopeSel) -> bool {
    a == ScopeSel::Any || a == b
}

fn host_covers(a: &str, b: &str) -> bool {
    if a.eq_ignore_ascii_case(b) {
        return true;
    }
    if b.contains('*') || a.contains(':') {
        return false;
    }
    let bare = b.rsplit_once(':').map_or(b, |(h, _)| h);
    HostPattern::lenient(a).matches(bare, 0)
}

fn path_covers(a: &Option<String>, b: &Option<String>) -> bool {
    match (a, b) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(a), Some(b)) => {
            a == b
                || a.strip_suffix("**")
                    .is_some_and(|prefix| b.starts_with(prefix))
        }
    }
}

fn methods_cover(a: &Option<Vec<String>>, b: &Option<Vec<String>>) -> bool {
    match (a, b) {
        (None, _) => true,
        (Some(a), _) if a.is_empty() => true,
        (Some(_), None) => false,
        (Some(_), Some(b)) if b.is_empty() => false,
        (Some(a), Some(b)) => b.iter().all(|m| a.contains(m)),
    }
}

fn target_covers(a: &Target, b: &Target) -> bool {
    use Target::*;
    if a == b {
        return true;
    }
    match (a, b) {
        (
            App {
                app_id: x,
                tools: t,
            },
            App {
                app_id: y,
                tools: u,
            },
        ) => x == y && tools_cover(t, u),
        (Connection { id: x, tools: t }, Connection { id: y, tools: u }) => {
            x == y && tools_cover(t, u)
        }
        (
            AppConnections {
                app_id: x,
                scope: s,
                tools: t,
            },
            AppConnections {
                app_id: y,
                scope: r,
                tools: u,
            },
        ) => x == y && scope_covers(*s, *r) && tools_cover(t, u),
        (Secrets { scope: s, kind: k }, Secrets { scope: r, kind: j }) => {
            scope_covers(*s, *r) && (k.is_none() || k == j)
        }
        (
            Network {
                host: h,
                path: p,
                methods: m,
            },
            Network {
                host: g,
                path: q,
                methods: n,
            },
        ) => host_covers(h, g) && path_covers(p, q) && methods_cover(m, n),
        _ => false,
    }
}

/// `a` decides every request `b` would match (first-match semantics).
fn rule_covers(a: &PolicyRule, b: &PolicyRule) -> bool {
    a.enabled
        && a.conditions.is_empty()
        && identities_cover(&a.identities, &b.identities)
        && (a.targets.is_empty()
            || (!b.targets.is_empty()
                && b.targets
                    .iter()
                    .all(|bt| a.targets.iter().any(|at| target_covers(at, bt)))))
}

pub fn shadowed(rules: &[PolicyRule]) -> Vec<PolicyWarning> {
    let mut out = Vec::new();
    for (i, r) in rules.iter().enumerate() {
        if !r.enabled {
            continue;
        }
        if let Some(by) = rules[..i].iter().find(|e| rule_covers(e, r)) {
            out.push(PolicyWarning {
                code: WarningCode::Shadowed,
                rule_logical_id: r.logical_id,
                message: format!(
                    "\"{}\" never applies: the earlier rule \"{}\" already decides every request it would match.",
                    r.name, by.name
                ),
                severity: None,
                details: None,
            });
        }
    }
    out
}

pub fn overridden(ws_rules: &[PolicyRule], org_rules: &[PolicyRule]) -> Vec<PolicyWarning> {
    ws_rules
        .iter()
        .filter(|r| r.enabled && r.action == Action::Allow)
        .filter_map(|r| {
            org_rules
                .iter()
                .find(|o| o.action == Action::Block && rule_covers(o, r))
                .map(|o| PolicyWarning {
                    code: WarningCode::OverriddenByOrg,
                    rule_logical_id: r.logical_id,
                    message: format!(
                        "\"{}\" is fully overridden by the organization block rule \"{}\".",
                        r.name, o.name
                    ),
                    severity: None,
                    details: None,
                })
        })
        .collect()
}

async fn inert(
    state: &AppState,
    s: Scope,
    rules: &[PolicyRule],
) -> Result<Vec<PolicyWarning>, ApiError> {
    let (mut secrets, mut conns) = (Vec::new(), Vec::new());
    for r in rules {
        for t in &r.targets {
            match t {
                Target::Secret { id } => secrets.push(*id),
                Target::Connection { id, .. } => conns.push(*id),
                _ => {}
            }
        }
    }
    let sec: HashMap<Uuid, Option<Uuid>> = repo::secret_scopes(&state.pool, s.org, &secrets)
        .await?
        .into_iter()
        .collect();
    let con: HashMap<Uuid, Option<Uuid>> = repo::connection_scopes(&state.pool, s.org, &conns)
        .await?
        .into_iter()
        .collect();
    let alive = |m: &HashMap<Uuid, Option<Uuid>>, id: &Uuid| match m.get(id) {
        None => false,
        Some(None) => true,
        Some(Some(w)) => s.ws == Some(*w),
    };
    let mut out = Vec::new();
    for r in rules.iter().filter(|r| r.enabled) {
        let gone = r.targets.iter().any(|t| match t {
            Target::Secret { id } => !alive(&sec, id),
            Target::Connection { id, .. } => !alive(&con, id),
            _ => false,
        });
        if gone {
            out.push(PolicyWarning {
                code: WarningCode::Inert,
                rule_logical_id: r.logical_id,
                message: format!(
                    "\"{}\" names a credential that no longer exists in this scope and matches nothing.",
                    r.name
                ),
                severity: None,
                details: None,
            });
        }
    }
    Ok(out)
}

/// (phase 2, RFE-025) `rule_applies_to_all_agents` (07 §Injection selection): every enabled
/// hand-written allow rule with empty identities that names credentials arms every agent of its
/// scope, including agents created later. `byo_enabled` adds "and by members creating BYO
/// agents" (the scope's effective BYO posture is `enabled`).
pub fn applies_to_all_agents(rules: &[PolicyRule], byo_enabled: bool) -> Vec<PolicyWarning> {
    rules
        .iter()
        .filter(|r| r.enabled && r.action == Action::Allow && r.identities.is_empty())
        .filter_map(|r| {
            let targets: Vec<&Target> = r
                .targets
                .iter()
                .filter(|t| {
                    matches!(
                        t,
                        Target::Connection { .. }
                            | Target::AppConnections { .. }
                            | Target::Secret { .. }
                            | Target::Secrets { .. }
                    )
                })
                .collect();
            if targets.is_empty() {
                return None;
            }
            let and_byo = if byo_enabled {
                " and by members creating BYO agents"
            } else {
                ""
            };
            Some(PolicyWarning {
                code: WarningCode::RuleAppliesToAllAgents,
                rule_logical_id: r.logical_id,
                message: format!(
                    "\"{}\" applies to all agents, including agents created later{and_byo}.",
                    r.name
                ),
                severity: Some("warning".into()),
                details: Some(serde_json::json!({
                    "rule_id": r.logical_id,
                    "targets": targets,
                })),
            })
        })
        .collect()
}

/// (phase 2, RFE-005) `user_rule_needs_speaker` for every enabled rule naming a `user`, and
/// `user_rule_no_agent_fallback` for allow rules that grant credentials and name only users
/// (07 §Authoring). `name_of` resolves a user id to a display name.
pub fn user_rules(rules: &[PolicyRule], name_of: &HashMap<Uuid, String>) -> Vec<PolicyWarning> {
    let mut out = Vec::new();
    for r in rules.iter().filter(|r| r.enabled) {
        let users: Vec<Uuid> = r
            .identities
            .iter()
            .filter_map(|i| match i {
                Identity::User { id } => Some(*id),
                Identity::Agent { .. } | Identity::Group { .. } => None,
            })
            .collect();
        if users.is_empty() {
            continue;
        }
        let who = users
            .iter()
            .map(|u| name_of.get(u).map_or("a removed user", String::as_str))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(PolicyWarning {
            code: WarningCode::UserRuleNeedsSpeaker,
            rule_logical_id: r.logical_id,
            message: format!(
                "\"{}\" matches only turns spoken by {who}; never schedules, webhooks, BYO or boot traffic.",
                r.name
            ),
            severity: None,
            details: None,
        });
        let grants_credentials = r.action == Action::Allow
            && r.targets.iter().any(|t| {
                matches!(
                    t,
                    Target::Connection { .. }
                        | Target::AppConnections { .. }
                        | Target::Secret { .. }
                        | Target::Secrets { .. }
                )
            });
        let only_users = r.identities.len() == users.len();
        if grants_credentials && only_users {
            out.push(PolicyWarning {
                code: WarningCode::UserRuleNoAgentFallback,
                rule_logical_id: r.logical_id,
                message: format!(
                    "\"{}\" grants credentials only while {who} is speaking; schedules, webhooks, BYO and boot traffic get none from it. Add an agent-level rule if they need access.",
                    r.name
                ),
                severity: Some("warning".into()),
                details: None,
            });
        }
    }
    out
}

/// (phase 4, P4-GRP-1, D-131) Warnings of rules with `group` identities. `groups` holds the
/// groups of the scope's org that still exist (name, member count); a named id missing from it
/// is a removed group (or another org's): the rule matches nobody for that identity.
pub fn group_rules(
    rules: &[PolicyRule],
    groups: &HashMap<Uuid, (String, i64)>,
) -> Vec<PolicyWarning> {
    let mut out = Vec::new();
    for r in rules.iter().filter(|r| r.enabled) {
        let named: Vec<Uuid> = r
            .identities
            .iter()
            .filter_map(|i| match i {
                Identity::Group { id } => Some(*id),
                _ => None,
            })
            .collect();
        if named.is_empty() {
            continue;
        }
        let mut live = Vec::new();
        for id in &named {
            match groups.get(id) {
                Some((name, members)) => {
                    live.push(name.clone());
                    if *members == 0 {
                        out.push(PolicyWarning {
                            code: WarningCode::GroupRuleEmpty,
                            rule_logical_id: r.logical_id,
                            message: format!("\"{}\": the group {name} has no members.", r.name),
                            severity: Some("info".into()),
                            details: Some(serde_json::json!({ "group_id": id })),
                        });
                    }
                }
                None => out.push(PolicyWarning {
                    code: WarningCode::RuleNamesRemovedGroup,
                    rule_logical_id: r.logical_id,
                    message: format!(
                        "\"{}\" names a group that no longer exists and matches nobody.",
                        r.name
                    ),
                    severity: Some("warning".into()),
                    details: Some(serde_json::json!({ "group_id": id })),
                }),
            }
        }
        if live.is_empty() {
            continue;
        }
        let who = live.join(", ");
        out.push(PolicyWarning {
            code: WarningCode::GroupRuleNeedsSpeaker,
            rule_logical_id: r.logical_id,
            message: format!(
                "\"{}\" matches only turns spoken by a member of {who}; never schedules, webhooks, BYO or boot traffic.",
                r.name
            ),
            severity: None,
            details: None,
        });
        let grants_credentials = r.action == Action::Allow
            && r.targets.iter().any(|t| {
                matches!(
                    t,
                    Target::Connection { .. }
                        | Target::AppConnections { .. }
                        | Target::Secret { .. }
                        | Target::Secrets { .. }
                )
            });
        if grants_credentials && r.identities.len() == named.len() {
            out.push(PolicyWarning {
                code: WarningCode::GroupRuleNoAgentFallback,
                rule_logical_id: r.logical_id,
                message: format!(
                    "\"{}\" grants credentials only while a member of {who} is speaking; schedules, webhooks, BYO and boot traffic get none from it. Add an agent-level rule if they need access.",
                    r.name
                ),
                severity: Some("warning".into()),
                details: None,
            });
        }
    }
    out
}

/// Whether the scope's effective BYO posture is `enabled` (members may create BYO agents).
async fn byo_enabled(state: &AppState, s: Scope) -> Result<bool, ApiError> {
    use aurora_llm::governance::ByoPosture;
    let org = crate::domains::agents::byo::org_byo(state, s.org).await?;
    let ws = match s.ws {
        Some(w) => aurora_db::repo::workspaces::get(&state.pool, w)
            .await?
            .and_then(|w| w.byo_posture),
        None => None,
    };
    let eff = crate::domains::agents::byo::effective(state, &org, ws.as_deref(), None);
    Ok(eff.posture == ByoPosture::Enabled)
}

async fn rules_for(state: &AppState, s: Scope, status: &str) -> Result<Vec<PolicyRule>, ApiError> {
    let Some(g) = repo::generation_by_status(&state.pool, s.org, s.ws, status).await? else {
        return Ok(Vec::new());
    };
    repo::rules_of(&state.pool, g.id)
        .await?
        .into_iter()
        .map(rule_dto)
        .collect()
}

pub async fn compute(state: &AppState, s: Scope) -> Result<PolicyWarnings, ApiError> {
    let rules = rules_for(state, s, "draft").await?;
    let mut items = shadowed(&rules);
    items.extend(inert(state, s, &rules).await?);
    items.extend(applies_to_all_agents(&rules, byo_enabled(state, s).await?));
    let user_ids: Vec<Uuid> = rules
        .iter()
        .flat_map(|r| &r.identities)
        .filter_map(|i| match i {
            Identity::User { id } => Some(*id),
            Identity::Agent { .. } | Identity::Group { .. } => None,
        })
        .collect();
    let group_ids: Vec<Uuid> = rules
        .iter()
        .flat_map(|r| &r.identities)
        .filter_map(|i| match i {
            Identity::Group { id } => Some(*id),
            _ => None,
        })
        .collect();
    let groups: HashMap<Uuid, (String, i64)> =
        aurora_db::repo::groups::names(&state.pool, &group_ids)
            .await?
            .into_iter()
            .filter(|(_, _, org, _)| *org == s.org)
            .map(|(id, name, _, n)| (id, (name, n)))
            .collect();
    items.extend(group_rules(&rules, &groups));
    let mut names = HashMap::new();
    for id in user_ids {
        if let std::collections::hash_map::Entry::Vacant(e) = names.entry(id)
            && let Some(u) = aurora_db::repo::users::find_by_id(&state.pool, id).await?
        {
            e.insert(u.display_name);
        }
    }
    items.extend(user_rules(&rules, &names));
    if s.ws.is_some() {
        let org = rules_for(
            state,
            Scope {
                org: s.org,
                ws: None,
            },
            "published",
        )
        .await?;
        items.extend(overridden(&rules, &org));
    }
    Ok(PolicyWarnings { items })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str, action: Action, host: &str) -> PolicyRule {
        PolicyRule {
            logical_id: Uuid::now_v7(),
            priority: 0,
            name: name.into(),
            description: None,
            enabled: true,
            identities: vec![],
            targets: vec![Target::Network {
                host: host.into(),
                path: None,
                methods: None,
            }],
            action,
            require_approval: false,
            approval_timeout_secs: None,
            rate_limit: None,
            conditions: vec![],
        }
    }

    #[test]
    fn wildcard_shadows_exact() {
        let w = shadowed(&[
            rule("all", Action::Allow, "*.example.com"),
            rule("one", Action::Block, "api.example.com"),
        ]);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].code, WarningCode::Shadowed);
        assert!(
            shadowed(&[
                rule("one", Action::Block, "api.example.com"),
                rule("all", Action::Allow, "*.example.com"),
            ])
            .is_empty()
        );
    }
}
