#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-ONB-2 (D-141, C-530..C-533): onboarding progress and the setup status.

use crate::common::*;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

async fn call(h: &Harness, tok: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
    let c = cookie(tok);
    let (s, _, v) = h
        .send(m, uri, &[("cookie", &c), ("origin", ORIGIN)], body)
        .await;
    (s.as_u16(), v)
}

fn step<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap_or_else(|| panic!("step {id} in {v}"))
}

fn check<'a>(s: &'a Value, id: &str) -> Option<&'a Value> {
    s["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
}

#[tokio::test]
async fn a_new_user_has_not_seen_the_welcome_and_changes_stick() {
    let h = Harness::new().await;
    let u = h.user("new@x.io", false).await;
    let (_, tok) = h.good_session(u).await;
    // No row yet: defaults.
    let (s, v) = call(&h, &tok, Method::GET, "/v1/me/onboarding", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["welcome_seen"], false);
    assert_eq!(v["guide_dismissed"], json!([]));
    assert!(v["tour_completed_at"].is_null());

    let ws = Uuid::now_v7();
    let (s, v) = call(
        &h,
        &tok,
        Method::PATCH,
        "/v1/me/onboarding",
        Some(json!({"welcome_seen": true, "guide_dismissed": [ws, ws], "tour_completed": true})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["welcome_seen"], true);
    assert_eq!(v["guide_dismissed"], json!([ws.to_string()]));
    assert!(v["tour_completed_at"].is_string());

    // Fields not sent do not change; `tour_completed: false` clears.
    let (s, v) = call(
        &h,
        &tok,
        Method::PATCH,
        "/v1/me/onboarding",
        Some(json!({"tour_completed": false})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["welcome_seen"], true);
    assert_eq!(v["guide_dismissed"], json!([ws.to_string()]));
    assert!(v["tour_completed_at"].is_null());

    // Progress is per user.
    let other = h.user("other@x.io", false).await;
    let (_, tok2) = h.good_session(other).await;
    let (_, v) = call(&h, &tok2, Method::GET, "/v1/me/onboarding", None).await;
    assert_eq!(v["welcome_seen"], false);
}

#[tokio::test]
async fn onboarding_rejects_unknown_fields_and_too_many_guides() {
    let h = Harness::new().await;
    let u = h.user("v@x.io", false).await;
    let (_, tok) = h.good_session(u).await;
    let (s, _) = call(
        &h,
        &tok,
        Method::PATCH,
        "/v1/me/onboarding",
        Some(json!({"is_instance_admin": true})),
    )
    .await;
    assert_eq!(s, 400);
    let many: Vec<String> = (0..201).map(|_| Uuid::now_v7().to_string()).collect();
    let (s, v) = call(
        &h,
        &tok,
        Method::PATCH,
        "/v1/me/onboarding",
        Some(json!({"guide_dismissed": many})),
    )
    .await;
    assert_eq!(s, 400, "{v}");
}

#[tokio::test]
async fn setup_status_is_for_instance_admins_and_has_reason_codes() {
    let h = Harness::new().await;
    let member = h.user("m@x.io", false).await;
    let (_, mtok) = h.good_session(member).await;
    let (s, _) = call(&h, &mtok, Method::GET, "/v1/admin/setup", None).await;
    assert_eq!(s, 403);

    let admin = h.user("a@x.io", true).await;
    let (_, tok) = h.good_session(admin).await;
    h.unsave_placement().await;
    let (s, v) = call(&h, &tok, Method::GET, "/v1/admin/setup", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["placement_saved"], false);
    let ids: Vec<&str> = v["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["roles", "placement", "llm", "runners", "optional"]);
    let placement = step(&v, "placement");
    assert_eq!(placement["status"], "todo");
    assert_eq!(placement["reason"], "placement_unsaved");
    // No runner or gateway in the test instance.
    let runners = step(&v, "runners");
    assert_eq!(runners["status"], "todo");
    assert_eq!(runners["reason"], "no_runner");
    assert_eq!(check(runners, "gateway").unwrap()["reason"], "no_gateway");
    // Optional steps never block.
    assert_eq!(step(&v, "optional")["status"], "ok");
    // No secret-shaped text anywhere.
    assert!(!v.to_string().contains("aur_"));

    // Saving a placement flips the step.
    let (s, p) = call(
        &h,
        &tok,
        Method::PUT,
        "/v1/admin/placement",
        Some(json!({"mode": "shared_org", "create_default_org": "Users"})),
    )
    .await;
    assert_eq!(s, 200, "{p}");
    let (_, v) = call(&h, &tok, Method::GET, "/v1/admin/setup", None).await;
    assert_eq!(v["placement_saved"], true);
    assert_eq!(step(&v, "placement")["status"], "ok");
    let llm = step(&v, "llm");
    assert_eq!(check(llm, "org_key").unwrap()["reason"], "no_org_key");
    assert_eq!(
        check(llm, "starter_grant").unwrap()["reason"],
        "no_starter_grant"
    );
}
