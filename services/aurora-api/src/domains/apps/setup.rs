//! Setup status and connect plan (P5-UX-2, D-321, D-330, C-931, C-955).
//!
//! [`setup_status`] is the one function: which ways to connect an app work here (a workspace or
//! an organization), what is missing, and who can fix it. `GET /v1/apps/{app}/setup` returns it
//! whole; `GET /v1/apps/{app}/connect-plan` returns [`ConnectPlan`], a projection of the same
//! value, so the two can never disagree. OAuth client readiness calls the very query the connect
//! flow resolves with (`connections::usable_clients`: workspace, then organization, then
//! instance; enabled only). Client secrets, client ids and e-mails never appear in an answer;
//! instance admins are never named.

use super::dto::AppFamily;
use super::service::{find_app, snapshot};
use crate::app::AppState;
use crate::http::who_can::{WhoCan, WhoCanRole};
use aurora_core::{ApiError, dash_paths};
use aurora_db::repo::{app_instances, connections, contacts, oauth_clients};
use aurora_registry::manifest::{Audience, Connect, ScopePresets};
use aurora_registry::snapshot::{AppEntry, PackageEntry};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

/// The level a setup status is computed for, with what the caller may do there. The route has
/// already authorized it (`can_own` for a workspace, owner or admin for an organization).
#[derive(Debug, Clone, Copy)]
pub struct SetupView {
    pub org: Uuid,
    pub workspace: Option<Uuid>,
    /// The caller is an organization owner or admin (an implicit owner of every workspace).
    pub org_admin: bool,
}

/// What a method still needs before people here can use it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SetupNeed {
    /// An enabled OAuth client the connect flow can resolve.
    OauthClient,
    /// A registered self-hosted server (the app has no hosted default).
    Instance,
    /// Material only an administrator holds (see `audience`).
    AdminMaterial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MethodAudience {
    Person,
    Admin,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MethodRef {
    pub id: String,
    pub label: String,
}

/// The scopes each journey choice requests (C-956), already defaulted.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MethodScopePresets {
    pub read: Vec<String>,
    pub write: Vec<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SetupMethod {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    /// `oauth2` | `fields` | `hook`.
    pub kind: String,
    pub audience: MethodAudience,
    pub recommended: bool,
    /// The method works for people here.
    pub ready: bool,
    pub needs: Vec<SetupNeed>,
    /// OAuth methods: the package's client key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,
    /// Present when something is missing: who can fix it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub who_can: Option<WhoCan>,
    pub scope_presets: MethodScopePresets,
}

/// An OAuth client by reference: never its id (the provider's), its secret or its fields.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ClientRef {
    pub id: Uuid,
    pub client_key: String,
    pub enabled: bool,
    pub used_by_methods: Vec<MethodRef>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InstanceClients {
    pub set_up: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SetupClients {
    /// This workspace's own clients; `null` in an organization view.
    pub workspace: Option<Vec<ClientRef>>,
    pub organization: Option<Vec<ClientRef>>,
    pub instance: InstanceClients,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SetupInstance {
    pub id: Uuid,
    pub label: String,
    pub base_url: String,
    /// `instance` (registered for every organization or shipped by a package) | `organization`.
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SetupValues {
    /// The redirect URI to register at the provider.
    pub callback_url: String,
    /// Only when the app's package reads `org_external_id` (e.g. `aws-keys`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
}

/// What the caller can do at this level (the page renders an action or the `who_can` sentence).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SetupCan {
    pub register_workspace_client: bool,
    pub register_org_client: bool,
    pub register_instance: bool,
    pub set_availability: bool,
    pub connect_workspace: bool,
    pub connect_org: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AppSetupStatus {
    pub app_id: String,
    pub name: String,
    pub family: Option<AppFamily>,
    pub available: bool,
    /// Present when `available` is false: who can allow the app.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_who_can: Option<WhoCan>,
    pub methods: Vec<SetupMethod>,
    /// The first ready person method: recommended, then oauth2, fields, hook; `null` if none.
    pub suggested_method: Option<String>,
    pub clients: SetupClients,
    pub instances: Vec<SetupInstance>,
    pub values: SetupValues,
    pub can: SetupCan,
}

/// The journey's view of [`AppSetupStatus`] (C-955): the same methods and availability, nothing else.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConnectPlan {
    pub app_id: String,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_who_can: Option<WhoCan>,
    pub methods: Vec<SetupMethod>,
    pub suggested_method: Option<String>,
}

impl From<AppSetupStatus> for ConnectPlan {
    fn from(s: AppSetupStatus) -> Self {
        Self {
            app_id: s.app_id,
            available: s.available,
            available_who_can: s.available_who_can,
            methods: s.methods,
            suggested_method: s.suggested_method,
        }
    }
}

fn label_of(c: &Connect) -> String {
    c.label().unwrap_or_else(|| c.id()).to_owned()
}

/// The methods that use the package client `key` (C-946).
#[must_use]
pub fn used_by_methods(app: &AppEntry, key: &str) -> Vec<MethodRef> {
    app.connect
        .iter()
        .filter_map(|c| match c {
            Connect::Oauth2(o) if o.client == key => Some(MethodRef {
                id: o.id.clone(),
                label: label_of(c),
            }),
            _ => None,
        })
        .collect()
}

fn kind_of(c: &Connect) -> &'static str {
    match c {
        Connect::Oauth2(_) => "oauth2",
        Connect::Fields(_) => "fields",
        Connect::Hook(_) => "hook",
    }
}

/// Suggestion order: recommended first, then oauth2, fields, hook.
fn rank(c: &Connect) -> (u8, u8) {
    (
        u8::from(!c.recommended()),
        match c {
            Connect::Oauth2(_) => 0,
            Connect::Fields(_) => 1,
            Connect::Hook(_) => 2,
        },
    )
}

#[must_use]
pub fn audience_of(c: &Connect) -> MethodAudience {
    match c.audience() {
        Audience::Person => MethodAudience::Person,
        Audience::Admin => MethodAudience::Admin,
    }
}

#[must_use]
pub fn presets(c: &Connect) -> MethodScopePresets {
    let ScopePresets { read, write } = c.effective_scope_presets();
    MethodScopePresets { read, write }
}

fn client_refs(app: &AppEntry, rows: &[oauth_clients::ClientLevel], level: &str) -> Vec<ClientRef> {
    rows.iter()
        .filter(|r| r.level == level)
        .map(|r| ClientRef {
            id: r.id,
            client_key: r.client_key.clone(),
            enabled: r.enabled,
            used_by_methods: used_by_methods(app, &r.client_key),
        })
        .collect()
}

fn base_url(host: &str, base_path: Option<&str>) -> String {
    format!(
        "https://{host}{}",
        base_path
            .map(|p| format!("/{}", p.trim_matches('/')))
            .unwrap_or_default()
    )
}

/// The one function (D-330): the setup status of `app_id` at `view`.
pub async fn setup_status(
    state: &AppState,
    app_id: &str,
    view: SetupView,
) -> Result<AppSetupStatus, ApiError> {
    let snap = snapshot(state)
        .await?
        .ok_or_else(|| ApiError::not_found("App not found."))?;
    let (app, pkg): (&AppEntry, &PackageEntry) = find_app(&snap, app_id)?;
    let (org_slug, _) = contacts::slugs(&state.pool, view.org, view.workspace)
        .await?
        .ok_or_else(|| ApiError::not_found("Organization not found."))?;

    // Availability: the check the connect route makes.
    let available = match view.workspace {
        Some(ws) => {
            crate::domains::connect::service::available_in_workspace(state, view.org, ws, app_id)
                .await?
        }
        None => connections::org_app_available(&state.pool, view.org, app_id).await?,
    };
    let available_who_can = if available {
        None
    } else {
        Some(
            WhoCan::build(
                state,
                WhoCanRole::OrgAdmin,
                view.org,
                None,
                "allow_app",
                view.org_admin
                    .then(|| dash_paths::org_apps(&org_slug, "apps")),
            )
            .await,
        )
    };

    // Registered self-hosted servers this organization can use.
    let registered: Vec<_> = if app.instances.is_some() {
        app_instances::list_for_org(&state.pool, view.org, Some(app_id), None, 50)
            .await?
            .into_iter()
            .filter(|i| i.status == "active")
            .collect()
    } else {
        Vec::new()
    };
    let hosted_default = app
        .instances
        .as_ref()
        .is_none_or(|i| i.default_host.is_some());

    let keys: Vec<String> = app
        .connect
        .iter()
        .filter_map(|c| match c {
            Connect::Oauth2(o) => Some(o.client.clone()),
            _ => None,
        })
        .collect();
    let levels = oauth_clients::levels_for_keys(
        &state.pool,
        &pkg.package_id,
        &keys,
        view.org,
        view.workspace,
    )
    .await?;

    let mut methods = Vec::new();
    for c in &app.connect {
        let mut needs = Vec::new();
        let mut client_key = None;
        let mut who: Option<(&str, WhoCanRole, Option<String>)> = None;
        if let Connect::Oauth2(o) = c {
            client_key = Some(o.client.clone());
            // The connect flow's own resolution, for the hosted default and for each registered
            // server (a server-bound client serves only its server).
            let mut ok = false;
            if hosted_default {
                ok = !connections::usable_clients(
                    &state.pool,
                    &pkg.package_id,
                    &o.client,
                    view.org,
                    view.workspace,
                    None,
                )
                .await?
                .is_empty();
            }
            if !ok {
                for i in &registered {
                    if !connections::usable_clients(
                        &state.pool,
                        &pkg.package_id,
                        &o.client,
                        view.org,
                        view.workspace,
                        Some(i.id),
                    )
                    .await?
                    .is_empty()
                    {
                        ok = true;
                        break;
                    }
                }
            }
            if !ok {
                needs.push(SetupNeed::OauthClient);
                who = Some((
                    "register_oauth_client",
                    WhoCanRole::OrgAdmin,
                    view.org_admin
                        .then(|| dash_paths::org_oauth_clients(&org_slug)),
                ));
            }
        }
        if !hosted_default && registered.is_empty() {
            needs.push(SetupNeed::Instance);
            who = Some((
                "register_instance",
                WhoCanRole::OrgAdmin,
                view.org_admin
                    .then(|| dash_paths::org_self_hosted(&org_slug)),
            ));
        }
        let who_can = match who {
            Some((action, role, path)) => {
                Some(WhoCan::build(state, role, view.org, None, action, path).await)
            }
            None => None,
        };
        methods.push(SetupMethod {
            id: c.id().to_owned(),
            label: label_of(c),
            description: c.description().map(str::to_owned),
            kind: kind_of(c).to_owned(),
            audience: audience_of(c),
            recommended: c.recommended(),
            ready: available && needs.is_empty(),
            needs,
            client_key,
            who_can,
            scope_presets: presets(c),
        });
    }

    let suggested_method = app
        .connect
        .iter()
        .zip(&methods)
        .filter(|(c, m)| m.ready && c.audience() == Audience::Person)
        .min_by_key(|(c, _)| rank(c))
        .map(|(c, _)| c.id().to_owned());

    let instance_set_up = levels.iter().any(|r| r.level == "instance" && r.enabled);
    let clients = SetupClients {
        workspace: view
            .workspace
            .map(|_| client_refs(app, &levels, "workspace")),
        organization: Some(client_refs(app, &levels, "organization")),
        instance: InstanceClients {
            set_up: instance_set_up,
        },
    };

    let external_id = if pkg.context.iter().any(|c| c == "org_external_id") {
        connections::org_external_id(&state.pool, view.org).await?
    } else {
        None
    };

    let instances = registered
        .iter()
        .map(|i| SetupInstance {
            id: i.id,
            label: i.label.clone(),
            base_url: base_url(&i.host, i.base_path.as_deref()),
            scope: if i.org_id.is_some() {
                "organization".to_owned()
            } else {
                "instance".to_owned()
            },
        })
        .collect();

    Ok(AppSetupStatus {
        app_id: app.app_id.clone(),
        name: app.about.name.clone(),
        family: app.about.family.as_ref().map(|f| AppFamily {
            id: f.id.clone(),
            name: f.name.clone(),
            variant_label: f.variant_label.clone(),
            instance_bound: app.instances.is_some(),
        }),
        available,
        available_who_can,
        methods,
        suggested_method,
        clients,
        instances,
        values: SetupValues {
            callback_url: format!(
                "{}/v1/oauth/callback",
                state.config.external_url.trim_end_matches('/')
            ),
            external_id,
        },
        can: SetupCan {
            register_workspace_client: view.workspace.is_some(),
            register_org_client: view.org_admin,
            register_instance: view.org_admin && app.instances.is_some(),
            set_availability: view.org_admin,
            connect_workspace: view.workspace.is_some() && available,
            connect_org: view.org_admin && available,
        },
    })
}

/// An app people cannot sign in to for lack of an OAuth client (C-924).
#[derive(Debug, Clone)]
pub struct SignInGap {
    pub app_id: String,
    pub name: String,
    pub package: String,
    /// The OAuth client key a person's sign-in method reads (the first one missing).
    pub client_key: String,
    /// Another way for a person to connect works (a token), so this is only informational.
    pub token_only: bool,
}

/// The apps of the active catalogue that have a method for a person that needs an OAuth client
/// which does not exist for `org`, computed by [`setup_status`] (C-924). An app whose people
/// methods are all blocked by the client is a gap to fix; one with another working way is
/// `token_only`. Apps blocked by something else (a self-hosted server) are not listed.
pub async fn sign_in_gaps(state: &AppState, org: Uuid) -> Result<Vec<SignInGap>, ApiError> {
    let Some(snap) = snapshot(state).await? else {
        return Ok(Vec::new());
    };
    let view = SetupView {
        org,
        workspace: None,
        org_admin: true,
    };
    let mut out = Vec::new();
    for app in &snap.apps {
        let status = setup_status(state, &app.app_id, view).await?;
        let people: Vec<&SetupMethod> = status
            .methods
            .iter()
            .filter(|m| m.audience == MethodAudience::Person)
            .collect();
        let Some(blocked) = people
            .iter()
            .find(|m| m.needs == [SetupNeed::OauthClient] && m.client_key.is_some())
        else {
            continue;
        };
        let works = people.iter().any(|m| m.needs.is_empty());
        let only_client = people
            .iter()
            .all(|m| m.needs.is_empty() || m.needs == [SetupNeed::OauthClient]);
        if !works && !only_client {
            continue;
        }
        out.push(SignInGap {
            app_id: app.app_id.clone(),
            name: status.name,
            package: app.package_id.clone(),
            client_key: blocked.client_key.clone().unwrap_or_default(),
            token_only: works,
        });
    }
    Ok(out)
}
