#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! SCIM scope (SEC-72, docs/18 §SCIM): an org's token cannot see or touch another org's
//! resources (404, never 403), manual/IdP groups are invisible, roles are unreachable, the
//! e-mail domain allow-lists hold, the seat cap holds, and the per-token rate limit answers
//! 429 with `retry-after`.

use crate::common::*;
use crate::scim_common::*;
use aurora_config::SignupMode;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

const USERS: &str = "/scim/v2/Users";
const GROUPS: &str = "/scim/v2/Groups";

fn urlencode(s: &str) -> String {
    s.replace(' ', "%20").replace('"', "%22")
}

#[tokio::test]
async fn org_a_token_cannot_reach_org_b() {
    let fx = ScimFx::new().await;
    let other = other_org(&fx.h).await;
    // A and B each provision one user.
    let a = fx.provision_ok("a@x.io").await;
    let (s, _, _) = fx
        .scim_with(
            Method::POST,
            USERS,
            &bearer(&other.token),
            Some(json!({"userName": "b@x.io"})),
        )
        .await;
    assert_eq!(s, 201);
    let a_id = a["id"].as_str().unwrap().to_owned();
    // Read, filter and write with the wrong token: 404.
    let (s, _, v) = fx
        .scim_with(
            Method::GET,
            &format!("{USERS}/{a_id}"),
            &bearer(&other.token),
            None,
        )
        .await;
    assert_eq!(s, 404, "{v}");
    let (s, _, v) = fx
        .scim_with(Method::GET, USERS, &bearer(&other.token), None)
        .await;
    assert_eq!(s, 200, "{v}");
    // The listing of B never contains A's resource.
    let resources = v["Resources"].as_array().unwrap();
    assert!(resources.iter().all(|r| r["id"] != a["id"]), "{v}");
    // A filter for A's user from B: empty result, not the resource.
    let (s, _, v) = fx
        .scim_with(
            Method::GET,
            &format!("{USERS}?filter={}", urlencode("userName eq \"a@x.io\"")),
            &bearer(&other.token),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["totalResults"], 0, "{v}");
    // Writes with the wrong token: 404 (PUT/DELETE on a foreign id).
    let (s, _, v) = fx
        .scim_with(
            Method::PUT,
            &format!("{USERS}/{a_id}"),
            &bearer(&other.token),
            Some(json!({"userName": "a@x.io", "active": false})),
        )
        .await;
    assert_eq!(s, 404, "{v}");
    let (s, _, _) = fx
        .scim_with(
            Method::DELETE,
            &format!("{USERS}/{a_id}"),
            &bearer(&other.token),
            None,
        )
        .await;
    assert_eq!(s, 404);
    // Still intact.
    let (s, _, v) = fx.scim(Method::GET, &format!("{USERS}/{a_id}"), None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["userName"], "a@x.io", "{v}");
}

#[tokio::test]
async fn manual_groups_invisible_no_roles_no_owner_mutations() {
    let fx = ScimFx::new().await;
    let owner_user = fx.h.user("real-owner@x.io", false).await;
    fx.h.member(fx.org, owner_user, "owner", "active").await;
    // A manual group via /v1.
    let (s, v) = fx
        .mgmt(
            Method::POST,
            &format!("/v1/orgs/{}/groups", fx.org),
            Some(json!({"name": "manual-1"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let manual: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // Invisible to SCIM: 404 on the item, absent from the list.
    let (s, _, v) = fx
        .scim(Method::GET, &format!("{GROUPS}/{manual}"), None)
        .await;
    assert_eq!(s, 404, "{v}");
    let (s, _, v) = fx.scim(Method::GET, GROUPS, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["totalResults"], 0, "{v}");
    // PUT/DELETE on it: 404 as well.
    let (s, _, _) = fx
        .scim(
            Method::PUT,
            &format!("{GROUPS}/{manual}"),
            Some(json!({"displayName": "x"})),
        )
        .await;
    assert_eq!(s, 404);

    // No way to set a role or create an owner: the owner is adopted as it is, never mutated.
    let (s, _, v) = fx
        .scim(
            Method::POST,
            USERS,
            Some(json!({"userName": "real-owner@x.io", "roles": [{"value": "admin"}]})),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    assert_eq!(scim_type(&v), Some("invalidValue"), "{v}");
    // The owner link (if provisioned) can never be deactivated.
    let (s, _, v) = fx
        .scim(
            Method::POST,
            USERS,
            Some(json!({"userName": "real-owner@x.io"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let sid = v["id"].as_str().unwrap().to_owned();
    let (s, _, v) = fx
        .scim(
            Method::PATCH,
            &format!("{USERS}/{sid}"),
            Some(json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                "Operations": [{"op": "replace", "path": "active", "value": false}],
            })),
        )
        .await;
    assert_eq!(s, 403, "{v}");
    let (s, _, v) = fx
        .scim(Method::DELETE, &format!("{USERS}/{sid}"), None)
        .await;
    assert_eq!(s, 403, "{v}");
    // The membership is untouched.
    let st: String =
        sqlx::query_scalar("SELECT status FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(fx.org)
            .bind(owner_user)
            .fetch_one(fx.h.pool())
            .await
            .unwrap();
    assert_eq!(st, "active");
}

#[tokio::test]
async fn email_domain_allow_lists() {
    // Org allow-list `x.io` (the fixture's default).
    let fx = ScimFx::new().await;
    let (s, _, v) = fx
        .scim(Method::POST, USERS, Some(json!({"userName": "y@other.io"})))
        .await;
    assert_eq!(s, 400, "{v}");
    assert_eq!(scim_type(&v), Some("invalidValue"), "{v}");
    // The instance allow-list is additional: only `corp.example` addresses are usable, so
    // the org allow-list must name a domain the instance permits.
    let h = Harness::with_config(|c| {
        c.scim.enabled = true;
        c.scim.allowed_email_domains = vec!["corp.example".into()];
        c.signup_mode = SignupMode::Open;
        c.org_creation = aurora_config::OrgCreation::Anyone;
    })
    .await;
    let owner = h.sign_in("o@corp.example").await;
    stamp_reauth(&h, owner.user_id).await;
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/orgs",
            &[("cookie", &owner.cookie), ("origin", ORIGIN)],
            Some(json!({"name": "A"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let org: Uuid = v["org"]["id"].as_str().unwrap().parse().unwrap();
    // An org domain outside the instance allow-list is refused at PUT time.
    let (s, _, v) = h
        .send(
            Method::PUT,
            &format!("/v1/orgs/{org}/scim"),
            &[("cookie", &owner.cookie), ("origin", ORIGIN)],
            Some(json!({"enabled": true, "email_domains": ["x.io"]})),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "scim_domain_not_allowed", "{v}");
    // A listed one is accepted.
    let (s, _, v) = h
        .send(
            Method::PUT,
            &format!("/v1/orgs/{org}/scim"),
            &[("cookie", &owner.cookie), ("origin", ORIGIN)],
            Some(json!({"enabled": true, "email_domains": ["corp.example"]})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let tok = mint_token(&h, &owner.cookie, org).await;
    let (s, _, v) = h
        .send(
            Method::POST,
            USERS,
            &[("authorization", &bearer(&tok))],
            Some(json!({"userName": "p@x.io"})),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    assert_eq!(scim_type(&v), Some("invalidValue"), "{v}");
    // Same org domain list but an instance-allowed domain works.
    let (s, _, v) = h
        .send(
            Method::POST,
            USERS,
            &[("authorization", &bearer(&tok))],
            Some(json!({"userName": "p@corp.example"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
}

#[tokio::test]
async fn seat_limit_holds() {
    let h = Harness::with_config(|c| {
        c.scim.enabled = true;
        c.signup_mode = SignupMode::Open;
        c.org_creation = aurora_config::OrgCreation::Anyone;
    })
    .await;
    let owner = h.sign_in("owner@x.io").await;
    stamp_reauth(&h, owner.user_id).await;
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/orgs",
            &[("cookie", &owner.cookie), ("origin", ORIGIN)],
            Some(json!({"name": "A"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let org: Uuid = v["org"]["id"].as_str().unwrap().parse().unwrap();
    // Cap the org at two active members (the owner is one).
    sqlx::query("UPDATE organizations SET settings = settings || '{\"max_members\": 2}'::jsonb WHERE id = $1")
        .bind(org)
        .execute(h.pool())
        .await
        .unwrap();
    let (s, _, v) = h
        .send(
            Method::PUT,
            &format!("/v1/orgs/{org}/scim"),
            &[("cookie", &owner.cookie), ("origin", ORIGIN)],
            Some(json!({"enabled": true, "email_domains": ["x.io"]})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let tok = mint_token(&h, &owner.cookie, org).await;
    let (s, _, v) = h
        .send(
            Method::POST,
            USERS,
            &[("authorization", &bearer(&tok))],
            Some(json!({"userName": "one@x.io"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    // The second provision would be member #3: 403 member_limit_reached.
    let (s, _, v) = h
        .send(
            Method::POST,
            USERS,
            &[("authorization", &bearer(&tok))],
            Some(json!({"userName": "two@x.io"})),
        )
        .await;
    assert_eq!(s, 403, "{v}");
    assert!(v["detail"].as_str().is_some(), "{v}");
    // Nothing partial was written.
    let n = count(
        &h,
        "SELECT count(*) FROM org_members WHERE org_id = $1",
        org,
    )
    .await;
    assert_eq!(n, 2, "owner + one provisioned");
}

#[tokio::test]
async fn per_token_rate_limit_429_with_retry_after() {
    // Tiny bucket: burst = per_minute / 5 → 5.
    let h = Harness::with_config(|c| {
        c.scim.enabled = true;
        c.scim.rate_per_minute = 25;
        c.signup_mode = SignupMode::Open;
        c.org_creation = aurora_config::OrgCreation::Anyone;
    })
    .await;
    let owner = h.sign_in("owner@x.io").await;
    stamp_reauth(&h, owner.user_id).await;
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/orgs",
            &[("cookie", &owner.cookie), ("origin", ORIGIN)],
            Some(json!({"name": "A"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let org: Uuid = v["org"]["id"].as_str().unwrap().parse().unwrap();
    let (s, _, v) = h
        .send(
            Method::PUT,
            &format!("/v1/orgs/{org}/scim"),
            &[("cookie", &owner.cookie), ("origin", ORIGIN)],
            Some(json!({"enabled": true, "email_domains": ["x.io"]})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let tok = mint_token(&h, &owner.cookie, org).await;
    // The burst is 5: the 6th request within the window is 429.
    let mut last = (0, Value::Null);
    for i in 0..6 {
        let (s, hd, v) = h
            .send(
                Method::GET,
                USERS,
                &[("authorization", &bearer(&tok))],
                None,
            )
            .await;
        last = (s.as_u16(), v.clone());
        if s != http::StatusCode::OK {
            assert_eq!(s.as_u16(), 429, "request {i}: {v}");
            assert!(hd.get("retry-after").is_some(), "429 carries retry-after");
            assert_eq!(
                v["schemas"][0], "urn:ietf:params:scim:api:messages:2.0:Error",
                "{v}"
            );
            break;
        }
    }
    assert_eq!(last.0, 429, "the bucket must refuse within 6 requests");
}

/// FIX-070: the seat cap holds under concurrent `POST /Users` — with one seat left, parallel
/// provisions serialize on the org lock before counting: exactly one is created, the others
/// answer 403 `member_limit_reached`, and the org never exceeds `max_members`.
#[tokio::test]
async fn concurrent_posts_at_the_last_seat_create_exactly_one() {
    const N: usize = 6;
    let fx = ScimFx::new().await;
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM org_members WHERE org_id = $1 AND status = 'active'",
    )
    .bind(fx.org)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    let cap = active + 1;
    sqlx::query(
        "UPDATE organizations SET settings = settings || jsonb_build_object('max_members', $2::int8) \
         WHERE id = $1",
    )
    .bind(fx.org)
    .bind(cap)
    .execute(fx.h.pool())
    .await
    .unwrap();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(N));
    let rs = futures::future::join_all((0..N).map(|i| {
        let barrier = barrier.clone();
        let fx = &fx;
        async move {
            barrier.wait().await;
            fx.scim(
                Method::POST,
                USERS,
                Some(json!({"userName": format!("seat-{i}@x.io")})),
            )
            .await
        }
    }))
    .await;
    let created = rs.iter().filter(|(s, _, _)| *s == 201).count();
    for (s, _, v) in &rs {
        assert!(
            *s == 201 || (*s == 403 && v["detail"] == "member_limit_reached"),
            "{s}: {v}"
        );
    }
    assert_eq!(created, 1, "exactly the last seat is taken");
    let after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM org_members WHERE org_id = $1 AND status = 'active'",
    )
    .bind(fx.org)
    .fetch_one(fx.h.pool())
    .await
    .unwrap();
    assert_eq!(after, cap, "never above max_members");
}

/// FIX-062 (C-473, SEC-72): a `PUT` of an owner's link is refused 403 whatever its body —
/// a harmless attribute change and an e-mail change alike — before anything is written: the
/// link row, the membership and the account stay exactly as they were.
#[tokio::test]
async fn owner_link_put_refused_nothing_written() {
    let fx = ScimFx::new().await;
    let owner_user = fx.h.user("put-owner@x.io", false).await;
    fx.h.member(fx.org, owner_user, "owner", "active").await;
    let (s, _, v) = fx
        .scim(
            Method::POST,
            USERS,
            Some(json!({"userName": "put-owner@x.io", "displayName": "Owner"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let sid = v["id"].as_str().unwrap().to_owned();
    type Snap = (Uuid, String, Option<String>, String, Option<String>, String);
    let snap = || async {
        let link: Snap = sqlx::query_as(
            "SELECT s.user_id, s.user_name::text, s.display_name, s.state, s.external_id, \
                    s.updated_at::text \
             FROM scim_users s WHERE s.id = $1",
        )
        .bind(sid.parse::<Uuid>().unwrap())
        .fetch_one(fx.h.pool())
        .await
        .unwrap();
        let seat: (String, String) = sqlx::query_as(
            "SELECT role, status FROM org_members WHERE org_id = $1 AND user_id = $2",
        )
        .bind(fx.org)
        .bind(owner_user)
        .fetch_one(fx.h.pool())
        .await
        .unwrap();
        let acct: (String, String) =
            sqlx::query_as("SELECT email::text, display_name FROM users WHERE id = $1")
                .bind(owner_user)
                .fetch_one(fx.h.pool())
                .await
                .unwrap();
        (link, seat, acct)
    };
    let before = snap().await;
    for body in [
        json!({"userName": "put-owner@x.io", "displayName": "Renamed"}),
        json!({"userName": "elsewhere@x.io"}),
        json!({"userName": "put-owner@x.io", "active": true}),
    ] {
        let (s, _, v) = fx
            .scim(Method::PUT, &format!("{USERS}/{sid}"), Some(body.clone()))
            .await;
        assert_eq!(s, 403, "{body}: {v}");
        assert_eq!(
            v["detail"], "Organization owners are managed in Aurora.",
            "{v}"
        );
        assert_eq!(snap().await, before, "{body}: nothing written");
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE email = 'elsewhere@x.io'")
        .fetch_one(fx.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 0);
    // A PATCH that does not deactivate stays allowed (docs/04: PATCH active=false, PUT and
    // DELETE are the refused mutations) and lands on the link only.
    let (s, _, v) = fx
        .scim(
            Method::PATCH,
            &format!("{USERS}/{sid}"),
            Some(json!({
                "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                "Operations": [{"op": "replace", "path": "displayName", "value": "Link only"}],
            })),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let after = snap().await;
    assert_eq!(after.1, before.1, "membership untouched");
    assert_eq!(after.2, before.2, "account untouched");
}
