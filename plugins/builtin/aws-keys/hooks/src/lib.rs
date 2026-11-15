//! `assume_role` connect-method hooks of the `aws-keys` package (docs/09 §8, D-95, FIX-123/D-216:
//! the former `aws-role` package is now this method): assume an IAM role with `sts:AssumeRole`, signed with the
//! operator's source access key (AWS Signature Version 4, implemented here with pure-Rust SHA-256),
//! and hand the temporary session credentials to the gateway's `aws_sigv4` finalizer.
//!
//! * `connect.exchange` validates the connect form and assumes the role once, so a wrong key, role
//!   ARN or trust policy is reported when connecting, not on the first agent request.
//! * `token.mint` assumes the role again whenever the session credentials expire.
//!
//! Every call carries `ExternalId` = the `org_external_id` context variable (the confused-deputy
//! protection of the AssumeRole docs); without it the hook refuses to call AWS. The source key is
//! used only here: it is never returned as a session credential and never appears in a message.
//!
//! API: https://docs.aws.amazon.com/STS/latest/APIReference/API_AssumeRole.html
//! Signing: https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv4-create-signed-request.html
//! Endpoints: https://docs.aws.amazon.com/general/latest/gr/sts.html (regional `sts.<region>.amazonaws.com`)
use aurora_plugin_sdk::*;
use sha2::{Digest, Sha256};

const APP: &str = "aws-keys";
const STS_VERSION: &str = "2011-06-15";
const MIN_DURATION: u64 = 900;
const DEFAULT_DURATION: u64 = 3600;
const MAX_DURATION: u64 = 43_200;
const FORM_TYPE: &str = "application/x-www-form-urlencoded; charset=utf-8";

struct Guest;

/// Everything one AssumeRole call needs. Built from connect-form values or from stored metadata.
struct Settings {
    role_arn: String,
    region: String,
    duration: u64,
    session_name: String,
    external_id: String,
}

// ---- SHA-256, HMAC, hex, percent-encoding -------------------------------------------------

fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&sha256(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let inner: [u8; 32] = inner.finalize().into();
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner);
    outer.finalize().into()
}

fn hex(b: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push(H[(x >> 4) as usize] as char);
        s.push(H[(x & 15) as usize] as char);
    }
    s
}

/// RFC 3986 unreserved characters stay, everything else is `%XX` (upper-case hex).
fn pct(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

// ---- Time -----------------------------------------------------------------------------------

/// Unix seconds to `(YYYYMMDD, YYYYMMDDTHHMMSSZ)`.
fn amz_dates(t: u64) -> (String, String) {
    let days = (t / 86_400) as i64;
    let rem = t % 86_400;
    // civil from days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let date = format!("{y:04}{m:02}{d:02}");
    let full = format!("{date}T{:02}{:02}{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60);
    (date, full)
}

/// `2019-11-01T20:26:47Z` (an optional `.fff` fraction is ignored) to unix seconds.
fn parse_expiration(s: &str) -> Option<u64> {
    let s = s.trim();
    let (head, frac) = match s.split_once('.') {
        Some((h, f)) => (h, f.strip_suffix('Z')?),
        None => (s.strip_suffix('Z')?, ""),
    };
    if !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let b = head.as_bytes();
    if b.len() != 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |a: usize, z: usize| -> Option<i64> { head.get(a..z)?.parse().ok() };
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok()
}

// ---- SigV4 ----------------------------------------------------------------------------------

struct Signed {
    canonical_request: String,
    string_to_sign: String,
    signature: String,
    authorization: String,
}

/// AWS Signature Version 4. `headers` are the headers to sign (lower-case names, trimmed values);
/// they are sorted here. `query` is the already-encoded canonical query string.
#[allow(clippy::too_many_arguments)]
fn sigv4(
    method: &str,
    path: &str,
    query: &str,
    headers: &[(&str, &str)],
    payload: &[u8],
    key_id: &str,
    secret: &str,
    region: &str,
    service: &str,
    amz_date: &str,
) -> Signed {
    let mut hs: Vec<(&str, &str)> = headers.to_vec();
    hs.sort_by(|a, b| a.0.cmp(b.0));
    let canon_headers: String = hs.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = hs.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(";");
    let canonical_request = format!(
        "{method}\n{path}\n{query}\n{canon_headers}\n{signed_headers}\n{}",
        hex(&sha256(payload))
    );
    let date = &amz_date[..8];
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex(&sha256(canonical_request.as_bytes()))
    );
    let k = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k = hmac_sha256(&k, region.as_bytes());
    let k = hmac_sha256(&k, service.as_bytes());
    let k = hmac_sha256(&k, b"aws4_request");
    let signature = hex(&hmac_sha256(&k, string_to_sign.as_bytes()));
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={key_id}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
    );
    Signed {
        canonical_request,
        string_to_sign,
        signature,
        authorization,
    }
}

// ---- Validation -----------------------------------------------------------------------------

/// `arn:aws[-a-z]*:iam::<12 digits>:role/<path/name>`. Returns `(account, role name)`.
fn parse_role_arn(arn: &str) -> Option<(&str, &str)> {
    let rest = arn.strip_prefix("arn:")?;
    let (partition, rest) = rest.split_once(':')?;
    let ok_partition = partition
        .strip_prefix("aws")
        .is_some_and(|t| t.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'));
    if !ok_partition {
        return None;
    }
    let rest = rest.strip_prefix("iam::")?;
    let (account, resource) = rest.split_once(':')?;
    if account.len() != 12 || !account.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let path = resource.strip_prefix("role/")?;
    if path.is_empty()
        || path.len() > 576
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains("//")
        || !path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'=' | b',' | b'.' | b'@' | b'_' | b'-' | b'/'))
    {
        return None;
    }
    let name = path.rsplit('/').next()?;
    Some((account, name))
}

/// A commercial or GovCloud region name such as `eu-west-1` or `us-gov-west-1`.
fn valid_region(r: &str) -> bool {
    let parts: Vec<&str> = r.split('-').collect();
    r.len() <= 25
        && parts.len() >= 3
        && parts[0].len() == 2
        && parts.iter().all(|p| !p.is_empty())
        && parts[..parts.len() - 1].iter().all(|p| p.bytes().all(|b| b.is_ascii_lowercase()))
        && parts[parts.len() - 1].bytes().all(|b| b.is_ascii_digit())
}

fn valid_access_key_id(s: &str) -> bool {
    (16..=128).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn valid_secret(s: &str) -> bool {
    (16..=128).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_graphic())
}

/// Empty means the default; otherwise 900..=43200 seconds (the AssumeRole limits).
fn parse_duration(raw: Option<&str>) -> Option<u64> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Some(DEFAULT_DURATION),
        Some(s) => s
            .parse::<u64>()
            .ok()
            .filter(|n| (MIN_DURATION..=MAX_DURATION).contains(n)),
    }
}

/// `aurora-<org id>`, restricted to the characters RoleSessionName allows (`[\w+=,.@-]`, 2..=64).
/// Derived from the organization id, never from a secret.
fn session_name(org_id: Option<&str>) -> String {
    let id: String = org_id
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(40)
        .collect();
    if id.is_empty() {
        "aurora".into()
    } else {
        format!("aurora-{id}")
    }
}

/// `form` is true for connect values (bad input is the user's to fix), false for stored metadata
/// (a broken connection asks to reconnect).
fn settings(meta: &[Kv], context: &[Kv], form: bool) -> Result<Settings, HookError> {
    let bad = |m: &str| -> HookError {
        if form {
            HookError::InvalidInput(m.into())
        } else {
            HookError::ReauthRequired(m.into())
        }
    };
    let role_arn = kv_get(meta, "role_arn")
        .map(str::trim)
        .filter(|a| parse_role_arn(a).is_some())
        .ok_or_else(|| bad("the role ARN is missing or malformed (arn:aws:iam::<account id>:role/<name>)"))?;
    if !role_arn.starts_with("arn:aws:") && !role_arn.starts_with("arn:aws-us-gov:") {
        return Err(bad(
            "only roles in the aws and aws-us-gov partitions are supported (the China and isolated partitions use other endpoints)",
        ));
    }
    let region = kv_get(meta, "region")
        .map(str::trim)
        .filter(|r| valid_region(r))
        .ok_or_else(|| bad("the region is missing or malformed, for example eu-west-1"))?;
    let duration = parse_duration(kv_get(meta, "session_duration"))
        .ok_or_else(|| bad("the session duration must be a number of seconds between 900 and 43200"))?;
    // Fail closed: never assume a role without the confused-deputy protection.
    let external_id = kv_get(context, "org_external_id")
        .filter(|e| !e.is_empty())
        .ok_or_else(|| HookError::Internal("the organization external id is not available; refusing to assume the role without it".into()))?;
    Ok(Settings {
        role_arn: role_arn.to_owned(),
        region: region.to_owned(),
        duration,
        session_name: session_name(kv_get(context, "org_id")),
        external_id: external_id.to_owned(),
    })
}

// ---- AssumeRole -----------------------------------------------------------------------------

struct Session {
    access_key_id: String,
    secret_access_key: String,
    session_token: String,
    expires_at: u64,
}

/// The form body (parameters sorted by name, as SigV4 requires of the canonical query).
fn assume_role_body(s: &Settings) -> String {
    let params = [
        ("Action", "AssumeRole".to_owned()),
        ("DurationSeconds", s.duration.to_string()),
        ("ExternalId", s.external_id.clone()),
        ("RoleArn", s.role_arn.clone()),
        ("RoleSessionName", s.session_name.clone()),
        ("Version", STS_VERSION.to_owned()),
    ];
    params
        .iter()
        .map(|(k, v)| format!("{k}={}", pct(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// `POST https://sts.<region>.amazonaws.com/`, signed with the source key.
fn assume_role_request(s: &Settings, key_id: &str, secret: &str, now: u64) -> HttpRequest {
    let host = format!("sts.{}.amazonaws.com", s.region);
    let body = assume_role_body(s);
    let (_, amz_date) = amz_dates(now);
    let signed = sigv4(
        "POST",
        "/",
        "",
        &[("content-type", FORM_TYPE), ("host", &host), ("x-amz-date", &amz_date)],
        body.as_bytes(),
        key_id,
        secret,
        &s.region,
        "sts",
        &amz_date,
    );
    HttpRequest {
        method: "POST".into(),
        url: format!("https://{host}/"),
        headers: kv([
            ("content-type", FORM_TYPE),
            ("x-amz-date", amz_date.as_str()),
            ("authorization", signed.authorization.as_str()),
            ("accept", "text/xml"),
            ("user-agent", "aurora-aws-role"),
        ]),
        body: Some(body.into_bytes()),
    }
}

/// The text of `<tag>…</tag>` inside `xml`, if present and free of markup and entities.
fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = start + xml[start..].find(&close)?;
    let v = xml[start..end].trim();
    (!v.is_empty() && !v.contains(['<', '&'])).then_some(v)
}

fn parse_session(xml: &str, now: u64) -> Result<Session, HookError> {
    let bad = |m: &str| HookError::Upstream(format!("AWS STS answered with {m}"));
    let creds_start = xml.find("<Credentials>").ok_or_else(|| bad("no credentials"))?;
    let creds = &xml[creds_start..];
    let creds = creds.split("</Credentials>").next().unwrap_or(creds);
    let field = |n: &str| tag(creds, n).ok_or_else(|| bad("an incomplete credentials block"));
    let expires = parse_expiration(field("Expiration")?).ok_or_else(|| bad("an invalid Expiration"))?;
    Ok(Session {
        access_key_id: field("AccessKeyId")?.to_owned(),
        secret_access_key: field("SecretAccessKey")?.to_owned(),
        session_token: field("SessionToken")?.to_owned(),
        // Never trust a lifetime beyond the AssumeRole maximum.
        expires_at: expires.min(now + MAX_DURATION),
    })
}

/// An STS error code that is safe to show (letters and digits only, short).
fn error_code(xml: &str) -> Option<&str> {
    tag(xml, "Code").filter(|c| c.len() <= 64 && c.bytes().all(|b| b.is_ascii_alphanumeric()))
}

fn assume_role(s: &Settings, key_id: &str, secret: &str) -> Result<Session, HookError> {
    let now = now_unix_seconds();
    let r = fetch(&assume_role_request(s, key_id, secret, now)).map_err(|e| match e {
        FetchError::Denied(m) => HookError::Upstream(format!("egress denied: {m}")),
        FetchError::Timeout => HookError::Upstream("timeout".into()),
        FetchError::TooLarge => HookError::Upstream("response too large".into()),
        FetchError::Network(m) => HookError::Upstream(m),
    })?;
    let xml = String::from_utf8_lossy(&r.body);
    if r.status == 200 {
        return parse_session(&xml, now);
    }
    // AWS error messages name principals and resources; only the code is repeated here.
    match error_code(&xml) {
        Some("AccessDenied") => Err(HookError::ReauthRequired(
            "AWS refused sts:AssumeRole (AccessDenied): check the role's trust policy, its external id condition and the source key's permission".into(),
        )),
        // The code itself is not repeated: the host redacts long token-like words in messages.
        Some("InvalidClientTokenId" | "SignatureDoesNotMatch" | "ExpiredToken" | "IncompleteSignature") => {
            Err(HookError::ReauthRequired(
                "AWS rejected the source access key (unknown key id or wrong secret): check the access key id and secret".into(),
            ))
        }
        Some(c) => Err(HookError::Upstream(format!("AWS STS answered {} ({c})", r.status))),
        None => Err(HookError::Upstream(format!("AWS STS answered {}", r.status))),
    }
}

fn session_credentials(s: &Session) -> Vec<Kv> {
    kv([
        ("access_key_id", s.access_key_id.as_str()),
        ("secret_access_key", s.secret_access_key.as_str()),
        ("session_token", s.session_token.as_str()),
    ])
}

impl Hooks for Guest {
    fn connect_exchange(i: ExchangeInput) -> Result<ExchangeOutput, HookError> {
        if i.app_id != APP {
            return Err(HookError::Unsupported);
        }
        let ExchangeSource::Form(form) = &i.source else {
            return Err(HookError::Unsupported);
        };
        let s = settings(form, &i.context, true)?;
        let key_id = kv_get(form, "source_access_key_id")
            .map(str::trim)
            .filter(|k| valid_access_key_id(k))
            .ok_or_else(|| HookError::InvalidInput("the source access key id is missing or malformed".into()))?;
        let secret = kv_get(form, "source_secret_access_key")
            .map(str::trim)
            .filter(|k| valid_secret(k))
            .ok_or_else(|| HookError::InvalidInput("the source secret access key is missing or malformed".into()))?;
        let session = assume_role(&s, key_id, secret).map_err(|e| match e {
            // On a form, "reconnect" makes no sense: the values just typed are wrong.
            HookError::ReauthRequired(m) => HookError::InvalidInput(m),
            other => other,
        })?;
        let (account, name) = parse_role_arn(&s.role_arn).unwrap_or(("", ""));
        let mut credentials = session_credentials(&session);
        credentials.extend(kv([
            ("source_access_key_id", key_id),
            ("source_secret_access_key", secret),
        ]));
        let duration = s.duration.to_string();
        Ok(ExchangeOutput::Connected(Connected {
            credentials,
            metadata: kv([
                ("role_arn", s.role_arn.as_str()),
                ("region", s.region.as_str()),
                ("session_duration", duration.as_str()),
            ]),
            scopes: vec![],
            label: format!("AWS role {name} ({account})"),
            expires_at: Some(session.expires_at),
            tenant_host: None,
        }))
    }

    fn token_mint(i: MintInput) -> Result<MintOutput, HookError> {
        if i.app_id != APP {
            return Err(HookError::Unsupported);
        }
        let s = settings(&i.metadata, &i.context, false)?;
        let key_id = kv_get(&i.credentials, "source_access_key_id")
            .filter(|k| valid_access_key_id(k))
            .ok_or_else(|| HookError::ReauthRequired("the source access key is missing: reconnect the app".into()))?;
        let secret = kv_get(&i.credentials, "source_secret_access_key")
            .filter(|k| valid_secret(k))
            .ok_or_else(|| HookError::ReauthRequired("the source secret access key is missing: reconnect the app".into()))?;
        let session = assume_role(&s, key_id, secret)?;
        Ok(MintOutput {
            credentials: session_credentials(&session),
            expires_at: session.expires_at,
        })
    }
}

export_hooks!(Guest);

/// Host-target unit tests (`cargo test` in this directory). The recorded-HTTP fixtures under
/// `tests/` cover the hook points end to end; their cassette matches method and URL only, so the
/// signed request (headers, ExternalId, RoleSessionName) is asserted here.
#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";

    fn settings_fixture() -> Settings {
        Settings {
            role_arn: "arn:aws:iam::123456789012:role/team/aurora-agent".into(),
            region: "eu-west-1".into(),
            duration: 3600,
            session_name: "aurora-org-1".into(),
            external_id: "0123456789abcdef0123456789abcdef".into(),
        }
    }

    /// AWS Signature Version 4 test suite, `get-vanilla`:
    /// https://docs.aws.amazon.com/general/latest/gr/signature-v4-test-suite.html
    #[test]
    fn sigv4_matches_the_published_get_vanilla_vector() {
        let s = sigv4(
            "GET",
            "/",
            "",
            &[("host", "example.amazonaws.com"), ("x-amz-date", "20150830T123600Z")],
            b"",
            "AKIDEXAMPLE",
            SECRET,
            "us-east-1",
            "service",
            "20150830T123600Z",
        );
        assert_eq!(
            s.canonical_request,
            "GET\n/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\nhost;x-amz-date\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            s.string_to_sign,
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\nbb579772317eb040ac9ed261061d46c1f17a8133879d6129b6e1c25292927e63"
        );
        assert_eq!(s.signature, "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31");
        assert_eq!(
            s.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    /// The IAM `ListUsers` worked example of "Create a signed AWS API request":
    /// https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv4-create-signed-request.html
    #[test]
    fn sigv4_matches_the_documented_iam_list_users_example() {
        let s = sigv4(
            "GET",
            "/",
            "Action=ListUsers&Version=2010-05-08",
            &[
                ("content-type", "application/x-www-form-urlencoded; charset=utf-8"),
                ("host", "iam.amazonaws.com"),
                ("x-amz-date", "20150830T123600Z"),
            ],
            b"",
            "AKIDEXAMPLE",
            SECRET,
            "us-east-1",
            "iam",
            "20150830T123600Z",
        );
        assert_eq!(
            s.string_to_sign,
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/iam/aws4_request\nf536975d06c0309214f805bb90ccff089219ecd68b2577efef23edd43b7e1a59"
        );
        assert_eq!(s.signature, "5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7");
    }

    #[test]
    fn hmac_matches_rfc_4231_case_2() {
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // A key longer than the block size is hashed first (RFC 4231 case 6).
        assert_eq!(
            hex(&hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    fn header<'a>(r: &'a HttpRequest, n: &str) -> Option<&'a str> {
        kv_get(&r.headers, n)
    }

    #[test]
    fn assume_role_request_is_regional_signed_and_carries_external_id_and_session_name() {
        let s = settings_fixture();
        let now = 1_446_302_807; // 2015-10-31T14:46:47Z
        let r = assume_role_request(&s, "AKIDFIXTURE00001", SECRET, now);
        assert_eq!(r.method, "POST");
        assert_eq!(r.url, "https://sts.eu-west-1.amazonaws.com/");
        let body = String::from_utf8(r.body.clone().unwrap()).unwrap();
        assert_eq!(
            body,
            "Action=AssumeRole&DurationSeconds=3600&ExternalId=0123456789abcdef0123456789abcdef\
             &RoleArn=arn%3Aaws%3Aiam%3A%3A123456789012%3Arole%2Fteam%2Faurora-agent\
             &RoleSessionName=aurora-org-1&Version=2011-06-15"
        );
        assert_eq!(header(&r, "x-amz-date"), Some("20151031T144647Z"));
        assert_eq!(header(&r, "content-type"), Some(FORM_TYPE));
        let auth = header(&r, "authorization").unwrap();
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDFIXTURE00001/20151031/eu-west-1/sts/aws4_request, SignedHeaders=content-type;host;x-amz-date, Signature="), "{auth}");
        // The signature is the one an independent signing of the same canonical request gives.
        let expect = sigv4(
            "POST",
            "/",
            "",
            &[("content-type", FORM_TYPE), ("host", "sts.eu-west-1.amazonaws.com"), ("x-amz-date", "20151031T144647Z")],
            body.as_bytes(),
            "AKIDFIXTURE00001",
            SECRET,
            "eu-west-1",
            "sts",
            "20151031T144647Z",
        );
        assert!(auth.ends_with(&expect.signature));
        assert!(expect.canonical_request.contains("host:sts.eu-west-1.amazonaws.com\n"));
        // The secret never travels, and the source key is only the credential scope's id.
        assert!(!auth.contains(SECRET));
        assert!(r.headers.iter().all(|h| !h.value.contains(SECRET)));
        assert!(!body.contains(SECRET));
    }

    #[test]
    fn external_id_is_required_and_taken_from_the_context() {
        let md = kv([("role_arn", "arn:aws:iam::123456789012:role/r"), ("region", "eu-west-1")]);
        assert!(matches!(settings(&md, &[], false), Err(HookError::Internal(_))));
        assert!(matches!(settings(&md, &kv([("org_external_id", "")]), false), Err(HookError::Internal(_))));
        let s = settings(&md, &kv([("org_external_id", "ext-1"), ("org_id", "0b1c2d3e-aaaa-bbbb-cccc-1234567890ab")]), false).unwrap();
        assert_eq!(s.external_id, "ext-1");
        assert_eq!(s.session_name, "aurora-0b1c2d3e-aaaa-bbbb-cccc-1234567890ab");
        assert_eq!(s.duration, 3600);
    }

    #[test]
    fn role_arns_are_validated() {
        assert_eq!(
            parse_role_arn("arn:aws:iam::123456789012:role/team/aurora-agent"),
            Some(("123456789012", "aurora-agent"))
        );
        assert!(parse_role_arn("arn:aws-us-gov:iam::123456789012:role/r").is_some());
        for bad in [
            "",
            "arn:aws:iam::12345678901:role/r",
            "arn:aws:iam::1234567890123:role/r",
            "arn:aws:iam::123456789012:user/r",
            "arn:aws:iam::123456789012:role/",
            "arn:aws:iam::123456789012:role//r",
            "arn:aws:iam::123456789012:role/r/",
            "arn:aws:iam::123456789012:role/r r",
            "arn:aws:s3::123456789012:role/r",
            "arn:AWS:iam::123456789012:role/r",
            "arn:azure:iam::123456789012:role/r",
        ] {
            assert!(parse_role_arn(bad).is_none(), "{bad}");
        }
        let ctx = kv([("org_external_id", "ext-1")]);
        let cn = kv([("role_arn", "arn:aws-cn:iam::123456789012:role/r"), ("region", "cn-north-1")]);
        assert!(matches!(settings(&cn, &ctx, true), Err(HookError::InvalidInput(m)) if m.contains("partition")));
    }

    #[test]
    fn regions_and_durations_are_validated() {
        for ok in ["eu-west-1", "us-east-1", "ap-southeast-2", "us-gov-west-1", "il-central-1"] {
            assert!(valid_region(ok), "{ok}");
        }
        for bad in ["", "eu", "eu-west", "EU-west-1", "eu-west-1.evil.com", "eu-west-1/x", "evil.com", "eu--1", "e-west-1"] {
            assert!(!valid_region(bad), "{bad}");
        }
        assert_eq!(parse_duration(None), Some(3600));
        assert_eq!(parse_duration(Some("  ")), Some(3600));
        assert_eq!(parse_duration(Some("900")), Some(900));
        assert_eq!(parse_duration(Some("43200")), Some(43_200));
        for bad in ["899", "43201", "abc", "-1", "1h"] {
            assert_eq!(parse_duration(Some(bad)), None, "{bad}");
        }
    }

    #[test]
    fn session_names_use_only_allowed_characters_and_never_a_secret() {
        assert_eq!(session_name(None), "aurora");
        assert_eq!(session_name(Some("")), "aurora");
        assert_eq!(session_name(Some("a b/c;d")), "aurora-abcd");
        assert!(session_name(Some(&"x".repeat(200))).len() <= 64);
    }

    const OK_XML: &str = "<AssumeRoleResponse xmlns=\"https://sts.amazonaws.com/doc/2011-06-15/\"><AssumeRoleResult><AssumedRoleUser><AssumedRoleId>AROAFIXTURE:aurora</AssumedRoleId><Arn>arn:aws:sts::123456789012:assumed-role/r/aurora</Arn></AssumedRoleUser><Credentials><AccessKeyId>ASIDFIXTURE00001</AccessKeyId><SecretAccessKey>fixture-session-secret-1</SecretAccessKey><SessionToken>fixture-session-token-1</SessionToken><Expiration>2099-01-01T00:00:00Z</Expiration></Credentials></AssumeRoleResult></AssumeRoleResponse>";

    #[test]
    fn the_documented_response_parses_and_the_expiry_is_capped() {
        let s = parse_session(OK_XML, 4_000_000_000).unwrap();
        assert_eq!(s.access_key_id, "ASIDFIXTURE00001");
        assert_eq!(s.session_token, "fixture-session-token-1");
        assert_eq!(s.expires_at, 4_000_000_000 + MAX_DURATION);
        let near = parse_session(OK_XML, 4_070_908_800 - 600).unwrap();
        assert_eq!(near.expires_at, 4_070_908_800);
        for bad in [
            "",
            "<Credentials></Credentials>",
            &OK_XML.replace("SessionToken", "Other"),
            &OK_XML.replace("2099-01-01T00:00:00Z", "tomorrow"),
            &OK_XML.replace("fixture-session-token-1", "a&lt;b"),
        ] {
            assert!(matches!(parse_session(bad, 0), Err(HookError::Upstream(_))), "{bad}");
        }
    }

    #[test]
    fn expiration_timestamps_parse() {
        assert_eq!(parse_expiration("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_expiration("2019-11-01T20:26:47Z"), Some(1_572_640_007));
        assert_eq!(parse_expiration("2019-11-01T20:26:47.123Z"), Some(1_572_640_007));
        assert_eq!(parse_expiration("2019-11-01 20:26:47"), None);
        assert_eq!(parse_expiration("2019-13-01T20:26:47Z"), None);
    }

    #[test]
    fn error_codes_are_extracted_but_hostile_ones_are_dropped() {
        assert_eq!(
            error_code("<ErrorResponse><Error><Type>Sender</Type><Code>AccessDenied</Code><Message>User: arn:aws:iam::1:user/x is not authorized</Message></Error></ErrorResponse>"),
            Some("AccessDenied")
        );
        assert_eq!(error_code("<Code>Bad Code!</Code>"), None);
        assert_eq!(error_code("not xml"), None);
    }

    #[test]
    fn credential_names_do_not_include_the_source_key() {
        let out = session_credentials(&parse_session(OK_XML, 0).unwrap());
        let names: Vec<&str> = out.iter().map(|k| k.key.as_str()).collect();
        assert_eq!(names, ["access_key_id", "secret_access_key", "session_token"]);
    }

    #[test]
    fn dates_format_as_sigv4_expects() {
        assert_eq!(amz_dates(0), ("19700101".into(), "19700101T000000Z".into()));
        assert_eq!(amz_dates(1_440_938_160), ("20150830".into(), "20150830T123600Z".into()));
        assert_eq!(amz_dates(1_709_208_000).1, "20240229T120000Z");
    }
}
