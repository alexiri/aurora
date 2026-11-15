#![allow(clippy::unwrap_used, clippy::expect_used)]
//! P4-LLM-2 (C-807, SEC-97): the gateway's LLM mint service against the verifying Google token mock: single flight,
//! reuse until skew and re-mint, the failure hold, the LRU bound, version and invalidation drops.
use aurora_config::{LlmEndpoint, LlmSection};
use aurora_gateway::guard::Guard;
use aurora_gateway::invalidation::{Invalidatable, Invalidate, Scope};
use aurora_gateway::llm_mint::{LlmMint, MintRequest};
use aurora_gateway::upstream::Upstream;
use aurora_llm::mint::MintClass;
use aurora_llm::{AuthMode, Endpoint, ProviderRegistry};
use aurora_testkit::llm::google_token::{Behaviour, MockGoogleToken, TestKey};
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

struct Rig {
    key: TestKey,
    google: MockGoogleToken,
    ep: Endpoint,
    mint: Arc<LlmMint>,
}

async fn rig(cap: usize, skew: Duration, hold: Duration) -> Rig {
    let key = TestKey::generate("key-proj", "sa@key-proj.iam.gserviceaccount.com");
    let google = MockGoogleToken::start(&key).await;
    let cfg = LlmSection {
        enabled_providers: vec!["vertex".into()],
        provider_preference: vec![],
        dev_upstream_overrides: [(
            "https://oauth2.googleapis.com".to_owned(),
            google.base().to_owned(),
        )]
        .into(),
        endpoints: vec![LlmEndpoint {
            id: "vertex".into(),
            provider: "vertex".into(),
            project_id: Some("proj-1".into()),
            location: Some("us-east5".into()),
            models: vec!["claude-x".into()],
            ..Default::default()
        }],
    };
    let reg = ProviderRegistry::from_config(&cfg).unwrap();
    let guard = Arc::new(Guard::new(&["127.0.0.0/8".to_owned()], vec![]).unwrap());
    let upstream = Arc::new(Upstream::new(guard, None, None, Duration::from_secs(10)).unwrap());
    let mint = Arc::new(LlmMint::new(upstream, reg.overrides().clone()).with_limits(
        cap,
        skew,
        hold,
        Duration::from_secs(10),
    ));
    Rig {
        key,
        google,
        ep: reg.endpoint("vertex").unwrap().clone(),
        mint,
    }
}

fn req<'a>(r: &'a Rig, secret: Uuid, version: i32, value: &'a [u8]) -> MintRequest<'a> {
    MintRequest {
        secret_id: secret,
        version,
        org_id: Uuid::nil(),
        workspace_id: None,
        endpoint: &r.ep,
        mode: AuthMode::GcpServiceAccount,
        value,
    }
}

#[tokio::test]
async fn fifty_concurrent_requests_make_one_mint_and_share_the_token() {
    let r = rig(100, Duration::from_secs(300), Duration::from_secs(5)).await;
    let (id, json) = (Uuid::now_v7(), r.key.json());
    let tasks: Vec<_> = (0..50)
        .map(|_| {
            let (mint, ep, json) = (r.mint.clone(), r.ep.clone(), json.clone());
            tokio::spawn(async move {
                mint.token(&MintRequest {
                    secret_id: id,
                    version: 1,
                    org_id: Uuid::nil(),
                    workspace_id: None,
                    endpoint: &ep,
                    mode: AuthMode::GcpServiceAccount,
                    value: json.as_bytes(),
                })
                .await
                .map(|t| t.expose().clone())
            })
        })
        .collect();
    for t in tasks {
        assert_eq!(t.await.unwrap().unwrap(), "ya29.mock-token-1");
    }
    assert_eq!(r.google.requests(), 1);
    assert!(r.google.refused().is_empty());
    // Reused while valid.
    let t = r
        .mint
        .token(&req(&r, id, 1, json.as_bytes()))
        .await
        .unwrap();
    assert_eq!(t.expose(), "ya29.mock-token-1");
    assert_eq!(r.google.requests(), 1);
    // A new value version is a new key: minted again.
    r.mint
        .token(&req(&r, id, 2, json.as_bytes()))
        .await
        .unwrap();
    assert_eq!(r.google.requests(), 2);
}

#[tokio::test]
async fn token_is_minted_again_after_the_skew_window() {
    // expires_in 3 s with a 2 s skew: reusable for about one second.
    let r = rig(100, Duration::from_secs(2), Duration::from_secs(5)).await;
    r.google.set_expires_in(3);
    let (id, json) = (Uuid::now_v7(), r.key.json());
    let first = r
        .mint
        .token(&req(&r, id, 1, json.as_bytes()))
        .await
        .unwrap();
    assert_eq!(first.expose(), "ya29.mock-token-1");
    let end = Instant::now() + Duration::from_secs(15);
    loop {
        let t = r
            .mint
            .token(&req(&r, id, 1, json.as_bytes()))
            .await
            .unwrap();
        if t.expose() != "ya29.mock-token-1" {
            assert_eq!(t.expose(), "ya29.mock-token-2");
            break;
        }
        assert!(Instant::now() < end, "never re-minted");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(r.google.requests(), 2);
}

#[tokio::test]
async fn a_failed_mint_is_held_and_classified() {
    let r = rig(100, Duration::from_secs(300), Duration::from_secs(30)).await;
    let (id, json) = (Uuid::now_v7(), r.key.json());
    r.google.set_behaviour(Behaviour::Unavailable);
    for _ in 0..10 {
        let e = r
            .mint
            .token(&req(&r, id, 1, json.as_bytes()))
            .await
            .err()
            .unwrap();
        assert_eq!(e.class, MintClass::Unavailable);
    }
    assert_eq!(
        r.google.requests(),
        1,
        "a burst after a failure makes one call"
    );
    // Another secret is not held by it; a rejected mint carries the provider's code only.
    r.google.set_behaviour(Behaviour::InvalidGrant);
    let e = r
        .mint
        .token(&req(&r, Uuid::now_v7(), 1, json.as_bytes()))
        .await
        .err()
        .unwrap();
    assert_eq!(e.class, MintClass::Rejected);
    assert_eq!(e.provider_code.as_deref(), Some("invalid_grant"));
    assert_eq!(r.google.requests(), 2);
}

#[tokio::test]
async fn the_hold_ends_and_the_next_request_mints_again() {
    let r = rig(100, Duration::from_secs(300), Duration::from_secs(1)).await;
    let (id, json) = (Uuid::now_v7(), r.key.json());
    r.google.set_behaviour(Behaviour::Unavailable);
    assert!(
        r.mint
            .token(&req(&r, id, 1, json.as_bytes()))
            .await
            .is_err()
    );
    r.google.set_behaviour(Behaviour::Ok);
    let end = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(t) = r.mint.token(&req(&r, id, 1, json.as_bytes())).await {
            assert_eq!(t.expose(), "ya29.mock-token-1");
            break;
        }
        assert!(Instant::now() < end, "the hold never ended");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn the_cache_is_bounded_and_evicts_the_least_recently_used() {
    let r = rig(2, Duration::from_secs(300), Duration::from_secs(5)).await;
    let json = r.key.json();
    let ids: Vec<Uuid> = (0..3).map(|_| Uuid::now_v7()).collect();
    for id in &ids {
        r.mint
            .token(&req(&r, *id, 1, json.as_bytes()))
            .await
            .unwrap();
    }
    assert_eq!(r.google.requests(), 3);
    assert_eq!(r.mint.stats().entries, 2);
    // The newest two are cached, the oldest was evicted and is minted again.
    r.mint
        .token(&req(&r, ids[2], 1, json.as_bytes()))
        .await
        .unwrap();
    r.mint
        .token(&req(&r, ids[1], 1, json.as_bytes()))
        .await
        .unwrap();
    assert_eq!(r.google.requests(), 3);
    r.mint
        .token(&req(&r, ids[0], 1, json.as_bytes()))
        .await
        .unwrap();
    assert_eq!(r.google.requests(), 4);
}

#[tokio::test]
async fn invalidation_drops_entries_by_scope() {
    let r = rig(100, Duration::from_secs(300), Duration::from_secs(5)).await;
    let (id, json) = (Uuid::now_v7(), r.key.json());
    let org = Uuid::now_v7();
    let mk = |org_id| MintRequest {
        org_id,
        ..req(&r, id, 1, json.as_bytes())
    };
    r.mint.token(&mk(org)).await.unwrap();
    r.mint.invalidate(&Invalidate {
        scope: Scope::Org(Uuid::now_v7()),
    });
    r.mint.token(&mk(org)).await.unwrap();
    assert_eq!(r.google.requests(), 1, "another org's event keeps it");
    r.mint.invalidate(&Invalidate {
        scope: Scope::Org(org),
    });
    r.mint.token(&mk(org)).await.unwrap();
    assert_eq!(r.google.requests(), 2);
    r.mint.invalidate(&Invalidate { scope: Scope::All });
    assert_eq!(r.mint.stats().entries, 0);
}
