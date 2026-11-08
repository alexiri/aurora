//! `github-app` hooks (docs/09 §8, D-91): a GitHub App authenticates as itself with a short-lived
//! RS256 JWT signed with the app's private key, and exchanges it for an installation access token
//! (https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/
//! generating-an-installation-access-token-for-a-github-app).
//!
//! * `connect.exchange` validates the connect form and mints once, so a wrong key, app id or
//!   installation id is reported when connecting, not on the first agent request.
//! * `token.mint` signs a new JWT and mints a fresh installation token whenever the stored one
//!   expires.
//!
//! Two apps share this guest: `github-app` talks to `https://api.github.com`; `github-app-ghes`
//! talks to `https://<ghes_host>/api/v3`, where `ghes_host` is a connect-form value that the hook
//! host's egress policy pins to the connection's registered instance (a different host is denied).
//! The SaaS app never reads `ghes_host`. The private key is never logged and never appears in an
//! error message.
use aurora_plugin_sdk::*;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::signature::{SignatureEncoding, Signer};
use rsa::RsaPrivateKey;
use sha2::Sha256;

const APP_SAAS: &str = "github-app";
const APP_GHES: &str = "github-app-ghes";
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
}

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
    Ok(Settings {
        app_id: app_id.to_owned(),
        installation_id: installation_id.to_owned(),
        base,
        ghes,
    })
}

/// Accept PKCS#1 (`RSA PRIVATE KEY`, what GitHub downloads) and PKCS#8 PEM, tolerating the
/// whitespace damage of a pasted key: the body is re-wrapped before parsing. Errors never
/// include any part of the input.
fn load_key(pem: &str) -> Result<RsaPrivateKey, HookError> {
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
        "RSA PRIVATE KEY" => RsaPrivateKey::from_pkcs1_pem(&wrapped).map_err(|_| ()),
        "PRIVATE KEY" => RsaPrivateKey::from_pkcs8_pem(&wrapped).map_err(|_| ()),
        _ => return Err(bad()),
    };
    key.map_err(|()| bad())
}

/// The app JWT (RS256): `iat = now - 60`, `exp = now + 540` (never more than 10 minutes),
/// `iss = <app id>`.
fn app_jwt(key: &RsaPrivateKey, app_id: &str, now: u64) -> Result<String, HookError> {
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
fn mint_request(s: &Settings, jwt: &str) -> HttpRequest {
    HttpRequest {
        method: "POST".into(),
        url: format!("{}/app/installations/{}/access_tokens", s.base, s.installation_id),
        headers: kv([
            ("authorization", format!("Bearer {jwt}").as_str()),
            ("accept", "application/vnd.github+json"),
            ("x-github-api-version", "2022-11-28"),
            ("user-agent", "aurora-github-app"),
        ]),
        body: None,
    }
}

fn mint(s: &Settings, key: &RsaPrivateKey) -> Result<(String, u64), HookError> {
    let now = now_unix_seconds();
    let jwt = app_jwt(key, &s.app_id, now)?;
    let r = fetch(&mint_request(s, &jwt)).map_err(|e| match e {
        FetchError::Denied(m) => HookError::Upstream(denied_message(s, &m)),
        FetchError::Timeout => HookError::Upstream("timeout".into()),
        FetchError::TooLarge => HookError::Upstream("response too large".into()),
        FetchError::Network(m) => HookError::Upstream(m),
    })?;
    match r.status {
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
        st => return Err(HookError::Upstream(format!("GitHub answered {st}"))),
    }
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

    fn key() -> RsaPrivateKey {
        RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 1024).expect("key")
    }

    fn saas() -> Settings {
        Settings {
            app_id: "123456".into(),
            installation_id: "7".into(),
            base: SAAS_BASE.into(),
            ghes: false,
        }
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
        let r = mint_request(&saas(), "aaa.bbb.ccc");
        assert_eq!(r.method, "POST");
        assert_eq!(r.url, "https://api.github.com/app/installations/7/access_tokens");
        assert_eq!(kv_get(&r.headers, "authorization"), Some("Bearer aaa.bbb.ccc"));
        let ghes = Settings {
            base: "https://github.example.com/api/v3".into(),
            ghes: true,
            ..saas()
        };
        assert_eq!(
            mint_request(&ghes, "x.y.z").url,
            "https://github.example.com/api/v3/app/installations/7/access_tokens"
        );
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
