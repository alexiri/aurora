//! The template package download (C-1108 step 5): https only, through the guarded instance
//! client (the destination guard runs at dial time), at most 3 redirects — each https and
//! guarded — the source's token as HTTP Basic on the repository's origin only (never after a
//! redirect to another origin, the C-1125 origin rule), the body streamed under
//! `TEMPLATE_ARCHIVE_MAX`, the whole download under `TEMPLATE_FETCH_TIMEOUT`.

use aurora_core::limits::TEMPLATE_ARCHIVE_MAX;
use aurora_core::sanitize;
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};

use crate::domains::connect::env::Env;
use crate::domains::skill_libraries::git::Token;

/// The whole download's budget (C-1108): the client's per-request timeout is this too.
pub const FETCH_TIMEOUT: std::time::Duration = aurora_core::limits::TEMPLATE_FETCH_TIMEOUT;

/// Why a download did not deliver the package. `NotFound` and `Failed` keep the version
/// `awaiting_package`; `TooLarge` invalidates it at once (C-1108 step 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    /// 404 or 410: `package_not_found`.
    NotFound,
    /// Over `TEMPLATE_ARCHIVE_MAX`: `too_large`, the version is `invalid` at once.
    TooLarge,
    /// Everything else: `download_failed`, with a sanitized reason (never credentials).
    Failed(String),
}

/// The origin a URL belongs to (the token rule compares this, as C-1125 does).
fn origin(u: &url::Url) -> url::Origin {
    u.origin()
}

/// Downloads one package. `repo_origin` is the origin of the source's stored git address;
/// the Basic header goes on a request only while the (logical) request URL has that origin.
pub async fn package(
    env: &Env,
    repo_origin: Option<&url::Origin>,
    url: &str,
    token: Option<&Token>,
) -> Result<Vec<u8>, DownloadError> {
    let failed = |m: String| DownloadError::Failed(sanitize::one_line(&m, 120));
    let mut logical: url::Url =
        url::Url::parse(url).map_err(|_| failed("bad package URL".into()))?;
    for _hop in 0..=3 {
        // The logical URL decides the token and the https rule; `prepare` applies the test
        // override and refuses non-https and credential URLs before any dial.
        let dial = env.prepare(logical.as_str()).map_err(|_| {
            failed("the package URL was refused (https and a public host are required)".into())
        })?;
        let mut req = env.http_instance.get(dial).timeout(FETCH_TIMEOUT);
        if let Some(t) = token
            && repo_origin.is_some_and(|r| origin(&logical) == *r)
        {
            // Basic only on the repository's own origin (SEC-146).
            let plain = format!("{}:{}", t.username, t.token.expose());
            req = req.header(
                reqwest::header::AUTHORIZATION,
                format!("Basic {}", B64.encode(plain.as_bytes())),
            );
        }
        let res = req.send().await.map_err(|e| failed(e.to_string()))?;
        if res.status().is_redirection() {
            let loc = res
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| failed("redirect without a target".into()))?;
            logical = url::Url::parse(loc).map_err(|_| failed("bad redirect target".into()))?;
            continue;
        }
        let status = res.status();
        if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
            return Err(DownloadError::NotFound);
        }
        if !status.is_success() {
            return Err(failed(format!(
                "the server answered {} {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("status")
            )));
        }
        if let Some(n) = res.content_length()
            && n > TEMPLATE_ARCHIVE_MAX as u64
        {
            return Err(DownloadError::TooLarge);
        }
        // Streamed under the cap: no chunk is buffered past it.
        let mut out: Vec<u8> = Vec::new();
        let mut body = res;
        loop {
            let chunk = body.chunk().await.map_err(|e| failed(e.to_string()))?;
            let Some(chunk) = chunk else { break };
            if out.len() + chunk.len() > TEMPLATE_ARCHIVE_MAX {
                return Err(DownloadError::TooLarge);
            }
            out.extend_from_slice(&chunk);
        }
        return Ok(out);
    }
    Err(failed("too many redirects".into()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn url(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }

    /// (C-1108 step 5) The token origin rule: the Basic header is a property of the logical
    /// URL only; a redirect to another origin drops it. Pure logic, no server.
    #[test]
    fn the_token_belongs_to_the_repository_origin() {
        let repo = origin(&url("https://git.example/cern/agent-templates.git"));
        assert_eq!(repo, origin(&url("https://git.example/other/path")));
        assert_ne!(repo, origin(&url("https://cdn.example/p.zip")));
        assert_ne!(repo, origin(&url("https://git.example:8443/p.zip")));
        assert_ne!(repo, origin(&url("http://git.example/p.zip")));
    }
}
