#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::field_reassign_with_default
)]

use aurora_config::{KeyOrigin as CfgKeyOrigin, LlmEndpoint, LlmSection};
use aurora_core::Secret;
use aurora_llm::origin::complies;
use aurora_llm::*;
use http::{HeaderMap, HeaderValue, Method};
use wiremock::matchers::{header, header_exists, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-super-secret-key-123";

fn key() -> Secret<String> {
    Secret::new(KEY.to_string())
}

fn ep(provider: &str, base: &str) -> Endpoint {
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
    }
}

fn no_key(e: &LlmError) {
    assert!(!format!("{e:?}").contains(KEY));
    assert!(!format!("{e}").contains(KEY));
}

#[tokio::test]
async fn litellm_models_ok_with_bearer() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!(
            {"object":"list","data":[{"id":"a"},{"id":"b"}]})))
        .expect(1)
        .mount(&s)
        .await;
    let m = LitellmProvider
        .list_models(&ep("litellm", &s.uri()), &key(), &reqwest::Client::new())
        .await
        .unwrap();
    assert_eq!(
        m.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
}

#[tokio::test]
async fn litellm_401_invalid_key_even_with_static_models() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&s)
        .await;
    let mut e = ep("litellm", &s.uri());
    e.models = vec!["static".into()];
    let err = LitellmProvider
        .list_models(&e, &key(), &reqwest::Client::new())
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::InvalidKey));
    no_key(&err);
}

#[tokio::test]
async fn litellm_500_falls_back_to_static() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&s)
        .await;
    let mut e = ep("litellm", &s.uri());
    e.models = vec!["static".into()];
    let m = LitellmProvider
        .list_models(&e, &key(), &reqwest::Client::new())
        .await
        .unwrap();
    assert_eq!(m[0].id, "static");
    e.models.clear();
    let err = LitellmProvider
        .list_models(&e, &key(), &reqwest::Client::new())
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::Status(500)));
}

#[tokio::test]
async fn litellm_unreachable() {
    let e = ep("litellm", "http://127.0.0.1:1");
    let err = LitellmProvider
        .verify_key_origin(&e, OriginMethod::Endpoint, &key(), &reqwest::Client::new())
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::Unreachable(_)));
    no_key(&err);
}

#[tokio::test]
async fn litellm_endpoint_origin_is_empty() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data":[]})))
        .mount(&s)
        .await;
    let o = LitellmProvider
        .verify_key_origin(
            &ep("litellm", &s.uri()),
            OriginMethod::Endpoint,
            &key(),
            &reqwest::Client::new(),
        )
        .await
        .unwrap();
    assert_eq!(o, KeyOrigin::default());
}

#[tokio::test]
async fn litellm_key_info_team() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!(
            {"key":"x","info":{"team_id":"t1","user_id":"u1"}})))
        .mount(&s)
        .await;
    let e = ep("litellm", &s.uri());
    let o = LitellmProvider
        .verify_key_origin(
            &e,
            OriginMethod::LitellmTeam,
            &key(),
            &reqwest::Client::new(),
        )
        .await
        .unwrap();
    assert_eq!(o.team_id.as_deref(), Some("t1"));
    assert_eq!(o.user_id.as_deref(), Some("u1"));

    let s2 = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/key/info"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"info":{"team_id":null}})),
        )
        .mount(&s2)
        .await;
    let err = LitellmProvider
        .verify_key_origin(
            &ep("litellm", &s2.uri()),
            OriginMethod::LitellmTeam,
            &key(),
            &reqwest::Client::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::Unverifiable(_)));
}

#[test]
fn litellm_static_surface() {
    let e = ep("litellm", "https://llm.example.org:8443/");
    assert_eq!(LitellmProvider.hosts(&e), ["llm.example.org:8443"]);
    assert_eq!(LitellmProvider.dialects(&e).len(), 3);
    let inj = LitellmProvider.injection(&AuthMode::ApiKey);
    assert_eq!(inj.header, InjectionHeader::Bearer);
    assert_eq!(inj.strip_request_headers, ["x-api-key"]);
    let env = LitellmProvider.client_env(&e, Dialect::OpenaiChat, &AuthMode::ApiKey);
    assert!(env.contains(&(
        "OPENAI_BASE_URL".into(),
        "https://llm.example.org:8443/v1".into()
    )));
    assert!(
        env.iter()
            .any(|(k, v)| k == "OPENAI_API_KEY" && v == "aurora-placeholder")
    );
    assert!(
        LitellmProvider
            .origin_from_response(&e, &HeaderMap::new())
            .is_none()
    );
}

#[tokio::test]
async fn anthropic_models_paged_with_headers() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(query_param("after_id", "m2"))
        .and(header("x-api-key", KEY))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!(
            {"data":[{"id":"m3","display_name":"M3"}],"has_more":false,"last_id":"m3"})))
        .expect(1)
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("x-api-key", KEY))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!(
            {"data":[{"id":"m1"},{"id":"m2"}],"has_more":true,"last_id":"m2"})))
        .up_to_n_times(1)
        .expect(1)
        .mount(&s)
        .await;
    let p = AnthropicProvider::with_base_url(s.uri());
    let m = p
        .list_models(&ep("anthropic", &s.uri()), &key(), &reqwest::Client::new())
        .await
        .unwrap();
    assert_eq!(
        m.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["m1", "m2", "m3"]
    );
    assert_eq!(m[2].display_name.as_deref(), Some("M3"));
    let reqs = s.received_requests().await.unwrap();
    assert!(
        reqs.iter()
            .all(|r| !r.headers.contains_key("authorization"))
    );
}

#[tokio::test]
async fn anthropic_invalid_key() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&s)
        .await;
    let p = AnthropicProvider::with_base_url(s.uri());
    let err = p
        .list_models(&ep("anthropic", &s.uri()), &key(), &reqwest::Client::new())
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::InvalidKey));
}

#[tokio::test]
async fn anthropic_origin_from_headers() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(query_param("limit", "1"))
        .and(header_exists("x-api-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("anthropic-organization-id", "org-1")
                .insert_header("anthropic-workspace-id", "ws-1")
                .set_body_json(serde_json::json!({"data":[],"has_more":false})),
        )
        .mount(&s)
        .await;
    let p = AnthropicProvider::with_base_url(s.uri());
    let e = ep("anthropic", &s.uri());
    let o = p
        .verify_key_origin(
            &e,
            OriginMethod::ProviderAccount,
            &key(),
            &reqwest::Client::new(),
        )
        .await
        .unwrap();
    assert_eq!(o.organization_id.as_deref(), Some("org-1"));
    assert_eq!(o.workspace_id.as_deref(), Some("ws-1"));
    let err = p
        .verify_key_origin(&e, OriginMethod::Endpoint, &key(), &reqwest::Client::new())
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::UnsupportedMethod(_)));
}

#[tokio::test]
async fn anthropic_origin_missing_headers_unverifiable() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data":[]})))
        .mount(&s)
        .await;
    let p = AnthropicProvider::with_base_url(s.uri());
    let err = p
        .verify_key_origin(
            &ep("anthropic", &s.uri()),
            OriginMethod::ProviderAccount,
            &key(),
            &reqwest::Client::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::Unverifiable(_)));
    no_key(&err);
}

#[test]
fn anthropic_origin_from_response_and_surface() {
    let p = AnthropicProvider::default();
    let e = ep("anthropic", ANTHROPIC_BASE_URL);
    let mut h = HeaderMap::new();
    assert!(p.origin_from_response(&e, &h).is_none());
    h.insert(
        "anthropic-organization-id",
        HeaderValue::from_static("org-9"),
    );
    let o = p.origin_from_response(&e, &h).unwrap();
    assert_eq!(o.organization_id.as_deref(), Some("org-9"));
    assert_eq!(o.workspace_id, None);
    assert_eq!(p.hosts(&e), ["api.anthropic.com"]);
    let inj = p.injection(&AuthMode::ApiKey);
    assert_eq!(inj.header, InjectionHeader::XApiKey);
    assert_eq!(inj.strip_request_headers, ["authorization"]);
    assert_eq!(p.dialects(&e), [Dialect::AnthropicMessages]);
}

fn section(eps: Vec<LlmEndpoint>, enabled: &[&str]) -> LlmSection {
    LlmSection {
        enabled_providers: enabled.iter().map(|s| s.to_string()).collect(),
        provider_preference: vec![],
        endpoints: eps,
    }
}

fn cfg_ep(id: &str, provider: &str, base: Option<&str>) -> LlmEndpoint {
    LlmEndpoint {
        id: id.into(),
        provider: provider.into(),
        base_url: base.map(Into::into),
        ..Default::default()
    }
}

#[test]
fn registry_implicit_anthropic() {
    let r = ProviderRegistry::from_config(&section(
        vec![cfg_ep(
            "central",
            "litellm",
            Some("https://llm.example.org"),
        )],
        &["litellm", "anthropic"],
    ))
    .unwrap();
    assert_eq!(r.endpoints().len(), 2);
    let a = r.endpoint("anthropic").unwrap();
    assert!(a.key_origin.is_none());
    assert_eq!(a.base_url, "https://api.anthropic.com");
    assert_eq!(r.endpoint_hosts(), ["llm.example.org", "api.anthropic.com"]);
    assert!(r.is_endpoint_host("API.anthropic.com"));
    assert!(r.is_endpoint_host("llm.example.org:443"));
    assert!(!r.is_endpoint_host("example.org"));
}

#[test]
fn registry_explicit_anthropic_and_origin_rule() {
    let mut e = cfg_ep("anthropic", "anthropic", None);
    e.key_origin = Some(CfgKeyOrigin {
        method: "provider_account".into(),
        allowed_organization_ids: vec!["o".into()],
        ..Default::default()
    });
    let r = ProviderRegistry::from_config(&section(vec![e], &["anthropic"])).unwrap();
    assert_eq!(r.endpoints().len(), 1);
    assert_eq!(
        r.endpoint("anthropic")
            .unwrap()
            .key_origin
            .as_ref()
            .unwrap()
            .method,
        OriginMethod::ProviderAccount
    );
}

#[test]
fn registry_errors_name_endpoint() {
    let err = |c: LlmSection| ProviderRegistry::from_config(&c).err().unwrap().to_string();
    assert!(
        err(section(
            vec![cfg_ep("weird", "openai", Some("https://x.test"))],
            &["litellm"]
        ))
        .contains("weird")
    );
    assert!(
        err(section(
            vec![cfg_ep("lite", "litellm", Some("https://x.test"))],
            &["anthropic"]
        ))
        .contains("lite")
    );
    assert!(
        err(section(
            vec![cfg_ep("nobase", "litellm", None)],
            &["litellm"]
        ))
        .contains("nobase")
    );
    assert!(
        err(section(
            vec![cfg_ep("a", "anthropic", Some("https://evil.test"))],
            &["anthropic"]
        ))
        .contains("`a`")
    );
    let mut bad = cfg_ep("m", "litellm", Some("https://x.test"));
    bad.key_origin = Some(CfgKeyOrigin {
        method: "provider_account".into(),
        ..Default::default()
    });
    assert!(err(section(vec![bad], &["litellm"])).contains("`m`"));
}

#[test]
fn governed_hosts_union() {
    let mut raw = aurora_config::RawConfig::default();
    raw.llm = section(
        vec![cfg_ep(
            "central",
            "litellm",
            Some("https://llm.example.org"),
        )],
        &["litellm"],
    );
    raw.governance.llm.known_llm_hosts =
        Some(vec!["api.openai.com".into(), "LLM.example.org".into()]);
    let r = ProviderRegistry::from_config(&raw.llm).unwrap();
    let h = governed_hosts(&r, &raw);
    assert_eq!(h, ["llm.example.org", "api.openai.com"]);
}

#[test]
fn model_from_request_table() {
    let post = Method::POST;
    let cases: Vec<(Method, &str, &[u8], bool, ModelRef)> = vec![
        (
            post.clone(),
            "/v1/messages",
            br#"{"model":"claude-x","messages":[]}"#,
            false,
            ModelRef::Model("claude-x".into()),
        ),
        (
            post.clone(),
            "/v1/messages/count_tokens",
            br#"{"model":"c"}"#,
            false,
            ModelRef::Model("c".into()),
        ),
        (
            post.clone(),
            "/litellm/v1/chat/completions",
            br#"{"model":"gpt"}"#,
            false,
            ModelRef::Model("gpt".into()),
        ),
        (
            post.clone(),
            "/v1/responses?x=1",
            br#"{"model":"r"}"#,
            false,
            ModelRef::Model("r".into()),
        ),
        (
            post.clone(),
            "/v1/embeddings/",
            br#"{"model":"e"}"#,
            false,
            ModelRef::Model("e".into()),
        ),
        (
            post.clone(),
            "/v1/messages",
            br#"{"model":"claude-x","mess"#,
            true,
            ModelRef::Unknown,
        ),
        (
            post.clone(),
            "/v1/messages",
            b"not json",
            false,
            ModelRef::Unknown,
        ),
        (
            post.clone(),
            "/v1/messages",
            br#"{"messages":[]}"#,
            false,
            ModelRef::Unknown,
        ),
        (
            post.clone(),
            "/v1/messages",
            br#"{"model":5}"#,
            false,
            ModelRef::Unknown,
        ),
        (Method::GET, "/v1/models", b"", false, ModelRef::NoModel),
        (
            post.clone(),
            "/v1/files",
            br#"{"model":"x"}"#,
            false,
            ModelRef::NoModel,
        ),
        (Method::GET, "/v1/messages", b"", false, ModelRef::NoModel),
    ];
    for (m, p, b, t, want) in cases {
        assert_eq!(model_from_request(&m, p, b, t), want, "{m} {p}");
    }
    let e = ep("litellm", "https://x.test");
    assert_eq!(
        LitellmProvider.model_from_request(
            &e,
            &Method::POST,
            "/v1/messages",
            br#"{"model":"a"}"#,
            false
        ),
        ModelRef::Model("a".into())
    );
}

#[test]
fn complies_table() {
    let rule = |m, o: &[&str], w: &[&str], t: &[&str]| KeyOriginRule {
        method: m,
        allowed_organization_ids: o.iter().map(|s| s.to_string()).collect(),
        allowed_workspace_ids: w.iter().map(|s| s.to_string()).collect(),
        allowed_team_ids: t.iter().map(|s| s.to_string()).collect(),
    };
    let o = |org: Option<&str>, ws: Option<&str>, team: Option<&str>| KeyOrigin {
        organization_id: org.map(Into::into),
        workspace_id: ws.map(Into::into),
        team_id: team.map(Into::into),
        user_id: None,
    };
    use OriginMethod::*;
    assert!(complies(
        &KeyOrigin::default(),
        &rule(Endpoint, &["x"], &[], &[])
    ));
    assert!(complies(
        &KeyOrigin::default(),
        &rule(ProviderAccount, &[], &[], &[])
    ));
    assert!(complies(
        &o(Some("a"), None, None),
        &rule(ProviderAccount, &["a", "b"], &[], &[])
    ));
    assert!(!complies(
        &o(Some("c"), None, None),
        &rule(ProviderAccount, &["a", "b"], &[], &[])
    ));
    assert!(!complies(
        &o(None, None, None),
        &rule(ProviderAccount, &["a"], &[], &[])
    ));
    assert!(!complies(
        &o(Some("a"), Some("w2"), None),
        &rule(ProviderAccount, &["a"], &["w1"], &[])
    ));
    assert!(complies(
        &o(Some("a"), Some("w1"), None),
        &rule(ProviderAccount, &["a"], &["w1"], &[])
    ));
    assert!(complies(
        &o(None, None, Some("t")),
        &rule(LitellmTeam, &[], &[], &["t"])
    ));
    assert!(!complies(
        &o(None, None, Some("u")),
        &rule(LitellmTeam, &[], &[], &["t"])
    ));
}

#[test]
fn endpoint_config_fields_from_08_example_are_resolved() {
    let toml = r#"
[llm]
enabled_providers = ["litellm", "anthropic"]
[[llm.endpoints]]
id = "central"
provider = "litellm"
display_name = "Central LiteLLM"
base_url = "https://llm.example.org"
dialects = ["openai_chat", "anthropic_messages"]
default_model = "claude-sonnet-5-5"
models = ["m1"]
key_origin = { method = "endpoint" }
"#;
    let dir = std::env::temp_dir().join(format!("aurora-llm-cfg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("aurora.toml");
    std::fs::write(&path, toml).unwrap();
    let raw = aurora_config::load(Some(&path), std::iter::empty()).unwrap();
    let reg = aurora_llm::ProviderRegistry::from_config(&raw.llm).unwrap();
    let ep = reg.endpoint("central").unwrap();
    assert_eq!(ep.display_name, "Central LiteLLM");
    assert_eq!(ep.dialects.len(), 2);
    assert_eq!(ep.default_model.as_deref(), Some("claude-sonnet-5-5"));
    assert_eq!(ep.models, vec!["m1".to_string()]);
}
