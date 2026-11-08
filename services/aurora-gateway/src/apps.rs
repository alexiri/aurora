//! App connections as credential candidates: identification of the serving app and tool
//! (docs/06 pipeline step 5), candidate type shared with generic secrets, selection
//! precedence (step 6) and the credential bundle the mechanisms render from.

use crate::mechanisms::template::Vars;
use crate::resolve::SecretCand;
use crate::snapshot::Loaded;
use aurora_core::ApiError;
use aurora_core::limits::CONNECTION_CHOICE_MAX;
use aurora_db::repo::gateway::AppConnectionRow;
use aurora_policy::facts::{Level, ToolTags};
use aurora_policy::graphql::{self, GraphqlKind as PolicyGql};
use aurora_policy::{Credential, PathPattern};
use aurora_registry::manifest::{GitKind, GraphqlKind, ToolItem};
use aurora_registry::snapshot::{AppEntry, HostEntry};
use aurora_secrets::SecretRef;
use http::StatusCode;
use serde_json::json;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use uuid::Uuid;

/// An active app connection serving a host.
#[derive(Debug, Clone)]
pub struct ConnCand {
    pub id: Uuid,
    pub org_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub app_id: String,
    pub label: String,
    pub scope: Level,
    pub vref: SecretRef,
    pub version: i64,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub metadata: serde_json::Value,
    pub tenant_host: Option<String>,
    /// The registered app instance this connection is bound to (D-74): it is injected only on
    /// that instance's exact host.
    pub instance_id: Option<Uuid>,
    /// What `{instance}` stands for: the host of `instance_id` (set by the resolver from the org's
    /// active instances), or, for a default-host connection of an instance-bound app, the app's
    /// `default_host` (D-86).
    pub instance_host: Option<String>,
    /// `status = reauth_required`: refused with reconnect guidance after the decision.
    pub reauth_required: bool,
}

impl ConnCand {
    pub fn from_row(r: AppConnectionRow, org_id: Uuid) -> Result<Self, String> {
        Ok(Self {
            id: r.id,
            org_id,
            workspace_id: r.workspace_id,
            app_id: r.app_id,
            label: r.label,
            scope: if r.workspace_id.is_some() {
                Level::Workspace
            } else {
                Level::Organization
            },
            vref: r
                .credentials_ref
                .parse()
                .map_err(|_| "bad credentials_ref".to_string())?,
            version: r.credentials_version,
            expires_at: r.credentials_expires_at,
            metadata: r.metadata,
            tenant_host: r.tenant_host,
            instance_id: r.instance_id,
            instance_host: None,
            reauth_required: r.status == "reauth_required",
        })
    }

    pub fn credential(&self) -> Credential {
        Credential::Connection {
            id: self.id,
            app_id: self.app_id.clone(),
            scope: self.scope,
        }
    }
}

/// A credential candidate: a generic secret or an app connection.
#[derive(Debug, Clone)]
pub enum Cand {
    Secret(Arc<SecretCand>),
    App(Arc<ConnCand>),
}

impl Cand {
    pub fn id(&self) -> Uuid {
        match self {
            Self::Secret(s) => s.id,
            Self::App(c) => c.id,
        }
    }
    pub fn credential(&self) -> Credential {
        match self {
            Self::Secret(s) => s.credential(),
            Self::App(c) => c.credential(),
        }
    }
    fn scope(&self) -> Level {
        match self {
            Self::Secret(s) => s.scope,
            Self::App(c) => c.scope,
        }
    }
    fn app_id(&self) -> Option<&str> {
        match self {
            Self::Secret(_) => None,
            Self::App(c) => Some(&c.app_id),
        }
    }
    fn label(&self) -> &str {
        match self {
            Self::Secret(s) => &s.name,
            Self::App(c) => &c.label,
        }
    }
    pub fn secret(&self) -> Option<&Arc<SecretCand>> {
        match self {
            Self::Secret(s) => Some(s),
            Self::App(_) => None,
        }
    }
}

pub fn choices(c: &[Cand]) -> serde_json::Value {
    c.iter()
        .take(CONNECTION_CHOICE_MAX as usize)
        .map(|s| {
            json!({
                "id": s.id(), "app_id": s.app_id(), "label": s.label(),
                "scope": if s.scope() == Level::Workspace { "workspace" } else { "organization" },
            })
        })
        .collect()
}

pub enum Pick {
    None,
    One(Cand),
    Refuse(ApiError),
}

/// Step 6: choose among the selected candidates. Precedence before ambiguity is
/// declared: the named id; app connection over generic secret; path-limited over
/// whole-host secret; workspace over organization.
pub fn pick(mut sel: Vec<Cand>, named: Option<&str>) -> Pick {
    if let Some(n) = named {
        let found = Uuid::parse_str(n)
            .ok()
            .and_then(|id| sel.iter().find(|c| c.id() == id).cloned());
        return match found {
            Some(c) => Pick::One(c),
            None => Pick::Refuse(ApiError::new(
                StatusCode::NOT_FOUND,
                "connection_not_found",
                "No granted credential with that id is available here.",
            )),
        };
    }
    if sel.iter().any(|c| matches!(c, Cand::App(_))) {
        sel.retain(|c| matches!(c, Cand::App(_)));
    }
    if sel
        .iter()
        .any(|c| c.secret().is_some_and(|s| s.path.is_some()))
    {
        sel.retain(|c| c.secret().is_some_and(|s| s.path.is_some()));
    }
    if sel.iter().any(|c| c.scope() == Level::Workspace) {
        sel.retain(|c| c.scope() == Level::Workspace);
    }
    match sel.len() {
        0 => Pick::None,
        1 => Pick::One(sel.remove(0)),
        _ => {
            let same_app = sel
                .iter()
                .all(|c| c.app_id().is_some() && c.app_id() == sel[0].app_id());
            let (code, message) = if same_app {
                (
                    "multiple_connections",
                    "Several connections of this app match this request. Choose one by sending the x-aurora-connection-id header with the id of one of the choices.",
                )
            } else {
                (
                    "multiple_providers",
                    "Several granted credentials match this request. Choose one by sending the x-aurora-connection-id header with the id of one of the choices.",
                )
            };
            Pick::Refuse(
                ApiError::new(StatusCode::CONFLICT, code, message).with_details(
                    json!({ "choices": choices(&sel), "header": "x-aurora-connection-id" }),
                ),
            )
        }
    }
}

/// The app serving the request (host entry of the snapshot, longest path prefix).
pub struct AppMatch<'a> {
    pub entry: &'a HostEntry,
    pub app: &'a AppEntry,
    /// The match comes from a registered app instance (the `{instance}` host entries), not from
    /// the static host index (D-74).
    pub instance: bool,
}

pub fn identify_app<'a>(l: &'a Loaded, host: &str, port: u16, path: &str) -> Option<AppMatch<'a>> {
    let entry = l.lookup(host, port, path)?;
    let app = l.snapshot.app(&entry.app_id)?;
    Some(AppMatch {
        entry,
        app,
        instance: false,
    })
}

/// The app serving a request whose host exactly equals a registered instance's host: the app's
/// `{instance}` host entry with the longest matching path prefix (D-74).
pub fn identify_instance_app<'a>(l: &'a Loaded, app_id: &str, path: &str) -> Option<AppMatch<'a>> {
    let app = l.snapshot.app(app_id)?;
    app.instances.as_ref()?;
    let entry = l
        .snapshot
        .instance_entries(app_id)
        .filter(|e| path.starts_with(e.path_prefix.as_deref().unwrap_or("/")))
        .max_by_key(|e| e.path_prefix.as_deref().map_or(0, str::len))?;
    Some(AppMatch {
        entry,
        app,
        instance: true,
    })
}

fn tools(app: &AppEntry) -> impl Iterator<Item = &ToolItem> {
    app.tools
        .iter()
        .flat_map(|t| [&t.read, &t.write])
        .flatten()
        .flat_map(|g| &g.items)
}

fn serves(
    app: &AppEntry,
    instance: bool,
    t: &ToolItem,
    host: &str,
    port: u16,
    path: &str,
    method: &str,
) -> bool {
    aurora_registry::index::tool_serves_host(app, t, host, port, instance)
        && PathPattern::lenient(&t.path).matches(path)
        && (t.methods.is_empty() || t.methods.iter().any(|m| m.eq_ignore_ascii_case(method)))
}

/// A GraphQL-tagged tool may match: the body prefix is needed to classify.
pub fn needs_body(m: &AppMatch<'_>, host: &str, port: u16, path: &str, method: &str) -> bool {
    tools(m.app)
        .any(|t| t.graphql.is_some() && serves(m.app, m.instance, t, host, port, path, method))
}

/// The catalogue tool for the request; GraphQL tools match on the fail-closed operation
/// kind, git tools on the smart-HTTP operation. Unmatched requests have `tool = None`.
#[allow(clippy::too_many_arguments)]
pub fn identify_tool<'a>(
    m: &AppMatch<'a>,
    host: &str,
    port: u16,
    path: &str,
    query: Option<&str>,
    method: &str,
    body_prefix: &[u8],
    truncated: bool,
) -> Option<&'a ToolItem> {
    identify_tool_in(
        m.app,
        m.instance,
        host,
        port,
        path,
        query,
        method,
        body_prefix,
        truncated,
    )
}

/// [`identify_tool`] for a known app without a host entry: the database lane resolves the app
/// from the connection and matches its tools on the synthetic `<id>.db.aurora.internal` host
/// (`instance` is `false` there: database apps are never instance-bound).
#[allow(clippy::too_many_arguments)]
pub fn identify_tool_in<'a>(
    app: &'a AppEntry,
    instance: bool,
    host: &str,
    port: u16,
    path: &str,
    query: Option<&str>,
    method: &str,
    body_prefix: &[u8],
    truncated: bool,
) -> Option<&'a ToolItem> {
    let gql = graphql::classify(body_prefix, truncated);
    let git = aurora_policy::git::classify(path, query);
    tools(app).find(|t| {
        serves(app, instance, t, host, port, path, method)
            && t.graphql.is_none_or(|k| {
                matches!(
                    (k, gql),
                    (GraphqlKind::Query, PolicyGql::Query)
                        | (GraphqlKind::Mutation, PolicyGql::Mutation)
                )
            })
            && t.git.is_none_or(|k| {
                matches!(
                    (k, git),
                    (GitKind::Fetch, Some(aurora_policy::GitOp::Fetch))
                        | (GitKind::Push, Some(aurora_policy::GitOp::Push))
                )
            })
    })
}

pub fn tool_tags(t: &ToolItem) -> ToolTags {
    ToolTags {
        graphql: t.graphql.is_some(),
        git: t.git.is_some(),
    }
}

/// Decrypted credentials of one connection plus what templates may reference.
pub struct Exec<'a> {
    creds: serde_json::Map<String, serde_json::Value>,
    optional: BTreeSet<&'a str>,
    metadata: &'a serde_json::Value,
    tenant_host: Option<&'a str>,
    allowed_context: &'a [String],
    context: HashMap<String, String>,
}

impl<'a> Exec<'a> {
    /// `None` when the stored credentials are not a JSON object.
    pub fn new(
        app: &'a AppEntry,
        allowed_context: &'a [String],
        conn: &'a ConnCand,
        credentials_json: &[u8],
        context: HashMap<String, String>,
    ) -> Option<Self> {
        let serde_json::Value::Object(creds) = serde_json::from_slice(credentials_json).ok()?
        else {
            return None;
        };
        Some(Self {
            creds,
            optional: app
                .credentials
                .iter()
                .filter(|c| c.optional)
                .map(|c| c.name.as_str())
                .collect(),
            metadata: &conn.metadata,
            tenant_host: conn.tenant_host.as_deref(),
            allowed_context,
            context,
        })
    }

    pub fn vars(&self) -> Vars<'_> {
        Vars {
            credentials: &self.creds,
            optional: &self.optional,
            metadata: self.metadata,
            tenant_host: self.tenant_host,
            allowed_context: self.allowed_context,
            context: &self.context,
        }
    }

    pub fn credential(&self, name: &str) -> Option<&str> {
        self.creds.get(name).and_then(|v| v.as_str())
    }
}
