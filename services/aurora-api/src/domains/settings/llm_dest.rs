//! (P4-LLM-6, D-262) Where a dashboard LLM endpoint may point: the write rules that need the
//! instance's state (`http_needs_private_host`, `endpoint_host_claimed`, C-800) and the advice
//! about private names (`needs_allowed_private` on GET, `endpoint_resolves_private` in the
//! preview, C-813). Only dashboard entries are judged: a Helm endpoint is the operator's own
//! choice and is validated at start, never by a write.

use super::service::{field, invalid};
use crate::app::AppState;
use crate::domains::plugins::registry;
use aurora_config::{EntrySource, LlmEndpoint, LlmSection};
use aurora_core::ApiError;
use aurora_core::ranges::{PrivateAllow, is_metadata, is_special};
use aurora_policy::host::HostPattern;
use aurora_registry::snapshot::ClaimStatus;
use serde_json::Value;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Name resolution for the advice (C-813): at most this long per name …
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(2);
/// … and an answer is reused this long (a settings page polling its own GET must not resolve on
/// every request).
const RESOLVE_TTL: Duration = Duration::from_secs(60);

type Cache = HashMap<String, (Instant, Option<Vec<IpAddr>>)>;
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The scheme, host and port an entry's `base_url` names.
fn origin_of(base_url: &str) -> Option<(bool, String, u16)> {
    let u = reqwest::Url::parse(base_url).ok()?;
    let host = u.host_str()?.trim_matches(['[', ']']).to_ascii_lowercase();
    Some((
        u.scheme() == "http",
        host,
        u.port_or_known_default().unwrap_or(443),
    ))
}

/// The addresses `host` resolves to (cached 60 s); `None` when it does not resolve now.
async fn resolve(host: &str, port: u16) -> Option<Vec<IpAddr>> {
    let key = format!("{host}:{port}");
    if let Ok(c) = CACHE.lock()
        && let Some((at, v)) = c.get(&key)
        && at.elapsed() < RESOLVE_TTL
    {
        return v.clone();
    }
    let found =
        match tokio::time::timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host((host, port))).await {
            Ok(Ok(a)) => Some(a.map(|s| s.ip()).collect::<Vec<_>>()),
            _ => None,
        };
    if let Ok(mut c) = CACHE.lock() {
        c.retain(|_, (at, _)| at.elapsed() < RESOLVE_TTL);
        c.insert(key, (Instant::now(), found.clone()));
    }
    found
}

/// Does `host` resolve into a range the guard refuses and `allow` does not open?
async fn resolves_private(allow: &PrivateAllow, host: &str, port: u16) -> bool {
    let refused = |ip: IpAddr| (is_special(ip) || is_metadata(ip)) && !allow.permits(ip, host);
    if let Ok(ip) = host.parse::<IpAddr>() {
        return refused(ip);
    }
    resolve(host, port)
        .await
        .is_some_and(|ips| ips.into_iter().any(refused))
}

/// Is a plain-`http` host private and opened (loopback, or covered by `gateway.allowed_private`)?
/// A name counts when an entry names it, or when everything it resolves to is a private address an
/// entry opens.
async fn http_host_covered(allow: &PrivateAllow, host: &str, port: u16) -> bool {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return is_special(ip) && allow.permits(ip, host);
    }
    if allow.covers_host(host) {
        return true;
    }
    resolve(host, port).await.is_some_and(|ips| {
        !ips.is_empty()
            && ips
                .into_iter()
                .all(|ip| is_special(ip) && allow.permits(ip, host))
    })
}

fn parse(entry: &Value) -> Option<LlmEndpoint> {
    serde_json::from_value(entry.clone()).ok()
}

/// The host names a dashboard endpoint talks to (its provider's host or hosts), for ownership.
fn hosts_of(e: &LlmEndpoint) -> Vec<String> {
    let section = LlmSection {
        enabled_providers: aurora_llm::supported_providers()
            .iter()
            .map(|p| (*p).to_owned())
            .collect(),
        endpoints: vec![e.clone()],
        ..LlmSection::default()
    };
    aurora_llm::ProviderRegistry::from_config(&section)
        .map(|r| {
            r.endpoints()
                .iter()
                .filter(|x| x.id == e.id)
                .flat_map(|x| r.provider_for(x).map(|p| p.hosts(x)).unwrap_or_default())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// The dashboard endpoints of a write, with the `gateway.allowed_private` the write leaves in
/// force (the `limits` section of the same request counts).
fn submitted(
    state: &AppState,
    reqs: &[super::service::SectionRequest],
) -> (Vec<(String, Value)>, PrivateAllow) {
    let mut entries = Vec::new();
    let mut allowed = state.ops().gateway_allowed_private.clone();
    for r in reqs {
        if r.section == "llm"
            && let Some(list) = r.values.get("llm.endpoints").and_then(Value::as_array)
        {
            for e in list {
                let id = e.get("id").and_then(Value::as_str).unwrap_or_default();
                entries.push((id.to_owned(), e.clone()));
            }
        }
        if r.section == "limits"
            && let Some(list) = r
                .values
                .get("gateway.allowed_private")
                .and_then(Value::as_array)
        {
            allowed = list
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
        }
    }
    (entries, PrivateAllow::new(&allowed))
}

/// Judge the dashboard endpoints of a write (before the transaction: DNS is never awaited while
/// the settings lock is held). 422 `settings_invalid` with `http_needs_private_host` on
/// `/llm.endpoints/<id>/base_url` and `endpoint_host_claimed` (`app_id` on the field error).
pub async fn check_submitted(
    state: &AppState,
    reqs: &[super::service::SectionRequest],
) -> Result<(), ApiError> {
    let (entries, allow) = submitted(state, reqs);
    if entries.is_empty() {
        return Ok(());
    }
    let snap = registry::active_typed(state).await?;
    let mut fields = Vec::new();
    for (id, raw) in &entries {
        let Some(e) = parse(raw) else { continue };
        if let Some((http, host, port)) = e.base_url.as_deref().and_then(origin_of) {
            let covered = http && http_host_covered(&allow, &host, port).await;
            let source = EntrySource::Dashboard {
                host_allowed_private: covered,
            };
            for fe in aurora_config::endpoint_field_errors(&e, source)
                .into_iter()
                .filter(|f| f.code == "http_needs_private_host")
            {
                fields.push(field(
                    format!("/llm.endpoints/{id}/{}", fe.field),
                    fe.code,
                    "plain http is only allowed to a private address that gateway.allowed_private opens (or loopback); use https, or ask an operator to open the address",
                ));
            }
        }
        for host in hosts_of(&e) {
            let port = e
                .base_url
                .as_deref()
                .and_then(origin_of)
                .map_or(443, |(_, _, p)| p);
            if let Some(app_id) = claimed_by(state, snap.as_ref(), &host, port).await? {
                let mut f = field(
                    format!("/llm.endpoints/{id}/base_url"),
                    "endpoint_host_claimed",
                    format!(
                        "the host `{host}` belongs to the app `{app_id}`; remove its claim or its registration first"
                    ),
                );
                f["app_id"] = Value::String(app_id);
                fields.push(f);
            }
        }
    }
    if fields.is_empty() {
        Ok(())
    } else {
        Err(invalid(fields))
    }
}

/// The app that holds `host`: an active static claim of the registry snapshot, or an app-instance
/// registration.
async fn claimed_by(
    state: &AppState,
    snap: Option<&aurora_registry::snapshot::Snapshot>,
    host: &str,
    port: u16,
) -> Result<Option<String>, ApiError> {
    if let Some(s) = snap
        && let Some(h) = s.hosts.iter().find(|h| {
            h.claim_status == ClaimStatus::Active
                && HostPattern::lenient(&h.host).matches(host, port)
        })
    {
        return Ok(Some(h.app_id.clone()));
    }
    let held = aurora_db::repo::app_instances::visible_on_host(&state.pool, None, host)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(held.into_iter().next().map(|h| h.app_id))
}

/// (C-813) The ids of the dashboard endpoints whose host resolves into a refused range that
/// `allow` does not cover.
pub async fn needing_allowed_private(entries: &[(String, Value)], allow: &[String]) -> Vec<String> {
    let allow = PrivateAllow::new(allow);
    let mut out = Vec::new();
    for (id, raw) in entries {
        let Some(e) = parse(raw) else { continue };
        let Some((_, host, port)) = e.base_url.as_deref().and_then(origin_of) else {
            continue;
        };
        if resolves_private(&allow, &host, port).await {
            out.push(id.clone());
        }
    }
    out
}
