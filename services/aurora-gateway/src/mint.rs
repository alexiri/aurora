//! `token.mint` client for `token: hook` apps (docs/09 §8, docs/16 §W9). The hook host is
//! reached over HTTP; the contract sits behind [`Minter`] so tests use a stub.

use aurora_core::limits::HOOK_WALL;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// Everything the hook sees for one connection (docs/09 §8.2).
pub struct MintInput {
    pub package_id: String,
    pub package_digest: String,
    pub app_id: String,
    pub credentials: Map<String, Value>,
    pub metadata: Value,
    pub tenant_host: Option<String>,
    /// Non-secret and secret OAuth client fields, when the app declares a client.
    pub client: Map<String, Value>,
    pub client_secret: Map<String, Value>,
    pub context: BTreeMap<String, String>,
}

#[derive(Debug, PartialEq)]
pub struct MintOutput {
    pub credentials: Map<String, Value>,
    /// Unix seconds.
    pub expires_at: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MintError {
    /// The hook asked for the user to reconnect.
    Reauth,
    /// Transport failure, timeout, hook error or malformed output.
    Unavailable,
}

#[async_trait::async_trait]
pub trait Minter: Send + Sync + 'static {
    async fn mint(&self, input: MintInput) -> Result<MintOutput, MintError>;
}

/// HTTP client of the hook host (`POST {base}/internal/v1/hooks/token.mint`).
pub struct HttpMinter {
    base: String,
    secret: String,
    key_id: Option<String>,
    http: reqwest::Client,
}

impl HttpMinter {
    /// Name the internal-secret ring entry presented (`X-Aurora-Key-Id`, RFE-007).
    #[must_use]
    pub fn with_key_id(mut self, id: &str) -> Self {
        self.key_id = Some(id.to_owned());
        self
    }

    pub fn new(base_url: &str, internal_secret: &str) -> Self {
        Self {
            base: base_url.trim_end_matches('/').to_owned(),
            secret: internal_secret.to_owned(),
            key_id: None,
            http: reqwest::Client::builder()
                .timeout(HOOK_WALL)
                .build()
                .unwrap_or_default(),
        }
    }
}

#[derive(Deserialize)]
struct HookError {
    code: String,
}

#[derive(Deserialize)]
struct HookReply {
    ok: Option<Value>,
    error: Option<HookError>,
}

fn kv(m: &Map<String, Value>) -> Value {
    Value::Array(
        m.iter()
            .filter_map(|(k, v)| v.as_str().map(|s| json!({ "key": k, "value": s })))
            .collect(),
    )
}

fn from_kv(v: &Value) -> Option<Map<String, Value>> {
    let mut out = Map::new();
    for e in v.as_array()? {
        out.insert(
            e.get("key")?.as_str()?.to_owned(),
            Value::String(e.get("value")?.as_str()?.to_owned()),
        );
    }
    Some(out)
}

fn parse_output(ok: &Value) -> Option<MintOutput> {
    Some(MintOutput {
        credentials: from_kv(ok.get("credentials")?)?,
        expires_at: ok.get("expires_at").or(ok.get("expires-at"))?.as_i64()?,
    })
}

#[async_trait::async_trait]
impl Minter for HttpMinter {
    async fn mint(&self, i: MintInput) -> Result<MintOutput, MintError> {
        let md = i.metadata.as_object().cloned().unwrap_or_default();
        let ctx: Map<String, Value> = i
            .context
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect();
        let body = json!({
            "package_id": i.package_id,
            "package_digest": i.package_digest,
            "app_id": i.app_id,
            "input": {
                "app_id": i.app_id,
                "credentials": kv(&i.credentials),
                "metadata": kv(&md),
                "tenant_host": i.tenant_host,
                "client": kv(&i.client),
                "client_secret": kv(&i.client_secret),
                "context": kv(&ctx),
            },
        });
        let resp = self
            .http
            .post(format!("{}/internal/v1/hooks/token.mint", self.base))
            .bearer_auth(&self.secret)
            .headers(crate::key_id_headers(self.key_id.as_deref()))
            .headers(aurora_core::telemetry::headers())
            .json(&body)
            .send()
            .await
            .map_err(|_| MintError::Unavailable)?;
        let reply: HookReply = resp.json().await.map_err(|_| MintError::Unavailable)?;
        match (reply.ok, reply.error) {
            (Some(ok), _) => parse_output(&ok).ok_or(MintError::Unavailable),
            (None, Some(e)) if e.code == "reauth_required" || e.code == "invalid_grant" => {
                Err(MintError::Reauth)
            }
            _ => Err(MintError::Unavailable),
        }
    }
}
