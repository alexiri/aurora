#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-IDP-1 (D-139, D-143, SEC-78): roles from IdP claims, against the in-process mock IdP.

use crate::common::Harness;
use crate::oidc::{authorize, callback, count, harness, has_session, header, location};
use aurora_config::{
    OidcOrgRole, OidcProvider, OidcRoleMapping, OidcWorkspaceRole, Redacted, SignupMode,
};
use aurora_testkit::oidc::{CLIENT_SECRET, MockIdp};
use http::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

fn map(value: &str, f: impl FnOnce(&mut OidcRoleMapping)) -> OidcRoleMapping {
    let mut m = OidcRoleMapping {
        value: value.into(),
        ..OidcRoleMapping::default()
    };
    f(&mut m);
    m
}

fn org(slug: &str, role: &str) -> Option<OidcOrgRole> {
    Some(OidcOrgRole {
        slug: slug.into(),
        role: role.into(),
    })
}

fn mappings() -> Vec<OidcRoleMapping> {
    vec![
        map("aurora-admins", |m| m.instance_admin = true),
        map("aurora-users", |m| m.org = org("users", "member")),
        map("aurora-staff", |m| m.org = org("users", "admin")),
        map("it-ai-team", |m| {
            m.org = org("it", "owner");
            m.workspaces = vec![
                OidcWorkspaceRole {
                    slug: "infra".into(),
                    role: "owner".into(),
                },
                OidcWorkspaceRole {
                    slug: "research".into(),
                    role: "approver".into(),
                },
            ];
        }),
    ]
}

fn roles_provider(p: &mut OidcProvider) {
    p.role_claim = Some("cern_roles".into());
    p.role_mappings = mappings();
}

async fn h(idp: &MockIdp, mode: SignupMode, require: bool) -> Harness {
    harness(
        idp,
        mode,
        move |p| {
            roles_provider(p);
            p.require_role = require;
        },
        &[],
    )
    .await
}

fn set_roles(idp: &MockIdp, email: &str, sub: &str, roles: Value) {
    idp.identity(sub, email, true);
    idp.set(|c| {
        c.extra.insert("cern_roles".into(), roles);
    });
}

/// Sign in; `Ok(session cookie)` or `Err(error code)`.
async fn sign_in(h: &Harness) -> Result<String, String> {
    let (path, cookie) = authorize(h, None).await;
    let (st, hd) = callback(h, &path, &cookie).await;
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

async fn user_id(h: &Harness, email: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(h.pool())
        .await
        .unwrap()
}

async fn is_admin(h: &Harness, email: &str) -> bool {
    sqlx::query_scalar("SELECT is_instance_admin FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(h.pool())
        .await
        .unwrap()
}

async fn org_role(h: &Harness, slug: &str, email: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT m.role FROM org_members m JOIN organizations o ON o.id = m.org_id
         JOIN users u ON u.id = m.user_id WHERE o.slug = $1 AND u.email = $2",
    )
    .bind(slug)
    .bind(email)
    .fetch_optional(h.pool())
    .await
    .unwrap()
}

async fn ws_role(h: &Harness, org: &str, ws: &str, email: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT a.role FROM workspace_access a JOIN workspaces w ON w.id = a.workspace_id
         JOIN organizations o ON o.id = w.org_id JOIN users u ON u.id = a.user_id
         WHERE o.slug = $1 AND w.slug = $2 AND u.email = $3",
    )
    .bind(org)
    .bind(ws)
    .bind(email)
    .fetch_optional(h.pool())
    .await
    .unwrap()
}

const E: &str = "boss@example.com";

#[tokio::test]
async fn sec78_mapped_values_grant_each_target_and_are_audited() {
    let idp = MockIdp::start().await;
    set_roles(
        &idp,
        E,
        "s1",
        json!(["aurora-admins", "it-ai-team", "other"]),
    );
    // invite_only: the mapped claim admits a new user (D-143).
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    sign_in(&h).await.expect("admitted");
    assert!(is_admin(&h, E).await);
    assert_eq!(org_role(&h, "it", E).await.as_deref(), Some("owner"));
    assert_eq!(
        ws_role(&h, "it", "infra", E).await.as_deref(),
        Some("owner")
    );
    assert_eq!(
        ws_role(&h, "it", "research", E).await.as_deref(),
        Some("approver")
    );
    // The org was created with its default workspace and default agent; the mapping, not the
    // creation, decides the user's workspace roles (no bonus owner binding on `default`).
    assert_eq!(
        count(&h, "SELECT count(*) FROM agents a JOIN workspaces w ON w.id = a.workspace_id JOIN organizations o ON o.id = w.org_id WHERE o.slug = 'it' AND w.slug = 'default'").await,
        1
    );
    assert_eq!(ws_role(&h, "it", "default", E).await, None);
    // Audit: service actor, provider label, claim value; instance admin grant and admission.
    for (action, value) in [
        ("instance_admin.grant", "aurora-admins"),
        ("member.add", "it-ai-team"),
        ("workspace_access.set", "it-ai-team"),
    ] {
        assert!(
            count(
                &h,
                &format!(
                    "SELECT count(*) FROM audit_log WHERE action = '{action}' AND actor_kind = 'service'
                     AND actor_label = 'idp:mock' AND metadata->>'claim_value' = '{value}'"
                )
            )
            .await
                >= 1,
            "{action}"
        );
    }
    let adm = sqlx::query_scalar::<_, Value>(
        "SELECT metadata FROM audit_log WHERE action = 'user.admit_by_claim'",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(adm["provider"], "mock");
    assert_eq!(adm["claim_values"], json!(["aurora-admins", "it-ai-team"]));
    // The claim debugger: all values (also the unmapped one) and what was granted.
    let c = sign_in(&h).await.unwrap();
    let (_, me) = h.get("/v1/me", &[("cookie", &c)]).await;
    assert_eq!(
        me["idp_roles"]["values"],
        json!(["aurora-admins", "it-ai-team", "other"])
    );
    assert_eq!(me["idp_roles"]["granted"][0]["value"], "aurora-admins");
    assert_eq!(me["idp_roles"]["granted"][1]["org"]["role"], "owner");
}

#[tokio::test]
async fn sec78_unmapped_and_differently_cased_values_grant_nothing() {
    let idp = MockIdp::start().await;
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    // D-143 (5): unmapped, absent, differently cased or padded values never admit.
    for roles in [
        json!(["unknown"]),
        json!(["AURORA-ADMINS"]),
        json!(["Aurora-Admins", "aurora-admins "]),
        json!([]),
    ] {
        set_roles(&idp, "x@example.com", "x1", roles);
        assert_eq!(sign_in(&h).await.unwrap_err(), "signup_closed");
    }
    idp.identity("x1", "x@example.com", true);
    idp.set(|c| c.extra.clear());
    assert_eq!(
        sign_in(&h).await.unwrap_err(),
        "signup_closed",
        "absent claim"
    );
    assert_eq!(count(&h, "SELECT count(*) FROM users").await, 0);
    // An existing user (open mode) with such values gains nothing.
    let h2 = h2_open(&idp).await;
    set_roles(
        &idp,
        "y@example.com",
        "y1",
        json!(["AURORA-ADMINS", "Aurora-Users"]),
    );
    sign_in(&h2).await.unwrap();
    assert!(!is_admin(&h2, "y@example.com").await);
    assert_eq!(org_role(&h2, "users", "y@example.com").await, None);
}

async fn h2_open(idp: &MockIdp) -> Harness {
    h(idp, SignupMode::Open, false).await
}

#[tokio::test]
async fn sec78_a_mapping_never_bypasses_the_other_sign_in_checks() {
    let idp = MockIdp::start().await;
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    // D-143 (2): an unverified e-mail is refused even with a mapped value.
    idp.identity("u1", "u@example.com", false);
    idp.set(|c| {
        c.extra
            .insert("cern_roles".into(), json!(["aurora-admins"]));
    });
    assert_eq!(sign_in(&h).await.unwrap_err(), "email_unverified");
    assert_eq!(count(&h, "SELECT count(*) FROM users").await, 0);
    // allowed_email_domains still applies.
    let h3 = harness(
        &idp,
        SignupMode::InviteOnly,
        |p| {
            roles_provider(p);
            p.allowed_email_domains = vec!["corp.example".into()];
        },
        &[],
    )
    .await;
    set_roles(&idp, "u@example.com", "u1", json!(["aurora-admins"]));
    assert_eq!(sign_in(&h3).await.unwrap_err(), "email_domain_not_allowed");
    // A disabled user is refused with a mapped value.
    set_roles(&idp, "d@example.com", "d1", json!(["aurora-users"]));
    sign_in(&h).await.unwrap();
    sqlx::query("UPDATE users SET disabled_at = now() WHERE email = 'd@example.com'")
        .execute(h.pool())
        .await
        .unwrap();
    assert_eq!(sign_in(&h).await.unwrap_err(), "user_disabled");
}

#[tokio::test]
async fn sec78_a_value_mapped_under_another_provider_grants_nothing() {
    let idp = MockIdp::start().await;
    let p = {
        let mut p = OidcProvider {
            id: "mock".into(),
            issuer: idp.issuer().into(),
            client_id: aurora_testkit::oidc::CLIENT_ID.into(),
            client_secret_env: "MOCK_SECRET".into(),
            role_claim: Some("cern_roles".into()),
            // This provider maps only `local-value`.
            role_mappings: vec![map("local-value", |m| m.org = org("users", "member"))],
            ..OidcProvider::default()
        };
        p.display_name = Some("Mock".into());
        p
    };
    let other = OidcProvider {
        id: "other".into(),
        role_claim: Some("cern_roles".into()),
        role_mappings: vec![map("aurora-admins", |m| m.instance_admin = true)],
        ..p.clone()
    };
    let h = Harness::with_config(move |c| {
        c.signup_mode = SignupMode::InviteOnly;
        c.auth.oidc_providers = vec![p, other];
        c.oidc_client_secrets
            .insert("mock".into(), Redacted::new(CLIENT_SECRET.into()));
        c.oidc_client_secrets
            .insert("other".into(), Redacted::new(CLIENT_SECRET.into()));
    })
    .await;
    set_roles(&idp, E, "s1", json!(["aurora-admins"]));
    assert_eq!(sign_in(&h).await.unwrap_err(), "signup_closed");
    assert_eq!(count(&h, "SELECT count(*) FROM users").await, 0);
    // The provider's own value admits.
    set_roles(&idp, E, "s1", json!(["local-value"]));
    sign_in(&h).await.unwrap();
    assert!(!is_admin(&h, E).await);
    // (FIX-050) The effective role is `owner` (the floor), the claim role stays `member`.
    let (role, idp_role, _) = member_row(&h, "users", E).await;
    assert_eq!(role, "owner");
    assert_eq!(idp_role.as_deref(), Some("member"));
}

#[tokio::test]
async fn sec78_a_claim_outside_the_validated_id_token_grants_nothing() {
    let idp = MockIdp::start().await;
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    // The role claim exists only in userinfo, but the provider reads the ID token.
    idp.identity("s1", E, true);
    idp.set(|c| {
        c.userinfo_extra
            .insert("cern_roles".into(), json!(["aurora-admins"]));
    });
    assert_eq!(sign_in(&h).await.unwrap_err(), "signup_closed");
    // With `role_claim_source = userinfo` the same claim is read (fetched with the flow's token).
    let hu = harness(
        &idp,
        SignupMode::InviteOnly,
        |p| {
            roles_provider(p);
            p.role_claim_source = "userinfo".into();
        },
        &[],
    )
    .await;
    sign_in(&hu).await.expect("userinfo roles");
    assert!(is_admin(&hu, E).await);
}

#[tokio::test]
async fn sec78_removal_drops_only_claim_sourced_rows_and_manual_roles_stay() {
    let idp = MockIdp::start().await;
    set_roles(
        &idp,
        E,
        "s1",
        json!(["aurora-admins", "aurora-users", "it-ai-team"]),
    );
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    sign_in(&h).await.unwrap();
    let uid = user_id(&h, E).await;
    // A second owner keeps `it` ownable when the claim goes away.
    let it: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE slug = 'it'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let other_owner = h.user("o2@example.com", false).await;
    h.member(it, other_owner, "owner", "active").await;
    // Manual roles of the same person: another org, and a manual binding on `users`' default ws.
    let manual = h.org().await;
    h.member(manual, uid, "member", "active").await;
    let users_org: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE slug = 'users'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let users_ws: Uuid =
        sqlx::query_scalar("SELECT id FROM workspaces WHERE org_id = $1 AND slug = 'default'")
            .bind(users_org)
            .fetch_one(h.pool())
            .await
            .unwrap();
    h.bind_ws(users_ws, uid, "member").await;
    // Manual instance-admin flag of someone else is untouched by this person's sync.
    let ia = h.user("ia@example.com", true).await;
    assert!(is_admin(&h, E).await);
    // (FIX-050) The claim-created `users` org would have ended the sign-in ownerless, so the
    // floor promoted E to owner (the claim role `member` is retained on the row).
    assert_eq!(org_role(&h, "users", E).await.as_deref(), Some("owner"));

    // Next sign-in: only `aurora-users` remains.
    set_roles(&idp, E, "s1", json!(["aurora-users"]));
    sign_in(&h).await.unwrap();
    assert!(!is_admin(&h, E).await, "admin value gone");
    assert_eq!(
        org_role(&h, "it", E).await,
        None,
        "claim-only membership removed"
    );
    assert_eq!(ws_role(&h, "it", "infra", E).await, None);
    assert_eq!(
        org_role(&h, "users", E).await.as_deref(),
        Some("owner"),
        "the demotion is blocked: E is the last owner the floor placed"
    );
    assert_eq!(
        ws_role(&h, "users", "default", E).await.as_deref(),
        Some("member"),
        "manual binding kept"
    );
    assert!(
        count(&h, &format!("SELECT count(*) FROM org_members WHERE org_id = '{manual}' AND user_id = '{uid}' AND role = 'member'")).await == 1,
        "manual membership kept"
    );
    assert!(
        count(
            &h,
            &format!("SELECT count(*) FROM users WHERE id = '{ia}' AND is_instance_admin")
        )
        .await
            == 1
    );
    assert!(
        count(&h, "SELECT count(*) FROM audit_log WHERE action = 'instance_admin.revoke' AND actor_label = 'idp:mock'").await == 1
    );

    // Everything gone: the claim-created `users` membership loses its owner role? No — the
    // floor placed E as its last owner, so the removal is blocked and the row stays claim-
    // managed; the manual binding's org membership was claim-created and `manual` is ownerless
    // without it... `manual` is a dashboard org (its creator keeps ownership), so the removal
    // runs there. The `users` membership stays (blocked).
    set_roles(&idp, E, "s1", json!([]));
    sign_in(&h).await.unwrap();
    assert_eq!(
        org_role(&h, "users", E).await.as_deref(),
        Some("owner"),
        "the floor's last owner is kept (idp_sync_blocked)"
    );
    assert_eq!(org_role(&h, "manual", E).await, None);
    assert!(
        count(
            &h,
            &format!(
                "SELECT count(*) FROM org_members WHERE org_id = '{manual}' AND user_id = '{uid}'"
            )
        )
        .await
            == 1
    );
}

/// FIX-140 item 2 (FIX-108 marker): a membership created by role-claim sync clears the admin
/// removal marker, like every other `org_members` insert (docs/04 §JIT step 4, docs/03). Only
/// JIT consults the marker; an IdP claim is the provider's explicit grant.
#[tokio::test]
async fn fix140_a_claim_created_membership_clears_the_removal_marker() {
    let idp = MockIdp::start().await;
    set_roles(&idp, E, "s1", json!(["aurora-users", "it-ai-team"]));
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    sign_in(&h).await.unwrap();
    let uid = user_id(&h, E).await;
    let it: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE slug = 'it'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    // A second owner keeps `it` ownable when the claim goes away.
    let other_owner = h.user("o2@example.com", false).await;
    h.member(it, other_owner, "owner", "active").await;
    // The claim goes: the claim-only membership is dropped; an admin had removed the person
    // before (the marker), which JIT would respect.
    set_roles(&idp, E, "s1", json!(["aurora-users"]));
    sign_in(&h).await.unwrap();
    assert_eq!(org_role(&h, "it", E).await, None);
    sqlx::query("INSERT INTO org_member_removals (org_id, user_id) VALUES ($1, $2)")
        .bind(it)
        .bind(uid)
        .execute(h.pool())
        .await
        .unwrap();
    // The claim comes back: the membership is created again and the marker is gone.
    set_roles(&idp, E, "s1", json!(["aurora-users", "it-ai-team"]));
    sign_in(&h).await.unwrap();
    assert_eq!(org_role(&h, "it", E).await.as_deref(), Some("owner"));
    assert_eq!(
        count(
            &h,
            &format!("SELECT count(*) FROM org_member_removals WHERE org_id = '{it}' AND user_id = '{uid}'")
        )
        .await,
        0
    );
}

#[tokio::test]
async fn sec78_a_claim_raises_a_manual_role_and_losing_it_restores_the_manual_one() {
    let idp = MockIdp::start().await;
    set_roles(&idp, E, "s1", json!(["aurora-users"]));
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    sign_in(&h).await.unwrap();
    // A second owner keeps the scenario away from the last-owner guard.
    let users: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE slug = 'users'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let keeper = h.user("keeper@example.com", false).await;
    h.member(users, keeper, "owner", "active").await;
    // Give the person a manual `member` role (as an invitation would have created it): the
    // floor's owner role is reset to the claim's role and the claim marking is detached.
    let uid = user_id(&h, E).await;
    sqlx::query(
        "UPDATE org_members SET role = 'member', idp_role = NULL, idp_source = NULL, \
         base_role = NULL WHERE user_id = $1",
    )
    .bind(uid)
    .execute(h.pool())
    .await
    .unwrap();
    set_roles(&idp, E, "s1", json!(["aurora-staff"]));
    sign_in(&h).await.unwrap();
    assert_eq!(org_role(&h, "users", E).await.as_deref(), Some("admin"));
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM org_members WHERE base_role = 'member' AND idp_role = 'admin'"
        )
        .await,
        1
    );
    set_roles(&idp, E, "s1", json!([]));
    sign_in(&h).await.unwrap();
    assert_eq!(
        org_role(&h, "users", E).await.as_deref(),
        Some("member"),
        "the manual role is restored, not removed"
    );
}

#[tokio::test]
async fn sec78_the_last_owner_is_kept_and_the_block_is_audited() {
    let idp = MockIdp::start().await;
    set_roles(&idp, E, "s1", json!(["it-ai-team"]));
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    sign_in(&h).await.unwrap();
    assert_eq!(org_role(&h, "it", E).await.as_deref(), Some("owner"));
    set_roles(&idp, E, "s1", json!([]));
    sign_in(&h).await.unwrap();
    assert_eq!(
        org_role(&h, "it", E).await.as_deref(),
        Some("owner"),
        "the last owner is never removed by a sync"
    );
    assert_eq!(
        count(&h, "SELECT count(*) FROM audit_log WHERE action = 'org_role.idp_sync_blocked' AND actor_label = 'idp:mock'").await,
        1
    );
    // Still claim-managed (the next sync retries): the dashboard cannot edit or remove it.
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM org_members WHERE idp_role = 'owner'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn sec78_require_role_refuses_with_no_user_row() {
    let idp = MockIdp::start().await;
    let h = h(&idp, SignupMode::Open, true).await;
    set_roles(&idp, "n@example.com", "n1", json!(["unmapped"]));
    assert_eq!(sign_in(&h).await.unwrap_err(), "no_mapped_role");
    idp.identity("n1", "n@example.com", true);
    idp.set(|c| c.extra.clear());
    assert_eq!(sign_in(&h).await.unwrap_err(), "no_mapped_role");
    assert_eq!(count(&h, "SELECT count(*) FROM users").await, 0);
    assert_eq!(count(&h, "SELECT count(*) FROM user_identities").await, 0);
    assert_eq!(count(&h, "SELECT count(*) FROM organizations").await, 0);
    // The denied login's audit row is best-effort and written after the answer (CI run 44823250244 read 0): wait, bounded.
    let denied = "SELECT count(*) FROM audit_log WHERE action = 'user.login' AND outcome = 'denied' AND metadata->>'reason' = 'no_mapped_role'";
    let mut rows = 0;
    for _ in 0..100 {
        rows = count(&h, denied).await;
        if rows >= 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(rows >= 1, "no denied login was audited");
    // A mapped value signs in.
    set_roles(&idp, "n@example.com", "n1", json!(["aurora-users"]));
    sign_in(&h).await.unwrap();
}

#[tokio::test]
async fn sec78_non_string_claim_shapes_are_ignored() {
    let idp = MockIdp::start().await;
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    for roles in [
        json!(5),
        json!({"aurora-admins": true}),
        json!(true),
        json!([1, 2, null]),
        json!([["aurora-admins"]]),
    ] {
        set_roles(&idp, E, "s1", roles);
        assert_eq!(sign_in(&h).await.unwrap_err(), "signup_closed");
    }
    // A single string with space- or comma-separated values is accepted.
    set_roles(&idp, E, "s1", json!("aurora-users,other aurora-admins"));
    sign_in(&h).await.unwrap();
    assert!(is_admin(&h, E).await);
}

#[tokio::test]
async fn sec78_dashboard_cannot_edit_claim_roles_and_no_route_writes_mappings() {
    let idp = MockIdp::start().await;
    set_roles(&idp, E, "s1", json!(["aurora-admins", "aurora-staff"]));
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    let boss = sign_in(&h).await.unwrap();
    let boss_id = user_id(&h, E).await;
    // A second instance admin (manual) and org owner of `users`.
    let owner = h.user("own@example.com", true).await;
    let users_org: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE slug = 'users'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    h.member(users_org, owner, "owner", "active").await;
    let (_, tok) = h.good_session(owner).await;
    let ck = format!("aurora_session={tok}");
    let hdr = [("cookie", ck.as_str()), ("origin", crate::common::ORIGIN)];

    // Admin › Users: the claim-derived flag cannot be changed (409), a manual one can.
    let (st, _, v) = h
        .send(
            Method::PATCH,
            &format!("/v1/admin/users/{boss_id}"),
            &hdr,
            Some(json!({"is_instance_admin": false})),
        )
        .await;
    assert_eq!(
        (st, v["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("managed_by_idp")),
        "{v}"
    );
    let (_, users) = h.get("/v1/admin/users", &hdr).await;
    let u = users["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["email"] == E)
        .unwrap();
    assert_eq!(u["instance_admin_idp_value"], "aurora-admins");
    assert_eq!(
        u["idp_roles"]["values"],
        json!(["aurora-admins", "aurora-staff"])
    );
    // Org members: the claim role cannot be edited or removed.
    let (st, _, v) = h
        .send(
            Method::PATCH,
            &format!("/v1/orgs/{users_org}/members/{boss_id}"),
            &hdr,
            Some(json!({"role": "member"})),
        )
        .await;
    assert_eq!(
        (st, v["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("managed_by_idp")),
        "{v}"
    );
    let (st, _, v) = h
        .send(
            Method::DELETE,
            &format!("/v1/orgs/{users_org}/members/{boss_id}"),
            &hdr,
            None,
        )
        .await;
    assert_eq!(
        (st, v["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("managed_by_idp")),
        "{v}"
    );
    let (_, list) = h.get(&format!("/v1/orgs/{users_org}/members"), &hdr).await;
    let m = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["email"] == E)
        .unwrap();
    assert_eq!(m["idp_value"], "aurora-staff");
    // SET-4: `GET /v1/admin/identity` is gone (Admin › Sign-in replaces the Identity page).
    let (st, _) = h.get("/v1/admin/identity", &hdr).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let _ = boss;
    let plain = h.user("plain@example.com", false).await;
    let (_, t2) = h.good_session(plain).await;
    let (st, _) = h
        .get(
            "/v1/admin/settings/sign_in",
            &[("cookie", &format!("aurora_session={t2}"))],
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_listed_admin_keeps_the_flag_when_the_claim_goes() {
    let idp = MockIdp::start().await;
    set_roles(&idp, E, "s1", json!(["aurora-admins"]));
    let h = harness(&idp, SignupMode::InviteOnly, roles_provider, &[E]).await;
    sign_in(&h).await.unwrap();
    set_roles(&idp, E, "s1", json!([]));
    sign_in(&h).await.unwrap();
    assert!(
        is_admin(&h, E).await,
        "auth.instance_admins is a manual source"
    );
}

/// The dev IdP emits a configurable roles claim, so tests and e2e can drive roles end to end:
/// admin value → admin; value removed → gone at the next sign-in; a manual role stays.
#[tokio::test]
async fn dev_idp_roles_claim_drives_admin_and_removal_keeps_manual_roles() {
    let h = Harness::with_config(|c| {
        c.signup_mode = SignupMode::InviteOnly;
        for p in &mut c.auth.oidc_providers {
            if p.id == aurora_config::DEV_IDP_PROVIDER_ID {
                roles_provider(p);
            }
        }
    })
    .await;
    let email = "dev-boss@example.com";
    h.idp.set_user_claims(
        email,
        json!({"cern_roles": ["aurora-admins", "aurora-users"]})
            .as_object()
            .unwrap()
            .clone(),
    );
    let s = h
        .try_sign_in("dev", email)
        .await
        .expect("admitted by claim");
    let (st, _) = h.get("/v1/admin/overview", &[("cookie", &s.cookie)]).await;
    assert_eq!(st, StatusCode::OK, "admin pages reachable");
    // A manual role of the same person on an org of its own.
    let manual = h.org().await;
    h.member(manual, s.user_id, "member", "active").await;
    h.idp.set_user_claims(
        email,
        json!({"cern_roles": ["aurora-users"]})
            .as_object()
            .unwrap()
            .clone(),
    );
    let s = h.try_sign_in("dev", email).await.unwrap();
    let (st, _) = h.get("/v1/admin/overview", &[("cookie", &s.cookie)]).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "admin gone at the next sign-in");
    assert_eq!(
        count(
            &h,
            &format!(
                "SELECT count(*) FROM org_members WHERE org_id = '{manual}' AND user_id = '{}'",
                s.user_id
            )
        )
        .await,
        1,
        "manual role kept"
    );
}

#[tokio::test]
async fn d118_trusted_domains_still_verify_alongside_role_claims() {
    let idp = MockIdp::start().await;
    idp.identity("t1", E, true);
    idp.set(|c| {
        c.email_verified = None;
        c.extra.insert("cern_roles".into(), json!(["aurora-users"]));
    });
    let h = harness(
        &idp,
        SignupMode::InviteOnly,
        |p| {
            roles_provider(p);
            p.trusted_email_domains = vec!["example.com".into()];
        },
        &[],
    )
    .await;
    sign_in(&h)
        .await
        .expect("trusted domain verifies the address");
    // (FIX-050) Effective role `owner` via the floor; the claim role stays `member`.
    let (role, idp_role, _) = member_row(&h, "users", E).await;
    assert_eq!(role, "owner");
    assert_eq!(idp_role.as_deref(), Some("member"));
}

// ---- FIX-050 (D-165, C-630): the owner-integrity floor -----------------------------------------

async fn member_row(
    h: &Harness,
    slug: &str,
    email: &str,
) -> (String, Option<String>, Option<String>) {
    sqlx::query_as(
        "SELECT m.role, m.idp_role, m.idp_source FROM org_members m
         JOIN organizations o ON o.id = m.org_id JOIN users u ON u.id = m.user_id
         WHERE o.slug = $1 AND u.email = $2",
    )
    .bind(slug)
    .bind(email)
    .fetch_one(h.pool())
    .await
    .unwrap()
}

/// (a) A member-only mapping claim-creates the org: the first sign-in leaves the syncing user
/// as the only owner — `role = 'owner'` with `idp_role = 'member'` retained — and the floor
/// promotion is audited. The org would otherwise have been ownerless (`ensure_org` removes the
/// bootstrap owner the mapping then does not restore), which silently refuses every gateway
/// credential of the org.
#[tokio::test]
async fn fix050_member_mapping_claim_creating_org_promotes_first_sign_in_to_owner() {
    let idp = MockIdp::start().await;
    set_roles(&idp, E, "s1", json!(["aurora-users"]));
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    sign_in(&h).await.unwrap();
    let (role, idp_role, idp_source) = member_row(&h, "users", E).await;
    assert_eq!(role, "owner", "the floor promotes the only member");
    assert_eq!(idp_role.as_deref(), Some("member"), "claim role retained");
    assert!(
        idp_source
            .as_deref()
            .is_some_and(|s| s.starts_with("idp:mock:"))
    );
    // The audit row: via idp_sync, the reason, the previous role.
    let meta: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'member.update'
         AND metadata->>'reason' = 'ownerless_org' AND actor_label = 'idp:mock'",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(meta["role"], "owner");
    assert_eq!(meta["previous_role"], "member");
    assert_eq!(meta["via"], "idp_sync");
    assert_eq!(meta["provider"], "mock");
    assert_eq!(meta["claim_value"], "aurora-users");
}

/// (b) A second member-mapping user signing in later is **not** promoted: the org has an owner.
#[tokio::test]
async fn fix050_a_second_member_is_not_promoted_when_an_owner_exists() {
    let idp = MockIdp::start().await;
    set_roles(&idp, E, "s1", json!(["aurora-users"]));
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    sign_in(&h).await.unwrap();
    set_roles(&idp, "two@example.com", "s2", json!(["aurora-users"]));
    sign_in(&h).await.unwrap();
    assert_eq!(
        org_role(&h, "users", "two@example.com").await.as_deref(),
        Some("member"),
        "an existing owner means no floor promotion"
    );
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM audit_log WHERE metadata->>'reason' = 'ownerless_org'"
        )
        .await,
        1,
        "only the first sign-in was audited"
    );
}

/// (c) A mapping granting `owner` behaves exactly as before: the claim writes the owner role
/// directly, and the floor adds no promotion and no audit noise.
#[tokio::test]
async fn fix050_owner_mapping_behaves_unchanged() {
    let idp = MockIdp::start().await;
    set_roles(&idp, E, "s1", json!(["it-ai-team"]));
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    sign_in(&h).await.unwrap();
    let (role, idp_role, _) = member_row(&h, "it", E).await;
    assert_eq!(role, "owner");
    assert_eq!(idp_role.as_deref(), Some("owner"));
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM audit_log WHERE metadata->>'reason' = 'ownerless_org'"
        )
        .await,
        0
    );
}

/// (d) The claim later granting a second owner demotes the floor owner to the claim's role:
/// the existing demotion arm runs (no `idp_sync_blocked`), the org keeps an owner either way.
#[tokio::test]
async fn fix050_a_second_claim_owner_dissolves_the_floor() {
    let idp = MockIdp::start().await;
    let h = harness(
        &idp,
        SignupMode::InviteOnly,
        |p| {
            p.role_claim = Some("cern_roles".into());
            p.role_mappings = vec![
                map("aurora-users", |m| m.org = org("users", "member")),
                map("aurora-owners", |m| m.org = org("users", "owner")),
            ];
        },
        &[],
    )
    .await;
    set_roles(&idp, E, "s1", json!(["aurora-users"]));
    sign_in(&h).await.unwrap();
    assert_eq!(org_role(&h, "users", E).await.as_deref(), Some("owner"));
    // A second owner arrives by claim: the org is no longer riding on the floor.
    set_roles(&idp, "two@example.com", "s2", json!(["aurora-owners"]));
    sign_in(&h).await.unwrap();
    assert_eq!(
        org_role(&h, "users", "two@example.com").await.as_deref(),
        Some("owner"),
        "the second owner arrives by claim"
    );
    set_roles(&idp, E, "s1", json!(["aurora-users"]));
    sign_in(&h).await.unwrap();
    assert_eq!(
        org_role(&h, "users", E).await.as_deref(),
        Some("member"),
        "the floor owner is demoted back to the claim's role"
    );
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM audit_log WHERE action = 'org_role.idp_sync_blocked'"
        )
        .await,
        0,
        "no block: an owner existed throughout"
    );
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM audit_log WHERE metadata->>'reason' = 'ownerless_org'"
        )
        .await,
        1,
        "still only the original floor promotion"
    );
}

/// (e) While the floor owner is the only owner, a demoting claim keeps them and audits
/// `org_role.idp_sync_blocked` (the last-owner guard now also protects the floor).
#[tokio::test]
async fn fix050_a_demoting_claim_on_a_sole_floor_owner_is_blocked_and_audited() {
    let idp = MockIdp::start().await;
    set_roles(&idp, E, "s1", json!(["aurora-users"]));
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    sign_in(&h).await.unwrap();
    assert_eq!(org_role(&h, "users", E).await.as_deref(), Some("owner"));
    set_roles(&idp, E, "s1", json!([]));
    sign_in(&h).await.unwrap();
    assert_eq!(
        org_role(&h, "users", E).await.as_deref(),
        Some("owner"),
        "the last owner (placed by the floor) is never removed by a sync"
    );
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM audit_log WHERE action = 'org_role.idp_sync_blocked' AND actor_label = 'idp:mock'"
        )
        .await,
        1
    );
    // Still claim-managed (the next sync retries): the dashboard cannot edit or remove it.
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM org_members m JOIN organizations o ON o.id = m.org_id
             WHERE o.slug = 'users' AND m.idp_role IS NOT NULL AND m.idp_source LIKE 'idp:mock:%'"
        )
        .await,
        1,
        "the row keeps its claim marking"
    );
}

/// (f) A suspended membership is never promoted: the floor logs and skips, no crash.
#[tokio::test]
async fn fix050_a_suspended_membership_is_not_promoted() {
    let idp = MockIdp::start().await;
    set_roles(&idp, E, "s1", json!(["aurora-users"]));
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    sign_in(&h).await.unwrap();
    // Suspend the (floor-promoted) sole owner and give the org another active owner, so the
    // suspended person's next sign-in has an ownerless org to be skipped in.
    let uid = user_id(&h, E).await;
    let users: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE slug = 'users'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE org_members SET status = 'suspended' WHERE org_id = $1 AND user_id = $2")
        .bind(users)
        .bind(uid)
        .execute(h.pool())
        .await
        .unwrap();
    let other = h.user("keep@example.com", false).await;
    h.member(users, other, "owner", "active").await;
    sqlx::query("UPDATE org_members SET status = 'suspended' WHERE org_id = $1 AND user_id = $2")
        .bind(users)
        .bind(other)
        .execute(h.pool())
        .await
        .unwrap();
    // Now the org is ownerless and the syncing user's membership is suspended: skipped, and the
    // sign-in itself succeeds.
    sign_in(&h).await.unwrap();
    let (role, status) = sqlx::query_as::<_, (String, String)>(
        "SELECT role, status FROM org_members WHERE org_id = $1 AND user_id = $2",
    )
    .bind(users)
    .bind(uid)
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(status, "suspended");
    assert_eq!(role, "owner", "the earlier promotion is untouched");
    // No second floor promotion happened for the suspended row (the audit count is unchanged
    // from the first sign-in's single promotion).
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM audit_log WHERE metadata->>'reason' = 'ownerless_org'"
        )
        .await,
        1
    );
}

/// The promotion and its audit row are one transaction: an audit failure rolls both back and
/// the sign-in fails (security-critical set, `member.update`).
#[tokio::test]
async fn fix050_floor_promotion_and_audit_are_atomic() {
    let idp = MockIdp::start().await;
    set_roles(&idp, E, "s1", json!(["aurora-users"]));
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    sqlx::query(
        "CREATE FUNCTION audit_critical_fail() RETURNS trigger AS $$ BEGIN \
         RAISE EXCEPTION 'audit down'; END $$ LANGUAGE plpgsql",
    )
    .execute(h.pool())
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER audit_critical_fail BEFORE INSERT ON audit_log FOR EACH ROW \
         WHEN (NEW.critical) EXECUTE FUNCTION audit_critical_fail()",
    )
    .execute(h.pool())
    .await
    .unwrap();
    // The sign-in fails: the critical audit insert of the floor promotion is refused and the
    // whole transaction (user, org, membership, audit) rolls back.
    assert!(sign_in(&h).await.is_err());
    sqlx::query("DROP TRIGGER audit_critical_fail ON audit_log")
        .execute(h.pool())
        .await
        .unwrap();
    assert_eq!(
        count(&h, "SELECT count(*) FROM users").await,
        0,
        "nothing persisted"
    );
    assert_eq!(count(&h, "SELECT count(*) FROM organizations").await, 0);
    // With the fault gone the same sign-in succeeds and both rows are there.
    sign_in(&h).await.unwrap();
    assert_eq!(org_role(&h, "users", E).await.as_deref(), Some("owner"));
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM audit_log WHERE metadata->>'reason' = 'ownerless_org' AND critical"
        )
        .await,
        1
    );
}

/// The admin overview names the ownerless orgs (C-630): one made ownerless by suspending its
/// owner appears with slug, member count and `created_at`; an org with an active owner does not.
#[tokio::test]
async fn fix050_admin_overview_lists_ownerless_orgs() {
    let idp = MockIdp::start().await;
    set_roles(&idp, E, "s1", json!(["aurora-admins", "aurora-users"]));
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    let c = sign_in(&h).await.unwrap();
    // `users` has an active (floor-promoted) owner: the field is absent (additive contract).
    let (st, v) = h.get("/v1/admin/overview", &[("cookie", &c)]).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(
        v.get("ownerless_orgs")
            .is_none_or(|x| x.as_array().is_none_or(Vec::is_empty)),
        "absent or empty with no ownerless org: {v}"
    );
    // Suspend the sole owner and add a suspended active member: the org becomes ownerless
    // with one (non-owner) member on record.
    let uid = user_id(&h, E).await;
    let users: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE slug = 'users'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let bystander = h.user("bystander@example.com", false).await;
    h.member(users, bystander, "member", "active").await;
    sqlx::query("UPDATE org_members SET status = 'suspended' WHERE org_id = $1 AND user_id = $2")
        .bind(users)
        .bind(uid)
        .execute(h.pool())
        .await
        .unwrap();
    let (st, v) = h.get("/v1/admin/overview", &[("cookie", &c)]).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let items = v["ownerless_orgs"].as_array().expect("ownerless_orgs");
    assert_eq!(items.len(), 1, "{v}");
    assert_eq!(items[0]["slug"], "users");
    assert_eq!(items[0]["members"], 1, "the active non-owner counts");
    assert!(items[0]["created_at"].as_str().is_some(), "{v}");
}

/// (FIX-236, verify LOW-4) When an IdP claim lowers or removes an owner's role, the sync ends
/// the person's open emergency grants **in the same transaction** (`access_lost`) — otherwise
/// the people's banner keeps announcing access the person can no longer use (reads were
/// already refused; the row stayed open until expiry and blocked a future reopen with 409).
#[tokio::test]
async fn a_lowered_or_removed_claim_ends_the_emergency_grants_as_access_lost() {
    let idp = MockIdp::start().await;
    set_roles(&idp, E, "s1", json!(["it-ai-team"]));
    let h = h(&idp, SignupMode::InviteOnly, false).await;
    let session = sign_in(&h).await.unwrap();
    assert_eq!(org_role(&h, "it", E).await.as_deref(), Some("owner"));
    // A second owner keeps the sync away from the last-owner guard.
    let it: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE slug = 'it'")
        .fetch_one(h.pool())
        .await
        .unwrap();
    let keeper = h.user("keeper@example.com", false).await;
    h.member(it, keeper, "owner", "active").await;

    // The claim owner opens emergency read access on a workspace of the org.
    let ws: Uuid = sqlx::query_scalar("SELECT id FROM workspaces WHERE org_id = $1 LIMIT 1")
        .bind(it)
        .fetch_one(h.pool())
        .await
        .unwrap();
    let (st, _, v) = h
        .send(
            Method::POST,
            &format!("/v1/workspaces/{ws}/emergency-access"),
            &[
                ("cookie", session.as_str()),
                ("origin", crate::common::ORIGIN),
            ],
            Some(json!({"reason": "opened under the claim's ownership"})),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");

    // The claim drops the org entirely: the sync removes the claim-sourced membership — the
    // ownership is lost, and the grant must end with it.
    set_roles(&idp, E, "s1", json!(["aurora-users"]));
    sign_in(&h).await.unwrap();
    assert_ne!(org_role(&h, "it", E).await.as_deref(), Some("owner"));

    let (ended, reason): (bool, Option<String>) = sqlx::query_as(
        "SELECT ended_at IS NOT NULL, end_reason FROM workspace_emergency_access \
         WHERE workspace_id = $1",
    )
    .bind(ws)
    .fetch_one(h.pool())
    .await
    .unwrap();
    assert_eq!(reason.as_deref(), Some("access_lost"), "the grant's end");
    assert!(ended, "the grant is not open any more");
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM audit_log WHERE action = 'workspace.emergency_access.end' \
             AND metadata->>'end_reason' = 'access_lost'"
        )
        .await,
        1,
        "the critical end row is in the log"
    );
    // The people's banner source: the grant is no longer announced as active.
    let (_, keeper_tok) = h.good_session(keeper).await;
    let (st, _, v) = h
        .send(
            Method::GET,
            &format!("/v1/workspaces/{ws}"),
            &[
                ("cookie", crate::common::cookie(&keeper_tok).as_str()),
                ("origin", crate::common::ORIGIN),
            ],
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["audience"]["emergency"]["active"], json!(false), "{v}");
    assert!(
        !v["audience"]["emergency"]["ended_at"].is_null(),
        "the banner shows it ended: {v}"
    );
}

/// Mark the workspace `ws_slug` (in any org) as `email`'s personal one: the sticky marker, the
/// way the placement path leaves it (FIX-097, SEC-79).
async fn make_personal(h: &Harness, email: &str, ws_slug: &str) {
    sqlx::query(
        "UPDATE workspaces w SET personal_owner_user_id = u.id, personal = true \
         FROM users u WHERE u.email = $1 AND w.slug = $2",
    )
    .bind(email)
    .bind(ws_slug)
    .execute(h.pool())
    .await
    .unwrap();
}

/// The mapping of the other tests, plus a second value that keeps the org membership while
/// dropping the workspace targets: that is how the removal half of the sync is reached without
/// losing the org (whose removal would cascade over every binding).
fn lair_provider(p: &mut OidcProvider) {
    p.role_claim = Some("cern_roles".into());
    p.role_mappings = vec![
        map("it-ai-team", |m| {
            m.org = org("it", "owner");
            m.workspaces = vec![
                OidcWorkspaceRole {
                    slug: "infra".into(),
                    role: "owner".into(),
                },
                OidcWorkspaceRole {
                    slug: "lair".into(),
                    role: "member".into(),
                },
            ];
        }),
        map("it-staff", |m| m.org = org("it", "member")),
    ];
}

/// (FIX-244) A role mapping never grants a role in a personal workspace: the other targets of
/// the same mapping still apply, the sign-in succeeds, and the owner's own binding is a no-op
/// for the sync.
#[tokio::test]
async fn a_mapping_never_grants_a_role_in_a_personal_workspace() {
    let idp = MockIdp::start().await;
    let h = harness(&idp, SignupMode::InviteOnly, lair_provider, &[]).await;
    // The first sign-in creates the org, both workspaces, and grants both targets.
    set_roles(&idp, E, "s1", json!(["it-ai-team"]));
    sign_in(&h).await.unwrap();
    assert_eq!(
        ws_role(&h, "it", "infra", E).await.as_deref(),
        Some("owner")
    );
    assert_eq!(
        ws_role(&h, "it", "lair", E).await.as_deref(),
        Some("member")
    );
    // `lair` becomes the owner's personal workspace.
    make_personal(&h, E, "lair").await;
    // Another claim holder signs in: the shared target applies, the personal one is skipped,
    // and the skip is not an error.
    set_roles(&idp, "tyke@example.com", "s2", json!(["it-ai-team"]));
    sign_in(&h)
        .await
        .expect("the skip must not fail the sign-in");
    assert_eq!(
        ws_role(&h, "it", "infra", "tyke@example.com")
            .await
            .as_deref(),
        Some("owner")
    );
    assert_eq!(ws_role(&h, "it", "lair", "tyke@example.com").await, None);
    // The owner signs in again with the mapping still in force: nothing changes on the
    // personal binding.
    set_roles(&idp, E, "s1", json!(["it-ai-team"]));
    sign_in(&h).await.unwrap();
    assert_eq!(
        ws_role(&h, "it", "lair", E).await.as_deref(),
        Some("member")
    );
}

// ---- (FIX-261, D-501, SEC-161) the claim sync and a suspended binding ----

/// [`mappings`] plus: `it-members` (the `it` org only) and `it-infra-member` (the org and
/// `infra` as a plain member).
fn suspension_provider(p: &mut OidcProvider) {
    roles_provider(p);
    p.role_mappings
        .push(map("it-members", |m| m.org = org("it", "member")));
    p.role_mappings.push(map("it-infra-member", |m| {
        m.org = org("it", "member");
        m.workspaces = vec![OidcWorkspaceRole {
            slug: "infra".into(),
            role: "member".into(),
        }];
    }));
}

/// E signed in with `it-ai-team` (claim-managed `owner` of `infra`), a second owner keeping
/// `it` ownable, and a group holding E bound to `infra`.
struct Claimed {
    idp: MockIdp,
    h: Harness,
    org: Uuid,
    infra: Uuid,
    uid: Uuid,
    group: Uuid,
    /// The second `it` owner (no access to `infra` in their own right).
    other: Uuid,
}

impl Claimed {
    async fn new() -> Self {
        let idp = MockIdp::start().await;
        let h = harness(&idp, SignupMode::InviteOnly, suspension_provider, &[]).await;
        set_roles(&idp, E, "s1", json!(["it-ai-team"]));
        sign_in(&h).await.unwrap();
        let uid = user_id(&h, E).await;
        let org: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE slug = 'it'")
            .fetch_one(h.pool())
            .await
            .unwrap();
        let infra: Uuid =
            sqlx::query_scalar("SELECT id FROM workspaces WHERE org_id = $1 AND slug = 'infra'")
                .bind(org)
                .fetch_one(h.pool())
                .await
                .unwrap();
        let other = h.user("o2@example.com", false).await;
        h.member(org, other, "owner", "active").await;
        let group = Uuid::now_v7();
        sqlx::query("INSERT INTO groups (id, org_id, name) VALUES ($1, $2, 'side-door')")
            .bind(group)
            .bind(org)
            .execute(h.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO group_workspace_access (group_id, workspace_id, org_id, role) \
             VALUES ($1, $2, $3, 'member')",
        )
        .bind(group)
        .bind(infra)
        .bind(org)
        .execute(h.pool())
        .await
        .unwrap();
        let this = Claimed {
            idp,
            h,
            org,
            infra,
            uid,
            group,
            other,
        };
        this.join_group().await;
        this
    }

    async fn join_group(&self) {
        sqlx::query("INSERT INTO group_members (group_id, org_id, user_id) VALUES ($1, $2, $3)")
            .bind(self.group)
            .bind(self.org)
            .bind(self.uid)
            .execute(self.h.pool())
            .await
            .unwrap();
    }

    async fn group_memberships(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM group_members WHERE user_id = $1")
            .bind(self.uid)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    async fn suspend(&self) {
        sqlx::query(
            "UPDATE workspace_access SET status = 'suspended' \
             WHERE workspace_id = $1 AND user_id = $2",
        )
        .bind(self.infra)
        .bind(self.uid)
        .execute(self.h.pool())
        .await
        .unwrap();
    }

    /// Suspend E's membership of `it` (as an administrator's suspension leaves the row).
    async fn suspend_membership(&self) {
        self.h
            .set_member_status(self.org, self.uid, "suspended")
            .await;
    }

    /// E's `it` membership: `(role, status, claim-managed)`.
    async fn membership(&self) -> Option<(String, String, bool)> {
        sqlx::query_as(
            "SELECT role, status, idp_source IS NOT NULL FROM org_members \
             WHERE org_id = $1 AND user_id = $2",
        )
        .bind(self.org)
        .bind(self.uid)
        .fetch_optional(self.h.pool())
        .await
        .unwrap()
    }

    async fn org_audits(&self, action: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action = $1 AND org_id = $2 \
             AND target_id = $3::text AND actor_label = 'idp:mock'",
        )
        .bind(action)
        .bind(self.org)
        .bind(self.uid)
        .fetch_one(self.h.pool())
        .await
        .unwrap()
    }

    /// A call on E's `infra` binding by the second `it` owner, with a session.
    async fn as_other(&self, method: Method, body: Option<Value>) -> (u16, Value) {
        let (_, token) = self.h.good_session(self.other).await;
        self.h
            .call(
                &crate::common::cookie(&token),
                method,
                &format!("/v1/workspaces/{}/access/{}", self.infra, self.uid),
                body,
            )
            .await
    }

    async fn sign_in_with(&self, roles: Value) {
        set_roles(&self.idp, E, "s1", roles);
        sign_in(&self.h).await.unwrap();
    }

    /// E's `infra` row: `(role, status, claim-managed)`.
    async fn row(&self) -> Option<(String, String, bool)> {
        sqlx::query_as(
            "SELECT role, status, idp_source IS NOT NULL FROM workspace_access \
             WHERE workspace_id = $1 AND user_id = $2",
        )
        .bind(self.infra)
        .bind(self.uid)
        .fetch_optional(self.h.pool())
        .await
        .unwrap()
    }

    async fn can_use(&self) -> bool {
        aurora_db::repo::gateway::can_use(self.h.pool(), self.infra, self.uid)
            .await
            .unwrap()
    }

    async fn infra_audits(&self, action: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = $1 AND workspace_id = $2")
            .bind(action)
            .bind(self.infra)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }
}

fn row(role: &str, status: &str, managed: bool) -> Option<(String, String, bool)> {
    Some((role.into(), status.into(), managed))
}

/// (FIX-256, D-492) The lapse of a claim that created a suspended binding leaves an ordinary
/// manual suspended row, as `member` (there is no base role to go back to).
#[tokio::test]
async fn a_lapsed_claim_never_removes_a_suspended_binding() {
    let c = Claimed::new().await;
    c.suspend().await;
    assert!(!c.can_use().await);
    let sets = c.infra_audits("workspace_access.set").await;
    c.sign_in_with(json!(["it-members"])).await;
    assert_eq!(c.row().await, row("member", "suspended", false));
    assert_eq!(c.group_memberships().await, 1, "still in the bound group");
    assert!(!c.can_use().await, "the group's access must not come back");
    assert_eq!(c.infra_audits("workspace_access.remove").await, 0);
    assert_eq!(c.infra_audits("workspace_access.set").await, sets + 1);
}

/// (FIX-256) Once manual, a person restores it, and it comes back as `member`, never as the
/// lapsed claim's `owner`.
#[tokio::test]
async fn a_restored_claim_created_owner_comes_back_as_member() {
    let c = Claimed::new().await;
    c.suspend().await;
    c.sign_in_with(json!(["it-members"])).await;
    let (s, v) = c
        .as_other(Method::PATCH, Some(json!({"status": "active"})))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(c.row().await, row("member", "active", false));
}

/// (FIX-256) Once manual, removing it follows the rules of lifting a suspension (D-501)
/// instead of answering 409 `managed_by_idp`.
#[tokio::test]
async fn a_lapsed_suspended_binding_can_be_removed() {
    let c = Claimed::new().await;
    c.suspend().await;
    c.sign_in_with(json!(["it-members"])).await;
    let (s, v) = c.as_other(Method::DELETE, None).await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(c.row().await, None);
}

/// An active claim-managed binding goes with its claim, as before.
#[tokio::test]
async fn a_lapsed_claim_still_removes_an_active_binding() {
    let c = Claimed::new().await;
    c.sign_in_with(json!(["it-members"])).await;
    assert_eq!(c.row().await, None);
    assert_eq!(c.infra_audits("workspace_access.remove").await, 1);
}

/// A claim that raised a manual role lapses: the suspended binding goes back to that role and
/// is manual again, still suspended (FIX-256, D-492).
#[tokio::test]
async fn a_lapsed_claim_leaves_a_suspended_binding_at_its_base_role() {
    let c = Claimed::new().await;
    sqlx::query(
        "UPDATE workspace_access SET base_role = 'approver' \
         WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(c.infra)
    .bind(c.uid)
    .execute(c.h.pool())
    .await
    .unwrap();
    c.suspend().await;
    c.sign_in_with(json!(["it-members"])).await;
    assert_eq!(c.row().await, row("approver", "suspended", false));
    assert!(!c.can_use().await);
}

/// (FIX-256, D-492) The claim behind a **suspended** membership lapses and returns: it stays a
/// suspended membership, and no workspace access comes back. Once lapsed it is a manual row.
#[tokio::test]
async fn a_suspended_membership_survives_its_claim_lapsing_and_returning() {
    let c = Claimed::new().await;
    c.suspend_membership().await;
    assert_eq!(c.membership().await, row("owner", "suspended", true));
    let updates = c.org_audits("member.update").await;

    c.sign_in_with(json!([])).await;
    assert_eq!(c.membership().await, row("member", "suspended", false));
    assert_eq!(c.org_audits("member.remove").await, 0);
    assert_eq!(c.org_audits("member.update").await, updates + 1);
    assert_eq!(c.group_memberships().await, 1, "never left the org");
    assert!(!c.can_use().await);

    c.sign_in_with(json!(["it-ai-team"])).await;
    let (_, status, _) = c.membership().await.expect("still a member row");
    assert_eq!(status, "suspended", "a returning claim reactivates nothing");
    assert_eq!(
        c.org_audits("member.add").await,
        1,
        "only the first sign-in"
    );
    assert!(!c.can_use().await, "no workspace access comes back");
}

/// (FIX-256) A claim that raised a manual org role lapses on a suspended membership: back to
/// that role, manual, still suspended.
#[tokio::test]
async fn a_suspended_membership_goes_back_to_its_base_role() {
    let c = Claimed::new().await;
    sqlx::query("UPDATE org_members SET base_role = 'admin' WHERE org_id = $1 AND user_id = $2")
        .bind(c.org)
        .bind(c.uid)
        .execute(c.h.pool())
        .await
        .unwrap();
    c.suspend_membership().await;
    c.sign_in_with(json!([])).await;
    assert_eq!(c.membership().await, row("admin", "suspended", false));
}

/// An active claim-managed membership goes with its claim, as before.
#[tokio::test]
async fn an_active_membership_still_goes_with_its_claim() {
    let c = Claimed::new().await;
    c.sign_in_with(json!([])).await;
    assert_eq!(c.membership().await, None);
    assert_eq!(c.org_audits("member.remove").await, 1);
}

/// A claim that now maps a weaker role: the suspended binding keeps its role; an active one is
/// lowered as before.
#[tokio::test]
async fn a_weaker_claim_lowers_only_an_active_binding() {
    let c = Claimed::new().await;
    c.suspend().await;
    c.sign_in_with(json!(["it-infra-member"])).await;
    assert_eq!(c.row().await, row("owner", "suspended", true));

    let c = Claimed::new().await;
    c.sign_in_with(json!(["it-infra-member"])).await;
    assert_eq!(c.row().await, row("member", "active", true));
}

/// The org claim lapses (the membership and the group memberships go) and comes back: the
/// suspension is still there, so neither the claim nor the group gives access again.
#[tokio::test]
async fn losing_and_regaining_the_org_claim_keeps_the_suspension() {
    let c = Claimed::new().await;
    c.suspend().await;
    c.sign_in_with(json!([])).await;
    assert_eq!(org_role(&c.h, "it", E).await, None, "left the org");
    assert_eq!(c.group_memberships().await, 0, "left every group");
    assert_eq!(c.row().await, row("member", "suspended", false));

    c.sign_in_with(json!(["it-ai-team"])).await;
    assert!(org_role(&c.h, "it", E).await.is_some(), "re-joined");
    c.join_group().await;
    assert_eq!(c.row().await, row("owner", "suspended", true));
    assert!(!c.can_use().await, "access must not come back by itself");
}

/// (FIX-244) A claim-sourced binding in a personal workspace that predated the rule is never
/// unbound or lowered by the sync — neither the owner's nor anybody else's.
#[tokio::test]
async fn a_personal_workspaces_claim_sourced_binding_is_never_unbound_or_lowered() {
    let idp = MockIdp::start().await;
    let h = harness(&idp, SignupMode::InviteOnly, lair_provider, &[]).await;
    // Both sign in while `lair` is still an ordinary workspace: both get claim-sourced bindings.
    set_roles(&idp, E, "s1", json!(["it-ai-team"]));
    sign_in(&h).await.unwrap();
    set_roles(&idp, "tyke@example.com", "s2", json!(["it-ai-team"]));
    sign_in(&h).await.unwrap();
    assert_eq!(
        ws_role(&h, "it", "lair", "tyke@example.com")
            .await
            .as_deref(),
        Some("member")
    );
    // `lair` becomes the owner's personal workspace; then the claims drop the value that
    // carried the workspace targets (the org stays, mapped by the second value).
    make_personal(&h, E, "lair").await;
    set_roles(&idp, E, "s1", json!(["it-staff"]));
    sign_in(&h).await.unwrap();
    set_roles(&idp, "tyke@example.com", "s2", json!(["it-staff"]));
    sign_in(&h).await.unwrap();
    // The shared target follows the claim down ...
    assert_eq!(ws_role(&h, "it", "infra", E).await, None);
    assert_eq!(ws_role(&h, "it", "infra", "tyke@example.com").await, None);
    // ... and the personal workspace's rows are exactly as they were, claim source and all.
    assert_eq!(
        ws_role(&h, "it", "lair", E).await.as_deref(),
        Some("member")
    );
    assert_eq!(
        ws_role(&h, "it", "lair", "tyke@example.com")
            .await
            .as_deref(),
        Some("member")
    );
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM workspace_access a \
             JOIN workspaces w ON w.id = a.workspace_id \
             WHERE w.slug = 'lair' AND a.idp_source IS NOT NULL"
        )
        .await,
        2
    );
    // Both are still in the org: only the workspace rows were touched.
    assert!(org_role(&h, "it", E).await.is_some());
    assert!(org_role(&h, "it", "tyke@example.com").await.is_some());
}
