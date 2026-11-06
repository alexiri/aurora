//! `approval.summarize` client (docs/09 §8.2, docs/16 §W9, C-19). Called only when a request
//! is held and the tool's approval template says `summarize: hook`. The whole call (network
//! included) has a hard 1 s budget; any failure yields `None` and the caller falls back to the
//! template, then the generic summary. The hook never has egress and never sees secrets:
//! the path is already redacted, the query values are masked and sensitive or injected
//! headers are dropped.

use aurora_core::limits::HOOK_WALL_SUMMARIZE;
use aurora_summary::{ApprovalSummary, SummaryField, SummarySource};
use serde::Deserialize;
use serde_json::{Value, json};

/// Everything one summarize call needs.
pub struct SummarizeCall<'a> {
    pub package_id: &'a str,
    pub package_digest: &'a str,
    pub app_id: &'a str,
    pub tool_id: Option<&'a str>,
    pub method: &'a str,
    pub host: &'a str,
    /// Redacted, without query.
    pub path: &'a str,
    pub query: Option<&'a str>,
    pub headers: &'a [(String, String)],
    /// Lower-case header names the matched host injects (never forwarded to the hook).
    pub injected_headers: &'a [String],
    pub body_prefix: &'a [u8],
    pub truncated: bool,
}

#[async_trait::async_trait]
pub trait Summarizer: Send + Sync + 'static {
    /// `None` on any error or timeout.
    async fn summarize(&self, call: SummarizeCall<'_>) -> Option<ApprovalSummary>;
}

/// HTTP client of the hook host (`POST {base}/internal/v1/hooks/approval.summarize`).
pub struct HttpSummarizer {
    base: String,
    secret: String,
    http: reqwest::Client,
}

impl HttpSummarizer {
    pub fn new(base_url: &str, internal_secret: &str) -> Self {
        Self {
            base: base_url.trim_end_matches('/').to_owned(),
            secret: internal_secret.to_owned(),
            http: reqwest::Client::builder()
                .timeout(HOOK_WALL_SUMMARIZE)
                .build()
                .unwrap_or_default(),
        }
    }
}

fn kv_pairs<'a>(it: impl Iterator<Item = (&'a str, &'a str)>) -> Value {
    Value::Array(
        it.map(|(k, v)| json!({ "key": k, "value": v }))
            .collect::<Vec<_>>(),
    )
}

fn sensitive(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(
        n.as_str(),
        "authorization" | "cookie" | "set-cookie" | "x-api-key" | "host"
    ) || n.starts_with("proxy-")
        || n.starts_with("x-aurora-")
}

/// W9 request body (exposed for tests).
pub fn request_body(c: &SummarizeCall<'_>) -> Value {
    let masked = c.query.map(aurora_summary::mask_query).unwrap_or_default();
    let query = kv_pairs(
        masked
            .split('&')
            .filter(|s| !s.is_empty())
            .map(|p| p.split_once('=').unwrap_or((p, ""))),
    );
    let headers = kv_pairs(
        c.headers
            .iter()
            .filter(|(k, _)| {
                !sensitive(k) && !c.injected_headers.iter().any(|h| h.eq_ignore_ascii_case(k))
            })
            .map(|(k, v)| (k.as_str(), v.as_str())),
    );
    json!({
        "package_id": c.package_id,
        "package_digest": c.package_digest,
        "app_id": c.app_id,
        "input": {
            "app_id": c.app_id,
            "tool_id": c.tool_id,
            "method": c.method,
            "host": c.host,
            "path": c.path,
            "query": query,
            "headers": headers,
            "body_prefix": c.body_prefix,
            "truncated": c.truncated,
        },
    })
}

#[derive(Deserialize)]
/// Hook host emits `{label, value}` for summary fields (C-19 says `{key, value}`): accept both.
struct Kv {
    #[serde(alias = "label")]
    key: String,
    value: String,
}

#[derive(Deserialize)]
struct Out {
    title: String,
    #[serde(default)]
    fields: Vec<Kv>,
    text: Option<String>,
    link: Option<String>,
    group: Option<String>,
}

#[derive(Deserialize)]
struct Reply {
    ok: Option<Out>,
}

#[async_trait::async_trait]
impl Summarizer for HttpSummarizer {
    async fn summarize(&self, call: SummarizeCall<'_>) -> Option<ApprovalSummary> {
        let body = request_body(&call);
        let fut = async {
            let resp = self
                .http
                .post(format!(
                    "{}/internal/v1/hooks/approval.summarize",
                    self.base
                ))
                .bearer_auth(&self.secret)
                .json(&body)
                .send()
                .await
                .map_err(|e| tracing::warn!(error = ?e, "approval.summarize request failed"))
                .ok()?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                tracing::warn!(%status, reply = %text.chars().take(200).collect::<String>(), "approval.summarize hook error");
                return None;
            }
            match resp.json::<Reply>().await {
                Ok(r) => r.ok,
                Err(e) => {
                    tracing::warn!(error = %e, "approval.summarize reply malformed");
                    None
                }
            }
        };
        // The client timeout covers connect + response head; this bounds the whole call.
        let out = tokio::time::timeout(HOOK_WALL_SUMMARIZE, fut)
            .await
            .ok()
            .flatten()?;
        let s = ApprovalSummary {
            title: out.title,
            fields: out
                .fields
                .into_iter()
                .map(|f| SummaryField {
                    label: f.key,
                    value: f.value,
                })
                .collect(),
            text: out.text,
            link: out.link,
            group: out.group,
            source: SummarySource::Hook,
        }
        .clamp();
        (!s.title.is_empty()).then_some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_drops_secrets_and_masks_query() {
        let h = vec![
            ("Authorization".to_owned(), "Bearer x".to_owned()),
            ("X-Injected".to_owned(), "y".to_owned()),
            ("Content-Type".to_owned(), "application/json".to_owned()),
        ];
        let inj = vec!["x-injected".to_owned()];
        let b = request_body(&SummarizeCall {
            package_id: "p",
            package_digest: "d",
            app_id: "a",
            tool_id: Some("t"),
            method: "POST",
            host: "h",
            path: "/v1",
            query: Some("a=1&access_token=SECRET"),
            headers: &h,
            injected_headers: &inj,
            body_prefix: b"hi",
            truncated: false,
        });
        let s = b.to_string();
        assert!(!s.contains("SECRET") && !s.contains("Bearer") && !s.contains("X-Injected"));
        assert!(s.contains("application/json"));
        assert_eq!(b["input"]["body_prefix"], json!([104, 105]));
    }
}
