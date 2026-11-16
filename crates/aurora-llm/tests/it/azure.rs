#![allow(clippy::unwrap_used, clippy::expect_used)]
//! P4-LLM-3: the Entra mint against the verifying authority mock, and the `azure_openai` provider's validation and
//! origin for both credentials.

use aurora_core::Secret;
use aurora_llm::mint::{self, MintClass, MintCtx};
use aurora_llm::*;
use aurora_testkit::llm::azure::MockAzure;
use aurora_testkit::llm::entra::{Behaviour, MockEntra, TestApp};
use std::collections::BTreeMap;
use std::time::SystemTime;

const TENANT: &str = "11111111-2222-3333-4444-555555555555";
const CLIENT: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

fn app() -> TestApp {
    TestApp {
        tenant_id: TENANT.into(),
        client_id: CLIENT.into(),
        client_secret: "very-secret~value".into(),
    }
}

fn endpoint(base: &str) -> Endpoint {
    Endpoint {
        id: "az".into(),
        provider: "azure_openai".into(),
        display_name: "Azure".into(),
        base_url: base.into(),
        dialects: vec![],
        default_model: Some("gpt-4o".into()),
        models: vec!["gpt-4o".into()],
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

fn overrides(m: &MockEntra) -> Overrides {
    let mut t = BTreeMap::new();
    t.insert(
        "https://login.microsoftonline.com".to_owned(),
        m.base().to_owned(),
    );
    Overrides::new(&t)
}

#[tokio::test]
async fn the_mint_posts_the_verified_form_and_reports_tenant_and_application() {
    let (a, mock) = (app(), MockEntra::start(&app()).await);
    let (ep, o, v) = (endpoint("http://x"), overrides(&mock), app().json());
    let ctx = MintCtx {
        endpoint: &ep,
        value: v.as_bytes(),
        org_id: "org",
        org_external_id: None,
        now: SystemTime::now(),
        overrides: &o,
    };
    let m = mint::run(&reqwest::Client::new(), &mint::entra::EntraSpec, &ctx)
        .await
        .unwrap();
    assert!(mock.refused().is_empty(), "{:?}", mock.refused());
    assert_eq!(m.material.expose(), &mock.issued()[0]);
    assert!(m.expires_at > SystemTime::now());
    let origin = m.origin.unwrap();
    assert_eq!(origin.organization_id.as_deref(), Some(TENANT));
    assert_eq!(origin.user_id.as_deref(), Some(CLIENT));
    // A wrong client secret is refused by the authority (and counted), a refused mint is `Rejected`.
    let bad = TestApp {
        client_secret: "nope".into(),
        ..a
    }
    .json();
    let ctx = MintCtx {
        value: bad.as_bytes(),
        ..ctx
    };
    let e = mint::run(&reqwest::Client::new(), &mint::entra::EntraSpec, &ctx)
        .await
        .err()
        .unwrap();
    assert_eq!(e.class, MintClass::Rejected);
    assert_eq!(mock.refused(), ["client_secret"]);
}

#[tokio::test]
async fn the_mint_maps_refusals_and_outages() {
    let mock = MockEntra::start(&app()).await;
    let (ep, o, v) = (endpoint("http://x"), overrides(&mock), app().json());
    let ctx = MintCtx {
        endpoint: &ep,
        value: v.as_bytes(),
        org_id: "org",
        org_external_id: None,
        now: SystemTime::now(),
        overrides: &o,
    };
    let http = reqwest::Client::new();
    mock.set_behaviour(Behaviour::InvalidClient);
    let e = mint::run(&http, &mint::entra::EntraSpec, &ctx)
        .await
        .err()
        .unwrap();
    assert_eq!(e.class, MintClass::Rejected);
    assert_eq!(e.provider_code.as_deref(), Some("invalid_client"));
    assert!(!format!("{e:?}").contains("AADSTS"));
    mock.set_behaviour(Behaviour::Unavailable);
    let e = mint::run(&http, &mint::entra::EntraSpec, &ctx)
        .await
        .err()
        .unwrap();
    assert_eq!(e.reason(), "mint_unavailable");
    // A token for another tenant than the secret names is refused.
    mock.set_behaviour(Behaviour::Ok);
    mock.set_tid(Some("99999999-2222-3333-4444-555555555555"));
    let e = mint::run(&http, &mint::entra::EntraSpec, &ctx)
        .await
        .err()
        .unwrap();
    assert_eq!(e.class, MintClass::Rejected);
}

fn provider(m: &MockEntra) -> AzureOpenaiProvider {
    AzureOpenaiProvider::with_overrides(overrides(m))
}

#[tokio::test]
async fn resource_key_validation_uses_models_with_the_api_key_header() {
    let (azure, entra) = (
        MockAzure::start("good-key").await,
        MockEntra::start(&app()).await,
    );
    let (ep, p, http) = (
        endpoint(azure.base()),
        provider(&entra),
        reqwest::Client::new(),
    );
    let models = p
        .list_models_strict(&ep, &Secret::new("good-key".into()), &http)
        .await
        .unwrap();
    assert_eq!(models[0].id, "gpt-4o");
    let r = azure.requests();
    assert_eq!(r[0].path, "/openai/v1/models");
    assert_eq!(r[0].header("api-key"), Some("good-key"));
    assert!(r[0].header("authorization").is_none());
    assert!(matches!(
        p.list_models_strict(&ep, &Secret::new("bad".into()), &http)
            .await,
        Err(LlmError::InvalidKey)
    ));
    assert_eq!(entra.requests(), 0, "a key never touches the authority");
    // A key tells nothing about the account.
    assert!(matches!(
        p.verify_key_origin_as(
            &ep,
            &KeyCheck::new(AuthMode::ApiKey),
            OriginMethod::ProviderAccount,
            &Secret::new("good-key".into()),
            &http
        )
        .await,
        Err(LlmError::Unverifiable(_))
    ));
}

#[tokio::test]
async fn entra_validation_mints_then_calls_models_with_the_token() {
    let (azure, entra) = (
        MockAzure::start("good-key").await,
        MockEntra::start(&app()).await,
    );
    let (ep, p, http) = (
        endpoint(azure.base()),
        provider(&entra),
        reqwest::Client::new(),
    );
    let value = Secret::new(app().json());
    p.list_models_strict_as(&ep, &KeyCheck::new(AuthMode::AzureEntra), &value, &http)
        .await
        .unwrap();
    let r = azure.requests();
    assert_eq!(
        r[0].header("authorization"),
        Some(format!("Bearer {}", entra.issued()[0]).as_str())
    );
    assert!(r[0].header("api-key").is_none());
    let o = p
        .verify_key_origin_as(
            &ep,
            &KeyCheck::new(AuthMode::AzureEntra),
            OriginMethod::ProviderAccount,
            &value,
            &http,
        )
        .await
        .unwrap();
    assert_eq!(o.organization_id.as_deref(), Some(TENANT));
    assert_eq!(o.user_id.as_deref(), Some(CLIENT));
    // A refused application is `MintRejected`, an outage is unreachable.
    entra.set_behaviour(Behaviour::InvalidClient);
    assert!(matches!(
        p.list_models_strict_as(&ep, &KeyCheck::new(AuthMode::AzureEntra), &value, &http)
            .await,
        Err(LlmError::MintRejected)
    ));
    entra.set_behaviour(Behaviour::Unavailable);
    assert!(matches!(
        p.list_models_strict_as(&ep, &KeyCheck::new(AuthMode::AzureEntra), &value, &http)
            .await,
        Err(LlmError::Unreachable(_))
    ));
}
