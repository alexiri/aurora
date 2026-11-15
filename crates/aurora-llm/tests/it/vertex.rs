#![allow(clippy::unwrap_used, clippy::expect_used)]
//! P4-LLM-2: the Google mint (`mint::gcp`), the `vertex` provider's validation and origin, against the verifying mock.

use aurora_core::Secret;
use aurora_llm::mint::{self, MintClass, MintCtx, MintSpec};
use aurora_llm::*;
use aurora_testkit::llm::google_token::{Behaviour, MockGoogleToken, TestKey};
use http::HeaderMap;
use std::collections::BTreeMap;
use std::time::SystemTime;

fn endpoint() -> Endpoint {
    Endpoint {
        id: "v".into(),
        provider: "vertex".into(),
        display_name: "Vertex".into(),
        base_url: String::new(),
        dialects: vec![],
        default_model: None,
        models: vec!["claude-x".into()],
        allowed_models: vec![],
        key_origin: None,
        allowed_paths: vec![],
        organization: None,
        project: None,
        auth_header: None,
        models_path: None,
        region: None,
        project_id: Some("inference-proj".into()),
        location: Some("us-east5".into()),
    }
}

fn overrides(m: &MockGoogleToken) -> Overrides {
    let mut t = BTreeMap::new();
    t.insert(
        "https://oauth2.googleapis.com".to_owned(),
        m.base().to_owned(),
    );
    Overrides::new(&t)
}

fn ctx<'a>(ep: &'a Endpoint, value: &'a [u8], o: &'a Overrides) -> MintCtx<'a> {
    MintCtx {
        endpoint: ep,
        value,
        org_id: "org",
        org_external_id: None,
        now: SystemTime::now(),
        overrides: o,
    }
}

#[tokio::test]
async fn mint_signs_a_verifiable_assertion_and_reports_the_account() {
    let key = TestKey::generate("key-proj", "sa@key-proj.iam.gserviceaccount.com");
    let mock = MockGoogleToken::start(&key).await;
    let (ep, o, json) = (endpoint(), overrides(&mock), key.json());
    let m = mint::run(
        &reqwest::Client::new(),
        &mint::gcp::GcpSpec,
        &ctx(&ep, json.as_bytes(), &o),
    )
    .await
    .unwrap();
    assert!(mock.refused().is_empty(), "{:?}", mock.refused());
    assert_eq!(m.material.expose(), "ya29.mock-token-1");
    assert!(m.expires_at > SystemTime::now());
    let origin = m.origin.unwrap();
    assert_eq!(origin.workspace_id.as_deref(), Some("key-proj"));
    assert_eq!(
        origin.user_id.as_deref(),
        Some("sa@key-proj.iam.gserviceaccount.com")
    );
}

#[tokio::test]
async fn mint_maps_failures_to_classes_without_the_providers_message() {
    let key = TestKey::generate("p", "sa@p.iam.gserviceaccount.com");
    let mock = MockGoogleToken::start(&key).await;
    let (ep, o, json) = (endpoint(), overrides(&mock), key.json());
    let http = reqwest::Client::new();
    mock.set_behaviour(Behaviour::InvalidGrant);
    let e = mint::run(&http, &mint::gcp::GcpSpec, &ctx(&ep, json.as_bytes(), &o))
        .await
        .err()
        .unwrap();
    assert_eq!(e.class, MintClass::Rejected);
    assert_eq!(e.provider_code.as_deref(), Some("invalid_grant"));
    assert!(!format!("{e:?}").contains("Invalid JWT"));
    mock.set_behaviour(Behaviour::Unavailable);
    let e = mint::run(&http, &mint::gcp::GcpSpec, &ctx(&ep, json.as_bytes(), &o))
        .await
        .err()
        .unwrap();
    assert_eq!(e.class, MintClass::Unavailable);
    assert_eq!(e.reason(), "mint_unavailable");
    // A signature made with another key is refused by the mock (and counted).
    let other = TestKey::generate("p", "sa@p.iam.gserviceaccount.com");
    mock.set_behaviour(Behaviour::Ok);
    let e = mint::run(
        &http,
        &mint::gcp::GcpSpec,
        &ctx(&ep, other.json().as_bytes(), &o),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(e.class, MintClass::Rejected);
    assert_eq!(mock.refused(), ["signature"]);
}

#[test]
fn parser_maps_statuses() {
    let key = TestKey::generate("p", "sa@p.iam.gserviceaccount.com");
    let (ep, json) = (endpoint(), key.json());
    let o = Overrides::default();
    let c = ctx(&ep, json.as_bytes(), &o);
    let parse = |s: u16, b: &str| mint::gcp::GcpSpec.parse(&c, s, &HeaderMap::new(), b.as_bytes());
    for s in [400, 401, 403] {
        assert_eq!(parse(s, "{}").err().unwrap().class, MintClass::Rejected);
    }
    for s in [429, 500, 502, 503, 504] {
        assert_eq!(parse(s, "{}").err().unwrap().class, MintClass::Unavailable);
    }
    // A malformed 200 says nothing about the key.
    assert_eq!(
        parse(200, "{}").err().unwrap().class,
        MintClass::Unavailable
    );
    assert_eq!(
        parse(200, "nope").err().unwrap().class,
        MintClass::Unavailable
    );
    let e = parse(
        400,
        r#"{"error":"a long description with spaces and key material"}"#,
    )
    .err()
    .unwrap();
    assert_eq!(e.provider_code, None);
}

#[test]
fn value_checks_name_the_c801_reason() {
    let key = TestKey::generate("p", "sa@p.iam.gserviceaccount.com");
    let reason = |v: &str| mint::gcp::parse_value(v.as_bytes()).err().map(|e| e.reason);
    assert!(mint::gcp::parse_value(key.json().as_bytes()).is_ok());
    assert_eq!(
        reason(&key.json_with("service_account", "https://evil.example/token")),
        Some("token_uri_not_google")
    );
    assert_eq!(
        reason(&key.json_with("authorized_user", mint::gcp::TOKEN_URI)),
        Some("not_service_account")
    );
    assert_eq!(reason("not json"), Some("not_service_account"));
    assert_eq!(reason("[]"), Some("not_service_account"));
    let mut v: serde_json::Value = serde_json::from_str(&key.json()).unwrap();
    v["private_key"] = "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----".into();
    assert_eq!(reason(&v.to_string()), Some("missing_field"));
    v = serde_json::from_str(&key.json()).unwrap();
    v["region"] = "us".into();
    assert_eq!(reason(&v.to_string()), Some("unknown_field"));
    v = serde_json::from_str(&key.json()).unwrap();
    v.as_object_mut().unwrap().remove("client_email");
    assert_eq!(reason(&v.to_string()), Some("missing_field"));
}

#[tokio::test]
async fn provider_validates_by_minting_and_proves_the_project_and_email() {
    let key = TestKey::generate("key-proj", "sa@key-proj.iam.gserviceaccount.com");
    let mock = MockGoogleToken::start(&key).await;
    let p = VertexProvider::with_overrides(overrides(&mock));
    let (ep, http) = (endpoint(), reqwest::Client::new());
    let secret = Secret::new(key.json());
    let models = p.list_models_strict(&ep, &secret, &http).await.unwrap();
    assert_eq!(models[0].id, "claude-x");
    let o = p
        .verify_key_origin(&ep, OriginMethod::ProviderAccount, &secret, &http)
        .await
        .unwrap();
    assert_eq!(o.workspace_id.as_deref(), Some("key-proj"));
    assert_eq!(
        o.user_id.as_deref(),
        Some("sa@key-proj.iam.gserviceaccount.com")
    );
    mock.set_behaviour(Behaviour::InvalidGrant);
    assert!(matches!(
        p.list_models_strict(&ep, &secret, &http).await,
        Err(LlmError::InvalidKey)
    ));
    mock.set_behaviour(Behaviour::Unavailable);
    assert!(matches!(
        p.list_models_strict(&ep, &secret, &http).await,
        Err(LlmError::Unreachable(_))
    ));
}
