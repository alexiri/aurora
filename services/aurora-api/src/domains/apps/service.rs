use super::assets;
use super::dto::*;
use crate::app::AppState;
use crate::domains::plugins::registry;
use crate::http::auth::Caller;
use aurora_core::ApiError;
use aurora_db::repo::{connections, gateway};
use aurora_registry::manifest::{self, Connect};
use aurora_registry::snapshot::{AppEntry, ClaimStatus, PackageEntry, Snapshot};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};
use uuid::Uuid;

/// Availability and connection counts for one workspace (open catalogue when `None`).
pub struct Avail {
    restricted: bool,
    allowed: HashSet<String>,
    counts: HashMap<String, i64>,
}

impl Avail {
    pub fn is_available(&self, app_id: &str) -> bool {
        !self.restricted || self.allowed.contains(app_id)
    }
    pub fn count(&self, app_id: &str) -> i64 {
        self.counts.get(app_id).copied().unwrap_or(0)
    }
}

pub async fn avail(
    state: &AppState,
    caller: &Caller,
    workspace: Option<Uuid>,
) -> Result<Avail, ApiError> {
    let Some(ws) = workspace else {
        return Ok(Avail {
            restricted: false,
            allowed: HashSet::new(),
            counts: HashMap::new(),
        });
    };
    let auth = caller.workspace(state, ws).await?;
    let (restricted, ids) = gateway::app_availability(&state.pool, auth.org_id, ws).await?;
    let counts = connections::counts_for_workspace(&state.pool, auth.org_id, ws)
        .await?
        .into_iter()
        .collect();
    Ok(Avail {
        restricted,
        allowed: ids.into_iter().collect(),
        counts,
    })
}

/// Availability of apps for `workspace` of `org` without a caller (server-side renders).
pub async fn availability_of(
    state: &AppState,
    org: Uuid,
    workspace: Uuid,
) -> Result<Avail, ApiError> {
    let (restricted, ids) = gateway::app_availability(&state.pool, org, workspace).await?;
    Ok(Avail {
        restricted,
        allowed: ids.into_iter().collect(),
        counts: HashMap::new(),
    })
}

/// The active snapshot, or an empty catalogue.
pub async fn snapshot(state: &AppState) -> Result<Option<Snapshot>, ApiError> {
    registry::active_typed(state).await
}

pub fn find_app<'a>(
    snap: &'a Snapshot,
    app_id: &str,
) -> Result<(&'a AppEntry, &'a PackageEntry), ApiError> {
    let app = snap
        .app(app_id)
        .ok_or_else(|| ApiError::not_found("App not found."))?;
    let pkg = snap
        .packages
        .iter()
        .find(|p| p.package_id == app.package_id)
        .ok_or_else(|| ApiError::not_found("App not found."))?;
    Ok((app, pkg))
}

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_owned())
}

pub async fn summary(state: &AppState, a: &AppEntry, p: &PackageEntry, av: &Avail) -> AppSummary {
    AppSummary {
        app_id: a.app_id.clone(),
        package_id: p.package_id.clone(),
        package_version: p.version.clone(),
        name: a.about.name.clone(),
        description: a.about.description.clone(),
        category: non_empty(&a.about.category),
        available: av.is_available(&a.app_id),
        connection_count: av.count(&a.app_id),
        has_dark_icon: assets::has_dark_icon(state, &p.digest).await,
        app_restrictions: a.grants.into(),
        serves_library: a.library.is_some(),
    }
}

fn s<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn field(f: &manifest::FormField) -> FormField {
    FormField {
        name: f.name.clone(),
        label: f.label.clone(),
        kind: s(&f.kind),
        store: s(&f.store),
        required: f.required,
        placeholder: f.placeholder.clone(),
        help_url: f.help_url.clone(),
        options: (!f.options.is_empty()).then(|| {
            f.options
                .iter()
                .map(|o| SelectOption {
                    value: o.value.clone(),
                    label: o.label.clone(),
                    warning: o.warning.clone(),
                })
                .collect()
        }),
        show_when: f.show_when.as_ref().map(|w| ShowWhen {
            field: w.field.clone(),
            equals: w.equals.clone(),
        }),
    }
}

fn method(c: &Connect) -> ConnectMethod {
    let empty = |id: &str, kind: &str, fields: Vec<FormField>| ConnectMethod {
        id: id.to_owned(),
        kind: kind.to_owned(),
        fields,
        scopes: Vec::new(),
        default_scopes: Vec::new(),
        access_levels: Vec::new(),
        client_key: None,
        callback: None,
    };
    match c {
        Connect::Fields(f) => empty(&f.id, "fields", f.fields.iter().map(field).collect()),
        Connect::Hook(h) => empty(&h.id, "hook", h.fields.iter().map(field).collect()),
        Connect::Oauth2(o) => ConnectMethod {
            scopes: o
                .scopes
                .iter()
                .map(|x| ScopeInfo {
                    scope: x.scope.clone(),
                    name: x.name.clone(),
                    access: s(&x.access),
                })
                .collect(),
            default_scopes: o.default_scopes.clone(),
            access_levels: o
                .access_levels
                .iter()
                .map(|l| AccessLevel {
                    id: l.id.clone(),
                    label: l.label.clone(),
                })
                .collect(),
            client_key: Some(o.client.clone()),
            callback: Some(s(&o.callback)),
            ..empty(&o.id, "oauth2", o.fields.iter().map(field).collect())
        },
    }
}

pub fn tools(a: &AppEntry) -> Vec<Tool> {
    let mut out = Vec::new();
    let Some(t) = &a.tools else { return out };
    for (group, g) in [("read", &t.read), ("write", &t.write)] {
        let Some(g) = g else { continue };
        if let Some(w) = &g.wildcard {
            let hosts: BTreeSet<&str> = g.items.iter().map(|i| i.host.as_str()).collect();
            let methods: BTreeSet<&str> = g
                .items
                .iter()
                .flat_map(|i| i.methods.iter().map(String::as_str))
                .collect();
            out.push(Tool {
                id: w.id.clone(),
                name: w.name.clone(),
                description: None,
                group: group.to_owned(),
                wildcard: true,
                host: if hosts.len() == 1 {
                    hosts.iter().next().map_or("*", |h| h).to_owned()
                } else {
                    "*".to_owned()
                },
                path: "/*".to_owned(),
                methods: methods.into_iter().map(str::to_owned).collect(),
                graphql: None,
                git: None,
            });
        }
        for i in &g.items {
            out.push(Tool {
                id: i.id.clone(),
                name: i.name.clone(),
                description: i.description.clone(),
                group: group.to_owned(),
                wildcard: false,
                host: i.host.clone(),
                path: i.path.clone(),
                methods: i.methods.clone(),
                graphql: i.graphql.as_ref().map(s),
                git: i.git.as_ref().map(s),
            });
        }
    }
    out
}

/// Package OAuth clients referenced by the app's oauth2 connect methods.
fn oauth_clients(
    state: &AppState,
    snap: &Snapshot,
    a: &AppEntry,
    p: &PackageEntry,
) -> Vec<AppOauthClient> {
    let keys: BTreeSet<&str> = a
        .connect
        .iter()
        .filter_map(|c| match c {
            Connect::Oauth2(o) => Some(o.client.as_str()),
            _ => None,
        })
        .collect();
    let redirect_uri = format!(
        "{}/v1/oauth/callback",
        state.config.external_url.trim_end_matches('/')
    );
    snap.oauth_clients
        .iter()
        .filter(|c| c.package_id == p.package_id && keys.contains(c.client_key.as_str()))
        .map(|c| AppOauthClient {
            client_key: c.client_key.clone(),
            fields: c
                .fields
                .iter()
                .map(|f| OauthClientField {
                    name: f.name.clone(),
                    label: f.label.clone(),
                    secret: f.secret,
                })
                .collect(),
            setup_note: c.setup_note.clone(),
            redirect_uri: redirect_uri.clone(),
        })
        .collect()
}

pub async fn detail(
    state: &AppState,
    snap: &Snapshot,
    a: &AppEntry,
    p: &PackageEntry,
    av: &Avail,
) -> AppDetail {
    let sm = summary(state, a, p, av).await;
    AppDetail {
        app_id: sm.app_id,
        package_id: sm.package_id,
        package_version: sm.package_version,
        name: sm.name,
        description: sm.description,
        category: sm.category,
        available: sm.available,
        connection_count: sm.connection_count,
        has_dark_icon: sm.has_dark_icon,
        app_restrictions: sm.app_restrictions,
        serves_library: sm.serves_library,
        api_docs: a.about.api_docs.clone(),
        connect_note: a.about.connect_note.as_ref().map(|n| ConnectNote {
            text: n.text.clone(),
            url: n.url.clone(),
        }),
        label_hint: a.about.label_hint.clone(),
        connect: a.connect.iter().map(method).collect(),
        tools: tools(a),
        hosts: snap
            .hosts
            .iter()
            .filter(|h| h.app_id == a.app_id)
            .map(|h| HostInfo {
                host: h.host.clone(),
                path_prefix: h.path_prefix.clone(),
                claim_status: match h.claim_status {
                    ClaimStatus::Active => "active",
                    ClaimStatus::Lost => "lost",
                }
                .to_owned(),
            })
            .collect(),
        agent_instructions: a.agent.as_ref().and_then(|g| non_empty(&g.instructions)),
        credentials: a
            .credentials
            .iter()
            .map(|c| CredentialInfo {
                name: c.name.clone(),
                optional: c.optional,
            })
            .collect(),
        metadata: a.metadata.iter().map(|m| m.name.clone()).collect(),
        oauth_clients: oauth_clients(state, snap, a, p),
        instances: a.instances.as_ref().map(|i| AppInstances {
            label: i.label.clone(),
            default_host: i.default_host.clone(),
        }),
    }
}
