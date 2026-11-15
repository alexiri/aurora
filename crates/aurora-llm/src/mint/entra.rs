//! `azure_entra` (C-801, C-807, D-261(c)): a Microsoft Entra application's client-credentials grant at the public-cloud
//! authority (`https://login.microsoftonline.com/<tenant_id>/oauth2/v2.0/token`, scope
//! `https://cognitiveservices.azure.com/.default`). The access token is injected as `Authorization: Bearer`.
//!
//! The token is a JWT whose `tid` and `appid` (v1 tokens) or `azp` (v2 tokens) name the tenant and the application. The
//! mint reads those claims from the token it just received from the pinned authority over TLS; it does **not** validate
//! the token's signature: the gateway is not the token's audience (Azure OpenAI is), the authority is pinned, and the
//! claims are used only as the key-origin evidence (C-804), never to authorize anything.
use super::{MintCtx, MintError, MintSpec, Minted};
use crate::{AuthMode, Endpoint, KeyOrigin};
use aurora_core::Secret;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::HeaderMap;
use std::time::Duration;

/// The pinned authority origin `llm.dev_upstream_overrides` may replace.
pub const AUTHORITY_ORIGIN: &str = "https://login.microsoftonline.com";
pub const SCOPE: &str = "https://cognitiveservices.azure.com/.default";

/// Why an Entra value is refused at save (the `details.reason` of C-801), with the offending field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueError {
    pub reason: &'static str,
    pub field: Option<&'static str>,
}

const fn bad(reason: &'static str, field: Option<&'static str>) -> ValueError {
    ValueError { reason, field }
}

/// A checked Entra application credential; the client secret lives in zeroizing memory and has no `Debug`.
pub struct EntraApp {
    pub tenant_id: String,
    pub client_id: String,
    secret: Secret<String>,
}

impl std::fmt::Debug for EntraApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntraApp")
            .field("tenant_id", &self.tenant_id)
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

/// `8-4-4-4-12` hexadecimal.
fn is_guid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The shape checks of C-801 for `azure_entra`, before any call: a JSON object of exactly `tenant_id`, `client_id`,
/// `client_secret`; GUIDs for the first two; the multi-tenant aliases are `tenant_not_specific`.
///
/// # Errors
/// [`ValueError`] naming the C-801 reason; never any part of the value.
pub fn parse_value(value: &[u8]) -> Result<EntraApp, ValueError> {
    let v: serde_json::Value =
        serde_json::from_slice(value).map_err(|_| bad("missing_field", None))?;
    let o = v.as_object().ok_or_else(|| bad("missing_field", None))?;
    if o.keys()
        .any(|k| !matches!(k.as_str(), "tenant_id" | "client_id" | "client_secret"))
    {
        return Err(bad("unknown_field", None));
    }
    let get = |f: &'static str| {
        o.get(f)
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad("missing_field", Some(f)))
    };
    let tenant = get("tenant_id")?;
    let client = get("client_id")?;
    let secret = get("client_secret")?;
    if matches!(
        tenant.to_ascii_lowercase().as_str(),
        "common" | "organizations" | "consumers"
    ) || !is_guid(tenant)
    {
        return Err(bad("tenant_not_specific", Some("tenant_id")));
    }
    if !is_guid(client) {
        return Err(bad("missing_field", Some("client_id")));
    }
    Ok(EntraApp {
        tenant_id: tenant.to_ascii_lowercase(),
        client_id: client.to_ascii_lowercase(),
        secret: Secret::new(secret.to_owned()),
    })
}

/// `application/x-www-form-urlencoded` component encoding.
fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'*') {
            out.push(char::from(b));
        } else if b == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The claims `tid` and `appid`/`azp` of a JWT's payload, without any signature check (see the module note).
fn claims(token: &str) -> Option<(String, Option<String>)> {
    let payload = token.split('.').nth(1)?;
    let raw = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    let s = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .filter(|x| !x.is_empty())
            .map(str::to_owned)
    };
    Some((s("tid")?, s("appid").or_else(|| s("azp"))))
}

pub struct EntraSpec;

impl MintSpec for EntraSpec {
    fn mode(&self) -> AuthMode {
        AuthMode::AzureEntra
    }

    fn region(&self, _ep: &Endpoint) -> String {
        // The tenant is part of the secret's value, which the version already pins; one cache slot per secret version.
        String::new()
    }

    fn build(&self, ctx: &MintCtx<'_>) -> Result<http::Request<Vec<u8>>, MintError> {
        let app = parse_value(ctx.value).map_err(|_| MintError::rejected(None))?;
        let body = format!(
            "grant_type=client_credentials&client_id={}&client_secret={}&scope={}",
            form_encode(&app.client_id),
            form_encode(app.secret.expose()),
            form_encode(SCOPE),
        );
        let url = format!(
            "{}/{}/oauth2/v2.0/token",
            ctx.overrides.origin(AUTHORITY_ORIGIN),
            app.tenant_id
        );
        http::Request::builder()
            .method(http::Method::POST)
            .uri(url)
            .header(
                http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .header(http::header::ACCEPT, "application/json")
            .body(body.into_bytes())
            .map_err(|_| MintError::unavailable())
    }

    fn parse(
        &self,
        ctx: &MintCtx<'_>,
        status: u16,
        _headers: &HeaderMap,
        body: &[u8],
    ) -> Result<Minted, MintError> {
        if status != 200 {
            // `invalid_client`, `unauthorized_client`, `invalid_request` (400/401): the application was refused.
            return Err(super::failure(status, body));
        }
        let v: serde_json::Value =
            serde_json::from_slice(body).map_err(|_| MintError::unavailable())?;
        let token = v
            .get("access_token")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
            .ok_or_else(MintError::unavailable)?;
        // Entra returns `expires_in` as a number (v2) or, on some paths, a numeric string.
        let expires_in = v
            .get("expires_in")
            .and_then(|e| {
                e.as_u64()
                    .or_else(|| e.as_str().and_then(|s| s.parse().ok()))
            })
            .filter(|e| *e > 0)
            .ok_or_else(MintError::unavailable)?;
        let (tid, app) = claims(token).ok_or_else(MintError::unavailable)?;
        // The token must be for the tenant the secret names (a mismatch is a refused credential, not an outage).
        let named = parse_value(ctx.value).map_err(|_| MintError::rejected(None))?;
        if !tid.eq_ignore_ascii_case(&named.tenant_id) {
            return Err(MintError::rejected(None));
        }
        Ok(Minted {
            material: Secret::new(token.to_owned()),
            expires_at: ctx.now + Duration::from_secs(expires_in),
            origin: Some(KeyOrigin {
                organization_id: Some(tid.to_ascii_lowercase()),
                user_id: app.map(|a| a.to_ascii_lowercase()),
                ..KeyOrigin::default()
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Overrides;
    use std::time::SystemTime;

    const TENANT: &str = "11111111-2222-3333-4444-555555555555";
    const CLIENT: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

    fn value(secret: &str) -> Vec<u8> {
        serde_json::json!({"tenant_id": TENANT, "client_id": CLIENT, "client_secret": secret})
            .to_string()
            .into_bytes()
    }

    fn ep() -> Endpoint {
        Endpoint {
            id: "az".into(),
            provider: "azure_openai".into(),
            display_name: "Azure".into(),
            base_url: "https://acme.openai.azure.com".into(),
            dialects: vec![],
            default_model: None,
            models: vec![],
            allowed_models: vec![],
            key_origin: None,
            allowed_paths: vec![],
            organization: None,
            project: None,
            auth_header: None,
            models_path: None,
            region: None,
            project_id: None,
            location: None,
        }
    }

    fn jwt(claims: &serde_json::Value) -> String {
        format!(
            "{}.{}.sig",
            URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        )
    }

    fn parse(value: &[u8], status: u16, body: &str) -> Result<Minted, MintError> {
        let (e, o) = (ep(), Overrides::default());
        let ctx = MintCtx {
            endpoint: &e,
            value,
            org_id: "",
            org_external_id: None,
            now: SystemTime::UNIX_EPOCH,
            overrides: &o,
        };
        EntraSpec.parse(&ctx, status, &HeaderMap::new(), body.as_bytes())
    }

    #[test]
    fn builds_the_exact_form_at_the_pinned_authority() {
        let (e, o) = (ep(), Overrides::default());
        let v = value("s3&cr=et +x");
        let ctx = MintCtx {
            endpoint: &e,
            value: &v,
            org_id: "",
            org_external_id: None,
            now: SystemTime::UNIX_EPOCH,
            overrides: &o,
        };
        let req = EntraSpec.build(&ctx).unwrap();
        assert_eq!(
            req.uri().to_string(),
            format!("https://login.microsoftonline.com/{TENANT}/oauth2/v2.0/token")
        );
        assert_eq!(
            String::from_utf8(req.into_body()).unwrap(),
            format!(
                "grant_type=client_credentials&client_id={CLIENT}&client_secret=s3%26cr%3Det+%2Bx\
&scope=https%3A%2F%2Fcognitiveservices.azure.com%2F.default"
            )
        );
    }

    #[test]
    fn debug_never_shows_the_secret() {
        let app = parse_value(&value("hunter2-hunter2")).unwrap();
        assert!(!format!("{app:?}").contains("hunter2"));
    }

    #[test]
    fn parses_expiry_and_claims() {
        let t = jwt(&serde_json::json!({"tid": TENANT, "appid": CLIENT}));
        let m = parse(
            &value("s"),
            200,
            &serde_json::json!({"access_token": t, "expires_in": 3599, "token_type": "Bearer"})
                .to_string(),
        )
        .unwrap();
        assert_eq!(
            m.expires_at,
            SystemTime::UNIX_EPOCH + Duration::from_secs(3599)
        );
        let o = m.origin.unwrap();
        assert_eq!(o.organization_id.as_deref(), Some(TENANT));
        assert_eq!(o.user_id.as_deref(), Some(CLIENT));
        // v2 tokens carry `azp`, and `expires_in` may be a string
        let t = jwt(&serde_json::json!({"tid": TENANT, "azp": CLIENT}));
        let m = parse(
            &value("s"),
            200,
            &serde_json::json!({"access_token": t, "expires_in": "60"}).to_string(),
        )
        .unwrap();
        assert_eq!(m.origin.unwrap().user_id.as_deref(), Some(CLIENT));
    }

    #[test]
    fn errors_map_to_classes_without_the_message() {
        let body = r#"{"error":"invalid_client","error_description":"AADSTS7000215: Invalid client secret s3cr3t"}"#;
        for status in [400, 401] {
            let e = parse(&value("s"), status, body).err().unwrap();
            assert_eq!(e.class, super::super::MintClass::Rejected);
            assert_eq!(e.provider_code.as_deref(), Some("invalid_client"));
            assert!(!format!("{e:?}").contains("AADSTS"));
        }
        for status in [429, 500, 503] {
            let e = parse(&value("s"), status, body).err().unwrap();
            assert_eq!(e.class, super::super::MintClass::Unavailable);
        }
        // a token for another tenant is a refused credential
        let t = jwt(
            &serde_json::json!({"tid": "99999999-2222-3333-4444-555555555555", "appid": CLIENT}),
        );
        let e = parse(
            &value("s"),
            200,
            &serde_json::json!({"access_token": t, "expires_in": 60}).to_string(),
        )
        .err()
        .unwrap();
        assert_eq!(e.class, super::super::MintClass::Rejected);
    }

    #[test]
    fn value_shape() {
        let v = |tenant: &str, client: &str| {
            serde_json::json!({"tenant_id": tenant, "client_id": client, "client_secret": "x"})
                .to_string()
                .into_bytes()
        };
        for t in [
            "common",
            "organizations",
            "consumers",
            "contoso.onmicrosoft.com",
        ] {
            assert_eq!(
                parse_value(&v(t, CLIENT)).err().unwrap().reason,
                "tenant_not_specific"
            );
        }
        assert_eq!(
            parse_value(&v(TENANT, "nope")).err().unwrap().field,
            Some("client_id")
        );
        let extra = serde_json::json!({"tenant_id": TENANT, "client_id": CLIENT, "client_secret": "x", "authority": "https://evil"})
            .to_string();
        assert_eq!(
            parse_value(extra.as_bytes()).err().unwrap().reason,
            "unknown_field"
        );
        let missing = serde_json::json!({"tenant_id": TENANT, "client_id": CLIENT}).to_string();
        assert_eq!(
            parse_value(missing.as_bytes()).err().unwrap(),
            bad("missing_field", Some("client_secret"))
        );
        assert!(parse_value(&v(TENANT, CLIENT)).is_ok());
    }
}
