#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-246: an organization owner or admin who is not one of a workspace's people resolves the
//! workspace's address (the manage page starts there), and nobody else learns it exists.

use crate::common::*;
use axum::http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

struct World {
    h: Harness,
    org: Uuid,
    /// Bob's personal workspace (Bob bound `owner`) and a shared workspace Carol owns.
    personal: Uuid,
    shared: Uuid,
    bob: Uuid,
    carol: Uuid,
    owner: Uuid,
    admin: Uuid,
    member: Uuid,
    owner_tok: String,
    admin_tok: String,
    member_tok: String,
    bob_tok: String,
}

async fn world() -> World {
    let h = Harness::new().await;
    let org = h.org().await;
    let owner = h.user("owner@x.io", false).await;
    let admin = h.user("admin@x.io", false).await;
    let member = h.user("member@x.io", false).await;
    let bob = h.user("bob@x.io", false).await;
    let carol = h.user("carol@x.io", false).await;
    h.member(org, owner, "owner", "active").await;
    h.member(org, admin, "admin", "active").await;
    h.member(org, member, "member", "active").await;
    h.member(org, bob, "member", "active").await;
    h.member(org, carol, "member", "active").await;
    let personal = h.workspace_owned_by(org, bob).await;
    sqlx::query("UPDATE workspaces SET personal_owner_user_id = $2, personal = true WHERE id = $1")
        .bind(personal)
        .bind(bob)
        .execute(h.pool())
        .await
        .unwrap();
    let shared = h.workspace_owned_by(org, carol).await;
    h.byo_agent(org, personal, "bot", "Bot").await;
    h.byo_agent(org, shared, "bot2", "Bot2").await;
    let (_, owner_tok) = h.good_session(owner).await;
    let (_, admin_tok) = h.good_session(admin).await;
    let (_, member_tok) = h.good_session(member).await;
    let (_, bob_tok) = h.good_session(bob).await;
    World {
        h,
        org,
        personal,
        shared,
        bob,
        carol,
        owner,
        admin,
        member,
        owner_tok,
        admin_tok,
        member_tok,
        bob_tok,
    }
}

async fn slug(h: &Harness, table: &str, id: Uuid) -> String {
    let sql = if table == "organizations" {
        "SELECT slug FROM organizations WHERE id=$1"
    } else {
        "SELECT slug FROM workspaces WHERE id=$1"
    };
    sqlx::query_scalar::<_, String>(sql)
        .bind(id)
        .fetch_one(h.pool())
        .await
        .unwrap()
}

impl World {
    async fn resolve(&self, tok: &str, org: Uuid, ws: Uuid) -> (u16, Value) {
        let uri = format!(
            "/v1/resolve?org={}&workspace={}",
            slug(&self.h, "organizations", org).await,
            slug(&self.h, "workspaces", ws).await
        );
        self.h.call_tok(tok, Method::GET, &uri, None).await
    }
}

#[tokio::test]
async fn resolve_admits_an_administrator_and_nobody_else() {
    let w = world().await;
    for ws in [w.personal, w.shared] {
        for (who, tok) in [("owner", &w.owner_tok), ("admin", &w.admin_tok)] {
            let (s, v) = w.resolve(tok, w.org, ws).await;
            assert_eq!(s, 200, "{who} {ws}: {v}");
            assert_eq!(v["workspace"]["id"], json!(ws));
            assert_eq!(
                v["workspace"].as_object().unwrap().len(),
                3,
                "id, slug, name only"
            );
            assert_eq!(v["org"]["id"], json!(w.org));
        }
        // An ordinary member who is not in the workspace.
        let (s, v) = w.resolve(&w.member_tok, w.org, ws).await;
        assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    }
    // The unknown slug answers exactly like a workspace the caller may not see.
    let uri = format!(
        "/v1/resolve?org={}&workspace=nope",
        slug(&w.h, "organizations", w.org).await
    );
    let (s_unknown, v_unknown) = w.h.call_tok(&w.member_tok, Method::GET, &uri, None).await;
    let (s_hidden, v_hidden) = w.resolve(&w.member_tok, w.org, w.shared).await;
    assert_eq!((s_unknown, &v_unknown), (s_hidden, &v_hidden));
    let (s, _) = w.h.call_tok(&w.admin_tok, Method::GET, &uri, None).await;
    assert_eq!(s, 404);

    // The person resolves their own workspace.
    let (s, v) = w.resolve(&w.bob_tok, w.org, w.personal).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["workspace"]["id"], json!(w.personal));

    // Another organization: a person there, and its own owner/admin, see nothing here.
    let other = w.h.org().await;
    let outsider = w.h.user("out@x.io", false).await;
    let other_admin = w.h.user("oadmin@x.io", false).await;
    w.h.member(other, outsider, "member", "active").await;
    w.h.member(other, other_admin, "admin", "active").await;
    for u in [outsider, other_admin] {
        let (_, tok) = w.h.good_session(u).await;
        for ws in [w.personal, w.shared] {
            let (s, v) = w.resolve(&tok, w.org, ws).await;
            assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
        }
    }

    // A workspace key of another workspace is refused; its own workspace still resolves.
    let (_, k) = w.h.api_key(w.admin, w.org, Some(w.shared), false).await;
    let b = bearer(&k);
    let org_slug = slug(&w.h, "organizations", w.org).await;
    let personal_slug = slug(&w.h, "workspaces", w.personal).await;
    let (s, _, v) =
        w.h.send(
            Method::GET,
            &format!("/v1/resolve?org={org_slug}&workspace={personal_slug}"),
            &[("authorization", &b)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 404, "{v}");
    // An admin's key for a workspace the admin is not in: confined to it, administer holds.
    let shared_slug = slug(&w.h, "workspaces", w.shared).await;
    let (s, _, v) =
        w.h.send(
            Method::GET,
            &format!("/v1/resolve?org={org_slug}&workspace={shared_slug}"),
            &[("authorization", &b)],
            None,
        )
        .await;
    assert_eq!(s.as_u16(), 200, "{v}");
    let _ = (w.owner, w.bob);
}

/// What the manage page calls for an administrator without own access: all of it answers 200.
#[tokio::test]
async fn manage_page_routes_never_404_an_administrator() {
    let w = world().await;
    let org = w.org;
    for (who, tok) in [("owner", &w.owner_tok), ("admin", &w.admin_tok)] {
        for (kind, ws, person) in [
            ("personal", w.personal, w.bob),
            ("shared", w.shared, w.carol),
        ] {
            let routes = [
                "/v1/me".to_owned(),
                format!("/v1/orgs/{org}"),
                format!("/v1/orgs/{org}/members"),
                format!("/v1/workspaces/{ws}"),
                format!("/v1/workspaces/{ws}/agents"),
                format!("/v1/workspaces/{ws}/access?expand=groups&limit=100"),
                format!("/v1/workspaces/{ws}/access/pending"),
                format!("/v1/workspaces/{ws}/members/{person}/offboarding"),
            ];
            for r in routes {
                let (s, v) = w.h.call_tok(tok, Method::GET, &r, None).await;
                eprintln!("MANAGE {who} {kind} GET {r} -> {s} {}", code(&v));
                assert_eq!(s, 200, "{who} {kind} {r}: {v}");
            }
        }
    }
    let _ = (w.member, w.owner);
}

const BRIEF: &str = "FIX246-agent-brief-content";

/// The agent list for an administrator without own access: the same agents, metadata only (no
/// brief, no BYO governance); the person with access still reads the brief.
#[tokio::test]
async fn agent_list_for_an_administrator_carries_no_brief() {
    let w = world().await;
    sqlx::query("UPDATE agents SET brief = $1 WHERE workspace_id = ANY($2)")
        .bind(BRIEF)
        .bind(vec![w.personal, w.shared])
        .execute(w.h.pool())
        .await
        .unwrap();
    // An agent frozen through the route by the admin (whose label is an e-mail address).
    let frozen_agent: Uuid = sqlx::query_scalar("SELECT id FROM agents WHERE workspace_id = $1")
        .bind(w.personal)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    let (s, v) =
        w.h.call_tok(
            &w.admin_tok,
            Method::POST,
            &format!("/v1/agents/{frozen_agent}/freeze"),
            Some(json!({"reason": "FIX-246 check"})),
        )
        .await;
    assert!((200..300).contains(&s), "{v}");
    let (_, carol_tok) = w.h.good_session(w.carol).await;
    for (ws, person_tok) in [(w.personal, &w.bob_tok), (w.shared, &carol_tok)] {
        let uri = format!("/v1/workspaces/{ws}/agents");
        let (s, own) = w.h.call_tok(person_tok, Method::GET, &uri, None).await;
        assert_eq!(s, 200, "{own}");
        assert!(
            own.to_string().contains(BRIEF),
            "the person reads the brief"
        );
        for (who, tok) in [("owner", &w.owner_tok), ("admin", &w.admin_tok)] {
            let (s, v) = w.h.call_tok(tok, Method::GET, &uri, None).await;
            assert_eq!(s, 200, "{who}: {v}");
            assert!(!v.to_string().contains(BRIEF), "{who}: {v}");
            let ids = |v: &Value| -> Vec<Value> {
                v["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|a| a["id"].clone())
                    .collect()
            };
            assert_eq!(ids(&v), ids(&own), "{who}: same agents");
            for a in v["items"].as_array().unwrap() {
                assert_eq!(a["brief"], json!(""));
                assert!(a["byo"].is_null());
                assert!(a.get("frozen").is_some() && a.get("effective_frozen").is_some());
                if a["id"] == json!(frozen_agent) {
                    assert_eq!(a["effective_frozen"], json!(true), "{a}");
                    assert_eq!(a["frozen"]["reason"], json!("FIX-246 check"));
                    assert_eq!(a["frozen"]["level"], json!("agent"));
                    assert!(
                        !a["frozen"]["by_label"].as_str().unwrap().contains('@'),
                        "no e-mail address as a label: {a}"
                    );
                }
            }
        }
        // An ordinary member without access keeps today's 404.
        let (s, _) = w.h.call_tok(&w.member_tok, Method::GET, &uri, None).await;
        assert_eq!(s, 404);
    }
}
