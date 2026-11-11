#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-ONB-1 (D-140, SEC-79): new-user placement, against the in-process mock IdP.

use crate::common::{Harness, ORIGIN, cookie};
use crate::oidc::{authorize, callback, count, harness, has_session, header, location};
use aurora_config::SignupMode;
use aurora_testkit::oidc::MockIdp;
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
            "personal_workspace": {"enabled": true, "name_template": "{preferred_username}|{email_local}"}
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
    let org = e.shared(json!({})).await;
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
    let org = e
        .shared(json!({"personal_workspace": {"enabled": false}}))
        .await;
    e.who("s1", "bob@example.com", Some("bobj"));
    e.sign_in().await.unwrap();
    assert!(e.personal("bob@example.com").await.is_none());
    // An existing user who is not in the default org is not moved.
    e.h.user("old@example.com", false).await;
    e.shared_again(json!({"personal_workspace": {"enabled": true, "role": "approver"}}))
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
    let org = e.shared(json!({})).await;
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

const FIX038_CUSTOM_ONLY: &str = "    grants: { custom_only: true }\n";

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
    let org = e
        .shared(json!({"personal_workspace": {"enabled": true}}))
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
    // Sign-in must not be blocked: the starter is skipped and the skip is audited with the
    // established (wrong-cause but fail-open) reason.
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
    assert_eq!(ev["reason"], "credential_unavailable");
}
