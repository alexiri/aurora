//! Typed, validated form of a `kind = "hashicorp-vault"` entry of `secrets.stores[]`
//! (`docs/15` Vault rows, `docs/API_CONTRACTS.md` C-370..C-375, SEC-61).
//!
//! The store table is opaque in [`SecretStoreConfig`]; this module interprets it. It is
//! used by startup validation (`validate`) and by `aurora-secrets` to build the store, so
//! both apply the same rules. No environment variable is read here (only names are kept).

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::raw::SecretStoreConfig;
use crate::urlcheck::parse_url;

pub const VAULT_KIND: &str = "hashicorp-vault";
const DEFAULT_JWT_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/token";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultAuthSettings {
    Kubernetes {
        mount: String,
        role: String,
        jwt_path: String,
    },
    AppRole {
        mount: String,
        role_id: CredSource,
        secret_id: CredSource,
    },
    /// Development only: the token is read from this environment variable.
    Token { token_env: String },
}

/// Where an AppRole credential comes from (FIX-124): the name of an environment variable (the
/// chart mounts a Kubernetes Secret), a literal (the role id is not a secret), or a value sealed
/// in the inline store by the dashboard (`{ "$secret": { ref, owner } }`, D-236(g)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredSource {
    Env(String),
    Literal(String),
    Sealed {
        sealed_ref: String,
        owner: uuid::Uuid,
    },
}

/// One `reference_scopes[]` entry; `org_id` is the organization's immutable id (D-107).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultReferenceScope {
    pub org_id: uuid::Uuid,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultSettings {
    pub id: String,
    pub address: String,
    pub ca_file: Option<String>,
    /// The store's CA as PEM text (dashboard-defined stores; excludes `ca_file`).
    pub ca_pem: Option<String>,
    pub auth: VaultAuthSettings,
    pub kv_mount: String,
    pub managed_prefix: String,
    pub reference_scopes: Vec<VaultReferenceScope>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStore {
    address: Option<String>,
    ca_file: Option<String>,
    ca_pem: Option<String>,
    auth: Option<RawAuth>,
    kv_mount: Option<String>,
    managed_prefix: Option<String>,
    #[serde(default)]
    reference_scopes: Vec<RawScope>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAuth {
    method: Option<String>,
    mount: Option<String>,
    role: Option<String>,
    jwt_path: Option<String>,
    role_id_env: Option<String>,
    secret_id_env: Option<String>,
    token_env: Option<String>,
    role_id: Option<String>,
    secret_id: Option<RawSealed>,
}

/// `{ "$secret": { ref, owner } }`: a value the dashboard sealed in the inline store.
#[derive(Deserialize)]
struct RawSealed {
    #[serde(rename = "$secret")]
    inner: RawSealedInner,
}

#[derive(Deserialize)]
struct RawSealedInner {
    #[serde(rename = "ref")]
    sealed_ref: String,
    owner: String,
}

/// Longest PEM text a store may carry.
pub const MAX_CA_PEM_LEN: usize = 16 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScope {
    org_id: Option<String>,
    paths: Option<Vec<String>>,
}

/// True for `localhost`, `*.localhost` and loopback IP literals.
fn is_loopback(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// A path relative to the KV mount: no leading/trailing `/`, no empty, `.` or `..` segment,
/// 1..=512 chars, no control characters (C-371).
#[must_use]
pub fn valid_kv_path(p: &str) -> bool {
    (1..=512).contains(&p.len())
        && !p.chars().any(char::is_control)
        && p.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
}

fn valid_mount(m: &str) -> bool {
    valid_kv_path(m)
}

fn valid_prefix(p: &str) -> bool {
    (1..=128).contains(&p.len())
        && p.split('/').all(|s| {
            !s.is_empty()
                && s != "."
                && s != ".."
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        })
}

/// `path` is the managed prefix itself or lies under it.
#[must_use]
pub fn under_managed_prefix(prefix: &str, path: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|r| r.starts_with('/'))
}

/// A scope glob: an exact path, or a prefix ending in one trailing `*`.
#[must_use]
pub fn glob_matches(glob: &str, path: &str) -> bool {
    match glob.strip_suffix('*') {
        Some(stem) => path.starts_with(stem),
        None => glob == path,
    }
}

/// Whether `glob` could match the managed prefix itself or anything under `prefix/`.
fn glob_reaches_managed(glob: &str, prefix: &str) -> bool {
    let managed_dir = format!("{prefix}/");
    match glob.strip_suffix('*') {
        // covers `prefix/…` (stem is a prefix of `prefix/`) or lies inside it
        Some(stem) => managed_dir.starts_with(stem) || stem.starts_with(&managed_dir),
        None => under_managed_prefix(prefix, glob),
    }
}

fn check_glob(glob: &str, prefix: &str) -> Result<(), String> {
    let body = glob.strip_suffix('*').unwrap_or(glob);
    if body.contains('*') {
        return Err("`*` is allowed only once, as the last character".into());
    }
    if glob.is_empty() || glob.starts_with('/') || glob.chars().any(char::is_control) {
        return Err("must be a path relative to kv_mount without a leading `/`".into());
    }
    // A trailing `/` is fine only before the `*` (`teams/*`).
    let segs: Vec<&str> = body.split('/').collect();
    for (i, s) in segs.iter().enumerate() {
        let last = i + 1 == segs.len();
        if (s.is_empty() && !(last && glob.ends_with('*'))) || *s == "." || *s == ".." {
            return Err("no empty, `.` or `..` segment".into());
        }
    }
    if glob_reaches_managed(glob, prefix) {
        return Err(format!(
            "must not equal, lie inside or cover the managed prefix `{prefix}/` (it holds every organization's managed values)"
        ));
    }
    Ok(())
}

impl VaultSettings {
    /// Parse and validate one store entry. Errors are `(key, message)` with the key relative
    /// to the store (`address`, `auth.role`, `reference_scopes[0].org_id`, …); the caller
    /// prefixes `secrets.stores[i].`. Messages never contain a configured value that could be
    /// secret (only names, and the offending non-secret fragment where useful).
    pub fn parse(cfg: &SecretStoreConfig) -> Result<Self, Vec<(String, String)>> {
        let errs: std::cell::RefCell<Vec<(String, String)>> = std::cell::RefCell::new(Vec::new());
        let err = |k: &str, m: &str| errs.borrow_mut().push((k.to_owned(), m.to_owned()));
        let raw: RawStore = match toml::Value::Table(cfg.settings.expose().clone()).try_into() {
            Ok(r) => r,
            Err(e) => {
                return Err(vec![(
                    String::new(),
                    format!("invalid store table: {}", e.message()),
                )]);
            }
        };

        let address = match raw.address.as_deref() {
            None => {
                err("address", "required");
                String::new()
            }
            Some(a) => {
                match parse_url(a, &["http", "https"]) {
                    Err(m) => err("address", &m),
                    Ok(u) => {
                        if u.has_userinfo {
                            err("address", "must not contain credentials");
                        }
                        if u.has_query_or_fragment || !(u.path.is_empty() || u.path == "/") {
                            err(
                                "address",
                                "must be a base URL without path, query or fragment",
                            );
                        }
                        if a.len() >= 5
                            && a[..5].eq_ignore_ascii_case("http:")
                            && !is_loopback(&u.host)
                        {
                            err(
                                "address",
                                "`http://` is accepted only for a loopback host (use https)",
                            );
                        }
                    }
                }
                a.trim_end_matches('/').to_owned()
            }
        };
        if raw.ca_file.as_deref() == Some("") {
            err("ca_file", "must not be empty when set");
        }
        if let Some(p) = raw.ca_pem.as_deref() {
            if raw.ca_file.is_some() {
                err("ca_pem", "cannot be combined with ca_file");
            } else if p.len() > MAX_CA_PEM_LEN {
                err("ca_pem", "too long");
            } else if !p.contains("-----BEGIN CERTIFICATE-----") {
                err("ca_pem", "not a PEM certificate");
            }
        }

        let kv_mount = raw.kv_mount.unwrap_or_else(|| "secret".into());
        if !valid_mount(&kv_mount) {
            err(
                "kv_mount",
                "non-empty, no leading or trailing `/`, no empty, `.` or `..` segment",
            );
        }
        let managed_prefix = raw.managed_prefix.unwrap_or_else(|| "aurora".into());
        if !valid_prefix(&managed_prefix) {
            err(
                "managed_prefix",
                "1..128 chars, segments of [A-Za-z0-9._-], no empty, `.` or `..` segment",
            );
        }

        let auth = match raw.auth {
            None => {
                err("auth.method", "required");
                None
            }
            Some(a) => {
                let mut e = Vec::new();
                let r = parse_auth(a, &mut e);
                errs.borrow_mut().extend(e);
                r
            }
        };

        let mut by_org: BTreeMap<uuid::Uuid, Vec<String>> = BTreeMap::new();
        for (i, sc) in raw.reference_scopes.iter().enumerate() {
            let org = match sc.org_id.as_deref().map(str::trim) {
                None => {
                    err(&format!("reference_scopes[{i}].org_id"), "required");
                    None
                }
                Some(o) => match uuid::Uuid::parse_str(o) {
                    // Hyphenated form only: `simple`/`urn:` spellings are not the id string.
                    Ok(u) if u.hyphenated().to_string().eq_ignore_ascii_case(o) => Some(u),
                    _ => {
                        err(
                            &format!("reference_scopes[{i}].org_id"),
                            "must be the organization's immutable id (a UUID), never its slug",
                        );
                        None
                    }
                },
            };
            let paths = match &sc.paths {
                Some(p) if !p.is_empty() => p.as_slice(),
                _ => {
                    err(
                        &format!("reference_scopes[{i}].paths"),
                        "required and non-empty",
                    );
                    &[]
                }
            };
            for (j, g) in paths.iter().enumerate() {
                if let Err(m) = check_glob(g, &managed_prefix) {
                    err(&format!("reference_scopes[{i}].paths[{j}]"), &m);
                }
            }
            if let Some(o) = org {
                by_org.entry(o).or_default().extend(paths.iter().cloned());
            }
        }

        if !errs.borrow().is_empty() {
            return Err(errs.into_inner());
        }
        let Some(auth) = auth else {
            return Err(errs.into_inner());
        };
        Ok(Self {
            id: cfg.id.clone(),
            address,
            ca_file: raw.ca_file,
            ca_pem: raw.ca_pem,
            auth,
            kv_mount,
            managed_prefix,
            reference_scopes: by_org
                .into_iter()
                .map(|(org_id, mut paths)| {
                    paths.sort();
                    paths.dedup();
                    VaultReferenceScope { org_id, paths }
                })
                .collect(),
        })
    }
}

fn parse_auth(a: RawAuth, errs: &mut Vec<(String, String)>) -> Option<VaultAuthSettings> {
    let mut err = |k: &str, m: &str| errs.push((k.to_owned(), m.to_owned()));
    let method = a.method.as_deref().unwrap_or("");
    let (allowed, name): (&[&str], _) = match method {
        "kubernetes" => (&["mount", "role", "jwt_path"], "kubernetes"),
        "approle" => (
            &[
                "mount",
                "role_id_env",
                "secret_id_env",
                "role_id",
                "secret_id",
            ],
            "approle",
        ),
        "token" => (&["token_env"], "token"),
        "" => {
            err("auth.method", "required: kubernetes, approle or token");
            return None;
        }
        _ => {
            err("auth.method", "must be kubernetes, approle or token");
            return None;
        }
    };
    let given = [
        ("mount", a.mount.is_some()),
        ("role", a.role.is_some()),
        ("jwt_path", a.jwt_path.is_some()),
        ("role_id_env", a.role_id_env.is_some()),
        ("secret_id_env", a.secret_id_env.is_some()),
        ("token_env", a.token_env.is_some()),
        ("role_id", a.role_id.is_some()),
        ("secret_id", a.secret_id.is_some()),
    ];
    for (k, set) in given {
        if set && !allowed.contains(&k) {
            err(
                &format!("auth.{k}"),
                &format!("not valid for auth.method `{name}`"),
            );
        }
    }
    let need = |v: &Option<String>, k: &str, err: &mut dyn FnMut(&str, &str)| -> String {
        match v.as_deref() {
            Some(s) if !s.is_empty() => s.to_owned(),
            _ => {
                err(&format!("auth.{k}"), "required and non-empty");
                String::new()
            }
        }
    };
    let mount_or = |v: &Option<String>, d: &str, err: &mut dyn FnMut(&str, &str)| -> String {
        match v.as_deref() {
            None => d.to_owned(),
            Some(m) if valid_mount(m) => m.to_owned(),
            Some(_) => {
                err("auth.mount", "non-empty, no leading or trailing `/`");
                String::new()
            }
        }
    };
    let before = errs.len();
    let out = {
        let mut err = |k: &str, m: &str| errs.push((k.to_owned(), m.to_owned()));
        match method {
            "kubernetes" => VaultAuthSettings::Kubernetes {
                mount: mount_or(&a.mount, "kubernetes", &mut err),
                role: need(&a.role, "role", &mut err),
                jwt_path: match a.jwt_path.as_deref() {
                    None => DEFAULT_JWT_PATH.to_owned(),
                    Some("") => {
                        err("auth.jwt_path", "must not be empty when set");
                        String::new()
                    }
                    Some(p) => p.to_owned(),
                },
            },
            "approle" => {
                let mount = mount_or(&a.mount, "approle", &mut err);
                let role_id = match (&a.role_id, &a.role_id_env) {
                    (Some(_), Some(_)) => {
                        err("auth.role_id", "give either role_id or role_id_env");
                        CredSource::Literal(String::new())
                    }
                    (Some(v), None) if !v.is_empty() && v.len() <= 256 => {
                        CredSource::Literal(v.clone())
                    }
                    (Some(_), None) => {
                        err("auth.role_id", "1..256 characters");
                        CredSource::Literal(String::new())
                    }
                    _ => CredSource::Env(need(&a.role_id_env, "role_id_env", &mut err)),
                };
                let secret_id = match (&a.secret_id, &a.secret_id_env) {
                    (Some(_), Some(_)) => {
                        err("auth.secret_id", "give either secret_id or secret_id_env");
                        CredSource::Env(String::new())
                    }
                    (Some(s), None) => match uuid::Uuid::parse_str(&s.inner.owner) {
                        Ok(owner) if !s.inner.sealed_ref.is_empty() => CredSource::Sealed {
                            sealed_ref: s.inner.sealed_ref.clone(),
                            owner,
                        },
                        _ => {
                            err("auth.secret_id", "not a sealed secret");
                            CredSource::Env(String::new())
                        }
                    },
                    _ => CredSource::Env(need(&a.secret_id_env, "secret_id_env", &mut err)),
                };
                VaultAuthSettings::AppRole {
                    mount,
                    role_id,
                    secret_id,
                }
            }
            _ => VaultAuthSettings::Token {
                token_env: need(&a.token_env, "token_env", &mut err),
            },
        }
    };
    (errs.len() == before).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(v: serde_json::Value) -> Result<VaultSettings, Vec<(String, String)>> {
        let cfg: SecretStoreConfig = serde_json::from_value(v).unwrap();
        VaultSettings::parse(&cfg)
    }

    fn base(auth: serde_json::Value) -> serde_json::Value {
        json!({ "id": "v", "kind": "hashicorp-vault", "address": "https://vault.example.org:8200", "auth": auth })
    }

    #[test]
    fn a_dashboard_approle_store_has_a_literal_role_id_and_a_sealed_secret_id() {
        let owner = uuid::Uuid::now_v7();
        let s = parse(base(json!({
            "method": "approle", "role_id": "r1",
            "secret_id": { "$secret": { "ref": "inline:v1:k1:abc", "owner": owner, "updated_at": "2026-11-15T00:00:00Z" } }
        })))
        .unwrap();
        let VaultAuthSettings::AppRole {
            role_id, secret_id, ..
        } = s.auth
        else {
            panic!("approle")
        };
        assert_eq!(role_id, CredSource::Literal("r1".into()));
        assert_eq!(
            secret_id,
            CredSource::Sealed {
                sealed_ref: "inline:v1:k1:abc".into(),
                owner
            }
        );
    }

    #[test]
    fn an_approle_credential_has_one_source() {
        let e = parse(base(json!({
            "method": "approle", "role_id": "r1", "role_id_env": "X", "secret_id_env": "Y"
        })))
        .unwrap_err();
        assert!(e.iter().any(|(k, _)| k == "auth.role_id"), "{e:?}");
        let e = parse(base(json!({ "method": "approle", "role_id": "r1" }))).unwrap_err();
        assert!(e.iter().any(|(k, _)| k == "auth.secret_id_env"), "{e:?}");
    }

    #[test]
    fn a_ca_is_a_file_or_pem_text_never_both_and_must_look_like_a_certificate() {
        let mut v = base(json!({ "method": "kubernetes", "role": "r" }));
        v["ca_pem"] = json!("-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n");
        assert!(parse(v.clone()).is_ok());
        v["ca_file"] = json!("/etc/ca.pem");
        assert!(
            parse(v.clone())
                .unwrap_err()
                .iter()
                .any(|(k, _)| k == "ca_pem")
        );
        v["ca_file"] = serde_json::Value::Null;
        v.as_object_mut().unwrap().remove("ca_file");
        v["ca_pem"] = json!("not a certificate");
        assert!(parse(v).unwrap_err().iter().any(|(k, _)| k == "ca_pem"));
    }
}
