//! Authoring validation (07 §Authoring): shape/limits first, then scope membership.
//! Every failure is a field path under `details.fields`; the code is `policy_invalid`.
use super::dto::*;
use crate::app::AppState;
use aurora_core::{ApiError, FieldError};
use aurora_db::repo::policy::{self as repo, NewRule};
use aurora_policy::{HostPattern, PathPattern};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub const MAX_RULES: usize = 500;
const MAX_NAME: usize = 120;
const MAX_DESCRIPTION: usize = 1000;
const MAX_LIST: usize = 100;
const MAX_TOOLS: usize = 200;
const MAX_CONDITIONS: usize = 20;
const MAX_CONDITION_VALUE: usize = 1024;
const MAX_RATE: u32 = 1_000_000;
const MAX_ALLOWLIST: usize = 500;

#[derive(Debug, Clone, Copy)]
pub struct Scope {
    pub org: Uuid,
    pub ws: Option<Uuid>,
}

impl Scope {
    pub fn name(self) -> ScopeName {
        if self.ws.is_some() {
            ScopeName::Workspace
        } else {
            ScopeName::Organization
        }
    }
}

/// The validated, storable form of a draft.
pub struct Checked {
    pub default_action: &'static str,
    pub uncredentialed_default: &'static str,
    pub allowlist: serde_json::Value,
    pub rules: Vec<NewRule>,
}

pub fn invalid(fields: Vec<FieldError>) -> ApiError {
    ApiError::unprocessable("policy_invalid", "The policy is not valid.")
        .with_details(json!({ "fields": fields }))
}

struct Errs(Vec<FieldError>);

impl Errs {
    fn add(&mut self, path: String, msg: &str) {
        self.0.push(FieldError::new(path, msg));
    }
}

fn check_tools(e: &mut Errs, p: &str, tools: &Option<Vec<String>>) {
    if let Some(t) = tools {
        if t.len() > MAX_TOOLS {
            e.add(format!("{p}/tools"), "too many tools");
        }
        for (k, x) in t.iter().enumerate() {
            if x.is_empty() || x.len() > 128 {
                e.add(format!("{p}/tools/{k}"), "tool id must be 1-128 characters");
            }
        }
    }
}

fn check_app(e: &mut Errs, p: &str, app: &str) {
    if app.is_empty() || app.len() > 128 {
        e.add(format!("{p}/app_id"), "app id must be 1-128 characters");
    }
}

fn check_host_path(e: &mut Errs, p: &str, host: &str, path: &Option<String>) {
    if let Err(err) = HostPattern::parse(host) {
        e.add(format!("{p}/host"), &format!("invalid host pattern: {err}"));
    }
    if let Some(path) = path
        && let Err(err) = PathPattern::parse(path)
    {
        e.add(format!("{p}/path"), &format!("invalid path pattern: {err}"));
    }
}

fn normalize_methods(e: &mut Errs, p: &str, methods: &mut Option<Vec<String>>) {
    let Some(ms) = methods else { return };
    if ms.len() > 20 {
        e.add(format!("{p}/methods"), "too many methods");
    }
    for (k, m) in ms.iter_mut().enumerate() {
        if m.is_empty() || m.len() > 16 || !m.bytes().all(|b| b.is_ascii_alphabetic()) {
            e.add(format!("{p}/methods/{k}"), "invalid HTTP method");
        } else {
            m.make_ascii_uppercase();
        }
    }
}

/// Everything that can be checked without the database.
fn check_shape(req: &mut DraftRequest, scope: Scope, e: &mut Errs) {
    if req.rules.len() > MAX_RULES {
        e.add("/rules".into(), "too many rules");
    }
    let mut seen = HashSet::new();
    for (i, r) in req.rules.iter_mut().enumerate() {
        let rp = format!("/rules/{i}");
        let name = r.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME {
            e.add(format!("{rp}/name"), "name must be 1-120 characters");
        }
        r.name = name.to_owned();
        if r.description
            .as_ref()
            .is_some_and(|d| d.chars().count() > MAX_DESCRIPTION)
        {
            e.add(format!("{rp}/description"), "description too long");
        }
        if let Some(id) = r.logical_id
            && !seen.insert(id)
        {
            e.add(format!("{rp}/logical_id"), "duplicate logical_id");
        }
        if r.identities.len() > MAX_LIST {
            e.add(format!("{rp}/identities"), "too many identities");
        }
        if r.targets.len() > MAX_LIST {
            e.add(format!("{rp}/targets"), "too many targets");
        }
        for (j, t) in r.targets.iter_mut().enumerate() {
            let tp = format!("{rp}/targets/{j}");
            match t {
                Target::App { app_id, tools } | Target::AppConnections { app_id, tools, .. } => {
                    check_app(e, &tp, app_id);
                    check_tools(e, &tp, tools);
                }
                Target::Connection { tools, .. } => check_tools(e, &tp, tools),
                Target::Secret { .. } => {}
                Target::Secrets { .. } => {}
                Target::Network {
                    host,
                    path,
                    methods,
                } => {
                    check_host_path(e, &tp, host, path);
                    normalize_methods(e, &tp, methods);
                }
            }
            let ws_sel = matches!(
                t,
                Target::AppConnections {
                    scope: ScopeSel::Workspace,
                    ..
                } | Target::Secrets {
                    scope: ScopeSel::Workspace,
                    ..
                }
            );
            if scope.ws.is_none() && ws_sel {
                e.add(
                    format!("{tp}/scope"),
                    "workspace-scoped credentials cannot be targeted from organization policy",
                );
            }
        }
        if r.action == Action::Block {
            if r.require_approval {
                e.add(
                    format!("{rp}/require_approval"),
                    "block rules take no modifiers",
                );
            }
            if r.rate_limit.is_some() {
                e.add(format!("{rp}/rate_limit"), "block rules take no modifiers");
            }
        }
        if let Some(rl) = &r.rate_limit
            && !(1..=MAX_RATE).contains(&rl.count)
        {
            e.add(format!("{rp}/rate_limit/count"), "count must be 1-1000000");
        }
        if r.conditions.len() > MAX_CONDITIONS {
            e.add(format!("{rp}/conditions"), "too many conditions");
        }
        for (k, c) in r.conditions.iter().enumerate() {
            if c.value.chars().count() > MAX_CONDITION_VALUE {
                e.add(format!("{rp}/conditions/{k}/value"), "value too long");
            }
            if let Some(ptr) = &c.json_pointer
                && (ptr.len() > 256 || !(ptr.is_empty() || ptr.starts_with('/')))
            {
                e.add(
                    format!("{rp}/conditions/{k}/json_pointer"),
                    "invalid JSON pointer",
                );
            }
        }
    }
    let allow = req.uncredentialed_allowlist.as_deref().unwrap_or(&[]);
    if allow.len() > MAX_ALLOWLIST {
        e.add("/uncredentialed_allowlist".into(), "too many entries");
    }
    for (i, a) in allow.iter().enumerate() {
        check_host_path(
            e,
            &format!("/uncredentialed_allowlist/{i}"),
            &a.host,
            &a.path,
        );
    }
    if scope.ws.is_none() {
        if req.uncredentialed_default == Some(UncredentialedDefault::Block) {
            e.add(
                "/uncredentialed_default".into(),
                "organization policy cannot set an uncredentialed default",
            );
        }
        if !allow.is_empty() {
            e.add(
                "/uncredentialed_allowlist".into(),
                "organization policy has no uncredentialed allowlist",
            );
        }
    }
}

/// Identities and credential targets must exist in the scope.
async fn check_scope(
    state: &AppState,
    req: &DraftRequest,
    scope: Scope,
    e: &mut Errs,
) -> Result<(), ApiError> {
    let (mut agents, mut users, mut secrets, mut conns) = (vec![], vec![], vec![], vec![]);
    for r in &req.rules {
        for i in &r.identities {
            match i {
                Identity::Agent { id } => agents.push(*id),
                Identity::User { id } => users.push(*id),
            }
        }
        for t in &r.targets {
            match t {
                Target::Secret { id } => secrets.push(*id),
                Target::Connection { id, .. } => conns.push(*id),
                _ => {}
            }
        }
    }
    let db = &state.pool;
    let agent_ok: HashSet<Uuid> = if agents.is_empty() {
        HashSet::new()
    } else {
        repo::agents_in_scope(db, scope.org, scope.ws, &agents)
            .await?
            .into_iter()
            .collect()
    };
    let user_ok: HashSet<Uuid> = if users.is_empty() {
        HashSet::new()
    } else {
        repo::users_in_scope(db, scope.org, scope.ws, &users)
            .await?
            .into_iter()
            .collect()
    };
    let cred_ok = |rows: Vec<(Uuid, Option<Uuid>)>| -> HashMap<Uuid, Option<Uuid>> {
        rows.into_iter().collect()
    };
    let secret_ok = if secrets.is_empty() {
        HashMap::new()
    } else {
        cred_ok(repo::secret_scopes(db, scope.org, &secrets).await?)
    };
    let conn_ok = if conns.is_empty() {
        HashMap::new()
    } else {
        cred_ok(repo::connection_scopes(db, scope.org, &conns).await?)
    };
    // A credential is in scope when it is org-level (workspace NULL; workspace policy may
    // name them too, 07 §Grants) or lives in this very workspace. Organization policy may
    // only name org-level credentials.
    let in_scope = |m: &HashMap<Uuid, Option<Uuid>>, id: &Uuid| match m.get(id) {
        None => false,
        Some(None) => true,
        Some(Some(w)) => scope.ws == Some(*w),
    };
    for (i, r) in req.rules.iter().enumerate() {
        for (j, idn) in r.identities.iter().enumerate() {
            let (ok, id) = match idn {
                Identity::Agent { id } => (agent_ok.contains(id), id),
                Identity::User { id } => (user_ok.contains(id), id),
            };
            if !ok {
                e.add(
                    format!("/rules/{i}/identities/{j}/id"),
                    &format!("{id} is not in this scope"),
                );
            }
        }
        for (j, t) in r.targets.iter().enumerate() {
            match t {
                Target::Secret { id } if !in_scope(&secret_ok, id) => e.add(
                    format!("/rules/{i}/targets/{j}/id"),
                    "secret is not available in this scope",
                ),
                Target::Connection { id, .. } if !in_scope(&conn_ok, id) => e.add(
                    format!("/rules/{i}/targets/{j}/id"),
                    "connection is not available in this scope",
                ),
                _ => {}
            }
        }
    }
    Ok(())
}

pub async fn check(
    state: &AppState,
    mut req: DraftRequest,
    scope: Scope,
) -> Result<Checked, ApiError> {
    let mut e = Errs(Vec::new());
    check_shape(&mut req, scope, &mut e);
    if e.0.is_empty() {
        check_scope(state, &req, scope, &mut e).await?;
    }
    if !e.0.is_empty() {
        return Err(invalid(e.0));
    }
    let mut rules = Vec::with_capacity(req.rules.len());
    for r in req.rules {
        rules.push(NewRule {
            logical_id: r.logical_id.unwrap_or_else(Uuid::now_v7),
            name: r.name,
            description: r.description,
            enabled: r.enabled,
            identities: to_json(&r.identities)?,
            targets: to_json(&r.targets)?,
            action: r.action.as_str().to_owned(),
            require_approval: r.require_approval,
            rate_limit: r.rate_limit.as_ref().map(to_json).transpose()?,
            conditions: to_json(&r.conditions)?,
        });
    }
    Ok(Checked {
        default_action: req.default_action.as_str(),
        uncredentialed_default: req
            .uncredentialed_default
            .unwrap_or(UncredentialedDefault::None)
            .as_str(),
        allowlist: to_json(&req.uncredentialed_allowlist.unwrap_or_default())?,
        rules,
    })
}

fn to_json<T: serde::Serialize>(v: &T) -> Result<serde_json::Value, ApiError> {
    serde_json::to_value(v).map_err(|e| {
        tracing::error!(error = %e, "serializing policy");
        ApiError::internal("Internal error.")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(rules: serde_json::Value) -> DraftRequest {
        serde_json::from_value(json!({"revision": 0, "default_action": "none", "rules": rules}))
            .unwrap()
    }

    #[test]
    fn shape_errors_carry_paths() {
        let mut r = req(json!([{
            "name": "x", "action": "block", "require_approval": true,
            "targets": [{"type": "network", "host": "a.*.com*", "path": "no-slash"}]
        }]));
        let mut e = Errs(vec![]);
        check_shape(
            &mut r,
            Scope {
                org: Uuid::nil(),
                ws: Some(Uuid::nil()),
            },
            &mut e,
        );
        let paths: Vec<_> = e.0.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"/rules/0/targets/0/host"));
        assert!(paths.contains(&"/rules/0/targets/0/path"));
        assert!(paths.contains(&"/rules/0/require_approval"));
    }

    #[test]
    fn stored_json_matches_policy_crate() {
        let r = req(json!([{
            "name": "x", "action": "allow",
            "identities": [{"kind": "agent", "id": Uuid::nil()}],
            "targets": [{"type": "secrets", "scope": "any", "kind": "llm"},
                        {"type": "network", "host": "a.com", "methods": ["GET"]}],
            "rate_limit": {"count": 3, "per": "hour"},
            "conditions": [{"on": "body", "op": "contains", "value": "x"}]
        }]));
        let rule = &r.rules[0];
        let v = json!({
            "name": rule.name, "action": "allow", "identities": rule.identities,
            "targets": rule.targets, "rate_limit": rule.rate_limit, "conditions": rule.conditions,
        });
        let parsed: aurora_policy::Rule = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.targets.len(), 2);
    }
}
