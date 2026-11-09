//! HashiCorp Vault KV v2 store (`kind = "hashicorp-vault"`, `docs/21 §Phase 3 — HashiCorp
//! Vault secret store`, `docs/08 §2`, C-370..C-375, SEC-11, SEC-61).
//!
//! A small direct KV v2 client over the workspace's reqwest/rustls stack: TLS verification is
//! always on (`ca_file` and `tls.extra_ca_bundle` only *add* trust anchors), redirects are never
//! followed (the token header must not travel), and no token, secret value or response body
//! ever reaches a log line or an error.
//!
//! * **Managed** values live at `<kv_mount>/data/<managed_prefix>/<org_id>/<purpose>/<owner_id>`;
//!   the ref pins the KV version; `get` re-checks the path binding *and* the context stored
//!   with the value.
//! * **Reference** values are read at the latest version, only when the path matches a
//!   `reference_scopes` glob of the `SealContext` org (matched by org id, D-107) and is not
//!   under the managed prefix; every refusal, including a missing path or field, is the same
//!   `ReferenceNotPermitted` so a tenant cannot probe paths outside its scopes.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use aurora_config::{
    VaultAuthSettings, VaultSettings, glob_matches, under_managed_prefix, valid_kv_path,
};
use aurora_core::Secret;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::time::Instant;
use uuid::Uuid;

use crate::{
    Result, SealContext, SecretError, SecretRef, SecretStore, StoreCapabilities, StoreHealth,
};

const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');
/// Reference field used when none is given (C-371).
const DEFAULT_FIELD: &str = "value";
const LATEST: &str = "latest";
const MAX_FIELD_LEN: usize = 256;

enum Auth {
    Kubernetes {
        mount: String,
        role: String,
        jwt_path: String,
    },
    AppRole {
        mount: String,
        role_id: Secret<String>,
        secret_id: Secret<String>,
    },
    Token(Secret<String>),
}

struct TokenState {
    token: Secret<String>,
    /// Two thirds of the lease; `None` = never expires (root/dev token, `lease_duration` 0).
    renew_at: Option<Instant>,
    renewable: bool,
}

/// Result of a KV read, before the caller decides what each case means.
enum Read {
    Found(serde_json::Map<String, Value>),
    Missing,
    Denied,
}

pub struct VaultStore {
    id: String,
    base: String,
    http: reqwest::Client,
    kv_mount: String,
    managed_prefix: String,
    scopes: HashMap<Uuid, Vec<String>>,
    auth: Auth,
    token: Mutex<Option<TokenState>>,
}

impl std::fmt::Debug for VaultStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultStore")
            .field("id", &self.id)
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

fn read_env(name: &str, key: &str, store: &str) -> Result<Secret<String>> {
    match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => Ok(Secret::new(v.trim().to_owned())),
        _ => Err(SecretError::InvalidConfig(format!(
            "secret store '{store}': {key}: environment variable `{name}` is unset or empty"
        ))),
    }
}

fn load_pem(path: &str, key: &str, store: &str) -> Result<Vec<reqwest::Certificate>> {
    let bad =
        |why: &str| SecretError::InvalidConfig(format!("secret store '{store}': {key}: {why}"));
    let bytes = std::fs::read(Path::new(path)).map_err(|_| bad("file is not readable"))?;
    let certs = reqwest::Certificate::from_pem_bundle(&bytes)
        .map_err(|_| bad("not a valid PEM certificate bundle"))?;
    if certs.is_empty() {
        return Err(bad("contains no certificate"));
    }
    Ok(certs)
}

fn build_client(
    store: &str,
    ca_file: Option<&str>,
    extra_ca_bundle: Option<&str>,
) -> Result<reqwest::Client> {
    let mut b = reqwest::Client::builder()
        .use_rustls_tls()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30));
    if let Some(f) = ca_file {
        // An explicit bundle replaces the default roots for this store.
        b = b.tls_built_in_root_certs(false);
        for c in load_pem(f, "ca_file", store)? {
            b = b.add_root_certificate(c);
        }
    } else {
        for c in rustls_native_certs::load_native_certs().certs {
            if let Ok(c) = reqwest::Certificate::from_der(c.as_ref()) {
                b = b.add_root_certificate(c);
            }
        }
    }
    if let Some(f) = extra_ca_bundle {
        for c in load_pem(f, "tls.extra_ca_bundle", store)? {
            b = b.add_root_certificate(c);
        }
    }
    b.build().map_err(|_| {
        SecretError::InvalidConfig(format!(
            "secret store '{store}': cannot build the HTTP client"
        ))
    })
}

impl VaultStore {
    /// Build from validated settings. Reads the credential environment variables and the CA
    /// files; any problem refuses startup naming the key (never a value).
    pub fn new(settings: VaultSettings, extra_ca_bundle: Option<&str>) -> Result<Self> {
        let store = settings.id.clone();
        let http = build_client(&store, settings.ca_file.as_deref(), extra_ca_bundle)?;
        let auth_key = |k: &str| format!("secrets.stores[{store}].auth.{k}");
        let (auth, initial) = match settings.auth {
            VaultAuthSettings::Kubernetes {
                mount,
                role,
                jwt_path,
            } => (
                Auth::Kubernetes {
                    mount,
                    role,
                    jwt_path,
                },
                None,
            ),
            VaultAuthSettings::AppRole {
                mount,
                role_id_env,
                secret_id_env,
            } => (
                Auth::AppRole {
                    mount,
                    role_id: read_env(&role_id_env, &auth_key("role_id_env"), &store)?,
                    secret_id: read_env(&secret_id_env, &auth_key("secret_id_env"), &store)?,
                },
                None,
            ),
            VaultAuthSettings::Token { token_env } => {
                tracing::warn!(
                    store = %store,
                    "secret store uses `token` auth: development only, never for production"
                );
                let t = read_env(&token_env, &auth_key("token_env"), &store)?;
                (
                    Auth::Token(t.clone()),
                    Some(TokenState {
                        token: t,
                        renew_at: None,
                        renewable: false,
                    }),
                )
            }
        };
        Ok(Self {
            id: settings.id,
            base: settings.address,
            http,
            kv_mount: settings.kv_mount,
            managed_prefix: settings.managed_prefix,
            scopes: settings
                .reference_scopes
                .into_iter()
                .map(|s| (s.org_id, s.paths))
                .collect(),
            auth,
            token: Mutex::new(initial),
        })
    }

    // ---- URLs -------------------------------------------------------------------------

    fn enc(path: &str) -> String {
        path.split('/')
            .map(|s| utf8_percent_encode(s, SEGMENT).to_string())
            .collect::<Vec<_>>()
            .join("/")
    }

    fn url(&self, tail: &str) -> String {
        format!("{}/v1/{tail}", self.base)
    }

    fn kv_url(&self, op: &str, path: &str) -> String {
        self.url(&format!(
            "{}/{op}/{}",
            Self::enc(&self.kv_mount),
            Self::enc(path)
        ))
    }

    fn managed_path(&self, ctx: &SealContext) -> String {
        format!(
            "{}/{}/{}/{}",
            self.managed_prefix,
            ctx.org_id,
            ctx.purpose.as_str(),
            ctx.owner_id
        )
    }

    // ---- authentication and renewal ---------------------------------------------------

    fn parse_auth(body: &Value) -> Option<TokenState> {
        let a = body.get("auth")?;
        let token = a.get("client_token")?.as_str().filter(|t| !t.is_empty())?;
        let lease = a.get("lease_duration").and_then(Value::as_u64).unwrap_or(0);
        Some(TokenState {
            token: Secret::new(token.to_owned()),
            renew_at: (lease > 0).then(|| Instant::now() + Duration::from_secs(lease) * 2 / 3),
            renewable: a.get("renewable").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    async fn post_json(&self, url: &str, token: Option<&str>, body: &Value) -> Option<Value> {
        let mut rb = self.http.post(url).json(body);
        if let Some(t) = token {
            rb = rb.header("X-Vault-Token", t);
        }
        let resp = rb.send().await.ok()?;
        if !resp.status().is_success() {
            // Status only: the body may echo request details.
            tracing::warn!(store = %self.id, status = resp.status().as_u16(), "vault auth call failed");
            return None;
        }
        resp.json::<Value>().await.ok()
    }

    async fn login(&self) -> Option<TokenState> {
        match &self.auth {
            Auth::Token(t) => Some(TokenState {
                token: t.clone(),
                renew_at: None,
                renewable: false,
            }),
            Auth::Kubernetes {
                mount,
                role,
                jwt_path,
            } => {
                // Re-read at every login: the kubelet rotates the projected token.
                let jwt = Secret::new(tokio::fs::read_to_string(jwt_path).await.ok()?);
                let body = json!({ "role": role, "jwt": jwt.expose().trim() });
                let url = self.url(&format!("auth/{}/login", Self::enc(mount)));
                Self::parse_auth(&self.post_json(&url, None, &body).await?)
            }
            Auth::AppRole {
                mount,
                role_id,
                secret_id,
            } => {
                let body = json!({ "role_id": role_id.expose(), "secret_id": secret_id.expose() });
                let url = self.url(&format!("auth/{}/login", Self::enc(mount)));
                Self::parse_auth(&self.post_json(&url, None, &body).await?)
            }
        }
    }

    async fn renew(&self, token: &Secret<String>) -> Option<TokenState> {
        let url = self.url("auth/token/renew-self");
        let body = self
            .post_json(&url, Some(token.expose()), &json!({}))
            .await?;
        Self::parse_auth(&body)
    }

    /// A valid client token: reuse, renew at two thirds of the TTL, or log in again.
    /// Failure to obtain one is `StoreUnavailable` (fail closed).
    async fn token(&self) -> Result<Secret<String>> {
        let mut g = self.token.lock().await;
        if let Some(st) = g.as_ref() {
            if st.renew_at.is_none_or(|t| Instant::now() < t) {
                return Ok(st.token.clone());
            }
            if st.renewable
                && let Some(n) = self.renew(&st.token).await
            {
                let t = n.token.clone();
                *g = Some(n);
                return Ok(t);
            }
        }
        *g = None;
        let n = self.login().await.ok_or(SecretError::StoreUnavailable)?;
        let t = n.token.clone();
        *g = Some(n);
        Ok(t)
    }

    async fn drop_token(&self, bad: &Secret<String>) {
        let mut g = self.token.lock().await;
        if g.as_ref().is_some_and(|s| s.token.expose() == bad.expose()) {
            *g = None;
        }
    }

    // ---- requests ---------------------------------------------------------------------

    async fn send(
        &self,
        method: Method,
        url: &str,
        body: Option<&Value>,
    ) -> Result<(StatusCode, Value)> {
        for attempt in 0..2 {
            let token = self.token().await?;
            let mut rb = self
                .http
                .request(method.clone(), url)
                .header("X-Vault-Token", token.expose().as_str());
            if let Some(b) = body {
                rb = rb.json(b);
            }
            let resp = rb.send().await.map_err(|_| SecretError::StoreUnavailable)?;
            let status = resp.status();
            if status == StatusCode::FORBIDDEN && attempt == 0 && !self.token_is_valid(&token).await
            {
                // Revoked or expired: log in again and retry once.
                self.drop_token(&token).await;
                continue;
            }
            let bytes = resp
                .bytes()
                .await
                .map_err(|_| SecretError::StoreUnavailable)?;
            let value = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap_or(Value::Null)
            };
            return Ok((status, value));
        }
        Err(SecretError::StoreUnavailable)
    }

    /// Distinguishes "this path is denied" from "this token is dead" after a 403.
    async fn token_is_valid(&self, token: &Secret<String>) -> bool {
        self.http
            .get(self.url("auth/token/lookup-self"))
            .header("X-Vault-Token", token.expose().as_str())
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
    }

    async fn kv_read(&self, path: &str, version: Option<u64>) -> Result<Read> {
        let mut url = self.kv_url("data", path);
        if let Some(v) = version {
            url.push_str(&format!("?version={v}"));
        }
        let (status, body) = self.send(Method::GET, &url, None).await?;
        match status {
            s if s.is_success() => Ok(body
                .pointer("/data/data")
                .and_then(Value::as_object)
                .map_or(Read::Missing, |m| Read::Found(m.clone()))),
            StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST => Ok(Read::Missing),
            StatusCode::FORBIDDEN => Ok(Read::Denied),
            _ => Err(SecretError::StoreUnavailable),
        }
    }

    // ---- reference rules --------------------------------------------------------------

    /// SEC-61: the secret's org (by id), the current configuration, and never the managed prefix.
    fn reference_permitted(&self, org: Uuid, path: &str) -> bool {
        !under_managed_prefix(&self.managed_prefix, path)
            && self
                .scopes
                .get(&org)
                .is_some_and(|gs| gs.iter().any(|g| glob_matches(g, path)))
    }

    async fn read_reference(
        &self,
        ctx: &SealContext,
        path: &str,
        field: &str,
    ) -> Result<Secret<Vec<u8>>> {
        if !valid_kv_path(path) || !self.reference_permitted(ctx.org_id, path) {
            return Err(SecretError::ReferenceNotPermitted);
        }
        // Latest version, always (no `?version=`).
        match self.kv_read(path, None).await? {
            Read::Found(m) => match m.get(field) {
                Some(Value::String(s)) => Ok(Secret::new(s.clone().into_bytes())),
                _ => Err(SecretError::ReferenceNotPermitted),
            },
            Read::Missing | Read::Denied => Err(SecretError::ReferenceNotPermitted),
        }
    }

    /// Split a ref for this store: `(path, field, version)`.
    fn parts<'a>(&self, r: &'a SecretRef) -> Result<(&'a str, Option<&'a str>, &'a str)> {
        match r {
            SecretRef::Ext {
                store_id,
                path,
                field,
                version,
            } if store_id == &self.id => Ok((path, field.as_deref(), version)),
            _ => Err(SecretError::Malformed("reference not for this store")),
        }
    }

    /// Managed refs must name exactly the path derived from the context (SEC-11 path binding)
    /// and a numeric version, and carry no field.
    fn managed_version(
        &self,
        ctx: &SealContext,
        path: &str,
        field: Option<&str>,
        version: &str,
    ) -> Result<u64> {
        if field.is_some() {
            return Err(SecretError::Malformed("managed reference with a field"));
        }
        let v = version
            .parse::<u64>()
            .map_err(|_| SecretError::Malformed("invalid version"))?;
        if path != self.managed_path(ctx) {
            return Err(SecretError::SecretRefMismatch);
        }
        Ok(v)
    }
}

#[async_trait]
impl SecretStore for VaultStore {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> &'static str {
        "hashicorp-vault"
    }

    fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            managed: true,
            references: true,
        }
    }

    async fn put(&self, ctx: &SealContext, plaintext: &Secret<Vec<u8>>) -> Result<SecretRef> {
        let path = self.managed_path(ctx);
        let body = json!({ "data": {
            "value": STANDARD.encode(plaintext.expose()),
            "purpose": ctx.purpose.as_str(),
            "owner_id": ctx.owner_id.to_string(),
            "org_id": ctx.org_id.to_string(),
        }});
        let (status, resp) = self
            .send(Method::POST, &self.kv_url("data", &path), Some(&body))
            .await?;
        let version = status
            .is_success()
            .then(|| resp.pointer("/data/version").and_then(Value::as_u64))
            .flatten()
            .ok_or(SecretError::StoreUnavailable)?;
        SecretRef::ext(&self.id, &path, None, &version.to_string())
    }

    async fn get(&self, ctx: &SealContext, r: &SecretRef) -> Result<Secret<Vec<u8>>> {
        let (path, field, version) = self.parts(r)?;
        if version == LATEST {
            return self
                .read_reference(ctx, path, field.unwrap_or(DEFAULT_FIELD))
                .await;
        }
        if field.is_some() {
            // A reference never pins a version (SEC-61).
            return Err(SecretError::Malformed(
                "reference refs read the latest version",
            ));
        }
        let v = self.managed_version(ctx, path, field, version)?;
        let m = match self.kv_read(path, Some(v)).await? {
            Read::Found(m) => m,
            Read::Missing => return Err(SecretError::NotFound),
            Read::Denied => return Err(SecretError::StoreUnavailable),
        };
        let same = |k: &str, want: &str| m.get(k).and_then(Value::as_str) == Some(want);
        if !(same("purpose", ctx.purpose.as_str())
            && same("owner_id", &ctx.owner_id.to_string())
            && same("org_id", &ctx.org_id.to_string()))
        {
            return Err(SecretError::SecretRefMismatch);
        }
        let b64 = m
            .get("value")
            .and_then(Value::as_str)
            .ok_or(SecretError::Malformed("stored value is not a string"))?;
        let bytes = STANDARD
            .decode(b64)
            .map_err(|_| SecretError::Malformed("stored value is not base64"))?;
        Ok(Secret::new(bytes))
    }

    async fn delete(&self, ctx: &SealContext, r: &SecretRef) -> Result<()> {
        let (path, field, version) = self.parts(r)?;
        if version == LATEST {
            // Aurora never destroys what it did not write.
            return Ok(());
        }
        let v = self.managed_version(ctx, path, field, version)?;
        let body = json!({ "versions": [v] });
        let (status, _) = self
            .send(Method::POST, &self.kv_url("destroy", path), Some(&body))
            .await?;
        match status {
            s if s.is_success() => Ok(()),
            StatusCode::NOT_FOUND => Ok(()),
            _ => Err(SecretError::StoreUnavailable),
        }
    }

    async fn resolve_reference(
        &self,
        ctx: &SealContext,
        path: &str,
        field: Option<&str>,
    ) -> Result<SecretRef> {
        if !valid_kv_path(path) {
            return Err(SecretError::Malformed("invalid reference path"));
        }
        let field = field.unwrap_or(DEFAULT_FIELD);
        if field.is_empty() || field.len() > MAX_FIELD_LEN || field.chars().any(char::is_control) {
            return Err(SecretError::Malformed("invalid reference field"));
        }
        self.read_reference(ctx, path, field).await?;
        SecretRef::ext(&self.id, path, Some(field), LATEST)
    }

    async fn health(&self) -> StoreHealth {
        let up = self
            .http
            .get(self.url("sys/health?standbyok=true"))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success());
        if up && self.token().await.is_ok() {
            StoreHealth::Healthy
        } else {
            StoreHealth::Unavailable
        }
    }
}
