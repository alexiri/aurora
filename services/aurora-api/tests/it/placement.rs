#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-ONB-1 (D-140, SEC-79): new-user placement, against the in-process mock IdP.

use crate::common::{Harness, ORIGIN, cookie};
use crate::oidc::{authorize, callback, count, harness, has_session, header, location};
use aurora_config::{OidcProvider, Redacted, SignupMode};
use aurora_testkit::oidc::{CLIENT_ID, CLIENT_SECRET, MockIdp};
use http::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

const ADMIN: &str = "admin@example.com";

struct Env {
    h: Harness,
    idp: MockIdp,
    admin: String,
}

async fn env(mode: SignupMode) -> Env {
    let idp = MockIdp::start().await;
    let h = harness(&idp, mode, |_| {}, &[]).await;
    h.unsave_placement().await;
    let admin = h.user(ADMIN, true).await;
    let (_, tok) = h.good_session(admin).await;
    Env {
        h,
        idp,
        admin: cookie(&tok),
    }
}

impl Env {
    async fn call(&self, who: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", who), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }

    async fn put(&self, body: Value) -> (u16, Value) {
        self.call(&self.admin, Method::PUT, "/v1/admin/placement", Some(body))
            .await
    }

    /// Save a `shared_org` placement that creates org *Users*; returns its id.
    async fn shared(&self, extra: Value) -> Uuid {
        let mut b = json!({
            "mode": "shared_org", "create_default_org": "Users",
            "personal_workspace": {"enabled": true, "name_template": "{sub}|{email_local}"}
        });
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        let (s, v) = self.put(b).await;
        assert_eq!(s, 200, "{v}");
        v["default_org"]["id"].as_str().unwrap().parse().unwrap()
    }

    /// The IdP's next sign-in: this subject, e-mail and `preferred_username`.
    fn who(&self, sub: &str, email: &str, username: Option<&str>) {
        self.idp.identity(sub, email, true);
        self.idp.set(|c| {
            c.extra.clear();
            if let Some(u) = username {
                c.extra.insert("preferred_username".into(), json!(u));
            }
        });
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

    async fn user(&self, email: &str) -> Uuid {
        sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
            .bind(email)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    /// `(slug, name, role)` of the user's personal workspace.
    async fn personal(&self, email: &str) -> Option<(String, String, Option<String>)> {
        sqlx::query_as(
            "SELECT w.slug, w.name, a.role FROM workspaces w JOIN users u ON u.id = w.personal_owner_user_id
             LEFT JOIN workspace_access a ON a.workspace_id = w.id AND a.user_id = u.id WHERE u.email = $1",
        )
        .bind(email)
        .fetch_optional(self.h.pool())
        .await
        .unwrap()
    }

    async fn org_secret(&self, org: Uuid, name: &str, kind: &str) -> Uuid {
        let (s, v) = self
            .call(
                &self.admin,
                Method::POST,
                &format!("/v1/orgs/{org}/secrets"),
                Some(json!({"name": name, "kind": kind, "host_pattern": "api.example.com", "value": "v"})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        v["id"].as_str().unwrap().parse().unwrap()
    }
}

#[tokio::test]
async fn two_users_one_org_two_personal_workspaces_with_starter_grant() {
    let e = env(SignupMode::Open).await;
    let org = e.shared(json!({})).await;
    let key = e.org_secret(org, "org-llm-key", "generic").await;
    let (s, v) = e
        .put(json!({
            "mode": "shared_org", "default_org_id": org, "default_org_role": "member",
            "personal_workspace": {"enabled": true, "role": "owner",
                "name_template": "{preferred_username}|{email_local}"},
            "starter_grants": [{"kind": "secret", "id": key}]
        }))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["starter_grants"][0]["name"], "org-llm-key");

    e.who("s-bob", "bob@example.com", Some("bobj"));
    e.sign_in().await.unwrap();
    e.who("s-ada", "ada@example.com", Some("ada"));
    e.sign_in().await.unwrap();

    // One org (Users, made by the admin), no per-user orgs.
    assert_eq!(e.n("SELECT count(*) FROM organizations").await, 1);
    for (email, slug) in [("bob@example.com", "bobj"), ("ada@example.com", "ada")] {
        let u = e.user(email).await;
        let role: String =
            sqlx::query_scalar("SELECT role FROM org_members WHERE org_id = $1 AND user_id = $2")
                .bind(org)
                .bind(u)
                .fetch_one(e.h.pool())
                .await
                .unwrap();
        assert_eq!(role, "member");
        let (s, name, wr) = e.personal(email).await.unwrap();
        assert_eq!(
            (s.as_str(), name.as_str(), wr.as_deref()),
            (slug, slug, Some("owner"))
        );
        // Default agent holding the starter grant, nothing else.
        assert_eq!(
            e.n(&format!(
                "SELECT count(*) FROM agents a JOIN workspaces w ON w.id = a.workspace_id \
                 JOIN grants g ON g.agent_id = a.id AND g.secret_id = '{key}' \
                 WHERE a.is_default AND w.slug = '{slug}' AND w.org_id = '{org}'"
            ))
            .await,
            1
        );
        let landing: Option<Uuid> =
            sqlx::query_scalar("SELECT default_org_id FROM users WHERE id = $1")
                .bind(u)
                .fetch_one(e.h.pool())
                .await
                .unwrap();
        assert_eq!(landing, Some(org));
    }
    assert_eq!(e.n("SELECT count(*) FROM grants").await, 2);

    // A second sign-in of the same user creates nothing.
    let before = (
        e.n("SELECT count(*) FROM workspaces").await,
        e.n("SELECT count(*) FROM agents").await,
        e.n("SELECT count(*) FROM grants").await,
        e.n("SELECT count(*) FROM org_members").await,
    );
    e.who("s-bob", "bob@example.com", Some("bobj"));
    e.sign_in().await.unwrap();
    let after = (
        e.n("SELECT count(*) FROM workspaces").await,
        e.n("SELECT count(*) FROM agents").await,
        e.n("SELECT count(*) FROM grants").await,
        e.n("SELECT count(*) FROM org_members").await,
    );
    assert_eq!(before, after);

    // A colliding name gets `-2`.
    e.who("s-bob2", "bobby@example.com", Some("bobj"));
    e.sign_in().await.unwrap();
    assert_eq!(e.personal("bobby@example.com").await.unwrap().0, "bobj-2");
}

#[tokio::test]
async fn sec79_collisions_never_attach_a_user_to_an_existing_workspace() {
    let e = env(SignupMode::Open).await;
    // The legacy alias template keeps its SEC-79 behaviour under an explicit save
    // (the new default names from `sub`, FIX-045).
    let org = e
        .shared(json!({"personal_workspace": {"enabled": true,
            "name_template": "{preferred_username}|{email_local}"}}))
        .await;
    // A pre-existing workspace already owns the slug.
    let (s, v) = e
        .call(
            &e.admin,
            Method::POST,
            &format!("/v1/orgs/{org}/workspaces"),
            Some(json!({"name": "bobj", "slug": "bobj"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let existing: Uuid = v["workspace"]["id"].as_str().unwrap().parse().unwrap();

    // Same preferred_username through different identities, case variants, lookalike
    // punctuation that slugs alike.
    let names = ["bobj", "BobJ", "BOBJ", "bobj!", "bobj?", "bobj "];
    let mut slugs = Vec::new();
    for (i, n) in names.iter().enumerate() {
        e.who(&format!("s{i}"), &format!("u{i}@example.com"), Some(n));
        e.sign_in().await.unwrap();
        let (slug, _, _) = e.personal(&format!("u{i}@example.com")).await.unwrap();
        slugs.push(slug);
    }
    let unique: std::collections::HashSet<_> = slugs.iter().collect();
    assert_eq!(unique.len(), names.len(), "{slugs:?}");
    assert!(slugs.iter().all(|s| s.starts_with("bobj")));
    assert!(!slugs.contains(&"bobj".to_owned()), "{slugs:?}");
    // None of them has any access to the pre-existing workspace.
    assert_eq!(
        e.n(&format!(
            "SELECT count(*) FROM workspace_access WHERE workspace_id = '{existing}' AND user_id <> (SELECT id FROM users WHERE email = '{ADMIN}')"
        ))
        .await,
        0
    );
    // Unicode lookalikes slug to something unusable here: they still get their own workspace.
    e.who("sc", "cy@example.com", Some("b\u{43e}bj"));
    e.sign_in().await.unwrap();
    let (slug, _, _) = e.personal("cy@example.com").await.unwrap();
    assert!(!slugs.contains(&slug));
    // One personal workspace per user, bound to the id.
    assert_eq!(
        e.n("SELECT count(*) FROM workspaces WHERE personal_owner_user_id IS NOT NULL")
            .await,
        7
    );
}

#[tokio::test]
async fn sec79_renames_never_move_or_duplicate_the_workspace() {
    let e = env(SignupMode::Open).await;
    let org = e.shared(json!({})).await;
    e.who("s1", "bob@example.com", Some("bobj"));
    e.sign_in().await.unwrap();
    let u = e.user("bob@example.com").await;
    let ws: Uuid =
        sqlx::query_scalar("SELECT id FROM workspaces WHERE personal_owner_user_id = $1")
            .bind(u)
            .fetch_one(e.h.pool())
            .await
            .unwrap();
    // Someone renames the workspace (slug and name) and takes the old slug for another one.
    sqlx::query("UPDATE workspaces SET slug = 'renamed', name = 'Renamed' WHERE id = $1")
        .bind(ws)
        .execute(e.h.pool())
        .await
        .unwrap();
    let other = e.h.workspace(org).await;
    sqlx::query("UPDATE workspaces SET slug = 'bobj' WHERE id = $1")
        .bind(other)
        .execute(e.h.pool())
        .await
        .unwrap();
    // The user is renamed at the IdP.
    e.who("s1", "bob@example.com", Some("robert"));
    e.sign_in().await.unwrap();
    assert_eq!(
        e.n(&format!(
            "SELECT count(*) FROM workspaces WHERE personal_owner_user_id = '{u}'"
        ))
        .await,
        1
    );
    assert_eq!(e.personal("bob@example.com").await.unwrap().0, "renamed");
    assert_eq!(
        e.n(&format!(
            "SELECT count(*) FROM workspace_access WHERE workspace_id = '{other}' AND user_id = '{u}'"
        ))
        .await,
        0
    );
}

#[tokio::test]
async fn sec79_templates_use_only_validated_claims_and_fall_back() {
    let e = env(SignupMode::Open).await;
    e.shared(json!({"personal_workspace": {"enabled": true,
        "name_template": "{claim:cern.upn}|{given_name} {family_name}|{email_local}"}}))
        .await;
    // Nested claim of the validated token.
    e.idp.identity("s1", "a@example.com", true);
    e.idp.set(|c| {
        c.extra.clear();
        c.extra.insert("cern".into(), json!({"upn": "Ada L"}));
    });
    e.sign_in().await.unwrap();
    assert_eq!(e.personal("a@example.com").await.unwrap().1, "Ada L");
    // Missing claim: next alternative.
    e.idp.identity("s2", "b@example.com", true);
    e.idp.set(|c| {
        c.extra.clear();
        c.extra.insert("given_name".into(), json!("Bo"));
        c.extra.insert("family_name".into(), json!("Jones"));
    });
    e.sign_in().await.unwrap();
    assert_eq!(e.personal("b@example.com").await.unwrap().1, "Bo Jones");
    // Nothing but the e-mail: last alternative.
    e.idp.identity("s3", "c.d@example.com", true);
    e.idp.set(|c| c.extra.clear());
    e.sign_in().await.unwrap();
    assert_eq!(e.personal("c.d@example.com").await.unwrap().1, "c.d");
    // Control characters never reach a name; a template with no usable result falls to user-<id>.
    e.shared_again(json!({"personal_workspace": {"enabled": true, "name_template": "{claim:x}"}}))
        .await;
    e.idp.identity("s4", "z@example.com", true);
    e.idp.set(|c| {
        c.extra.clear();
        c.extra.insert("x".into(), json!("\u{7}\n\t"));
    });
    e.sign_in().await.unwrap();
    let u = e.user("z@example.com").await;
    let (_, name, _) = e.personal("z@example.com").await.unwrap();
    assert_eq!(name, format!("user-{}", &u.simple().to_string()[..8]));
    // Request input cannot add variables: unknown ones are refused when saving.
    let (s, _) = e
        .put(json!({"mode": "shared_org", "create_default_org": "X",
            "personal_workspace": {"enabled": true, "name_template": "{password}"}}))
        .await;
    assert_eq!(s, 400);
}

impl Env {
    /// Save again for the existing default org.
    async fn shared_again(&self, extra: Value) {
        let org: Uuid = sqlx::query_scalar("SELECT id FROM organizations WHERE name = 'Users'")
            .fetch_one(self.h.pool())
            .await
            .unwrap();
        let mut b = json!({"mode": "shared_org", "default_org_id": org});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        let (s, v) = self.put(b).await;
        assert_eq!(s, 200, "{v}");
    }
}

#[tokio::test]
async fn sec79_starter_grants_land_only_on_the_new_default_agent_and_are_audited() {
    let e = env(SignupMode::Open).await;
    let org = e.shared(json!({})).await;
    let key = e.org_secret(org, "org-llm-key", "generic").await;
    e.shared_again(json!({"personal_workspace": {"enabled": true},
        "starter_grants": [{"kind": "secret", "id": key}]}))
        .await;
    // Another, older workspace with its own agent: never touched.
    let old_ws = e.h.workspace(org).await;
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES (gen_random_uuid(), $1, $2, 'old', 'Old', 'byo', 'medium', '')")
        .bind(org)
        .bind(old_ws)
        .execute(e.h.pool())
        .await
        .unwrap();
    e.who("s1", "bob@example.com", Some("bobj"));
    e.sign_in().await.unwrap();
    assert_eq!(e.n("SELECT count(*) FROM grants").await, 1);
    assert_eq!(
        e.n(&format!(
            "SELECT count(*) FROM grants g JOIN agents a ON a.id = g.agent_id JOIN workspaces w ON w.id = a.workspace_id \
             WHERE a.is_default AND w.personal_owner_user_id IS NOT NULL AND g.created_by IS NULL AND g.secret_id = '{key}'"
        ))
        .await,
        1
    );
    let ev: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'grant.create' AND actor_label = 'placement'",
    )
    .fetch_one(e.h.pool())
    .await
    .unwrap();
    assert_eq!(ev["starter"], true);
    assert_eq!(ev["secret_id"], json!(key));
    let kind: String = sqlx::query_scalar(
        "SELECT actor_kind FROM audit_log WHERE action = 'grant.create' AND actor_label = 'placement'",
    )
    .fetch_one(e.h.pool())
    .await
    .unwrap();
    assert_eq!(kind, "system");
    // The grant compiled a derived rule like any other grant.
    assert!(
        e.n("SELECT count(*) FROM derived_rules WHERE source_kind = 'grant'")
            .await
            >= 1
    );

    // A credential deleted since saving never blocks sign-in; the skip is audited.
    sqlx::query("DELETE FROM secrets WHERE id = $1")
        .bind(key)
        .execute(e.h.pool())
        .await
        .unwrap();
    e.who("s2", "ada@example.com", Some("ada"));
    e.sign_in().await.unwrap();
    assert!(e.personal("ada@example.com").await.is_some());
    // (Deleting the secret cascaded its first grant away; the second user got none.)
    assert_eq!(e.n("SELECT count(*) FROM grants").await, 0);
    assert_eq!(
        e.n("SELECT count(*) FROM audit_log WHERE action = 'grant.create' AND outcome = 'denied' AND actor_label = 'placement'").await,
        1
    );
}

#[tokio::test]
async fn sec79_starter_grants_must_be_org_credentials_of_the_default_org() {
    let e = env(SignupMode::Open).await;
    let org = e.shared(json!({})).await;
    let other_org = e.h.org().await;
    let adm = e.user(ADMIN).await;
    e.h.member(other_org, adm, "owner", "active").await;
    let foreign = e.org_secret(other_org, "foreign", "generic").await;
    let ws = e.h.workspace(org).await;
    let (_, v) = e
        .call(
            &e.admin,
            Method::POST,
            &format!("/v1/workspaces/{ws}/secrets"),
            Some(json!({"name": "ws-only", "kind": "generic", "host_pattern": "a.example.com", "value": "v"})),
        )
        .await;
    let ws_secret: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let base = |g: Value| {
        json!({"mode": "shared_org", "default_org_id": org,
            "personal_workspace": {"enabled": true}, "starter_grants": [g]})
    };
    for id in [foreign, ws_secret, Uuid::now_v7()] {
        let (s, v) = e.put(base(json!({"kind": "secret", "id": id}))).await;
        assert_eq!(s, 400, "{v}");
    }
    let (s, _) = e
        .put(base(json!({"kind": "connection", "id": Uuid::now_v7()})))
        .await;
    assert_eq!(s, 400);
    // Not without personal workspaces.
    let (s, _) = e
        .put(json!({"mode": "shared_org", "default_org_id": org,
            "starter_grants": [{"kind": "secret", "id": foreign}]}))
        .await;
    assert_eq!(s, 400);
    assert_eq!(
        e.n("SELECT count(*) FROM instance_placement WHERE starter_grants <> '[]'")
            .await,
        0
    );
}

#[tokio::test]
async fn sec79_placement_routes_need_an_instance_admin_and_changes_are_audited() {
    let e = env(SignupMode::Open).await;
    let u = e.h.user("plain@example.com", false).await;
    let (_, t) = e.h.good_session(u).await;
    let plain = cookie(&t);
    for (m, uri, body) in [
        (Method::GET, "/v1/admin/placement", None),
        (Method::GET, "/v1/admin/placement/preview?sample=x", None),
        (
            Method::GET,
            "/v1/admin/placement/choices?org_id=00000000-0000-0000-0000-000000000000",
            None,
        ),
        (
            Method::PUT,
            "/v1/admin/placement",
            Some(json!({"mode": "none"})),
        ),
    ] {
        let (s, _) = e.call(&plain, m.clone(), uri, body.clone()).await;
        assert_eq!(s, 403, "{m} {uri}");
        let (s, _, _) = e.h.send(m, uri, &[("origin", ORIGIN)], body).await;
        assert_eq!(s, 401, "{uri}");
    }
    assert_eq!(e.n("SELECT count(*) FROM instance_placement").await, 0);
    e.shared(json!({})).await;
    let m: Value =
        sqlx::query_scalar("SELECT metadata FROM audit_log WHERE action = 'placement.update'")
            .fetch_one(e.h.pool())
            .await
            .unwrap();
    assert_eq!(m["mode"], "shared_org");
    assert_eq!(m["personal_enabled"], true);
    let kind: String =
        sqlx::query_scalar("SELECT actor_kind FROM audit_log WHERE action = 'placement.update'")
            .fetch_one(e.h.pool())
            .await
            .unwrap();
    assert_eq!(kind, "instance_admin");
}

#[tokio::test]
async fn personal_workspaces_turned_on_later_reach_existing_members_only() {
    let e = env(SignupMode::Open).await;
    // The legacy alias template, explicit: this test pins `preferred_username` naming.
    let org = e
        .shared(json!({"personal_workspace": {"enabled": false,
            "name_template": "{preferred_username}|{email_local}"}}))
        .await;
    e.who("s1", "bob@example.com", Some("bobj"));
    e.sign_in().await.unwrap();
    assert!(e.personal("bob@example.com").await.is_none());
    // An existing user who is not in the default org is not moved.
    e.h.user("old@example.com", false).await;
    e.shared_again(
        json!({"personal_workspace": {"enabled": true, "role": "approver",
        "name_template": "{preferred_username}|{email_local}"}}),
    )
    .await;
    e.who("s1", "bob@example.com", Some("bobj"));
    e.sign_in().await.unwrap();
    let (slug, _, role) = e.personal("bob@example.com").await.unwrap();
    assert_eq!((slug.as_str(), role.as_deref()), ("bobj", Some("approver")));
    e.who("s9", "old@example.com", Some("oldie"));
    e.sign_in().await.unwrap();
    assert!(e.personal("old@example.com").await.is_none());
    assert_eq!(
        e.n(&format!(
            "SELECT count(*) FROM org_members m JOIN users u ON u.id = m.user_id WHERE m.org_id = '{org}' AND u.email = 'old@example.com'"
        ))
        .await,
        0
    );
    // A suspended member does not get one either.
    let u = e.user("bob@example.com").await;
    sqlx::query("DELETE FROM workspaces WHERE personal_owner_user_id = $1")
        .bind(u)
        .execute(e.h.pool())
        .await
        .unwrap();
    e.h.set_member_status(org, u, "suspended").await;
    e.who("s1", "bob@example.com", Some("bobj"));
    let _ = e.sign_in().await;
    assert!(e.personal("bob@example.com").await.is_none());
}

#[tokio::test]
async fn shared_workspaces_are_joined_at_creation_with_their_role() {
    let e = env(SignupMode::Open).await;
    let org = e.shared(json!({})).await;
    let (_, v) = e
        .call(
            &e.admin,
            Method::POST,
            &format!("/v1/orgs/{org}/workspaces"),
            Some(json!({"name": "General"})),
        )
        .await;
    let general: Uuid = v["workspace"]["id"].as_str().unwrap().parse().unwrap();
    e.shared_again(json!({
        "shared_workspaces": [{"workspace_id": general, "role": "approver"}],
        "default_org_role": "admin",
        "personal_workspace": {"enabled": true}
    }))
    .await;
    e.who("s1", "bob@example.com", Some("bobj"));
    e.sign_in().await.unwrap();
    let u = e.user("bob@example.com").await;
    let role: String = sqlx::query_scalar(
        "SELECT role FROM workspace_access WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(general)
    .bind(u)
    .fetch_one(e.h.pool())
    .await
    .unwrap();
    assert_eq!(role, "approver");
    let org_role: String =
        sqlx::query_scalar("SELECT role FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(org)
            .bind(u)
            .fetch_one(e.h.pool())
            .await
            .unwrap();
    assert_eq!(org_role, "admin");
    assert_eq!(
        e.n("SELECT count(*) FROM audit_log WHERE actor_label = 'placement' AND action IN ('member.add','workspace_access.set','workspace.create')").await,
        3
    );
}

#[tokio::test]
async fn validation_of_the_saved_placement() {
    let e = env(SignupMode::Open).await;
    let org = e.shared(json!({})).await;
    let other = e.h.org().await;
    let other_ws = e.h.workspace(other).await;
    let ws = e.h.workspace(org).await;
    e.h.bind_ws(ws, e.user(ADMIN).await, "owner").await;
    sqlx::query("UPDATE workspaces SET personal_owner_user_id = $2 WHERE id = $1")
        .bind(ws)
        .bind(e.user(ADMIN).await)
        .execute(e.h.pool())
        .await
        .unwrap();
    let cases = vec![
        json!({"mode": "shared_org"}),
        json!({"mode": "shared_org", "default_org_id": org, "create_default_org": "X"}),
        json!({"mode": "shared_org", "default_org_id": Uuid::now_v7()}),
        json!({"mode": "shared_org", "default_org_id": org,
            "shared_workspaces": [{"workspace_id": other_ws, "role": "member"}]}),
        json!({"mode": "shared_org", "default_org_id": org,
            "shared_workspaces": [{"workspace_id": ws, "role": "member"}]}),
        json!({"mode": "shared_org", "default_org_id": org,
            "shared_workspaces": [{"workspace_id": Uuid::now_v7(), "role": "member"}]}),
        json!({"mode": "shared_org", "default_org_id": org,
            "personal_workspace": {"enabled": true, "name_template": "{name"}}),
        json!({"mode": "shared_org", "default_org_id": org,
            "personal_workspace": {"enabled": true, "role": "boss"}}),
        json!({"mode": "personal_org", "default_org_id": org}),
        json!({"mode": "none", "personal_workspace": {"enabled": true}}),
        json!({"mode": "shared_org", "create_default_org": ""}),
        json!({"mode": "bogus"}),
    ];
    for c in cases {
        let (s, v) = e.put(c.clone()).await;
        assert!(s == 400 || s == 422, "{c} -> {s} {v}");
    }
    // Rejected requests changed nothing, and created no org.
    let (_, v) = e
        .call(&e.admin, Method::GET, "/v1/admin/placement", None)
        .await;
    assert_eq!(v["mode"], "shared_org");
    assert_eq!(
        e.n("SELECT count(*) FROM organizations WHERE name = 'X'")
            .await,
        0
    );
}

#[tokio::test]
async fn preview_renders_samples_and_the_callers_profile() {
    let e = env(SignupMode::Open).await;
    let get = |q: &str| {
        let q = q.to_owned();
        let e = &e;
        async move {
            e.call(
                &e.admin,
                Method::GET,
                &format!("/v1/admin/placement/preview?{q}"),
                None,
            )
            .await
        }
    };
    let (s, v) = get("template=%7Bpreferred_username%7D&sample=bobj").await;
    assert_eq!(
        (s, v["name"].as_str(), v["slug"].as_str()),
        (200, Some("bobj"), Some("bobj"))
    );
    let (_, v) = get("template=%7Bname%7D&sample=Ada%20Lovelace").await;
    assert_eq!(
        (v["name"].as_str(), v["slug"].as_str()),
        (Some("Ada Lovelace"), Some("ada-lovelace"))
    );
    // Without a sample: the caller's own profile (the e-mail local part here).
    let (_, v) = get("template=%7Bemail_local%7D").await;
    assert_eq!(v["name"], "admin");
    // A claim-only variable has no value for a sample: reported, next alternative used.
    let (_, v) = get("template=%7Bclaim%3Ax%7D%7C%7Bname%7D&sample=Zed").await;
    assert_eq!(
        (v["name"].as_str(), v["used"].as_u64()),
        (Some("Zed"), Some(1))
    );
    assert_eq!(v["unresolved"], json!(["claim:x"]));
    let (s, _) = get("template=%7Bnope%7D").await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn fresh_open_instance_waits_for_the_placement_unless_admin() {
    let e = env(SignupMode::Open).await;
    // The seeded admin row exists but no organization does: a fresh instance.
    e.who("s1", "bob@example.com", Some("bobj"));
    assert_eq!(e.sign_in().await.unwrap_err(), "instance_setup_pending");
    assert_eq!(
        e.n("SELECT count(*) FROM users WHERE email = 'bob@example.com'")
            .await,
        0
    );
    assert_eq!(e.n("SELECT count(*) FROM organizations").await, 0);
    // Saving the placement opens it.
    e.shared(json!({})).await;
    e.sign_in().await.unwrap();
    assert!(e.personal("bob@example.com").await.is_some());
}

#[tokio::test]
async fn fresh_open_instance_admits_a_listed_admin() {
    let idp = MockIdp::start().await;
    let h = harness(&idp, SignupMode::Open, |_| {}, &[ADMIN]).await;
    h.unsave_placement().await;
    idp.identity("s1", ADMIN, true);
    let e = Env {
        h,
        idp,
        admin: String::new(),
    };
    e.sign_in().await.unwrap();
    // Unsaved: today's behaviour (own org) for the admin.
    assert_eq!(e.n("SELECT count(*) FROM organizations").await, 1);
}

#[tokio::test]
async fn unsaved_and_personal_org_modes_keep_todays_behaviour() {
    let e = env(SignupMode::Open).await;
    // An organization exists, so the instance is no longer fresh.
    e.h.org().await;
    e.who("s1", "bob@example.com", Some("bobj"));
    e.sign_in().await.unwrap();
    assert_eq!(e.n("SELECT count(*) FROM organizations").await, 2);
    let (_, v) = e
        .call(&e.admin, Method::GET, "/v1/admin/placement", None)
        .await;
    assert_eq!(v["saved"], false);
    assert_eq!(v["mode"], "personal_org");
    assert_eq!(
        v["warnings"],
        json!(["placement_unsaved", "per_user_orgs_active"])
    );
    assert_eq!(v["personal_orgs"]["count"], 1);
    assert_eq!(
        v["personal_orgs"]["items"][0]["owner_email"],
        "bob@example.com"
    );
    let (s, v) = e.put(json!({"mode": "personal_org"})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["saved"], true);
    assert_eq!(v["warnings"], json!(["per_user_orgs_active"]));
    e.who("s2", "ada@example.com", Some("ada"));
    e.sign_in().await.unwrap();
    assert_eq!(e.n("SELECT count(*) FROM organizations").await, 3);
}

#[tokio::test]
async fn mode_none_admits_only_by_invitation_claim_or_jit() {
    let e = env(SignupMode::Open).await;
    e.h.org().await;
    let (s, _) = e.put(json!({"mode": "none"})).await;
    assert_eq!(s, 200);
    e.who("s1", "bob@example.com", Some("bobj"));
    assert_eq!(e.sign_in().await.unwrap_err(), "signup_closed");
    assert_eq!(
        e.n("SELECT count(*) FROM users WHERE email = 'bob@example.com'")
            .await,
        0
    );
    // An invitation admits (and is accepted), with no automatic placement.
    let adm = e.user(ADMIN).await;
    let org: Uuid = sqlx::query_scalar("SELECT id FROM organizations LIMIT 1")
        .fetch_one(e.h.pool())
        .await
        .unwrap();
    e.h.member(org, adm, "owner", "active").await;
    let (s, v) = e
        .call(
            &e.admin,
            Method::POST,
            &format!("/v1/orgs/{org}/invitations"),
            Some(json!({"email": "bob@example.com", "role": "member"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    e.sign_in().await.unwrap();
    assert_eq!(e.n("SELECT count(*) FROM organizations").await, 1);
    assert_eq!(
        e.n("SELECT count(*) FROM org_members WHERE role = 'member'")
            .await,
        1
    );
}

#[tokio::test]
async fn invited_users_also_get_their_personal_workspace() {
    let e = env(SignupMode::InviteOnly).await;
    // The legacy alias template, explicit: this test pins `preferred_username` naming.
    let org = e
        .shared(json!({"personal_workspace": {"enabled": true,
            "name_template": "{preferred_username}|{email_local}"}}))
        .await;
    let (s, v) = e
        .call(
            &e.admin,
            Method::POST,
            &format!("/v1/orgs/{org}/invitations"),
            Some(json!({"email": "bob@example.com", "role": "member"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    e.who("s1", "bob@example.com", Some("bobj"));
    e.sign_in().await.unwrap();
    assert_eq!(e.personal("bob@example.com").await.unwrap().0, "bobj");
    // Not invited, not mapped: still refused.
    e.who("s2", "eve@example.com", Some("eve"));
    assert_eq!(e.sign_in().await.unwrap_err(), "signup_closed");
}

#[tokio::test]
async fn choices_offer_org_level_credentials_and_never_personal_workspaces() {
    let e = env(SignupMode::Open).await;
    let org = e.shared(json!({})).await;
    let key = e.org_secret(org, "org-llm-key", "generic").await;
    let ws = e.h.workspace(org).await;
    let personal = e.h.workspace(org).await;
    sqlx::query("UPDATE workspaces SET personal_owner_user_id = $2 WHERE id = $1")
        .bind(personal)
        .bind(e.user(ADMIN).await)
        .execute(e.h.pool())
        .await
        .unwrap();
    let (s, v) = e
        .call(
            &e.admin,
            Method::GET,
            &format!("/v1/admin/placement/choices?org_id={org}"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let ids: Vec<&str> = v["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&ws.to_string().as_str()));
    assert!(!ids.contains(&personal.to_string().as_str()));
    assert_eq!(v["secrets"][0]["id"], json!(key));
    let (s, _) = e
        .call(
            &e.admin,
            Method::GET,
            &format!("/v1/admin/placement/choices?org_id={}", Uuid::now_v7()),
            None,
        )
        .await;
    assert_eq!(s, 404);
}

/// (FIX-097 6, docs/04 §Deletion semantics) Account deletion deletes the person's personal
/// workspace when they are its only member; a personal workspace somebody else is bound to
/// stays, keeps its `personal` marker after `personal_owner_user_id` is nulled, and is never
/// offered as a shared workspace again (choices) nor accepted by `PUT /v1/admin/placement`.
#[tokio::test]
async fn deleting_an_account_deletes_a_solely_used_personal_workspace_and_keeps_a_shared_one_personal()
 {
    let e = env(SignupMode::Open).await;
    let org = e.shared(json!({})).await;
    let mut cookies = Vec::new();
    for (sub, email) in [("s1", "bob@example.com"), ("s2", "cy@example.com")] {
        e.who(sub, email, None);
        cookies.push(e.sign_in().await.unwrap());
    }
    let (bob, cy) = (
        e.user("bob@example.com").await,
        e.user("cy@example.com").await,
    );
    let ws_of = |u: Uuid| {
        let pool = e.h.pool().clone();
        async move {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM workspaces WHERE personal_owner_user_id = $1",
            )
            .bind(u)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let (bws, cws) = (ws_of(bob).await, ws_of(cy).await);
    // Cy is bound to Bob's personal workspace too: it is not Bob's alone.
    e.h.bind_ws(bws, cy, "member").await;
    for u in [bob, cy] {
        sqlx::query("UPDATE sessions SET reauth_at = now() WHERE user_id = $1")
            .bind(u)
            .execute(e.h.pool())
            .await
            .unwrap();
    }
    let (s, v) = e.call(&cookies[0], Method::DELETE, "/v1/me", None).await;
    assert_eq!(s, 204, "{v}");
    let (owner, personal): (Option<Uuid>, bool) =
        sqlx::query_as("SELECT personal_owner_user_id, personal FROM workspaces WHERE id = $1")
            .bind(bws)
            .fetch_one(e.h.pool())
            .await
            .unwrap();
    assert_eq!(
        (owner, personal),
        (None, true),
        "kept, still marked personal"
    );
    let (s, v) = e
        .call(
            &e.admin,
            Method::GET,
            &format!("/v1/admin/placement/choices?org_id={org}"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(
        v["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .all(|w| w["id"] != json!(bws)),
        "a departed person's workspace is not offered: {v}"
    );
    let (s, v) = e
        .put(json!({"mode": "shared_org", "default_org_id": org,
            "shared_workspaces": [{"workspace_id": bws, "role": "member"}],
            "personal_workspace": {"enabled": true}}))
        .await;
    assert_eq!(s, 400, "cannot be saved as shared: {v}");
    // Cy's own workspace has no other member: it goes with the account.
    let (s, v) = e.call(&cookies[1], Method::DELETE, "/v1/me", None).await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(
        e.n(&format!(
            "SELECT count(*) FROM workspaces WHERE id = '{cws}'"
        ))
        .await,
        0
    );
    assert_eq!(
        e.n(&format!(
            "SELECT count(*) FROM workspaces WHERE id = '{bws}'"
        ))
        .await,
        1
    );
}

// ---- FIX-045 (D-162): the default template names workspaces from the OIDC-unique `sub` ----

#[tokio::test]
async fn fix045_default_template_names_new_workspaces_from_the_sub() {
    let e = env(SignupMode::Open).await;
    // The unsaved view and the PUT default report the new default (D-162).
    let (_, v) = e
        .call(&e.admin, Method::GET, "/v1/admin/placement", None)
        .await;
    assert_eq!(
        v["personal_workspace"]["name_template"],
        "{sub}|{email_local}"
    );
    let _org = e.shared(json!({})).await;
    // `shared` omits name_template: the saved row carries the PUT default.
    let saved: String = sqlx::query_scalar("SELECT name_template FROM instance_placement")
        .fetch_one(e.h.pool())
        .await
        .unwrap();
    assert_eq!(saved, "{sub}|{email_local}");

    // Two users with the same `preferred_username` (providers recycle aliases): their
    // workspaces are named from their distinct subs, no numbered slug.
    e.who("sub-bob-1", "bob@example.com", Some("bobj"));
    e.sign_in().await.unwrap();
    e.who("sub-bob-2", "robert@example.com", Some("bobj"));
    e.sign_in().await.unwrap();
    let (slug1, name1, _) = e.personal("bob@example.com").await.unwrap();
    let (slug2, name2, _) = e.personal("robert@example.com").await.unwrap();
    assert_eq!((slug1.as_str(), name1.as_str()), ("sub-bob-1", "sub-bob-1"));
    assert_eq!((slug2.as_str(), name2.as_str()), ("sub-bob-2", "sub-bob-2"));
    // A `sub` with characters the slug derivation trims still yields a usable slug.
    e.who("Sub.Bob@3", "bobby@example.com", Some("bobj"));
    e.sign_in().await.unwrap();
    let (slug3, name3, _) = e.personal("bobby@example.com").await.unwrap();
    assert_eq!(name3, "Sub.Bob@3");
    assert_eq!(slug3, "sub-bob-3");
    // Three distinct workspaces, no collision slug anywhere.
    assert_eq!(
        e.n("SELECT count(*) FROM workspaces WHERE personal_owner_user_id IS NOT NULL")
            .await,
        3
    );
    // The legacy alias slugs (the old default) never appear: naming comes from the subs.
    assert_eq!(
        e.n("SELECT count(*) FROM workspaces WHERE slug IN ('bobj', 'bobj-2')")
            .await,
        0
    );
}

#[tokio::test]
async fn fix045_same_alias_from_two_providers_gets_distinct_sub_named_workspaces() {
    // The SEC-79 matrix extension (D-162): two users, same `preferred_username`, different
    // providers/subs, the new default -> subs-named distinct workspaces, no numbered slug.
    // A second provider ("corp") over the same mock issuer: same alias, different sub.
    let idp = MockIdp::start().await;
    let corp_issuer = idp.issuer().to_owned();
    let h = Harness::with_config(move |c| {
        // Both providers are served by the one mock IdP; both share its issuer and client.
        c.signup_mode = SignupMode::Open;
        c.oidc_client_secrets
            .insert("mock".into(), Redacted::new(CLIENT_SECRET.into()));
        c.oidc_client_secrets
            .insert("corp".into(), Redacted::new(CLIENT_SECRET.into()));
        let base = |id: &str, secret_env: &str| OidcProvider {
            id: id.into(),
            display_name: Some("Mock SSO".into()),
            issuer: corp_issuer.clone(),
            client_id: CLIENT_ID.into(),
            client_secret_env: secret_env.into(),
            ..OidcProvider::default()
        };
        c.auth.oidc_providers = vec![
            base("mock", "MOCK_SECRET"),
            base("corp", "MOCK_CORP_SECRET"),
        ];
    })
    .await;
    h.unsave_placement().await;
    let admin = h.user(ADMIN, true).await;
    let (_, tok) = h.good_session(admin).await;
    let admin_cookie = cookie(&tok);

    // Save the placement through the API (C-521) with the new default template.
    let body = json!({
        "mode": "shared_org", "create_default_org": "Users",
        "personal_workspace": {"enabled": true, "name_template": "{sub}|{email_local}"}
    });
    let (s, _, v) = h
        .send(
            Method::PUT,
            "/v1/admin/placement",
            &[("cookie", admin_cookie.as_str()), ("origin", ORIGIN)],
            Some(body),
        )
        .await;
    assert_eq!(s.as_u16(), 200, "{v}");

    // The same alias through two providers, different subs -> two identities, two workspaces
    // named from the subs (one per provider row), no numbered slug. Each sign-in drives its
    // own start -> IdP -> callback chain, so the flow cookie always matches the provider.
    async fn sign_in_via(
        idp: &MockIdp,
        h: &Harness,
        provider: &str,
        sub: &str,
        email: &str,
        alias: &str,
    ) {
        idp_identity(idp, sub, email, alias);
        let (path, flow) = start_flow(h, provider).await;
        let (st, hd) = callback(h, &path, &flow).await;
        assert_eq!(st, StatusCode::FOUND, "{provider} callback");
        assert!(has_session(&hd), "{provider} session");
    }
    sign_in_via(&idp, &h, "mock", "s-bob-a", "bob@example.com", "bobj").await;
    sign_in_via(&idp, &h, "corp", "s-bob-b", "robert@example.com", "bobj").await;

    let row_a = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        personal_of(&h, "bob@example.com"),
    )
    .await;
    let row_b = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        personal_of(&h, "robert@example.com"),
    )
    .await;
    let (slug_a, name_a) = row_a
        .expect("personal_of(bob) timed out")
        .expect("no personal ws for bob");
    let (slug_b, name_b) = row_b
        .expect("personal_of(robert) timed out")
        .expect("no personal ws for robert");
    assert_eq!((slug_a.as_str(), name_a.as_str()), ("s-bob-a", "s-bob-a"));
    assert_eq!((slug_b.as_str(), name_b.as_str()), ("s-bob-b", "s-bob-b"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM workspaces WHERE personal_owner_user_id IS NOT NULL"
        )
        .fetch_one(h.pool())
        .await
        .unwrap(),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM user_identities WHERE provider_id = 'corp'"
        )
        .fetch_one(h.pool())
        .await
        .unwrap(),
        1
    );
}

/// Point the mock IdP's next token at `(sub, email)` with a `preferred_username` alias.
fn idp_identity(idp: &MockIdp, sub: &str, email: &str, alias: &str) {
    idp.identity(sub, email, true);
    idp.set(|c| {
        c.extra.clear();
        c.extra.insert("preferred_username".into(), json!(alias));
    });
}

/// `oidc::authorize` for any provider: `/v1/auth/oidc/{id}/start` plus the IdP step.
/// Returns `(callback path+query, __Host-aurora_oidc cookie pair)`.
async fn start_flow(h: &Harness, provider: &str) -> (String, String) {
    let uri = format!("/v1/auth/oidc/{provider}/start");
    let (st, hd, _) = h.send(Method::GET, &uri, &[], None).await;
    assert_eq!(st, StatusCode::FOUND, "start");
    let flow = header(&hd, "set-cookie")
        .into_iter()
        .find(|c| c.starts_with("__Host-aurora_oidc="))
        .expect("oidc cookie");
    let flow = flow.split(';').next().unwrap().to_owned();
    // Follow the provider's authorization URL at the mock IdP (no redirects), like
    // `oidc::authorize` does for the `mock` provider.
    let loc = header(&hd, "location").remove(0);
    assert!(loc.contains("code_challenge_method=S256") && loc.contains("nonce="));
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let res = http.get(&loc).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 303);
    let back = res.headers()["location"].to_str().unwrap().to_owned();
    let path = back
        .strip_prefix(ORIGIN)
        .expect("redirect_uri on external url")
        .to_owned();
    (path, flow)
}

/// `(slug, name)` of the personal workspace of the user with this e-mail.
async fn personal_of(h: &Harness, email: &str) -> Option<(String, String)> {
    sqlx::query_as(
        "SELECT w.slug, w.name FROM workspaces w JOIN users u ON u.id = w.personal_owner_user_id \
         WHERE u.email = $1",
    )
    .bind(email)
    .fetch_optional(h.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn fix045_preview_resolves_sub_from_the_callers_identity() {
    let e = env(SignupMode::Open).await;
    e.shared(json!({})).await;
    // The session-based admin has no identity row yet: sign in once through the dev
    // provider so `(dev, dev-admin@example.com)` is stored. Without a `login_hint` the
    // mock issues its *configured* claims, so the subject is exactly what's set here.
    e.idp.identity("dev-admin@example.com", ADMIN, true);
    e.sign_in().await.unwrap();
    let get = |q: &str| {
        let q = q.to_owned();
        let e = &e;
        async move {
            e.call(
                &e.admin,
                Method::GET,
                &format!("/v1/admin/placement/preview?{q}"),
                None,
            )
            .await
        }
    };
    // The admin's stored identity (dev provider, sub `dev-admin@example.com`); no sample.
    let (s, v) = get("").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["name"].as_str(), Some("dev-admin@example.com"));
    assert_eq!(v["unresolved"], json!([]));
    // A typed sample never invents a subject: `{sub}` is reported, the chain falls through.
    let (_, v) = get("sample=bobj").await;
    assert_eq!(v["name"].as_str(), Some("bobj"));
    assert_eq!(v["used"], 1);
    assert_eq!(v["unresolved"], json!(["sub"]));
    // `{sub}` first with nothing else: the `user-<short id>` last resort.
    let (_, v) = get("template=%7Bsub%7D&sample=bobj").await;
    let admin_id = e.user(ADMIN).await;
    assert_eq!(
        v["name"],
        format!("user-{}", &admin_id.simple().to_string()[..8])
    );
    assert_eq!(v["used"], Value::Null);
    assert_eq!(v["unresolved"], json!(["sub"]));
    // Without a sample, the saved default resolves `{sub}` from the stored row.
    let (_, v) = get("template=%7Bsub%7D%7C%7Bemail_local%7D").await;
    assert_eq!(v["name"].as_str(), Some("dev-admin@example.com"));
    // An admin with no linked identity (pre-provisioned) leaves `{sub}` unresolved.
    sqlx::query("DELETE FROM user_identities WHERE user_id = $1")
        .bind(admin_id)
        .execute(e.h.pool())
        .await
        .unwrap();
    let (_, v) = get("template=%7Bsub%7D").await;
    assert_eq!(v["used"], Value::Null);
    assert_eq!(v["unresolved"], json!(["sub"]));
}

// ---- FIX-038: a `custom_only` app's connection is never saved as a starter grant ---------

/// Package `pkg` (app `pkg_app`) with the given `grants:` line (may be empty).
fn fix038_yaml(version: &str, grants: &str) -> String {
    let host = "api.fix038.example";
    format!(
        r#"package: {{ id: pkg, version: {version}, contract: 1, min_core: 0.1.0, publisher: acme, name: P }}
apps:
  - about: {{ id: pkg_app, name: P }}
    credentials: [ {{ name: api_token, secret: true }} ]
    connect:
      - id: api_key
        kind: fields
        fields: [ {{ name: api_token, label: T, type: secret, required: true }} ]
{grants}    tools:
      read:
        items:
          - {{ id: list_items, name: List, host: "{host}", path: /items, methods: [GET] }}
    hosts:
      - match: {{ host: "{host}" }}
        inject:
          - bearer: "{{credentials.api_token}}"
"#
    )
}

// (FIX-085) `custom_only` implies `block_untooled`; the manifest says both.
const FIX038_CUSTOM_ONLY: &str = "    grants: { custom_only: true, block_untooled: true }\n";

impl Env {
    /// An org-level connection of `pkg_app`, installed from `yaml`.
    async fn fix038_connection(&self, yaml: &str) -> Uuid {
        let a = crate::plugins_lifecycle::setup(&self.h).await;
        let v = a.upload(yaml).await;
        let v = if v["status"] == "pending_approval" {
            let (s, b) = a.act(&v["id"], "approve").await;
            assert_eq!(s, 200, "{b}");
            b
        } else {
            v
        };
        assert_eq!(v["status"], "active", "{v}");
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,NULL,'pkg_app','pkg','1.0.0','cc','active','x')")
            .bind(id)
            .bind(self.org_id().await)
            .execute(self.h.pool())
            .await
            .unwrap();
        id
    }

    async fn org_id(&self) -> Uuid {
        sqlx::query_scalar("SELECT id FROM organizations WHERE name = 'Users'")
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn fix038_custom_only_app_connection_is_refused_as_a_starter_grant() {
    let e = env(SignupMode::Open).await;
    let org = e
        .shared(json!({"personal_workspace": {"enabled": true}}))
        .await;
    let conn = e
        .fix038_connection(&fix038_yaml("1.0.0", FIX038_CUSTOM_ONLY))
        .await;
    let (s, v) = e
        .put(json!({
            "mode": "shared_org", "default_org_id": org,
            "personal_workspace": {"enabled": true},
            "starter_grants": [{"kind": "connection", "id": conn}]
        }))
        .await;
    assert_eq!(s, 400, "{v}");
    assert_eq!(v["error"]["code"].as_str(), Some("invalid_request"), "{v}");
    assert_eq!(
        v["error"]["details"]["fields"][0]["path"].as_str(),
        Some("/starter_grants/0/id"),
        "{v}"
    );
    // The error names the app.
    assert!(
        v["error"]["details"]["fields"][0]["message"]
            .as_str()
            .is_some_and(|m| m.contains("pkg_app") && m.contains("custom access")),
        "{v}"
    );
    // Nothing was saved.
    assert_eq!(
        e.n("SELECT count(*) FROM instance_placement WHERE starter_grants <> '[]'")
            .await,
        0
    );
    // A secret of the same instance is unaffected (secrets have no manifest restrictions).
    let key = e.org_secret(org, "org-llm-key", "generic").await;
    let (s, _) = e
        .put(json!({
            "mode": "shared_org", "default_org_id": org,
            "personal_workspace": {"enabled": true},
            "starter_grants": [{"kind": "secret", "id": key}]
        }))
        .await;
    assert_eq!(s, 200);
}

#[tokio::test]
async fn fix038_full_mode_app_connection_is_still_saved_and_granted_at_sign_in() {
    let e = env(SignupMode::Open).await;
    let org = e
        .shared(json!({"personal_workspace": {"enabled": true}}))
        .await;
    let conn = e.fix038_connection(&fix038_yaml("1.0.0", "")).await;
    let (s, v) = e
        .put(json!({
            "mode": "shared_org", "default_org_id": org,
            "personal_workspace": {"enabled": true},
            "starter_grants": [{"kind": "connection", "id": conn}]
        }))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["starter_grants"][0]["id"], json!(conn), "{v}");

    // The saved starter row keeps working: the new user's default agent gets the full grant.
    e.who("s1", "bob@example.com", Some("bobj"));
    e.sign_in().await.unwrap();
    assert!(e.personal("bob@example.com").await.is_some());
    assert_eq!(
        e.n(&format!(
            "SELECT count(*) FROM grants g JOIN agents a ON a.id = g.agent_id \
             JOIN workspaces w ON w.id = a.workspace_id \
             WHERE a.is_default AND g.connection_id = '{conn}' AND g.mode = 'full' \
             AND w.personal_owner_user_id IS NOT NULL"
        ))
        .await,
        1
    );
    let ev: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'grant.create' AND outcome = 'success' AND actor_label = 'placement'",
    )
    .fetch_one(e.h.pool())
    .await
    .unwrap();
    assert_eq!(ev["connection_id"], json!(conn));
    assert_eq!(ev["starter"], json!(true));
    let denied = e
        .n("SELECT count(*) FROM audit_log WHERE action = 'grant.create' AND outcome = 'denied'")
        .await;
    assert_eq!(denied, 0, "no skip-and-audit for a healthy starter");
}

#[tokio::test]
async fn fix038_pre_fix_custom_only_starter_row_skips_and_audits_without_blocking_sign_in() {
    let e = env(SignupMode::Open).await;
    // The legacy alias template, explicit: the slug assertion below pins it.
    let org = e
        .shared(json!({"personal_workspace": {"enabled": true,
            "name_template": "{preferred_username}|{email_local}"}}))
        .await;
    let conn = e
        .fix038_connection(&fix038_yaml("1.0.0", FIX038_CUSTOM_ONLY))
        .await;
    // A row saved before the fix (the PUT now refuses this): write it directly.
    sqlx::query("UPDATE instance_placement SET starter_grants = $2 WHERE default_org_id = $1")
        .bind(org)
        .bind(json!([{"kind": "connection", "id": conn}]))
        .execute(e.h.pool())
        .await
        .unwrap();
    // Sign-in must not be blocked: the starter is skipped and the skip is audited with its own
    // reason (FIX-064: not `credential_unavailable`, the wrong cause FIX-038 left).
    e.who("s1", "bob@example.com", Some("bobj"));
    e.sign_in().await.unwrap();
    let (slug, _, _) = e.personal("bob@example.com").await.unwrap();
    assert_eq!(slug, "bobj");
    assert_eq!(e.n("SELECT count(*) FROM grants").await, 0);
    let ev: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'grant.create' AND outcome = 'denied' AND actor_label = 'placement'",
    )
    .fetch_one(e.h.pool())
    .await
    .unwrap();
    assert_eq!(ev["starter"], json!(true));
    assert_eq!(ev["reason"], "full_grant_forbidden");
}

/// FIX-064: a database error inside a starter grant is not "a credential that went away": it is
/// never audited as `credential_unavailable`, and a refusal before it (skipped in a savepoint)
/// does not poison the shared sign-in transaction.
#[tokio::test]
async fn fix064_a_database_error_in_a_starter_grant_is_not_reported_as_unavailable() {
    let e = env(SignupMode::Open).await;
    let org = e
        .shared(json!({"personal_workspace": {"enabled": true,
            "name_template": "{preferred_username}|{email_local}"}}))
        .await;
    // The package also carries a plain app (FIX-084, SEC-67: an app the snapshot does not hold
    // is `custom_only`, so the second starter's app must be installed for its grant to be tried).
    let plain_app = r#"  - about: { id: plain_app, name: Plain }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    hosts:
      - match: { host: "api.plain.example" }
        inject:
          - bearer: "{credentials.api_token}"
"#;
    let conn = e
        .fix038_connection(&format!(
            "{}{plain_app}",
            fix038_yaml("1.0.0", FIX038_CUSTOM_ONLY)
        ))
        .await;
    let key = Uuid::now_v7();
    sqlx::query("UPDATE instance_placement SET starter_grants = $2 WHERE default_org_id = $1")
        .bind(org)
        .bind(json!([{"kind": "connection", "id": conn}, {"kind": "connection", "id": key}]))
        .execute(e.h.pool())
        .await
        .unwrap();
    // The second starter: a live connection of a plain app whose grant insert fails in the
    // database (an injected trigger), i.e. a real `Db` error, not a refusal.
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,NULL,'plain_app','pkg','1.0.0','pc','active','x')")
        .bind(key)
        .bind(org)
        .execute(e.h.pool())
        .await
        .unwrap();
    sqlx::query(&format!(
        "CREATE FUNCTION fix064_boom() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN \
         IF NEW.connection_id = '{key}' THEN RAISE EXCEPTION 'injected'; END IF; RETURN NEW; END $$"
    ))
    .execute(e.h.pool())
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER fix064_boom BEFORE INSERT ON grants FOR EACH ROW EXECUTE FUNCTION fix064_boom()",
    )
    .execute(e.h.pool())
    .await
    .unwrap();
    e.who("s1", "bob@example.com", Some("bobj"));
    let r = e.sign_in().await;
    assert!(r.is_err(), "the real failure surfaces: {r:?}");
    let unavailable = e
        .n("SELECT count(*) FROM audit_log WHERE action = 'grant.create' AND metadata->>'reason' = 'credential_unavailable'")
        .await;
    assert_eq!(
        unavailable, 0,
        "a database error is not a vanished credential"
    );
}
