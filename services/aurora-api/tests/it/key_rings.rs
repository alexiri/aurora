//! RFE-007 / SEC-46: signing-key and internal-secret rings across a full rolling rotation
//! (append everywhere → activate → remove), with live action tokens, OIDC states and
//! internal calls.
#![allow(clippy::unwrap_used)]

use super::common::Harness;
use aurora_api::domains::plugins::internal;
use aurora_channels::{ActionKind, ActionToken, Claims, Decision};
use aurora_crypto::{Purpose, RingKind, SecretRing};
use axum::body::Body;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

fn val(b: u8) -> String {
    STANDARD.encode([b; 32])
}

/// `ids` = (id, byte); entries are comma-joined `id:base64`.
fn list(ids: &[(&str, u8)]) -> String {
    ids.iter()
        .map(|(i, b)| format!("{i}:{}", val(*b)))
        .collect::<Vec<_>>()
        .join(",")
}

fn ring(ids: &[(&str, u8)], active: &str, kind: RingKind) -> SecretRing {
    SecretRing::parse(&list(ids), Some(active), kind).unwrap()
}

async fn status_call(
    h: &Harness,
    bearer: Option<(&str, Option<&str>)>,
) -> (u16, serde_json::Value) {
    let mut b = Request::builder().uri("/internal/v1/status");
    if let Some((secret_b64, key_id)) = bearer {
        b = b.header("authorization", format!("Bearer {secret_b64}"));
        if let Some(id) = key_id {
            b = b.header("x-aurora-key-id", id);
        }
    }
    let res = internal::router(h.state.clone())
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let st = res.status().as_u16();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (st, serde_json::from_slice(&bytes).unwrap_or_default())
}

async fn harness(signing: SecretRing, internal: SecretRing) -> Harness {
    Harness::with_config(|c| {
        c.signing_ring = signing;
        c.internal_ring = internal;
    })
    .await
}

const SIGN_OLD: u8 = 1;
const SIGN_NEW: u8 = 2;
const INT_OLD: u8 = 11;
const INT_NEW: u8 = 12;

#[tokio::test]
async fn internal_calls_across_the_rotation_sequence() {
    let s = ring(&[("a1", SIGN_OLD)], "a1", RingKind::Signing);
    let old_bearer = val(INT_OLD);
    let new_bearer = val(INT_NEW);
    // Receivers at each step of the rotation.
    let step1 = ring(
        &[("a1", INT_OLD), ("b2", INT_NEW)],
        "a1",
        RingKind::Internal,
    );
    let step2 = ring(
        &[("a1", INT_OLD), ("b2", INT_NEW)],
        "b2",
        RingKind::Internal,
    );
    let step3 = ring(&[("b2", INT_NEW)], "b2", RingKind::Internal);

    // Phase-0 receiver (bare value, ring v0 = old secret) accepts a header-less legacy call.
    let legacy = harness(
        s.clone(),
        SecretRing::parse(&old_bearer, None, RingKind::Internal).unwrap(),
    )
    .await;
    assert_eq!(status_call(&legacy, Some((&old_bearer, None))).await.0, 200);
    assert_eq!(
        status_call(&legacy, Some((&old_bearer, Some("v0"))))
            .await
            .0,
        200
    );

    // Step 1: new key appended, old still active. Old and new senders both work; legacy
    // (header-less) callers too.
    let h1 = harness(s.clone(), step1).await;
    for (b, id, want) in [
        (&old_bearer, Some("a1"), 200),
        (&new_bearer, Some("b2"), 200),
        (&old_bearer, None, 200),
        (&new_bearer, None, 200),
        (&old_bearer, Some("b2"), 401), // wrong key for the named id
        (&new_bearer, Some("a1"), 401),
        (&new_bearer, Some("zz"), 401), // id outside the ring
    ] {
        assert_eq!(
            status_call(&h1, Some((b, id))).await.0,
            want,
            "step1 {id:?}"
        );
    }
    assert_eq!(status_call(&h1, None).await.0, 401);

    // Step 2: new key active; old still verifies in-flight calls.
    let h2 = harness(s.clone(), step2).await;
    assert_eq!(
        status_call(&h2, Some((&old_bearer, Some("a1")))).await.0,
        200
    );
    assert_eq!(
        status_call(&h2, Some((&new_bearer, Some("b2")))).await.0,
        200
    );

    // Step 3: old key removed — a retired key stops working.
    let h3 = harness(s, step3).await;
    assert_eq!(
        status_call(&h3, Some((&old_bearer, Some("a1")))).await.0,
        401
    );
    assert_eq!(status_call(&h3, Some((&old_bearer, None))).await.0, 401);
    let (st, body) = status_call(&h3, Some((&new_bearer, Some("b2")))).await;
    assert_eq!(st, 200);
    assert_eq!(body["internal_secret"]["active"], "b2");
}

#[tokio::test]
async fn status_reports_ids_and_fingerprints_never_values() {
    let int = ring(
        &[("a1", INT_OLD), ("b2", INT_NEW)],
        "a1",
        RingKind::Internal,
    );
    let sign = ring(
        &[("s1", SIGN_OLD), ("s2", SIGN_NEW)],
        "s2",
        RingKind::Signing,
    );
    let h = harness(sign, int).await;
    let (st, body) = status_call(&h, Some((&val(INT_NEW), Some("b2")))).await;
    assert_eq!(st, 200);
    assert_eq!(body["internal_secret"]["active"], "a1");
    assert_eq!(body["signing_key"]["active"], "s2");
    let ids: Vec<_> = body["internal_secret"]["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, ["a1", "b2"]);
    let text = body.to_string();
    for b in [SIGN_OLD, SIGN_NEW, INT_OLD, INT_NEW] {
        assert!(!text.contains(&val(b)), "status leaked key material");
    }
    assert_eq!(
        body["internal_secret"]["keys"][0]["fingerprint"]
            .as_str()
            .unwrap()
            .len(),
        8
    );
}

fn claims(integ: Uuid) -> Claims {
    Claims::new(
        integ,
        Uuid::now_v7(),
        chrono::Utc::now().timestamp() + 3600,
        ActionKind::ApprovalDecide {
            decision: Decision::Approve,
        },
    )
}

#[tokio::test]
async fn action_tokens_and_states_across_the_rotation_sequence() {
    let integ = Uuid::now_v7();
    let now = chrono::Utc::now().timestamp();
    let int = ring(&[("a1", INT_OLD)], "a1", RingKind::Internal);
    // Phase-0 deployment: bare signing value (ring v0).
    let h0 = harness(
        SecretRing::parse(&val(SIGN_OLD), None, RingKind::Signing).unwrap(),
        int.clone(),
    )
    .await;
    let tok_v0 = ActionToken::sign(&h0.state.derived, &claims(integ)).unwrap();
    assert!(tok_v0.starts_with("v2.v0."));
    // A phase-0 token (`v1.<payload>.<tag>`) and an unprefixed OIDC state.
    let v1 = {
        let mut p = tok_v0.split('.').skip(2);
        format!("v1.{}.{}", p.next().unwrap(), p.next().unwrap())
    };
    let sealed0 = h0
        .state
        .derived
        .seal(Purpose::OidcState, b"aad", b"state")
        .unwrap();
    let bare_state = sealed0.split_once('.').unwrap().1.to_owned();

    // Step 1: append "n2" everywhere, active unchanged ("v0").
    let step1 = ring(
        &[("v0", SIGN_OLD), ("n2", SIGN_NEW)],
        "v0",
        RingKind::Signing,
    );
    // Step 2: activate "n2".
    let step2 = ring(
        &[("v0", SIGN_OLD), ("n2", SIGN_NEW)],
        "n2",
        RingKind::Signing,
    );
    // Step 3: remove "v0".
    let step3 = ring(&[("n2", SIGN_NEW)], "n2", RingKind::Signing);

    let h1 = harness(step1, int.clone()).await;
    let h2 = harness(step2, int.clone()).await;
    let h3 = harness(step3, int).await;

    // Mixed fleet at step 1/2: an un-updated replica (h0) cannot read what the activated
    // replica (h2) produces (id outside its ring → refused, not guessed)…
    let tok_n2 = ActionToken::sign(&h2.state.derived, &claims(integ)).unwrap();
    assert!(tok_n2.starts_with("v2.n2."));
    assert!(ActionToken::verify(&h0.state.derived, &tok_n2, integ, now).is_err());
    // …which is why the key is appended everywhere first: step-1 replicas accept it.
    assert!(ActionToken::verify(&h1.state.derived, &tok_n2, integ, now).is_ok());
    let sealed_n2 = h2
        .state
        .derived
        .seal(Purpose::OidcState, b"aad", b"state")
        .unwrap();
    assert!(sealed_n2.starts_with("n2."));
    assert!(
        h1.state
            .derived
            .open(Purpose::OidcState, b"aad", &sealed_n2)
            .is_ok()
    );
    assert!(
        h0.state
            .derived
            .open(Purpose::OidcState, b"aad", &sealed_n2)
            .is_err()
    );

    // In-flight artifacts from before the rotation keep working through steps 1 and 2.
    for h in [&h1, &h2] {
        assert!(ActionToken::verify(&h.state.derived, &tok_v0, integ, now).is_ok());
        assert!(ActionToken::verify(&h.state.derived, &v1, integ, now).is_ok());
        assert!(
            h.state
                .derived
                .open(Purpose::OidcState, b"aad", &sealed0)
                .is_ok()
        );
        assert!(
            h.state
                .derived
                .open(Purpose::OidcState, b"aad", &bare_state)
                .is_ok()
        );
    }

    // Step 3: the retired key stops working; new artifacts are fine.
    assert!(ActionToken::verify(&h3.state.derived, &tok_v0, integ, now).is_err());
    assert!(ActionToken::verify(&h3.state.derived, &v1, integ, now).is_err());
    assert!(
        h3.state
            .derived
            .open(Purpose::OidcState, b"aad", &sealed0)
            .is_err()
    );
    assert!(
        h3.state
            .derived
            .open(Purpose::OidcState, b"aad", &bare_state)
            .is_err()
    );
    assert!(ActionToken::verify(&h3.state.derived, &tok_n2, integ, now).is_ok());
    assert!(
        h3.state
            .derived
            .open(Purpose::OidcState, b"aad", &sealed_n2)
            .is_ok()
    );
}
