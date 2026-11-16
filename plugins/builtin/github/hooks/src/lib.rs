//! `github_app` connect-method hooks of the `github` package (docs/09 §8, D-91, FIX-123/D-216: the
//! former `github-app` package is now this method): a GitHub App authenticates as itself with a short-lived
//! RS256 JWT signed with the app's private key, and exchanges it for an installation access token
//! (https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/
//! generating-an-installation-access-token-for-a-github-app).
//!
//! * `connect.exchange` validates the connect form and mints once, so a wrong key, app id or
//!   installation id is reported when connecting, not on the first agent request.
//! * `token.mint` signs a new JWT and mints a fresh installation token whenever the stored one
//!   expires.
//!
//! Two apps share this guest: `github` talks to `https://api.github.com`; `github-ghes`
//! talks to `https://<ghes_host>/api/v3`, where `ghes_host` is a connect-form value that the hook
//! host's egress policy pins to the connection's registered instance (a different host is denied).
//! The SaaS app never reads `ghes_host`. The private key is never logged and never appears in an
//! error message.
use aurora_plugin_sdk::*;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::signature::{SignatureEncoding, Signer};
use rsa::PrivateKey;
use sha2::Sha256;

const APP_SAAS: &str = "github";
const APP_GHES: &str = "github-ghes";
const SAAS_BASE: &str = "https://api.github.com";
/// GitHub: `exp` is at most 10 minutes after the issue time; `iat` is set 60 s in the past to
/// absorb clock drift, so `exp = now + 540` keeps the whole span within 600 s.
const IAT_SKEW: u64 = 60;
const JWT_TTL: u64 = 540;
/// Installation tokens last one hour; never trust a longer expiry.
const MAX_TOKEN_SECS: u64 = 3600;

struct Guest;

/// Where to mint and for whom. Built from connect-form values or from the stored metadata.
struct Settings {
    app_id: String,
    installation_id: String,
    base: String,
    ghes: bool,
    /// (FIX-133, D-240) The connection is read-only: the minted token is narrowed to `read`
    /// permissions, so GitHub itself refuses writes.
    read_only: bool,
    /// Repository names (of the installation's account) the token is limited to; empty = every
    /// repository the installation has.
    repositories: Vec<String>,
}

/// The installation permissions a read-only token may keep, each as `read`: what the app's read
/// tools need (repository contents, issues, pull requests, Actions runs, checks, statuses,
/// deployments, discussions, packages and the always-granted metadata). Anything else the
/// installation holds (administration, secrets, workflows, hooks, security events, organisation
/// permissions) is dropped from a read-only token even as `read`.
const READ_ALLOWLIST: &[&str] = &[
    "actions",
    "checks",
    "contents",
    "deployments",
    "discussions",
    "issues",
    "metadata",
    "packages",
    "pull_requests",
    "statuses",
];
/// GitHub accepts at most 500 repository names per token.
const MAX_REPOS: usize = 500;

fn base64url(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        if c.len() > 1 {
            out.push(T[(n >> 6) as usize & 63] as char);
        }
        if c.len() > 2 {
            out.push(T[n as usize & 63] as char);
        }
    }
    out
}

fn valid_app_id(s: &str) -> bool {
    // A numeric app id or a client id such as `Iv1.0123456789abcdef`.
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

/// `host[:port]`: no scheme, path, query, userinfo or whitespace; a DNS name, never an IP literal.
fn normalise_host(raw: &str) -> Option<String> {
    let h = raw.trim().to_ascii_lowercase();
    let (name, port) = match h.rsplit_once(':') {
        Some((n, p)) => (n, Some(p)),
        None => (h.as_str(), None),
    };
    if let Some(p) = port
        && (p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit()) || p == "0")
    {
        return None;
    }
    let dns = !name.is_empty()
        && name.len() <= 253
        && name.contains('.')
        && !name.starts_with(['.', '-'])
        && !name.ends_with(['.', '-'])
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && !name.bytes().all(|b| b.is_ascii_digit() || b == b'.');
    dns.then_some(h)
}

/// `form` is true for connect values (bad input is the user's to fix), false for stored metadata
/// (a broken connection asks to reconnect).
fn settings(app: &str, src: &[Kv], form: bool) -> Result<Settings, HookError> {
    let bad = |m: &str| -> HookError {
        if form {
            HookError::InvalidInput(m.into())
        } else {
            HookError::ReauthRequired(m.into())
        }
    };
    let app_id = kv_get(src, "app_id")
        .map(str::trim)
        .filter(|s| valid_app_id(s))
        .ok_or_else(|| bad("the GitHub App id is missing or malformed"))?;
    let installation_id = kv_get(src, "installation_id")
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| bad("the installation id is missing or not a number"))?;
    let (base, ghes) = match app {
        APP_SAAS => (SAAS_BASE.to_owned(), false),
        APP_GHES => {
            let host = kv_get(src, "ghes_host")
                .and_then(normalise_host)
                .ok_or_else(|| {
                    bad("ghes_host must be the GitHub Enterprise Server host name, for example github.example.com (no https://, path or credentials)")
                })?;
            (format!("https://{host}/api/v3"), true)
        }
        _ => return Err(HookError::Unsupported),
    };
    // The reserved `aurora_access` key is set by Aurora (the api on the connect form, the gateway
    // on a mint), never by the user. Anything but an explicit `write` narrows: fail closed.
    let read_only = kv_get(src, "aurora_access").is_some_and(|a| a != "write");
    let repositories = repositories(kv_get(src, "repositories").unwrap_or_default()).ok_or_else(|| {
        bad("repositories must be repository names (not owner/name) separated by commas, at most 500")
    })?;
    Ok(Settings {
        app_id: app_id.to_owned(),
        installation_id: installation_id.to_owned(),
        base,
        ghes,
        read_only,
        repositories,
    })
}

/// Comma, space or newline separated repository names; `None` when one is malformed.
fn repositories(raw: &str) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for n in raw.split([',', ' ', '\n', '\r', '\t']).map(str::trim).filter(|n| !n.is_empty()) {
        let ok = n.len() <= 100
            && n != "."
            && n != ".."
            && n.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if !ok {
            return None;
        }
        if !out.iter().any(|o| o == n) {
            out.push(n.to_owned());
        }
    }
    (out.len() <= MAX_REPOS).then_some(out)
}

/// The `POST …/access_tokens` body: for a read-only connection every installation permission
/// that is on the read allowlist, as `read`; the repository subset when one is configured;
/// `None` (an empty body: all of the installation's permissions and repositories) otherwise.
/// A read-only connection whose installation has no permission on the allowlist cannot be
/// narrowed and is refused rather than minting something wider.
fn mint_body(s: &Settings, installation: Option<&serde_json::Value>) -> Result<Option<serde_json::Value>, HookError> {
    let mut body = serde_json::Map::new();
    if s.read_only {
        let held = installation
            .and_then(|i| i.get("permissions"))
            .and_then(|p| p.as_object())
            .ok_or_else(|| HookError::Upstream("GitHub answered without the installation permissions".into()))?;
        let narrowed: serde_json::Map<String, serde_json::Value> = READ_ALLOWLIST
            .iter()
            .filter(|k| held.contains_key(**k))
            .map(|k| ((*k).to_owned(), serde_json::Value::String("read".into())))
            .collect();
        if narrowed.is_empty() {
            return Err(HookError::Upstream(
                "the installation holds none of the permissions Aurora needs for a view-only token: give the app Contents, Issues or Pull requests access on GitHub".into(),
            ));
        }
        body.insert("permissions".into(), narrowed.into());
    }
    if !s.repositories.is_empty() {
        body.insert("repositories".into(), s.repositories.clone().into());
    }
    Ok((!body.is_empty()).then_some(body.into()))
}

/// Accept PKCS#1 (`RSA PRIVATE KEY`, what GitHub downloads) and PKCS#8 PEM, tolerating the
/// whitespace damage of a pasted key: the body is re-wrapped before parsing. Errors never
/// include any part of the input.
fn load_key(pem: &str) -> Result<PrivateKey, HookError> {
    let bad = || HookError::InvalidInput("the private key is not a valid RSA private key in PEM format".into());
    let t = pem.trim();
    let rest = t.strip_prefix("-----BEGIN ").ok_or_else(bad)?;
    let (label, rest) = rest.split_once("-----").ok_or_else(bad)?;
    let end = format!("-----END {label}-----");
    let (body, _) = rest.split_once(end.as_str()).ok_or_else(bad)?;
    let body: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    if body.is_empty() || !body.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=')) {
        return Err(bad());
    }
    let mut wrapped = format!("-----BEGIN {label}-----\n");
    for chunk in body.as_bytes().chunks(64) {
        wrapped.push_str(std::str::from_utf8(chunk).map_err(|_| bad())?);
        wrapped.push('\n');
    }
    wrapped.push_str(&end);
    wrapped.push('\n');
    let key = match label {
        "RSA PRIVATE KEY" => PrivateKey::from_pkcs1_pem(&wrapped).map_err(|_| ()),
        "PRIVATE KEY" => PrivateKey::from_pkcs8_pem(&wrapped).map_err(|_| ()),
        _ => return Err(bad()),
    };
    key.map_err(|()| bad())
}

/// The app JWT (RS256): `iat = now - 60`, `exp = now + 540` (never more than 10 minutes),
/// `iss = <app id>`.
fn app_jwt(key: &PrivateKey, app_id: &str, now: u64) -> Result<String, HookError> {
    let header = base64url(br#"{"alg":"RS256","typ":"JWT"}"#);
    let claims = serde_json::json!({
        "iat": now.saturating_sub(IAT_SKEW),
        "exp": now + JWT_TTL,
        "iss": app_id,
    });
    let signing_input = format!("{header}.{}", base64url(claims.to_string().as_bytes()));
    let sig = SigningKey::<Sha256>::new(key.clone())
        .try_sign(signing_input.as_bytes())
        .map_err(|_| HookError::Internal("could not sign the app JWT".into()))?;
    Ok(format!("{signing_input}.{}", base64url(&sig.to_bytes())))
}

/// `2026-11-08T12:00:00Z` to unix seconds.
fn parse_rfc3339_utc(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z' {
        return None;
    }
    let num = |a: usize, z: usize| -> Option<i64> { s.get(a..z)?.parse().ok() };
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // days from civil (Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok()
}

fn denied_message(s: &Settings, m: &str) -> String {
    if s.ghes {
        "egress denied: ghes_host does not match the registered instance of this connection".into()
    } else {
        format!("egress denied: {m}")
    }
}

/// `POST {base}/app/installations/{installation_id}/access_tokens`, authenticated as the app.
fn mint_request(s: &Settings, jwt: &str, body: Option<&serde_json::Value>) -> HttpRequest {
    let mut headers = kv([
        ("authorization", format!("Bearer {jwt}").as_str()),
        ("accept", "application/vnd.github+json"),
        ("x-github-api-version", "2022-11-28"),
        ("user-agent", "aurora-github-app"),
    ]);
    if body.is_some() {
        headers.extend(kv([("content-type", "application/json")]));
    }
    HttpRequest {
        method: "POST".into(),
        url: format!("{}/app/installations/{}/access_tokens", s.base, s.installation_id),
        headers,
        body: body.map(|b| b.to_string().into_bytes()),
    }
}

/// `GET {base}/app/installations/{installation_id}`, authenticated as the app: what the
/// installation was granted (`permissions`), which bounds what a narrowed token may ask for.
fn installation_request(s: &Settings, jwt: &str) -> HttpRequest {
    HttpRequest {
        method: "GET".into(),
        url: format!("{}/app/installations/{}", s.base, s.installation_id),
        headers: kv([
            ("authorization", format!("Bearer {jwt}").as_str()),
            ("accept", "application/vnd.github+json"),
            ("x-github-api-version", "2022-11-28"),
            ("user-agent", "aurora-github-app"),
        ]),
        body: None,
    }
}

fn send(s: &Settings, req: &HttpRequest) -> Result<HttpResponse, HookError> {
    let r = fetch(req).map_err(|e| match e {
        FetchError::Denied(m) => HookError::Upstream(denied_message(s, &m)),
        FetchError::Timeout => HookError::Upstream("timeout".into()),
        FetchError::TooLarge => HookError::Upstream("response too large".into()),
        FetchError::Network(m) => HookError::Upstream(m),
    })?;
    check_status(r.status)?;
    Ok(r)
}

fn mint(s: &Settings, key: &PrivateKey) -> Result<(String, u64), HookError> {
    let now = now_unix_seconds();
    let jwt = app_jwt(key, &s.app_id, now)?;
    // A read-only connection first learns what the installation holds, so the narrowed
    // `permissions` never ask for more than that (GitHub answers 422 to a permission the
    // installation lacks).
    let installation = if s.read_only {
        let r = send(s, &installation_request(s, &jwt))?;
        Some(
            serde_json::from_slice::<serde_json::Value>(&r.body)
                .map_err(|_| HookError::Upstream("GitHub answered with a malformed body".into()))?,
        )
    } else {
        None
    };
    let body = mint_body(s, installation.as_ref())?;
    let r = send(s, &mint_request(s, &jwt, body.as_ref()))?;
    parse_token(&r, now)
}

fn check_status(status: u16) -> Result<(), HookError> {
    match status {
        200 | 201 => {}
        401 => {
            return Err(HookError::ReauthRequired(
                "GitHub rejected the app credentials: check the app id and the private key".into(),
            ));
        }
        404 => {
            return Err(HookError::ReauthRequired(
                "installation not found: check the installation id and that the app is still installed".into(),
            ));
        }
        403 => {
            return Err(HookError::Upstream(
                "GitHub refused to mint a token (the installation may be suspended)".into(),
            ));
        }
        422 => {
            return Err(HookError::Upstream(
                "GitHub refused the requested repositories or permissions: check that every listed repository is part of the installation".into(),
            ));
        }
        st => return Err(HookError::Upstream(format!("GitHub answered {st}"))),
    }
    Ok(())
}

fn parse_token(r: &HttpResponse, now: u64) -> Result<(String, u64), HookError> {
    let body: serde_json::Value = serde_json::from_slice(&r.body)
        .map_err(|_| HookError::Upstream("GitHub answered with a malformed body".into()))?;
    let token = body
        .get("token")
        .and_then(|v| v.as_str())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| HookError::Upstream("GitHub answered without a token".into()))?;
    let expires = body
        .get("expires_at")
        .and_then(|v| v.as_str())
        .and_then(parse_rfc3339_utc)
        .ok_or_else(|| HookError::Upstream("GitHub answered without a valid expires_at".into()))?;
    Ok((token.to_owned(), expires.min(now + MAX_TOKEN_SECS)))
}

impl Hooks for Guest {
    fn connect_exchange(i: ExchangeInput) -> Result<ExchangeOutput, HookError> {
        let ExchangeSource::Form(form) = &i.source else {
            return Err(HookError::Unsupported);
        };
        let s = settings(&i.app_id, form, true)?;
        let pem = kv_get(form, "private_key")
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(|| HookError::InvalidInput("the private key is required".into()))?;
        let key = load_key(pem)?;
        let (token, expires_at) = mint(&s, &key).map_err(|e| match e {
            // On a form, "reconnect" makes no sense: the values just typed are wrong.
            HookError::ReauthRequired(m) => HookError::InvalidInput(m),
            other => other,
        })?;
        let mut metadata = vec![("app_id", s.app_id.as_str()), ("installation_id", s.installation_id.as_str())];
        let repos = s.repositories.join(",");
        if !repos.is_empty() {
            metadata.push(("repositories", repos.as_str()));
        }
        let host;
        if s.ghes {
            host = kv_get(form, "ghes_host").and_then(normalise_host).unwrap_or_default();
            metadata.push(("ghes_host", host.as_str()));
        }
        let label = format!("GitHub App {} (installation {})", s.app_id, s.installation_id);
        Ok(ExchangeOutput::Connected(Connected {
            credentials: kv([("private_key", pem.trim()), ("access_token", token.as_str())]),
            metadata: kv(metadata),
            scopes: vec![],
            label,
            expires_at: Some(expires_at),
            tenant_host: None,
        }))
    }

    fn token_mint(i: MintInput) -> Result<MintOutput, HookError> {
        let s = settings(&i.app_id, &i.metadata, false)?;
        let pem = kv_get(&i.credentials, "private_key")
            .ok_or_else(|| HookError::ReauthRequired("the private key is missing: reconnect the app".into()))?;
        let key = load_key(pem).map_err(|_| {
            HookError::ReauthRequired("the stored private key is unusable: reconnect the app".into())
        })?;
        let (token, expires_at) = mint(&s, &key)?;
        Ok(MintOutput {
            credentials: kv([("access_token", token.as_str())]),
            expires_at,
        })
    }
}

export_hooks!(Guest);

/// Host-target unit tests (`cargo test` in this directory): the pure parts of the guest. The
/// recorded-HTTP fixtures under `tests/` cover the hook points end to end; the cassette matches
/// method and URL only, so the request headers and the JWT are asserted here.
#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs1::{EncodeRsaPrivateKey, LineEnding};
    use rsa::pkcs1v15::{Signature, VerifyingKey};
    use rsa::pkcs8::EncodePrivateKey;
    use rsa::signature::Verifier;

    fn b64d(s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let (mut acc, mut bits) = (0u32, 0u32);
        for b in s.bytes() {
            let v = match b {
                b'A'..=b'Z' => b - b'A',
                b'a'..=b'z' => b - b'a' + 26,
                b'0'..=b'9' => b - b'0' + 52,
                b'-' => 62,
                b'_' => 63,
                _ => panic!("not base64url"),
            };
            acc = (acc << 6) | u32::from(v);
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
        out
    }

    fn key() -> PrivateKey {
        PrivateKey::new(&mut getrandom::SysRng, 1024).expect("key")
    }

    fn saas() -> Settings {
        Settings {
            app_id: "123456".into(),
            installation_id: "7".into(),
            base: SAAS_BASE.into(),
            ghes: false,
            read_only: false,
            repositories: vec![],
        }
    }

    fn installation() -> serde_json::Value {
        serde_json::json!({"id": 7, "permissions": {
            "contents": "write", "issues": "write", "pull_requests": "read", "metadata": "read",
            "administration": "write", "secrets": "read", "workflows": "write", "members": "read"
        }})
    }

    #[test]
    fn jwt_is_rs256_with_the_documented_claims_and_verifies() {
        let k = key();
        let now = 1_800_000_000;
        let jwt = app_jwt(&k, "123456", now).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: serde_json::Value = serde_json::from_slice(&b64d(parts[0])).unwrap();
        assert_eq!(header, serde_json::json!({"alg": "RS256", "typ": "JWT"}));
        let claims: serde_json::Value = serde_json::from_slice(&b64d(parts[1])).unwrap();
        assert_eq!(claims["iss"], "123456");
        assert_eq!(claims["iat"], now - 60);
        assert!(claims["exp"].as_u64().unwrap() <= now + 600);
        assert!(claims["exp"].as_u64().unwrap() > now);
        let sig = Signature::try_from(b64d(parts[2]).as_slice()).unwrap();
        VerifyingKey::<Sha256>::new(k.to_public_key())
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
            .expect("signature verifies with the public key");
    }

    #[test]
    fn mint_request_targets_the_documented_path_with_a_bearer_jwt() {
        let r = mint_request(&saas(), "aaa.bbb.ccc", None);
        assert_eq!(r.method, "POST");
        assert!(r.body.is_none(), "read-and-write sends no body");
        assert_eq!(r.url, "https://api.github.com/app/installations/7/access_tokens");
        assert_eq!(kv_get(&r.headers, "authorization"), Some("Bearer aaa.bbb.ccc"));
        let ghes = Settings {
            base: "https://github.example.com/api/v3".into(),
            ghes: true,
            ..saas()
        };
        assert_eq!(
            mint_request(&ghes, "x.y.z", None).url,
            "https://github.example.com/api/v3/app/installations/7/access_tokens"
        );
    }

    #[test]
    fn read_and_write_mints_with_no_narrowing() {
        let s = saas();
        assert_eq!(mint_body(&s, None).unwrap(), None);
        // Even when the installation answer is at hand, nothing is narrowed.
        assert_eq!(mint_body(&s, Some(&installation())).unwrap(), None);
    }

    #[test]
    fn read_only_narrows_to_the_read_allowlist_intersected_with_the_installation() {
        let s = Settings { read_only: true, ..saas() };
        let body = mint_body(&s, Some(&installation())).unwrap().unwrap();
        // Held by the installation and on the allowlist, all as `read`; administration, secrets,
        // workflows and organisation permissions never appear, nor does anything it lacks.
        assert_eq!(
            body,
            serde_json::json!({"permissions": {
                "contents": "read", "issues": "read", "pull_requests": "read", "metadata": "read"
            }})
        );
        let none = serde_json::json!({"permissions": {"administration": "write", "secrets": "write"}});
        assert!(matches!(mint_body(&s, Some(&none)), Err(HookError::Upstream(_))));
        assert!(matches!(mint_body(&s, None), Err(HookError::Upstream(_))));
        assert!(matches!(
            mint_body(&s, Some(&serde_json::json!({"id": 7}))),
            Err(HookError::Upstream(_))
        ));
    }

    #[test]
    fn a_repository_subset_is_sent_alone_or_with_the_read_permissions() {
        let rw = Settings { repositories: vec!["api".into(), "web".into()], ..saas() };
        assert_eq!(mint_body(&rw, None).unwrap(), Some(serde_json::json!({"repositories": ["api", "web"]})));
        let ro = Settings { read_only: true, ..rw };
        let body = mint_body(&ro, Some(&installation())).unwrap().unwrap();
        assert_eq!(body["repositories"], serde_json::json!(["api", "web"]));
        assert_eq!(body["permissions"]["contents"], "read");
        let r = mint_request(&ro, "a.b.c", Some(&body));
        assert_eq!(kv_get(&r.headers, "content-type"), Some("application/json"));
        let sent: serde_json::Value = serde_json::from_slice(&r.body.unwrap()).unwrap();
        assert_eq!(sent, body);
    }

    #[test]
    fn the_installation_lookup_targets_the_documented_path() {
        assert_eq!(installation_request(&saas(), "a.b.c").url, "https://api.github.com/app/installations/7");
        assert_eq!(installation_request(&saas(), "a.b.c").method, "GET");
    }

    #[test]
    fn access_comes_only_from_the_reserved_key_and_fails_closed() {
        let base = [("app_id", "1"), ("installation_id", "7")];
        let with = |a: &str| {
            let mut v = base.to_vec();
            v.push(("aurora_access", a));
            settings(APP_SAAS, &kv(v), false).unwrap().read_only
        };
        assert!(with("read"));
        assert!(!with("write"));
        assert!(with("anything-else"));
        assert!(!settings(APP_SAAS, &kv(base), false).unwrap().read_only);
    }

    #[test]
    fn repositories_parse_names_only() {
        assert_eq!(repositories(""), Some(vec![]));
        assert_eq!(repositories(" api, web\nweb docs.v2 "), Some(vec!["api".into(), "web".into(), "docs.v2".into()]));
        for bad in ["owner/api", "..", "a b/c", "x;y", &"a".repeat(101)] {
            assert_eq!(repositories(bad), None, "{bad}");
        }
        let many: Vec<String> = (0..501).map(|i| format!("r{i}")).collect();
        assert_eq!(repositories(&many.join(",")), None);
    }

    #[test]
    fn keys_load_from_pkcs1_pkcs8_and_pasted_pem() {
        let k = key();
        let p1 = k.to_pkcs1_pem(LineEnding::LF).unwrap().to_string();
        let p8 = k.to_pkcs8_pem(LineEnding::LF).unwrap().to_string();
        assert!(load_key(&p1).is_ok());
        assert!(load_key(&p8).is_ok());
        // Newlines turned into spaces by a form field, CRLF endings, surrounding whitespace.
        assert!(load_key(&format!("  {}\n", p1.replace('\n', " "))).is_ok());
        assert!(load_key(&p1.replace('\n', "\r\n")).is_ok());
    }

    #[test]
    fn bad_keys_are_rejected_without_echoing_the_input() {
        // Labels are assembled so that no PEM header appears literally in the source.
        let pem = |label: &str, body: &str| format!("-----BEGIN {label}-----\n{body}\n-----END {label}-----");
        let rsa_label = ["RSA PRIVATE", "KEY"].join(" ");
        for bad in [
            String::new(),
            "not a key SECRETMARKER".to_owned(),
            format!("SECRETMARKER{}", pem(&rsa_label, "AAAA")),
            pem(&rsa_label, "SECRETMARKER!!!"),
            pem(&rsa_label, "QUJDREVGR0hJSktMTU5PUA=="),
            pem("PUBLIC KEY", "QUJDREVGR0hJSktMTU5PUA=="),
        ] {
            match load_key(&bad) {
                Err(HookError::InvalidInput(m)) => assert!(!m.contains("SECRETMARKER"), "{m}"),
                Err(_) => panic!("wrong error kind"),
                Ok(_) => panic!("accepted a bad key"),
            }
        }
    }

    #[test]
    fn ghes_host_is_a_bare_dns_name_with_an_optional_port() {
        assert_eq!(normalise_host("  GitHub.Example.com "), Some("github.example.com".into()));
        assert_eq!(normalise_host("github.example.com:8443"), Some("github.example.com:8443".into()));
        for bad in [
            "",
            "github",
            "https://github.example.com",
            "github.example.com/api/v3",
            "user@github.example.com",
            "github.example.com?x=1",
            "github.example.com#x",
            "github.example.com:",
            "github.example.com:0",
            "github.example.com:99999x",
            "10.0.0.1",
            "[::1]",
            "a b.example.com",
            ".example.com",
            "example..com",
            "-a.example.com",
            "github.example.com\\x",
        ] {
            assert_eq!(normalise_host(bad), None, "{bad}");
        }
    }

    #[test]
    fn settings_pin_each_app_to_its_own_host() {
        let md = kv([("app_id", "123456"), ("installation_id", "7"), ("ghes_host", "github.example.com")]);
        // The SaaS app never uses ghes_host.
        let s = settings(APP_SAAS, &md, false).unwrap();
        assert_eq!(s.base, "https://api.github.com");
        // The GHES app never uses api.github.com.
        let g = settings(APP_GHES, &md, false).unwrap();
        assert_eq!(g.base, "https://github.example.com/api/v3");
        assert!(matches!(
            settings(APP_GHES, &kv([("app_id", "1"), ("installation_id", "7")]), false),
            Err(HookError::ReauthRequired(_))
        ));
        assert!(matches!(
            settings(APP_GHES, &kv([("app_id", "1"), ("installation_id", "7"), ("ghes_host", "https://x.example.com")]), true),
            Err(HookError::InvalidInput(_))
        ));
        assert!(matches!(settings("other", &md, false), Err(HookError::Unsupported)));
        assert!(matches!(
            settings(APP_SAAS, &kv([("app_id", "1"), ("installation_id", "7x")]), true),
            Err(HookError::InvalidInput(_))
        ));
    }

    #[test]
    fn github_timestamps_parse() {
        assert_eq!(parse_rfc3339_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339_utc("2016-07-11T22:14:10Z"), Some(1_468_275_250));
        assert_eq!(parse_rfc3339_utc("2024-02-29T12:00:00Z"), Some(1_709_208_000));
        assert_eq!(parse_rfc3339_utc("2016-07-11 22:14:10"), None);
        assert_eq!(parse_rfc3339_utc("2016-13-11T22:14:10Z"), None);
    }
}
