#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-148 (AUDIT-004 F-6): `PUT /v1/orgs/{org}/scim` answers with what it stored.

use crate::scim_common::*;
use http::Method;
use serde_json::json;

#[tokio::test]
async fn settings_put_returns_what_it_stored() {
    let fx = ScimFx::new().await;
    let org = fx.org;
    let (s, v) = fx
        .mgmt(
            Method::PUT,
            &format!("/v1/orgs/{org}/scim"),
            Some(json!({"enabled": true, "email_domains": ["a.io", "b.io"], "exclusive": true})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["enabled"], true, "{v}");
    assert_eq!(v["email_domains"], json!(["a.io", "b.io"]), "{v}");
    assert_eq!(v["exclusive"], true, "{v}");
    let (s, g) = fx
        .mgmt(Method::GET, &format!("/v1/orgs/{org}/scim"), None)
        .await;
    assert_eq!(s, 200, "{g}");
    assert_eq!(g["enabled"], v["enabled"]);
    assert_eq!(g["email_domains"], v["email_domains"]);
    assert_eq!(g["exclusive"], v["exclusive"]);
}
