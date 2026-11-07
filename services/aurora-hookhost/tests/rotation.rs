#![allow(clippy::unwrap_used, clippy::expect_used)]
//! RFE-007 / SEC-46: the hook host accepts any key of its internal-secret ring (named by
//! `X-Aurora-Key-Id`, or tried in turn when the header is absent), refuses retired keys, and
//! presents its own active key + id when it calls the API.
use std::sync::Arc;

use aurora_crypto::{RingKind, SecretRing};
use aurora_hookhost::fetch::FetchPolicy;
use aurora_hookhost::runtime::Runtime;
use aurora_hookhost::server::{AppState, router};
use aurora_hookhost::snapshot::Catalog;
use axum::body::Body;
use axum::http::Request;
use base64::Engine as _;
use http_body_util::BodyExt;
use tower::ServiceExt;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn b64(b: u8) -> String {
    base64::engine::general_purpose::STANDARD.encode([b; 32])
}

fn ring(ids: &[(&str, u8)], active: &str) -> Arc<SecretRing> {
    let l = ids
        .iter()
        .map(|(i, b)| format!("{i}:{}", b64(*b)))
        .collect::<Vec<_>>()
        .join(",");
    Arc::new(SecretRing::parse(&l, Some(active), RingKind::Internal).unwrap())
}

fn app(ring: Arc<SecretRing>) -> axum::Router {
    let rt = Runtime::new(FetchPolicy::default()).unwrap();
    router(AppState {
        catalog: Catalog::new("http://127.0.0.1:1", String::new(), rt),
        ring,
    })
}

async fn status(app: &axum::Router, secret: u8, id: Option<&str>) -> (u16, serde_json::Value) {
    let mut b = Request::builder()
        .uri("/internal/v1/status")
        .header("authorization", format!("Bearer {}", b64(secret)));
    if let Some(id) = id {
        b = b.header("x-aurora-key-id", id);
    }
    let res = app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let st = res.status().as_u16();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (st, serde_json::from_slice(&bytes).unwrap_or_default())
}

#[tokio::test]
async fn ring_rotation_sequence_on_the_hook_host() {
    // Phase-0 (single key, ring v0), step 1 (append), step 2 (activate), step 3 (remove).
    let p0 = app(ring(&[("v0", 1)], "v0"));
    let s1 = app(ring(&[("v0", 1), ("n2", 2)], "v0"));
    let s2 = app(ring(&[("v0", 1), ("n2", 2)], "n2"));
    let s3 = app(ring(&[("n2", 2)], "n2"));
    // An unauthenticated or wrongly-keyed call is refused.
    let res = p0
        .clone()
        .oneshot(
            Request::builder()
                .uri("/internal/v1/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    // Header-less legacy call (old sender) is accepted against any ring key.
    assert_eq!(status(&p0, 1, None).await.0, 200);
    assert_eq!(status(&s1, 1, None).await.0, 200);
    assert_eq!(status(&s2, 1, None).await.0, 200);
    // A new sender naming `n2` works wherever `n2` was appended — not on the phase-0 replica.
    assert_eq!(status(&p0, 2, Some("n2")).await.0, 401);
    assert_eq!(status(&s1, 2, Some("n2")).await.0, 200);
    assert_eq!(status(&s2, 2, Some("n2")).await.0, 200);
    // Wrong secret for the named id, and unknown ids, are refused.
    assert_eq!(status(&s2, 1, Some("n2")).await.0, 401);
    assert_eq!(status(&s2, 1, Some("zz")).await.0, 401);
    // Retired key after step 3.
    assert_eq!(status(&s3, 1, Some("v0")).await.0, 401);
    assert_eq!(status(&s3, 1, None).await.0, 401);
    let (st, body) = status(&s3, 2, Some("n2")).await;
    assert_eq!(st, 200);
    assert_eq!(body["service"], "hookhost");
    assert_eq!(body["internal_secret"]["active"], "n2");
    assert!(
        !body.to_string().contains(&b64(2)),
        "status leaks key material"
    );
}

#[tokio::test]
async fn hook_endpoint_uses_the_same_ring() {
    let a = app(ring(&[("a1", 1), ("b2", 2)], "a1"));
    let call = |secret: u8, id: Option<&str>| {
        let mut b = Request::builder()
            .method("POST")
            .uri("/internal/v1/hooks/nope")
            .header("authorization", format!("Bearer {}", b64(secret)));
        if let Some(id) = id {
            b = b.header("x-aurora-key-id", id);
        }
        b.body(Body::empty()).unwrap()
    };
    // Authenticated calls reach routing (404 unknown hook); others stop at 401.
    assert_eq!(
        a.clone()
            .oneshot(call(2, Some("b2")))
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        a.clone().oneshot(call(2, None)).await.unwrap().status(),
        404
    );
    assert_eq!(
        a.clone()
            .oneshot(call(2, Some("a1")))
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        a.clone().oneshot(call(9, None)).await.unwrap().status(),
        401
    );
}

#[tokio::test]
async fn catalog_presents_active_key_and_id_to_the_api() {
    let api = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/internal/v1/registry/active"))
        .and(header(
            "authorization",
            format!("Bearer {}", b64(2)).as_str(),
        ))
        .and(header("x-aurora-key-id", "b2"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1..)
        .mount(&api)
        .await;
    let r = ring(&[("a1", 1), ("b2", 2)], "b2");
    let rt = Runtime::new(FetchPolicy::default()).unwrap();
    let catalog = Catalog::with_key_id(&api.uri(), r.active_bearer(), Some("b2".into()), rt);
    // Trigger one fetch; a 404 is fine, the mock matcher asserts the headers.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), catalog.refresh()).await;
}
