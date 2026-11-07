//! Finalize stage: `aws_sigv4` (docs/09 §5). Signs the final request with payload hash
//! `UNSIGNED-PAYLOAD` (streamed bodies). Implemented over HMAC-SHA256 directly.

use super::Unavailable;
use super::template::{Dest, Vars, render};
use aurora_registry::manifest::AwsSigv4;
use hmac::{Hmac, Mac};
use http::{HeaderMap, HeaderName, HeaderValue, Method};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

const SIGV4_ENC: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');
const SIGV4_PATH: &AsciiSet = &SIGV4_ENC.remove(b'/');

fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    // HMAC accepts keys of any length.
    let Ok(mut m) = HmacSha256::new_from_slice(key) else {
        return Vec::new();
    };
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

pub struct SignParams<'a> {
    pub method: &'a Method,
    pub host_header: &'a str,
    /// Path as sent (already percent-encoded once).
    pub path: &'a str,
    pub query: Option<&'a str>,
    pub region: &'a str,
    pub service: &'a str,
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub session_token: Option<&'a str>,
    /// `YYYYMMDDTHHMMSSZ`
    pub amz_date: &'a str,
    pub payload_hash: &'a str,
    /// Also sign `x-amz-content-sha256` (the gateway does; the AWS suite vector does not).
    pub sign_content_sha256: bool,
}

pub struct Signed {
    pub authorization: String,
    pub signature: String,
}

fn canonical_query(q: Option<&str>) -> String {
    let mut pairs: Vec<(String, String)> = q
        .unwrap_or("")
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            let dec = |s: &str| {
                percent_encoding::percent_decode_str(s)
                    .decode_utf8_lossy()
                    .into_owned()
            };
            (
                utf8_percent_encode(&dec(k), SIGV4_ENC).to_string(),
                utf8_percent_encode(&dec(v), SIGV4_ENC).to_string(),
            )
        })
        .collect();
    pairs.sort();
    pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

pub fn sign(p: &SignParams<'_>) -> Signed {
    let date = &p.amz_date[..8.min(p.amz_date.len())];
    // Non-S3 services encode the (already encoded) path a second time.
    let canon_path = if p.service == "s3" {
        p.path.to_owned()
    } else {
        utf8_percent_encode(p.path, SIGV4_PATH).to_string()
    };
    let mut hdrs: Vec<(&str, String)> = vec![("host", p.host_header.to_owned())];
    if p.sign_content_sha256 {
        hdrs.push(("x-amz-content-sha256", p.payload_hash.to_owned()));
    }
    hdrs.push(("x-amz-date", p.amz_date.to_owned()));
    if let Some(t) = p.session_token {
        hdrs.push(("x-amz-security-token", t.to_owned()));
    }
    hdrs.sort_by(|a, b| a.0.cmp(b.0));
    let canon_headers: String = hdrs.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed = hdrs.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(";");
    let canonical = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        p.method,
        canon_path,
        canonical_query(p.query),
        canon_headers,
        signed,
        p.payload_hash
    );
    let scope = format!("{date}/{}/{}/aws4_request", p.region, p.service);
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{scope}\n{}",
        p.amz_date,
        hex(&Sha256::digest(canonical.as_bytes()))
    );
    let k = hmac(format!("AWS4{}", p.secret_key).as_bytes(), date.as_bytes());
    let k = hmac(&k, p.region.as_bytes());
    let k = hmac(&k, p.service.as_bytes());
    let k = hmac(&k, b"aws4_request");
    let signature = hex(&hmac(&k, to_sign.as_bytes()));
    Signed {
        authorization: format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={signature}",
            p.access_key
        ),
        signature,
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// AWS Signature V4 test suite, `get-vanilla`.
    #[test]
    fn aws_suite_get_vanilla() {
        let s = sign(&SignParams {
            method: &Method::GET,
            host_header: "example.amazonaws.com",
            path: "/",
            query: None,
            region: "us-east-1",
            service: "service",
            access_key: "AKIDEXAMPLE",
            secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            session_token: None,
            amz_date: "20150830T123600Z",
            payload_hash: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            sign_content_sha256: false,
        });
        assert_eq!(
            s.signature,
            "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
        assert_eq!(
            s.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    const SK: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn suite(method: &Method, path: &str, query: Option<&str>) -> String {
        sign(&SignParams {
            method,
            host_header: "example.amazonaws.com",
            path,
            query,
            region: "us-east-1",
            service: "service",
            access_key: "AKIDEXAMPLE",
            secret_key: SK,
            session_token: None,
            amz_date: "20150830T123600Z",
            payload_hash: EMPTY,
            sign_content_sha256: false,
        })
        .signature
    }

    /// AWS suite `get-vanilla-query` (empty query string).
    #[test]
    fn aws_suite_get_vanilla_query() {
        assert_eq!(
            suite(&Method::GET, "/", None),
            "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    /// AWS suite `get-unreserved`: unreserved characters stay unescaped.
    #[test]
    fn aws_suite_get_unreserved() {
        assert_eq!(
            suite(
                &Method::GET,
                "/-._~0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
                None
            ),
            "07ef7494c76fa4850883e2b006601f940f8a34d404d0cfa977f52a65bbf5f24f"
        );
    }

    /// AWS suite `post-header-key-case`.
    #[test]
    fn aws_suite_post_header_key_case() {
        assert_eq!(
            suite(&Method::POST, "/", None),
            "5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b"
        );
    }
}
