use super::dto::{ConnectGrant, ConnectRequest, ConnectResponse};
use super::fields::{self, Parsed};
use super::flow::{self, Flow};
use super::hooks::{Call, Connected, Exchanged, kv};
use super::probe::{self, Vars};
use crate::app::AppState;
use crate::audit::{self, AuditEvent};
use crate::domains::apps::service::find_app;
use crate::domains::connections::dto::Connection;
use crate::domains::grants::dto::ConnectionGrant;
use crate::domains::grants::levels::{self, Level};
use crate::domains::grants::service as grants_service;
use crate::domains::plugins::registry;
use crate::domains::secrets::service::secret_err;
use crate::http::auth::{Caller, require_own};
use crate::http::who_can::{self, WhoCanRole};
use aurora_core::{ApiError, ids};
use aurora_db::DbError;
use aurora_db::repo::app_instances::{self, InstanceRow};
use aurora_db::repo::connections::{self as repo, ConnectionRow, NewConnection};
use aurora_db::repo::{credentials, derived_rules, gateway};
use aurora_registry::instance::InstanceBase;
use aurora_registry::manifest::{
    BodyFormat, ClientAuth, Connect, FieldsConnect, Oauth2Connect, UrlOrTemplate,
};
use aurora_registry::snapshot::{AppEntry, OauthClientEntry, PackageEntry, Snapshot};
use aurora_secrets::{Purpose, SealContext, SecretRef, ValueSource};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

// (P5-ICS, D-273) The secret-link apps' connect check: validate, fetch once, check the prefix.
mod link;
mod own_server;
mod webhook;

pub enum ConnectErr {
    Api(ApiError),
    Probe(probe::ProbeFail),
    Hook(String),
    Exchange,
}

impl From<ApiError> for ConnectErr {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}

impl From<DbError> for ConnectErr {
    fn from(e: DbError) -> Self {
        Self::Api(e.into())
    }
}

impl From<ConnectErr> for ApiError {
    fn from(e: ConnectErr) -> Self {
        match e {
            ConnectErr::Api(a) => a,
            // (C-958) The journey tells the person why: `rejected` (401/403), `unreachable`
            // (DNS, connect, TLS, timeout; with the host) or `unexpected` (the status is kept).
            ConnectErr::Probe(f) => {
                ApiError::unprocessable("connect_probe_failed", "The connection probe failed.")
                    .with_details(json!({
                        "upstream_status": f.status,
                        "cause": f.cause.as_str(),
                        "host": f.host,
                    }))
            }
            ConnectErr::Hook(m) => ApiError::unprocessable("connect_hook_failed", m),
            ConnectErr::Exchange => {
                ApiError::unprocessable("oauth_exchange_failed", "The token exchange failed.")
            }
        }
    }
}

impl ConnectErr {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Probe(_) => "connect_probe_failed",
            Self::Hook(_) => "connect_hook_failed",
            Self::Api(_) | Self::Exchange => "oauth_exchange_failed",
        }
    }
}

fn invalid(path: &str, msg: &str) -> ConnectErr {
    ConnectErr::Api(ApiError::invalid_request(
        "Invalid request.",
        vec![aurora_core::FieldError::new(path, msg)],
    ))
}

pub struct Started {
    pub response: ConnectResponse,
    pub cookie: Option<String>,
}

/// (P5-UX-2, C-957) What the access part of a connect did: the grants written, and, when the
/// callback could not apply the access, the code of why (the account itself is kept).
#[derive(Default)]
pub struct Applied {
    pub grants: Vec<ConnectionGrant>,
    pub granted_agent_ids: Vec<Uuid>,
    pub failed: Option<String>,
}

/// The access a connect gives, and how failure behaves: `soft` (the OAuth callback) keeps the
/// account and reports the failure; strict (a request that connects directly) fails the whole
/// request, so nothing is written.
struct Intent {
    grant: ConnectGrant,
    soft: bool,
}

pub enum Done {
    Connected(Uuid, Vec<String>, Landing, Applied),
    Redirect { url: String, cookie: String },
}

/// (FIX-149, C-842) What the callback page needs to tell the waiting dialog which flow finished
/// (`app_id`, already known to the flow) and where to send the person if no dialog answers
/// (the Connections tab of the flow's scope).
pub struct Landing {
    pub app_id: String,
    pub connections_path: Option<String>,
}

/// A failed callback: the fixed error code, plus the landing once the flow was opened and bound
/// to this user (an unbound or tampered flow reveals nothing, not even the app id).
pub struct Failure {
    pub code: &'static str,
    pub landing: Option<Landing>,
}

impl From<&'static str> for Failure {
    fn from(code: &'static str) -> Self {
        Self {
            code,
            landing: None,
        }
    }
}

async fn landing(state: &AppState, flow: &Flow) -> Landing {
    let slugs: Option<(String, Option<String>)> = match flow.workspace_id {
        Some(ws) => sqlx::query_as(
            "SELECT o.slug, w.slug FROM workspaces w JOIN organizations o ON o.id = w.org_id WHERE w.id = $1",
        )
        .bind(ws)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten(),
        None => sqlx::query_scalar::<_, String>("SELECT slug FROM organizations WHERE id = $1")
            .bind(flow.org_id)
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten()
            .map(|o| (o, None)),
    };
    let connections_path = slugs.map(|(org, ws)| match ws {
        Some(ws) => format!("/o/{org}/w/{ws}/credentials?tab=connections"),
        None => format!("/o/{org}/credentials?tab=connections"),
    });
    Landing {
        app_id: flow.app_id.clone(),
        connections_path,
    }
}

struct Target {
    org: Uuid,
    ws: Option<Uuid>,
}

async fn authorize_scope(
    state: &AppState,
    caller: &Caller,
    scope: &str,
    ws: Option<Uuid>,
    org: Option<Uuid>,
    app_id: &str,
) -> Result<Target, ApiError> {
    let t = match (scope, ws, org) {
        ("workspace", Some(ws), _) => {
            let a = caller.workspace(state, ws).await?;
            require_own(&a)?;
            Target {
                org: a.org_id,
                ws: Some(ws),
            }
        }
        ("organization", _, Some(org)) => {
            caller.org(state, org).await?.require_admin()?;
            Target { org, ws: None }
        }
        _ => {
            return Err(ApiError::invalid_request(
                "Invalid scope.",
                vec![aurora_core::FieldError::new(
                    "/scope",
                    "workspace needs workspace_id; organization needs org_id",
                )],
            ));
        }
    };
    let available = match t.ws {
        Some(ws) => {
            let (restricted, ids) = gateway::app_availability(&state.pool, t.org, ws).await?;
            !restricted || ids.iter().any(|i| i == app_id)
        }
        None => repo::org_app_available(&state.pool, t.org, app_id).await?,
    };
    if !available {
        return Err(ApiError::new(
            http::StatusCode::FORBIDDEN,
            "app_unavailable",
            "This app is not available here.",
        ));
    }
    Ok(t)
}

/// (FIX-154) Whether the org's posture makes `app_id` available to the workspace: the check
/// [`authorize_scope`] makes for a workspace connection.
pub async fn available_in_workspace(
    state: &AppState,
    org_id: Uuid,
    workspace_id: Uuid,
    app_id: &str,
) -> Result<bool, ApiError> {
    let (restricted, ids) = gateway::app_availability(&state.pool, org_id, workspace_id).await?;
    Ok(!restricted || ids.iter().any(|i| i == app_id))
}

fn redirect_uri(state: &AppState) -> String {
    format!(
        "{}/v1/oauth/callback",
        state.config.external_url.trim_end_matches('/')
    )
}

fn client_ctx(c: &aurora_db::repo::oauth_clients::ClientRow) -> SealContext {
    SealContext {
        purpose: Purpose::OauthClientSecret,
        owner_id: c.id,
        org_id: c.org_id.unwrap_or_else(Uuid::nil),
        workspace_id: c.workspace_id,
    }
}

type ClientRow = aurora_db::repo::oauth_clients::ClientRow;

fn client_required(org: Uuid, msg: &str) -> ConnectErr {
    ConnectErr::Api(
        ApiError::unprocessable("oauth_client_required", msg.to_owned())
            // (C-958) Who can register the client: the organization's administrators.
            .with_details(who_can::stub(WhoCanRole::OrgAdmin, Some(org), None)),
    )
}

/// OAuth methods of a self-hosted instance use only a BYOC client bound to that instance (C-243).
fn instance_client_required(org: Uuid) -> ConnectErr {
    ConnectErr::Api(
        ApiError::unprocessable(
            "instance_oauth_client_required",
            "Register an OAuth application for this instance first.",
        )
        .with_details(who_can::stub(WhoCanRole::OrgAdmin, Some(org), None)),
    )
}

async fn resolve_client(
    state: &AppState,
    pkg: &str,
    key: &str,
    t: &Target,
    requested: Option<Uuid>,
    instance: Option<Uuid>,
) -> Result<ClientRow, ConnectErr> {
    let all = repo::usable_clients(&state.pool, pkg, key, t.org, t.ws, instance).await?;
    let found = match requested {
        Some(id) => all.into_iter().find(|c| c.id == id),
        None => all.into_iter().next(),
    };
    found.ok_or_else(|| {
        if instance.is_some() {
            instance_client_required(t.org)
        } else {
            client_required(t.org, "No usable OAuth client is configured.")
        }
    })
}

async fn client_fields(
    state: &AppState,
    c: &ClientRow,
) -> Result<(Map<String, Value>, Map<String, Value>), ConnectErr> {
    let public = c.fields.as_object().cloned().unwrap_or_default();
    let r: SecretRef = c.secret_ref.parse().map_err(|_| ConnectErr::Exchange)?;
    let plain = state
        .vault
        .open(&client_ctx(c), &r)
        .await
        .map_err(secret_err)?;
    let secret: Map<String, Value> = serde_json::from_slice(plain.expose()).unwrap_or_default();
    Ok((public, secret))
}

async fn context(
    state: &AppState,
    pkg: &PackageEntry,
    org: Uuid,
    ws: Option<Uuid>,
) -> Result<Map<String, Value>, ConnectErr> {
    let mut m = Map::new();
    for name in &pkg.context {
        let v = match name.as_str() {
            "org_external_id" => repo::org_external_id(&state.pool, org).await?,
            "org_id" => Some(org.to_string()),
            "workspace_id" => ws.map(|w| w.to_string()),
            "instance_url" => Some(state.config.external_url.clone()),
            _ => None,
        };
        if let Some(v) = v {
            m.insert(name.clone(), Value::String(v));
        }
    }
    Ok(m)
}

/// `https` and on a declared host (the registered instance's host included) or an authorize host
/// of the package's clients.
fn redirect_allowed(
    snap: &Snapshot,
    app: &AppEntry,
    pkg: &PackageEntry,
    url: &str,
    instance: Option<InstanceBase<'_>>,
) -> bool {
    if probe::declared_host(snap, app, url, instance) {
        return true;
    }
    let Ok(u) = reqwest::Url::parse(url) else {
        return false;
    };
    u.scheme() == "https"
        && snap
            .oauth_clients
            .iter()
            .filter(|c| c.package_id == pkg.package_id)
            .any(|c| match &c.authorize_url {
                UrlOrTemplate::Url(a) => {
                    reqwest::Url::parse(a).is_ok_and(|a| a.host_str() == u.host_str())
                }
                UrlOrTemplate::Templated(z) => probe::in_zone(url, &z.zone),
            })
}

struct Ctx<'a> {
    state: &'a AppState,
    snap: &'a Snapshot,
    app: &'a AppEntry,
    pkg: &'a PackageEntry,
    user: Uuid,
    t: Target,
    reconnect: Option<ConnectionRow>,
    /// The registered self-hosted instance being connected (D-74); `None` = `default_host` or an
    /// app without `instances`.
    instance: Option<InstanceRow>,
    /// (FIX-133, D-240) `read` | `write` requested for the connection (validated); `None` = `write`
    /// for a new connection, unchanged for a reconnect.
    access: Option<String>,
    /// (P5-UX-11, C-990) The limit of a new organization account.
    limit: Option<String>,
    /// (P5-UX-2, C-957) The access to write with the connection.
    grant: Option<Intent>,
}

#[derive(Default)]
struct Gathered {
    credentials: Map<String, Value>,
    metadata: Map<String, Value>,
    tenant_host: Option<String>,
    granted: Option<Vec<String>>,
    requested: Vec<String>,
    label: Option<String>,
    /// (FIX-194) The form's `store: label` field: wins over the probe's and the hook's label.
    form_label: Option<String>,
    expires_at: Option<i64>,
    fields: Map<String, Value>,
    client: Option<Uuid>,
    /// (P5-WEBHOOK-2) An outgoing webhook's configuration, stored beside the credential.
    webhook: Option<webhook::Gathered>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Fields,
    Oauth,
    Hook,
}

impl Ctx<'_> {
    /// (FIX-136) The declared read scopes whose probe a pasted token passes (write scopes are never
    /// probed; a probe that fails or errors just leaves its scope off).
    async fn probe_declared_scopes(&self, f: &FieldsConnect, g: &Gathered) -> Vec<String> {
        let mut out = Vec::new();
        for s in &f.scopes {
            let Some(p) = f.probe_scopes.get(&s.scope) else {
                continue;
            };
            let r = probe::run(
                &self.state.connect,
                self.snap,
                self.app,
                p,
                &Vars {
                    credentials: &g.credentials,
                    metadata: &g.metadata,
                    tenant_host: g.tenant_host.as_deref(),
                    fields: &g.fields,
                    instance_host: self.instance_host(),
                    instance_base_path: self.instance_base_path(),
                    instance_registered: self.instance.is_some(),
                },
            )
            .await;
            if r.is_ok() {
                out.push(s.scope.clone());
            }
        }
        out
    }

    /// (FIX-133, D-240) The access the connection will have: the chosen one, else a reconnect's
    /// current one, else `write`.
    fn effective_access(&self) -> &str {
        self.access
            .as_deref()
            .or_else(|| self.reconnect.as_ref().map(|r| r.access.as_str()))
            .unwrap_or("write")
    }

    /// The host `{instance}` stands for: the registered instance's, else the app's `default_host`.
    fn instance_host(&self) -> Option<&str> {
        match &self.instance {
            Some(i) => Some(i.host.as_str()),
            None => self
                .app
                .instances
                .as_ref()
                .and_then(|i| i.default_host.as_deref()),
        }
    }

    /// The registered instance's base path (D-231); a default-host connection has none.
    fn instance_base_path(&self) -> Option<&str> {
        self.instance.as_ref().and_then(|i| i.base_path.as_deref())
    }

    /// Where `{instance}` points: the registered instance's address, else the app's `default_host`.
    fn instance_base(&self) -> Option<InstanceBase<'_>> {
        self.instance_host()
            .map(|h| InstanceBase::new(h, self.instance_base_path()))
    }

    /// The registered instance's address (the host and path the `{instance}` host entries apply to).
    fn registered_base(&self) -> Option<InstanceBase<'_>> {
        self.instance
            .as_ref()
            .map(|i| InstanceBase::new(&i.host, i.base_path.as_deref()))
    }

    fn bad(&self, k: Kind) -> ConnectErr {
        match k {
            Kind::Fields => invalid("/fields", "values violate the app declaration"),
            Kind::Oauth => ConnectErr::Exchange,
            Kind::Hook => ConnectErr::Hook("hook output rejected".into()),
        }
    }

    fn call(&self) -> Call<'_> {
        Call {
            package_id: &self.pkg.package_id,
            package_digest: &self.pkg.digest,
            app_id: &self.app.app_id,
            instance_host: self.instance_host(),
            instance_base_path: self.instance_base_path(),
        }
    }

    fn new_flow(
        &self,
        method: &str,
        client: Option<Uuid>,
        level: Option<String>,
    ) -> Result<Flow, ApiError> {
        Ok(Flow {
            nonce: flow::random(16)?,
            app_id: self.app.app_id.clone(),
            method_id: method.to_owned(),
            scope: if self.t.ws.is_some() {
                "workspace"
            } else {
                "organization"
            }
            .into(),
            workspace_id: self.t.ws,
            org_id: self.t.org,
            user_id: self.user,
            client_id: client,
            instance_id: self.instance.as_ref().map(|i| i.id),
            access_level: level,
            access: self.access.clone(),
            limit: self.limit.clone(),
            grant: self.grant.as_ref().map(|g| g.grant.clone()),
            pkce_verifier: None,
            reconnect: self.reconnect.as_ref().map(|r| r.id),
            continuation: None,
            scopes: Vec::new(),
            tenant_host: None,
            metadata: Map::new(),
            exp: Flow::expiry(),
        })
    }

    /// Validate, probe, persist.
    async fn finalize(
        &self,
        caller: &Caller,
        method: &Connect,
        kind: Kind,
        g: Gathered,
    ) -> Result<(ConnectionRow, Vec<String>, Applied), ConnectErr> {
        // (P5-WEBHOOK-2, C-909) The core adds an outgoing webhook's secret values and signing secret
        // to the sealed credential beside the URL.
        let webhook = g.webhook.is_some();
        let declared = |n: &str| {
            self.app.credentials.iter().any(|c| c.name == n)
                || (webhook && matches!(n, "secret_values" | "signing_secret"))
        };
        if g.credentials.keys().any(|k| !declared(k))
            || g.metadata
                .keys()
                .any(|k| !self.app.metadata.iter().any(|m| &m.name == k))
        {
            return Err(self.bad(kind));
        }
        if let Some(h) = &g.tenant_host
            && !probe::tenant_ok(self.snap, self.app, h)
        {
            return Err(self.bad(kind));
        }
        let mut metadata = g.metadata.clone();
        let mut label = g.label.as_deref().and_then(probe::clean_label);
        let pr = match method {
            Connect::Oauth2(o) => o.probe.as_ref(),
            Connect::Fields(f) => f.probe.as_ref(),
            Connect::Hook(_) => None,
        };
        if let Some(p) = pr {
            let out = probe::run(
                &self.state.connect,
                self.snap,
                self.app,
                p,
                &Vars {
                    credentials: &g.credentials,
                    metadata: &g.metadata,
                    tenant_host: g.tenant_host.as_deref(),
                    fields: &g.fields,
                    instance_host: self.instance_host(),
                    instance_base_path: self.instance_base_path(),
                    instance_registered: self.instance.is_some(),
                },
            )
            .await
            .map_err(ConnectErr::Probe)?;
            metadata.extend(out.metadata);
            label = out.label.or(label);
        }
        let label = g
            .form_label
            .clone()
            .or(label)
            .or_else(|| self.reconnect.as_ref().map(|r| r.label.clone()))
            .unwrap_or_else(|| self.app.about.name.clone());
        let granted = g.granted.clone().unwrap_or_else(|| g.requested.clone());
        let missing: Vec<String> = g
            .requested
            .iter()
            .filter(|s| !granted.contains(s))
            .cloned()
            .collect();
        let (row, applied) = self
            .persist(
                caller,
                &g,
                &label,
                &Value::Object(metadata),
                &granted,
                method.id(),
            )
            .await?;
        Ok((row, missing, applied))
    }

    async fn persist(
        &self,
        caller: &Caller,
        g: &Gathered,
        label: &str,
        metadata: &Value,
        scopes: &[String],
        method_id: &str,
    ) -> Result<(ConnectionRow, Applied), ConnectErr> {
        let state = self.state;
        let id = self.reconnect.as_ref().map_or_else(ids::new_id, |r| r.id);
        let sctx = SealContext {
            purpose: Purpose::ConnectionCredentials,
            owner_id: id,
            org_id: self.t.org,
            workspace_id: self.t.ws,
        };
        let bytes = serde_json::to_vec(&g.credentials)
            .map_err(|_| ApiError::internal("Internal error."))?;
        let sref = state
            .vault
            .seal(&sctx, aurora_core::Secret::new(bytes))
            .await
            .map_err(secret_err)?;
        let col = sref.to_column();
        // Names only, never values (RFE-022): the plugin major-upgrade check reads these.
        let mut keys: Vec<String> = g.credentials.keys().cloned().collect();
        keys.sort_unstable();
        let expires: Option<DateTime<Utc>> =
            g.expires_at.and_then(|s| DateTime::from_timestamp(s, 0));
        let n = NewConnection {
            id,
            org_id: self.t.org,
            workspace_id: self.t.ws,
            app_id: &self.app.app_id,
            package_id: &self.pkg.package_id,
            app_version: &self.pkg.version,
            label,
            credentials_ref: &col,
            credentials_expires_at: expires,
            scopes,
            requested_scopes: &g.requested,
            metadata,
            tenant_host: g.tenant_host.as_deref(),
            oauth_client_id: g.client,
            connected_by: self.user,
            instance_id: self.instance.as_ref().map(|i| i.id),
            credential_keys: &keys,
            connect_method: Some(method_id),
            access: self.access.as_deref(),
            limit_level: self.limit.as_deref(),
            config: g.webhook.as_ref().and_then(|w| w.config_json.as_ref()),
            replace_label: g.form_label.is_some(),
        };
        let (old, applied) = match self.write(caller, &n).await {
            Ok(o) => o,
            Err(e) => {
                discard(state, &sctx, &sref).await;
                return Err(e);
            }
        };
        if let Some(r) = old.and_then(|o| o.parse::<SecretRef>().ok()) {
            discard(state, &sctx, &r).await;
        }
        let row = repo::get(&state.pool, id)
            .await?
            .ok_or_else(|| ApiError::internal("Internal error."))?;
        Ok((row, applied))
    }

    /// Insert or CAS-update inside one transaction (with its `connection.create` /
    /// `connection.reconnect` audit row, RFE-008, and, with a `grant`, the access, SEC-130);
    /// returns the replaced credentials ref and what the access part did.
    async fn write(
        &self,
        caller: &Caller,
        n: &NewConnection<'_>,
    ) -> Result<(Option<String>, Applied), ConnectErr> {
        let mut tx = audit::begin(self.state).await?;
        let extra = self.grant.as_ref().map(|g| {
            json!({"via": "connect_journey", "agent_ids": g.grant.agent_ids, "level": g.grant.level})
        });
        let (old, ev) = self.write_in_tx(&mut tx, caller, n, extra).await?;
        let mut events = vec![ev];
        let applied = match &self.grant {
            Some(g) => {
                self.apply_grant(&mut tx, caller, n.id, g, &mut events)
                    .await?
            }
            None => Applied::default(),
        };
        audit::commit_critical_many(tx, &events).await?;
        Ok((old, applied))
    }

    /// The access of a connect, in the connection's own transaction (SEC-130): every agent is
    /// authorized again against current state by the grant service (visible, `can_own`, the
    /// connection in its workspace, `custom_only` respected) and the level is expanded there from
    /// the snapshot's tools. A strict apply fails the whole request, which rolls the connection
    /// back; a soft one (the OAuth callback) rolls back only the access, to a savepoint, so the
    /// account stays and the failure is reported.
    async fn apply_grant(
        &self,
        tx: &mut audit::Tx,
        caller: &Caller,
        connection_id: Uuid,
        intent: &Intent,
        events: &mut Vec<AuditEvent>,
    ) -> Result<Applied, ConnectErr> {
        if intent.soft {
            sqlx::query("SAVEPOINT connect_access")
                .execute(&mut **tx)
                .await
                .map_err(DbError::from)?;
        }
        let tools = crate::domains::apps::service::tools(self.app);
        let req = levels::expand(intent.grant.level, &tools);
        let mut applied = Applied::default();
        let mut written = Vec::new();
        let mut failure = None;
        for &agent_id in &intent.grant.agent_ids {
            match grants_service::grant_connection_in_tx(
                tx,
                self.state,
                caller,
                agent_id,
                connection_id,
                req.clone(),
                grants_service::Origin::ConnectJourney,
            )
            .await
            {
                Ok(w) => {
                    written.extend(w.event);
                    applied.grants.push(w.grant);
                    applied.granted_agent_ids.push(agent_id);
                }
                Err(e) => {
                    failure = Some(grants_service::at_agent(e, agent_id));
                    break;
                }
            }
        }
        match failure {
            None => {
                if intent.soft {
                    sqlx::query("RELEASE SAVEPOINT connect_access")
                        .execute(&mut **tx)
                        .await
                        .map_err(DbError::from)?;
                }
                events.extend(written);
                Ok(applied)
            }
            Some(e) if intent.soft => {
                sqlx::query("ROLLBACK TO SAVEPOINT connect_access")
                    .execute(&mut **tx)
                    .await
                    .map_err(DbError::from)?;
                Ok(Applied {
                    failed: Some(e.code().to_owned()),
                    ..Applied::default()
                })
            }
            Some(e) => Err(ConnectErr::Api(e)),
        }
    }

    /// [`Self::write`] inside the caller's transaction: returns the replaced credentials ref and
    /// the critical audit row, which the caller commits with the transaction. `extra` is merged
    /// into the audit metadata (FIX-154: `via`, `agent_id`).
    async fn write_in_tx(
        &self,
        tx: &mut audit::Tx,
        caller: &Caller,
        n: &NewConnection<'_>,
        extra: Option<Value>,
    ) -> Result<(Option<String>, AuditEvent), ConnectErr> {
        let tx = &mut **tx;
        let mut old = None;
        if self.reconnect.is_some() {
            let (r, ver) = repo::credentials_state(&mut *tx, n.id)
                .await?
                .ok_or_else(|| ApiError::not_found("Connection not found."))?;
            if !repo::reconnect_cas(&mut *tx, n, ver).await? {
                return Err(ConnectErr::Api(ApiError::conflict(
                    "conflict",
                    "The connection changed concurrently.",
                )));
            }
            old = Some(r);
        } else {
            repo::insert(&mut *tx, n).await?;
        }
        let hosts = self
            .app
            .agent
            .as_ref()
            .map(|a| a.default_blocklist.clone())
            .unwrap_or_default();
        // (FIX-109) A reconnect to a package version whose blocklist is now empty must not
        // leave the previous version's derived block rule in force.
        let stale_rule = hosts.is_empty() && self.reconnect.is_some();
        if stale_rule {
            derived_rules::delete_for_source(&mut *tx, "blocklist", n.id).await?;
        }
        if !hosts.is_empty() {
            let rule = json!({
                "name": format!("Blocklist: {}", self.app.about.name),
                "enabled": true, "identities": [],
                "targets": hosts.iter().map(|h| json!({"type": "network", "host": h})).collect::<Vec<_>>(),
                "action": "block", "require_approval": false, "rate_limit": null, "conditions": [],
            });
            derived_rules::upsert(
                &mut *tx,
                n.org_id,
                n.workspace_id,
                "blocklist",
                n.id,
                0,
                &rule,
            )
            .await?;
        }
        if !hosts.is_empty() || stale_rule {
            match n.workspace_id {
                Some(ws) => {
                    credentials::notify_gateway_invalidate(&mut *tx, "workspace", ws).await?
                }
                None => credentials::notify_gateway_invalidate(&mut *tx, "org", n.org_id).await?,
            }
        }
        credentials::notify_gateway_invalidate(&mut *tx, "connection", n.id).await?;
        let action = if self.reconnect.is_some() {
            "connection.reconnect"
        } else {
            "connection.create"
        };
        let mut ev = AuditEvent::by(caller, action)
            .org(n.org_id)
            .target("connection", n.id)
            .metadata(json!({"app_id": n.app_id, "package_id": n.package_id, "label": n.label}));
        // (FIX-106, D-178 Q-R5-7) The database lane's opt-out of server certificate verification is
        // a named, audited choice of the person who connected (shown on the connection too).
        if n.metadata.get("tls_verification").and_then(|v| v.as_str()) == Some("off") {
            ev = ev.metadata(json!({
                "app_id": n.app_id, "package_id": n.package_id, "label": n.label,
                "tls_verification": "off",
            }));
        }
        if let Some(Value::Object(x)) = extra {
            let mut m = match &ev.metadata {
                Value::Object(m) => m.clone(),
                _ => Map::new(),
            };
            m.extend(x);
            ev = ev.metadata(Value::Object(m));
        }
        if let Some(ws) = n.workspace_id {
            ev = ev.workspace(ws);
        }
        Ok((old, ev))
    }
}

async fn discard(state: &AppState, c: &SealContext, r: &SecretRef) {
    if let Err(e) = state.vault.discard(c, r, ValueSource::Managed).await {
        tracing::warn!(error = %e, "discarding connection credentials failed");
    }
}

fn exchange_input(
    ctx: &Ctx<'_>,
    method: &str,
    source: Value,
    client: &(Map<String, Value>, Map<String, Value>),
    flow: Option<&Flow>,
    cx: &Map<String, Value>,
) -> Value {
    json!({
        "app_id": ctx.app.app_id, "method_id": method, "source": source,
        "client": kv(&client.0), "client_secret": kv(&client.1),
        "redirect_uri": redirect_uri(ctx.state),
        "pkce_verifier": flow.and_then(|f| f.pkce_verifier.clone()),
        "continuation": flow.and_then(|f| f.continuation.clone()),
        "context": kv(cx),
    })
}

fn gathered_from_hook(c: Connected, client: Option<Uuid>) -> Gathered {
    Gathered {
        credentials: c.credentials,
        metadata: c.metadata,
        tenant_host: c.tenant_host.map(|h| h.to_ascii_lowercase()),
        granted: (!c.scopes.is_empty()).then_some(c.scopes),
        requested: Vec::new(),
        label: Some(c.label),
        form_label: None,
        expires_at: c.expires_at,
        fields: Map::new(),
        client,
        webhook: None,
    }
}

/// The registered instance a connect request targets (C-243): absent = `default_host` (422
/// `instance_required` when the app has none); present = an `active` instance of this app
/// visible to the org (404 otherwise). A reconnect keeps the connection's instance.
async fn resolve_instance(
    state: &AppState,
    app: &AppEntry,
    t: &Target,
    requested: Option<Uuid>,
    reconnect: Option<&ConnectionRow>,
) -> Result<Option<InstanceRow>, ConnectErr> {
    let Some(ins) = &app.instances else {
        return if requested.is_some() {
            Err(invalid(
                "/instance_id",
                "this app has no self-hosted instances",
            ))
        } else {
            Ok(None)
        };
    };
    let id = match reconnect {
        Some(r) => {
            if requested.is_some() && requested != r.instance_id {
                return Err(invalid(
                    "/instance_id",
                    "a reconnect keeps the connection's instance",
                ));
            }
            r.instance_id
        }
        None => requested,
    };
    let Some(id) = id else {
        if reconnect.is_none() && ins.default_host.is_none() {
            return Err(ConnectErr::Api(ApiError::unprocessable(
                "instance_required",
                "Choose the instance to connect to.",
            )));
        }
        return Ok(None);
    };
    app_instances::get(&state.pool, id)
        .await?
        .filter(|i| {
            i.app_id == app.app_id && i.status == "active" && i.org_id.is_none_or(|o| o == t.org)
        })
        .map(Some)
        .ok_or_else(|| ConnectErr::Api(ApiError::not_found("Instance not found.")))
}

pub async fn start(
    state: &AppState,
    caller: &Caller,
    app_id: &str,
    req: ConnectRequest,
) -> Result<Started, ConnectErr> {
    let snap = registry::active_typed(state)
        .await?
        .ok_or_else(|| ApiError::not_found("App not found."))?;
    let (app, pkg) = find_app(&snap, app_id)?;
    let reconnect = match req.reconnect_connection_id {
        Some(id) => Some(
            repo::get(&state.pool, id)
                .await?
                .filter(|r| r.app_id == app_id)
                .ok_or_else(|| ApiError::not_found("Connection not found."))?,
        ),
        None => None,
    };
    let org = req.org_id.or(reconnect.as_ref().map(|r| r.org_id));
    let t = authorize_scope(state, caller, &req.scope, req.workspace_id, org, app_id).await?;
    if let Some(r) = &reconnect
        && (r.org_id != t.org || r.workspace_id != t.ws)
    {
        return Err(ApiError::not_found("Connection not found.").into());
    }
    let method = app
        .connect
        .iter()
        .find(|c| c.id() == req.method_id)
        .ok_or_else(|| invalid("/method_id", "unknown connect method"))?;
    let instance = resolve_instance(state, app, &t, req.instance_id, reconnect.as_ref()).await?;
    let mut access = match req.access.as_deref() {
        None => None,
        Some(a @ ("read" | "write")) => Some(a.to_owned()),
        Some(_) => return Err(invalid("/access", "must be read or write")),
    };
    // (P5-UX-2, C-957, SEC-130) The access that travels with the account is checked before any
    // write or redirect; the connection's `access` follows the level (C-980).
    let grant = match &req.grant {
        None => None,
        Some(g) => {
            if reconnect.is_some() {
                return Err(ConnectErr::Api(ApiError::unprocessable(
                    "grant_not_allowed_on_reconnect",
                    "A reconnect keeps the access it has; change it on the agent's Access tab.",
                )));
            }
            let Some(ws) = t.ws else {
                return Err(invalid("/grant", "needs workspace scope"));
            };
            grants_service::check_agents_for_new_account(state, caller, ws, &g.agent_ids).await?;
            // (C-1067, D-419) `read` on a send-only app is refused before it is expanded.
            levels::require_offered(
                g.level,
                &crate::domains::apps::service::tools(app),
                app.grants.custom_only,
                app_id,
            )?;
            if g.level == Level::Act && app.grants.custom_only {
                return Err(ConnectErr::Api(
                    ApiError::unprocessable(
                        "full_grant_forbidden",
                        "This app only allows custom access: choose Read only or Ask before changes.",
                    )
                    .with_details(json!({"app_id": app_id})),
                ));
            }
            let derived = levels::access_for_levels(&[g.level]);
            if access.as_deref().is_some_and(|a| a != derived) {
                return Err(invalid(
                    "/access",
                    "follows grant.level; leave it out when a grant is given",
                ));
            }
            access = Some(derived.to_owned());
            Some(Intent {
                grant: g.clone(),
                soft: false,
            })
        }
    };
    // (P5-UX-11, C-990) A new organization account carries its limit; `access` follows it. A
    // workspace's account has no limit, and a reconnect keeps the one the account has.
    let limit = match req.limit {
        None => None,
        Some(l) => {
            if t.ws.is_some() || reconnect.is_some() {
                return Err(invalid(
                    "/limit",
                    "only a new organization account has a limit; change it on the account",
                ));
            }
            levels::require_offered(
                l,
                &crate::domains::apps::service::tools(app),
                app.grants.custom_only,
                app_id,
            )?;
            if l == Level::Act && app.grants.custom_only {
                return Err(ConnectErr::Api(
                    ApiError::unprocessable(
                        "full_grant_forbidden",
                        "This app only allows custom access: choose Read only or Ask before changes.",
                    )
                    .with_details(json!({"app_id": app_id})),
                ));
            }
            let derived = levels::access_for_limit(l);
            if access.as_deref().is_some_and(|a| a != derived) {
                return Err(invalid(
                    "/access",
                    "follows limit; leave it out when a limit is given",
                ));
            }
            access = Some(derived.to_owned());
            Some(l.as_str().to_owned())
        }
    };
    let ctx = Ctx {
        state,
        snap: &snap,
        app,
        pkg,
        user: caller.user_id,
        t,
        reconnect,
        instance,
        access,
        limit,
        grant,
    };
    match method {
        Connect::Fields(f) => {
            let p = fields::parse(&f.fields, &req.fields)?;
            let mut g = from_fields(p);
            // (P5-ICS, D-273, C-843) A secret-link app's URL is validated, fetched once and
            // stored as the credential (its host as metadata); no probe runs for it.
            webhook::prepare(&ctx, &mut g, req.webhook.as_ref()).await?;
            link::check(&ctx, &mut g).await?;
            // (P5-CALDAV, D-275, C-845) An own-server app's address is discovered (principal →
            // calendar home) and pinned, with the origins used, before anything is stored.
            own_server::check(&ctx, &mut g).await?;
            // (FIX-136, D-241) The scopes the pasted token has: declared by the person, else
            // prefilled by the method's read probes. Stored as the connection's `scopes`; the
            // gateway trusts them only to refuse a tool the token cannot serve.
            if !f.scopes.is_empty() {
                g.granted = Some(match &req.scopes {
                    Some(s) => {
                        if let Some(bad) =
                            s.iter().find(|x| !f.scopes.iter().any(|d| &d.scope == *x))
                        {
                            return Err(invalid(
                                "/scopes",
                                &format!("`{bad}` is not a scope of this connect method"),
                            ));
                        }
                        s.clone()
                    }
                    None => ctx.probe_declared_scopes(f, &g).await,
                });
            }
            let generated = g.webhook.as_ref().and_then(|w| w.generated.clone());
            let (row, missing, applied) = ctx.finalize(caller, method, Kind::Fields, g).await?;
            let mut started = connected(row, missing, applied, &snap);
            if let ConnectResponse::Connected { signing_secret, .. } = &mut started.response {
                *signing_secret = generated;
            }
            Ok(started)
        }
        Connect::Oauth2(o) => start_oauth(&ctx, o, &req).await,
        Connect::Hook(h) => {
            let p = fields::parse(&h.fields, &req.fields)?;
            let client = find_ce_client(&ctx, &req).await?;
            let cf = match &client {
                Some(c) => client_fields(state, c).await?,
                None => Default::default(),
            };
            let cx = context(state, pkg, ctx.t.org, ctx.t.ws).await?;
            let mut flow = ctx.new_flow(
                &h.id,
                client.as_ref().map(|c| c.id),
                req.access_level.clone(),
            )?;
            if h.authorize {
                let sealed = flow
                    .seal(state)
                    .ok_or_else(|| ApiError::internal("Internal error."))?;
                let input = json!({
                    "app_id": app.app_id, "method_id": h.id, "client": kv(&cf.0),
                    "redirect_uri": redirect_uri(state), "state": sealed, "scopes": [],
                    "access_level": req.access_level, "fields": kv(&p.raw), "context": kv(&cx),
                });
                let out = state
                    .connect
                    .hooks
                    .authorize(&ctx.call(), input)
                    .await
                    .map_err(|e| ConnectErr::Hook(e.0))?;
                if !redirect_allowed(&snap, app, pkg, &out.url, ctx.registered_base()) {
                    return Err(ConnectErr::Hook("hook returned a disallowed URL".into()));
                }
                flow.continuation = out.continuation;
                let cookie = flow
                    .seal(state)
                    .ok_or_else(|| ApiError::internal("Internal error."))?;
                return Ok(Started {
                    response: ConnectResponse::Redirect { url: out.url },
                    cookie: Some(flow::cookie(&cookie, flow::cookie_ttl())),
                });
            }
            // (FIX-133) The reserved `aurora_access` key tells the hook what the connection may
            // do so it can narrow the token it mints (vendor-enforced); it is set here after the
            // form was validated, so a user-supplied value never reaches the guest.
            let mut form = p.raw.clone();
            form.insert(
                "aurora_access".into(),
                Value::String(ctx.effective_access().to_owned()),
            );
            let input = exchange_input(&ctx, &h.id, json!({"form": kv(&form)}), &cf, None, &cx);
            match state
                .connect
                .hooks
                .exchange(&ctx.call(), input)
                .await
                .map_err(|e| ConnectErr::Hook(e.0))?
            {
                Exchanged::Connected(c) => {
                    let (row, missing, applied) = ctx
                        .finalize(
                            caller,
                            method,
                            Kind::Hook,
                            gathered_from_hook(c, client.map(|c| c.id)),
                        )
                        .await?;
                    Ok(connected(row, missing, applied, &snap))
                }
                Exchanged::Continue { url, continuation } => {
                    let (url, cookie) = continue_to(&ctx, &mut flow, url, continuation)?;
                    Ok(Started {
                        response: ConnectResponse::Redirect { url },
                        cookie: Some(cookie),
                    })
                }
            }
        }
    }
}

/// The sealed (empty) credentials of a connection created inside a transaction that may still
/// roll back: the caller discards them when it does (FIX-154).
pub struct SealedCredentials(SealContext, SecretRef);

impl SealedCredentials {
    pub async fn discard(self, state: &AppState) {
        discard(state, &self.0, &self.1).await;
    }
}

/// (FIX-154, D-306(c), C-896) The workspace's connection of a credential-free app, inside the
/// caller's transaction: the oldest active one, else a new one through the same creation path as
/// the `fields` branch of [`start`] (zero-field method, label from `about.label_hint`, else the
/// app name), audited `connection.create` with `via: "agent_grant"` and the agent. The caller has
/// already taken the per-(workspace, app) lock and checked the app and the caller.
///
/// Returns the row, whether it was created, and — when it was — the sealed credentials to
/// discard if the transaction does not commit.
#[allow(clippy::too_many_arguments)]
pub async fn ensure_credential_free_in_tx(
    state: &AppState,
    tx: &mut audit::Tx,
    caller: &Caller,
    snap: &Snapshot,
    app: &AppEntry,
    pkg: &PackageEntry,
    org_id: Uuid,
    workspace_id: Uuid,
    agent_id: Uuid,
    events: &mut Vec<AuditEvent>,
) -> Result<(ConnectionRow, Option<SealedCredentials>), ApiError> {
    if let Some(row) =
        repo::oldest_active_in_workspace(&mut **tx, workspace_id, &app.app_id).await?
    {
        return Ok((row, None));
    }
    let method = app
        .connect
        .iter()
        .find(|c| matches!(c, Connect::Fields(f) if f.fields.is_empty()))
        .ok_or_else(|| ApiError::internal("Internal error."))?;
    let ctx = Ctx {
        state,
        snap,
        app,
        pkg,
        user: caller.user_id,
        t: Target {
            org: org_id,
            ws: Some(workspace_id),
        },
        reconnect: None,
        instance: None,
        access: None,
        limit: None,
        grant: None,
    };
    let label = app
        .about
        .label_hint
        .as_deref()
        .and_then(probe::clean_label)
        .unwrap_or_else(|| app.about.name.clone());
    let id = ids::new_id();
    let sctx = SealContext {
        purpose: Purpose::ConnectionCredentials,
        owner_id: id,
        org_id,
        workspace_id: Some(workspace_id),
    };
    let bytes = serde_json::to_vec(&Map::<String, Value>::new())
        .map_err(|_| ApiError::internal("Internal error."))?;
    let sref = state
        .vault
        .seal(&sctx, aurora_core::Secret::new(bytes))
        .await
        .map_err(secret_err)?;
    let col = sref.to_column();
    let n = NewConnection {
        id,
        org_id,
        workspace_id: Some(workspace_id),
        app_id: &app.app_id,
        package_id: &pkg.package_id,
        app_version: &pkg.version,
        label: &label,
        credentials_ref: &col,
        credentials_expires_at: None,
        scopes: &[],
        requested_scopes: &[],
        metadata: &json!({}),
        tenant_host: None,
        oauth_client_id: None,
        connected_by: caller.user_id,
        instance_id: None,
        credential_keys: &[],
        connect_method: Some(method.id()),
        access: None,
        limit_level: None,
        config: None,
        replace_label: false,
    };
    let extra = json!({"via": "agent_grant", "agent_id": agent_id});
    let ev = match ctx.write_in_tx(tx, caller, &n, Some(extra)).await {
        Ok((_, ev)) => ev,
        Err(e) => {
            discard(state, &sctx, &sref).await;
            return Err(e.into());
        }
    };
    events.push(ev);
    let row = match repo::get(&mut **tx, id).await {
        Ok(Some(r)) => r,
        _ => {
            discard(state, &sctx, &sref).await;
            return Err(ApiError::internal("Internal error."));
        }
    };
    Ok((row, Some(SealedCredentials(sctx, sref))))
}

fn from_fields(p: Parsed) -> Gathered {
    Gathered {
        credentials: p.credentials,
        metadata: p.metadata,
        tenant_host: p.tenant_host,
        form_label: p.label,
        fields: p.raw,
        ..Gathered::default()
    }
}

fn connected(
    row: ConnectionRow,
    missing: Vec<String>,
    applied: Applied,
    snap: &Snapshot,
) -> Started {
    Started {
        response: ConnectResponse::Connected {
            connection: Box::new(Connection::new(row, Some(snap))),
            missing_scopes: missing,
            grants: applied.grants,
            signing_secret: None,
        },
        cookie: None,
    }
}

/// Hook methods may use an OAuth client only when the caller names one.
async fn find_ce_client(
    ctx: &Ctx<'_>,
    req: &ConnectRequest,
) -> Result<Option<ClientRow>, ConnectErr> {
    let Some(id) = req.oauth_client_id else {
        return Ok(None);
    };
    let c = aurora_db::repo::oauth_clients::get(&ctx.state.pool, id).await?;
    let usable = c.filter(|c| {
        c.enabled
            && c.instance_id == ctx.instance.as_ref().map(|i| i.id)
            && c.package_id == ctx.pkg.package_id
            && c.org_id.is_none_or(|o| {
                o == ctx.t.org && c.workspace_id.is_none_or(|w| Some(w) == ctx.t.ws)
            })
    });
    usable
        .map(Some)
        .ok_or_else(|| client_required(ctx.t.org, "OAuth client not usable here."))
}

fn continue_to(
    ctx: &Ctx<'_>,
    flow: &mut Flow,
    url: String,
    continuation: String,
) -> Result<(String, String), ConnectErr> {
    if !redirect_allowed(ctx.snap, ctx.app, ctx.pkg, &url, ctx.registered_base()) {
        return Err(ConnectErr::Hook("hook returned a disallowed URL".into()));
    }
    flow.continuation = Some(continuation);
    flow.exp = Flow::expiry();
    let sealed = flow
        .seal(ctx.state)
        .ok_or_else(|| ApiError::internal("Internal error."))?;
    let mut u = reqwest::Url::parse(&url).map_err(|_| ConnectErr::Hook("bad URL".into()))?;
    u.query_pairs_mut().append_pair("state", &sealed);
    Ok((u.to_string(), flow::cookie(&sealed, flow::cookie_ttl())))
}

async fn start_oauth(
    ctx: &Ctx<'_>,
    o: &Oauth2Connect,
    req: &ConnectRequest,
) -> Result<Started, ConnectErr> {
    let ce = ctx
        .snap
        .oauth_clients
        .iter()
        .find(|c| c.package_id == ctx.pkg.package_id && c.client_key == o.client)
        .ok_or_else(|| client_required(ctx.t.org, "The app's OAuth client is not declared."))?;
    let client = resolve_client(
        ctx.state,
        &ctx.pkg.package_id,
        &o.client,
        &ctx.t,
        req.oauth_client_id,
        ctx.instance.as_ref().map(|i| i.id),
    )
    .await?;
    let level = match (&req.access_level, o.access_levels.is_empty()) {
        (Some(l), false) => Some(
            o.access_levels
                .iter()
                .find(|a| &a.id == l)
                .ok_or_else(|| invalid("/access_level", "unknown access level"))?,
        ),
        (None, false) => o.access_levels.first(),
        (Some(_), true) => {
            return Err(invalid("/access_level", "this method has no access levels"));
        }
        (None, true) => None,
    };
    let client_id = client
        .fields
        .get("client_id")
        .and_then(Value::as_str)
        .ok_or_else(|| client_required(ctx.t.org, "The OAuth client has no client_id."))?
        .to_owned();
    let parsed = fields::parse(&o.fields, &req.fields)?;
    let tenant = parsed
        .tenant_host
        .clone()
        .or_else(|| ctx.reconnect.as_ref().and_then(|r| r.tenant_host.clone()));
    // Validate the tenant host (zone, no IP/userinfo/port) before any outbound request or
    // redirect (S-14).
    if let Some(h) = &tenant
        && !probe::tenant_ok(ctx.snap, ctx.app, h)
    {
        return Err(invalid(
            "/fields",
            "tenant host is not allowed for this app",
        ));
    }
    let authorize = oauth_url(&ce.authorize_url, tenant.as_deref(), ctx.instance_base())
        .ok_or_else(|| {
            invalid(
                "/fields",
                "a tenant host is required for this connect method",
            )
        })?;
    if oauth_url(&ce.token_url, tenant.as_deref(), ctx.instance_base()).is_none() {
        return Err(invalid(
            "/fields",
            "a tenant host is required for this connect method",
        ));
    }
    let mut flow = ctx.new_flow(&o.id, Some(client.id), level.map(|l| l.id.clone()))?;
    flow.scopes = o.default_scopes.clone();
    flow.tenant_host = tenant;
    flow.metadata = parsed.metadata;
    let mut params: BTreeMap<String, String> = o.authorize_params.clone();
    if let Some(l) = level {
        params.extend(l.authorize_params.clone());
    }
    if ce.pkce {
        let v = flow::random(32)?;
        params.insert("code_challenge".into(), flow::challenge(&v));
        params.insert("code_challenge_method".into(), "S256".into());
        flow.pkce_verifier = Some(v);
    }
    let sealed = flow
        .seal(ctx.state)
        .ok_or_else(|| ApiError::internal("Internal error."))?;
    params.insert("response_type".into(), "code".into());
    params.insert("client_id".into(), client_id);
    params.insert("redirect_uri".into(), redirect_uri(ctx.state));
    params.insert("state".into(), sealed.clone());
    if !o.default_scopes.is_empty() {
        params.insert("scope".into(), o.default_scopes.join(" "));
    }
    let mut url =
        reqwest::Url::parse(&authorize).map_err(|_| ApiError::internal("Bad authorize URL."))?;
    url.query_pairs_mut().extend_pairs(params);
    Ok(Started {
        response: ConnectResponse::Redirect {
            url: url.to_string(),
        },
        cookie: Some(flow::cookie(&sealed, flow::cookie_ttl())),
    })
}

// ---- callback ----

pub async fn callback(
    state: &AppState,
    caller: &Caller,
    q: &HashMap<String, String>,
    cookie: Option<&str>,
) -> Result<Done, Failure> {
    const BAD: &str = "oauth_state_invalid";
    let (param, cookie) = q.get("state").map(String::as_str).zip(cookie).ok_or(BAD)?;
    let pf = Flow::open(state, param).ok_or(BAD)?;
    let flow = Flow::open(state, cookie).ok_or(BAD)?;
    if pf.nonce != flow.nonce || flow.user_id != caller.user_id {
        return Err(BAD.into());
    }
    if q.contains_key("error") {
        return Err(Failure {
            code: "oauth_exchange_failed",
            landing: Some(landing(state, &flow).await),
        });
    }
    let land = landing(state, &flow).await;
    match run_callback(state, caller, q, flow).await {
        Ok(Done::Connected(id, missing, _, applied)) => {
            Ok(Done::Connected(id, missing, land, applied))
        }
        Ok(other) => Ok(other),
        Err(e) => {
            if let ConnectErr::Api(a) = &e {
                tracing::warn!(code = a.code(), "connect callback failed");
            }
            Err(Failure {
                code: e.code(),
                landing: Some(land),
            })
        }
    }
}

async fn run_callback(
    state: &AppState,
    caller: &Caller,
    q: &HashMap<String, String>,
    flow: Flow,
) -> Result<Done, ConnectErr> {
    let snap = registry::active_typed(state)
        .await?
        .ok_or(ConnectErr::Exchange)?;
    let (app, pkg) = find_app(&snap, &flow.app_id).map_err(|_| ConnectErr::Exchange)?;
    let org = Some(flow.org_id);
    let t = authorize_scope(
        state,
        caller,
        &flow.scope,
        flow.workspace_id,
        org,
        &flow.app_id,
    )
    .await?;
    let reconnect = match flow.reconnect {
        Some(id) => repo::get(&state.pool, id).await?,
        None => None,
    };
    let method = app
        .connect
        .iter()
        .find(|c| c.id() == flow.method_id)
        .ok_or(ConnectErr::Exchange)?;
    // The instance must still be registered, active and visible: a flow cannot outlive a delete.
    let instance = match flow.instance_id {
        Some(id) => Some(
            app_instances::get(&state.pool, id)
                .await?
                .filter(|i| {
                    i.app_id == app.app_id
                        && i.status == "active"
                        && i.org_id.is_none_or(|o| o == t.org)
                })
                .ok_or(ConnectErr::Exchange)?,
        ),
        None => None,
    };
    let ctx = Ctx {
        state,
        snap: &snap,
        app,
        pkg,
        user: caller.user_id,
        t,
        reconnect,
        instance,
        access: flow.access.clone(),
        limit: flow.limit.clone(),
        // (SEC-130) The access carried through the sign-in is applied now, softly: the grant
        // service authorizes every agent again, and a failure keeps the account.
        grant: flow.grant.clone().map(|grant| Intent { grant, soft: true }),
    };
    let client_row = match flow.client_id {
        Some(id) => aurora_db::repo::oauth_clients::get(&state.pool, id).await?,
        None => None,
    };
    // A client bound to an instance serves only that instance (and an unbound one never an
    // instance): the OAuth application matches the host the code is exchanged on (SEC-59).
    if client_row
        .as_ref()
        .is_some_and(|c| c.instance_id != flow.instance_id)
    {
        return Err(ConnectErr::Exchange);
    }
    let cf = match &client_row {
        Some(c) => client_fields(state, c).await?,
        None => Default::default(),
    };
    let cx = context(state, pkg, ctx.t.org, ctx.t.ws).await?;
    let (kind, g) = match method {
        Connect::Oauth2(o) => {
            let ce = snap
                .oauth_clients
                .iter()
                .find(|c| c.package_id == pkg.package_id && c.client_key == o.client)
                .ok_or(ConnectErr::Exchange)?;
            let code = q
                .get("code")
                .filter(|c| !c.is_empty())
                .ok_or(ConnectErr::Exchange)?;
            let g = token_exchange(&ctx, ce, o, &cf, code, &flow).await?;
            (Kind::Oauth, g)
        }
        Connect::Hook(h) => {
            let params: Map<String, Value> = q
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect();
            let input = exchange_input(
                &ctx,
                &h.id,
                json!({"oauth_callback": kv(&params)}),
                &cf,
                Some(&flow),
                &cx,
            );
            match state
                .connect
                .hooks
                .exchange(&ctx.call(), input)
                .await
                .map_err(|e| ConnectErr::Hook(e.0))?
            {
                Exchanged::Connected(c) => (Kind::Hook, gathered_from_hook(c, flow.client_id)),
                Exchanged::Continue { url, continuation } => {
                    let mut f = flow.clone();
                    let (url, cookie) = continue_to(&ctx, &mut f, url, continuation)?;
                    return Ok(Done::Redirect { url, cookie });
                }
            }
        }
        Connect::Fields(_) => return Err(ConnectErr::Exchange),
    };
    let (row, missing, applied) = ctx.finalize(caller, method, kind, g).await?;
    Ok(Done::Connected(
        row.id,
        missing,
        Landing {
            app_id: flow.app_id.clone(),
            connections_path: None,
        },
        applied,
    ))
}

/// Resolve an OAuth endpoint: fixed URLs as-is; templates render with the tenant host and must
/// stay `https` inside their zone (SEC-17 / I-20).
fn oauth_url(
    u: &UrlOrTemplate,
    tenant: Option<&str>,
    instance: Option<InstanceBase<'_>>,
) -> Option<String> {
    match u {
        UrlOrTemplate::Url(u) if u.contains(aurora_registry::manifest::INSTANCE_HOST) => {
            probe::instance_url(u, instance?)
        }
        UrlOrTemplate::Url(u) => Some(u.clone()),
        UrlOrTemplate::Templated(z) => {
            let url = probe::render_url(&z.template, tenant)?;
            probe::in_zone(&url, &z.zone).then_some(url)
        }
    }
}

/// The authorization-code request to a token endpoint (`grant_type=authorization_code`, the code,
/// the redirect URI, the PKCE verifier) with the client's id and secret where the manifest's
/// `client_auth` puts them and in its `body_format`. The code exchange and the sign-in app Test
/// (C-1070) both build their request here; `http` is one of the guarded clients of `Env`.
fn token_request(
    http: &reqwest::Client,
    url: reqwest::Url,
    ce: &OauthClientEntry,
    cf: &(Map<String, Value>, Map<String, Value>),
    code: &str,
    redirect: String,
    pkce_verifier: Option<&str>,
) -> reqwest::RequestBuilder {
    let get = |k: &str| {
        cf.1.get(k)
            .or_else(|| cf.0.get(k))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let mut params: BTreeMap<String, String> = BTreeMap::from([
        ("grant_type".into(), "authorization_code".into()),
        ("code".into(), code.to_owned()),
        ("redirect_uri".into(), redirect),
    ]);
    if let Some(v) = pkce_verifier {
        params.insert("code_verifier".into(), v.to_owned());
    }
    let mut req = http.post(url).header("accept", "application/json");
    match ce.client_auth {
        ClientAuth::Basic => {
            req = req.basic_auth(get("client_id").unwrap_or_default(), get("client_secret"));
        }
        ClientAuth::Body => {
            for k in ["client_id", "client_secret"] {
                if let Some(v) = get(k) {
                    params.insert(k.into(), v);
                }
            }
        }
    }
    match ce.body_format {
        BodyFormat::Form => req.form(&params),
        BodyFormat::Json => req.json(&params),
    }
}

/// (P5-REFINE-6, C-1070) What a sign-in app's token endpoint said to a made-up code.
#[derive(Debug)]
pub(crate) enum TokenProbe {
    /// The app's sign-in has no fixed token endpoint (a tenant template) or none at all.
    NotTestable,
    /// A network error, a timeout or the destination guard's refusal.
    Unreachable { host: Option<String> },
    /// The provider answered: its HTTP status and the OAuth `error` code, if any. Nothing else
    /// of the body is kept.
    Answered {
        host: Option<String>,
        status: u16,
        error: Option<String>,
    },
}

/// (C-1070) Send one authorization-code request with a random code to the token endpoint of
/// `client`'s sign-in (from the active manifest), through the destination guard (the instance
/// client for a client bound to a self-hosted instance), built by [`token_request`]. At most
/// 8 KiB of the answer is read and only its `error` code is kept.
pub(crate) async fn probe_token_endpoint(
    state: &AppState,
    client: &ClientRow,
) -> Result<TokenProbe, ApiError> {
    let Some(snap) = registry::active_typed(state).await? else {
        return Ok(TokenProbe::NotTestable);
    };
    let Some(ce) = snap
        .oauth_clients
        .iter()
        .find(|c| c.package_id == client.package_id && c.client_key == client.client_key)
    else {
        return Ok(TokenProbe::NotTestable);
    };
    let instance = match client.instance_id {
        Some(id) => app_instances::get(&state.pool, id).await?,
        None => None,
    };
    let base = instance
        .as_ref()
        .map(|i| InstanceBase::new(&i.host, i.base_path.as_deref()));
    let Some(raw) = oauth_url(&ce.token_url, None, base) else {
        return Ok(TokenProbe::NotTestable);
    };
    let host_of = |u: &str| {
        reqwest::Url::parse(u)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
    };
    let env = &state.connect;
    let Ok(url) = env.prepare(&raw) else {
        return Ok(TokenProbe::Unreachable {
            host: host_of(&raw),
        });
    };
    let host = host_of(&raw);
    let cf = client_fields(state, client).await.map_err(ApiError::from)?;
    let http = if instance.is_some() {
        &env.http_instance
    } else {
        &env.http
    };
    let code = format!("aurora-test-{}", flow::random(16)?);
    let verifier = if ce.pkce {
        Some(flow::random(32)?)
    } else {
        None
    };
    let req = token_request(
        http,
        url,
        ce,
        &cf,
        &code,
        redirect_uri(state),
        verifier.as_deref(),
    )
    .timeout(std::time::Duration::from_secs(10));
    let Ok(resp) = req.send().await else {
        return Ok(TokenProbe::Unreachable { host });
    };
    let status = resp.status().as_u16();
    let body = read_at_most(resp, 8 * 1024).await;
    Ok(TokenProbe::Answered {
        host,
        status,
        error: oauth_error_code(&body),
    })
}

/// Up to `cap` bytes of a response body (a longer one is cut; a read error gives what arrived).
async fn read_at_most(mut resp: reqwest::Response, cap: usize) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(Some(chunk)) = resp.chunk().await {
        let room = cap.saturating_sub(out.len());
        out.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if out.len() >= cap {
            break;
        }
    }
    out
}

/// The OAuth `error` code of a token endpoint's answer (JSON, or form-encoded as some providers
/// answer), at most 64 characters of `[A-Za-z0-9_.-]`; nothing else of the body.
fn oauth_error_code(body: &[u8]) -> Option<String> {
    let clean = |s: &str| {
        let s = s.trim();
        (!s.is_empty()
            && s.len() <= 64
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')))
        .then(|| s.to_owned())
    };
    if let Ok(v) = serde_json::from_slice::<Value>(body) {
        return v.get("error").and_then(Value::as_str).and_then(clean);
    }
    let text = std::str::from_utf8(body).ok()?;
    url::form_urlencoded::parse(text.as_bytes())
        .find(|(k, _)| k == "error")
        .and_then(|(_, v)| clean(&v))
}

async fn token_exchange(
    ctx: &Ctx<'_>,
    ce: &OauthClientEntry,
    o: &Oauth2Connect,
    cf: &(Map<String, Value>, Map<String, Value>),
    code: &str,
    flow: &Flow,
) -> Result<Gathered, ConnectErr> {
    let env = &ctx.state.connect;
    let tenant = flow.tenant_host.as_deref().or_else(|| {
        ctx.reconnect
            .as_ref()
            .and_then(|r| r.tenant_host.as_deref())
    });
    if let Some(h) = tenant
        && !probe::tenant_ok(ctx.snap, ctx.app, h)
    {
        return Err(ConnectErr::Exchange);
    }
    let url = oauth_url(&ce.token_url, tenant, ctx.instance_base()).ok_or(ConnectErr::Exchange)?;
    let url = env.prepare(&url).map_err(|_| ConnectErr::Exchange)?;
    // A registered instance is dialled through the guard that honours `gateway.allowed_private`.
    let http = if ctx.instance.is_some() {
        &env.http_instance
    } else {
        &env.http
    };
    let req = token_request(
        http,
        url,
        ce,
        cf,
        code,
        redirect_uri(ctx.state),
        flow.pkce_verifier.as_deref(),
    );
    let resp = req.send().await.map_err(|_| ConnectErr::Exchange)?;
    if !resp.status().is_success() {
        return Err(ConnectErr::Exchange);
    }
    let body = probe::read_capped(resp).await.ok_or(ConnectErr::Exchange)?;
    let doc: Value = serde_json::from_slice(&body).map_err(|_| ConnectErr::Exchange)?;
    let mut mapping: BTreeMap<String, String> = o.token_response.clone();
    if mapping.is_empty() {
        for (k, p) in [
            ("access_token", "/access_token"),
            ("refresh_token", "/refresh_token"),
            ("expires_in", "/expires_in"),
            ("scope", "/scope"),
        ] {
            mapping.insert(k.into(), p.into());
        }
    }
    let mut g = Gathered {
        requested: flow.scopes.clone(),
        client: flow.client_id,
        ..Gathered::default()
    };
    for (field, ptr) in &mapping {
        let val = doc.pointer(ptr);
        match field.as_str() {
            "expires_in" => {
                g.expires_at = val
                    .and_then(|v| {
                        v.as_i64()
                            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                    })
                    .map(|s| Utc::now().timestamp() + s);
            }
            "scope" => {
                g.granted = val.map(|v| match v {
                    Value::Array(a) => a
                        .iter()
                        .filter_map(|s| s.as_str().map(str::to_owned))
                        .collect(),
                    other => probe::sval_pub(other)
                        .unwrap_or_default()
                        .split([' ', ','])
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect(),
                });
            }
            _ => match val.and_then(probe::sval_pub).filter(|s| !s.is_empty()) {
                Some(s) => {
                    g.credentials.insert(field.clone(), Value::String(s));
                }
                None => {
                    if ctx
                        .app
                        .credentials
                        .iter()
                        .any(|c| &c.name == field && !c.optional)
                    {
                        return Err(ConnectErr::Exchange);
                    }
                }
            },
        }
    }
    g.tenant_host = tenant.map(str::to_owned);
    g.metadata = flow.metadata.clone();
    Ok(g)
}
