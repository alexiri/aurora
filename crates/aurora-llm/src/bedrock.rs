use crate::http_util::host_of;
use crate::mint::sts::{self, SessionCredentials};
use crate::mint::{self, MintClass, MintCtx, MintError};
use crate::sigv4::{self, SignParams};
use crate::{
    AuthMode, Dialect, Endpoint, EndpointKind, InjectionHeader, KeyCheck, KeyOrigin, LlmError,
    LlmInjection, LlmProvider, ModelInfo, ModelRef, OriginMethod, Overrides, Result,
};
use aurora_core::Secret;
use http::{HeaderMap, Method};
use percent_encoding::percent_decode_str;
use std::time::SystemTime;

const PLACEHOLDER: &str = "aurora-placeholder";

/// AWS Bedrock (`08 §Phase 4 providers`, C-800, C-801, C-804, C-806, C-809, C-810; D-261(a), (b)): the runtime host
/// `bedrock-runtime.<region>.amazonaws.com` (or the entry's VPC `base_url`) is the only injection target; the control-plane
/// host `bedrock.<region>.amazonaws.com` is used by the API for listing and is never reachable with a key. Three
/// credentials: `aws_assume_role` (a source IAM key and a role the gateway assumes with the org's external id),
/// `aws_sigv4` (a static IAM key) and `api_key` (a Bedrock API key as bearer). Requests are signed in the gateway only.
#[derive(Debug, Clone, Default)]
pub struct BedrockProvider {
    overrides: Overrides,
}

impl BedrockProvider {
    /// With `llm.dev_upstream_overrides` applied to the AWS origins (tests and development only).
    #[must_use]
    pub fn with_overrides(overrides: Overrides) -> Self {
        Self { overrides }
    }

    fn region(ep: &Endpoint) -> &str {
        ep.region.as_deref().unwrap_or_default()
    }

    /// The pinned runtime origin of an entry (`https://bedrock-runtime.<region>.amazonaws.com`).
    #[must_use]
    pub fn pinned_origin(ep: &Endpoint) -> String {
        format!("https://bedrock-runtime.{}.amazonaws.com", Self::region(ep))
    }

    fn control_origin(&self, ep: &Endpoint) -> String {
        self.overrides.origin(&format!(
            "https://bedrock.{}.amazonaws.com",
            Self::region(ep)
        ))
    }

    fn sts_origin(&self, ep: &Endpoint) -> String {
        self.overrides.origin(&sts::sts_origin(Self::region(ep)))
    }

    fn mint_ctx<'a>(
        &'a self,
        ep: &'a Endpoint,
        check: &'a KeyCheck,
        value: &'a [u8],
    ) -> MintCtx<'a> {
        MintCtx {
            endpoint: ep,
            value,
            org_id: &check.org_id,
            org_external_id: check.org_external_id.as_deref(),
            now: SystemTime::now(),
            overrides: &self.overrides,
        }
    }

    /// One `AssumeRole`: the session credentials and the role's account.
    async fn assume(
        &self,
        ep: &Endpoint,
        check: &KeyCheck,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<mint::Minted> {
        let value = key.expose().as_bytes();
        sts::parse_assume_role_value(value, Self::region(ep))
            .map_err(|e| LlmError::Unverifiable(format!("key_invalid:{}", e.reason)))?;
        match mint::run(http, &sts::StsSpec, &self.mint_ctx(ep, check, value)).await {
            Ok(m) => Ok(m),
            Err(MintError {
                class: MintClass::Rejected,
                provider_code,
            }) => Err(if provider_code.as_deref() == Some("AccessDenied") {
                LlmError::AssumeRoleDenied
            } else {
                LlmError::MintRejected
            }),
            Err(_) => Err(LlmError::Unreachable("STS unavailable".into())),
        }
    }

    /// The account of a static IAM key (signed `GetCallerIdentity`).
    async fn caller(
        &self,
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<String> {
        let k = sts::parse_sigv4_value(key.expose().as_bytes())
            .map_err(|e| LlmError::Unverifiable(format!("key_invalid:{}", e.reason)))?;
        let req = sts::caller_identity_request(
            &self.sts_origin(ep),
            Self::region(ep),
            &k,
            SystemTime::now(),
        )
        .map_err(|_| LlmError::Unreachable("request failed".into()))?;
        let (status, body) = send_built(http, req).await?;
        sts::caller_account(status, &body).map_err(|e| match e.class {
            MintClass::Rejected => LlmError::InvalidKey,
            MintClass::Unavailable => LlmError::Unreachable("STS unavailable".into()),
        })
    }

    /// The credentials a listing is signed with: the session of an assumed role, or the key itself.
    async fn signer(
        &self,
        ep: &Endpoint,
        check: &KeyCheck,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Auth> {
        match check.mode {
            AuthMode::AwsAssumeRole => {
                let m = self.assume(ep, check, key, http).await?;
                let c: SessionCredentials = serde_json::from_str(m.material.expose())
                    .map_err(|_| LlmError::BadResponse("session credentials".into()))?;
                Ok(Auth::Sigv4 {
                    access_key: c.access_key_id,
                    secret_key: Secret::new(c.secret_access_key),
                    token: Some(Secret::new(c.session_token)),
                })
            }
            AuthMode::AwsSigv4 => {
                let k = sts::parse_sigv4_value(key.expose().as_bytes())
                    .map_err(|_| LlmError::InvalidKey)?;
                Ok(Auth::Sigv4 {
                    access_key: k.access_key_id.clone(),
                    secret_key: Secret::new(k.secret_access_key().to_owned()),
                    token: k.session_token().map(|t| Secret::new(t.to_owned())),
                })
            }
            _ => Ok(Auth::Bearer(Secret::new(key.expose().clone()))),
        }
    }

    /// `GET <control plane><path>` with the credentials; the JSON answer.
    async fn control_get(
        &self,
        ep: &Endpoint,
        auth: &Auth,
        path: &str,
        http: &reqwest::Client,
    ) -> Result<serde_json::Value> {
        let origin = self.control_origin(ep);
        let uri: http::Uri = format!("{origin}{path}")
            .parse()
            .map_err(|_| LlmError::Config("control-plane url".into()))?;
        let mut rb = http.get(uri.to_string());
        match auth {
            Auth::Bearer(k) => rb = rb.bearer_auth(k.expose()),
            Auth::Sigv4 {
                access_key,
                secret_key,
                token,
            } => {
                let host = sigv4::host_header(&uri)
                    .ok_or_else(|| LlmError::Config("control-plane host".into()))?;
                let date = sigv4::amz_date(SystemTime::now());
                let signed = sigv4::sign(&SignParams {
                    method: &Method::GET,
                    host_header: &host,
                    path,
                    query: None,
                    region: Self::region(ep),
                    service: "bedrock",
                    access_key,
                    secret_key: secret_key.expose(),
                    session_token: token.as_ref().map(|t| t.expose().as_str()),
                    amz_date: &date,
                    payload_hash: &sigv4::sha256_hex(b""),
                    sign_content_sha256: false,
                });
                rb = rb
                    .header("x-amz-date", date)
                    .header("authorization", signed.authorization);
                if let Some(t) = token {
                    rb = rb.header("x-amz-security-token", t.expose());
                }
            }
        }
        let resp = crate::http_util::send(rb).await?;
        crate::http_util::json(crate::http_util::check(resp)?).await
    }

    /// `ListInferenceProfiles` then `ListFoundationModels`, inference-profile ids first.
    async fn listing(
        &self,
        ep: &Endpoint,
        auth: &Auth,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        let profiles = self
            .control_get(ep, auth, "/inference-profiles", http)
            .await?;
        let models = self
            .control_get(ep, auth, "/foundation-models", http)
            .await?;
        let pick = |v: &serde_json::Value, list: &str, id: &str, name: &str| -> Vec<ModelInfo> {
            v.get(list)
                .and_then(|l| l.as_array())
                .into_iter()
                .flatten()
                .filter_map(|m| {
                    Some(ModelInfo {
                        id: m.get(id)?.as_str()?.to_owned(),
                        display_name: m.get(name).and_then(|n| n.as_str()).map(str::to_owned),
                    })
                })
                .collect()
        };
        let mut out = pick(
            &profiles,
            "inferenceProfileSummaries",
            "inferenceProfileId",
            "inferenceProfileName",
        );
        for m in pick(&models, "modelSummaries", "modelId", "modelName") {
            if !out.iter().any(|o| o.id == m.id) {
                out.push(m);
            }
        }
        Ok(out)
    }
}

enum Auth {
    Bearer(Secret<String>),
    Sigv4 {
        access_key: String,
        secret_key: Secret<String>,
        token: Option<Secret<String>>,
    },
}

/// Send a built request with the API's guarded client; the status and the (bounded) body.
async fn send_built(http: &reqwest::Client, req: http::Request<Vec<u8>>) -> Result<(u16, Vec<u8>)> {
    let (parts, body) = req.into_parts();
    let mut rb = http.request(parts.method, parts.uri.to_string());
    for (k, v) in &parts.headers {
        rb = rb.header(k, v);
    }
    let resp = crate::http_util::send(rb.body(body)).await?;
    let status = resp.status().as_u16();
    let bytes = resp
        .bytes()
        .await
        .map_err(|_| LlmError::Unreachable("request failed".into()))?;
    Ok((
        status,
        bytes[..bytes.len().min(mint::MAX_RESPONSE_BYTES)].to_vec(),
    ))
}

fn static_models(ep: &Endpoint) -> Vec<ModelInfo> {
    ep.models
        .iter()
        .map(|id| ModelInfo {
            id: id.clone(),
            display_name: None,
        })
        .collect()
}

#[async_trait::async_trait]
impl LlmProvider for BedrockProvider {
    fn id(&self) -> &'static str {
        "bedrock"
    }
    fn display_name(&self) -> &'static str {
        "AWS Bedrock"
    }
    fn endpoint_kind(&self) -> EndpointKind {
        EndpointKind::Derived
    }
    fn auth_modes(&self) -> &'static [AuthMode] {
        &[
            AuthMode::AwsAssumeRole,
            AuthMode::AwsSigv4,
            AuthMode::ApiKey,
        ]
    }
    fn dialects(&self, ep: &Endpoint) -> Vec<Dialect> {
        if ep.dialects.is_empty() {
            vec![Dialect::AnthropicBedrock]
        } else {
            ep.dialects.clone()
        }
    }
    fn hosts(&self, ep: &Endpoint) -> Vec<String> {
        host_of(&self.base_url(ep)).into_iter().collect()
    }
    /// The VPC endpoint of the entry, else the regional runtime host (with `llm.dev_upstream_overrides` applied).
    fn base_url(&self, ep: &Endpoint) -> String {
        if ep.base_url.is_empty() {
            self.overrides.origin(&Self::pinned_origin(ep))
        } else {
            ep.base_url.trim_end_matches('/').to_owned()
        }
    }
    fn injection(&self, mode: &AuthMode) -> LlmInjection {
        let header = match mode {
            AuthMode::AwsAssumeRole => InjectionHeader::AwsAssumeRole,
            AuthMode::AwsSigv4 => InjectionHeader::AwsSigV4,
            _ => InjectionHeader::Bearer,
        };
        let mut i = LlmInjection::new(header, &["authorization", "x-api-key", "api-key"]);
        i.strip_request_prefixes.push("x-amz-");
        i
    }
    fn inference_path_permitted(&self, ep: &Endpoint, method: &Method, path: &str) -> bool {
        // No `allowed_paths`, ever (C-800 refuses them on `bedrock`).
        let d = self.dialects(ep);
        crate::paths::bedrock_path_permitted(
            method,
            path,
            d.contains(&Dialect::OpenaiChat) || d.contains(&Dialect::OpenaiResponses),
        )
    }
    fn model_from_request(
        &self,
        ep: &Endpoint,
        method: &Method,
        path: &str,
        body_prefix: &[u8],
        truncated: bool,
    ) -> ModelRef {
        // `/model/{modelId}/{op}`: the percent-decoded id (an inference-profile id or an ARN), compared exactly.
        let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
        if let ["model", id, _] = segs.as_slice() {
            return match percent_decode_str(id).decode_utf8() {
                Ok(m) if !m.is_empty() => ModelRef::Model(m.into_owned()),
                _ => ModelRef::Unknown,
            };
        }
        let _ = ep;
        crate::model_from_request(method, path, body_prefix, truncated)
    }
    async fn list_models(
        &self,
        ep: &Endpoint,
        _key: &Secret<String>,
        _http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        Ok(static_models(ep))
    }
    /// (C-810) The merged listing, signed by the API with the key (the session of an assumed role is computed here for
    /// the call and dropped); any failure falls back to the entry's static `models`.
    async fn list_models_as(
        &self,
        ep: &Endpoint,
        check: &KeyCheck,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        let listed = async {
            let auth = self.signer(ep, check, key, http).await?;
            self.listing(ep, &auth, http).await
        }
        .await;
        match listed {
            Ok(l) if !l.is_empty() => Ok(l),
            _ => Ok(static_models(ep)),
        }
    }
    async fn list_models_strict(
        &self,
        ep: &Endpoint,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        self.list_models_strict_as(ep, &KeyCheck::new(AuthMode::ApiKey), key, http)
            .await
    }
    /// Validation is each mode's cheapest live proof: `AssumeRole`, a signed `GetCallerIdentity`, or the listing (a
    /// Bedrock API key has no identity call).
    async fn list_models_strict_as(
        &self,
        ep: &Endpoint,
        check: &KeyCheck,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<Vec<ModelInfo>> {
        let proof = match check.mode {
            AuthMode::AwsAssumeRole => self.assume(ep, check, key, http).await.map(|_| ()),
            AuthMode::AwsSigv4 => self.caller(ep, key, http).await.map(|_| ()),
            _ => {
                let auth = Auth::Bearer(Secret::new(key.expose().clone()));
                self.control_get(ep, &auth, "/foundation-models", http)
                    .await
                    .map(|_| ())
            }
        };
        match proof {
            // A value that is not an AWS credential is a bad key, not an unverifiable origin.
            Err(LlmError::Unverifiable(_)) => Err(LlmError::InvalidKey),
            Err(e) => Err(e),
            Ok(()) => Ok(static_models(ep)),
        }
    }
    fn default_model(&self, ep: &Endpoint) -> Option<String> {
        ep.default_model.clone()
    }
    fn client_env(
        &self,
        ep: &Endpoint,
        dialect: Dialect,
        _mode: &AuthMode,
    ) -> Vec<(String, String)> {
        let base = self.base_url(ep);
        match dialect {
            Dialect::OpenaiChat | Dialect::OpenaiResponses => vec![
                ("OPENAI_BASE_URL".into(), format!("{base}/openai/v1")),
                ("OPENAI_API_KEY".into(), PLACEHOLDER.into()),
            ],
            _ => vec![
                ("CLAUDE_CODE_USE_BEDROCK".into(), "1".into()),
                ("AWS_REGION".into(), Self::region(ep).to_owned()),
                ("ANTHROPIC_BEDROCK_BASE_URL".into(), base),
                ("CLAUDE_CODE_SKIP_BEDROCK_AUTH".into(), "1".into()),
            ],
        }
    }

    fn origin_methods(&self) -> &'static [OriginMethod] {
        &[OriginMethod::Endpoint, OriginMethod::ProviderAccount]
    }
    async fn verify_key_origin(
        &self,
        ep: &Endpoint,
        method: OriginMethod,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<KeyOrigin> {
        self.verify_key_origin_as(ep, &KeyCheck::new(AuthMode::ApiKey), method, key, http)
            .await
    }
    async fn verify_key_origin_as(
        &self,
        ep: &Endpoint,
        check: &KeyCheck,
        method: OriginMethod,
        key: &Secret<String>,
        http: &reqwest::Client,
    ) -> Result<KeyOrigin> {
        match (method, check.mode) {
            (OriginMethod::Endpoint, _) => Ok(KeyOrigin::default()),
            // The assumed role's account (not the source key's) and the role ARN (C-804).
            (OriginMethod::ProviderAccount, AuthMode::AwsAssumeRole) => self
                .assume(ep, check, key, http)
                .await?
                .origin
                .ok_or_else(|| LlmError::Unverifiable("no_account".into())),
            (OriginMethod::ProviderAccount, AuthMode::AwsSigv4) => {
                let account = match self.caller(ep, key, http).await {
                    Err(LlmError::Unverifiable(_)) => return Err(LlmError::InvalidKey),
                    r => r?,
                };
                Ok(KeyOrigin {
                    organization_id: Some(account),
                    ..KeyOrigin::default()
                })
            }
            // A Bedrock API key says nothing about whose it is (C-804).
            (OriginMethod::ProviderAccount, _) => {
                Err(LlmError::Unverifiable("api_key_reports_no_account".into()))
            }
            (other, _) => Err(LlmError::UnsupportedMethod(other.as_str())),
        }
    }
    fn origin_from_response(&self, _ep: &Endpoint, _headers: &HeaderMap) -> Option<KeyOrigin> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(region: &str, base: &str) -> Endpoint {
        Endpoint {
            id: "b".into(),
            provider: "bedrock".into(),
            display_name: "Bedrock".into(),
            base_url: base.into(),
            dialects: vec![],
            default_model: None,
            models: vec!["m1".into()],
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
        }
    }

    #[test]
    fn host_follows_the_region_or_the_vpc_endpoint() {
        let p = BedrockProvider::default();
        assert_eq!(
            p.hosts(&ep("eu-west-1", "")),
            ["bedrock-runtime.eu-west-1.amazonaws.com"]
        );
        assert_eq!(
            p.hosts(&ep(
                "us-gov-west-1",
                "https://vpce-1.bedrock-runtime.us-gov-west-1.vpce.amazonaws.com"
            )),
            ["vpce-1.bedrock-runtime.us-gov-west-1.vpce.amazonaws.com"]
        );
    }

    #[test]
    fn the_model_is_the_percent_decoded_id() {
        let p = BedrockProvider::default();
        let e = ep("us-east-1", "");
        let m = |path: &str| p.model_from_request(&e, &Method::POST, path, b"", false);
        assert_eq!(
            m("/model/us.anthropic.claude-sonnet-4-v1%3A0/invoke"),
            ModelRef::Model("us.anthropic.claude-sonnet-4-v1:0".into())
        );
        assert_eq!(
            m("/model/arn%3Aaws%3Abedrock%3Aus-east-1%3A1%3Ainference-profile%2Fx/converse-stream"),
            ModelRef::Model("arn:aws:bedrock:us-east-1:1:inference-profile/x".into())
        );
    }

    #[test]
    fn env_has_no_credential_and_is_the_same_for_every_mode() {
        let p = BedrockProvider::default();
        let e = ep("us-east-1", "");
        let a = p.client_env(&e, Dialect::AnthropicBedrock, &AuthMode::AwsAssumeRole);
        assert_eq!(
            a,
            p.client_env(&e, Dialect::AnthropicBedrock, &AuthMode::ApiKey)
        );
        assert!(a.contains(&("CLAUDE_CODE_SKIP_BEDROCK_AUTH".into(), "1".into())));
        assert!(a.contains(&(
            "ANTHROPIC_BEDROCK_BASE_URL".into(),
            "https://bedrock-runtime.us-east-1.amazonaws.com".into()
        )));
        assert!(
            a.iter()
                .all(|(k, _)| !k.starts_with("AWS_SECRET") && !k.contains("TOKEN"))
        );
    }
}
