//! `aws_assume_role` (C-801, C-807, D-261(b)): the gateway calls STS `AssumeRole` at `sts.<endpoint region>.amazonaws.com`
//! with the secret's **source** IAM key (SigV4, service `sts`), `ExternalId` = the org's own `external_id` and
//! `RoleSessionName = aurora-<org_id>`, and signs inference with the session credentials only. `Minted.material` is the
//! session credentials as JSON ([`SessionCredentials`]); `Minted.origin` is the assumed role's account.
//!
//! Also here: the value shapes of `aws_sigv4` and `aws_assume_role`, and the signed `GetCallerIdentity` request the API
//! uses to learn a static key's account (needs no permission).
use super::entra::ValueError;
use super::{MintCtx, MintError, MintSpec, Minted};
use crate::sigv4::{self, SignParams};
use crate::{AuthMode, Endpoint, KeyOrigin};
use aurora_core::Secret;
use aurora_core::limits::LLM_ASSUME_ROLE_DURATION;
use http::HeaderMap;
use serde::{Deserialize, Serialize};

const API_VERSION: &str = "2011-06-15";
/// `session_duration_secs` when the value names none (`02 §Limits`).
const DEFAULT_DURATION_SECS: u64 = 3_600;

const fn bad(reason: &'static str, field: Option<&'static str>) -> ValueError {
    ValueError { reason, field }
}

/// The partition of an AWS region name (`aws-us-gov` for `us-gov-*`, else `aws`).
#[must_use]
pub fn partition_of_region(region: &str) -> &'static str {
    if region.starts_with("us-gov-") {
        "aws-us-gov"
    } else {
        "aws"
    }
}

/// The pinned STS origin of a region, which `llm.dev_upstream_overrides` may replace (never the global endpoint).
#[must_use]
pub fn sts_origin(region: &str) -> String {
    format!("https://sts.{region}.amazonaws.com")
}

/// A checked IAM key (`aws_sigv4`), or the source key of an `aws_assume_role` value.
pub struct AwsKey {
    pub access_key_id: String,
    secret: Secret<String>,
    session: Option<Secret<String>>,
}

impl std::fmt::Debug for AwsKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsKey").finish_non_exhaustive()
    }
}

impl AwsKey {
    #[must_use]
    pub fn secret_access_key(&self) -> &str {
        self.secret.expose()
    }
    #[must_use]
    pub fn session_token(&self) -> Option<&str> {
        self.session.as_ref().map(|s| s.expose().as_str())
    }
}

/// A checked `aws_assume_role` value.
pub struct AssumeRole {
    pub key: AwsKey,
    pub role_arn: String,
    pub duration_secs: u64,
}

impl std::fmt::Debug for AssumeRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssumeRole")
            .field("role_arn", &self.role_arn)
            .finish_non_exhaustive()
    }
}

/// The session credentials of an assumed role: the `material` of the STS mint (JSON), which the gateway signs
/// inference with. No `Debug`.
#[derive(Serialize, Deserialize)]
pub struct SessionCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
}

fn object<'a>(
    v: &'a serde_json::Value,
    allowed: &[&str],
) -> Result<&'a serde_json::Map<String, serde_json::Value>, ValueError> {
    let o = v.as_object().ok_or_else(|| bad("missing_field", None))?;
    if o.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(bad("unknown_field", None));
    }
    Ok(o)
}

fn text<'a>(
    o: &'a serde_json::Map<String, serde_json::Value>,
    f: &'static str,
) -> Result<&'a str, ValueError> {
    o.get(f)
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad("missing_field", Some(f)))
}

fn key_of(
    o: &serde_json::Map<String, serde_json::Value>,
    with_session: bool,
) -> Result<AwsKey, ValueError> {
    let session = if with_session {
        match o.get("session_token") {
            None | Some(serde_json::Value::Null) => None,
            Some(_) => Some(Secret::new(text(o, "session_token")?.to_owned())),
        }
    } else {
        None
    };
    Ok(AwsKey {
        access_key_id: text(o, "access_key_id")?.to_owned(),
        secret: Secret::new(text(o, "secret_access_key")?.to_owned()),
        session,
    })
}

/// C-801 `aws_sigv4`: `{ access_key_id, secret_access_key, session_token? }`.
///
/// # Errors
/// [`ValueError`] with the C-801 reason; never any part of the value.
pub fn parse_sigv4_value(value: &[u8]) -> Result<AwsKey, ValueError> {
    let v: serde_json::Value =
        serde_json::from_slice(value).map_err(|_| bad("missing_field", None))?;
    let o = object(&v, &["access_key_id", "secret_access_key", "session_token"])?;
    key_of(o, true)
}

/// `arn:aws(-us-gov)?:iam::<12 digits>:role/<path and name>`: the account id and the partition.
fn role_arn_parts(arn: &str) -> Option<(&str, &str)> {
    let rest = arn.strip_prefix("arn:")?;
    let (partition, rest) = rest.split_once(":iam::")?;
    let (account, role) = rest.split_once(":role/")?;
    (matches!(partition, "aws" | "aws-us-gov")
        && account.len() == 12
        && account.bytes().all(|b| b.is_ascii_digit())
        && !role.is_empty()
        && role.len() <= 512
        && role
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+=,.@_-/".contains(&b)))
    .then_some((partition, account))
}

/// C-801 `aws_assume_role`: `{ access_key_id, secret_access_key, role_arn, session_duration_secs? }` with the
/// partition of `role_arn` equal to the endpoint region's and the duration within `LLM_ASSUME_ROLE_DURATION`.
///
/// # Errors
/// [`ValueError`] with the C-801 reason (`role_arn_invalid`, `partition_mismatch`, `duration_out_of_range`, …).
pub fn parse_assume_role_value(value: &[u8], region: &str) -> Result<AssumeRole, ValueError> {
    let v: serde_json::Value =
        serde_json::from_slice(value).map_err(|_| bad("missing_field", None))?;
    let o = object(
        &v,
        &[
            "access_key_id",
            "secret_access_key",
            "role_arn",
            "session_duration_secs",
        ],
    )?;
    let key = key_of(o, false)?;
    let role_arn = text(o, "role_arn")?;
    let (partition, _) =
        role_arn_parts(role_arn).ok_or_else(|| bad("role_arn_invalid", Some("role_arn")))?;
    if partition != partition_of_region(region) {
        return Err(bad("partition_mismatch", Some("role_arn")));
    }
    let duration_secs = match o.get("session_duration_secs") {
        None | Some(serde_json::Value::Null) => DEFAULT_DURATION_SECS,
        Some(d) => d
            .as_u64()
            .filter(|d| LLM_ASSUME_ROLE_DURATION.contains(d))
            .ok_or_else(|| bad("duration_out_of_range", Some("session_duration_secs")))?,
    };
    Ok(AssumeRole {
        key,
        role_arn: role_arn.to_owned(),
        duration_secs,
    })
}

/// `application/x-www-form-urlencoded` component encoding.
fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A signed `POST /` form call to STS in `region` with `key` (service `sts`); `origin` already carries any override.
///
/// # Errors
/// `Unavailable` when the request cannot be built.
pub fn signed_sts_request(
    origin: &str,
    region: &str,
    key: &AwsKey,
    now: std::time::SystemTime,
    body: String,
) -> Result<http::Request<Vec<u8>>, MintError> {
    let uri: http::Uri = format!("{origin}/")
        .parse()
        .map_err(|_| MintError::unavailable())?;
    let host = sigv4::host_header(&uri).ok_or_else(MintError::unavailable)?;
    let amz_date = sigv4::amz_date(now);
    let signed = sigv4::sign(&SignParams {
        method: &http::Method::POST,
        host_header: &host,
        path: "/",
        query: None,
        region,
        service: "sts",
        access_key: &key.access_key_id,
        secret_key: key.secret_access_key(),
        session_token: key.session_token(),
        amz_date: &amz_date,
        payload_hash: &sigv4::sha256_hex(body.as_bytes()),
        sign_content_sha256: false,
    });
    let mut b = http::Request::builder()
        .method(http::Method::POST)
        .uri(uri)
        .header(
            http::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded; charset=utf-8",
        )
        .header(http::header::HOST, host)
        .header("x-amz-date", &amz_date)
        .header(http::header::AUTHORIZATION, signed.authorization);
    if let Some(t) = key.session_token() {
        b = b.header("x-amz-security-token", t);
    }
    b.body(body.into_bytes())
        .map_err(|_| MintError::unavailable())
}

/// The signed `GetCallerIdentity` call of a static IAM key (`aws_sigv4` validation and origin).
///
/// # Errors
/// `Unavailable` when the request cannot be built.
pub fn caller_identity_request(
    origin: &str,
    region: &str,
    key: &AwsKey,
    now: std::time::SystemTime,
) -> Result<http::Request<Vec<u8>>, MintError> {
    signed_sts_request(
        origin,
        region,
        key,
        now,
        format!("Action=GetCallerIdentity&Version={API_VERSION}"),
    )
}

/// The text of the first `<tag>…</tag>` of an XML document (the STS answers are flat enough for this); `None` when
/// absent or when the text holds markup.
#[must_use]
pub fn xml_text<'a>(doc: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let start = doc.find(&open)? + open.len();
    let end = start + doc[start..].find(&format!("</{tag}>"))?;
    let t = doc[start..end].trim();
    (!t.is_empty() && !t.contains('<')).then_some(t)
}

/// The account id of `GetCallerIdentity`'s answer.
///
/// # Errors
/// The STS error class, or `Unavailable` for an answer without an account.
pub fn caller_account(status: u16, body: &[u8]) -> Result<String, MintError> {
    let doc = String::from_utf8_lossy(body);
    if status != 200 {
        return Err(sts_failure(&doc));
    }
    xml_text(&doc, "Account")
        .filter(|a| a.len() == 12 && a.bytes().all(|b| b.is_ascii_digit()))
        .map(str::to_owned)
        .ok_or_else(MintError::unavailable)
}

/// An STS error document: the codes that say the key or role is refused are `Rejected`, everything else `Unavailable`.
fn sts_failure(doc: &str) -> MintError {
    let code = xml_text(doc, "Code").filter(|c| {
        c.len() <= 64
            && c.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    });
    match code {
        Some(
            c
            @ ("AccessDenied" | "InvalidClientTokenId" | "SignatureDoesNotMatch" | "ExpiredToken"),
        ) => MintError::rejected(Some(c.to_owned())),
        _ => MintError::unavailable(),
    }
}

pub struct StsSpec;

impl MintSpec for StsSpec {
    fn mode(&self) -> AuthMode {
        AuthMode::AwsAssumeRole
    }

    fn region(&self, ep: &Endpoint) -> String {
        ep.region.clone().unwrap_or_default()
    }

    fn build(&self, ctx: &MintCtx<'_>) -> Result<http::Request<Vec<u8>>, MintError> {
        let region = ctx.endpoint.region.as_deref().unwrap_or_default();
        let v =
            parse_assume_role_value(ctx.value, region).map_err(|_| MintError::rejected(None))?;
        // Fail closed: without the org's external id the role would be assumed without the confused-deputy guard.
        let external = ctx
            .org_external_id
            .filter(|e| !e.is_empty())
            .ok_or_else(MintError::unavailable)?;
        let body = format!(
            "Action=AssumeRole&Version={API_VERSION}&RoleArn={}&RoleSessionName={}&ExternalId={}&DurationSeconds={}",
            form_encode(&v.role_arn),
            form_encode(&format!("aurora-{}", ctx.org_id)),
            form_encode(external),
            v.duration_secs
        );
        signed_sts_request(
            &ctx.overrides.origin(&sts_origin(region)),
            region,
            &v.key,
            ctx.now,
            body,
        )
    }

    fn parse(
        &self,
        ctx: &MintCtx<'_>,
        status: u16,
        _headers: &HeaderMap,
        body: &[u8],
    ) -> Result<Minted, MintError> {
        let doc = String::from_utf8_lossy(body);
        if status != 200 {
            return Err(sts_failure(&doc));
        }
        let field = |t: &str| xml_text(&doc, t).ok_or_else(MintError::unavailable);
        let creds = SessionCredentials {
            access_key_id: field("AccessKeyId")?.to_owned(),
            secret_access_key: field("SecretAccessKey")?.to_owned(),
            session_token: field("SessionToken")?.to_owned(),
        };
        let expires_at =
            sigv4::parse_rfc3339(field("Expiration")?).ok_or_else(MintError::unavailable)?;
        // The account of the assumed role (not the source key's): `arn:aws:sts::<account>:assumed-role/<role>/<session>`.
        let assumed = field("Arn")?;
        let account = assumed
            .split(':')
            .nth(4)
            .filter(|a| a.len() == 12 && a.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(MintError::unavailable)?;
        let region = ctx.endpoint.region.as_deref().unwrap_or_default();
        let role_arn = parse_assume_role_value(ctx.value, region)
            .map_err(|_| MintError::rejected(None))?
            .role_arn;
        let material = serde_json::to_string(&creds).map_err(|_| MintError::unavailable())?;
        Ok(Minted {
            material: Secret::new(material),
            expires_at,
            origin: Some(KeyOrigin {
                organization_id: Some(account.to_owned()),
                user_id: Some(role_arn),
                ..KeyOrigin::default()
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Overrides;
    use std::time::{Duration, UNIX_EPOCH};

    const ROLE: &str = "arn:aws:iam::123456789012:role/aurora-bedrock";

    fn value(role: &str, extra: &str) -> Vec<u8> {
        format!(
            r#"{{"access_key_id":"AKIDEXAMPLE","secret_access_key":"wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY","role_arn":"{role}"{extra}}}"#
        )
        .into_bytes()
    }

    fn ep(region: &str) -> Endpoint {
        Endpoint {
            id: "b".into(),
            provider: "bedrock".into(),
            display_name: "Bedrock".into(),
            base_url: String::new(),
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
            region: Some(region.into()),
            project_id: None,
            location: None,
            model_roles: Default::default(),
            model_info: Default::default(),
        }
    }

    fn reason(r: Result<AssumeRole, ValueError>) -> (&'static str, Option<&'static str>) {
        let e = r.expect_err("refused");
        (e.reason, e.field)
    }

    #[test]
    fn value_shapes_and_partitions() {
        let ok = parse_assume_role_value(&value(ROLE, ""), "us-east-1").unwrap();
        assert_eq!(ok.duration_secs, 3600);
        assert_eq!(
            parse_assume_role_value(&value(ROLE, r#","session_duration_secs":900"#), "us-east-1")
                .unwrap()
                .duration_secs,
            900
        );
        for bad in [899, 43_201] {
            assert_eq!(
                reason(parse_assume_role_value(
                    &value(ROLE, &format!(r#","session_duration_secs":{bad}"#)),
                    "us-east-1"
                )),
                ("duration_out_of_range", Some("session_duration_secs"))
            );
        }
        assert_eq!(
            reason(parse_assume_role_value(
                &value("arn:aws:iam::123:role/x", ""),
                "us-east-1"
            )),
            ("role_arn_invalid", Some("role_arn"))
        );
        // A GovCloud role on a commercial region (and the reverse) is refused.
        let gov = "arn:aws-us-gov:iam::123456789012:role/r";
        assert_eq!(
            reason(parse_assume_role_value(&value(gov, ""), "us-east-1")),
            ("partition_mismatch", Some("role_arn"))
        );
        assert_eq!(
            reason(parse_assume_role_value(&value(ROLE, ""), "us-gov-west-1")),
            ("partition_mismatch", Some("role_arn"))
        );
        assert!(parse_assume_role_value(&value(gov, ""), "us-gov-west-1").is_ok());
        assert_eq!(
            reason(parse_assume_role_value(
                &value(ROLE, r#","region":"us-east-1""#),
                "us-east-1"
            )),
            ("unknown_field", None)
        );
        assert!(
            parse_sigv4_value(
                br#"{"access_key_id":"A","secret_access_key":"S","session_token":"T"}"#
            )
            .is_ok()
        );
        assert_eq!(
            parse_sigv4_value(br#"{"access_key_id":"A"}"#)
                .unwrap_err()
                .reason,
            "missing_field"
        );
    }

    /// The request for a fixed time, key and org: the canonical request and the signature are the AWS algorithm's
    /// (recomputed here independently of `build`).
    #[test]
    fn the_assume_role_request_is_signed_with_the_source_key() {
        let e = ep("us-east-1");
        let o = Overrides::default();
        let now = UNIX_EPOCH + Duration::from_secs(1_440_938_160); // 2015-08-30T12:36:00Z
        let v = value(ROLE, "");
        let ctx = MintCtx {
            endpoint: &e,
            value: &v,
            org_id: "0b0b0b0b-0000-4000-8000-000000000001",
            org_external_id: Some("ext-1"),
            now,
            overrides: &o,
        };
        let req = StsSpec.build(&ctx).unwrap();
        assert_eq!(
            req.uri().to_string(),
            "https://sts.us-east-1.amazonaws.com/"
        );
        let body = String::from_utf8(req.body().clone()).unwrap();
        assert_eq!(
            body,
            "Action=AssumeRole&Version=2011-06-15&RoleArn=arn%3Aaws%3Aiam%3A%3A123456789012%3Arole%2Faurora-bedrock&RoleSessionName=aurora-0b0b0b0b-0000-4000-8000-000000000001&ExternalId=ext-1&DurationSeconds=3600"
        );
        assert_eq!(req.headers()["x-amz-date"], "20150830T123600Z");
        let expected = sigv4::sign(&SignParams {
            method: &http::Method::POST,
            host_header: "sts.us-east-1.amazonaws.com",
            path: "/",
            query: None,
            region: "us-east-1",
            service: "sts",
            access_key: "AKIDEXAMPLE",
            secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            session_token: None,
            amz_date: "20150830T123600Z",
            payload_hash: &sigv4::sha256_hex(body.as_bytes()),
            sign_content_sha256: false,
        });
        assert_eq!(
            req.headers()["authorization"],
            expected.authorization.as_str()
        );
        assert!(
            expected
                .authorization
                .contains("Credential=AKIDEXAMPLE/20150830/us-east-1/sts/aws4_request, SignedHeaders=host;x-amz-date")
        );
        // No external id: no request at all.
        let none = MintCtx {
            org_external_id: None,
            ..ctx
        };
        assert_eq!(
            StsSpec.build(&none).unwrap_err().class,
            super::super::MintClass::Unavailable
        );
    }

    const OK: &str = r#"<AssumeRoleResponse xmlns="https://sts.amazonaws.com/doc/2011-06-15/"><AssumeRoleResult><Credentials><AccessKeyId>ASIAEXAMPLE</AccessKeyId><SecretAccessKey>sess-secret</SecretAccessKey><SessionToken>sess-token</SessionToken><Expiration>2026-11-15T13:00:00Z</Expiration></Credentials><AssumedRoleUser><AssumedRoleId>AROA:aurora</AssumedRoleId><Arn>arn:aws:sts::123456789012:assumed-role/aurora-bedrock/aurora-x</Arn></AssumedRoleUser></AssumeRoleResult></AssumeRoleResponse>"#;

    fn parse(status: u16, body: &str) -> Result<Minted, MintError> {
        let (e, o) = (ep("us-east-1"), Overrides::default());
        let v = value(ROLE, "");
        let ctx = MintCtx {
            endpoint: &e,
            value: &v,
            org_id: "x",
            org_external_id: Some("e"),
            now: UNIX_EPOCH,
            overrides: &o,
        };
        StsSpec.parse(&ctx, status, &HeaderMap::new(), body.as_bytes())
    }

    #[test]
    fn the_answer_gives_session_credentials_and_the_roles_account() {
        let m = parse(200, OK).unwrap();
        let c: SessionCredentials = serde_json::from_str(m.material.expose()).unwrap();
        assert_eq!(
            (c.access_key_id.as_str(), c.session_token.as_str()),
            ("ASIAEXAMPLE", "sess-token")
        );
        assert_eq!(
            m.expires_at,
            sigv4::parse_rfc3339("2026-11-15T13:00:00Z").unwrap()
        );
        let o = m.origin.unwrap();
        assert_eq!(o.organization_id.as_deref(), Some("123456789012"));
        assert_eq!(o.user_id.as_deref(), Some(ROLE));
    }

    #[test]
    fn error_documents_are_classified_by_code() {
        let err = |code: &str| {
            format!(
                "<ErrorResponse><Error><Type>Sender</Type><Code>{code}</Code><Message>nope arn:aws:iam::1:role/x</Message></Error></ErrorResponse>"
            )
        };
        for c in [
            "AccessDenied",
            "InvalidClientTokenId",
            "SignatureDoesNotMatch",
            "ExpiredToken",
        ] {
            let e = parse(403, &err(c)).err().unwrap();
            assert_eq!(e.class, super::super::MintClass::Rejected, "{c}");
            assert_eq!(e.provider_code.as_deref(), Some(c));
        }
        for c in [
            "Throttling",
            "ServiceUnavailable",
            "RegionDisabledException",
        ] {
            assert_eq!(
                parse(400, &err(c)).err().unwrap().class,
                super::super::MintClass::Unavailable,
                "{c}"
            );
        }
        assert_eq!(
            parse(200, "<AssumeRoleResponse/>").err().unwrap().class,
            super::super::MintClass::Unavailable
        );
    }
}
