//! (D-261, C-807, docs/09 §5) AWS Signature Version 4 over HMAC-SHA256: the one signer of the platform. The gateway's
//! `aws_sigv4` finalizer, its Bedrock inference signing and the STS `AssumeRole` mint all call [`sign`]; the API signs
//! its Bedrock listing and `GetCallerIdentity` calls with it too. Checked against the AWS signature test suite.
use hmac::{Hmac, Mac};
use http::Method;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

const SIGV4_ENC: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');
const SIGV4_PATH: &AsciiSet = &SIGV4_ENC.remove(b'/');

/// Lowercase hexadecimal.
#[must_use]
pub fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

/// Lowercase hexadecimal SHA-256 (the `x-amz-content-sha256` payload hash).
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
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

#[must_use]
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
        sha256_hex(canonical.as_bytes())
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

/// The `Host` header value a request to `uri` carries (and is signed with): the host, with `:port` only when the port is
/// not the scheme's default.
#[must_use]
pub fn host_header(uri: &http::Uri) -> Option<String> {
    let host = uri.host()?;
    let default = match uri.scheme_str() {
        Some("https") => 443,
        Some("http") => 80,
        _ => 0,
    };
    Some(match uri.port_u16() {
        Some(p) if p != default => format!("{host}:{p}"),
        _ => host.to_owned(),
    })
}

/// Days since 1970-01-01 of a civil date (Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `YYYYMMDDTHHMMSSZ` of `t` (the `X-Amz-Date` value).
#[must_use]
pub fn amz_date(t: std::time::SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// An RFC 3339 UTC timestamp (`2026-11-15T12:00:00Z`, optional fraction) as a point in time.
#[must_use]
pub fn parse_rfc3339(s: &str) -> Option<std::time::SystemTime> {
    let s = s.trim().strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let time = time.split('.').next()?;
    let mut t = time.split(':');
    let (h, mi, sec): (i64, i64, i64) = (
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let secs = days_from_civil(y, m, day) * 86_400 + h * 3600 + mi * 60 + sec;
    u64::try_from(secs)
        .ok()
        .map(|s| std::time::UNIX_EPOCH + std::time::Duration::from_secs(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_round_trip() {
        let t = parse_rfc3339("2026-11-15T12:34:56.000Z").unwrap();
        assert_eq!(amz_date(t), "20261115T123456Z");
        assert_eq!(amz_date(std::time::UNIX_EPOCH), "19700101T000000Z");
        // 2015-08-30T12:36:00Z, the AWS suite's time.
        assert_eq!(
            amz_date(parse_rfc3339("2015-08-30T12:36:00Z").unwrap()),
            "20150830T123600Z"
        );
        assert!(parse_rfc3339("2026-13-01T00:00:00Z").is_none());
    }

    const SK: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn suite(method: &Method, path: &str, query: Option<&str>) -> Signed {
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
    }

    /// AWS Signature V4 test suite, `get-vanilla`.
    #[test]
    fn aws_suite_get_vanilla() {
        let s = suite(&Method::GET, "/", None);
        assert_eq!(
            s.signature,
            "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
        assert_eq!(
            s.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
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
            )
            .signature,
            "07ef7494c76fa4850883e2b006601f940f8a34d404d0cfa977f52a65bbf5f24f"
        );
    }

    /// AWS suite `post-header-key-case`.
    #[test]
    fn aws_suite_post_header_key_case() {
        assert_eq!(
            suite(&Method::POST, "/", None).signature,
            "5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b"
        );
    }

    #[test]
    fn host_header_keeps_a_port_only_when_it_is_not_the_default() {
        let h = |u: &str| host_header(&u.parse().unwrap()).unwrap();
        assert_eq!(
            h("https://bedrock-runtime.us-east-1.amazonaws.com/x"),
            "bedrock-runtime.us-east-1.amazonaws.com"
        );
        assert_eq!(h("https://a.example:443/x"), "a.example");
        assert_eq!(h("http://127.0.0.1:8123/x"), "127.0.0.1:8123");
        assert_eq!(h("https://a.example:8443/x"), "a.example:8443");
    }
}
