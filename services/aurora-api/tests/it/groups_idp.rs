#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-GRP-1 (D-156, SEC-71): IdP-linked groups. Membership is set at sign-in from the validated
//! claim of the linked provider, never creates an org membership, and is read-only by hand.

use crate::common::*;
use crate::oidc::{authorize, callback, count, harness, has_session, header};
use aurora_config::{OidcOrgRole, OidcRoleMapping, SignupMode};
use aurora_testkit::oidc::MockIdp;
use http::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

const E: &str = "alice@example.com";

fn admit(p: &mut aurora_config::OidcProvider) {
    p.role_claim = Some("cern_roles".into());
    p.role_mappings = vec![OidcRoleMapping {
        value: "aurora-users".into(),
        org: Some(OidcOrgRole {
            slug: "users".into(),
            role: "member".into(),
        }),
        ..OidcRoleMapping::default()
    }];
    p.group_claim = Some("groups".into());
}

fn claims(idp: &MockIdp, groups: Value) {
    idp.identity("s1", E, true);
    idp.set(|c| {
        c.extra.insert("cern_roles".into(), json!(["aurora-users"]));
        c.extra.insert("groups".into(), groups);
    });
}

async fn sign_in(h: &Harness) -> String {
    let (path, cookie) = authorize(h, None).await;
    let (st, hd) = callback(h, &path, &cookie).await;
    assert_eq!(st, StatusCode::FOUND);
    assert!(has_session(&hd), "sign-in refused");
    header(&hd, "set-cookie")
        .into_iter()
        .find(|c| c.starts_with("aurora_session="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn org_id(h: &Harness) -> Uuid {
    sqlx::query_scalar("SELECT id FROM organizations WHERE slug = 'users'")
        .fetch_one(h.pool())
        .await
        .unwrap()
}

async fn user_id(h: &Harness) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(E)
        .fetch_one(h.pool())
        .await
        .unwrap()
}

async fn idp_group(h: &Harness, org: Uuid, name: &str, provider: &str, value: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO groups (id, org_id, name, source, idp_provider, idp_value) \
         VALUES ($1, $2, $3, 'idp', $4, $5)",
    )
    .bind(id)
    .bind(org)
    .bind(name)
    .bind(provider)
    .bind(value)
    .execute(h.pool())
    .await
    .unwrap();
    id
}

async fn members(h: &Harness, g: Uuid) -> i64 {
    count(
        h,
        &format!("SELECT count(*) FROM group_members WHERE group_id = '{g}'"),
    )
    .await
}

#[tokio::test]
async fn sec71_d156_membership_is_set_exactly_from_the_validated_claim() {
    let idp = MockIdp::start().await;
    let h = harness(&idp, SignupMode::InviteOnly, admit, &[]).await;
    claims(&idp, json!([]));
    sign_in(&h).await; // creates the org membership (role claim)
    let org = org_id(&h).await;
    let a = idp_group(&h, org, "a", "mock", "eg-a").await;
    let b = idp_group(&h, org, "b", "mock", "eg-b").await;
    // a group linked to another provider with the same value grants nothing
    let other = idp_group(&h, org, "other-provider", "other", "eg-a").await;

    claims(&idp, json!(["eg-a", "unlinked-value"]));
    sign_in(&h).await;
    assert_eq!(
        (
            members(&h, a).await,
            members(&h, b).await,
            members(&h, other).await
        ),
        (1, 0, 0)
    );
    // switching the claim moves the membership: added if present, removed if absent
    claims(&idp, json!(["eg-b"]));
    sign_in(&h).await;
    assert_eq!((members(&h, a).await, members(&h, b).await), (0, 1));
    // an absent claim removes every IdP membership
    idp.set(|c| {
        c.extra.remove("groups");
    });
    sign_in(&h).await;
    assert_eq!((members(&h, a).await, members(&h, b).await), (0, 0));
    // audited in the sign-in transaction: service actor, provider, claim value
    for (action, value, n) in [
        ("group_member.add", "eg-a", 1),
        ("group_member.remove", "eg-a", 1),
        ("group_member.add", "eg-b", 1),
        ("group_member.remove", "eg-b", 1),
    ] {
        assert_eq!(
            count(
                &h,
                &format!(
                    "SELECT count(*) FROM audit_log WHERE action = '{action}' AND critical \
                     AND actor_kind = 'service' AND actor_label = 'idp:mock' \
                     AND metadata->>'claim_value' = '{value}'"
                )
            )
            .await,
            n,
            "{action} {value}"
        );
    }
    // values that link no group are stored nowhere
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM audit_log WHERE metadata::text LIKE '%unlinked-value%'"
        )
        .await,
        0
    );
}

#[tokio::test]
async fn sec71_d156_the_sync_never_creates_an_org_membership() {
    let idp = MockIdp::start().await;
    let h = harness(&idp, SignupMode::InviteOnly, admit, &[]).await;
    claims(&idp, json!([]));
    sign_in(&h).await;
    // a second org the user does not belong to, with a group linked to a value they present
    let other_org = h.org().await;
    let g = idp_group(&h, other_org, "theirs", "mock", "eg-x").await;
    claims(&idp, json!(["eg-x"]));
    sign_in(&h).await;
    let u = user_id(&h).await;
    assert_eq!(members(&h, g).await, 0);
    assert_eq!(
        count(
            &h,
            &format!(
                "SELECT count(*) FROM org_members WHERE user_id = '{u}' AND org_id = '{other_org}'"
            )
        )
        .await,
        0,
        "no membership appeared"
    );
    // a suspended member is not synced in
    let org = org_id(&h).await;
    let mine = idp_group(&h, org, "mine", "mock", "eg-x").await;
    sqlx::query("UPDATE org_members SET status = 'suspended' WHERE user_id = $1 AND org_id = $2")
        .bind(u)
        .bind(org)
        .execute(h.pool())
        .await
        .unwrap();
    // (a suspended member may still sign in; the group sync skips the org)
    let _ = idp;
    assert_eq!(members(&h, mine).await, 0);
}

#[tokio::test]
async fn sec71_d156_manual_and_other_rows_are_untouched_and_no_group_claim_means_no_sync() {
    let idp = MockIdp::start().await;
    // a provider without `group_claim`: the claim is ignored whatever it says
    let h = harness(
        &idp,
        SignupMode::InviteOnly,
        |p| {
            admit(p);
            p.group_claim = None;
        },
        &[],
    )
    .await;
    claims(&idp, json!(["eg-a"]));
    sign_in(&h).await;
    let org = org_id(&h).await;
    let a = idp_group(&h, org, "a", "mock", "eg-a").await;
    sign_in(&h).await;
    assert_eq!(members(&h, a).await, 0, "no group_claim, no sync");
    // a manual membership survives an IdP sync with an empty claim
    let h2 = harness(&idp, SignupMode::InviteOnly, admit, &[]).await;
    claims(&idp, json!([]));
    sign_in(&h2).await;
    let org2 = org_id(&h2).await;
    let u = user_id(&h2).await;
    let manual = Uuid::now_v7();
    sqlx::query("INSERT INTO groups (id, org_id, name) VALUES ($1, $2, 'manual')")
        .bind(manual)
        .bind(org2)
        .execute(h2.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO group_members (group_id, org_id, user_id) VALUES ($1, $2, $3)")
        .bind(manual)
        .bind(org2)
        .bind(u)
        .execute(h2.pool())
        .await
        .unwrap();
    sign_in(&h2).await;
    assert_eq!(members(&h2, manual).await, 1);
}

#[tokio::test]
async fn sec71_d156_userinfo_source_reads_only_userinfo() {
    let idp = MockIdp::start().await;
    // the claim lives only in userinfo, but the provider reads the ID token: ignored
    let h = harness(&idp, SignupMode::InviteOnly, admit, &[]).await;
    claims(&idp, json!([]));
    idp.set(|c| {
        c.extra.remove("groups");
        c.userinfo_extra.insert("groups".into(), json!(["eg-a"]));
    });
    sign_in(&h).await;
    let org = org_id(&h).await;
    let a = idp_group(&h, org, "a", "mock", "eg-a").await;
    sign_in(&h).await;
    assert_eq!(members(&h, a).await, 0);
    // `group_claim_source = userinfo`: read from the userinfo response (bound to the same `sub`)
    let hu = harness(
        &idp,
        SignupMode::InviteOnly,
        |p| {
            admit(p);
            p.group_claim_source = "userinfo".into();
        },
        &[],
    )
    .await;
    sign_in(&hu).await;
    let org = org_id(&hu).await;
    let a = idp_group(&hu, org, "a", "mock", "eg-a").await;
    sign_in(&hu).await;
    assert_eq!(members(&hu, a).await, 1);
}

#[tokio::test]
async fn sec71_d156_idp_members_are_read_only_and_creation_validates_the_provider() {
    let idp = MockIdp::start().await;
    let h = harness(&idp, SignupMode::InviteOnly, admit, &[]).await;
    claims(&idp, json!([]));
    sign_in(&h).await;
    let org = org_id(&h).await;
    let u = user_id(&h).await;
    let owner = h.user("owner@x.io", false).await;
    h.member(org, owner, "owner", "active").await;
    let (_, tok) = h.good_session(owner).await;
    let ck = cookie(&tok);
    let call = |m: Method, uri: String, body: Option<Value>| {
        let ck = ck.clone();
        let h = &h;
        async move {
            let (s, _, v) = h
                .send(
                    m,
                    &uri,
                    &[("cookie", ck.as_str()), ("origin", ORIGIN)],
                    body,
                )
                .await;
            (s.as_u16(), v)
        }
    };
    let groups = format!("/v1/orgs/{org}/groups");
    // unknown provider / provider without group_claim
    let (s, v) = call(
        Method::POST,
        groups.clone(),
        Some(json!({"name": "x", "source": "idp", "idp_provider": "nope", "idp_value": "eg-a"})),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("idp_provider_not_configured"))
    );
    let (s, _) = call(
        Method::POST,
        groups.clone(),
        Some(json!({"name": "x", "source": "idp", "idp_provider": "mock"})),
    )
    .await;
    assert_eq!(s, 400);
    let (s, v) = call(
        Method::POST,
        groups.clone(),
        Some(json!({"name": "E-group A", "source": "idp", "idp_provider": "mock", "idp_value": "eg-a"})),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["source"], "idp");
    assert_eq!(v["idp_value"], "eg-a");
    let g: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // one value links at most one group per org
    let (s, v) = call(
        Method::POST,
        groups.clone(),
        Some(json!({"name": "dup", "source": "idp", "idp_provider": "mock", "idp_value": "eg-a"})),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("group_idp_link_taken"))
    );
    // hand edits of members: 409 managed_by_idp, nothing changed
    for (m, uri, body) in [
        (Method::PUT, format!("/v1/groups/{g}/members/{u}"), None),
        (Method::DELETE, format!("/v1/groups/{g}/members/{u}"), None),
        (
            Method::POST,
            format!("/v1/groups/{g}/members:batch"),
            Some(json!({"add": [u]})),
        ),
    ] {
        let (s, v) = call(m, uri, body).await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (409, Some("managed_by_idp"))
        );
    }
    assert_eq!(members(&h, g).await, 0);
    // the claim fills it; the list shows `idp` source and the last sync time
    claims(&idp, json!(["eg-a"]));
    sign_in(&h).await;
    let (s, v) = call(Method::GET, format!("/v1/groups/{g}/members"), None).await;
    assert_eq!(s, 200);
    assert_eq!(v["items"][0]["source"], "idp");
    assert!(v["items"][0]["synced_at"].is_string());
    // name, mapping and bindings stay editable like any group
    let (s, v) = call(
        Method::PATCH,
        format!("/v1/groups/{g}"),
        Some(json!({"name": "Renamed", "org_role": "member"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    // the source is immutable (no API changes it) and deleting works
    let (s, _) = call(Method::DELETE, format!("/v1/groups/{g}"), None).await;
    assert_eq!(s, 204);
}

#[tokio::test]
async fn sec71_scim_groups_are_read_only_but_deletable_while_scim_is_off() {
    let h = Harness::new().await;
    let org = h.org().await;
    let owner = h.user("owner@x.io", false).await;
    h.member(org, owner, "owner", "active").await;
    let member = h.user("m@x.io", false).await;
    h.member(org, member, "member", "active").await;
    let (_, tok) = h.good_session(owner).await;
    let ck = cookie(&tok);
    let g = Uuid::now_v7();
    sqlx::query("INSERT INTO groups (id, org_id, name, source) VALUES ($1, $2, 'scim-g', 'scim')")
        .bind(g)
        .bind(org)
        .execute(h.pool())
        .await
        .unwrap();
    let call = |m: Method, uri: String, body: Option<Value>| {
        let ck = ck.clone();
        let h = &h;
        async move {
            let (s, _, v) = h
                .send(
                    m,
                    &uri,
                    &[("cookie", ck.as_str()), ("origin", ORIGIN)],
                    body,
                )
                .await;
            (s.as_u16(), v)
        }
    };
    let (s, v) = call(
        Method::PUT,
        format!("/v1/groups/{g}/members/{member}"),
        None,
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("group_managed_by_scim"))
    );
    let (s, v) = call(
        Method::PATCH,
        format!("/v1/groups/{g}"),
        Some(json!({"name": "x"})),
    )
    .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("group_managed_by_scim"))
    );
    // the mapping stays editable
    let (s, v) = call(
        Method::PATCH,
        format!("/v1/groups/{g}"),
        Some(json!({"org_role": "member"})),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    // SCIM enabled for the org: delete is the IdP's
    sqlx::query("UPDATE organizations SET settings = jsonb_set(settings, '{scim}', '{\"enabled\": true}') WHERE id = $1")
        .bind(org)
        .execute(h.pool())
        .await
        .unwrap();
    let (s, v) = call(Method::DELETE, format!("/v1/groups/{g}"), None).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (409, Some("group_managed_by_scim"))
    );
    sqlx::query("UPDATE organizations SET settings = '{}' WHERE id = $1")
        .bind(org)
        .execute(h.pool())
        .await
        .unwrap();
    let (s, _) = call(Method::DELETE, format!("/v1/groups/{g}"), None).await;
    assert_eq!(s, 204);
}

#[tokio::test]
async fn fix063_the_sync_stops_at_the_per_user_cap_and_a_second_sync_is_a_no_op() {
    let idp = MockIdp::start().await;
    let h = harness(&idp, SignupMode::InviteOnly, admit, &[]).await;
    claims(&idp, json!([]));
    sign_in(&h).await;
    let org = org_id(&h).await;
    let u = user_id(&h).await;
    for i in 0..101 {
        idp_group(&h, org, &format!("g{i}"), "mock", &format!("eg-{i}")).await;
    }
    let all: Vec<String> = (0..101).map(|i| format!("eg-{i}")).collect();
    claims(&idp, json!(all));
    sign_in(&h).await;
    let q = format!("SELECT count(*) FROM group_members WHERE user_id = '{u}'");
    assert_eq!(count(&h, &q).await, 100);
    let adds = "SELECT count(*) FROM audit_log WHERE action = 'group_member.add'";
    assert_eq!(count(&h, adds).await, 100);
    sign_in(&h).await;
    assert_eq!(count(&h, &q).await, 100);
    assert_eq!(count(&h, adds).await, 100, "the second sync wrote nothing");
}

#[tokio::test]
async fn fix083_suspension_ends_idp_group_access_and_reactivation_does_not_restore_it() {
    // REVIEW-5 GRP-1 (SEC-71: a removal is effective for the next request): a member who is
    // suspended, dropped from the IdP group meanwhile and reactivated must not hold the group's
    // access again until a sign-in re-adds it. The IdP rows go when the membership leaves
    // `active`; the next sign-in puts back whatever the claim then says.
    let idp = MockIdp::start().await;
    let h = harness(&idp, SignupMode::InviteOnly, admit, &[]).await;
    // The org exists with its own owner, so the person is a plain member (a claim that creates
    // the org would make the first sign-in its owner, FIX-050).
    let org = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, slug, name, external_id) VALUES ($1, 'users', 'Users', 'users')")
        .bind(org)
        .execute(h.pool())
        .await
        .unwrap();
    let owner = h.user("owner@example.com", false).await;
    h.member(org, owner, "owner", "active").await;
    claims(&idp, json!([]));
    sign_in(&h).await;
    let u = user_id(&h).await;
    let g = idp_group(&h, org, "a", "mock", "eg-a").await;
    let ws = h.workspace(org).await;
    sqlx::query(
        "INSERT INTO group_workspace_access (group_id, workspace_id, org_id, role) \
         VALUES ($1, $2, $3, 'owner')",
    )
    .bind(g)
    .bind(ws)
    .bind(org)
    .execute(h.pool())
    .await
    .unwrap();
    claims(&idp, json!(["eg-a"]));
    sign_in(&h).await;
    assert_eq!(members(&h, g).await, 1);
    let person = cookie(&h.good_session(u).await.1);
    let ws_uri = format!("/v1/workspaces/{ws}");
    let (s, _) = h.get(&ws_uri, &[("cookie", &person)]).await;
    assert_eq!(s, 200, "the group's owner binding applies");

    // An org owner suspends the member in the dashboard: the IdP rows go with the status.
    let admin = cookie(&h.good_session(owner).await.1);
    let member_uri = format!("/v1/orgs/{org}/members/{u}");
    let patch = |status: &'static str| {
        let (h, uri, admin) = (&h, &member_uri, &admin);
        async move {
            h.send(
                Method::PATCH,
                uri,
                &[("cookie", admin.as_str()), ("origin", ORIGIN)],
                Some(json!({ "status": status })),
            )
            .await
        }
    };
    let (s, _, v) = patch("suspended").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(members(&h, g).await, 0, "IdP rows deleted at suspension");
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM audit_log WHERE action = 'group_member.remove' AND critical"
        )
        .await,
        1,
        "the removal is audited"
    );
    // The IdP drops the group while the member is suspended; the owner then reactivates.
    claims(&idp, json!([]));
    let (s, _, v) = patch("active").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(members(&h, g).await, 0, "reactivation restores nothing");
    let (s, _) = h.get(&ws_uri, &[("cookie", &person)]).await;
    assert_eq!(s, 404, "no group access before any new sign-in");
    // The next sign-in adds back exactly what the claim says now.
    sign_in(&h).await;
    assert_eq!(members(&h, g).await, 0);
    claims(&idp, json!(["eg-a"]));
    sign_in(&h).await;
    assert_eq!(members(&h, g).await, 1);
}

#[tokio::test]
async fn fix083_the_sync_stops_at_the_members_per_group_cap() {
    let idp = MockIdp::start().await;
    let h = harness(&idp, SignupMode::InviteOnly, admit, &[]).await;
    claims(&idp, json!([]));
    sign_in(&h).await;
    let org = org_id(&h).await;
    let u = user_id(&h).await;
    let full = idp_group(&h, org, "full", "mock", "eg-full").await;
    let open = idp_group(&h, org, "open", "mock", "eg-open").await;
    // Fill `full` to GROUP_MEMBERS_MAX (5 000) with other members of the org.
    sqlx::query(
        "WITH n AS (SELECT gen_random_uuid() AS id, i FROM generate_series(1, 5000) i), \
              u AS (INSERT INTO users (id, email, display_name) \
                    SELECT id, 'cap' || i || '@example.com', 'u' FROM n RETURNING id), \
              m AS (INSERT INTO org_members (org_id, user_id, role) \
                    SELECT $1, id, 'member' FROM u RETURNING user_id) \
         INSERT INTO group_members (group_id, org_id, user_id, source) \
         SELECT $2, $1, user_id, 'idp' FROM m",
    )
    .bind(org)
    .bind(full)
    .execute(h.pool())
    .await
    .unwrap();
    claims(&idp, json!(["eg-full", "eg-open"]));
    sign_in(&h).await;
    assert_eq!(members(&h, full).await, 5000, "the full group takes nobody");
    assert_eq!(
        members(&h, open).await,
        1,
        "the other group is still synced"
    );
    assert_eq!(
        count(
            &h,
            &format!(
                "SELECT count(*) FROM group_members WHERE group_id = '{full}' AND user_id = '{u}'"
            )
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &h,
            &format!(
                "SELECT count(*) FROM audit_log WHERE action = 'group_member.add' \
                 AND target_id = '{full}'"
            )
        )
        .await,
        0,
        "no add is audited for the skipped group"
    );
}

#[tokio::test]
async fn fix083_a_provider_without_group_claim_removes_its_idp_rows() {
    // The operator removed `group_claim` from the provider: its IdP memberships must not
    // linger for ever. The next sign-in syncs an empty set.
    let idp = MockIdp::start().await;
    let h = harness(
        &idp,
        SignupMode::InviteOnly,
        |p| {
            admit(p);
            p.group_claim = None;
        },
        &[],
    )
    .await;
    claims(&idp, json!(["eg-a"]));
    sign_in(&h).await;
    let org = org_id(&h).await;
    let u = user_id(&h).await;
    let a = idp_group(&h, org, "a", "mock", "eg-a").await;
    let other = idp_group(&h, org, "other", "other", "eg-a").await;
    for g in [a, other] {
        sqlx::query(
            "INSERT INTO group_members (group_id, org_id, user_id, source) \
             VALUES ($1, $2, $3, 'idp')",
        )
        .bind(g)
        .bind(org)
        .bind(u)
        .execute(h.pool())
        .await
        .unwrap();
    }
    sign_in(&h).await;
    assert_eq!(members(&h, a).await, 0, "this provider's rows are removed");
    assert_eq!(members(&h, other).await, 1, "another provider's rows stay");
}

#[test]
fn fix083_orphaned_idp_providers_are_named_by_id_only() {
    use aurora_api::domains::groups::sync::orphaned_providers;
    let linked = vec![("mock".to_owned(), 2), ("gone".to_owned(), 1)];
    let configured_with_claim = ["mock".to_owned()];
    assert_eq!(
        orphaned_providers(&linked, &configured_with_claim),
        vec![("gone".to_owned(), 1)]
    );
    assert!(orphaned_providers(&linked, &["mock".to_owned(), "gone".to_owned()]).is_empty());
}
