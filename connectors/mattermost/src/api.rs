//! Thin Mattermost REST client plus the neutral error mapping (docs/20 §Operations).

use std::time::Duration;

use aurora_protocol::connector::{ErrorCode, OpError};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;

pub type R<T> = Result<T, OpError>;

const TIMEOUT: Duration = Duration::from_secs(30);

/// Maps an HTTP status to the neutral error code (docs/20 error mapping).
/// `retry_after` is the number of seconds until the rate limit resets.
pub fn map_status(status: u16, body: &str, retry_after: Option<u32>) -> OpError {
    let code = match status {
        401 => ErrorCode::CredentialDead,
        403 => ErrorCode::Forbidden,
        404 => ErrorCode::NotFound,
        413 => ErrorCode::TooLarge,
        429 => ErrorCode::RateLimited,
        408 | 500..=599 => ErrorCode::Unavailable,
        _ => ErrorCode::Invalid,
    };
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default();
    let mut e = OpError::new(
        code,
        format!("mattermost answered {status}{}", suffix(&detail)),
    );
    if status == 429 {
        e.retry_after_secs = Some(retry_after.unwrap_or(30).clamp(1, 3600));
    }
    e
}

fn suffix(detail: &str) -> String {
    if detail.is_empty() {
        String::new()
    } else {
        let short: String = detail.chars().take(200).collect();
        format!(": {short}")
    }
}

/// A transport failure (connect, TLS, timeout) is `unavailable`.
pub fn map_transport(e: &reqwest::Error) -> OpError {
    OpError::new(
        ErrorCode::Unavailable,
        format!("mattermost unreachable: {}", clean(e)),
    )
}

/// The error text without the request URL (it may carry query data).
fn clean(e: &reqwest::Error) -> String {
    let s = e.to_string();
    match e.url() {
        Some(u) => s.replace(u.as_str(), "<url>"),
        None => s,
    }
}

/// Mattermost ids are 26 chars of `[a-z0-9]`; anything else cannot exist.
pub fn is_mm_id(s: &str) -> bool {
    s.len() == 26
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// A client for one Mattermost server (`base` has no trailing slash).
#[derive(Clone)]
pub struct Mm {
    http: reqwest::Client,
    base: String,
}

impl Mm {
    pub fn new(server_url: &str) -> Result<Self, String> {
        let base = server_url.trim().trim_end_matches('/').to_owned();
        let url = reqwest::Url::parse(&base).map_err(|e| format!("invalid server_url: {e}"))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err("server_url must be an http(s) URL".into());
        }
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { http, base })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn host(&self) -> String {
        reqwest::Url::parse(&self.base)
            .ok()
            .map(|u| match (u.host_str(), u.port()) {
                (Some(h), Some(p)) => format!("{h}:{p}"),
                (Some(h), None) => h.to_owned(),
                _ => String::new(),
            })
            .unwrap_or_default()
    }

    /// `ws(s)://host/api/v4/websocket`.
    pub fn websocket_url(&self) -> String {
        let swapped = if let Some(rest) = self.base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.base.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            self.base.clone()
        };
        format!("{swapped}/api/v4/websocket")
    }

    pub async fn send(
        &self,
        token: &str,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> R<reqwest::Response> {
        let mut rb = self
            .http
            .request(method, format!("{}/api/v4{path}", self.base))
            .bearer_auth(token)
            .query(query);
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let resp = rb.send().await.map_err(|e| map_transport(&e))?;
        check(resp).await
    }

    pub async fn json<T: DeserializeOwned>(
        &self,
        token: &str,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> R<T> {
        let resp = self.send(token, method, path, query, body).await?;
        resp.json::<T>().await.map_err(|e| {
            OpError::new(
                ErrorCode::Unavailable,
                format!("unexpected mattermost response: {}", clean(&e)),
            )
        })
    }

    pub async fn get(&self, token: &str, path: &str) -> R<Value> {
        self.json(token, Method::GET, path, &[], None).await
    }

    pub async fn get_q(&self, token: &str, path: &str, q: &[(&str, &str)]) -> R<Value> {
        self.json(token, Method::GET, path, q, None).await
    }

    pub async fn post(&self, token: &str, path: &str, body: &Value) -> R<Value> {
        self.json(token, Method::POST, path, &[], Some(body)).await
    }

    /// Raw-body upload (`POST /files?channel_id=&filename=`); returns the file id.
    pub async fn upload_file(
        &self,
        token: &str,
        channel_id: &str,
        filename: &str,
        bytes: Vec<u8>,
    ) -> R<String> {
        let resp = self
            .http
            .post(format!("{}/api/v4/files", self.base))
            .bearer_auth(token)
            .query(&[("channel_id", channel_id), ("filename", filename)])
            .header("content-type", "application/octet-stream")
            .body(bytes)
            .send()
            .await
            .map_err(|e| map_transport(&e))?;
        let resp = check(resp).await?;
        let v: Value = resp.json().await.map_err(|e| map_transport(&e))?;
        v.pointer("/file_infos/0/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                OpError::new(ErrorCode::Unavailable, "upload answered without a file id")
            })
    }

    /// `POST /users/{id}/image` (multipart field `image`).
    pub async fn set_user_image(
        &self,
        token: &str,
        user_id: &str,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> R<()> {
        let boundary = format!("aurora{}", uuid::Uuid::now_v7().simple());
        let ext = content_type.rsplit('/').next().unwrap_or("png");
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"avatar.{ext}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(&bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let resp = self
            .http
            .post(format!("{}/api/v4/users/{user_id}/image", self.base))
            .bearer_auth(token)
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
            .send()
            .await
            .map_err(|e| map_transport(&e))?;
        check(resp).await.map(|_| ())
    }

    /// Streams `GET /files/{id}` into memory, refusing more than `max_bytes`.
    pub async fn download_file(&self, token: &str, id: &str, max_bytes: u64) -> R<Vec<u8>> {
        let mut resp = self
            .send(token, Method::GET, &format!("/files/{id}"), &[], None)
            .await?;
        let mut out: Vec<u8> = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| map_transport(&e))? {
            if out.len() as u64 + chunk.len() as u64 > max_bytes {
                return Err(OpError::new(
                    ErrorCode::TooLarge,
                    "file exceeds the operation's max_bytes",
                ));
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }
}

async fn check(resp: reqwest::Response) -> R<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let retry_after = resp
        .headers()
        .get("x-ratelimit-reset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u32>().ok());
    let body = resp.text().await.unwrap_or_default();
    Err(map_status(status_u16(status), &body, retry_after))
}

fn status_u16(s: StatusCode) -> u16 {
    s.as_u16()
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping_follows_the_spec_table() {
        let code = |s| map_status(s, "", None).code;
        assert_eq!(code(401), ErrorCode::CredentialDead);
        assert_eq!(code(403), ErrorCode::Forbidden);
        assert_eq!(code(404), ErrorCode::NotFound);
        assert_eq!(code(413), ErrorCode::TooLarge);
        assert_eq!(code(429), ErrorCode::RateLimited);
        assert_eq!(code(500), ErrorCode::Unavailable);
        assert_eq!(code(503), ErrorCode::Unavailable);
        assert_eq!(code(408), ErrorCode::Unavailable);
        assert_eq!(code(400), ErrorCode::Invalid);
        assert_eq!(code(422), ErrorCode::Invalid);
    }

    #[test]
    fn rate_limit_carries_retry_after() {
        let e = map_status(429, "", Some(7));
        assert_eq!(e.retry_after_secs, Some(7));
        let e = map_status(429, "", None);
        assert_eq!(e.retry_after_secs, Some(30));
        let e = map_status(429, "", Some(0));
        assert_eq!(e.retry_after_secs, Some(1));
        assert_eq!(map_status(503, "", Some(7)).retry_after_secs, None);
    }

    #[test]
    fn error_detail_is_included_and_bounded() {
        let body = format!(r#"{{"message":"{}"}}"#, "x".repeat(500));
        let e = map_status(403, &body, None);
        assert!(e.message.starts_with("mattermost answered 403: xxx"));
        assert!(e.message.chars().count() < 260);
    }

    #[test]
    fn mm_ids() {
        assert!(is_mm_id("abcdefghijklmnopqrstuvwxyz"));
        assert!(!is_mm_id("nope"));
        assert!(!is_mm_id("ABCDEFGHIJKLMNOPQRSTUVWXYZ"));
    }

    #[test]
    fn urls() {
        let mm = Mm::new("https://chat.example.com:8443/").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(mm.host(), "chat.example.com:8443");
        assert_eq!(
            mm.websocket_url(),
            "wss://chat.example.com:8443/api/v4/websocket"
        );
        let mm = Mm::new("http://127.0.0.1:8065").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(mm.websocket_url(), "ws://127.0.0.1:8065/api/v4/websocket");
        assert!(Mm::new("ftp://x").is_err());
        assert!(Mm::new("not a url").is_err());
    }
}
