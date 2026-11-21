//! P5-SKLIB-2 (D-449; C-1128; SEC-149): instance-level skill libraries. One library managed by an
//! instance admin, used by two organizations with their own authority, numbers, proposals and
//! budgets; the authority per tier; one refresh for everyone; deletion everywhere. Child of
//! `skill_library` so it shares the world and the git server.

use super::*;

const ADMIN_LIBS: &str = "/v1/admin/skill-libraries";

/// Organization B of the two-organization world: an admin, a member bound to a workspace, an agent.
struct B {
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    admin: String,
    member: String,
}

async fn second_org(w: &W) -> B {
    let org = w.h.org().await;
    let ws = w.h.workspace(org).await;
    let admin_u = w.h.user("b-admin@x.io", false).await;
    w.h.member(org, admin_u, "admin", "active").await;
    let member_u = w.h.user("b-member@x.io", false).await;
    w.h.member(org, member_u, "member", "active").await;
    w.h.bind_ws(ws, member_u, "member").await;
    let agent = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness)
         VALUES ($1,$2,$3,'b1','b1','hosted','fake')",
    )
    .bind(agent)
    .bind(org)
    .bind(ws)
    .execute(w.h.pool())
    .await
    .unwrap();
    B {
        org,
        ws,
        agent,
        admin: cookie(&w.h.good_session(admin_u).await.1),
        member: cookie(&w.h.good_session(member_u).await.1),
    }
}

impl W {
    /// An instance library over the default repository, created by the instance admin.
    async fn instance_library(&self, label: &str) -> Uuid {
        let (s, v) = self
            .call(
                &self.root,
                Method::POST,
                ADMIN_LIBS,
                Some(json!({"label": label, "url": url("acme/skills")})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        assert_eq!(
            (v["scope"].as_str(), v["org_id"].is_null()),
            (Some("instance"), true),
            "{v}"
        );
        v["id"].as_str().unwrap().parse().unwrap()
    }

    async fn org_list(&self, cookie: &str, org: Uuid) -> Vec<Value> {
        let (s, v) = self
            .call(
                cookie,
                Method::GET,
                &format!("/v1/orgs/{org}/skill-libraries"),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v["items"].as_array().unwrap().clone()
    }

    async fn add_agent_of(&self, b: &B, lib: Uuid, path: &str, sha: &str) -> (u16, Value) {
        self.call(
            &b.member,
            Method::POST,
            &format!("/v1/agents/{}/skills", b.agent),
            Some(Self::from_library(lib, path, sha)),
        )
        .await
    }
}

fn of<'a>(items: &'a [Value], id: Uuid) -> &'a Value {
    items
        .iter()
        .find(|i| i["id"] == id.to_string())
        .unwrap_or_else(|| panic!("{id} not listed in {items:?}"))
}

#[tokio::test]
async fn two_organizations_use_one_instance_library_each_with_its_own_numbers() {
    let w = world().await;
    let sha = w.seed();
    let b = second_org(&w).await;
    // an organization library of A, to see the order of the list
    w.git.add_repo("acme/own");
    w.git
        .commit("acme/own", "main", &[put("one/SKILL.md", md("One", "b"))]);
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &format!("/v1/orgs/{}/skill-libraries", w.org),
            Some(json!({"label": "zz-own", "url": url("acme/own")})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let own: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let lib = w.instance_library("aa-shared").await;

    // both organizations list it, beside their own: the organization's libraries first, then the
    // instance's, each by label
    let a_list = w.org_list(&w.admin, w.org).await;
    let order: Vec<_> = a_list
        .iter()
        .map(|i| {
            (
                i["id"].as_str().unwrap().to_owned(),
                i["scope"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        order,
        [
            (own.to_string(), "org".to_owned()),
            (lib.to_string(), "instance".to_owned())
        ]
    );
    let b_list = w.org_list(&b.admin, b.org).await;
    assert_eq!(b_list.len(), 1, "B has no library of its own: {b_list:?}");
    assert_eq!(b_list[0]["scope"], "instance");
    assert!(b_list[0]["org_id"].is_null());
    assert_eq!(b_list[0]["url"], url("acme/skills"));
    assert_eq!(b_list[0]["token_set"], false);
    assert!(
        b_list[0]["token_updated_at"].is_null(),
        "an organization never sees when the instance's token changed"
    );
    // a member gets the picker shape only
    let picker = w.org_list(&b.member, b.org).await;
    assert_eq!(picker.len(), 1);
    for k in [
        "url",
        "repo",
        "path",
        "last_head_sha",
        "last_error",
        "skill_counts",
        "pending_updates",
        "orgs_using",
    ] {
        assert!(
            picker[0][k].is_null(),
            "{k} is not for members: {}",
            picker[0]
        );
    }
    assert_eq!(
        (
            picker[0]["label"].as_str(),
            picker[0]["status"].as_str(),
            picker[0]["scope"].as_str()
        ),
        (Some("aa-shared"), Some("active"), Some("instance"))
    );

    // A adds `alpha` at its org tier, B adds `alpha` at an agent tier of its workspace
    let (s, v) = w.add_org(lib, "alpha", &sha).await;
    assert_eq!(s, 201, "{v}");
    let a_skill: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, v) = w.add_agent_of(&b, lib, "alpha", &sha).await;
    assert_eq!(s, 201, "{v}");
    let b_skill: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // the skill remembers where it came from (label and display address)
    let (label, repo_text): (String, String) =
        sqlx::query_as("SELECT library_label, library_repo FROM skills WHERE id = $1")
            .bind(b_skill)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(
        (label.as_str(), repo_text.as_str()),
        ("aa-shared", "git.example/acme/skills")
    );

    // each organization's numbers are its own
    let a = w.org_list(&w.admin, w.org).await;
    assert_eq!(
        of(&a, lib)["skill_counts"],
        json!({"agent": 0, "workspace": 0, "org": 1})
    );
    let bl = w.org_list(&b.admin, b.org).await;
    assert_eq!(
        of(&bl, lib)["skill_counts"],
        json!({"agent": 1, "workspace": 0, "org": 0})
    );

    // browse: `added` is the caller's organization's, computed per request
    let uri = |org: Uuid| format!("/v1/skill-libraries/{lib}/skills?org={org}");
    let (s, v) = w.call(&w.admin, Method::GET, &uri(w.org), None).await;
    assert_eq!(s, 200, "{v}");
    let alpha = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["path"] == "alpha")
        .unwrap()
        .clone();
    assert_eq!(alpha["added"], json!([{"tier": "org", "id": a_skill}]));
    let (s, v) = w.call(&b.admin, Method::GET, &uri(b.org), None).await;
    assert_eq!(s, 200, "{v}");
    let alpha = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["path"] == "alpha")
        .unwrap()
        .clone();
    assert_eq!(alpha["added"], json!([{"tier": "agent", "id": b_skill}]));
    // B's member sees the agent-tier skill of a workspace they can use; A's members see nothing of B's
    let (_, v) = w.call(&b.member, Method::GET, &uri(b.org), None).await;
    let alpha = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["path"] == "alpha")
        .unwrap()
        .clone();
    assert_eq!(alpha["added"], json!([{"tier": "agent", "id": b_skill}]));
    let (_, v) = w.call(&w.member, Method::GET, &uri(w.org), None).await;
    let alpha = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["path"] == "alpha")
        .unwrap()
        .clone();
    assert_eq!(alpha["added"], json!([{"tier": "org", "id": a_skill}]));
    // browse without an organization: 400; as another organization: 404; as one's own: 200
    let (s, v) = w
        .call(
            &w.admin,
            Method::GET,
            &format!("/v1/skill-libraries/{lib}/skills"),
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (400, "invalid_request"), "{v}");
    let (s, v) = w.call(&w.admin, Method::GET, &uri(b.org), None).await;
    assert_eq!(
        (s, code(&v)),
        (404, "not_found"),
        "A's admin cannot browse as B: {v}"
    );
    let (s, _) = w.call(&w.nonmember, Method::GET, &uri(w.org), None).await;
    assert_eq!(s, 404);
    let (s, _) = w.call(&w.unbound, Method::GET, &uri(w.org), None).await;
    assert_eq!(
        s, 404,
        "a member who can use no workspace gets the same 404 for an instance library"
    );
    // the content endpoint follows the same rule
    let (s, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!(
                "/v1/skill-libraries/{lib}/skills/alpha?org={}&commit_sha={sha}",
                w.org
            ),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, _) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/skill-libraries/{lib}/skills/alpha?commit_sha={sha}"),
            None,
        )
        .await;
    assert_eq!(s, 400);

    // the admin's list: totals, `orgs_using`, no skill names
    let (s, v) = w.call(&w.root, Method::GET, ADMIN_LIBS, None).await;
    assert_eq!(s, 200, "{v}");
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "only instance libraries: {v}");
    assert_eq!(items[0]["orgs_using"], 2);
    assert_eq!(
        items[0]["skill_counts"],
        json!({"agent": 1, "workspace": 0, "org": 1})
    );
    assert!(items[0]["token_updated_at"].is_null() && items[0]["token_set"] == false);
    assert!(
        !v.to_string().contains("\"alpha\""),
        "no skill names in the admin list: {v}"
    );
    let _ = b.ws;
}

#[tokio::test]
async fn only_instance_admins_manage_an_instance_library_and_never_by_key() {
    let w = world().await;
    w.seed();
    let b = second_org(&w).await;
    let lib = w.instance_library("shared").await;
    let one = format!("/v1/skill-libraries/{lib}");
    // an organization's owner or admin cannot manage it: 404, not 403 (the id is not confirmed)
    for who in [&w.owner, &w.admin, &b.admin] {
        for (m, uri, body) in [
            (Method::PATCH, one.clone(), Some(json!({"label": "mine"}))),
            (Method::DELETE, one.clone(), None),
            (Method::POST, format!("{one}/refresh"), None),
            (Method::GET, one.clone(), None),
        ] {
            let (s, v) = w.call(who, m.clone(), &uri, body).await;
            assert_eq!((s, code(&v)), (404, "not_found"), "{m} {uri}: {v}");
        }
    }
    // an organization's owner/admin sees its own view with `?org=`, a member and a stranger do not
    let (s, v) = w
        .call(&w.admin, Method::GET, &format!("{one}?org={}", w.org), None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["scope"], "instance");
    assert!(
        v["orgs_using"].is_null() && v["token_updated_at"].is_null(),
        "{v}"
    );
    for (who, org) in [(&w.member, w.org), (&w.admin, b.org), (&w.nonmember, w.org)] {
        let (s, _) = w
            .call(who, Method::GET, &format!("{one}?org={org}"), None)
            .await;
        assert_eq!(s, 404);
    }
    // the instance admin's session manages it
    let (s, v) = w.call(&w.root, Method::GET, &one, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["orgs_using"], 0);
    let (s, v) = w
        .call(
            &w.root,
            Method::PATCH,
            &one,
            Some(json!({"label": "renamed"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["label"], "renamed");
    // an API key never manages it, not even the key of an instance admin (session only)
    let root: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = 'root@x.io'")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    w.h.member(w.org, root, "owner", "active").await;
    let (_, key) = w.h.api_key(root, w.org, None, false).await;
    let auth = bearer(&key);
    for (m, uri, body) in [
        (Method::PATCH, one.clone(), Some(json!({"label": "by-key"}))),
        (Method::DELETE, one.clone(), None),
        (Method::POST, format!("{one}/refresh"), None),
    ] {
        let (s, _, v) =
            w.h.send(m.clone(), &uri, &[("authorization", auth.as_str())], body)
                .await;
        assert_eq!((s.as_u16(), code(&v)), (404, "not_found"), "{m} {uri}: {v}");
    }
    for (m, body) in [
        (Method::GET, None),
        (
            Method::POST,
            Some(json!({"label": "k", "url": url("acme/skills")})),
        ),
    ] {
        let (s, _, v) =
            w.h.send(m, ADMIN_LIBS, &[("authorization", auth.as_str())], body)
                .await;
        assert!(
            matches!(s.as_u16(), 401 | 403),
            "an API key on an admin route: {s} {v}"
        );
    }
    assert_eq!(
        w.count("SELECT count(*) FROM skill_libraries WHERE label = 'renamed'")
            .await,
        1,
        "nothing changed by key or by an organization"
    );
    // an organization owner on the admin routes: 403
    for (m, body) in [
        (Method::GET, None),
        (
            Method::POST,
            Some(json!({"label": "o", "url": url("acme/skills")})),
        ),
    ] {
        let (s, v) = w.call(&w.owner, m, ADMIN_LIBS, body).await;
        assert_eq!((s, code(&v)), (403, "instance_admin_required"), "{v}");
    }
}

#[tokio::test]
async fn one_refresh_writes_one_proposal_per_skill_and_each_organization_decides_its_own() {
    let w = world().await;
    let sha = w.seed();
    let b = second_org(&w).await;
    let lib = w.instance_library("shared").await;
    let (_, v) = w.add_org(lib, "alpha", &sha).await;
    let a_alpha: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (_, v) = w.add_org(lib, "beta", &sha).await;
    let a_beta: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (s, v) = w.add_agent_of(&b, lib, "alpha", &sha).await;
    assert_eq!(s, 201, "{v}");
    let b_alpha: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // a push changes alpha: one pass, one proposal for A's skill and one for B's
    w.git.commit(
        "acme/skills",
        "main",
        &[put("alpha/SKILL.md", md("Alpha v2", "v2 body"))],
    );
    w.refresh(lib).await;
    assert_eq!(
        w.count("SELECT count(*) FROM skill_library_updates WHERE status = 'pending'")
            .await,
        2
    );
    let pa = w.pending(a_alpha).await;
    assert_eq!(pa["update"]["kind"], "changed");
    let (s, pb) = w
        .call(
            &b.member,
            Method::GET,
            &format!("/v1/skills/{b_alpha}/library"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{pb}");
    assert_eq!(pb["update"]["kind"], "changed");
    // each organization's counts are its own
    let a = w.org_list(&w.admin, w.org).await;
    assert_eq!(of(&a, lib)["pending_updates"], 1);
    let bl = w.org_list(&b.admin, b.org).await;
    assert_eq!(of(&bl, lib)["pending_updates"], 1);
    let (_, v) = w.call(&w.root, Method::GET, ADMIN_LIBS, None).await;
    assert_eq!(
        v["items"][0]["pending_updates"], 2,
        "the instance admin sees the total: {v}"
    );
    // A approves with its tier authority, quoting the hash it reviewed; B's stays pending
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &format!(
                "/v1/skills/{a_alpha}/library/updates/{}/approve",
                pa["update"]["id"].as_str().unwrap()
            ),
            Some(json!({"content_sha256": pa["update"]["content_sha256"]})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["body"], "v2 body\n");
    let (_, pb2) = w
        .call(
            &b.member,
            Method::GET,
            &format!("/v1/skills/{b_alpha}/library"),
            None,
        )
        .await;
    assert_eq!(
        pb2["update"]["id"], pb["update"]["id"],
        "B's proposal is untouched"
    );
    let (s, _) = w
        .call(
            &w.admin,
            Method::POST,
            &format!(
                "/v1/skills/{b_alpha}/library/updates/{}/approve",
                pb["update"]["id"].as_str().unwrap()
            ),
            Some(json!({"content_sha256": pb["update"]["content_sha256"]})),
        )
        .await;
    assert_eq!(s, 404, "A's admin has no authority over B's skill");
    let _ = a_beta;
}

#[tokio::test]
async fn an_organization_over_its_budget_gets_no_proposal_and_never_stops_another() {
    let w = world().await;
    let sha = w.seed();
    let b = second_org(&w).await;
    let lib = w.instance_library("shared").await;
    let (_, v) = w.add_org(lib, "alpha", &sha).await;
    let a_alpha: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (_, v) = w.add_org(lib, "beta", &sha).await;
    let a_beta: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (_, v) = w.add_agent_of(&b, lib, "alpha", &sha).await;
    let b_alpha: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // A's open proposals already fill its budget
    let filler = Uuid::now_v7();
    sqlx::query("INSERT INTO skill_library_updates (id, skill_id, library_id, kind, status, commit_sha, content_sha256, bytes) VALUES ($1,$2,$3,'removed','pending','x','\\x00',4194300)")
        .bind(filler).bind(a_beta).bind(lib).execute(w.h.pool()).await.unwrap();
    w.git.commit(
        "acme/skills",
        "main",
        &[put("alpha/SKILL.md", md("Alpha v2", "v2 body"))],
    );
    w.refresh(lib).await;
    // A: no proposal for alpha, `over_budget` in its view; B: its proposal, `active`
    assert_eq!(w.pending(a_alpha).await["update"], Value::Null);
    let (_, pb) = w
        .call(
            &b.member,
            Method::GET,
            &format!("/v1/skills/{b_alpha}/library"),
            None,
        )
        .await;
    assert_eq!(
        pb["update"]["kind"], "changed",
        "B's proposal is written: {pb}"
    );
    let a = w.org_list(&w.admin, w.org).await;
    assert_eq!(of(&a, lib)["status"], "over_budget");
    let bl = w.org_list(&b.admin, b.org).await;
    assert_eq!(of(&bl, lib)["status"], "active");
    let (_, v) = w.call(&w.root, Method::GET, ADMIN_LIBS, None).await;
    assert_eq!(
        v["items"][0]["status"], "active",
        "the library's own status does not follow one organization: {v}"
    );
    assert_eq!(
        w.count(&format!(
            "SELECT count(*) FROM skill_library_org_state WHERE library_id = '{lib}'"
        ))
        .await,
        1
    );
    assert_eq!(
        w.count(&format!(
            "SELECT count(*) FROM skill_library_org_state WHERE org_id = '{}'",
            b.org
        ))
        .await,
        0
    );
    // freeing A's space clears the state at the next pass (the head did not change)
    sqlx::query("DELETE FROM skill_library_updates WHERE id = $1")
        .bind(filler)
        .execute(w.h.pool())
        .await
        .unwrap();
    w.refresh(lib).await;
    assert_eq!(w.pending(a_alpha).await["update"]["kind"], "changed");
    assert_eq!(
        w.count("SELECT count(*) FROM skill_library_org_state")
            .await,
        0
    );
    let a = w.org_list(&w.admin, w.org).await;
    assert_eq!(of(&a, lib)["status"], "active");
}

#[tokio::test]
async fn deleting_an_instance_library_detaches_everywhere_and_audits_each_organization() {
    let w = world().await;
    let sha = w.seed();
    let b = second_org(&w).await;
    w.git.require_token("x-access-token", "instance-canary");
    let (s, v) = w
        .call(
            &w.root,
            Method::POST,
            ADMIN_LIBS,
            Some(json!({"label": "shared", "url": url("acme/skills"), "token": "instance-canary"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["token_set"], true);
    let lib: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // the token is the instance admin's: organizations see only that one is set
    let a = w.org_list(&w.admin, w.org).await;
    assert_eq!(of(&a, lib)["token_set"], true);
    assert!(
        !serde_json::to_string(&a)
            .unwrap()
            .contains("instance-canary")
    );
    let (_, v) = w.add_org(lib, "alpha", &sha).await;
    let a_skill: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (_, v) = w.add_agent_of(&b, lib, "alpha", &sha).await;
    let b_skill: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let before = w.snapshot().await;
    let (s, _) = w
        .call(
            &w.root,
            Method::DELETE,
            &format!("/v1/skill-libraries/{lib}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    // both skills are detached with their content unchanged
    for id in [a_skill, b_skill] {
        let (lid, body): (Option<Uuid>, String) =
            sqlx::query_as("SELECT library_id, body FROM skills WHERE id = $1")
                .bind(id)
                .fetch_one(w.h.pool())
                .await
                .unwrap();
        assert!(lid.is_none(), "detached");
        assert_eq!(body, "Alpha body v1\n");
    }
    let after = w.snapshot().await;
    assert_eq!(after["projection"], before["projection"]);
    assert_eq!(after["files"], before["files"]);
    // one instance row and one row in each organization, none carrying the token
    let rows: Vec<(Option<Uuid>, Value)> = sqlx::query_as(
        "SELECT org_id, metadata FROM audit_log WHERE action = 'skill_library.delete' ORDER BY org_id NULLS FIRST",
    )
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    let orgs: Vec<Option<Uuid>> = rows.iter().map(|r| r.0).collect();
    let mut want = vec![None, Some(w.org), Some(b.org)];
    want.sort();
    assert_eq!(
        orgs, want,
        "instance row, then one per organization: {rows:?}"
    );
    for (org, m) in &rows {
        let n = if org.is_none() { 2 } else { 1 };
        assert_eq!(m["skills_detached"], n, "{org:?}: {m}");
        assert_eq!(m["library_label"], "shared");
        assert_eq!(m["repo"], "git.example/acme/skills");
        assert!(!m.to_string().contains("instance-canary"));
    }
    // the sealed token went with the row
    assert_eq!(
        w.count("SELECT count(*) FROM skill_libraries WHERE token_ref IS NOT NULL")
            .await,
        0
    );
    // the create left an instance row too (never an organization's)
    let (create_org, meta): (Option<Uuid>, Value) = sqlx::query_as(
        "SELECT org_id, metadata FROM audit_log WHERE action = 'skill_library.create'",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(create_org.is_none());
    assert_eq!(meta["token"], "set");
}

#[tokio::test]
async fn paused_instance_libraries_the_kill_switch_and_the_cap() {
    let w = world().await;
    w.seed();
    let b = second_org(&w).await;
    let lib = w.instance_library("shared").await;
    let one = format!("/v1/skill-libraries/{lib}");
    let (s, v) = w
        .call(
            &w.root,
            Method::PATCH,
            &one,
            Some(json!({"status": "paused"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // organizations see it as Paused (admins), members do not see it; browse follows the org rule
    let a = w.org_list(&w.admin, w.org).await;
    assert_eq!(of(&a, lib)["status"], "paused");
    assert!(w.org_list(&b.member, b.org).await.is_empty());
    let uri = |org: Uuid| format!("{one}/skills?org={org}");
    let (s, _) = w.call(&b.member, Method::GET, &uri(b.org), None).await;
    assert_eq!(s, 404);
    let (s, _) = w.call(&b.admin, Method::GET, &uri(b.org), None).await;
    assert_eq!(s, 200);
    // the label and the address are unique among instance libraries
    let (s, v) = w
        .call(
            &w.root,
            Method::POST,
            ADMIN_LIBS,
            Some(json!({"label": "shared", "url": url("acme/skills")})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "library_exists"), "{v}");
    w.git.add_repo("acme/other");
    w.git
        .commit("acme/other", "main", &[put("x/SKILL.md", md("X", "b"))]);
    let (s, v) = w
        .call(
            &w.root,
            Method::POST,
            ADMIN_LIBS,
            Some(json!({"label": "shared", "url": url("acme/other")})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "library_exists"), "same label: {v}");
    let (s, v) = w
        .call(
            &w.root,
            Method::POST,
            ADMIN_LIBS,
            Some(json!({"label": "second", "url": url("acme/skills")})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "library_exists"), "same address: {v}");
    // an organization may use the same label and address as an instance library
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &format!("/v1/orgs/{}/skill-libraries", w.org),
            Some(json!({"label": "shared", "url": url("acme/skills")})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    // the cap: LIBRARIES_PER_INSTANCE
    sqlx::query(
        "INSERT INTO skill_libraries (id, org_id, label, url, repo)
         SELECT gen_random_uuid(), NULL, 'fill' || i, 'https://git.example/fill/' || i, 'git.example/fill/' || i
         FROM generate_series(1, $1::int - 1) AS i",
    )
    .bind(aurora_core::limits::LIBRARIES_PER_INSTANCE as i32)
    .execute(w.h.pool())
    .await
    .unwrap();
    let (s, v) = w
        .call(
            &w.root,
            Method::POST,
            ADMIN_LIBS,
            Some(json!({"label": "one-too-many", "url": url("acme/other")})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "library_limit"), "{v}");
    // the check route of the admin: the C-1124 shape, stores nothing
    let before = w.count("SELECT count(*) FROM skill_libraries").await;
    let (s, v) = w
        .call(
            &w.root,
            Method::POST,
            &format!("{ADMIN_LIBS}/check"),
            Some(json!({"url": url("acme/skills")})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["skills"]["valid"], 2);
    assert_eq!(
        w.count("SELECT count(*) FROM skill_libraries").await,
        before
    );
    let (s, _) = w
        .call(
            &w.owner,
            Method::POST,
            &format!("{ADMIN_LIBS}/check"),
            Some(json!({"url": url("acme/skills")})),
        )
        .await;
    assert_eq!(s, 403);
}

#[tokio::test]
async fn the_kill_switch_hides_instance_libraries_too() {
    let w = world_with(|c| c.skills_library.enabled = false).await;
    w.seed();
    for (m, uri, body) in [
        (Method::GET, ADMIN_LIBS.to_owned(), None),
        (
            Method::POST,
            ADMIN_LIBS.to_owned(),
            Some(json!({"label": "x", "url": url("acme/skills")})),
        ),
        (
            Method::POST,
            format!("{ADMIN_LIBS}/check"),
            Some(json!({"url": url("acme/skills")})),
        ),
        (
            Method::GET,
            format!("/v1/orgs/{}/skill-libraries", w.org),
            None,
        ),
    ] {
        let (s, v) = w.call(&w.root, m.clone(), &uri, body).await;
        assert_eq!(
            (s, code(&v)),
            (404, "skill_libraries_disabled"),
            "{m} {uri}: {v}"
        );
    }
}
