//! API-side W9 client for `connect.authorize` / `connect.exchange` (C-19).
use aurora_core::limits::HOOK_WALL;
use base64::Engine;
use serde_json::{Map, Value, json};

pub struct HookClient {
    base: String,
    bearer: String,
    http: reqwest::Client,
}

#[derive(Debug)]
pub struct HookFail(pub String);

pub struct Authorized {
    pub url: String,
    pub continuation: Option<String>,
}

pub struct Connected {
    pub credentials: Map<String, Value>,
    pub metadata: Map<String, Value>,
    pub scopes: Vec<String>,
    pub label: String,
    pub expires_at: Option<i64>,
    pub tenant_host: Option<String>,
}

pub enum Exchanged {
    Connected(Connected),
    Continue { url: String, continuation: String },
}

pub fn kv(m: &Map<String, Value>) -> Value {
    Value::Array(
        m.iter()
            .filter_map(|(k, v)| v.as_str().map(|s| json!({"key": k, "value": s})))
            .collect(),
    )
}

fn from_kv(v: Option<&Value>) -> Result<Map<String, Value>, HookFail> {
    let mut out = Map::new();
    for e in v.and_then(Value::as_array).map_or(&[][..], Vec::as_slice) {
        match (
            e.get("key").and_then(Value::as_str),
            e.get("value").and_then(Value::as_str),
        ) {
            (Some(k), Some(val)) => {
                out.insert(k.to_owned(), Value::String(val.to_owned()));
            }
            _ => return Err(fail("malformed key/value list")),
        }
    }
    Ok(out)
}

fn fail(m: &str) -> HookFail {
    HookFail(m.to_owned())
}

pub struct Call<'a> {
    pub package_id: &'a str,
    pub package_digest: &'a str,
    pub app_id: &'a str,
}

impl HookClient {
    pub fn new(base: &str, secret: &[u8]) -> Result<Self, String> {
        Ok(Self {
            base: base.trim_end_matches('/').to_owned(),
            bearer: base64::engine::general_purpose::STANDARD.encode(secret),
            http: reqwest::Client::builder()
                .timeout(HOOK_WALL)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| format!("hook client: {e}"))?,
        })
    }

    async fn call(&self, hook: &str, c: &Call<'_>, input: Value) -> Result<Value, HookFail> {
        let body = json!({"package_id": c.package_id, "package_digest": c.package_digest,
            "app_id": c.app_id, "input": input});
        let resp = self
            .http
            .post(format!("{}/internal/v1/hooks/{hook}", self.base))
            .bearer_auth(&self.bearer)
            .json(&body)
            .send()
            .await
            .map_err(|_| fail("hook host unavailable"))?;
        let v: Value = resp
            .json()
            .await
            .map_err(|_| fail("hook host unavailable"))?;
        if let Some(ok) = v.get("ok") {
            return Ok(ok.clone());
        }
        let code = v
            .pointer("/error/code")
            .and_then(Value::as_str)
            .unwrap_or("internal");
        Err(HookFail(format!("hook failed: {code}")))
    }

    pub async fn authorize(&self, c: &Call<'_>, input: Value) -> Result<Authorized, HookFail> {
        let ok = self.call("connect.authorize", c, input).await?;
        Ok(Authorized {
            url: ok
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| fail("malformed authorize output"))?
                .to_owned(),
            continuation: ok
                .get("continuation")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }

    pub async fn exchange(&self, c: &Call<'_>, input: Value) -> Result<Exchanged, HookFail> {
        let ok = self.call("connect.exchange", c, input).await?;
        if let Some(k) = ok.get("continue") {
            let s = |n: &str| k.get(n).and_then(Value::as_str).map(str::to_owned);
            return match (s("url"), s("continuation")) {
                (Some(url), Some(continuation)) => Ok(Exchanged::Continue { url, continuation }),
                _ => Err(fail("malformed continue output")),
            };
        }
        let k = ok
            .get("connected")
            .ok_or_else(|| fail("malformed exchange output"))?;
        let pick = |a: &str, b: &str| k.get(a).or_else(|| k.get(b));
        Ok(Exchanged::Connected(Connected {
            credentials: from_kv(k.get("credentials"))?,
            metadata: from_kv(k.get("metadata"))?,
            scopes: k
                .get("scopes")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            label: k
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            expires_at: pick("expires_at", "expires-at").and_then(Value::as_i64),
            tenant_host: pick("tenant_host", "tenant-host")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }))
    }
}
