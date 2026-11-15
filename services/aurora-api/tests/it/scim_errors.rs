#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-148 (AUDIT-004 F-3, F-6): rejected authenticated SCIM requests land in the per-org
//! error log (`GET /v1/orgs/{org}/scim/status` → `errors`, sanitized; the 401 has no org and
//! is not logged).

use crate::scim_common::*;
use http::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn rejected_authenticated_requests_are_in_the_status_errors() {
    let fx = ScimFx::new().await;
    let org = fx.org;
    fx.provision_ok("dup@x.io").await;

    // 409 uniqueness (duplicate userName).
    let (s, _, _) = fx
        .scim(
            Method::POST,
            "/scim/v2/Users",
            Some(json!({
                "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
                "userName": "dup@x.io",
            })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    // 400 invalidFilter.
    let (s, _, _) = fx
        .scim(
            Method::GET,
            "/scim/v2/Users?filter=this%20is%20not%20a%20filter",
            None,
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // 401: no org, never logged.
    let (s, _, _) =
        fx.h.send(
            Method::GET,
            "/scim/v2/Users",
            &[("authorization", "Bearer aud_nope")],
            None,
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    let (st, v) = fx
        .mgmt(Method::GET, &format!("/v1/orgs/{org}/scim/status"), None)
        .await;
    assert_eq!(st, 200, "{v}");
    let errors = v["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 2, "{v}");
    // Newest first.
    assert_eq!(errors[0]["status"], 400, "{v}");
    assert_eq!(errors[0]["method"], "GET", "{v}");
    assert_eq!(errors[0]["resource"], "Users", "{v}");
    assert_eq!(errors[0]["scim_type"], "invalidFilter", "{v}");
    assert_eq!(errors[1]["status"], 409, "{v}");
    assert_eq!(errors[1]["method"], "POST", "{v}");
    assert_eq!(errors[1]["scim_type"], "uniqueness", "{v}");
    // Sanitized: no e-mail, filter text or token anywhere in the status body.
    let text = v.to_string();
    assert!(!text.contains("dup@x.io"), "{text}");
    assert!(!text.contains("not a filter"), "{text}");
    assert!(!text.contains("aud_"), "{text}");
}
