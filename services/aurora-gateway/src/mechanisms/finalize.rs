//! Finalize stage: `aws_sigv4` (docs/09 §5). Signs the final request with payload hash
//! `UNSIGNED-PAYLOAD` (streamed bodies). The signer itself is `aurora_llm::sigv4` (one signer, D-261).

use super::Unavailable;
use super::template::{Dest, Vars, render};
pub use aurora_llm::sigv4::{SignParams, Signed, sign};
use aurora_registry::manifest::AwsSigv4;
use http::{HeaderMap, HeaderName, HeaderValue, Method};

pub const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";

/// Sign the final request in place.
#[allow(clippy::too_many_arguments)]
pub fn finalize(
    f: &AwsSigv4,
    v: &Vars<'_>,
    method: &Method,
    host_header: &str,
    path: &str,
    query: Option<&str>,
    headers: &mut HeaderMap,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), Unavailable> {
    let region = render(&f.region, Dest::Raw, v)?.text;
    let access = render(&f.access_key, Dest::Raw, v)?;
    let secret = render(&f.secret_key, Dest::Raw, v)?;
    let token = match &f.session_token {
        Some(t) => Some(render(t, Dest::Raw, v)?.text).filter(|t| !t.is_empty()),
        None => None,
    };
    if region.is_empty() || access.text.is_empty() || secret.text.is_empty() {
        return Err(Unavailable);
    }
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let signed = sign(&SignParams {
        method,
        host_header,
        path,
        query,
        region: &region,
        service: &f.service,
        access_key: &access.text,
        secret_key: &secret.text,
        session_token: token.as_deref(),
        amz_date: &amz_date,
        payload_hash: UNSIGNED_PAYLOAD,
        sign_content_sha256: true,
    });
    for n in [
        "authorization",
        "x-amz-date",
        "x-amz-content-sha256",
        "x-amz-security-token",
    ] {
        headers.remove(n);
    }
    let set = |headers: &mut HeaderMap, n: &'static str, val: &str| -> Result<(), Unavailable> {
        let mut hv = HeaderValue::from_str(val).map_err(|_| Unavailable)?;
        hv.set_sensitive(true);
        headers.insert(HeaderName::from_static(n), hv);
        Ok(())
    };
    set(headers, "authorization", &signed.authorization)?;
    set(headers, "x-amz-date", &amz_date)?;
    set(headers, "x-amz-content-sha256", UNSIGNED_PAYLOAD)?;
    if let Some(t) = token {
        set(headers, "x-amz-security-token", &t)?;
    }
    Ok(())
}
