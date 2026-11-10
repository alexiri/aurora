//! Deterministic registry snapshot compiler (09 §9).
//!
//! Canonical form: compact JSON, object keys sorted at every level; arrays
//! sorted by id (`packages` by package_id, `oauth_clients` by
//! (package_id, client_key), `apps` by app_id, `hosts` by
//! (host, path_prefix, app_id), `reserved_hosts` lexicographic). Arrays inside
//! an app/host rule keep manifest order because order is semantic there
//! (e.g. `inject` stages). `hash` is the hex sha256 of `json`.
use std::collections::{BTreeMap, BTreeSet};

use aurora_policy::HostPattern;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::claims::{ConflictGroup, claim_key, claims, conflict_groups, static_host};
use crate::manifest::{
    About, Agent, Approval, BodyFormat, ClientAuth, ClientField, Connect, CredentialDecl, Database,
    Finalize, Grants, Hook, HostRule, INSTANCE_HOST, Inject, Instances, Intercept, Manifest,
    MetadataDecl, Passthrough, RewriteHost, Tenant, Token, Tools, Transform, UrlOrTemplate, de_opt,
    de_vec,
};

pub const FORMAT: u32 = 1;

pub(crate) fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

fn write_canonical(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push(b'{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend(Value::String(k.clone()).to_string().bytes());
                out.push(b':');
                write_canonical(&m[k], out);
            }
            out.push(b'}');
        }
        Value::Array(a) => {
            out.push(b'[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(x, out);
            }
            out.push(b']');
        }
        other => out.extend(other.to_string().bytes()),
    }
}

/// Compact JSON with sorted object keys.
pub fn canonical_bytes(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_canonical(v, &mut out);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClaimStatus {
    Active,
    Lost,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageEntry {
    pub package_id: String,
    pub version: String,
    pub digest: String,
    pub publisher: String,
    pub hooks_digest: Option<String>,
    pub egress: BTreeMap<Hook, Vec<String>>,
    pub context: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OauthClientEntry {
    pub package_id: String,
    pub client_key: String,
    pub authorize_url: UrlOrTemplate,
    pub token_url: UrlOrTemplate,
    pub pkce: bool,
    pub client_auth: ClientAuth,
    pub body_format: BodyFormat,
    pub fields: Vec<ClientField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppEntry {
    pub app_id: String,
    pub package_id: String,
    pub about: About,
    /// Present for an instance-bound app (D-74).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instances: Option<Instances>,
    pub credentials: Vec<CredentialDecl>,
    pub metadata: Vec<MetadataDecl>,
    pub connect: Vec<Connect>,
    pub token: Option<Token>,
    pub tools: Option<Tools>,
    pub approvals: BTreeMap<String, Approval>,
    pub agent: Option<Agent>,
    pub database: Option<Database>,
    /// (P4-GRANTS, D-123) copied from the manifest; absent in older snapshots = both false.
    #[serde(default)]
    pub grants: Grants,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostEntry {
    pub host: String,
    pub path_prefix: Option<String>,
    pub app_id: String,
    pub claim_status: ClaimStatus,
    pub tenant: Option<Tenant>,
    pub rewrite_host: Option<RewriteHost>,
    #[serde(default, deserialize_with = "de_vec")]
    pub inject: Vec<Inject>,
    #[serde(default, deserialize_with = "de_opt")]
    pub finalize: Option<Finalize>,
    #[serde(default, deserialize_with = "de_opt")]
    pub transform: Option<Transform>,
    #[serde(default, deserialize_with = "de_opt")]
    pub intercept: Option<Intercept>,
    #[serde(default)]
    pub passthrough: Vec<Passthrough>,
}

/// Typed view of the snapshot JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub format: u32,
    pub contract: u32,
    pub packages: Vec<PackageEntry>,
    pub oauth_clients: Vec<OauthClientEntry>,
    pub apps: Vec<AppEntry>,
    pub hosts: Vec<HostEntry>,
    /// Host entries of instance-bound apps whose host is `{instance}` (D-74). Kept apart from
    /// the static host index: they apply only to a registered app instance's exact host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instance_hosts: Vec<HostEntry>,
    pub reserved_hosts: Vec<String>,
}

impl Snapshot {
    /// The `{instance}` entries of an app (longest path prefix first).
    pub fn instance_entries(&self, app_id: &str) -> impl Iterator<Item = &HostEntry> {
        self.instance_hosts
            .iter()
            .filter(move |e| e.app_id == app_id)
    }

    pub fn from_slice(b: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(b)
    }

    pub fn app(&self, app_id: &str) -> Option<&AppEntry> {
        self.apps.iter().find(|a| a.app_id == app_id)
    }
}

pub struct ActivePackage {
    pub manifest: Manifest,
    pub digest: String,
    pub publisher: String,
    pub hooks_digest: Option<String>,
}

/// Admin decision for a [`ConflictGroup`] (`host_claim_decisions`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub claim_key: String,
    pub fingerprint: String,
    pub winner_package_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledSnapshot {
    pub json: Vec<u8>,
    pub hash: String,
    pub contract: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompileError {
    #[error("package `{package_id}` has contract {contract}, newer than compiler contract {max}")]
    UnsupportedContract {
        package_id: String,
        contract: u32,
        max: u32,
    },
    #[error("duplicate package `{0}`")]
    DuplicatePackage(String),
    #[error("duplicate app id `{0}`")]
    DuplicateApp(String),
    #[error("invalid host pattern `{host}` in app `{app_id}`")]
    InvalidHost { app_id: String, host: String },
    #[error("{} unresolved host claim conflict(s)", .0.len())]
    UnresolvedConflicts(Vec<ConflictGroup>),
    #[error("serialization failed: {0}")]
    Serialize(String),
}

fn sorted(v: &[String]) -> Vec<String> {
    v.iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn compile(
    packages: &[ActivePackage],
    decisions: &[Decision],
    reserved_hosts: &[String],
    contract: u32,
) -> Result<CompiledSnapshot, CompileError> {
    let mut seen = BTreeSet::new();
    let mut app_ids = BTreeSet::new();
    let mut all = Vec::new();
    for p in packages {
        let pk = &p.manifest.package;
        if pk.contract > contract {
            return Err(CompileError::UnsupportedContract {
                package_id: pk.id.clone(),
                contract: pk.contract,
                max: contract,
            });
        }
        if !seen.insert(pk.id.as_str()) {
            return Err(CompileError::DuplicatePackage(pk.id.clone()));
        }
        for a in &p.manifest.apps {
            if !app_ids.insert(a.about.id.as_str()) {
                return Err(CompileError::DuplicateApp(a.about.id.clone()));
            }
            for h in &a.hosts {
                if HostPattern::parse(&h.matcher.host).is_err() {
                    return Err(CompileError::InvalidHost {
                        app_id: a.about.id.clone(),
                        host: h.matcher.host.clone(),
                    });
                }
            }
        }
        all.extend(claims(&p.manifest).into_iter().map(|c| (pk.id.clone(), c)));
    }

    // Resolve conflicts: (package, app, claim_key) of every lost claim.
    let mut lost = BTreeSet::new();
    let mut unresolved = Vec::new();
    for g in conflict_groups(&all) {
        let winner = decisions
            .iter()
            .find(|d| {
                d.claim_key == g.claim_key
                    && d.fingerprint == g.fingerprint
                    && g.members
                        .iter()
                        .any(|m| m.package_id == d.winner_package_id)
            })
            .map(|d| d.winner_package_id.as_str());
        match winner {
            Some(w) => lost.extend(
                g.members
                    .iter()
                    .filter(|m| m.package_id != w)
                    .map(|m| (m.package_id.clone(), m.app_id.clone(), m.claim_key.clone())),
            ),
            None => unresolved.push(g),
        }
    }
    if !unresolved.is_empty() {
        return Err(CompileError::UnresolvedConflicts(unresolved));
    }

    let mut snap = Snapshot {
        format: FORMAT,
        contract,
        packages: Vec::new(),
        oauth_clients: Vec::new(),
        apps: Vec::new(),
        hosts: Vec::new(),
        instance_hosts: Vec::new(),
        reserved_hosts: reserved_hosts
            .iter()
            .map(|h| h.to_ascii_lowercase())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    };
    for p in packages {
        let m = &p.manifest;
        let pid = &m.package.id;
        snap.packages.push(PackageEntry {
            package_id: pid.clone(),
            version: m.package.version.clone(),
            digest: p.digest.clone(),
            publisher: p.publisher.clone(),
            hooks_digest: p.hooks_digest.clone(),
            egress: m
                .permissions
                .egress
                .iter()
                .map(|(k, v)| (*k, sorted(v)))
                .collect(),
            context: sorted(&m.permissions.context),
        });
        for c in &m.oauth_clients {
            snap.oauth_clients.push(OauthClientEntry {
                package_id: pid.clone(),
                client_key: c.id.clone(),
                authorize_url: c.authorize_url.clone(),
                token_url: c.token_url.clone(),
                pkce: c.pkce,
                client_auth: c.client_auth,
                body_format: c.body_format,
                fields: c.fields.clone(),
                setup_note: c.setup_note.clone(),
            });
        }
        for a in &m.apps {
            let app_id = a.about.id.clone();
            snap.apps.push(AppEntry {
                app_id: app_id.clone(),
                package_id: pid.clone(),
                about: a.about.clone(),
                instances: a.instances.clone(),
                credentials: a.credentials.clone(),
                metadata: a.metadata.clone(),
                connect: a.connect.clone(),
                token: a.token.clone(),
                tools: a.tools.clone(),
                approvals: a.approvals.clone(),
                agent: a.agent.clone(),
                database: a.database.clone(),
                grants: a.grants,
            });
            for r in &a.hosts {
                if r.matcher.host == INSTANCE_HOST {
                    snap.instance_hosts
                        .push(host_entry(&app_id, INSTANCE_HOST, r, false));
                }
                // An `{instance}` rule is also an ordinary static claim on `default_host`.
                if let Some(h) = static_host(a, r) {
                    snap.hosts.push(host_entry(
                        &app_id,
                        h,
                        r,
                        lost.contains(&(
                            pid.clone(),
                            app_id.clone(),
                            claim_key(h, r.matcher.path_prefix.as_deref()),
                        )),
                    ));
                }
            }
        }
    }
    snap.packages
        .sort_by(|a, b| a.package_id.cmp(&b.package_id));
    snap.oauth_clients
        .sort_by(|a, b| (&a.package_id, &a.client_key).cmp(&(&b.package_id, &b.client_key)));
    snap.apps.sort_by(|a, b| a.app_id.cmp(&b.app_id));
    snap.hosts.sort_by(|a, b| {
        (&a.host, &a.path_prefix, &a.app_id).cmp(&(&b.host, &b.path_prefix, &b.app_id))
    });

    snap.instance_hosts
        .sort_by(|a, b| (&a.path_prefix, &a.app_id).cmp(&(&b.path_prefix, &b.app_id)));

    let v = serde_json::to_value(&snap).map_err(|e| CompileError::Serialize(e.to_string()))?;
    let json = canonical_bytes(&v);
    let hash = hex(&Sha256::digest(&json));
    Ok(CompiledSnapshot {
        json,
        hash,
        contract,
    })
}

fn host_entry(app_id: &str, host: &str, r: &HostRule, lost: bool) -> HostEntry {
    HostEntry {
        host: if host == INSTANCE_HOST {
            INSTANCE_HOST.to_owned()
        } else {
            HostPattern::lenient(host).as_str().to_owned()
        },
        path_prefix: r.matcher.path_prefix.clone(),
        app_id: app_id.to_owned(),
        claim_status: if lost {
            ClaimStatus::Lost
        } else {
            ClaimStatus::Active
        },
        tenant: r.tenant.clone(),
        rewrite_host: r.rewrite_host.clone(),
        inject: r.inject.clone(),
        finalize: r.finalize.clone(),
        transform: r.transform.clone(),
        intercept: r.intercept.clone(),
        passthrough: r.passthrough.clone(),
    }
}

/// Activation rule (09 §9): among candidates whose contract is `supported` or
/// `supported - 1`, the highest contract, then the most recent `activated_at`
/// (ties: greater hash, for determinism). Returns the content hash.
pub fn select_active<A: Ord>(candidates: &[(u32, A, String)], supported: u32) -> Option<&str> {
    let min = supported.saturating_sub(1);
    candidates
        .iter()
        .filter(|(c, _, _)| (min..=supported).contains(c))
        .max_by(|(c1, a1, h1), (c2, a2, h2)| (c1, a1, h1).cmp(&(c2, a2, h2)))
        .map(|(_, _, h)| h.as_str())
}
