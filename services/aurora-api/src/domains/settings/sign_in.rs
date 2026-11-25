//! The `sign_in` and `email` sections (SET-4, D-235(b), D-236(g), D-237(g), D-238(b), D-309):
//! provider entry rules, what loosens sign-in, the preview warnings, the discovery and address
//! checks on a save, the provider test (C-780) and the test e-mail (C-781).
//!
//! Sign-in is security-critical, so the rules here are deliberately strict:
//! * a client secret is sealed and write-only; no environment variable or file is ever named from
//!   the dashboard (SEC-89, SEC-90): only a provider Helm used to define keeps its
//!   `client_secret_env`, and only while its issuer and client id are unchanged;
//! * `{ "$secret": "keep" }` never carries a stored secret to another issuer or client id;
//! * a dashboard issuer is `https` (or a loopback/private literal the operator opened in
//!   `gateway.allowed_private`) and every endpoint its discovery names passes the destination
//!   guard;
//! * a save that would leave the writing admin with no identity that can still sign in is refused.

use super::dto::{LooseningItem, SettingWarning};
use super::rules::EntryProblem;
use super::service::{field, invalid};
use crate::app::AppState;
use aurora_config::{Effective, OidcProvider, Source};
use aurora_core::ApiError;
use aurora_db::repo::identities;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use utoipa::ToSchema;
use uuid::Uuid;

pub const PROVIDERS_KEY: &str = "auth.oidc_providers";

/// Keys of a provider entry (the typed `OidcProvider`).
const OIDC_KEYS: &[&str] = &[
    "id",
    "display_name",
    "issuer",
    "issuer_url",
    "client_id",
    "client_secret",
    "client_secret_env",
    "scopes",
    "require_email_verified",
    "allowed_email_domains",
    "trusted_email_domains",
    "link_existing_users",
    "jit",
    "role_claim",
    "role_claim_source",
    "require_role",
    "role_mappings",
    "group_claim",
    "group_claim_source",
];

fn credential_like(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    k.contains("secret") && k != "client_secret" && k != "client_secret_env"
        || k.contains("password")
        || k.contains("token")
        || k.ends_with("_file")
        || k.ends_with("_path")
}

fn is_marker(v: &Value) -> bool {
    v.as_object()
        .is_some_and(|m| m.len() == 1 && m.contains_key("$secret"))
}

/// Shape checks of a provider entry that must run on the JSON, before it is deserialized: a key
/// that is not part of a provider is refused by name, and the only field that may carry a secret
/// marker is `client_secret`.
pub fn entry_problems(entry: &Value) -> Vec<EntryProblem> {
    let mut out = Vec::new();
    let Some(map) = entry.as_object() else {
        return out;
    };
    for (k, v) in map {
        if !OIDC_KEYS.contains(&k.as_str()) {
            let (code, message) = if credential_like(k) {
                (
                    "credential_in_entry",
                    format!(
                        "`{k}` looks like a credential or a file reference; the client secret is `client_secret`, entered once"
                    ),
                )
            } else {
                ("key_unknown", format!("`{k}` is not a provider setting"))
            };
            out.push(EntryProblem {
                field: k.clone(),
                code,
                message,
            });
        } else if k != "client_secret" && contains_marker(v) {
            out.push(EntryProblem {
                field: k.clone(),
                code: "invalid_value",
                message: "only `client_secret` is a secret".into(),
            });
        }
    }
    out
}

fn contains_marker(v: &Value) -> bool {
    if is_marker(v) {
        return true;
    }
    match v {
        Value::Object(m) => m.values().any(contains_marker),
        Value::Array(a) => a.iter().any(contains_marker),
        _ => false,
    }
}

fn id_of(e: &Value) -> &str {
    e.get("id").and_then(Value::as_str).unwrap_or_default()
}

fn str_of<'a>(e: &'a Value, k: &str) -> &'a str {
    e.get(k).and_then(Value::as_str).unwrap_or_default()
}

/// The stored provider with this id.
fn stored_entry<'a>(old: &'a Map<String, Value>, id: &str) -> Option<&'a Value> {
    old.get(PROVIDERS_KEY)
        .and_then(Value::as_array)?
        .iter()
        .find(|e| id_of(e) == id)
}

/// SEC-90 for provider entries, run on the submitted dashboard entries against the stored ones:
/// a plain-text secret replaces any `client_secret_env`; an entry that still names an environment
/// variable must be the very entry that already did, with the same issuer and client id (a
/// provider Helm used to define, handed to the dashboard) — never a new reference, never a kept
/// reference pointed at another issuer. Returns the field errors.
pub fn prepare_entries(norm: &mut Map<String, Value>, old: &Map<String, Value>) -> Vec<Value> {
    let mut fields = Vec::new();
    let Some(Value::Array(items)) = norm.get_mut(PROVIDERS_KEY) else {
        return fields;
    };
    for e in items.iter_mut() {
        let id = id_of(e).to_owned();
        let Some(map) = e.as_object_mut() else {
            continue;
        };
        let replaces = map
            .get("client_secret")
            .and_then(|s| s.get("$secret"))
            .and_then(Value::as_str)
            .is_some_and(|w| w != "keep");
        if replaces {
            map.remove("client_secret_env");
            continue;
        }
        let env = map
            .get("client_secret_env")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if env.is_empty() {
            map.remove("client_secret_env");
            continue;
        }
        let same = stored_entry(old, &id).is_some_and(|o| {
            str_of(o, "client_secret_env") == env
                && str_of(o, "issuer") == map.get("issuer").and_then(Value::as_str).unwrap_or("")
                && str_of(o, "client_id")
                    == map.get("client_id").and_then(Value::as_str).unwrap_or("")
        });
        if !same {
            fields.push(field(
                format!("/{PROVIDERS_KEY}/{id}/client_secret_env"),
                "dashboard_reference_refused",
                "an environment variable cannot be named from the dashboard; enter the client secret instead",
            ));
        }
    }
    fields
}

/// SEC-90: `keep` of a provider's sealed secret is refused when the issuer or the client id
/// changed (the secret must be entered again). Other fields (roles, domains, scopes …) may change.
pub fn keep_retargets(new: &Value, old: &Value) -> bool {
    new.get("client_secret")
        .and_then(|s| s.get("$secret"))
        .and_then(Value::as_str)
        == Some("keep")
        && (str_of(new, "issuer") != str_of(old, "issuer")
            || str_of(new, "client_id") != str_of(old, "client_id"))
}

// ------------------------------------------------------------------------------- loosening

fn org_rank(r: &str) -> u8 {
    match r {
        "owner" => 3,
        "admin" => 2,
        "member" => 1,
        _ => 0,
    }
}

fn ws_rank(r: &str) -> u8 {
    match r {
        "owner" => 3,
        "approver" => 2,
        "member" => 1,
        _ => 0,
    }
}

/// What a provider's role mappings grant: instance admin by claim value, organization roles by
/// (value, org), workspace roles by (value, org, workspace).
#[derive(Default)]
struct Grants {
    admin: BTreeSet<String>,
    org: BTreeMap<(String, String), u8>,
    ws: BTreeMap<(String, String, String), u8>,
}

fn grants(p: &OidcProvider) -> Grants {
    let mut g = Grants::default();
    for m in &p.role_mappings {
        if m.instance_admin {
            g.admin.insert(m.value.clone());
        }
        if let Some(o) = &m.org {
            g.org
                .insert((m.value.clone(), o.slug.clone()), org_rank(&o.role));
            for w in &m.workspaces {
                g.ws.insert(
                    (m.value.clone(), o.slug.clone(), w.slug.clone()),
                    ws_rank(&w.role),
                );
            }
        }
    }
    g
}

fn widened<K: Ord>(before: &BTreeMap<K, u8>, after: &BTreeMap<K, u8>) -> bool {
    after
        .iter()
        .any(|(k, r)| before.get(k).is_none_or(|b| r > b))
}

/// Why a provider's settings, changed from `before`, let more people in (or in as more); `None`
/// for a change that does not loosen. A new provider loosens by being added.
fn provider_loosens(before: Option<&OidcProvider>, after: &OidcProvider) -> Option<String> {
    let Some(b) = before else {
        return Some("a sign-in provider is added".into());
    };
    let (gb, ga) = (grants(b), grants(after));
    if ga.admin.difference(&gb.admin).next().is_some() {
        return Some("a role mapping grants instance admin".into());
    }
    if widened(&gb.org, &ga.org) {
        return Some("a role mapping grants an organization role (or a wider one)".into());
    }
    if widened(&gb.ws, &ga.ws) {
        return Some("a role mapping grants a workspace role (or a wider one)".into());
    }
    let jit_widened = match (&b.jit, &after.jit) {
        (None, Some(_)) => true,
        (Some(x), Some(y)) => {
            x.org_slug != y.org_slug
                || (x.role != "admin" && y.role == "admin")
                || (x.workspace_role != "approver" && y.workspace_role == "approver")
                || y.workspace_slugs
                    .iter()
                    .any(|w| !x.workspace_slugs.contains(w))
        }
        _ => false,
    };
    if jit_widened {
        return Some("just-in-time provisioning is added or widened".into());
    }
    if after
        .trusted_email_domains
        .iter()
        .any(|d| !b.trusted_email_domains.contains(d))
    {
        return Some("a trusted e-mail domain is added".into());
    }
    if !b.links_existing_users() && after.links_existing_users() {
        return Some("sign-ins may link to existing accounts".into());
    }
    let allow_widened =
        (!b.allowed_email_domains.is_empty() && after.allowed_email_domains.is_empty())
            || after.allowed_email_domains.iter().any(|d| {
                !b.allowed_email_domains.is_empty() && !b.allowed_email_domains.contains(d)
            });
    if allow_widened {
        return Some("more e-mail domains may sign in".into());
    }
    if b.require_role && !after.require_role {
        return Some("a sign-in without a mapped role is allowed".into());
    }
    None
}

fn providers(e: &Effective) -> &[OidcProvider] {
    &e.raw.auth.oidc_providers
}

/// The loosening items of a `sign_in` change at the level of provider entries (C-770, D-237(g)).
pub fn entry_loosening(before: &Effective, after: &Effective) -> Vec<LooseningItem> {
    let mut out = Vec::new();
    for a in providers(after) {
        let b = providers(before).iter().find(|p| p.id == a.id);
        if let Some(reason) = provider_loosens(b, a) {
            out.push(LooseningItem {
                key: PROVIDERS_KEY.into(),
                entry_id: Some(a.id.clone()),
                reason,
            });
        }
    }
    out
}

// -------------------------------------------------------------------------------- warnings

fn retargeted(b: &OidcProvider, a: &OidcProvider) -> bool {
    b.issuer != a.issuer || b.client_id != a.client_id
}

/// The providers the admin has an identity at (lowest id first).
pub async fn caller_providers(state: &AppState, user: Uuid) -> Vec<String> {
    let mut ids: Vec<String> = identities::by_user(&state.pool, user)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(p, _)| p)
        // The development IdP is not a configurable provider (it is never stranded by a save).
        .filter(|p| p != aurora_config::DEV_IDP_PROVIDER_ID)
        .collect();
    ids.dedup();
    ids
}

/// Do all of the caller's providers stop working in `after` (removed, or pointed at another
/// issuer or client)? An admin with no identity (a break-glass Helm provider only) is never stranded
/// by this rule.
fn strands(own: &[String], before: &Effective, after: &Effective) -> bool {
    !own.is_empty()
        && own.iter().all(|id| {
            let b = providers(before).iter().find(|p| &p.id == id);
            let a = providers(after).iter().find(|p| &p.id == id);
            match (b, a) {
                (_, None) => true,
                (Some(b), Some(a)) => retargeted(b, a),
                (None, Some(_)) => false,
            }
        })
}

/// Preview warnings of the `sign_in` section: `grants_instance_admin`, `removes_own_provider` and
/// `identities_orphaned` (C-780).
pub async fn warnings(
    state: &AppState,
    caller: Option<Uuid>,
    before: &Effective,
    after: &Effective,
) -> Result<Vec<SettingWarning>, ApiError> {
    let mut out = Vec::new();
    for a in providers(after) {
        let b = providers(before).iter().find(|p| p.id == a.id);
        let (ga, gb) = (grants(a), b.map(grants).unwrap_or_default());
        let added: Vec<&String> = ga.admin.difference(&gb.admin).collect();
        if !added.is_empty() {
            out.push(SettingWarning {
                entry_id: None,
                code: "grants_instance_admin".into(),
                key: Some(PROVIDERS_KEY.into()),
                message: format!(
                    "Provider `{}`: {} would become instance admin{}.",
                    a.id,
                    if added.len() == 1 {
                        "everyone whose claim has this value".to_owned()
                    } else {
                        "everyone whose claim has one of these values".to_owned()
                    },
                    if let [one] = added.as_slice() {
                        format!(" (`{one}`)")
                    } else {
                        format!(
                            " ({})",
                            added
                                .iter()
                                .map(|v| format!("`{v}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    }
                ),
            });
        }
    }
    // Identities that can no longer sign in: a provider removed, or its issuer / client changed.
    let gone: Vec<String> = providers(before)
        .iter()
        .filter(|b| {
            providers(after)
                .iter()
                .find(|a| a.id == b.id)
                .is_none_or(|a| retargeted(b, a))
        })
        .map(|b| b.id.clone())
        .collect();
    if !gone.is_empty() {
        let counts = identities::counts_by_provider(&state.pool, &gone)
            .await
            .map_err(ApiError::from)?;
        for (id, n) in counts {
            out.push(SettingWarning {
                entry_id: None,
                code: "identities_orphaned".into(),
                key: Some(PROVIDERS_KEY.into()),
                message: format!(
                    "{n} account{} signed in through `{id}` and will no longer be able to sign in through it (the provider is removed or points at another issuer or client).",
                    if n == 1 { "" } else { "s" },
                ),
            });
        }
    }
    if let Some(user) = caller {
        let own = caller_providers(state, user).await;
        if strands(&own, before, after) {
            out.push(SettingWarning {
                entry_id: None,
                code: "removes_own_provider".into(),
                key: Some(PROVIDERS_KEY.into()),
                message: "This removes or re-points the provider you sign in with. You would not be able to sign in again through it; the save is refused unless another provider of yours stays.".into(),
            });
        }
    }
    Ok(out)
}

/// A save by an admin may not strand that admin (D-238(b), D-309): refused when every provider the
/// admin has an identity at is removed or re-pointed. The operator's shell (`Via::Cli`) is exempt.
pub async fn guard_own_provider(
    state: &AppState,
    user: Uuid,
    before: &Effective,
    after: &Effective,
) -> Result<(), ApiError> {
    let own = caller_providers(state, user).await;
    if strands(&own, before, after) {
        return Err(invalid(vec![field(
            format!("/{PROVIDERS_KEY}"),
            "removes_own_provider",
            "this would remove or re-point every provider you can sign in with; keep one of them, or sign in through another account (or `aurora-api settings` from the shell) to change it",
        )]));
    }
    Ok(())
}

// ------------------------------------------------------------------------------- extra view

/// `extra` of the section (C-780): how many providers Helm defines, and the one the caller signs
/// in with.
pub async fn extra(state: &AppState, caller: Option<Uuid>) -> Value {
    let snap = state.settings.snapshot();
    let from_helm = snap
        .effective
        .keys
        .get(PROVIDERS_KEY)
        .and_then(|k| k.entries.as_ref())
        .map_or(0, |es| {
            es.iter().filter(|e| e.source == Source::Helm).count()
        });
    let signed_in_with = match caller {
        Some(u) => caller_providers(state, u).await.into_iter().next(),
        None => None,
    };
    serde_json::json!({ "providers_from_helm": from_helm, "signed_in_with": signed_in_with })
}

// ------------------------------------------------------------------------ discovery and guard

/// A dashboard issuer is `https`; plain `http` only for an address literal or `localhost` (which
/// the destination guard then judges against `gateway.allowed_private`).
fn issuer_scheme_ok(issuer: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(issuer) else {
        return false;
    };
    match u.scheme() {
        "https" => true,
        "http" => u.host_str().is_some_and(|h| {
            let h = h.trim_matches(['[', ']']);
            h == "localhost" || h.ends_with(".localhost") || h.parse::<std::net::IpAddr>().is_ok()
        }),
        _ => false,
    }
}

/// What discovery of one issuer found, or the reason it failed.
pub struct Discovered {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: Option<String>,
    pub jwks_uri: String,
}

/// Discover `issuer` from the API through the destination guard (`gateway.allowed_private`
/// applies to the issuer and to every endpoint its document names).
pub async fn discover(state: &AppState, issuer: &str) -> Result<Discovered, &'static str> {
    if !issuer_scheme_ok(issuer) {
        return Err("issuer_not_https");
    }
    let guard = aurora_secrets::DialGuard::new(&state.ops().gateway_allowed_private);
    if !guard.permits_address(issuer).await {
        return Err("destination_refused");
    }
    let meta = state.oidc.discover(issuer).await?;
    let d = Discovered {
        issuer: meta.issuer().as_str().to_owned(),
        authorization_endpoint: meta.authorization_endpoint().url().to_string(),
        token_endpoint: meta.token_endpoint().map(|t| t.url().to_string()),
        jwks_uri: meta.jwks_uri().url().to_string(),
    };
    for url in [
        Some(d.authorization_endpoint.as_str()),
        d.token_endpoint.as_deref(),
        Some(d.jwks_uri.as_str()),
    ]
    .into_iter()
    .flatten()
    {
        if !guard.permits_address(url).await {
            return Err("destination_refused");
        }
    }
    Ok(d)
}

/// Before a write (outside the settings lock: DNS and discovery are awaited): every dashboard
/// provider that is new or whose issuer changed must be discoverable (422
/// `oidc_discovery_failed`), and its address and endpoints must pass the guard.
pub async fn check_submitted(
    state: &AppState,
    values: &Map<String, Value>,
) -> Result<(), ApiError> {
    let Some(list) = values.get(PROVIDERS_KEY).and_then(Value::as_array) else {
        return Ok(());
    };
    let mut fields = check_personal_targets(state, values).await?;
    let snap = state.settings.snapshot();
    let stored: Vec<Value> = snap
        .stored
        .get("sign_in", PROVIDERS_KEY)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for e in list {
        let id = id_of(e);
        let issuer = str_of(e, "issuer");
        if issuer.is_empty() {
            continue;
        }
        let unchanged = stored
            .iter()
            .any(|o| id_of(o) == id && str_of(o, "issuer") == issuer);
        if unchanged {
            continue;
        }
        if let Err(reason) = discover(state, issuer).await {
            let (code, message) = if matches!(reason, "destination_refused" | "issuer_not_https") {
                (
                    reason,
                    "this issuer is not reachable from here: it must be https (or a loopback or private address an operator opened with gateway.allowed_private), and may not be a metadata, private or link-local address",
                )
            } else {
                (
                    "oidc_discovery_failed",
                    "the issuer's OpenID discovery document could not be read",
                )
            };
            let mut f = field(format!("/{PROVIDERS_KEY}/{id}/issuer"), code, message);
            f["reason"] = Value::String(reason.into());
            fields.push(f);
        }
    }
    if fields.is_empty() {
        Ok(())
    } else {
        Err(invalid(fields))
    }
}

// ------------------------------------------------------------------------------ provider test

/// `check_submitted` is the save path's state-aware hook: the discovery checks above, and
/// (D-486) the rule that no role mapping ever targets a personal workspace. A mapping saved
/// before the rule is skipped at sign-in, so the save is where it is refused: the dashboard PUT,
/// the import and the CLI all come through here.
async fn check_personal_targets(
    state: &AppState,
    values: &Map<String, Value>,
) -> Result<Vec<Value>, ApiError> {
    let Some(list) = values.get(PROVIDERS_KEY).and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut fields = Vec::new();
    let mut checked: BTreeMap<(String, String), Option<String>> = BTreeMap::new();
    for e in list {
        let id = id_of(e).to_owned();
        let Some(mappings) = e.get("role_mappings").and_then(Value::as_array) else {
            continue;
        };
        for (j, m) in mappings.iter().enumerate() {
            let Some(org_slug) = m
                .get("org")
                .and_then(|o| o.get("slug"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let Some(workspaces) = m.get("workspaces").and_then(Value::as_array) else {
                continue;
            };
            for (k, w) in workspaces.iter().enumerate() {
                let Some(ws_slug) = w.get("slug").and_then(Value::as_str) else {
                    continue;
                };
                let name = personal_workspace_name(state, &mut checked, org_slug, ws_slug).await?;
                if let Some(name) = name {
                    fields.push(field(
                        format!("/{PROVIDERS_KEY}/{id}/role_mappings/{j}/workspaces/{k}"),
                        "role_mapping_personal_workspace",
                        format!(
                            "`{}` is a personal workspace: nobody but its person may hold a role \
                             there, so a role mapping cannot target it.",
                            name
                        ),
                    ));
                }
            }
        }
    }
    Ok(fields)
}

/// The display name of the personal workspace at `(org_slug, ws_slug)`, or `None` when it is not
/// one (or does not exist). Cached per save. A DB error here fails the save: the rule must not
/// silently pass because a read failed.
async fn personal_workspace_name(
    state: &AppState,
    checked: &mut BTreeMap<(String, String), Option<String>>,
    org_slug: &str,
    ws_slug: &str,
) -> Result<Option<String>, ApiError> {
    let key = (org_slug.to_owned(), ws_slug.to_owned());
    if let Some(hit) = checked.get(&key) {
        return Ok(hit.clone());
    }
    // The sticky `personal` marker, not the owner id (SEC-79, FIX-097): the same
    // definition `aurora_db::repo::placement::is_personal` uses.
    let hit: Option<String> = sqlx::query_scalar(
        "SELECT w.name FROM workspaces w \
         JOIN organizations o ON o.id = w.org_id \
         WHERE o.slug = $1 AND w.slug = $2 \
           AND (w.personal OR w.personal_owner_user_id IS NOT NULL) \
         LIMIT 1",
    )
    .bind(org_slug)
    .bind(ws_slug)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ApiError::internal(format!("checking role mapping targets: {e}")))?;
    checked.insert(key, hit.clone());
    Ok(hit)
}

/// `POST /v1/admin/settings/sign_in/provider-test` body (C-780).
#[derive(Deserialize, ToSchema)]
pub struct ProviderTestRequest {
    /// One provider entry, as in `auth.oidc_providers` (the secret is not needed).
    #[schema(value_type = Object)]
    pub provider: Value,
}

/// What discovery found.
#[derive(Serialize, ToSchema)]
pub struct ProviderTestResult {
    pub ok: bool,
    pub issuer: Option<String>,
    pub authorization_endpoint: Option<String>,
    pub token_endpoint: Option<String>,
    pub jwks_uri: Option<String>,
    /// `issuer_invalid`, `issuer_not_https`, `destination_refused`, `issuer_mismatch`,
    /// `not_oidc`, `http_error` or `unreachable`.
    pub error_code: Option<String>,
}

/// Discovery only: nothing is saved, no secret is used.
pub async fn provider_test(
    state: &AppState,
    req: ProviderTestRequest,
) -> Result<ProviderTestResult, ApiError> {
    let problems = entry_problems(&req.provider);
    if !problems.is_empty() {
        return Err(invalid(
            problems
                .into_iter()
                .map(|p| field(format!("/provider/{}", p.field), p.code, p.message))
                .collect(),
        ));
    }
    let issuer = str_of(&req.provider, "issuer");
    if issuer.is_empty() {
        return Err(invalid(vec![field(
            "/provider/issuer".into(),
            "invalid_value",
            "required",
        )]));
    }
    Ok(match discover(state, issuer).await {
        Ok(d) => ProviderTestResult {
            ok: true,
            issuer: Some(d.issuer),
            authorization_endpoint: Some(d.authorization_endpoint),
            token_endpoint: d.token_endpoint,
            jwks_uri: Some(d.jwks_uri),
            error_code: None,
        },
        Err(code) => ProviderTestResult {
            ok: false,
            issuer: None,
            authorization_endpoint: None,
            token_endpoint: None,
            jwks_uri: None,
            error_code: Some(code.into()),
        },
    })
}

// --------------------------------------------------------------------------------- test mail

/// `POST /v1/admin/settings/email/test` body (C-781).
#[derive(Deserialize, ToSchema)]
pub struct EmailTestRequest {
    /// The section's values as in a save; absent = the settings in force. A secret written as
    /// `{ "$secret": "<plaintext>" }` is used for this test only and never stored.
    #[schema(value_type = Option<Object>)]
    pub values: Option<Map<String, Value>>,
}

#[derive(Serialize, ToSchema)]
pub struct EmailTestResult {
    pub sent: bool,
    /// `connect`, `tls`, `auth`, `rejected`, `timeout` or `log_only`.
    pub error_code: Option<String>,
}

fn classify_mail_error(msg: &str) -> &'static str {
    let m = msg.to_ascii_lowercase();
    if m.contains("timed out") || m.contains("timeout") {
        "timeout"
    } else if m.contains("tls") || m.contains("certificate") || m.contains("handshake") {
        "tls"
    } else if m.contains("authentication") || m.contains("credentials") || m.contains("535") {
        "auth"
    } else if m.contains("permanent") || m.contains("rejected") || m.contains("550") {
        "rejected"
    } else {
        "connect"
    }
}

/// Send one test message to the caller's own address with the proposed (or effective) e-mail
/// settings. Never to anyone else, never logging the password.
pub async fn email_test(
    state: &AppState,
    to: &str,
    req: EmailTestRequest,
) -> Result<EmailTestResult, ApiError> {
    let snap = state.settings.snapshot();
    let (email, password) = match req.values {
        None => {
            let e = snap.effective.raw.email.clone();
            let p = super::live::smtp_password(&state.vault, &e).await;
            (e, p)
        }
        Some(values) => {
            let eff = super::service::proposed_effective(state, "email", &values).await?;
            let plaintext = values
                .get("email.smtp_password")
                .and_then(|v| v.get("$secret"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let e = eff.raw.email.clone();
            let cur = &snap.effective.raw.email;
            // A stored or Helm-set password only ever goes where it is already configured to go
            // (SEC-90): the proposal must name the same server and user.
            let same_url = e.smtp_url.as_ref().map(|u| u.expose().clone())
                == cur.smtp_url.as_ref().map(|u| u.expose().clone());
            let same_target = same_url && e.smtp_username == cur.smtp_username;
            let typed = matches!(plaintext.as_deref(), Some(p) if p != "keep" && !p.is_empty());
            let held = plaintext.as_deref() == Some("keep") || e.smtp_password.is_some();
            if !typed && held && !same_target {
                return Err(super::service::reentry_required("/email.smtp_password"));
            }
            if !same_url {
                // A server named on the page (not Helm's) passes the destination guard.
                let guard = aurora_secrets::DialGuard::new(&state.ops().gateway_allowed_private);
                if let Some(u) = e.smtp_url.as_ref()
                    && !state.settings.source.sets("email.smtp_url")
                    && !guard.permits_address(u.expose()).await
                {
                    return Err(ApiError::unprocessable(
                        "destination_refused",
                        "This address is not reachable from here (private, loopback, link-local or metadata); an operator can open it with gateway.allowed_private.",
                    ));
                }
            }
            let p = match plaintext.as_deref() {
                Some("keep") => super::live::smtp_password(&state.vault, cur).await,
                Some(plain) if !plain.is_empty() => Ok(Some(plain.to_owned())),
                // The environment's password wins whatever the page sends (the key is locked).
                _ => Ok(e.smtp_password.as_ref().map(|p| p.expose().clone())),
            };
            (e, p)
        }
    };
    let Some(url) = email.smtp_url.as_ref() else {
        return Ok(EmailTestResult {
            sent: false,
            error_code: Some("log_only".into()),
        });
    };
    let password = match password {
        Ok(p) => p,
        Err(_) => {
            return Ok(EmailTestResult {
                sent: false,
                error_code: Some("auth".into()),
            });
        }
    };
    let credentials = email.smtp_username.clone().zip(password);
    let mailer = match crate::domains::auth::mail::SmtpMailer::new(
        url.expose(),
        email.from.as_deref().unwrap_or_default(),
        credentials,
    ) {
        Ok(m) => m,
        Err(_) => {
            return Ok(EmailTestResult {
                sent: false,
                error_code: Some("connect".into()),
            });
        }
    };
    let mail = crate::domains::auth::mail::Mail::new(
        to,
        "Aurora: test e-mail",
        "This is a test message from Aurora's e-mail settings. If you can read it, outgoing mail works.",
        &state.config.external_url,
    );
    let sent = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        crate::domains::auth::mail::Mailer::send(&mailer, mail),
    )
    .await;
    Ok(match sent {
        Ok(Ok(())) => EmailTestResult {
            sent: true,
            error_code: None,
        },
        Ok(Err(e)) => EmailTestResult {
            sent: false,
            error_code: Some(classify_mail_error(&e.0).into()),
        },
        Err(_) => EmailTestResult {
            sent: false,
            error_code: Some("timeout".into()),
        },
    })
}
