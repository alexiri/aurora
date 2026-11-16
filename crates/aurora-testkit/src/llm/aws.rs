//! Shared half of the AWS mocks (`18 §Additional LLM providers`): a SigV4 **verifier** that recomputes the signature from
//! a known secret over what the request really carried (it shares no code with the platform's signer, so a signing bug
//! cannot verify itself), and the AWS event-stream framing.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = Hmac::<Sha256>::new_from_slice(key).unwrap();
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

/// Percent-encode everything but the unreserved set (and `/` when `keep_slash`).
fn enc(s: &str, keep_slash: bool) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'.' | b'_' | b'~')
            || (keep_slash && b == b'/')
        {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// What a verified request was signed with.
#[derive(Debug, Clone, Default)]
pub struct Verified {
    pub access_key: String,
    pub region: String,
    pub service: String,
    pub session_token: Option<String>,
}

/// Why a request did not verify (the mock answers the matching AWS error type).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No usable `Authorization: AWS4-HMAC-SHA256 …`.
    Unsigned,
    /// The access key id is not one the mock knows (`UnrecognizedClientException` / `InvalidClientTokenId`).
    UnknownKey(String),
    /// The key is known but the recomputed signature differs (`InvalidSignatureException` / `SignatureDoesNotMatch`).
    BadSignature,
    /// A session key without its token, or a wrong token.
    BadToken,
}

/// Verify a request signed with SigV4. `lookup(access_key)` → `(secret, session_token it must carry)`. The canonical
/// request is rebuilt from the headers named in `SignedHeaders` exactly as received, the body's SHA-256 (or the
/// `x-amz-content-sha256` header it names), and the path encoded a second time (non-S3 services).
pub fn verify(
    method: &str,
    path: &str,
    query: Option<&str>,
    headers: &[(String, String)],
    body: &[u8],
    lookup: impl Fn(&str) -> Option<(String, Option<String>)>,
) -> Result<Verified, Refusal> {
    let get = |n: &str| {
        headers
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.as_str())
    };
    let auth = get("authorization")
        .and_then(|a| a.strip_prefix("AWS4-HMAC-SHA256 "))
        .ok_or(Refusal::Unsigned)?;
    let part = |name: &str| {
        auth.split(',')
            .map(str::trim)
            .find_map(|p| p.strip_prefix(&format!("{name}=")))
            .map(str::to_owned)
    };
    let cred = part("Credential").ok_or(Refusal::Unsigned)?;
    let signed = part("SignedHeaders").ok_or(Refusal::Unsigned)?;
    let sig = part("Signature").ok_or(Refusal::Unsigned)?;
    let mut c = cred.split('/');
    let (ak, date, region, service) = (
        c.next().ok_or(Refusal::Unsigned)?.to_owned(),
        c.next().ok_or(Refusal::Unsigned)?.to_owned(),
        c.next().ok_or(Refusal::Unsigned)?.to_owned(),
        c.next().ok_or(Refusal::Unsigned)?.to_owned(),
    );
    let (secret, want_token) = lookup(&ak).ok_or_else(|| Refusal::UnknownKey(ak.clone()))?;
    let token = get("x-amz-security-token").map(str::to_owned);
    if want_token != token {
        return Err(Refusal::BadToken);
    }
    let amz_date = get("x-amz-date").ok_or(Refusal::Unsigned)?;
    let mut canon_headers = String::new();
    for h in signed.split(';') {
        let v = get(h).ok_or(Refusal::BadSignature)?;
        canon_headers.push_str(&format!("{h}:{}\n", v.trim()));
    }
    let payload = get("x-amz-content-sha256")
        .filter(|_| signed.split(';').any(|h| h == "x-amz-content-sha256"))
        .map_or_else(|| hex(&Sha256::digest(body)), str::to_owned);
    let mut q: Vec<(String, String)> = query
        .unwrap_or("")
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (enc(k, false), enc(v, false))
        })
        .collect();
    q.sort();
    let canon_query = q
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let canonical = format!(
        "{method}\n{}\n{canon_query}\n{canon_headers}\n{signed}\n{payload}",
        enc(path, true)
    );
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex(&Sha256::digest(canonical.as_bytes()))
    );
    let k = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k = hmac(&k, region.as_bytes());
    let k = hmac(&k, service.as_bytes());
    let k = hmac(&k, b"aws4_request");
    if hex(&hmac(&k, to_sign.as_bytes())) != sig {
        return Err(Refusal::BadSignature);
    }
    Ok(Verified {
        access_key: ak,
        region,
        service,
        session_token: token,
    })
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for b in data {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// One `application/vnd.amazon.eventstream` message with string headers and a payload (prelude and message CRCs).
pub fn event_frame(headers: &[(&str, &str)], payload: &[u8]) -> Vec<u8> {
    let mut hb = Vec::new();
    for (n, v) in headers {
        hb.push(u8::try_from(n.len()).unwrap());
        hb.extend_from_slice(n.as_bytes());
        hb.push(7); // string
        hb.extend_from_slice(&u16::try_from(v.len()).unwrap().to_be_bytes());
        hb.extend_from_slice(v.as_bytes());
    }
    let total = u32::try_from(16 + hb.len() + payload.len()).unwrap();
    let mut m = Vec::new();
    m.extend_from_slice(&total.to_be_bytes());
    m.extend_from_slice(&u32::try_from(hb.len()).unwrap().to_be_bytes());
    let prelude_crc = crc32(&m);
    m.extend_from_slice(&prelude_crc.to_be_bytes());
    m.extend_from_slice(&hb);
    m.extend_from_slice(payload);
    let msg_crc = crc32(&m);
    m.extend_from_slice(&msg_crc.to_be_bytes());
    m
}
