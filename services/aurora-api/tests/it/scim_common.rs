#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-GRP-2 shared fixture (docs/18 §SCIM): a harness with the instance switch on, an org
//! with SCIM enabled through the real management routes, an owner session and a live `aud_`
//! token. The `/scim/v2` calls go through the real router (no test-only nesting).

use crate::common::*;
use http::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

use aurora_config::SignupMode;

pub struct ScimFx {
    pub h: Harness,
    /// The org with SCIM enabled.
    pub org: Uuid,
    /// The org owner (session for the management routes).
    pub owner: SignedIn,
    /// The org's live `aud_` token (raw; shown once).
    pub token: String,
}

/// A member of org B (a second org) with its own enabled SCIM — the cross-org cases.
pub struct OtherOrg {
    #[allow(dead_code)]
    pub org: Uuid,
    pub token: String,
}

impl ScimFx {
    /// Full setup: SCIM on for the instance and the org, one token.
    pub async fn new() -> Self {
        Self::with_domains(&[]).await
    }

    /// Setup with an org e-mail-domain allow-list (empty = unrestricted).
    pub async fn with_domains(domains: &[&str]) -> Self {
        Self::with("x.io", domains).await
    }

    /// Setup for the e-mail domain `domain` (the org allow-list is `[domain]` when
    /// `domains` is empty) — most suites provision `u@x.io`.
    pub async fn with(_domain: &'static str, domains: &[&str]) -> Self {
        Self::with_config(
            |c| {
                c.scim.enabled = true;
                c.signup_mode = SignupMode::Open;
                c.org_creation = aurora_config::OrgCreation::Anyone;
            },
            domains,
        )
        .await
    }

    /// Like [`ScimFx::with`], but the dev provider JIT-provisions into the fixture org
    /// (slug `acme`): sign-ins pass the SCIM admission gate (SEC-73 tests).
    pub async fn with_jit(domains: &[&str]) -> Self {
        Self::with_config(
            |c| {
                c.scim.enabled = true;
                c.signup_mode = SignupMode::Open;
                c.org_creation = aurora_config::OrgCreation::Anyone;
                for p in &mut c.auth.oidc_providers {
                    if p.id == aurora_config::DEV_IDP_PROVIDER_ID {
                        p.jit = Some(aurora_config::OidcJit {
                            org_slug: "acme".into(),
                            role: "member".into(),
                            workspace_slugs: vec![],
                            workspace_role: String::new(),
                        });
                    }
                }
            },
            domains,
        )
        .await
    }

    pub async fn with_config(
        mutate: impl FnOnce(&mut aurora_config::ApiConfig),
        domains: &[&str],
    ) -> Self {
        let h = Harness::with_config(mutate).await;
        let owner = h.sign_in("owner@x.io").await;
        stamp_reauth(&h, owner.user_id).await;
        // The org is created through the real route so the owner role and slug match.
        let (s, _, v) = h
            .send(
                Method::POST,
                "/v1/orgs",
                &[("cookie", &owner.cookie), ("origin", ORIGIN)],
                Some(json!({"name": "Acme"})),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        let org: Uuid = v["org"]["id"].as_str().unwrap().parse().unwrap();
        // Enable SCIM with the e-mail domain of the test people (the caller may list
        // several; the default is the domain of the fixture's addresses).
        let list: Vec<String> = if domains.is_empty() {
            vec!["x.io".to_owned()]
        } else {
            domains.iter().map(|d| d.to_string()).collect()
        };
        let body = json!({
            "enabled": true,
            "email_domains": list,
        });
        let (s, _, v) = h
            .send(
                Method::PUT,
                &format!("/v1/orgs/{org}/scim"),
                &[("cookie", &owner.cookie), ("origin", ORIGIN)],
                Some(body),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let token = mint_token(&h, &owner.cookie, org).await;
        Self {
            h,
            org,
            owner,
            token,
        }
    }

    /// The bearer header pair for the org's token.
    pub fn auth(&self) -> Vec<(&'static str, String)> {
        vec![("authorization", format!("Bearer {}", self.token))]
    }

    /// A `/scim/v2` request with the org's token. Returns `(status, headers, body)`.
    pub async fn scim(
        &self,
        m: Method,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, http::HeaderMap, Value) {
        let auth = self.auth();
        let headers: Vec<(&str, &str)> = auth.iter().map(|(k, v)| (*k, v.as_str())).collect();
        self.h.send(m, uri, &headers, body).await
    }

    /// A `/scim/v2` request with an arbitrary bearer (bad-token cases).
    pub async fn scim_with(
        &self,
        m: Method,
        uri: &str,
        bearer: &str,
        body: Option<Value>,
    ) -> (StatusCode, http::HeaderMap, Value) {
        self.h
            .send(m, uri, &[("authorization", bearer)], body)
            .await
    }

    /// A `/v1` management call as the owner.
    pub async fn mgmt(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(
                m,
                uri,
                &[("cookie", &self.owner.cookie), ("origin", ORIGIN)],
                body,
            )
            .await;
        (s.as_u16(), v)
    }

    /// The SCIM Users payload of one provisioned person (`POST /Users`).
    pub async fn provision(&self, email: &str, extra: Value) -> Value {
        let mut body = json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "userName": email,
            "name": {"givenName": "U", "familyName": "X"},
        });
        if let Some(obj) = extra.as_object() {
            for (k, v) in obj {
                body[k] = v.clone();
            }
        }
        let (s, _, v) = self.scim(Method::POST, "/scim/v2/Users", Some(body)).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        v
    }

    pub async fn provision_ok(&self, email: &str) -> Value {
        self.provision(email, json!({})).await
    }

    /// The Aurora user id behind a SCIM user resource.
    pub async fn aurora_user(&self, scim_id: &str) -> Uuid {
        sqlx::query_scalar::<_, Uuid>("SELECT user_id FROM scim_users WHERE id = $1")
            .bind(scim_id.parse::<Uuid>().unwrap())
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }
}

/// Mint a token through the management route (owner session; 412 without re-auth is handled
/// by seeding a fresh session — the harness sessions are born inside the re-auth window).
pub async fn mint_token(h: &Harness, cookie: &str, org: Uuid) -> String {
    let (s, _, v) = h
        .send(
            Method::POST,
            &format!("/v1/orgs/{org}/scim/tokens"),
            &[("cookie", cookie), ("origin", ORIGIN)],
            Some(json!({"name": "it"})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    v["token"].as_str().unwrap().to_owned()
}

/// Stamp `sessions.reauth_at = now()` for the user's newest session (the management routes
/// require a re-auth within the last 10 minutes; the mock sign-in flow doesn't do one).
pub async fn stamp_reauth(h: &Harness, user: Uuid) {
    sqlx::query(
        "UPDATE sessions SET reauth_at = now() \
         WHERE id = (SELECT id FROM sessions WHERE user_id = $1 ORDER BY id DESC LIMIT 1)",
    )
    .bind(user)
    .execute(h.pool())
    .await
    .unwrap();
}

/// A second org with SCIM enabled and its own token (org B of the cross-org cases).
pub async fn other_org(h: &Harness) -> OtherOrg {
    let owner = h.sign_in("b-owner@x.io").await;
    stamp_reauth(h, owner.user_id).await;
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/orgs",
            &[("cookie", &owner.cookie), ("origin", ORIGIN)],
            Some(json!({"name": "Beta"})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let org: Uuid = v["org"]["id"].as_str().unwrap().parse().unwrap();
    let (s, _, v) = h
        .send(
            Method::PUT,
            &format!("/v1/orgs/{org}/scim"),
            &[("cookie", &owner.cookie), ("origin", ORIGIN)],
            Some(json!({"enabled": true, "email_domains": ["x.io"]})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let token = mint_token(h, &owner.cookie, org).await;
    OtherOrg { org, token }
}

/// The SCIM error envelope's `scimType` (absent when the RFC names none).
pub fn scim_type(v: &Value) -> Option<&str> {
    v["scimType"].as_str()
}

/// Assert the body is the SCIM error format (`16 §W15`): the Error schema, `status` a string.
pub fn assert_scim_error(status: u16, v: &Value) {
    assert_eq!(
        v["schemas"][0], "urn:ietf:params:scim:api:messages:2.0:Error",
        "{v}"
    );
    assert_eq!(v["status"], status.to_string(), "{v}");
    assert!(v["detail"].as_str().is_some(), "{v}");
}

/// Count rows of a table matching one org.
pub async fn count(h: &Harness, sql: &str, id: Uuid) -> i64 {
    sqlx::query_scalar(sql)
        .bind(id)
        .fetch_one(h.pool())
        .await
        .unwrap()
}
