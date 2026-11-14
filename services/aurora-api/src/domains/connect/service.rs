use super::dto::{ConnectRequest, ConnectResponse};
use super::fields::{self, Parsed};
use super::flow::{self, Flow};
use super::hooks::{Call, Connected, Exchanged, kv};
use super::probe::{self, Vars};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::apps::service::find_app;
use crate::domains::connections::dto::Connection;
use crate::domains::plugins::registry;
use crate::domains::secrets::service::secret_err;
use crate::http::auth::{Caller, require_own};
use aurora_core::{ApiError, ids};
use aurora_db::DbError;
use aurora_db::repo::app_instances::{self, InstanceRow};
use aurora_db::repo::connections::{self as repo, ConnectionRow, NewConnection};
use aurora_db::repo::{credentials, derived_rules, gateway};
use aurora_registry::manifest::{BodyFormat, ClientAuth, Connect, Oauth2Connect, UrlOrTemplate};
use aurora_registry::snapshot::{AppEntry, OauthClientEntry, PackageEntry, Snapshot};
use aurora_secrets::{Purpose, SealContext, SecretRef, ValueSource};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

pub enum ConnectErr {
    Api(ApiError),
    Probe(Option<u16>),
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
            ConnectErr::Probe(s) => {
                ApiError::unprocessable("connect_probe_failed", "The connection probe failed.")
                    .with_details(json!({"upstream_status": s}))
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

pub enum Done {
    Connected(Uuid, Vec<String>),
    Redirect { url: String, cookie: String },
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

fn client_required(msg: &str) -> ConnectErr {
    ConnectErr::Api(ApiError::unprocessable(
        "oauth_client_required",
        msg.to_owned(),
    ))
}

/// OAuth methods of a self-hosted instance use only a BYOC client bound to that instance (C-243).
fn instance_client_required() -> ConnectErr {
    ConnectErr::Api(ApiError::unprocessable(
        "instance_oauth_client_required",
        "Register an OAuth application for this instance first.",
    ))
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
            instance_client_required()
        } else {
            client_required("No usable OAuth client is configured.")
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
    instance: Option<&str>,
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
}

#[derive(Default)]
struct Gathered {
    credentials: Map<String, Value>,
    metadata: Map<String, Value>,
    tenant_host: Option<String>,
    granted: Option<Vec<String>>,
    requested: Vec<String>,
    label: Option<String>,
    expires_at: Option<i64>,
    fields: Map<String, Value>,
    client: Option<Uuid>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Fields,
    Oauth,
    Hook,
}

impl Ctx<'_> {
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

    /// The registered instance's host (the host the `{instance}` host entries apply to).
    fn registered_host(&self) -> Option<&str> {
        self.instance.as_ref().map(|i| i.host.as_str())
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
    ) -> Result<(ConnectionRow, Vec<String>), ConnectErr> {
        let declared = |n: &str| self.app.credentials.iter().any(|c| c.name == n);
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
                    instance_registered: self.instance.is_some(),
                },
            )
            .await
            .map_err(|f| ConnectErr::Probe(f.status))?;
            metadata.extend(out.metadata);
            label = out.label.or(label);
        }
        let label = label
            .or_else(|| self.reconnect.as_ref().map(|r| r.label.clone()))
            .unwrap_or_else(|| self.app.about.name.clone());
        let granted = g.granted.clone().unwrap_or_else(|| g.requested.clone());
        let missing: Vec<String> = g
            .requested
            .iter()
            .filter(|s| !granted.contains(s))
            .cloned()
            .collect();
        let row = self
            .persist(
                caller,
                &g,
                &label,
                &Value::Object(metadata),
                &granted,
                method.id(),
            )
            .await?;
        Ok((row, missing))
    }

    async fn persist(
        &self,
        caller: &Caller,
        g: &Gathered,
        label: &str,
        metadata: &Value,
        scopes: &[String],
        method_id: &str,
    ) -> Result<ConnectionRow, ConnectErr> {
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
        };
        let old = match self.write(caller, &n).await {
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
        Ok(row)
    }

    /// Insert or CAS-update inside one transaction (with its `connection.create` /
    /// `connection.reconnect` audit row, RFE-008); returns the replaced credentials ref.
    async fn write(
        &self,
        caller: &Caller,
        n: &NewConnection<'_>,
    ) -> Result<Option<String>, ConnectErr> {
        let mut tx = audit::begin(self.state).await?;
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
        let mut ev = AuditEvent::new(AuditActor::from_caller(caller), action, Outcome::Success)
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
        if let Some(ws) = n.workspace_id {
            ev = ev.workspace(ws);
        }
        audit::commit_critical(tx, &ev).await?;
        Ok(old)
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
        expires_at: c.expires_at,
        fields: Map::new(),
        client,
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
    let ctx = Ctx {
        state,
        snap: &snap,
        app,
        pkg,
        user: caller.user_id,
        t,
        reconnect,
        instance,
    };
    match method {
        Connect::Fields(f) => {
            let p = fields::parse(&f.fields, &req.fields)?;
            let (row, missing) = ctx
                .finalize(caller, method, Kind::Fields, from_fields(p))
                .await?;
            Ok(connected(row, missing, &snap))
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
                if !redirect_allowed(&snap, app, pkg, &out.url, ctx.registered_host()) {
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
            let input = exchange_input(&ctx, &h.id, json!({"form": kv(&p.raw)}), &cf, None, &cx);
            match state
                .connect
                .hooks
                .exchange(&ctx.call(), input)
                .await
                .map_err(|e| ConnectErr::Hook(e.0))?
            {
                Exchanged::Connected(c) => {
                    let (row, missing) = ctx
                        .finalize(
                            caller,
                            method,
                            Kind::Hook,
                            gathered_from_hook(c, client.map(|c| c.id)),
                        )
                        .await?;
                    Ok(connected(row, missing, &snap))
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

fn from_fields(p: Parsed) -> Gathered {
    Gathered {
        credentials: p.credentials,
        metadata: p.metadata,
        tenant_host: p.tenant_host,
        fields: p.raw,
        ..Gathered::default()
    }
}

fn connected(row: ConnectionRow, missing: Vec<String>, snap: &Snapshot) -> Started {
    Started {
        response: ConnectResponse::Connected {
            connection: Box::new(Connection::new(row, Some(snap))),
            missing_scopes: missing,
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
        .ok_or_else(|| client_required("OAuth client not usable here."))
}

fn continue_to(
    ctx: &Ctx<'_>,
    flow: &mut Flow,
    url: String,
    continuation: String,
) -> Result<(String, String), ConnectErr> {
    if !redirect_allowed(ctx.snap, ctx.app, ctx.pkg, &url, ctx.registered_host()) {
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
        .ok_or_else(|| client_required("The app's OAuth client is not declared."))?;
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
        .ok_or_else(|| client_required("The OAuth client has no client_id."))?
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
    let authorize = oauth_url(&ce.authorize_url, tenant.as_deref(), ctx.instance_host())
        .ok_or_else(|| {
            invalid(
                "/fields",
                "a tenant host is required for this connect method",
            )
        })?;
    if oauth_url(&ce.token_url, tenant.as_deref(), ctx.instance_host()).is_none() {
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
) -> Result<Done, &'static str> {
    const BAD: &str = "oauth_state_invalid";
    let (param, cookie) = q.get("state").map(String::as_str).zip(cookie).ok_or(BAD)?;
    let pf = Flow::open(state, param).ok_or(BAD)?;
    let flow = Flow::open(state, cookie).ok_or(BAD)?;
    if pf.nonce != flow.nonce || flow.user_id != caller.user_id {
        return Err(BAD);
    }
    if q.contains_key("error") {
        return Err("oauth_exchange_failed");
    }
    run_callback(state, caller, q, flow).await.map_err(|e| {
        if let ConnectErr::Api(a) = &e {
            tracing::warn!(code = a.code(), "connect callback failed");
        }
        e.code()
    })
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
    let (row, missing) = ctx.finalize(caller, method, kind, g).await?;
    Ok(Done::Connected(row.id, missing))
}

/// Resolve an OAuth endpoint: fixed URLs as-is; templates render with the tenant host and must
/// stay `https` inside their zone (SEC-17 / I-20).
fn oauth_url(
    u: &UrlOrTemplate,
    tenant: Option<&str>,
    instance_host: Option<&str>,
) -> Option<String> {
    match u {
        UrlOrTemplate::Url(u) if u.contains(aurora_registry::manifest::INSTANCE_HOST) => {
            probe::instance_url(u, instance_host?)
        }
        UrlOrTemplate::Url(u) => Some(u.clone()),
        UrlOrTemplate::Templated(z) => {
            let url = probe::render_url(&z.template, tenant)?;
            probe::in_zone(&url, &z.zone).then_some(url)
        }
    }
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
    let url = oauth_url(&ce.token_url, tenant, ctx.instance_host()).ok_or(ConnectErr::Exchange)?;
    let url = env.prepare(&url).map_err(|_| ConnectErr::Exchange)?;
    // A registered instance is dialled through the guard that honours `gateway.allowed_private`.
    let http = if ctx.instance.is_some() {
        &env.http_instance
    } else {
        &env.http
    };
    let get = |k: &str| {
        cf.1.get(k)
            .or_else(|| cf.0.get(k))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let mut params: BTreeMap<String, String> = BTreeMap::from([
        ("grant_type".into(), "authorization_code".into()),
        ("code".into(), code.to_owned()),
        ("redirect_uri".into(), redirect_uri(ctx.state)),
    ]);
    if let Some(v) = &flow.pkce_verifier {
        params.insert("code_verifier".into(), v.clone());
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
    req = match ce.body_format {
        BodyFormat::Form => req.form(&params),
        BodyFormat::Json => req.json(&params),
    };
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
