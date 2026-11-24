#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-UX-5 (D-325, D-328, SEC-128, C-919, C-920): a workspace owner adds people by e-mail; the access waits for the
//! person's first sign-in and never lets anyone into the organization. Against the in-process mock IdP.

use crate::common::{Harness, cookie};
use crate::oidc::{PID, authorize, callback, count, has_session, header, location, provider};
use aurora_config::{RateClassOverride, Redacted, SignupMode};
use aurora_testkit::oidc::{CLIENT_SECRET, MockIdp};
use http::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

const ADMIN: &str = "admin@example.com";

struct Env {
    h: Harness,
    idp: MockIdp,
    admin: String,
    org: Uuid,
}

async fn env_with(configure: impl FnOnce(&mut aurora_config::ApiConfig)) -> Env {
    let idp = MockIdp::start().await;
    let p = provider(&idp, |_| {});
    let h = Harness::with_config(move |c| {
        c.signup_mode = SignupMode::Open;
        c.auth.oidc_providers = vec![p];
        c.oidc_client_secrets
            .insert(PID.into(), Redacted::new(CLIENT_SECRET.into()));
        configure(c);
    })
    .await;
    h.unsave_placement().await;
    let admin = h.user(ADMIN, true).await;
    let (_, tok) = h.good_session(admin).await;
    let mut e = Env {
        h,
        idp,
        admin: cookie(&tok),
        org: Uuid::nil(),
    };
    // Open sign-up into one shared organization, with personal workspaces: the walkthrough's instance.
    let (s, v) = e
        .call(
            &e.admin.clone(),
            Method::PUT,
            "/v1/admin/placement",
            Some(json!({
                "mode": "shared_org", "create_default_org": "Users",
                "personal_workspace": {"enabled": true, "name_template": "{sub}|{email_local}"}
            })),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    e.org = v["default_org"]["id"].as_str().unwrap().parse().unwrap();
    e
}

async fn env() -> Env {
    env_with(|_| {}).await
}

impl Env {
    async fn call(&self, who: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        self.h.call(who, m, uri, body).await
    }

    fn who(&self, sub: &str, email: &str, verified: bool) {
        self.idp.identity(sub, email, verified);
        self.idp.set(|c| c.extra.clear());
    }

    async fn sign_in(&self) -> Result<String, String> {
        let (path, c) = authorize(&self.h, None).await;
        let (st, hd) = callback(&self.h, &path, &c).await;
        assert_eq!(st, StatusCode::FOUND);
        if has_session(&hd) {
            let c = header(&hd, "set-cookie")
                .into_iter()
                .find(|c| c.starts_with("aurora_session="))
                .unwrap();
            Ok(c.split(';').next().unwrap().to_owned())
        } else {
            Err(location(&hd).rsplit("error=").next().unwrap().to_owned())
        }
    }

    async fn n(&self, sql: &str) -> i64 {
        count(&self.h, sql).await
    }

    /// The personal workspace of the user with this e-mail.
    async fn personal(&self, email: &str) -> Uuid {
        sqlx::query_scalar(
            "SELECT w.id FROM workspaces w JOIN users u ON u.id = w.personal_owner_user_id WHERE u.email = $1",
        )
        .bind(email)
        .fetch_one(self.h.pool())
        .await
        .unwrap()
    }

    /// Bob: signs in (open sign-up places him in the organization with his own workspace); his cookie and workspace.
    async fn bob(&self) -> (String, Uuid) {
        self.who("sub-bob", "bob@example.com", true);
        let c = self.sign_in().await.unwrap();
        (c, self.personal("bob@example.com").await)
    }

    async fn add(&self, who: &str, ws: Uuid, email: &str, role: &str) -> (u16, Value) {
        self.call(
            who,
            Method::POST,
            &format!("/v1/workspaces/{ws}/access/by-email"),
            Some(json!({"email": email, "role": role})),
        )
        .await
    }

    async fn role_of(&self, ws: Uuid, email: &str) -> Option<String> {
        sqlx::query_scalar(
            "SELECT a.role FROM workspace_access a JOIN users u ON u.id = a.user_id WHERE a.workspace_id = $1 AND u.email = $2",
        )
        .bind(ws)
        .bind(email)
        .fetch_optional(self.h.pool())
        .await
        .unwrap()
    }

    async fn audit(&self, action: &str) -> i64 {
        self.n(&format!(
            "SELECT count(*) FROM audit_log WHERE action = '{action}' AND critical AND outcome = 'success'"
        ))
        .await
    }
}

// The walkthrough: Bob, an ordinary member who owns his personal workspace, adds carol@ before Carol ever signs in;
// at her first sign-in she has access as Member. Nothing created an invitation or a membership for her by itself.
#[tokio::test]
async fn bob_adds_carol_before_her_first_sign_in_and_she_gets_member_access() {
    let e = env().await;
    let (bob, ws) = e.bob().await;
    let (s, v) = e.add(&bob, ws, "  Carol@Example.com ", "member").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v, json!({"result": "pending", "org_entry": "open_signup"}));
    assert_eq!(e.audit("workspace.access.pending_create").await, 1);
    // The waiting list: the normalized address, role and expiry; nothing else about anyone.
    let (s, v) = e
        .call(
            &bob,
            Method::GET,
            &format!("/v1/workspaces/{ws}/access/pending"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    assert_eq!(v["items"][0]["email"], "carol@example.com");
    assert_eq!(v["items"][0]["role"], "member");
    // Adding never creates an invitation or a membership (D-328).
    assert_eq!(e.n("SELECT count(*) FROM invitations").await, 0);
    assert_eq!(
        e.n("SELECT count(*) FROM users WHERE email = 'carol@example.com'")
            .await,
        0
    );

    // Carol's first sign-in: placement makes her a member of the organization, then the row is applied and deleted.
    e.who("sub-carol", "carol@example.com", true);
    e.sign_in().await.unwrap();
    assert_eq!(
        e.role_of(ws, "carol@example.com").await.as_deref(),
        Some("member")
    );
    assert_eq!(
        e.n("SELECT count(*) FROM workspace_pending_access").await,
        0
    );
    assert_eq!(e.audit("workspace.access.pending_apply").await, 1);
    // Idempotent: another sign-in applies nothing and changes nothing.
    e.sign_in().await.unwrap();
    assert_eq!(e.audit("workspace.access.pending_apply").await, 1);
    assert_eq!(e.n("SELECT count(*) FROM invitations").await, 0);
}

// An unverified e-mail at sign-in never applies a row (the same rule as invitations).
#[tokio::test]
async fn a_sign_in_with_an_unverified_email_applies_nothing() {
    let e = env().await;
    let (bob, ws) = e.bob().await;
    assert_eq!(e.add(&bob, ws, "dave@example.com", "member").await.0, 200);
    e.who("sub-dave", "dave@example.com", false);
    // Whether the instance lets the unverified person in or not, the row is not applied.
    let _ = e.sign_in().await;
    assert_eq!(e.role_of(ws, "dave@example.com").await, None);
    assert_eq!(
        e.n("SELECT count(*) FROM workspace_pending_access").await,
        1
    );
    assert_eq!(e.audit("workspace.access.pending_apply").await, 0);
}

// A matching person who is NOT a member of the organization: nothing is applied and no membership is created.
#[tokio::test]
async fn a_matching_person_who_is_not_in_the_organization_gets_nothing_and_no_membership() {
    let e = env().await;
    let (bob, ws) = e.bob().await;
    assert_eq!(e.add(&bob, ws, "erin@example.com", "member").await.0, 200);
    // The organization stops placing new people; Erin already has an account (verified e-mail) but no membership.
    let (s, v) = e
        .call(
            &e.admin,
            Method::PUT,
            "/v1/admin/placement",
            Some(json!({"mode": "none"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    e.h.user("erin@example.com", false).await;
    sqlx::query("UPDATE users SET email_verified_at = now() WHERE email = 'erin@example.com'")
        .execute(e.h.pool())
        .await
        .unwrap();
    e.who("sub-erin", "erin@example.com", true);
    e.sign_in().await.unwrap();
    assert_eq!(
        e.n("SELECT count(*) FROM org_members m JOIN users u ON u.id = m.user_id WHERE u.email = 'erin@example.com'")
            .await,
        0,
        "a pending row never makes anyone an organization member"
    );
    assert_eq!(e.role_of(ws, "erin@example.com").await, None);
    assert_eq!(
        e.n("SELECT count(*) FROM workspace_pending_access").await,
        1
    );
    assert_eq!(e.n("SELECT count(*) FROM invitations").await, 0);
}

// Exact equality only: no prefix, no wildcard, no other domain.
#[tokio::test]
async fn only_the_exact_address_matches() {
    let e = env().await;
    let (bob, ws) = e.bob().await;
    for addr in [
        "fra%@example.com",
        "frank@example.com.au",
        "*@example.com",
        "ank@example.com",
    ] {
        assert_eq!(e.add(&bob, ws, addr, "member").await.0, 200, "{addr}");
    }
    e.who("sub-frank", "frank@example.com", true);
    e.sign_in().await.unwrap();
    assert_eq!(e.role_of(ws, "frank@example.com").await, None);
    assert_eq!(
        e.n("SELECT count(*) FROM workspace_pending_access").await,
        4
    );
}

// The owner role is for someone already in the organization.
#[tokio::test]
async fn owner_for_a_non_member_is_refused_and_for_a_member_is_added_at_once() {
    let e = env().await;
    let (bob, ws) = e.bob().await;
    let (s, v) = e.add(&bob, ws, "grace@example.com", "owner").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("owner_requires_member")),
        "{v}"
    );
    assert_eq!(
        e.n("SELECT count(*) FROM workspace_pending_access").await,
        0
    );
    // Heidi is already an active member (she signed in) with a verified e-mail: bound at once, any role.
    e.who("sub-heidi", "heidi@example.com", true);
    e.sign_in().await.unwrap();
    let (s, v) = e.add(&bob, ws, "heidi@example.com", "owner").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v, json!({"result": "added", "org_entry": "member"}));
    assert_eq!(
        e.role_of(ws, "heidi@example.com").await.as_deref(),
        Some("owner")
    );
    assert_eq!(e.audit("workspace.access.grant").await, 1);
    // Adding her as a member afterwards never lowers the owner role.
    let (s, _) = e.add(&bob, ws, "heidi@example.com", "member").await;
    assert_eq!(s, 200);
    assert_eq!(
        e.role_of(ws, "heidi@example.com").await.as_deref(),
        Some("owner")
    );
}

// A pending row never carries the owner role, whatever wrote it.
#[tokio::test]
async fn the_table_refuses_an_owner_row() {
    let e = env().await;
    let (_, ws) = e.bob().await;
    let r = sqlx::query(
        "INSERT INTO workspace_pending_access (id, workspace_id, org_id, email_normalized, role, expires_at)
         VALUES (gen_random_uuid(), $1, $2, 'x@example.com', 'owner', now() + interval '1 day')",
    )
    .bind(ws)
    .bind(e.org)
    .execute(e.h.pool())
    .await;
    assert!(r.is_err());
}

// At the maximum another address is refused; refreshing a waiting one is not; removing one frees a place.
#[tokio::test]
async fn the_maximum_is_enforced_per_workspace() {
    let e = env().await;
    let (bob, ws) = e.bob().await;
    sqlx::query(
        "INSERT INTO workspace_pending_access (id, workspace_id, org_id, email_normalized, role, expires_at)
         SELECT gen_random_uuid(), $1, $2, 'p' || g || '@example.com', 'member', now() + interval '1 day'
         FROM generate_series(1, 50) g",
    )
    .bind(ws)
    .bind(e.org)
    .execute(e.h.pool())
    .await
    .unwrap();
    let (s, v) = e.add(&bob, ws, "extra@example.com", "member").await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("pending_access_limit")),
        "{v}"
    );
    // The same address again is a refresh, not a new row.
    assert_eq!(e.add(&bob, ws, "p7@example.com", "approver").await.0, 200);
    assert_eq!(
        e.n("SELECT count(*) FROM workspace_pending_access WHERE email_normalized = 'p7@example.com' AND role = 'approver'")
            .await,
        1
    );
    // Remove one: room again.
    let id: Uuid = sqlx::query_scalar(
        "SELECT id FROM workspace_pending_access WHERE email_normalized = 'p1@example.com'",
    )
    .fetch_one(e.h.pool())
    .await
    .unwrap();
    let (s, _) = e
        .call(
            &bob,
            Method::DELETE,
            &format!("/v1/workspaces/{ws}/access/pending/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    assert_eq!(e.audit("workspace.access.pending_delete").await, 1);
    assert_eq!(e.add(&bob, ws, "extra@example.com", "member").await.0, 200);
}

// Rows expire: an expired row is not applied, not listed, and the sweeper removes it.
#[tokio::test]
async fn expired_rows_are_not_applied_not_listed_and_are_swept() {
    let e = env().await;
    let (bob, ws) = e.bob().await;
    assert_eq!(e.add(&bob, ws, "ivan@example.com", "member").await.0, 200);
    assert_eq!(e.add(&bob, ws, "judy@example.com", "member").await.0, 200);
    sqlx::query("UPDATE workspace_pending_access SET expires_at = now() - interval '1 minute' WHERE email_normalized = 'ivan@example.com'")
        .execute(e.h.pool())
        .await
        .unwrap();
    let (_, v) = e
        .call(
            &bob,
            Method::GET,
            &format!("/v1/workspaces/{ws}/access/pending"),
            None,
        )
        .await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1, "{v}");
    e.who("sub-ivan", "ivan@example.com", true);
    e.sign_in().await.unwrap();
    assert_eq!(e.role_of(ws, "ivan@example.com").await, None);
    let n = aurora_api::domains::tenancy::pending_access::sweep_expired(e.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(
        e.n("SELECT count(*) FROM workspace_pending_access").await,
        1
    );
}

// Who may do what: a member of the workspace cannot add or read; a person outside it gets a 404; one workspace's
// owner can neither read nor delete another workspace's rows.
#[tokio::test]
async fn only_owners_of_that_workspace_manage_its_pending_rows() {
    let e = env().await;
    let (bob, ws) = e.bob().await;
    e.who("sub-carol", "carol@example.com", true);
    let carol = e.sign_in().await.unwrap();
    let carol_id: Uuid =
        sqlx::query_scalar("SELECT id FROM users WHERE email = 'carol@example.com'")
            .fetch_one(e.h.pool())
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO workspace_access (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(ws)
    .bind(carol_id)
    .execute(e.h.pool())
    .await
    .unwrap();
    assert_eq!(e.add(&bob, ws, "kim@example.com", "member").await.0, 200);
    let (s, _) = e.add(&carol, ws, "lee@example.com", "member").await;
    // (FIX-234) The people routes are administer routes: a plain member without
    // `can_administer` finds nothing there (404).
    assert_eq!(s, 404, "a plain member of the workspace cannot add people");
    let (s, _) = e
        .call(
            &carol,
            Method::GET,
            &format!("/v1/workspaces/{ws}/access/pending"),
            None,
        )
        .await;
    assert_eq!(s, 404);
    // Carol owns her own workspace: the other workspace's rows are not hers.
    let carols_ws = e.personal("carol@example.com").await;
    let (s, v) = e
        .call(
            &carol,
            Method::GET,
            &format!("/v1/workspaces/{carols_ws}/access/pending"),
            None,
        )
        .await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 0), "{v}");
    let id: Uuid = sqlx::query_scalar(
        "SELECT id FROM workspace_pending_access WHERE email_normalized = 'kim@example.com'",
    )
    .fetch_one(e.h.pool())
    .await
    .unwrap();
    let (s, _) = e
        .call(
            &carol,
            Method::DELETE,
            &format!("/v1/workspaces/{carols_ws}/access/pending/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 404, "another workspace's row is never found");
    let (s, _) = e
        .call(
            &carol,
            Method::DELETE,
            &format!("/v1/workspaces/{ws}/access/pending/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 404, "a plain member finds nothing to delete there");
    assert_eq!(
        e.n("SELECT count(*) FROM workspace_pending_access").await,
        1
    );
    // A person who cannot see the workspace at all: 404.
    e.who("sub-mallory", "mallory@example.com", true);
    let mallory = e.sign_in().await.unwrap();
    let (s, _) = e.add(&mallory, ws, "x@example.com", "member").await;
    assert_eq!(s, 404);
}

// No route gives a workspace owner who is not an organization admin a list or a search of the organization's people.
#[tokio::test]
async fn an_owner_who_is_not_an_org_admin_cannot_list_the_organizations_people() {
    let e = env().await;
    let (bob, ws) = e.bob().await;
    for uri in [
        format!("/v1/orgs/{}/members", e.org),
        format!("/v1/orgs/{}/invitations", e.org),
    ] {
        let (s, _) = e.call(&bob, Method::GET, &uri, None).await;
        assert!(matches!(s, 403 | 404), "{uri}: {s}");
    }
    // The answer of a by-email add says only whether that one address was added or is waiting.
    e.who("sub-nina", "nina@example.com", true);
    e.sign_in().await.unwrap();
    let (_, added) = e.add(&bob, ws, "nina@example.com", "member").await;
    let (_, waiting) = e.add(&bob, ws, "someone@example.com", "member").await;
    assert_eq!(added.as_object().unwrap().len(), 2);
    assert_eq!(waiting.as_object().unwrap().len(), 2);
}

// The rate limits: the `invite` class per principal, and 30 per hour per workspace across principals.
#[tokio::test]
async fn by_email_has_its_own_class_and_a_per_workspace_cap() {
    let e = env_with(|c| {
        c.ratelimit.api.multiplier = 1.0;
        c.ratelimit.api.classes.insert(
            "invite".into(),
            RateClassOverride {
                per_minute: Some(1),
                per_hour: None,
                burst: Some(5),
            },
        );
    })
    .await;
    let (bob, ws) = e.bob().await;
    for i in 0..5 {
        assert_eq!(
            e.add(&bob, ws, &format!("a{i}@example.com"), "member")
                .await
                .0,
            200,
            "{i}"
        );
    }
    let (s, v) = e.add(&bob, ws, "a5@example.com", "member").await;
    assert_eq!(
        (s, v["error"]["details"]["class"].as_str()),
        (429, Some("invite")),
        "{v}"
    );
}

#[tokio::test]
async fn the_per_workspace_cap_is_thirty_an_hour() {
    let e = env_with(|c| {
        c.ratelimit.api.multiplier = 1.0;
        c.ratelimit.api.classes.insert(
            "invite".into(),
            RateClassOverride {
                per_minute: Some(6000),
                per_hour: None,
                burst: Some(6000),
            },
        );
    })
    .await;
    let (bob, ws) = e.bob().await;
    let mut last = (0, Value::Null);
    for i in 0..31 {
        last = e
            .add(&bob, ws, &format!("b{i}@example.com"), "member")
            .await;
        if i < 30 {
            assert_eq!(last.0, 200, "{i}: {}", last.1);
        }
    }
    assert_eq!(
        (last.0, last.1["error"]["details"]["scope"].as_str()),
        (429, Some("workspace")),
        "{}",
        last.1
    );
}
