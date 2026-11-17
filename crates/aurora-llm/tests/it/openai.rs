#![allow(clippy::unwrap_used, clippy::expect_used)]
//! P4-LLM-1: the `openai` and `openai_compatible` providers, `client_env`, injection recipes, and the registry.

use aurora_config::{LlmEndpoint, LlmSection};
use aurora_core::Secret;
use aurora_llm::*;
use http::{HeaderMap, HeaderValue, Method};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-proj-very-secret-0123456789";

fn key() -> Secret<String> {
    Secret::new(KEY.to_string())
}

fn endpoint(provider: &str, base: &str) -> Endpoint {
    Endpoint {
        id: "e".into(),
        provider: provider.into(),
        display_name: "E".into(),
        base_url: base.into(),
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
        model_roles: Default::default(),
        model_info: Default::default(),
    }
}

#[tokio::test]
async fn openai_lists_models_with_bearer_org_and_project_and_reports_the_account() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .and(header("openai-organization", "org-acme"))
        .and(header("openai-project", "proj_1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("openai-organization", "org-acme")
                .insert_header("openai-project", "proj_1")
                .set_body_json(serde_json::json!({"object": "list", "data": [{"id": "gpt-5"}, {"id": "gpt-5-mini"}]})),
        )
        .expect(2)
        .mount(&s)
        .await;
    let p = OpenaiProvider::with_base_url(s.uri());
    let mut e = endpoint("openai", &s.uri());
    e.organization = Some("org-acme".into());
    e.project = Some("proj_1".into());
    let http = reqwest::Client::new();
    let m = p.list_models_strict(&e, &key(), &http).await.unwrap();
    assert_eq!(
        m.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["gpt-5", "gpt-5-mini"]
    );
    let o = p
        .verify_key_origin(&e, OriginMethod::ProviderAccount, &key(), &http)
        .await
        .unwrap();
    assert_eq!(o.organization_id.as_deref(), Some("org-acme"));
    assert_eq!(o.workspace_id.as_deref(), Some("proj_1"));
    assert!(matches!(
        p.verify_key_origin(&e, OriginMethod::Endpoint, &key(), &http)
            .await,
        Err(LlmError::UnsupportedMethod(_))
    ));
}

#[tokio::test]
async fn openai_401_is_an_invalid_key_even_with_a_static_list_and_other_failures_fall_back() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&s)
        .await;
    let p = OpenaiProvider::with_base_url(s.uri());
    let mut e = endpoint("openai", &s.uri());
    e.models = vec!["gpt-5".into()];
    assert!(matches!(
        p.list_models(&e, &key(), &reqwest::Client::new()).await,
        Err(LlmError::InvalidKey)
    ));
    let s2 = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&s2)
        .await;
    let p2 = OpenaiProvider::with_base_url(s2.uri());
    let m = p2
        .list_models(&e, &key(), &reqwest::Client::new())
        .await
        .unwrap();
    assert_eq!(m[0].id, "gpt-5");
    // strict listing never falls back
    assert!(matches!(
        p2.list_models_strict(&e, &key(), &reqwest::Client::new())
            .await,
        Err(LlmError::Status(503))
    ));
}

#[test]
fn openai_origin_backstop_reads_the_response_headers() {
    let p = OpenaiProvider::default();
    let e = endpoint("openai", OPENAI_BASE_URL);
    let mut h = HeaderMap::new();
    assert!(p.origin_from_response(&e, &h).is_none());
    h.insert("openai-organization", HeaderValue::from_static("org-x"));
    let o = p.origin_from_response(&e, &h).unwrap();
    assert_eq!(
        (o.organization_id.as_deref(), o.workspace_id.as_deref()),
        (Some("org-x"), None)
    );
}

#[test]
fn openai_recipe_strips_agent_headers_and_sets_the_configured_ones() {
    let p = OpenaiProvider::default();
    let mut e = endpoint("openai", OPENAI_BASE_URL);
    let bare = p.injection_for(&e, &AuthMode::ApiKey);
    assert_eq!(bare.header, InjectionHeader::Bearer);
    for h in [
        "authorization",
        "x-api-key",
        "api-key",
        "openai-organization",
        "openai-project",
    ] {
        assert!(bare.strip_request_headers.contains(&h), "{h}");
    }
    assert!(bare.set_headers.is_empty());
    e.organization = Some("org-acme".into());
    e.project = Some("proj_1".into());
    let i = p.injection_for(&e, &AuthMode::ApiKey);
    assert_eq!(
        i.set_headers,
        [
            ("openai-organization", "org-acme".to_string()),
            ("openai-project", "proj_1".to_string())
        ]
    );
}

#[test]
fn openai_surface_and_client_env_carry_no_key() {
    let p = OpenaiProvider::default();
    let e = endpoint("openai", OPENAI_BASE_URL);
    assert_eq!(p.hosts(&e), ["api.openai.com"]);
    assert_eq!(
        p.dialects(&e),
        [Dialect::OpenaiChat, Dialect::OpenaiResponses]
    );
    assert_eq!(p.auth_modes(), &[AuthMode::ApiKey]);
    let env = p.client_env(&e, Dialect::OpenaiResponses, &AuthMode::ApiKey);
    assert_eq!(
        env,
        [
            (
                "OPENAI_BASE_URL".to_string(),
                "https://api.openai.com/v1".to_string()
            ),
            (
                "OPENAI_API_KEY".to_string(),
                "aurora-placeholder".to_string()
            )
        ]
    );
    assert!(p.inference_path_permitted(&e, &Method::POST, "/v1/responses"));
    assert!(!p.inference_path_permitted(&e, &Method::POST, "/v1/files"));
}

#[tokio::test]
async fn compatible_lists_models_with_the_configured_header_and_path() {
    for (auth_header, hname, value) in [
        ("authorization", "authorization", format!("Bearer {KEY}")),
        ("api-key", "api-key", KEY.to_string()),
        ("x-api-key", "x-api-key", KEY.to_string()),
    ] {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/models"))
            .and(header(hname, value.as_str()))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"data": [{"id": "llama"}]})),
            )
            .expect(2) // the listing and the origin check
            .mount(&s)
            .await;
        let mut e = endpoint("openai_compatible", &s.uri());
        e.auth_header = Some(auth_header.into());
        e.models_path = Some("/api/models".into());
        let m = OpenaiCompatibleProvider
            .list_models_strict(&e, &key(), &reqwest::Client::new())
            .await
            .unwrap();
        assert_eq!(m[0].id, "llama", "{auth_header}");
        let origin = OpenaiCompatibleProvider
            .verify_key_origin(&e, OriginMethod::Endpoint, &key(), &reqwest::Client::new())
            .await
            .unwrap();
        assert_eq!(origin, KeyOrigin::default());
    }
}

#[tokio::test]
async fn compatible_default_models_path_and_rejected_key() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&s)
        .await;
    let e = endpoint("openai_compatible", &s.uri());
    assert!(matches!(
        OpenaiCompatibleProvider
            .list_models_strict(&e, &key(), &reqwest::Client::new())
            .await,
        Err(LlmError::InvalidKey)
    ));
}

#[test]
fn compatible_recipes_client_env_and_paths() {
    let mut e = endpoint("openai_compatible", "https://llm.example.org/gw/");
    let p = OpenaiCompatibleProvider;
    assert_eq!(p.dialects(&e), [Dialect::OpenaiChat]);
    assert_eq!(
        p.injection_for(&e, &AuthMode::ApiKey).header,
        InjectionHeader::Bearer
    );
    e.auth_header = Some("api-key".into());
    assert_eq!(
        p.injection_for(&e, &AuthMode::ApiKey).header,
        InjectionHeader::Header("api-key")
    );
    e.auth_header = Some("x-api-key".into());
    assert_eq!(
        p.injection_for(&e, &AuthMode::ApiKey).header,
        InjectionHeader::XApiKey
    );
    for h in ["authorization", "x-api-key", "api-key"] {
        assert!(
            p.injection_for(&e, &AuthMode::ApiKey)
                .strip_request_headers
                .contains(&h)
        );
    }
    assert_eq!(
        p.client_env(&e, Dialect::OpenaiChat, &AuthMode::ApiKey)[0],
        (
            "OPENAI_BASE_URL".to_string(),
            "https://llm.example.org/gw/v1".to_string()
        )
    );
    let a = p.client_env(&e, Dialect::AnthropicMessages, &AuthMode::ApiKey);
    assert_eq!(
        a[0],
        (
            "ANTHROPIC_BASE_URL".to_string(),
            "https://llm.example.org/gw".to_string()
        )
    );
    assert_eq!(
        a[1],
        (
            "ANTHROPIC_AUTH_TOKEN".to_string(),
            "aurora-placeholder".to_string()
        )
    );
    // the prefix is part of the path set; the other dialects' routes are not permitted until declared
    assert!(p.inference_path_permitted(&e, &Method::POST, "/gw/v1/chat/completions"));
    assert!(!p.inference_path_permitted(&e, &Method::POST, "/v1/chat/completions"));
    assert!(!p.inference_path_permitted(&e, &Method::POST, "/gw/v1/messages"));
    e.dialects = vec![Dialect::AnthropicMessages];
    assert!(p.inference_path_permitted(&e, &Method::POST, "/gw/v1/messages"));
    e.allowed_paths = vec!["/*".into()];
    assert!(p.inference_path_permitted(&e, &Method::POST, "/gw/anything"));
}

#[test]
fn no_client_env_value_is_a_key() {
    let all = [
        (OpenaiProvider::default().client_env(
            &endpoint("openai", OPENAI_BASE_URL),
            Dialect::OpenaiChat,
            &AuthMode::ApiKey,
        )),
        OpenaiCompatibleProvider.client_env(
            &endpoint("openai_compatible", "https://x.example"),
            Dialect::OpenaiChat,
            &AuthMode::ApiKey,
        ),
        OpenaiCompatibleProvider.client_env(
            &endpoint("openai_compatible", "https://x.example"),
            Dialect::AnthropicMessages,
            &AuthMode::ApiKey,
        ),
        AnthropicProvider::default().client_env(
            &endpoint("anthropic", ANTHROPIC_BASE_URL),
            Dialect::AnthropicMessages,
            &AuthMode::ApiKey,
        ),
    ];
    for env in all {
        for (k, v) in env {
            let credential_name = k.contains("KEY") || k.contains("TOKEN");
            assert!(!credential_name || v == "aurora-placeholder", "{k}={v}");
            assert!(!v.starts_with("sk-") && !v.contains(KEY), "{k}");
        }
    }
}

fn section(eps: Vec<LlmEndpoint>, enabled: &[&str]) -> LlmSection {
    LlmSection {
        enabled_providers: enabled.iter().map(|s| (*s).to_string()).collect(),
        endpoints: eps,
        ..LlmSection::default()
    }
}

#[test]
fn registry_resolves_the_new_providers_and_applies_path_sets_per_provider() {
    let reg = ProviderRegistry::from_config(&section(
        vec![
            LlmEndpoint {
                id: "oai".into(),
                provider: "openai".into(),
                organization: Some("org-a".into()),
                ..LlmEndpoint::default()
            },
            LlmEndpoint {
                id: "vllm".into(),
                provider: "openai_compatible".into(),
                base_url: Some("http://vllm.svc:8000".into()),
                ..LlmEndpoint::default()
            },
        ],
        &["litellm", "anthropic", "openai", "openai_compatible"],
    ))
    .unwrap();
    assert_eq!(
        reg.endpoint("oai").unwrap().base_url,
        "https://api.openai.com"
    );
    assert_eq!(
        reg.endpoint("oai").unwrap().organization.as_deref(),
        Some("org-a")
    );
    assert!(reg.endpoint_hosts().contains(&"api.openai.com".to_string()));
    assert!(reg.endpoint_hosts().contains(&"vllm.svc:8000".to_string()));
    assert!(reg.path_permitted("oai", &Method::POST, "/v1/chat/completions"));
    assert!(!reg.path_permitted("oai", &Method::POST, "/v1/files"));
    assert!(reg.path_permitted("vllm", &Method::POST, "/v1/chat/completions"));
    assert!(!reg.path_permitted("vllm", &Method::POST, "/v1/responses"));
    // (D-260(c), D-268(d)) the implicit anthropic endpoint now has its inference set
    assert!(reg.path_permitted("anthropic", &Method::POST, "/v1/messages"));
    assert!(reg.path_permitted("anthropic", &Method::GET, "/v1/models"));
    assert!(!reg.path_permitted("anthropic", &Method::POST, "/v1/files"));
    assert!(!reg.path_permitted("anthropic", &Method::POST, "/v1/messages/batches"));
    // a fixed-host provider refuses a base_url
    assert!(
        ProviderRegistry::from_config(&section(
            vec![LlmEndpoint {
                id: "oai".into(),
                provider: "openai".into(),
                base_url: Some("https://x.example".into()),
                ..LlmEndpoint::default()
            }],
            &["openai"]
        ))
        .is_err()
    );
}

#[test]
fn anthropic_allowed_paths_admit_what_the_operator_lists() {
    let reg = ProviderRegistry::from_config(&section(
        vec![LlmEndpoint {
            id: "anthropic".into(),
            provider: "anthropic".into(),
            allowed_paths: vec!["/v1/files".into()],
            ..LlmEndpoint::default()
        }],
        &["anthropic"],
    ))
    .unwrap();
    assert!(reg.path_permitted("anthropic", &Method::GET, "/v1/files"));
    assert!(!reg.path_permitted("anthropic", &Method::GET, "/v1/files/f1"));
}

#[test]
fn dev_upstream_overrides_replace_the_fixed_origin() {
    let mut cfg = section(
        vec![LlmEndpoint {
            id: "oai".into(),
            provider: "openai".into(),
            ..LlmEndpoint::default()
        }],
        &["anthropic", "openai"],
    );
    cfg.dev_upstream_overrides.insert(
        "https://api.openai.com".into(),
        "http://127.0.0.1:9/".into(),
    );
    cfg.dev_upstream_overrides.insert(
        "https://api.anthropic.com".into(),
        "http://127.0.0.1:10".into(),
    );
    let reg = ProviderRegistry::from_config(&cfg).unwrap();
    assert_eq!(reg.endpoint_hosts(), ["127.0.0.1:9", "127.0.0.1:10"]);
    assert!(reg.accepts_injection(Some("oai"), None, false, "127.0.0.1", 9));
    assert!(!reg.accepts_injection(Some("oai"), None, true, "api.openai.com", 443));
}

#[test]
fn vocabulary_round_trips() {
    for m in [
        AuthMode::ApiKey,
        AuthMode::AwsSigv4,
        AuthMode::AwsAssumeRole,
        AuthMode::GcpServiceAccount,
        AuthMode::AzureEntra,
    ] {
        assert_eq!(AuthMode::parse(m.as_str()), Some(m));
    }
    assert_eq!(AuthMode::ApiKey.value_shape(), "string");
    assert_eq!(AuthMode::AwsAssumeRole.value_shape(), "aws_assume_role");
    assert_eq!(AuthMode::parse("password"), None);
    for d in [Dialect::AnthropicBedrock, Dialect::AnthropicVertex] {
        assert_eq!(Dialect::parse(d.as_str()), Some(d));
    }
    assert!(InjectionHeader::Bearer.is_plain() && InjectionHeader::Header("api-key").is_plain());
    assert!(!InjectionHeader::AwsSigV4.is_plain() && !InjectionHeader::GcpToken.is_plain());
}
