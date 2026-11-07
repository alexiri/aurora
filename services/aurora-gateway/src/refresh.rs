//! Token freshness for app connections (docs/08 §8, SEC-27): `oauth2_refresh` and
//! `token: hook` (`token.mint`) serialized per connection by an in-process striped mutex plus
//! a session-level Postgres advisory lock, with compare-and-set persistence. The whole locked
//! section runs in a spawned task so a client hang-up cannot leave the lock held or lose a
//! rotated refresh token.

use crate::apps::ConnCand;
use crate::mechanisms::template::{Dest, Vars, render};
use crate::mint::{MintError, MintInput, Minter};
use crate::snapshot::Loaded;
use aurora_core::Secret;
use aurora_core::limits::{REFRESH_LOCK_WAIT, REFRESH_MARGIN};
use aurora_db::PgPool;
use aurora_db::repo::gateway as repo;
use aurora_registry::manifest::{BodyFormat, ClientAuth, Token, TokenKind, UrlOrTemplate};
use aurora_registry::snapshot::OauthClientEntry;
use aurora_secrets::{Purpose, SealContext, SecretRef, SecretVault, ValueSource};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const STRIPES: usize = 64;
const LOCK_POLL: Duration = Duration::from_millis(25);
const TOKEN_CALL_WALL: Duration = Duration::from_secs(15);
const DEFAULT_LIFETIME_SECS: i64 = 3600;

#[derive(Debug, PartialEq, Eq)]
pub enum RefreshError {
    /// The user must reconnect (`status = reauth_required`).
    Reauth,
    /// Transient failure or unusable configuration; no state changed.
    Unavailable,
    /// External secret store unreachable.
    StoreUnavailable(String),
}

/// Owned inputs of one locked refresh (the work outlives the request future).
struct Job {
    conn: ConnCand,
    force: bool,
    token: Token,
    package_id: String,
    package_digest: String,
    client: Option<OauthClientEntry>,
    context: BTreeMap<String, String>,
}

pub struct TokenManager {
    pool: PgPool,
    vault: Arc<SecretVault>,
    http: reqwest::Client,
    minter: Option<Arc<dyn Minter>>,
    stripes: Vec<tokio::sync::Mutex<()>>,
    lock_wait: Duration,
    margin: Duration,
}

pub fn expiring(expires_at: Option<DateTime<Utc>>, margin: Duration) -> bool {
    expires_at.is_some_and(|e| chrono::Duration::from_std(margin).is_ok_and(|m| e < Utc::now() + m))
}

impl TokenManager {
    pub fn new(pool: PgPool, vault: Arc<SecretVault>, minter: Option<Arc<dyn Minter>>) -> Self {
        Self {
            pool,
            vault,
            http: reqwest::Client::builder()
                .timeout(TOKEN_CALL_WALL)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
            minter,
            stripes: (0..STRIPES).map(|_| tokio::sync::Mutex::new(())).collect(),
            lock_wait: REFRESH_LOCK_WAIT,
            margin: REFRESH_MARGIN,
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn with_lock_wait(mut self, d: Duration) -> Self {
        self.lock_wait = d;
        self
    }

    /// Does this app refresh tokens at all?
    pub fn applies(app: &aurora_registry::snapshot::AppEntry) -> bool {
        app.token
            .as_ref()
            .is_some_and(|t| t.kind != TokenKind::Static)
    }

    /// Returns the connection with fresh credentials. `force` (a stale-code response)
    /// refreshes even when the stored expiry looks fine, unless another process already did.
    pub async fn ensure_fresh(
        self: &Arc<Self>,
        loaded: &Loaded,
        conn: &ConnCand,
        context: &HashMap<String, String>,
        force: bool,
    ) -> Result<ConnCand, RefreshError> {
        let snap = &loaded.snapshot;
        let Some(app) = snap.app(&conn.app_id) else {
            return Err(RefreshError::Unavailable);
        };
        let Some(token) = app.token.clone().filter(|t| t.kind != TokenKind::Static) else {
            return Ok(conn.clone());
        };
        if !force && !expiring(conn.expires_at, self.margin) {
            return Ok(conn.clone());
        }
        let pkg = snap
            .packages
            .iter()
            .find(|p| p.package_id == app.package_id);
        let client = token.client.as_ref().and_then(|k| {
            snap.oauth_clients
                .iter()
                .find(|c| c.package_id == app.package_id && &c.client_key == k)
                .cloned()
        });
        let job = Job {
            conn: conn.clone(),
            force,
            token,
            package_id: app.package_id.clone(),
            package_digest: pkg.map(|p| p.digest.clone()).unwrap_or_default(),
            client,
            context: context
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        };
        let me = self.clone();
        tokio::spawn(async move { me.run(job).await })
            .await
            .map_err(|_| RefreshError::Unavailable)?
    }

    async fn run(&self, job: Job) -> Result<ConnCand, RefreshError> {
        let id = job.conn.id;
        let stripe = &self.stripes[(id.as_u128() % STRIPES as u128) as usize];
        let _local = stripe.lock().await;
        let mut pg = self
            .pool
            .acquire()
            .await
            .map_err(|_| RefreshError::Unavailable)?;
        let deadline = tokio::time::Instant::now() + self.lock_wait;
        loop {
            match repo::try_refresh_lock(&mut pg, id).await {
                Ok(true) => break,
                Ok(false) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(LOCK_POLL).await;
                }
                _ => return Err(RefreshError::Unavailable),
            }
        }
        let out = self.locked(&job).await;
        if !matches!(repo::refresh_unlock(&mut pg, id).await, Ok(true)) {
            // Never return a connection that may still hold the session lock to the pool.
            drop(pg.detach());
        }
        out
    }

    fn cand(&self, base: &ConnCand, st: &repo::ConnectionState) -> Result<ConnCand, RefreshError> {
        let mut c = base.clone();
        c.vref = st
            .credentials_ref
            .parse()
            .map_err(|_| RefreshError::Unavailable)?;
        c.version = st.credentials_version;
        c.expires_at = st.credentials_expires_at;
        Ok(c)
    }

    async fn state(&self, id: Uuid) -> Result<repo::ConnectionState, RefreshError> {
        let st = repo::connection_state(&self.pool, id)
            .await
            .map_err(|_| RefreshError::Unavailable)?
            .ok_or(RefreshError::Unavailable)?;
        match st.status.as_str() {
            "active" => Ok(st),
            "reauth_required" => Err(RefreshError::Reauth),
            _ => Err(RefreshError::Unavailable),
        }
    }

    async fn locked(&self, job: &Job) -> Result<ConnCand, RefreshError> {
        let id = job.conn.id;
        // Re-read after taking the lock: another instance may have refreshed.
        let st = self.state(id).await?;
        let changed = st.credentials_version != job.conn.version;
        let fresh = !expiring(st.credentials_expires_at, self.margin);
        if (changed && (job.force || fresh)) || (!changed && !job.force && fresh) {
            return self.cand(&job.conn, &st);
        }
        let ctx = SealContext {
            purpose: Purpose::ConnectionCredentials,
            owner_id: id,
            org_id: st.org_id,
            workspace_id: st.workspace_id,
        };
        let vref: SecretRef = st
            .credentials_ref
            .parse()
            .map_err(|_| RefreshError::Unavailable)?;
        let current = self.open_json(&ctx, &vref).await?;
        let (creds, expires_at) = match job.token.kind {
            TokenKind::Oauth2Refresh => self.exchange(job, &st, &current).await?,
            TokenKind::Hook => self.mint(job, &st, &current).await?,
            TokenKind::Static => return self.cand(&job.conn, &st),
        };
        let bytes =
            serde_json::to_vec(&Value::Object(creds)).map_err(|_| RefreshError::Unavailable)?;
        let new_ref = self
            .vault
            .seal(&ctx, Secret::from(bytes))
            .await
            .map_err(store_err)?;
        let won = repo::cas_connection_credentials(
            &self.pool,
            id,
            &new_ref.to_column(),
            st.credentials_version,
            Some(expires_at),
        )
        .await
        .map_err(|_| RefreshError::Unavailable)?;
        if !won {
            // Someone else won: drop ours and use theirs.
            let _ = self
                .vault
                .discard(&ctx, &new_ref, ValueSource::Managed)
                .await;
            let st = self.state(id).await?;
            return self.cand(&job.conn, &st);
        }
        let _ = aurora_db::bus::notify(
            &self.pool,
            aurora_db::bus::Channel::GatewayInvalidate,
            &serde_json::json!({ "scope": "connection", "id": id }),
        )
        .await;
        let mut c = job.conn.clone();
        c.vref = new_ref;
        c.version = st.credentials_version + 1;
        c.expires_at = Some(expires_at);
        Ok(c)
    }

    async fn open_json(
        &self,
        ctx: &SealContext,
        r: &SecretRef,
    ) -> Result<Map<String, Value>, RefreshError> {
        let s = self.vault.open(ctx, r).await.map_err(store_err)?;
        match serde_json::from_slice(s.expose()) {
            Ok(Value::Object(m)) => Ok(m),
            _ => Err(RefreshError::Unavailable),
        }
    }

    /// Non-secret and secret client fields of the minting client.
    async fn client_fields(
        &self,
        st: &repo::ConnectionState,
    ) -> Result<(Map<String, Value>, Map<String, Value>), RefreshError> {
        let Some(cid) = st.oauth_client_id else {
            return Ok((Map::new(), Map::new()));
        };
        let row = repo::oauth_client(&self.pool, cid)
            .await
            .map_err(|_| RefreshError::Unavailable)?
            .filter(|r| r.enabled)
            .ok_or(RefreshError::Unavailable)?;
        let public = row.fields.as_object().cloned().unwrap_or_default();
        let ctx = SealContext {
            purpose: Purpose::OauthClientSecret,
            owner_id: cid,
            org_id: row.org_id.unwrap_or_else(Uuid::nil),
            workspace_id: row.workspace_id,
        };
        let r: SecretRef = row
            .secret_ref
            .parse()
            .map_err(|_| RefreshError::Unavailable)?;
        Ok((public, self.open_json(&ctx, &r).await?))
    }

    fn lifetime(job: &Job, secs: i64) -> DateTime<Utc> {
        let mut s = secs.clamp(0, 10 * 365 * 86_400);
        if let Some(max) = job.token.max_lifetime_secs {
            s = s.min(i64::try_from(max).unwrap_or(i64::MAX));
        }
        Utc::now() + chrono::Duration::seconds(s)
    }

    async fn reauth(&self, id: Uuid, reason: &str) -> RefreshError {
        if repo::mark_reauth_required(&self.pool, id, reason)
            .await
            .unwrap_or(false)
        {
            let _ = aurora_db::bus::notify(
                &self.pool,
                aurora_db::bus::Channel::GatewayInvalidate,
                &serde_json::json!({ "scope": "connection", "id": id }),
            )
            .await;
        }
        RefreshError::Reauth
    }

    async fn mint(
        &self,
        job: &Job,
        st: &repo::ConnectionState,
        current: &Map<String, Value>,
    ) -> Result<(Map<String, Value>, DateTime<Utc>), RefreshError> {
        let minter = self.minter.as_ref().ok_or(RefreshError::Unavailable)?;
        let (client, client_secret) = self.client_fields(st).await?;
        let out = minter
            .mint(MintInput {
                package_id: job.package_id.clone(),
                package_digest: job.package_digest.clone(),
                app_id: job.conn.app_id.clone(),
                credentials: current.clone(),
                metadata: job.conn.metadata.clone(),
                tenant_host: job.conn.tenant_host.clone(),
                client,
                client_secret,
                context: job.context.clone(),
            })
            .await;
        match out {
            Ok(o) => {
                let mut creds = current.clone();
                creds.extend(o.credentials);
                let at =
                    DateTime::from_timestamp(o.expires_at, 0).ok_or(RefreshError::Unavailable)?;
                let cap = Self::lifetime(job, 10 * 365 * 86_400);
                Ok((creds, at.min(cap)))
            }
            Err(MintError::Reauth) => Err(self.reauth(job.conn.id, "mint_reauth").await),
            Err(MintError::Unavailable) => Err(RefreshError::Unavailable),
        }
    }

    async fn exchange(
        &self,
        job: &Job,
        st: &repo::ConnectionState,
        current: &Map<String, Value>,
    ) -> Result<(Map<String, Value>, DateTime<Utc>), RefreshError> {
        let client = job.client.as_ref().ok_or(RefreshError::Unavailable)?;
        let field = job
            .token
            .refresh_field
            .as_deref()
            .unwrap_or("refresh_token");
        let refresh = current
            .get(field)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or(RefreshError::Unavailable)?;
        let (public, secret) = self.client_fields(st).await?;
        let url = token_url(client, job.conn.tenant_host.as_deref())?;
        let mut params: Vec<(String, String)> = vec![
            ("grant_type".into(), "refresh_token".into()),
            ("refresh_token".into(), refresh.to_owned()),
        ];
        let optional = BTreeSet::new();
        let metadata = Value::Null;
        let ctxmap = HashMap::new();
        let vars = Vars {
            credentials: current,
            optional: &optional,
            metadata: &metadata,
            tenant_host: job.conn.tenant_host.as_deref(),
            allowed_context: &[],
            context: &ctxmap,
        };
        for (k, t) in &job.token.extra_params {
            let r = render(t, Dest::Raw, &vars).map_err(|_| RefreshError::Unavailable)?;
            params.push((k.clone(), r.text));
        }
        let get = |k: &str| {
            secret
                .get(k)
                .or_else(|| public.get(k))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let mut req = self.http.post(&url).header("accept", "application/json");
        match client.client_auth {
            ClientAuth::Basic => {
                let creds = format!(
                    "{}:{}",
                    get("client_id").unwrap_or_default(),
                    get("client_secret").unwrap_or_default()
                );
                req = req.header("authorization", format!("Basic {}", STANDARD.encode(creds)));
            }
            ClientAuth::Body => {
                for k in ["client_id", "client_secret"] {
                    if let Some(v) = get(k) {
                        params.push((k.into(), v));
                    }
                }
            }
        }
        req = match client.body_format {
            BodyFormat::Form => req.form(&params),
            BodyFormat::Json => req.json(&params.into_iter().collect::<BTreeMap<_, _>>()),
        };
        let resp = req.send().await.map_err(|_| RefreshError::Unavailable)?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let code = body.get("error").and_then(Value::as_str).unwrap_or("");
            if status.is_client_error()
                && (code == "invalid_grant" || job.token.reauth_codes.iter().any(|c| c == code))
            {
                return Err(self.reauth(job.conn.id, "invalid_grant").await);
            }
            return Err(RefreshError::Unavailable);
        }
        let access = body
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or(RefreshError::Unavailable)?;
        let mut creds = current.clone();
        creds.insert("access_token".into(), Value::String(access.to_owned()));
        if job.token.rotate_refresh_token
            && let Some(r) = body.get("refresh_token").and_then(Value::as_str)
            && !r.is_empty()
        {
            creds.insert(field.to_owned(), Value::String(r.to_owned()));
        }
        let secs = body
            .get("expires_in")
            .and_then(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(DEFAULT_LIFETIME_SECS);
        Ok((creds, Self::lifetime(job, secs)))
    }
}

fn token_url(c: &OauthClientEntry, tenant_host: Option<&str>) -> Result<String, RefreshError> {
    match &c.token_url {
        UrlOrTemplate::Url(u) => Ok(u.clone()),
        UrlOrTemplate::Templated(z) => {
            let creds = Map::new();
            let optional = BTreeSet::new();
            let metadata = Value::Null;
            let ctx = HashMap::new();
            let vars = Vars {
                credentials: &creds,
                optional: &optional,
                metadata: &metadata,
                tenant_host,
                allowed_context: &[],
                context: &ctx,
            };
            let url = render(&z.template, Dest::Host, &vars)
                .map(|r| r.text)
                .map_err(|_| RefreshError::Unavailable)?;
            check_templated_token_url(&url, &z.zone)?;
            Ok(url)
        }
    }
}

/// SEC-17 / `09 §6`: a templated token endpoint must resolve to `https` on a host
/// inside its declared zone; anything else fails closed.
fn check_templated_token_url(url: &str, zone: &str) -> Result<(), RefreshError> {
    let uri: http::Uri = url.parse().map_err(|_| RefreshError::Unavailable)?;
    if uri.scheme_str() != Some("https") {
        return Err(RefreshError::Unavailable);
    }
    let host = uri.host().ok_or(RefreshError::Unavailable)?;
    // No userinfo tricks, no IP literals and no non-default port, whatever the zone says (SEC-17).
    let authority = uri.authority().map_or("", http::uri::Authority::as_str);
    if authority.contains('@')
        || host.starts_with('[')
        || host.parse::<std::net::IpAddr>().is_ok()
        || uri.port_u16().is_some_and(|p| p != 443)
    {
        return Err(RefreshError::Unavailable);
    }
    let pattern = aurora_policy::HostPattern::parse(zone).map_err(|_| RefreshError::Unavailable)?;
    if pattern.matches(&host.to_ascii_lowercase(), uri.port_u16().unwrap_or(443)) {
        Ok(())
    } else {
        Err(RefreshError::Unavailable)
    }
}

fn store_err(e: aurora_secrets::SecretError) -> RefreshError {
    match e {
        aurora_secrets::SecretError::StoreUnavailable => {
            RefreshError::StoreUnavailable("secrets".into())
        }
        _ => RefreshError::Unavailable,
    }
}

#[cfg(test)]
mod token_url_tests {
    use super::check_templated_token_url;

    #[test]
    fn templated_token_url_must_stay_in_zone() {
        assert!(
            check_templated_token_url("https://acme.example.com/oauth/token", "*.example.com")
                .is_ok()
        );
        assert!(
            check_templated_token_url("https://evil.test/oauth/token", "*.example.com").is_err()
        );
        assert!(
            check_templated_token_url("http://acme.example.com/oauth/token", "*.example.com")
                .is_err()
        );
        assert!(
            check_templated_token_url("https://example.com/oauth/token", "*.example.com").is_err()
        );
        for bad in [
            "https://10.0.0.1/oauth/token",
            "https://[::1]/oauth/token",
            "https://evil.test@acme.example.com/oauth/token",
            "https://acme.example.com@evil.test/oauth/token",
            "https://acme.example.com:8443/oauth/token",
        ] {
            assert!(
                check_templated_token_url(bad, "*.example.com").is_err(),
                "{bad}"
            );
        }
    }
}
