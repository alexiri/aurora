#![allow(clippy::unwrap_used, clippy::expect_used)]
//! P4-LLM-4: the STS `AssumeRole` mint and the `bedrock` provider's validation, origin and listing against the
//! SigV4-**verifying** STS and Bedrock mocks.

use aurora_core::Secret;
use aurora_llm::*;
use aurora_testkit::llm::bedrock::MockBedrock;
use aurora_testkit::llm::sts::MockSts;
use std::collections::BTreeMap;

const SRC_KEY: &str = "AKIASOURCEKEY00001";
const SRC_SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
const ROLE_ACCOUNT: &str = "210987654321";
const ROLE: &str = "arn:aws:iam::210987654321:role/aurora-bedrock";

fn endpoint(base: &str) -> Endpoint {
    Endpoint {
        id: "b".into(),
        provider: "bedrock".into(),
        display_name: "Bedrock".into(),
        base_url: base.into(),
        dialects: vec![],
        default_model: None,
        models: vec!["static-model".into()],
        allowed_models: vec![],
        key_origin: None,
        allowed_paths: vec![],
        organization: None,
        project: None,
        auth_header: None,
        models_path: None,
        region: Some("us-east-1".into()),
        project_id: None,
        location: None,
        model_roles: Default::default(),
        model_info: Default::default(),
    }
}

struct Rig {
    sts: MockSts,
    bedrock: MockBedrock,
    p: BedrockProvider,
    ep: Endpoint,
    http: reqwest::Client,
}

async fn rig() -> Rig {
    let sts = MockSts::start(SRC_KEY, SRC_SECRET, ROLE_ACCOUNT).await;
    let bedrock = MockBedrock::start().await;
    let b = bedrock.clone();
    // Every session STS hands out is a key the Bedrock mock accepts (with its token).
    sts.on_session(move |ak, sk, tok| b.accept_key(ak, sk, Some(tok)));
    let mut t = BTreeMap::new();
    t.insert(
        "https://sts.us-east-1.amazonaws.com".to_owned(),
        sts.base().to_owned(),
    );
    t.insert(
        "https://bedrock.us-east-1.amazonaws.com".to_owned(),
        bedrock.base().to_owned(),
    );
    Rig {
        ep: endpoint(""),
        p: BedrockProvider::with_overrides(Overrides::new(&t)),
        sts,
        bedrock,
        http: reqwest::Client::new(),
    }
}

fn role_value() -> Secret<String> {
    Secret::new(
        serde_json::json!({"access_key_id": SRC_KEY, "secret_access_key": SRC_SECRET, "role_arn": ROLE})
            .to_string(),
    )
}

fn check(org_ext: Option<&str>) -> KeyCheck {
    KeyCheck {
        mode: AuthMode::AwsAssumeRole,
        org_id: "0b0b0b0b-0000-4000-8000-000000000001".into(),
        org_external_id: org_ext.map(str::to_owned),
    }
}

#[tokio::test]
async fn assume_role_is_validated_and_attributed_with_the_orgs_own_external_id() {
    let r = rig().await;
    let o =
        r.p.verify_key_origin_as(
            &r.ep,
            &check(Some("org-ext-1")),
            OriginMethod::ProviderAccount,
            &role_value(),
            &r.http,
        )
        .await
        .unwrap();
    // The role's account (not the source key's) and the role ARN.
    assert_eq!(o.organization_id.as_deref(), Some(ROLE_ACCOUNT));
    assert_eq!(o.user_id.as_deref(), Some(ROLE));
    let calls = r.sts.assume_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].signed_by, SRC_KEY);
    assert_eq!(calls[0].external_id, "org-ext-1");
    assert_eq!(
        calls[0].role_session_name,
        "aurora-0b0b0b0b-0000-4000-8000-000000000001"
    );
    assert_eq!(calls[0].duration_seconds, "3600");
    // A second organization's validation sends its own external id.
    r.p.list_models_strict_as(&r.ep, &check(Some("org-ext-2")), &role_value(), &r.http)
        .await
        .unwrap();
    assert_eq!(r.sts.assume_calls()[1].external_id, "org-ext-2");
    // No external id: no call at all (fail closed), reported as unreachable, never as a granted role.
    let before = r.sts.calls();
    assert!(
        r.p.list_models_strict_as(&r.ep, &check(None), &role_value(), &r.http)
            .await
            .is_err()
    );
    assert_eq!(r.sts.calls(), before);
}

#[tokio::test]
async fn access_denied_is_assume_role_denied_and_a_wrong_secret_is_an_invalid_key() {
    let r = rig().await;
    r.sts.deny_assume(true);
    assert!(matches!(
        r.p.list_models_strict_as(&r.ep, &check(Some("e")), &role_value(), &r.http)
            .await,
        Err(LlmError::AssumeRoleDenied)
    ));
    r.sts.deny_assume(false);
    let wrong = Secret::new(
        serde_json::json!({"access_key_id": SRC_KEY, "secret_access_key": "not-the-secret", "role_arn": ROLE})
            .to_string(),
    );
    // SignatureDoesNotMatch: refused by the token endpoint, not "denied".
    assert!(matches!(
        r.p.list_models_strict_as(&r.ep, &check(Some("e")), &wrong, &r.http)
            .await,
        Err(LlmError::MintRejected)
    ));
    // A value of the wrong shape is a bad key.
    assert!(matches!(
        r.p.list_models_strict_as(&r.ep, &check(Some("e")), &Secret::new("{}".into()), &r.http)
            .await,
        Err(LlmError::InvalidKey)
    ));
}

#[tokio::test]
async fn a_static_key_proves_its_account_with_a_signed_caller_identity() {
    let r = rig().await;
    let value = Secret::new(
        serde_json::json!({"access_key_id": SRC_KEY, "secret_access_key": SRC_SECRET}).to_string(),
    );
    let c = KeyCheck::new(AuthMode::AwsSigv4);
    r.p.list_models_strict_as(&r.ep, &c, &value, &r.http)
        .await
        .unwrap();
    let o =
        r.p.verify_key_origin_as(&r.ep, &c, OriginMethod::ProviderAccount, &value, &r.http)
            .await
            .unwrap();
    assert_eq!(o.organization_id.as_deref(), Some(ROLE_ACCOUNT));
    assert!(r.sts.assume_calls().is_empty());
    // A bad secret fails the signature check.
    let bad = Secret::new(
        serde_json::json!({"access_key_id": SRC_KEY, "secret_access_key": "nope"}).to_string(),
    );
    assert!(matches!(
        r.p.list_models_strict_as(&r.ep, &c, &bad, &r.http).await,
        Err(LlmError::InvalidKey)
    ));
}

#[tokio::test]
async fn an_api_key_reports_no_account_and_lists_with_a_bearer() {
    let r = rig().await;
    r.bedrock.accept_api_key("bedrock-api-key-1");
    let key = Secret::new("bedrock-api-key-1".to_owned());
    let c = KeyCheck::new(AuthMode::ApiKey);
    r.p.list_models_strict_as(&r.ep, &c, &key, &r.http)
        .await
        .unwrap();
    assert!(matches!(
        r.p.verify_key_origin_as(&r.ep, &c, OriginMethod::ProviderAccount, &key, &r.http)
            .await,
        Err(LlmError::Unverifiable(_))
    ));
    assert!(matches!(
        r.p.list_models_strict_as(&r.ep, &c, &Secret::new("wrong".into()), &r.http)
            .await,
        Err(LlmError::InvalidKey)
    ));
}

#[tokio::test]
async fn listing_merges_profiles_first_and_falls_back_to_static_on_403() {
    let r = rig().await;
    let c = check(Some("e"));
    let ids: Vec<String> =
        r.p.list_models_as(&r.ep, &c, &role_value(), &r.http)
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
    assert_eq!(
        ids,
        [
            "us.anthropic.claude-3-haiku-20240307-v1:0",
            "anthropic.claude-3-haiku-20240307-v1:0"
        ]
    );
    // The list calls were signed by the session, never by the source key.
    let signed: Vec<String> = r
        .bedrock
        .requests()
        .iter()
        .filter_map(|s| s.verified.as_ref().ok().map(|v| v.access_key.clone()))
        .collect();
    assert!(
        !signed.is_empty() && signed.iter().all(|k| k.starts_with("ASIA")),
        "{signed:?}"
    );
    r.bedrock.set_list_status(403);
    let ids: Vec<String> =
        r.p.list_models_as(&r.ep, &c, &role_value(), &r.http)
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
    assert_eq!(ids, ["static-model"]);
}
