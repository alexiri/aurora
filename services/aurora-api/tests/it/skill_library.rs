#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-GSL (D-148, D-149; SEC-83, SEC-84): the git skill library API against the fake git host
//! (`aurora_testkit::git_host`) through a gateway stand-in that builds the same requests from the
//! package's `library` block (`18 §Git skill library`). Real Postgres.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use aurora_api::domains::skill_libraries::gateway::{
    FetchOk, FetchRequest, GatewayError, LibraryGateway,
};
use aurora_api::domains::skill_libraries::refresh;
use aurora_crypto::{SigningKey, generate_signing_key};
use aurora_db::repo::skill_libraries as repo;
use aurora_registry::builder::PackageBuilder;
use aurora_registry::library::{self, Params};
use aurora_registry::manifest::Library;
use aurora_testkit::git_host::{Change, FakeGitHost};
use axum::body::Body;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use crate::common::*;

const YAML: &str = r#"package: { id: forge, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: Forge }
apps:
  - about: { id: forge, name: Forge, description: D, category: development }
    credentials: [ { name: access_token, secret: true } ]
    connect:
      - id: token
        kind: fields
        fields: [ { name: access_token, label: Token, type: secret, required: true } ]
    hosts:
      - match: { host: api.forge.example }
        inject:
          - bearer: "{credentials.access_token}"
    library:
      repo_pattern: "^[A-Za-z0-9_.-]{1,100}/[A-Za-z0-9_.-]{1,100}$"
      request_host: api.forge.example
      resolve_ref:
        path: "/repos/{repo}/commits/{ref}"
        default_path: "/repos/{repo}"
        sha: "$.sha"
        default_branch: "$.default_branch"
      list_tree:
        path: "/repos/{repo}/git/trees/{sha}?recursive=1"
        entries: "$.tree[*]"
        fields: { path: "$.path", type: "$.type", mode: "$.mode", blob: "$.sha", size: "$.size" }
        file_type: blob
        truncated: "$.truncated"
        paginate: none
      get_blob:
        path: "/repos/{repo}/git/blobs/{blob}"
        content: "$.content"
        encoding: "$.encoding"
  - about: { id: forgelab, name: ForgeLab, description: D, category: development }
    credentials: [ { name: access_token, secret: true } ]
    connect:
      - id: token
        kind: fields
        fields: [ { name: access_token, label: Token, type: secret, required: true } ]
    hosts:
      - match: { host: lab.forge.example, path_prefix: /api/v4 }
        inject:
          - bearer: "{credentials.access_token}"
    library:
      repo_pattern: "^[A-Za-z0-9_.-]{1,100}(/[A-Za-z0-9_.-]{1,100}){1,19}$"
      request_host: lab.forge.example
      resolve_ref:
        path: "/api/v4/projects/{repo|urlencode}/repository/commits/{ref|urlencode}"
        default_path: "/api/v4/projects/{repo|urlencode}"
        sha: "$.id"
        default_branch: "$.default_branch"
      list_tree:
        path: "/api/v4/projects/{repo|urlencode}/repository/tree?recursive=true&per_page=100&ref={sha}"
        entries: "$[*]"
        fields: { path: "$.path", type: "$.type", mode: "$.mode", blob: "$.id" }
        file_type: blob
        paginate: link_header
        max_pages: 50
      get_blob:
        path: "/api/v4/projects/{repo|urlencode}/repository/blobs/{blob}/raw"
        encoding: raw
  - about: { id: plain, name: Plain, description: D, category: development }
    credentials: [ { name: access_token, secret: true } ]
    connect:
      - id: token
        kind: fields
        fields: [ { name: access_token, label: Token, type: secret, required: true } ]
    hosts:
      - match: { host: plain.forge.example }
        inject:
          - bearer: "{credentials.access_token}"
"#;

/// The gateway stand-in: the same checks and the same request building as the real route, sent
/// to the fake git host with a fixed token.
struct Direct {
    pool: sqlx::PgPool,
    base: String,
    blocks: HashMap<String, Library>,
}

#[async_trait]
impl LibraryGateway for Direct {
    async fn fetch(&self, r: &FetchRequest) -> Result<FetchOk, GatewayError> {
        let row: Option<(Option<Uuid>, String, String)> = sqlx::query_as(
            "SELECT workspace_id, app_id, status FROM app_connections WHERE id = $1 AND org_id = $2",
        )
        .bind(r.connection_id)
        .bind(r.org_id)
        .fetch_optional(&self.pool)
        .await
        .unwrap();
        let Some((ws, app, status)) = row else {
            return Err(GatewayError::ConnectionNotFound);
        };
        if ws.is_some() {
            return Err(GatewayError::Unavailable);
        }
        match status.as_str() {
            "active" => {}
            "reauth_required" => return Err(GatewayError::Reauth),
            _ => return Err(GatewayError::Unavailable),
        }
        let Some(block) = self.blocks.get(&app) else {
            return Err(GatewayError::Unavailable);
        };
        let p = Params {
            repo: r.params.repo.clone(),
            reference: r.params.reference.clone(),
            sha: r.params.sha.clone(),
            blob: r.params.blob.clone(),
            page: r.params.page,
        };
        let target = library::build(block, r.step, &p).map_err(|_| GatewayError::Param)?;
        let resp = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
            .get(format!("{}{target}", self.base))
            .bearer_auth("tok")
            .send()
            .await
            .map_err(|_| GatewayError::Upstream("the git host could not be reached"))?;
        let status = resp.status().as_u16();
        let link = resp
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = resp.bytes().await.unwrap().to_vec();
        if body.len() > r.max_bytes {
            return Err(GatewayError::Upstream(
                "the git host's response was too large",
            ));
        }
        Ok(FetchOk {
            status,
            link,
            retry_after,
            body,
        })
    }
}

struct W {
    h: Harness,
    git: FakeGitHost,
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    root: String,
    owner: String,
    admin: String,
    wsowner: String,
    member: String,
    unbound: String,
    nonmember: String,
    /// Org-scoped connection of `forge` (GitHub-shaped).
    conn: Uuid,
    /// Org-scoped connection of `forgelab` (GitLab-shaped).
    lab: Uuid,
}

async fn upload(h: &Harness, admin: &str, yaml: &str) {
    let key: SigningKey = generate_signing_key().unwrap();
    let hd = [("cookie", admin), ("origin", ORIGIN)];
    let (s, _, v) = h
        .send(
            Method::POST,
            "/v1/admin/trusted-keys",
            &hd,
            Some(json!({"publisher": "acme", "name": "k", "public_key": PackageBuilder::key_id(&key)})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let mut b = PackageBuilder::new(yaml);
    b.sign(&key, &PackageBuilder::key_id(&key));
    let boundary = "XB";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"package\"; filename=\"p\"\r\nContent-Type: application/gzip\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(&b.build().unwrap());
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/admin/plugins")
        .header("cookie", admin)
        .header("origin", ORIGIN)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let res = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), 201);
    let v: Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(v["status"], "pending_approval", "{}", v["violations"]);
    let (s, _, v) = h
        .send(
            Method::POST,
            &format!("/v1/admin/plugins/{}/approve", v["id"].as_str().unwrap()),
            &hd,
            Some(json!({})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
}

fn blocks() -> HashMap<String, Library> {
    let m = aurora_registry::Manifest::parse(YAML, "0.1.0").unwrap();
    m.apps
        .iter()
        .filter_map(|a| a.library.clone().map(|l| (a.about.id.clone(), l)))
        .collect()
}

async fn world_with(mutate: impl FnOnce(&mut aurora_config::ApiConfig) + Send + 'static) -> W {
    let mut h = Harness::with_config(mutate).await;
    let git = FakeGitHost::start().await;
    let state = h.state.clone().with_library_gateway(Arc::new(Direct {
        pool: h.pool().clone(),
        base: git.base(),
        blocks: blocks(),
    }));
    h.app = test_router(state.clone());
    h.state = state;
    let root = h.user("root@x.io", true).await;
    let root_cookie = cookie(&h.good_session(root).await.1);
    upload(&h, &root_cookie, YAML).await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let mk = |email: &'static str, role: &'static str, ws_role: Option<&'static str>| {
        let (h, email) = (&h, email);
        async move {
            let u = h.user(email, false).await;
            if !role.is_empty() {
                h.member(org, u, role, "active").await;
            }
            if let Some(r) = ws_role {
                h.bind_ws(ws, u, r).await;
            }
            cookie(&h.good_session(u).await.1)
        }
    };
    let owner = mk("owner@x.io", "owner", Some("owner")).await;
    let admin = mk("admin@x.io", "admin", None).await;
    let wsowner = mk("wsowner@x.io", "member", Some("owner")).await;
    let member = mk("member@x.io", "member", Some("member")).await;
    let unbound = mk("unbound@x.io", "member", None).await;
    let nonmember = mk("non@x.io", "", None).await;
    let agent = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness)
         VALUES ($1,$2,$3,'a1','a1','hosted','fake')",
    )
    .bind(agent)
    .bind(org)
    .bind(ws)
    .execute(h.pool())
    .await
    .unwrap();
    let conn = connection(&h, org, None, "forge", "active").await;
    let lab = connection(&h, org, None, "forgelab", "active").await;
    W {
        h,
        git,
        org,
        ws,
        agent,
        root: root_cookie,
        owner,
        admin,
        wsowner,
        member,
        unbound,
        nonmember,
        conn,
        lab,
    }
}

async fn world() -> W {
    world_with(|_| {}).await
}

async fn connection(h: &Harness, org: Uuid, ws: Option<Uuid>, app: &str, status: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref)
         VALUES ($1,$2,$3,$4,'forge','1.0.0','c','active','x')",
    )
    .bind(id)
    .bind(org)
    .bind(ws)
    .bind(app)
    .execute(h.pool())
    .await
    .unwrap();
    if status != "active" {
        sqlx::query("UPDATE app_connections SET status = $2 WHERE id = $1")
            .bind(id)
            .bind(status)
            .execute(h.pool())
            .await
            .unwrap();
    }
    id
}

fn md(desc: &str, body: &str) -> Vec<u8> {
    format!("---\ndescription: {desc}\n---\n\n{body}\n").into_bytes()
}

fn put(path: &str, content: Vec<u8>) -> Change {
    Change::Put(path.to_owned(), content)
}

impl W {
    async fn call(&self, who: &str, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (s, _, v) = self
            .h
            .send(m, uri, &[("cookie", who), ("origin", ORIGIN)], body)
            .await;
        (s.as_u16(), v)
    }

    /// The default repository with two valid skills.
    fn seed(&self) -> String {
        self.git.add_repo("acme/skills");
        self.git.commit(
            "acme/skills",
            "main",
            &[
                put("alpha/SKILL.md", md("Alpha skill", "Alpha body v1")),
                put("alpha/notes.txt", b"alpha notes".to_vec()),
                put("beta/SKILL.md", md("Beta skill", "Beta body v1")),
            ],
        )
    }

    async fn library(&self) -> Uuid {
        self.library_at("lib", "").await
    }

    async fn library_at(&self, label: &str, path: &str) -> Uuid {
        let (s, v) = self
            .call(
                &self.admin,
                Method::POST,
                &format!("/v1/orgs/{}/skill-libraries", self.org),
                Some(
                    json!({"label": label, "connection_id": self.conn, "repo": "acme/skills",
                    "path": path}),
                ),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        v["id"].as_str().unwrap().parse().unwrap()
    }

    fn from_library(lib: Uuid, path: &str, sha: &str) -> Value {
        json!({"from_library": {"library_id": lib, "path": path, "commit_sha": sha}})
    }

    async fn add_org(&self, lib: Uuid, path: &str, sha: &str) -> (u16, Value) {
        self.call(
            &self.admin,
            Method::POST,
            &format!("/v1/orgs/{}/skills", self.org),
            Some(Self::from_library(lib, path, sha)),
        )
        .await
    }

    async fn add_agent(&self, lib: Uuid, path: &str, sha: &str) -> (u16, Value) {
        self.call(
            &self.member,
            Method::POST,
            &format!("/v1/agents/{}/skills", self.agent),
            Some(Self::from_library(lib, path, sha)),
        )
        .await
    }

    async fn add_ws(&self, lib: Uuid, path: &str, sha: &str) -> (u16, Value) {
        self.call(
            &self.wsowner,
            Method::POST,
            &format!("/v1/workspaces/{}/skills", self.ws),
            Some(Self::from_library(lib, path, sha)),
        )
        .await
    }

    /// Make the library due and run the refresh job once.
    async fn refresh(&self, lib: Uuid) {
        sqlx::query("UPDATE skill_libraries SET next_refresh_at = now() - interval '1 second', refresh_lease_until = NULL, lease_owner = NULL WHERE id = $1")
            .bind(lib)
            .execute(self.h.pool())
            .await
            .unwrap();
        refresh::run_once(&self.h.state).await.unwrap();
    }

    async fn library_row(&self, lib: Uuid) -> Value {
        let (s, v) = self
            .call(
                &self.admin,
                Method::GET,
                &format!("/v1/skill-libraries/{lib}"),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }

    async fn pending(&self, skill: Uuid) -> Value {
        let (s, v) = self
            .call(
                &self.owner,
                Method::GET,
                &format!("/v1/skills/{skill}/library"),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }

    /// Everything a refresh must not touch: skill rows and files, every home-sync generation.
    async fn snapshot(&self) -> Value {
        let skills: Vec<(String, String, String, bool, Option<String>)> = sqlx::query_as(
            "SELECT id::text, name, body, enabled, library_commit_sha FROM skills ORDER BY id",
        )
        .fetch_all(self.h.pool())
        .await
        .unwrap();
        let files: Vec<(String, String, Vec<u8>, bool)> = sqlx::query_as(
            "SELECT skill_id::text, path, content, executable FROM skill_files ORDER BY skill_id, path",
        )
        .fetch_all(self.h.pool())
        .await
        .unwrap();
        let ag: Vec<(String, i64)> =
            sqlx::query_as("SELECT id::text, home_sync_generation FROM agents ORDER BY id")
                .fetch_all(self.h.pool())
                .await
                .unwrap();
        let wg: Vec<(String, i64)> =
            sqlx::query_as("SELECT id::text, home_sync_generation FROM workspaces ORDER BY id")
                .fetch_all(self.h.pool())
                .await
                .unwrap();
        // The projection the supervisor receives (effective skills of the agent).
        let mut c = self.h.pool().acquire().await.unwrap();
        let proj = aurora_db::repo::hosted::skills(&mut c, self.org, self.ws, self.agent)
            .await
            .unwrap()
            .into_iter()
            .map(|s| (s.name, s.description, s.body))
            .collect::<Vec<_>>();
        json!({"skills": skills, "files": files, "agents": ag, "workspaces": wg, "projection": proj})
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    async fn audit_critical(&self, action: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action = $1 AND critical")
            .bind(action)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

// ---- management -----------------------------------------------------------------------------

#[tokio::test]
async fn create_list_patch_delete_with_the_permission_matrix() {
    let w = world().await;
    w.seed();
    let org = w.org;
    let list = format!("/v1/orgs/{org}/skill-libraries");
    let body = json!({"label": "Team skills", "connection_id": w.conn, "repo": "acme/skills"});
    // org owner/admin only
    for who in [&w.member, &w.wsowner, &w.unbound] {
        let (s, v) = w.call(who, Method::POST, &list, Some(body.clone())).await;
        assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    }
    let (s, v) = w
        .call(&w.nonmember, Method::POST, &list, Some(body.clone()))
        .await;
    assert_eq!(s, 404, "{v}");
    let (s, v) = w
        .call(&w.admin, Method::POST, &list, Some(body.clone()))
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["status"], "active");
    assert_eq!(v["repo"], "acme/skills");
    assert_eq!(v["last_head_sha"], w.git.head("acme/skills", "main"));
    assert_eq!(v["connection"]["id"], w.conn.to_string());
    assert_eq!(
        v["skill_counts"],
        json!({"agent": 0, "workspace": 0, "org": 0})
    );
    assert_eq!(v["pending_updates"], 0);
    let id = v["id"].as_str().unwrap().to_owned();
    assert_eq!(w.audit_critical("skill_library.create").await, 1);

    // duplicate label
    let (s, v) = w
        .call(&w.admin, Method::POST, &list, Some(body.clone()))
        .await;
    assert_eq!((s, code(&v)), (409, "library_exists"), "{v}");

    // admins see everything; people who can use a workspace get id/label/status of active ones
    let (s, v) = w.call(&w.owner, Method::GET, &list, None).await;
    assert_eq!(
        (s, v["items"][0]["repo"].as_str()),
        (200, Some("acme/skills"))
    );
    let (s, v) = w.call(&w.member, Method::GET, &list, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"][0]["label"], "Team skills");
    assert!(v["items"][0]["repo"].is_null() && v["items"][0]["connection"].is_null());
    let (s, v) = w.call(&w.unbound, Method::GET, &list, None).await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    let (s, _) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/skill-libraries/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 403);

    // pause hides it from the picker; resume
    let one = format!("/v1/skill-libraries/{id}");
    let (s, v) = w
        .call(
            &w.admin,
            Method::PATCH,
            &one,
            Some(json!({"status": "paused", "label": "Renamed"})),
        )
        .await;
    assert_eq!(
        (s, v["status"].as_str(), v["label"].as_str()),
        (200, Some("paused"), Some("Renamed")),
        "{v}"
    );
    let (_, v) = w.call(&w.member, Method::GET, &list, None).await;
    assert!(v["items"].as_array().unwrap().is_empty());
    let (s, v) = w
        .call(
            &w.admin,
            Method::PATCH,
            &one,
            Some(json!({"status": "bogus"})),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    assert_eq!(w.audit_critical("skill_library.update").await, 1);
    let (s, _) = w
        .call(
            &w.admin,
            Method::PATCH,
            &one,
            Some(json!({"status": "active"})),
        )
        .await;
    assert_eq!(s, 200);

    // delete
    let (s, _) = w.call(&w.member, Method::DELETE, &one, None).await;
    assert_eq!(s, 403);
    let (s, _) = w.call(&w.admin, Method::DELETE, &one, None).await;
    assert_eq!(s, 204);
    assert_eq!(w.audit_critical("skill_library.delete").await, 1);
    let (s, _) = w.call(&w.admin, Method::GET, &one, None).await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn create_refusals_scope_app_repo_reach_and_limit() {
    let w = world().await;
    w.seed();
    let list = format!("/v1/orgs/{}/skill-libraries", w.org);
    let mk = |conn: Uuid, repo: &str, label: &str| json!({"label": label, "connection_id": conn, "repo": repo});
    // workspace-scoped connection
    let ws_conn = connection(&w.h, w.org, Some(w.ws), "forge", "active").await;
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &list,
            Some(mk(ws_conn, "acme/skills", "a")),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "connection_not_org_scoped"), "{v}");
    // an app without a library block
    let plain = connection(&w.h, w.org, None, "plain", "active").await;
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &list,
            Some(mk(plain, "acme/skills", "b")),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "app_without_library"), "{v}");
    // not active
    let off = connection(&w.h, w.org, None, "forge", "disabled").await;
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &list,
            Some(mk(off, "acme/skills", "c")),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "library_unreachable"), "{v}");
    // a connection of another organization
    let other = w.h.org().await;
    let foreign = connection(&w.h, other, None, "forge", "active").await;
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &list,
            Some(mk(foreign, "acme/skills", "d")),
        )
        .await;
    assert_eq!((s, code(&v)), (400, "invalid_request"), "{v}");
    // repository grammar, ref grammar, path grammar
    for (repo, reference, path) in [
        ("../x", None, ""),
        ("acme", None, ""),
        ("acme/skills", Some("a..b"), ""),
        ("acme/skills", Some("-x"), ""),
        ("acme/skills", None, "../up"),
        ("acme/skills", None, "a/b/c/d/e"),
    ] {
        let mut b = mk(w.conn, repo, "e");
        b["ref"] = json!(reference);
        b["path"] = json!(path);
        let (s, v) = w.call(&w.admin, Method::POST, &list, Some(b)).await;
        assert_eq!(
            (s, code(&v)),
            (400, "invalid_request"),
            "{repo} {reference:?} {path}: {v}"
        );
    }
    // unreachable repository: the reason is shown, sanitized
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &list,
            Some(mk(w.conn, "acme/missing", "f")),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "library_unreachable"), "{v}");
    assert!(
        !v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("127.0.0.1")
    );
    // unknown ref
    let mut b = mk(w.conn, "acme/skills", "g");
    b["ref"] = json!("nonexistent");
    let (s, v) = w.call(&w.admin, Method::POST, &list, Some(b)).await;
    assert_eq!((s, code(&v)), (422, "library_unreachable"), "{v}");
    assert_eq!(w.count("SELECT count(*) FROM skill_libraries").await, 0);
    // LIBRARIES_PER_ORG
    for i in 0..10 {
        let (s, v) = w
            .call(
                &w.admin,
                Method::POST,
                &list,
                Some(mk(w.conn, "acme/skills", &format!("l{i}"))),
            )
            .await;
        assert_eq!(s, 201, "{v}");
    }
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &list,
            Some(mk(w.conn, "acme/skills", "l10")),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "library_limit"), "{v}");
    // a ref (branch) and a path are honoured; the GitLab shape works with a nested project
    w.git.add_repo("grp/sub/proj");
    w.git
        .commit("grp/sub/proj", "main", &[put("s/SKILL.md", md("d", "b"))]);
    let (s, v) = w
        .call(
            &w.admin,
            Method::DELETE,
            &format!("/v1/skill-libraries/{}", {
                let (_, l) = w.call(&w.admin, Method::GET, &list, None).await;
                l["items"][0]["id"].as_str().unwrap().to_owned()
            }),
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &list,
            Some(mk(w.lab, "grp/sub/proj", "gl")),
        )
        .await;
    assert_eq!(s, 201, "{v}");
}

#[tokio::test]
async fn the_kill_switch_hides_the_feature_and_stops_the_job() {
    let w = world_with(|c| c.skills_library.enabled = false).await;
    w.seed();
    for (m, uri) in [
        (Method::GET, format!("/v1/orgs/{}/skill-libraries", w.org)),
        (Method::POST, format!("/v1/orgs/{}/skill-libraries", w.org)),
        (
            Method::GET,
            format!("/v1/orgs/{}/skill-library-updates", w.org),
        ),
    ] {
        let (s, v) = w
            .call(
                &w.admin,
                m,
                &uri,
                Some(json!({"label": "x", "connection_id": w.conn, "repo": "a/b"})),
            )
            .await;
        assert_eq!(
            (s, code(&v)),
            (404, "skill_libraries_disabled"),
            "{uri}: {v}"
        );
    }
    assert_eq!(refresh::run_once(&w.h.state).await.unwrap(), 0);
}

// ---- browse ---------------------------------------------------------------------------------

#[tokio::test]
async fn browse_is_live_stores_nothing_is_cached_and_marks_untrusted_text() {
    let w = world().await;
    w.git.add_repo("acme/skills");
    // (`\a` is YAML's escape for the BEL control character)
    let hostile = "Evil <b>bold</b> [x](javascript:alert(1)) \u{202e}rtl\u{200b}zero \\abell";
    let head = w.git.commit(
        "acme/skills",
        "main",
        &[
            put("alpha/SKILL.md", md("Alpha skill", "Alpha body")),
            put("alpha/notes.txt", b"alpha notes".to_vec()),
            put(
                "hostile/SKILL.md",
                format!("---\ndescription: \"{hostile}\"\n---\nbody\n").into_bytes(),
            ),
            put("linked/SKILL.md", md("Has a symlink", "b")),
            Change::Symlink("linked/evil".into(), "/etc/passwd".into()),
            put("Bad_Name/SKILL.md", md("Bad directory name", "b")),
            put("nofm/SKILL.md", b"no frontmatter here\n".to_vec()),
            put(
                "mismatch/SKILL.md",
                b"---\nname: other\ndescription: d\n---\nb\n".to_vec(),
            ),
            put("big/SKILL.md", md("Has an oversize file", "b")),
            put("big/data.bin", vec![b'x'; 70 * 1024]),
            put("aurora-reserved/SKILL.md", md("Reserved prefix", "b")),
            put("notaskill/readme.md", b"just a readme".to_vec()),
        ],
    );
    let lib = w.library().await;
    let (s, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/skill-libraries/{lib}/skills"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["commit_sha"], head);
    let items = v["items"].as_array().unwrap();
    let by = |p: &str| {
        items
            .iter()
            .find(|i| i["path"] == p)
            .unwrap_or_else(|| panic!("{p} missing in {items:?}"))
    };
    assert_eq!(by("alpha")["valid"], true);
    assert_eq!(by("alpha")["description"], "Alpha skill");
    for bad in [
        "linked",
        "Bad_Name",
        "nofm",
        "mismatch",
        "big",
        "aurora-reserved",
    ] {
        assert_eq!(by(bad)["valid"], false, "{bad}");
        assert!(
            by(bad)["reason"].as_str().is_some_and(|r| !r.is_empty()),
            "{bad}"
        );
    }
    assert!(by("linked")["reason"].as_str().unwrap().contains("symlink"));
    assert!(
        items.iter().all(|i| i["path"] != "notaskill"),
        "no SKILL.md, not a skill"
    );
    // the hostile description is data: controls, bidi and zero-width characters are gone
    let d = by("hostile")["description"].as_str().unwrap();
    assert!(!d.contains(['\u{202e}', '\u{200b}', '\u{7}']), "{d:?}");
    assert!(
        d.contains("<b>bold</b>"),
        "markup is kept as inert text: {d:?}"
    );
    // nothing is stored by browsing
    assert_eq!(w.count("SELECT count(*) FROM skills").await, 0);
    assert_eq!(w.count("SELECT count(*) FROM skill_files").await, 0);
    // cached for 5 minutes: a second browse costs no request, even after a push
    let calls = w.git.requests().len();
    w.git.commit(
        "acme/skills",
        "main",
        &[put("alpha/SKILL.md", md("Alpha v2", "x"))],
    );
    let (s, v2) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/skill-libraries/{lib}/skills"),
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v2["commit_sha"], head);
    assert_eq!(w.git.requests().len(), calls);
    // q filters; the content endpoint returns exactly what an add would store
    let (_, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/skill-libraries/{lib}/skills?q=ALPHA"),
            None,
        )
        .await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    let (s, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/skill-libraries/{lib}/skills/alpha?commit_sha={head}"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (v["valid"].as_bool(), v["body"].as_str()),
        (Some(true), Some("Alpha body\n"))
    );
    assert_eq!(v["files"][0]["content"], "alpha notes");
    assert_eq!(v["content_sha256"].as_str().unwrap().len(), 64);
    let (s, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/skill-libraries/{lib}/skills/big?commit_sha={head}"),
            None,
        )
        .await;
    assert_eq!((s, v["valid"].as_bool()), (200, Some(false)), "{v}");
    // not for people who cannot use any workspace
    let (s, _) = w
        .call(
            &w.unbound,
            Method::GET,
            &format!("/v1/skill-libraries/{lib}/skills"),
            None,
        )
        .await;
    assert_eq!(s, 403);
    let (s, _) = w
        .call(
            &w.nonmember,
            Method::GET,
            &format!("/v1/skill-libraries/{lib}/skills"),
            None,
        )
        .await;
    assert_eq!(s, 404);
    assert_eq!(w.count("SELECT count(*) FROM skills").await, 0);
}

#[tokio::test]
async fn browse_follows_pagination_and_a_truncated_tree_is_refused() {
    let w = world().await;
    w.git.add_repo("grp/proj");
    let changes: Vec<Change> = (0..12)
        .map(|i| put(&format!("s{i:02}/SKILL.md"), md(&format!("Skill {i}"), "b")))
        .collect();
    w.git.commit("grp/proj", "main", &changes);
    w.git.set_page_size(5);
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &format!("/v1/orgs/{}/skill-libraries", w.org),
            Some(json!({"label": "gl", "connection_id": w.lab, "repo": "grp/proj"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let lib = v["id"].as_str().unwrap().to_owned();
    let (s, v) = w
        .call(
            &w.admin,
            Method::GET,
            &format!("/v1/skill-libraries/{lib}/skills"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"].as_array().unwrap().len(), 12);
    assert!(w.git.count("page=3") >= 1, "three pages were read");
    // GitHub shape: a truncated listing is refused with advice to narrow the folder
    let w2 = world().await;
    w2.seed();
    let lib = w2.library().await;
    w2.git.set_truncated(true);
    let (s, v) = w2
        .call(
            &w2.admin,
            Method::GET,
            &format!("/v1/skill-libraries/{lib}/skills"),
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (422, "library_tree_too_large"), "{v}");
}

#[tokio::test]
async fn browse_errors_are_reasons_never_credentials() {
    let w = world().await;
    w.seed();
    let mut libs = Vec::new();
    // one library row per round: no browse cache between rounds
    for label in ["r404", "r401", "r429", "r500", "redir"] {
        libs.push((label, w.library_at(label, "").await));
    }
    for (label, status, needle) in [
        ("r404", 404, "not found"),
        ("r401", 401, "rejected"),
        ("r429", 429, "rate limiting"),
        ("r500", 500, "status 500"),
    ] {
        w.git.clear_overrides();
        w.git.override_status("/commits/", status);
        let id = libs.iter().find(|(l, _)| *l == label).unwrap().1;
        let (s, v) = w
            .call(
                &w.admin,
                Method::GET,
                &format!("/v1/skill-libraries/{id}/skills"),
                None,
            )
            .await;
        assert_eq!((s, code(&v)), (502, "library_unavailable"), "{status}: {v}");
        let m = v["error"]["message"].as_str().unwrap();
        assert!(m.contains(needle), "{status}: {m}");
        assert!(!m.contains("tok") && !m.contains("Bearer") && !m.contains("127.0.0.1"));
    }
    // a redirect is reported, never followed
    w.git.clear_overrides();
    w.git.redirect("/commits/", "http://evil.example/");
    let id = libs.iter().find(|(l, _)| *l == "redir").unwrap().1;
    let (s, v) = w
        .call(
            &w.admin,
            Method::GET,
            &format!("/v1/skill-libraries/{id}/skills"),
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (502, "library_unavailable"), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("redirect"));
}

// ---- add ------------------------------------------------------------------------------------

#[tokio::test]
async fn add_pins_the_exact_requested_commit_even_when_the_head_moved() {
    let w = world().await;
    let sha1 = w.seed();
    let lib = w.library().await;
    let sha2 = w.git.commit(
        "acme/skills",
        "main",
        &[put("alpha/SKILL.md", md("Alpha v2", "Alpha body v2"))],
    );
    w.git.clear_requests();
    let (s, v) = w.add_org(lib, "alpha", &sha1).await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["tier"], "org");
    assert_eq!(v["body"], "Alpha body v1\n");
    assert_eq!(v["description"], "Alpha skill");
    assert_eq!(v["files"][0]["path"], "notes.txt");
    assert_eq!(v["source"]["kind"], "library");
    assert_eq!(v["source"]["commit_sha"], sha1);
    assert_eq!(
        (v["source"]["repo"].as_str(), v["source"]["path"].as_str()),
        (Some("acme/skills"), Some("alpha"))
    );
    assert_eq!(v["source"]["detached"], false);
    // a different head is never fetched
    assert_eq!(w.git.count(&format!("/trees/{sha2}")), 0);
    assert_eq!(
        w.git.count("/commits/"),
        0,
        "no ref is resolved when a commit is quoted"
    );
    let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let row: (Option<String>, Option<Vec<u8>>, serde_json::Value) = sqlx::query_as(
        "SELECT library_commit_sha, library_content_sha256, library_blobs FROM skills WHERE id = $1",
    )
    .bind(id)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(row.0.as_deref(), Some(sha1.as_str()));
    assert_eq!(row.1.unwrap().len(), 32);
    assert!(row.2.get("SKILL.md").is_some() && row.2.get("notes.txt").is_some());
    assert_eq!(w.audit_critical("skill.library.add").await, 1);
    // the commit is required and must be a full id
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &format!("/v1/orgs/{}/skills", w.org),
            Some(json!({"from_library": {"library_id": lib, "path": "beta"}})),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "commit_required"), "{v}");
    let (s, v) = w.add_org(lib, "beta", "main").await;
    assert_eq!((s, code(&v)), (400, "invalid_request"), "{v}");
    // from_library excludes the manual fields; manual creation still needs them
    let (s, v) = w
        .call(&w.admin, Method::POST, &format!("/v1/orgs/{}/skills", w.org),
            Some(json!({"name": "x", "from_library": {"library_id": lib, "path": "beta", "commit_sha": sha1}})))
        .await;
    assert_eq!(s, 400, "{v}");
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &format!("/v1/orgs/{}/skills", w.org),
            Some(json!({})),
        )
        .await;
    assert_eq!(s, 400, "{v}");
    // the review endpoint reads the pin
    let (s, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/skills/{id}/library"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (v["commit_sha"].as_str(), v["library"]["repo"].as_str()),
        (Some(sha1.as_str()), Some("acme/skills"))
    );
    assert!(v["update"].is_null());
}

#[tokio::test]
async fn add_follows_each_tiers_write_authority_and_refuses_agent_credentials() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    // org tier: owner/admin only
    for who in [&w.wsowner, &w.member] {
        let (s, v) = w
            .call(
                who,
                Method::POST,
                &format!("/v1/orgs/{}/skills", w.org),
                Some(W::from_library(lib, "alpha", &sha)),
            )
            .await;
        assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    }
    let (s, v) = w.add_org(lib, "alpha", &sha).await;
    assert_eq!(s, 201, "{v}");
    // workspace tier: can_own (a plain member cannot, an org admin can)
    let (s, v) = w
        .call(
            &w.member,
            Method::POST,
            &format!("/v1/workspaces/{}/skills", w.ws),
            Some(W::from_library(lib, "alpha", &sha)),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    let (s, v) = w.add_ws(lib, "alpha", &sha).await;
    assert_eq!((s, v["tier"].as_str()), (201, Some("workspace")), "{v}");
    let (s, _) = w
        .call(
            &w.admin,
            Method::POST,
            &format!("/v1/workspaces/{}/skills", w.ws),
            Some(W::from_library(lib, "beta", &sha)),
        )
        .await;
    assert_eq!(s, 201);
    // agent tier: can_use
    let (s, v) = w.add_agent(lib, "alpha", &sha).await;
    assert_eq!((s, v["tier"].as_str()), (201, Some("agent")), "{v}");
    let (s, _) = w
        .call(
            &w.unbound,
            Method::POST,
            &format!("/v1/agents/{}/skills", w.agent),
            Some(W::from_library(lib, "beta", &sha)),
        )
        .await;
    assert_eq!(s, 404, "no access to the workspace");
    // an API key of an org admin may add (session or API key)
    let admin_user: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = 'admin@x.io'")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    let (_, key) = w.h.api_key(admin_user, w.org, None, false).await;
    let b = bearer(&key);
    let (s, _, v) =
        w.h.send(
            Method::POST,
            &format!("/v1/orgs/{}/skills", w.org),
            &[("authorization", &b)],
            Some(W::from_library(lib, "beta", &sha)),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    // an agent credential is no principal here at all
    let (s, _, v) =
        w.h.send(
            Method::POST,
            &format!("/v1/agents/{}/skills", w.agent),
            &[("authorization", "Bearer aug_deadbeef")],
            Some(W::from_library(lib, "alpha", &sha)),
        )
        .await;
    assert_eq!(s, 401, "{v}");
    // a library of another organization is not visible
    let other = w.h.org().await;
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &format!("/v1/orgs/{other}/skills"),
            Some(W::from_library(lib, "alpha", &sha)),
        )
        .await;
    assert_eq!(s, 404, "{v}");
    assert_eq!(w.audit_critical("skill.library.add").await, 5);
}

#[tokio::test]
async fn add_validation_refuses_and_stores_nothing() {
    let w = world().await;
    w.git.add_repo("acme/skills");
    let big_body = "x".repeat(25_000);
    let sha = w.git.commit(
        "acme/skills",
        "main",
        &[
            put("ok/SKILL.md", md("fine", "b")),
            put("sym/SKILL.md", md("d", "b")),
            Change::Symlink("sym/link".into(), "ok".into()),
            put("sub/SKILL.md", md("d", "b")),
            Change::Submodule("sub/vendor".into()),
            put("bigfile/SKILL.md", md("d", "b")),
            put("bigfile/blob.bin", vec![0u8; 65 * 1024]),
            put(
                "badname/SKILL.md",
                b"---\nname: someone-else\ndescription: d\n---\nb\n".to_vec(),
            ),
            put("nodesc/SKILL.md", b"---\nname: nodesc\n---\nb\n".to_vec()),
            put("bigbody/SKILL.md", md("d", &big_body)),
            put("bigtotal/SKILL.md", md("d", &"y".repeat(20_000))),
            put("bigtotal/a.txt", "z".repeat(14_000).into_bytes()),
            put("aurora-x/SKILL.md", md("d", "b")),
            put("Caps/SKILL.md", md("d", "b")),
            put("weird/SKILL.md", md("d", "b")),
            put("weird/a b.txt", b"space".to_vec()),
            put("skillmd/SKILL.md", md("d", "b")),
            put("skillmd/docs/skill.md", b"nested".to_vec()),
            put("lower/skill.md", md("d", "b")),
        ],
    );
    let lib = w.library().await;
    let before = w.snapshot().await;
    for (dir, needle) in [
        ("sym", "symlink"),
        ("sub", "submodule"),
        ("bigfile", "larger"),
        ("badname", "name"),
        ("nodesc", "description"),
        ("bigbody", "body"),
        ("bigtotal", "total"),
        ("aurora-x", "name"),
        ("Caps", "name"),
        ("weird", "path"),
        ("skillmd", "path"),
        ("lower", "SKILL.md"),
        ("missing", "no skill"),
    ] {
        let (s, v) = w.add_org(lib, dir, &sha).await;
        assert_eq!((s, code(&v)), (422, "library_skill_invalid"), "{dir}: {v}");
        let msg = v["error"]["message"].as_str().unwrap().to_lowercase();
        assert!(msg.contains(&needle.to_lowercase()), "{dir}: {msg}");
        assert_eq!(w.snapshot().await, before, "{dir} stored something");
    }
    // a valid one in the same repository is fine, and a body with ordinary markdown is stored as is
    let (s, v) = w.add_org(lib, "ok", &sha).await;
    assert_eq!(s, 201, "{v}");
    // the same name at the same tier: 409
    let (s, v) = w.add_org(lib, "ok", &sha).await;
    assert_eq!((s, code(&v)), (409, "skill_exists"), "{v}");
}

#[tokio::test]
async fn names_are_unique_per_tier_and_precedence_is_unchanged() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    let (s, o) = w.add_org(lib, "alpha", &sha).await;
    assert_eq!(s, 201, "{o}");
    let (s, a) = w.add_agent(lib, "alpha", &sha).await;
    assert_eq!(s, 201, "{a}");
    let (s, v) = w.add_agent(lib, "alpha", &sha).await;
    assert_eq!((s, code(&v)), (409, "skill_exists"), "{v}");
    let (s, e) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/agents/{}/skills/effective", w.agent),
            None,
        )
        .await;
    assert_eq!(s, 200, "{e}");
    let items = e["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["tier"], "agent", "agent > workspace > org");
    assert_eq!(items[0]["shadows"][0]["tier"], "org");
    assert_eq!(items[0]["skill"]["source"]["commit_sha"], sha);
    assert_eq!(e["shadowed"][0]["skill"]["source"]["kind"], "library");
}

#[tokio::test]
async fn budgets_apply_at_add_and_approve() {
    // a tiny per-agent budget (the operator may only lower it)
    let w = world_with(|c| c.skills_library.bytes_per_agent = 3_000).await;
    w.git.add_repo("acme/skills");
    let sha = w.git.commit(
        "acme/skills",
        "main",
        &[
            put("one/SKILL.md", md("d", &"a".repeat(1_500))),
            put("two/SKILL.md", md("d", &"b".repeat(1_500))),
            put("three/SKILL.md", md("d", &"c".repeat(100))),
        ],
    );
    let lib = w.library().await;
    let (s, v) = w.add_agent(lib, "one", &sha).await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = w.add_agent(lib, "two", &sha).await;
    assert_eq!((s, code(&v)), (422, "library_budget_exceeded"), "{v}");
    assert_eq!(v["error"]["details"]["tier"], "agent");
    assert_eq!(v["error"]["details"]["bytes_limit"], 3_000);
    let (s, v) = w.add_agent(lib, "three", &sha).await;
    assert_eq!(s, 201, "the budget counts only what was added: {v}");
    // count budget: 25 skills per agent
    let w = world().await;
    w.git.add_repo("acme/skills");
    let changes: Vec<Change> = (0..26)
        .map(|i| put(&format!("s{i:02}/SKILL.md"), md("d", "b")))
        .collect();
    let sha = w.git.commit("acme/skills", "main", &changes);
    let lib = w.library().await;
    for i in 0..25 {
        let (s, v) = w.add_agent(lib, &format!("s{i:02}"), &sha).await;
        assert_eq!(s, 201, "{i}: {v}");
    }
    let (s, v) = w.add_agent(lib, "s25", &sha).await;
    assert_eq!((s, code(&v)), (422, "library_budget_exceeded"), "{v}");
    // approve is re-checked: an update that no longer fits stays pending, "over budget"
    let w = world_with(|c| c.skills_library.bytes_per_agent = 2_000).await;
    w.git.add_repo("acme/skills");
    let sha = w.git.commit(
        "acme/skills",
        "main",
        &[put("one/SKILL.md", md("d", &"a".repeat(1_000)))],
    );
    let lib = w.library().await;
    let (s, v) = w.add_agent(lib, "one", &sha).await;
    assert_eq!(s, 201, "{v}");
    let skill: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    w.git.commit(
        "acme/skills",
        "main",
        &[put("one/SKILL.md", md("d", &"a".repeat(2_500)))],
    );
    w.refresh(lib).await;
    let p = w.pending(skill).await;
    assert_eq!(p["update"]["kind"], "changed");
    assert_eq!(p["update"]["over_budget"], true);
    let hash = p["update"]["content_sha256"].as_str().unwrap();
    let uid = p["update"]["id"].as_str().unwrap();
    let (s, v) = w
        .call(
            &w.member,
            Method::POST,
            &format!("/v1/skills/{skill}/library/updates/{uid}/approve"),
            Some(json!({"content_sha256": hash})),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "library_budget_exceeded"), "{v}");
    let (_, p) = (0, w.pending(skill).await);
    assert!(p["update"]["id"].is_string(), "still pending");
}

// ---- read-only, detach, deletion ------------------------------------------------------------

#[tokio::test]
async fn library_skills_are_read_only_through_every_route() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    let (_, v) = w.add_org(lib, "alpha", &sha).await;
    let id = v["id"].as_str().unwrap();
    let one = format!("/v1/skills/{id}");
    for body in [
        json!({"body": "x"}),
        json!({"description": "x"}),
        json!({"body": "x", "enabled": false}),
    ] {
        let (s, v) = w.call(&w.admin, Method::PATCH, &one, Some(body)).await;
        assert_eq!((s, code(&v)), (409, "library_skill_readonly"), "{v}");
    }
    let put_file = |m: Method, p: &str| {
        let req = Request::builder()
            .method(m)
            .uri(format!("{one}/files/{p}"))
            .header("cookie", &w.admin)
            .header("origin", ORIGIN)
            .header("content-type", "application/octet-stream")
            .body(Body::from("new"))
            .unwrap();
        w.h.app.clone().oneshot(req)
    };
    let res = put_file(Method::PUT, "extra.txt").await.unwrap();
    assert_eq!(res.status(), 409);
    let res = put_file(Method::DELETE, "notes.txt").await.unwrap();
    assert_eq!(res.status(), 409);
    // enable/disable is allowed
    let (s, v) = w
        .call(
            &w.admin,
            Method::PATCH,
            &one,
            Some(json!({"enabled": false})),
        )
        .await;
    assert_eq!((s, v["enabled"].as_bool()), (200, Some(false)), "{v}");
    let (s, _) = w
        .call(
            &w.admin,
            Method::PATCH,
            &one,
            Some(json!({"enabled": true})),
        )
        .await;
    assert_eq!(s, 200);
    // content is unchanged
    let (_, v) = w.call(&w.admin, Method::GET, &one, None).await;
    assert_eq!(v["body"], "Alpha body v1\n");
    assert_eq!(v["files"].as_array().unwrap().len(), 1);
    // remove (delete) is the tier's authority
    let (s, _) = w.call(&w.member, Method::DELETE, &one, None).await;
    assert_eq!(s, 403);
    let (s, _) = w.call(&w.admin, Method::DELETE, &one, None).await;
    assert_eq!(s, 204);
}

#[tokio::test]
async fn detach_and_library_deletion_keep_the_content() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    let (_, a) = w.add_org(lib, "alpha", &sha).await;
    let (_, b) = w.add_agent(lib, "beta", &sha).await;
    let (a, b) = (
        a["id"].as_str().unwrap().to_owned(),
        b["id"].as_str().unwrap().to_owned(),
    );
    // detach: session only, tier authority
    let url = format!("/v1/skills/{a}/library/detach");
    let admin_user: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = 'admin@x.io'")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    let (_, key) = w.h.api_key(admin_user, w.org, None, false).await;
    let bk = bearer(&key);
    let (s, _, v) =
        w.h.send(Method::POST, &url, &[("authorization", &bk)], None)
            .await;
    assert_eq!((s.as_u16(), code(&v)), (403, "session_required"), "{v}");
    let (s, v) = w.call(&w.member, Method::POST, &url, None).await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    let (s, v) = w.call(&w.admin, Method::POST, &url, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["source"]["detached"], true);
    assert_eq!(v["source"]["commit_sha"], sha);
    assert!(v["source"]["library_id"].is_null());
    assert_eq!(w.audit_critical("skill.library.detach").await, 1);
    // now an ordinary editable skill
    let (s, v) = w
        .call(
            &w.admin,
            Method::PATCH,
            &format!("/v1/skills/{a}"),
            Some(json!({"body": "my edit"})),
        )
        .await;
    assert_eq!((s, v["body"].as_str()), (200, Some("my edit")), "{v}");
    let (s, v) = w.call(&w.admin, Method::POST, &url, None).await;
    assert_eq!((s, code(&v)), (409, "not_a_library_skill"), "{v}");
    // deleting the library detaches the rest and keeps their content
    let (s, _) = w
        .call(
            &w.admin,
            Method::DELETE,
            &format!("/v1/skill-libraries/{lib}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    let (s, v) = w
        .call(&w.member, Method::GET, &format!("/v1/skills/{b}"), None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["body"], "Beta body v1\n");
    assert_eq!(
        (
            v["source"]["detached"].as_bool(),
            v["source"]["repo"].as_str()
        ),
        (Some(true), Some("acme/skills"))
    );
    let (s, _) = w
        .call(
            &w.member,
            Method::PATCH,
            &format!("/v1/skills/{b}"),
            Some(json!({"body": "edit"})),
        )
        .await;
    assert_eq!(s, 200, "editable after the library is gone");
    assert_eq!(
        w.count("SELECT count(*) FROM skill_library_updates WHERE status = 'pending'")
            .await,
        0
    );
}

// ---- refresh --------------------------------------------------------------------------------

#[tokio::test]
async fn refresh_unchanged_head_costs_one_call_and_changes_nothing() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    let (_, v) = w.add_org(lib, "alpha", &sha).await;
    let _ = v;
    let before = w.snapshot().await;
    w.git.clear_requests();
    w.refresh(lib).await;
    assert_eq!(w.git.count("/commits/"), 1, "one resolve_ref");
    assert_eq!(w.git.count("/trees/"), 0);
    assert_eq!(w.git.requests().len(), 1);
    let l = w.library_row(lib).await;
    assert_eq!(l["status"], "active");
    assert!(l["last_refresh_at"].is_string());
    assert_eq!(w.snapshot().await, before);
    assert_eq!(
        w.count("SELECT count(*) FROM skill_library_updates").await,
        0
    );
}

#[tokio::test]
async fn sec_83_a_push_only_ever_creates_proposals() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    for tier in ["org", "ws", "agent"] {
        let (s, v) = match tier {
            "org" => w.add_org(lib, "alpha", &sha).await,
            "ws" => w.add_ws(lib, "alpha", &sha).await,
            _ => w.add_agent(lib, "alpha", &sha).await,
        };
        assert_eq!(s, 201, "{v}");
    }
    let (_, beta) = w.add_org(lib, "beta", &sha).await;
    let beta: Uuid = beta["id"].as_str().unwrap().parse().unwrap();
    let before = w.snapshot().await;
    // changed content, a file added, the executable bit flipped, and a skill removed
    let head = w.git.commit(
        "acme/skills",
        "main",
        &[
            put("alpha/SKILL.md", md("Alpha v2", "Alpha body v2")),
            put("alpha/new.txt", b"new file".to_vec()),
            Change::DeleteDir("beta".into()),
        ],
    );
    w.refresh(lib).await;
    // nothing a person did not approve moved: no skill, file, generation or projection
    assert_eq!(
        w.snapshot().await,
        before,
        "a refresh changed what agents are told"
    );
    assert_eq!(
        w.count("SELECT count(*) FROM skill_library_updates WHERE status = 'pending'")
            .await,
        4
    );
    let l = w.library_row(lib).await;
    assert_eq!(l["pending_updates"], 4);
    assert_eq!(l["last_head_sha"], head);
    let p = w.pending(beta).await;
    assert_eq!(
        (
            p["update"]["kind"].as_str(),
            p["update"]["commit_sha"].as_str()
        ),
        (Some("removed"), Some(head.as_str()))
    );
    // the whole diff is shown, never a summary
    let alpha: Uuid = sqlx::query_scalar(
        "SELECT id FROM skills WHERE name = 'alpha' AND agent_id IS NULL AND workspace_id IS NULL",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    let p = w.pending(alpha).await;
    let d = &p["update"]["diff"];
    assert_eq!(
        (d["body"]["old"].as_str(), d["body"]["new"].as_str()),
        (Some("Alpha body v1\n"), Some("Alpha body v2\n"))
    );
    assert_eq!(d["description"]["changed"], true);
    let files: Vec<(&str, &str)> = d["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["path"].as_str().unwrap(), f["change"].as_str().unwrap()))
        .collect();
    assert_eq!(
        files,
        vec![("new.txt", "added"), ("notes.txt", "unchanged")]
    );
    assert!(w.audit_critical("skill.library.update_apply").await == 0);
    // best-effort system rows are written off the request path: wait for them
    let mut proposed = 0;
    for _ in 0..50 {
        proposed = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM audit_log WHERE action = 'skill.library.update_proposed' AND NOT critical")
            .fetch_one(w.h.pool()).await.unwrap();
        if proposed == 4 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(proposed, 4, "best-effort system rows");
    // the pending badge lists
    let (s, v) = w
        .call(
            &w.admin,
            Method::GET,
            &format!("/v1/orgs/{}/skill-library-updates", w.org),
            None,
        )
        .await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 4), "{v}");
    let (s, v) = w
        .call(
            &w.wsowner,
            Method::GET,
            &format!("/v1/workspaces/{}/skill-library-updates", w.ws),
            None,
        )
        .await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 1), "{v}");
    let (s, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/workspaces/{}/skill-library-updates", w.ws),
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    let (s, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/agents/{}/skill-library-updates", w.agent),
            None,
        )
        .await;
    assert_eq!((s, v["items"].as_array().unwrap().len()), (200, 1), "{v}");
}

#[tokio::test]
async fn refresh_proposes_invalid_supersedes_and_remembers_a_rejection() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    let (_, v) = w.add_org(lib, "alpha", &sha).await;
    let alpha: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // the repository breaks the skill: invalid, with a reason, cannot be approved
    w.git.commit(
        "acme/skills",
        "main",
        &[Change::Symlink("alpha/link".into(), "/etc".into())],
    );
    w.refresh(lib).await;
    let p = w.pending(alpha).await;
    assert_eq!(p["update"]["kind"], "invalid");
    assert!(p["update"]["reason"].as_str().unwrap().contains("symlink"));
    assert!(p["update"]["diff"].is_null());
    let uid = p["update"]["id"].as_str().unwrap().to_owned();
    let hash = p["update"]["content_sha256"].as_str().unwrap().to_owned();
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &format!("/v1/skills/{alpha}/library/updates/{uid}/approve"),
            Some(json!({"content_sha256": hash})),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "library_skill_invalid"), "{v}");
    // a newer proposal supersedes the older one: one pending per skill
    w.git.commit(
        "acme/skills",
        "main",
        &[
            Change::Delete("alpha/link".into()),
            put("alpha/SKILL.md", md("Alpha v2", "v2")),
        ],
    );
    w.refresh(lib).await;
    let p2 = w.pending(alpha).await;
    assert_eq!(p2["update"]["kind"], "changed");
    assert_ne!(p2["update"]["id"], uid);
    assert_eq!(
        w.count("SELECT count(*) FROM skill_library_updates WHERE status = 'superseded'")
            .await,
        1
    );
    assert_eq!(
        w.count("SELECT count(*) FROM skill_library_updates WHERE status = 'pending'")
            .await,
        1
    );
    // approving the superseded one: 409
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &format!("/v1/skills/{alpha}/library/updates/{uid}/approve"),
            Some(json!({"content_sha256": hash})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "update_superseded"), "{v}");
    // reject: that repository state is not proposed again, a later change is
    let uid2 = p2["update"]["id"].as_str().unwrap().to_owned();
    let (s, _) = w
        .call(
            &w.member,
            Method::POST,
            &format!("/v1/skills/{alpha}/library/updates/{uid2}/reject"),
            None,
        )
        .await;
    assert_eq!(s, 403, "a member has no authority at the org tier");
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &format!("/v1/skills/{alpha}/library/updates/{uid2}/reject"),
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(w.audit_critical("skill.library.update_reject").await, 1);
    w.refresh(lib).await;
    assert_eq!(
        w.count("SELECT count(*) FROM skill_library_updates WHERE status = 'pending'")
            .await,
        0
    );
    w.git.commit(
        "acme/skills",
        "main",
        &[put("alpha/SKILL.md", md("Alpha v3", "v3"))],
    );
    w.refresh(lib).await;
    assert_eq!(w.pending(alpha).await["update"]["kind"], "changed");
    // going back to the pinned state withdraws the open proposal
    let (_, orig) = (
        0,
        w.git.commit(
            "acme/skills",
            "main",
            &[put("alpha/SKILL.md", md("Alpha skill", "Alpha body v1"))],
        ),
    );
    let _ = orig;
    w.refresh(lib).await;
    assert!(w.pending(alpha).await["update"].is_null());
}

#[tokio::test]
async fn refresh_errors_back_off_and_leave_pinned_skills_alone() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    w.add_org(lib, "alpha", &sha).await;
    let before = w.snapshot().await;
    // the git host fails: error with a reason, 15 min -> 1 h -> 6 h
    w.git.override_status("/commits/", 500);
    let mut waits = Vec::new();
    for _ in 0..4 {
        w.refresh(lib).await;
        let l = w.library_row(lib).await;
        assert_eq!(l["status"], "error", "{l}");
        assert!(l["last_error"].as_str().unwrap().contains("500"));
        let next: f64 = sqlx::query_scalar("SELECT extract(epoch FROM next_refresh_at - now())::float8 FROM skill_libraries WHERE id = $1")
            .bind(lib).fetch_one(w.h.pool()).await.unwrap();
        waits.push(next);
    }
    assert!((800.0..=1000.0).contains(&waits[0]), "{waits:?}");
    assert!((3200.0..=4000.0).contains(&waits[1]), "{waits:?}");
    assert!(
        (19_000.0..=24_000.0).contains(&waits[2]) && (19_000.0..=24_000.0).contains(&waits[3]),
        "{waits:?}"
    );
    assert_eq!(w.snapshot().await, before);
    // it recovers on its own and the backoff resets
    w.git.clear_overrides();
    w.refresh(lib).await;
    let l = w.library_row(lib).await;
    assert_eq!(
        (l["status"].as_str(), l["last_error"].is_null()),
        (Some("active"), true),
        "{l}"
    );
    // connection revoked / reauth_required / deleted, package without a library
    sqlx::query("UPDATE app_connections SET status = 'reauth_required' WHERE id = $1")
        .bind(w.conn)
        .execute(w.h.pool())
        .await
        .unwrap();
    w.refresh(lib).await;
    let l = w.library_row(lib).await;
    assert_eq!(l["status"], "error");
    assert!(
        l["last_error"]
            .as_str()
            .unwrap()
            .contains("reauth_required"),
        "{l}"
    );
    sqlx::query("UPDATE app_connections SET status = 'active' WHERE id = $1")
        .bind(w.conn)
        .execute(w.h.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE skill_libraries SET app_id = 'plain' WHERE id = $1")
        .bind(lib)
        .execute(w.h.pool())
        .await
        .unwrap();
    w.refresh(lib).await;
    assert!(
        w.library_row(lib).await["last_error"]
            .as_str()
            .unwrap()
            .contains("no longer supports")
    );
    sqlx::query("UPDATE skill_libraries SET app_id = 'forge' WHERE id = $1")
        .bind(lib)
        .execute(w.h.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM app_connections WHERE id = $1")
        .bind(w.conn)
        .execute(w.h.pool())
        .await
        .unwrap();
    w.refresh(lib).await;
    let l = w.library_row(lib).await;
    assert_eq!(
        (l["status"].as_str(), l["last_error"].as_str()),
        (Some("error"), Some("connection_deleted")),
        "{l}"
    );
    assert!(l["connection"].is_null());
    assert_eq!(
        w.snapshot().await,
        before,
        "pinned skills keep working unchanged"
    );
    // the daily retention also marks a library whose connection vanished
    let (_, m) = refresh::retention(&w.h.state).await.unwrap().unwrap();
    assert_eq!(m, 0, "already marked");
}

#[tokio::test]
async fn refresh_runs_only_for_libraries_with_skills_under_a_lease() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    sqlx::query("UPDATE skill_libraries SET next_refresh_at = now() - interval '1 second'")
        .execute(w.h.pool())
        .await
        .unwrap();
    let mut c = w.h.pool().acquire().await.unwrap();
    let (r1, r2) = (Uuid::now_v7(), Uuid::now_v7());
    assert!(
        repo::claim_due(&mut c, r1, 4, 300.0)
            .await
            .unwrap()
            .is_empty(),
        "no added skill, no refresh"
    );
    w.add_org(lib, "alpha", &sha).await;
    // two replicas never refresh one library
    let a = repo::claim_due(&mut c, r1, 4, 300.0).await.unwrap();
    let b = repo::claim_due(&mut c, r2, 4, 300.0).await.unwrap();
    assert_eq!((a.len(), b.len()), (1, 0));
    // the lease expires
    sqlx::query("UPDATE skill_libraries SET refresh_lease_until = now() - interval '1 second'")
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(
        repo::claim_due(&mut c, r2, 4, 300.0).await.unwrap().len(),
        1
    );
    // only the lease owner records the outcome
    let done = repo::RefreshDone {
        id: lib,
        owner: r1,
        head_sha: Some("x"),
        head_epoch: 0,
        status: "active",
        error: None,
        failures: 0,
        next_in_secs: 900.0,
    };
    assert!(!repo::finish_refresh(&mut c, &done).await.unwrap());
    // paused libraries are not claimed; the interval floor is 5 minutes
    sqlx::query("UPDATE skill_libraries SET status = 'paused', refresh_lease_until = NULL")
        .execute(w.h.pool())
        .await
        .unwrap();
    assert!(
        repo::claim_due(&mut c, r1, 4, 300.0)
            .await
            .unwrap()
            .is_empty()
    );
    for _ in 0..50 {
        assert!(refresh::next_in_secs(300, 0) >= 270.0);
        assert!(refresh::next_in_secs(900, 0) >= 810.0);
    }
    // "Check now" at most once a minute
    sqlx::query(
        "UPDATE skill_libraries SET status = 'active', next_refresh_at = now() + interval '1 hour'",
    )
    .execute(w.h.pool())
    .await
    .unwrap();
    let (s, _) = w
        .call(
            &w.admin,
            Method::POST,
            &format!("/v1/skill-libraries/{lib}/refresh"),
            None,
        )
        .await;
    assert_eq!(s, 202);
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &format!("/v1/skill-libraries/{lib}/refresh"),
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (429, "library_check_too_soon"), "{v}");
    let (s, _) = w
        .call(
            &w.member,
            Method::POST,
            &format!("/v1/skill-libraries/{lib}/refresh"),
            None,
        )
        .await;
    assert_eq!(s, 403);
}

#[tokio::test]
async fn open_proposals_are_bounded_per_organization() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    let (_, v) = w.add_org(lib, "alpha", &sha).await;
    let alpha: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (_, v) = w.add_org(lib, "beta", &sha).await;
    let beta: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    // 4 MiB of open proposals already (a row of another skill)
    sqlx::query("INSERT INTO skill_library_updates (id, skill_id, library_id, kind, status, commit_sha, content_sha256, bytes) VALUES ($1,$2,$3,'removed','pending','x','\\x00',4194300)")
        .bind(Uuid::now_v7()).bind(beta).bind(lib).execute(w.h.pool()).await.unwrap();
    w.git.commit(
        "acme/skills",
        "main",
        &[put("alpha/SKILL.md", md("Alpha v2", "v2"))],
    );
    w.refresh(lib).await;
    let l = w.library_row(lib).await;
    assert_eq!(l["status"], "over_budget", "{l}");
    assert!(w.pending(alpha).await["update"].is_null());
    // space is freed: the job picks the change up on its next pass
    sqlx::query("DELETE FROM skill_library_updates")
        .execute(w.h.pool())
        .await
        .unwrap();
    w.refresh(lib).await;
    assert_eq!(w.pending(alpha).await["update"]["kind"], "changed");
    assert_eq!(w.library_row(lib).await["status"], "active");
}

/// FIX-066: a pass cut short by the budget must not record the head it was comparing — or a
/// later pass at the same head (e.g. after an intervening error/active flip) would short-circuit
/// as "unchanged" and never compare the skills the pass did not reach.
#[tokio::test]
async fn over_budget_pass_does_not_advance_the_head_and_the_next_pass_compares_the_rest() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    let (_, v) = w.add_org(lib, "alpha", &sha).await;
    let alpha: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let (_, v) = w.add_org(lib, "beta", &sha).await;
    let beta: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("INSERT INTO skill_library_updates (id, skill_id, library_id, kind, status, commit_sha, content_sha256, bytes) VALUES ($1,$2,$3,'removed','pending','x','\\x00',4194300)")
        .bind(Uuid::now_v7()).bind(beta).bind(lib).execute(w.h.pool()).await.unwrap();
    let head = w.git.commit(
        "acme/skills",
        "main",
        &[put("alpha/SKILL.md", md("Alpha v2", "v2"))],
    );
    w.refresh(lib).await;
    assert_eq!(w.library_row(lib).await["status"], "over_budget");
    let recorded: Option<String> =
        sqlx::query_scalar("SELECT last_head_sha FROM skill_libraries WHERE id = $1")
            .bind(lib)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_ne!(
        recorded.as_deref(),
        Some(head.as_str()),
        "a partial pass records no head"
    );
    // The state an intervening pass would leave (status back to active, same head), and the
    // space freed: the remaining skill is compared on the next pass.
    sqlx::query("UPDATE skill_libraries SET status = 'active' WHERE id = $1")
        .bind(lib)
        .execute(w.h.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM skill_library_updates")
        .execute(w.h.pool())
        .await
        .unwrap();
    w.refresh(lib).await;
    assert_eq!(w.pending(alpha).await["update"]["kind"], "changed");
}

/// A gateway that never answers (a stalled git host).
struct Stall;

#[async_trait]
impl LibraryGateway for Stall {
    async fn fetch(&self, _r: &FetchRequest) -> Result<FetchOk, GatewayError> {
        std::future::pending().await
    }
}

/// FIX-066: a stalled refresh gives up inside its bound (below the lease), records the failure
/// and releases the lease; while it ran, no other replica could claim the library.
#[tokio::test]
async fn a_stalled_refresh_gives_up_before_the_lease_and_releases_it() {
    assert!(
        aurora_core::limits::LIBRARY_REFRESH_BUDGET < aurora_core::limits::LIBRARY_LEASE,
        "the refresh bound is below the lease"
    );
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    w.add_org(lib, "alpha", &sha).await;
    sqlx::query("UPDATE skill_libraries SET next_refresh_at = now() - interval '1 second', refresh_lease_until = NULL, lease_owner = NULL WHERE id = $1")
        .bind(lib)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    let mut c = w.h.pool().acquire().await.unwrap();
    let mut mine = repo::claim_due(&mut c, a, 4, 300.0).await.unwrap();
    assert_eq!(mine.len(), 1);
    // A second replica cannot claim it while the lease runs.
    assert!(
        repo::claim_due(&mut c, b, 4, 300.0)
            .await
            .unwrap()
            .is_empty()
    );
    let stalled = w.h.state.clone().with_library_gateway(Arc::new(Stall));
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        refresh::refresh_library_within(
            &stalled,
            mine.remove(0),
            a,
            std::time::Duration::from_millis(300),
        ),
    )
    .await
    .expect("the refresh must give up on its own bound");
    let (status, err, lease): (String, Option<String>, Option<Uuid>) =
        sqlx::query_as("SELECT status, last_error, lease_owner FROM skill_libraries WHERE id = $1")
            .bind(lib)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(status, "error");
    assert!(err.unwrap_or_default().contains("too long"));
    assert!(lease.is_none(), "the lease is released");
}

/// FIX-066: members cannot browse, read or add from a paused library (404, like the list
/// hides it); admins still can.
#[tokio::test]
async fn paused_libraries_are_invisible_to_members_but_not_admins() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    let ok = w.add_agent(lib, "alpha", &sha).await.0;
    assert_eq!(ok, 201, "an active library is usable by a member");
    sqlx::query("UPDATE skill_libraries SET status = 'paused' WHERE id = $1")
        .bind(lib)
        .execute(w.h.pool())
        .await
        .unwrap();
    for uri in [
        format!("/v1/skill-libraries/{lib}/skills"),
        format!("/v1/skill-libraries/{lib}/skills/beta?commit_sha={sha}"),
    ] {
        let (s, v) = w.call(&w.member, Method::GET, &uri, None).await;
        assert_eq!((s, code(&v)), (404, "not_found"), "member {uri}: {v}");
        let (s, v) = w.call(&w.admin, Method::GET, &uri, None).await;
        assert_eq!(s, 200, "admin {uri}: {v}");
    }
    let (s, v) = w.add_agent(lib, "beta", &sha).await;
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    let (s, v) = w.add_org(lib, "beta", &sha).await;
    assert_eq!(s, 201, "an admin may still add: {v}");
}

/// FIX-066: concurrent creates at 9 libraries admit exactly one (the cap is checked under the
/// organization's lock).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creates_at_the_cap_admit_exactly_one() {
    let w = world().await;
    w.seed();
    for i in 0..9 {
        w.library_at(&format!("l{i}"), "").await;
    }
    let uri = format!("/v1/orgs/{}/skill-libraries", w.org);
    let results = futures::future::join_all((0..4).map(|i| {
        let body = json!({"label": format!("race{i}"), "connection_id": w.conn,
            "repo": "acme/skills", "path": ""});
        w.call(&w.admin, Method::POST, &uri, Some(body))
    }))
    .await;
    let created = results.iter().filter(|(s, _)| *s == 201).count();
    assert_eq!(created, 1, "{results:?}");
    assert!(
        results
            .iter()
            .filter(|(s, _)| *s != 201)
            .all(|(s, v)| *s == 409 && code(v) == "library_limit"),
        "{results:?}"
    );
    assert_eq!(w.count("SELECT count(*) FROM skill_libraries").await, 10);
}

// ---- approve --------------------------------------------------------------------------------

async fn propose_v2(w: &W, _lib: Uuid) -> String {
    w.git.commit(
        "acme/skills",
        "main",
        &[
            put("alpha/SKILL.md", md("Alpha v2", "Alpha body v2")),
            put("alpha/more.txt", b"more".to_vec()),
            Change::Delete("alpha/notes.txt".into()),
        ],
    )
}

#[tokio::test]
async fn sec_83_approve_applies_exactly_the_quoted_hash_session_only() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    let (_, o) = w.add_org(lib, "alpha", &sha).await;
    let alpha: Uuid = o["id"].as_str().unwrap().parse().unwrap();
    let (_, a) = w.add_agent(lib, "alpha", &sha).await;
    let agent_skill: Uuid = a["id"].as_str().unwrap().parse().unwrap();
    let head = propose_v2(&w, lib).await;
    w.refresh(lib).await;
    let p = w.pending(alpha).await;
    let uid = p["update"]["id"].as_str().unwrap().to_owned();
    let hash = p["update"]["content_sha256"].as_str().unwrap().to_owned();
    let url = format!("/v1/skills/{alpha}/library/updates/{uid}/approve");
    let admin_user: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = 'admin@x.io'")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    // an API key cannot approve
    let (_, key) = w.h.api_key(admin_user, w.org, None, false).await;
    let (s, _, v) =
        w.h.send(
            Method::POST,
            &url,
            &[("authorization", &bearer(&key))],
            Some(json!({"content_sha256": hash})),
        )
        .await;
    assert_eq!((s.as_u16(), code(&v)), (403, "session_required"), "{v}");
    // a member has no authority at the org tier
    let (s, v) = w
        .call(
            &w.member,
            Method::POST,
            &url,
            Some(json!({"content_sha256": hash})),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    // another content hash: not what the person reviewed
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &url,
            Some(json!({"content_sha256": "0".repeat(64)})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "update_superseded"), "{v}");
    let (s, _) = w
        .call(
            &w.admin,
            Method::POST,
            &url,
            Some(json!({"content_sha256": "zz"})),
        )
        .await;
    assert_eq!(s, 400);
    // nothing moved so far
    assert_eq!(w.pending(alpha).await["commit_sha"], sha);
    let gens = w.snapshot().await["agents"].clone();
    // approve
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &url,
            Some(json!({"content_sha256": hash})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["body"], "Alpha body v2\n");
    assert_eq!(v["description"], "Alpha v2");
    assert_eq!(v["source"]["commit_sha"], head);
    assert_eq!(v["files"].as_array().unwrap().len(), 1);
    assert_eq!(v["files"][0]["path"], "more.txt");
    assert_eq!(v["source"]["update_pending"], false);
    assert_eq!(w.audit_critical("skill.library.update_apply").await, 1);
    // the org-tier change bumped every agent of the org (exactly those affected)
    let after = w.snapshot().await["agents"].clone();
    assert_eq!(
        after[0][1].as_i64().unwrap(),
        gens[0][1].as_i64().unwrap() + 1
    );
    // the same approval again, and a decided update: 409
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &url,
            Some(json!({"content_sha256": hash})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "update_superseded"), "{v}");
    // the agent-tier copy still has its own pending update; approving it bumps that agent only
    let p = w.pending(agent_skill).await;
    let uid2 = p["update"]["id"].as_str().unwrap();
    let hash2 = p["update"]["content_sha256"].as_str().unwrap();
    let ws_before: Vec<(String, i64)> =
        sqlx::query_as("SELECT id::text, home_sync_generation FROM workspaces")
            .fetch_all(w.h.pool())
            .await
            .unwrap();
    let org_agent_gen: i64 =
        sqlx::query_scalar("SELECT home_sync_generation FROM agents WHERE id = $1")
            .bind(w.agent)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    let (s, v) = w
        .call(
            &w.member,
            Method::POST,
            &format!("/v1/skills/{agent_skill}/library/updates/{uid2}/approve"),
            Some(json!({"content_sha256": hash2})),
        )
        .await;
    assert_eq!(s, 200, "can_use is the agent tier's authority: {v}");
    let g2: i64 = sqlx::query_scalar("SELECT home_sync_generation FROM agents WHERE id = $1")
        .bind(w.agent)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(g2, org_agent_gen + 1);
    let ws_after: Vec<(String, i64)> =
        sqlx::query_as("SELECT id::text, home_sync_generation FROM workspaces")
            .fetch_all(w.h.pool())
            .await
            .unwrap();
    assert_eq!(ws_before, ws_after, "the workspace generation did not move");
    assert_eq!(w.audit_critical("skill.library.update_apply").await, 2);
}

#[tokio::test]
async fn approve_races_a_newer_proposal_and_applies_removals() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    let (_, o) = w.add_ws(lib, "alpha", &sha).await;
    let alpha: Uuid = o["id"].as_str().unwrap().parse().unwrap();
    let (_, b) = w.add_ws(lib, "beta", &sha).await;
    let beta: Uuid = b["id"].as_str().unwrap().parse().unwrap();
    propose_v2(&w, lib).await;
    w.git
        .commit("acme/skills", "main", &[Change::DeleteDir("beta".into())]);
    w.refresh(lib).await;
    let p = w.pending(alpha).await;
    let (uid, hash) = (
        p["update"]["id"].as_str().unwrap().to_owned(),
        p["update"]["content_sha256"].as_str().unwrap().to_owned(),
    );
    // while the person reads, a newer push arrives and the job proposes it
    w.git.commit(
        "acme/skills",
        "main",
        &[put("alpha/SKILL.md", md("Alpha v3", "Alpha body v3"))],
    );
    w.refresh(lib).await;
    let url = format!("/v1/skills/{alpha}/library/updates/{uid}/approve");
    let (s, v) = w
        .call(
            &w.wsowner,
            Method::POST,
            &url,
            Some(json!({"content_sha256": hash})),
        )
        .await;
    assert_eq!(
        (s, code(&v)),
        (409, "update_superseded"),
        "the content changed after review: {v}"
    );
    let p3 = w.pending(alpha).await;
    assert_eq!(p3["update"]["diff"]["body"]["new"], "Alpha body v3\n");
    // a workspace skill: can_own approves, a plain member cannot
    let url3 = format!(
        "/v1/skills/{alpha}/library/updates/{}/approve",
        p3["update"]["id"].as_str().unwrap()
    );
    let body3 = json!({"content_sha256": p3["update"]["content_sha256"]});
    let (s, v) = w
        .call(&w.member, Method::POST, &url3, Some(body3.clone()))
        .await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    let (s, v) = w.call(&w.wsowner, Method::POST, &url3, Some(body3)).await;
    assert_eq!(s, 200, "{v}");
    let ws_gen: i64 =
        sqlx::query_scalar("SELECT home_sync_generation FROM workspaces WHERE id = $1")
            .bind(w.ws)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(
        ws_gen >= 3,
        "adds and the approval bumped the workspace generation"
    );
    // an approved removal deletes the skill
    let pb = w.pending(beta).await;
    assert_eq!(pb["update"]["kind"], "removed");
    let (s, v) = w
        .call(
            &w.wsowner,
            Method::POST,
            &format!(
                "/v1/skills/{beta}/library/updates/{}/approve",
                pb["update"]["id"].as_str().unwrap()
            ),
            Some(json!({"content_sha256": pb["update"]["content_sha256"]})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        w.count(&format!("SELECT count(*) FROM skills WHERE id = '{beta}'"))
            .await,
        0
    );
    assert_eq!(w.audit_critical("skill.library.update_apply").await, 2);
}

#[tokio::test]
async fn sec_83_a_failing_or_removed_library_leaves_pinned_skills_as_they_were() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    w.add_org(lib, "alpha", &sha).await;
    w.add_agent(lib, "beta", &sha).await;
    let before = w.snapshot().await;
    // repository deleted, token revoked, library deleted: nothing the agents are told moves
    w.git.override_status("/", 404);
    w.refresh(lib).await;
    assert_eq!(w.library_row(lib).await["status"], "error");
    sqlx::query("UPDATE app_connections SET status = 'disabled' WHERE id = $1")
        .bind(w.conn)
        .execute(w.h.pool())
        .await
        .unwrap();
    w.refresh(lib).await;
    assert_eq!(w.snapshot().await, before);
    let (s, _) = w
        .call(
            &w.admin,
            Method::DELETE,
            &format!("/v1/skill-libraries/{lib}"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    let after = w.snapshot().await;
    assert_eq!(after["projection"], before["projection"]);
    assert_eq!(after["files"], before["files"]);
    assert_eq!(after["agents"], before["agents"]);
}

/// Adds `dir` of the seeded repository at `sha` to the org tier and returns the skill id.
async fn add_org_id(w: &W, lib: Uuid, dir: &str, sha: &str) -> Uuid {
    let (s, v) = w.add_org(lib, dir, sha).await;
    assert_eq!(s, 201, "{v}");
    v["id"].as_str().unwrap().parse().unwrap()
}

/// FIX-096: an approval re-validates the stored proposal like the refresh did (SEC-83): a path
/// with `..` or more than 50 files is a 422 and the proposal stays pending.
#[tokio::test]
async fn approve_revalidates_the_stored_proposal() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    let alpha = add_org_id(&w, lib, "alpha", &sha).await;
    propose_v2(&w, lib).await;
    w.refresh(lib).await;
    let p = w.pending(alpha).await;
    let uid = p["update"]["id"].as_str().unwrap().to_owned();
    let hash = p["update"]["content_sha256"].as_str().unwrap().to_owned();
    let url = format!("/v1/skills/{alpha}/library/updates/{uid}/approve");
    let many: Vec<Value> = (0..51)
        .map(|i| json!({"path": format!("f{i}.txt"), "content_b64": "", "executable": false}))
        .collect();
    for (files, want) in [
        (
            json!([{"path": "a/../b", "content_b64": "eA==", "executable": false}]),
            "path",
        ),
        (Value::Array(many), "more than 50 files"),
        (
            json!([{"path": "big.txt", "content_b64": "eA==", "executable": false},
                   {"path": "big.txt", "content_b64": "eA==", "executable": false}]),
            "twice",
        ),
    ] {
        sqlx::query(
            "UPDATE skill_library_updates SET proposed = jsonb_set(proposed, '{files}', $2) WHERE id = $1",
        )
        .bind(uid.parse::<Uuid>().unwrap())
        .bind(&files)
        .execute(w.h.pool())
        .await
        .unwrap();
        let (s, v) = w
            .call(
                &w.admin,
                Method::POST,
                &url,
                Some(json!({"content_sha256": hash})),
            )
            .await;
        assert_eq!((s, code(&v)), (422, "library_skill_invalid"), "{v}");
        assert!(
            v["error"]["message"].as_str().unwrap().contains(want),
            "{want}: {v}"
        );
    }
    assert_eq!(w.pending(alpha).await["update"]["id"], uid);
}

/// FIX-096 (D-178 Q-R5-6): a paused library's pending proposals cannot be approved (409
/// `library_paused`); they can still be rejected, and resuming makes them approvable again.
#[tokio::test]
async fn approving_a_paused_librarys_proposal_is_refused_but_rejecting_is_not() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    let alpha = add_org_id(&w, lib, "alpha", &sha).await;
    propose_v2(&w, lib).await;
    w.refresh(lib).await;
    let p = w.pending(alpha).await;
    let uid = p["update"]["id"].as_str().unwrap().to_owned();
    let hash = p["update"]["content_sha256"].as_str().unwrap().to_owned();
    let patch = |status: &'static str| {
        let (w, uri) = (&w, format!("/v1/skill-libraries/{lib}"));
        async move {
            w.call(
                &w.admin,
                Method::PATCH,
                &uri,
                Some(json!({"status": status})),
            )
            .await
        }
    };
    assert_eq!(patch("paused").await.0, 200);
    let approve = format!("/v1/skills/{alpha}/library/updates/{uid}/approve");
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &approve,
            Some(json!({"content_sha256": hash})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "library_paused"), "{v}");
    assert_eq!(patch("active").await.0, 200);
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &approve,
            Some(json!({"content_sha256": hash})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // and rejecting works while paused
    let beta = add_org_id(&w, lib, "beta", &w.git.head("acme/skills", "main")).await;
    w.git.commit(
        "acme/skills",
        "main",
        &[put("beta/SKILL.md", md("Beta v2", "Beta body v2"))],
    );
    w.refresh(lib).await;
    let p = w.pending(beta).await;
    let uid = p["update"]["id"].as_str().unwrap().to_owned();
    assert_eq!(patch("paused").await.0, 200);
    let (s, _) = w
        .call(
            &w.admin,
            Method::POST,
            &format!("/v1/skills/{beta}/library/updates/{uid}/reject"),
            None,
        )
        .await;
    assert_eq!(s, 204);
}

/// FIX-096 (D-178 Q-R5-3): browse and add take only the ref's current head and heads the
/// library recorded; a commit of another branch (unmerged, force-pushed away) is refused.
#[tokio::test]
async fn browse_and_add_accept_only_the_current_and_recorded_heads() {
    let w = world().await;
    let sha1 = w.seed();
    let lib = w.library().await;
    // an unmerged commit on another branch
    let other = w.git.commit(
        "acme/skills",
        "feature",
        &[put("alpha/SKILL.md", md("Alpha evil", "evil"))],
    );
    assert_ne!(other, sha1);
    let content = |sha: String| {
        let w = &w;
        async move {
            w.call(
                &w.admin,
                Method::GET,
                &format!("/v1/skill-libraries/{lib}/skills/alpha?commit_sha={sha}"),
                None,
            )
            .await
        }
    };
    let (s, v) = content(other.clone()).await;
    assert_eq!((s, code(&v)), (422, "commit_not_in_library"), "{v}");
    let (s, v) = w.add_org(lib, "alpha", &other).await;
    assert_eq!((s, code(&v)), (422, "commit_not_in_library"), "{v}");
    assert_eq!(w.count("SELECT count(*) FROM skills").await, 0);
    // the recorded head (taken at create) is accepted even after the head moved
    let sha2 = w.git.commit(
        "acme/skills",
        "main",
        &[put("alpha/SKILL.md", md("Alpha v2", "Alpha body v2"))],
    );
    let (s, v) = content(sha1.clone()).await;
    assert_eq!(s, 200, "{v}");
    // the current head (never recorded before) is accepted and recorded
    let (s, v) = content(sha2.clone()).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w.add_org(lib, "alpha", &sha2).await;
    assert_eq!(s, 201, "{v}");
    // a head that moved away without anything recording it is not accepted any more
    let sha3 = w.git.commit(
        "acme/skills",
        "main",
        &[put("beta/SKILL.md", md("Beta v2", "Beta body v2"))],
    );
    let sha4 = w.git.commit(
        "acme/skills",
        "main",
        &[put("beta/SKILL.md", md("Beta v3", "Beta body v3"))],
    );
    let (s, v) = content(sha4).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w.add_org(lib, "beta", &sha3).await;
    assert_eq!((s, code(&v)), (422, "commit_not_in_library"), "{v}");
}

/// FIX-096: an add of an older commit that lands between a pass's head read and its finish must
/// not be undone by the pass writing its head back (C-577, D-154(d)): the next pass proposes.
#[tokio::test]
async fn an_add_of_an_older_commit_during_a_pass_is_not_undone_by_its_finish() {
    let w = world().await;
    let sha1 = w.seed();
    let lib = w.library().await;
    add_org_id(&w, lib, "alpha", &sha1).await;
    let sha2 = w.git.commit(
        "acme/skills",
        "main",
        &[put("beta/SKILL.md", md("Beta v2", "Beta body v2"))],
    );
    w.refresh(lib).await;
    assert_eq!(
        w.library_row(lib).await["last_head_sha"],
        sha2,
        "the pass recorded the head"
    );
    // a pass claims the library and reads its row …
    sqlx::query("UPDATE skill_libraries SET next_refresh_at = now() - interval '1 second', refresh_lease_until = NULL, lease_owner = NULL WHERE id = $1")
        .bind(lib)
        .execute(w.h.pool())
        .await
        .unwrap();
    let owner = Uuid::now_v7();
    let mut c = w.h.pool().acquire().await.unwrap();
    let mut claimed = repo::claim_due(&mut c, owner, 4, 300.0).await.unwrap();
    assert_eq!(claimed.len(), 1);
    // … an add of the older commit lands meanwhile (clears the head) …
    let beta = add_org_id(&w, lib, "beta", &sha1).await;
    assert_eq!(w.library_row(lib).await["last_head_sha"], Value::Null);
    // … and the pass finishes at the head it read.
    refresh::refresh_library(&w.h.state, claimed.remove(0), owner).await;
    assert_eq!(
        w.library_row(lib).await["last_head_sha"],
        Value::Null,
        "the finished pass did not write its head back over the cleared one"
    );
    w.refresh(lib).await;
    assert_eq!(w.pending(beta).await["update"]["kind"], "changed");
}

/// FIX-096: two libraries of one organization refreshing together never leave more than 4 MiB
/// of open proposals (sum and insert run under the organization's lock).
#[tokio::test]
async fn two_libraries_racing_the_proposal_cap_never_exceed_it() {
    let w = world().await;
    let big = |t: &str| md(&format!("{t} big"), &"x".repeat(20_000));
    w.git.add_repo("acme/skills");
    let sha = w.git.commit(
        "acme/skills",
        "main",
        &[
            put("alpha/SKILL.md", md("Alpha", "a")),
            put("sub/gamma/SKILL.md", md("Gamma", "g")),
            put("beta/SKILL.md", md("Beta", "b")),
        ],
    );
    let l1 = w.library_at("one", "").await;
    let l2 = w.library_at("two", "sub").await;
    add_org_id(&w, l1, "alpha", &sha).await;
    let beta = add_org_id(&w, l1, "beta", &sha).await;
    add_org_id(&w, l2, "gamma", &sha).await;
    // the organization already holds all but ~30 000 bytes of open proposals: one of the two
    // new ~20 000-byte proposals fits, both do not
    sqlx::query("INSERT INTO skill_library_updates (id, skill_id, library_id, kind, status, commit_sha, content_sha256, bytes) VALUES ($1,$2,$3,'removed','pending','x','\\x00',$4)")
        .bind(Uuid::now_v7())
        .bind(beta)
        .bind(l1)
        .bind(4_194_304_i32 - 30_000)
        .execute(w.h.pool())
        .await
        .unwrap();
    w.git.commit(
        "acme/skills",
        "main",
        &[
            put("alpha/SKILL.md", big("Alpha")),
            put("sub/gamma/SKILL.md", big("Gamma")),
        ],
    );
    sqlx::query("UPDATE skill_libraries SET next_refresh_at = now() - interval '1 second', refresh_lease_until = NULL, lease_owner = NULL")
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(refresh::run_once(&w.h.state).await.unwrap(), 2);
    let total: i64 = w
        .count("SELECT COALESCE(sum(bytes), 0)::bigint FROM skill_library_updates WHERE status = 'pending'")
        .await;
    assert!(total <= 4_194_304, "open proposals total {total} bytes");
    let proposed = w
        .count("SELECT count(*) FROM skill_library_updates WHERE status = 'pending' AND kind = 'changed'")
        .await;
    assert_eq!(proposed, 1, "exactly one of the two fits");
}

/// FIX-096: parallel cold browses of one library cost one tree read and one read per SKILL.md.
#[tokio::test]
async fn parallel_cold_browses_share_one_read() {
    let w = world().await;
    w.seed();
    let lib = w.library().await;
    w.git.clear_requests();
    let uri = format!("/v1/skill-libraries/{lib}/skills");
    let calls = (0..8).map(|_| w.call(&w.admin, Method::GET, &uri, None));
    for (s, v) in futures::future::join_all(calls).await {
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["items"].as_array().unwrap().len(), 2);
    }
    assert_eq!(w.git.count("/git/trees/"), 1, "one tree read");
    assert_eq!(w.git.count("/git/blobs/"), 2, "one read per SKILL.md");
    assert_eq!(w.git.count("/commits/"), 1, "one ref resolution");
}

/// A gateway whose git host always answers 429 with `Retry-After: 120`.
struct Throttle;

#[async_trait]
impl LibraryGateway for Throttle {
    async fn fetch(&self, _r: &FetchRequest) -> Result<FetchOk, GatewayError> {
        Ok(FetchOk {
            status: 429,
            link: None,
            retry_after: Some("120".into()),
            body: Vec::new(),
        })
    }
}

/// FIX-096: a git-host 429 is not a failure of the library: no failure counted, no `error`
/// status, and the next attempt comes after the host's `Retry-After`.
#[tokio::test]
async fn git_host_throttling_is_not_a_failure_and_honours_retry_after() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    add_org_id(&w, lib, "alpha", &sha).await;
    let throttled = w.h.state.clone().with_library_gateway(Arc::new(Throttle));
    for _ in 0..3 {
        sqlx::query("UPDATE skill_libraries SET next_refresh_at = now() - interval '1 second', refresh_lease_until = NULL, lease_owner = NULL WHERE id = $1")
            .bind(lib)
            .execute(w.h.pool())
            .await
            .unwrap();
        let owner = Uuid::now_v7();
        let mut c = w.h.pool().acquire().await.unwrap();
        let mut mine = repo::claim_due(&mut c, owner, 4, 300.0).await.unwrap();
        assert_eq!(mine.len(), 1);
        refresh::refresh_library(&throttled, mine.remove(0), owner).await;
    }
    let (status, failures, ahead): (String, i32, f64) = sqlx::query_as(
        "SELECT status, refresh_failures, extract(epoch FROM next_refresh_at - now())::float8
         FROM skill_libraries WHERE id = $1",
    )
    .bind(lib)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!((status.as_str(), failures), ("active", 0));
    assert!((100.0..=130.0).contains(&ahead), "next attempt in {ahead}s");
}

#[tokio::test]
async fn refresh_reads_the_host_with_the_host_credential_only_through_the_stand_in() {
    // the API never sees a token: the request carries no credential of its own and the
    // stand-in gateway adds the fixed one
    let w = world().await;
    w.seed();
    w.git.expect_authorization("Bearer tok");
    let lib = w.library().await;
    let (s, v) = w
        .call(
            &w.admin,
            Method::GET,
            &format!("/v1/skill-libraries/{lib}/skills"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    for r in w.git.requests() {
        assert_eq!(r.method, "GET");
        assert_eq!(r.authorization.as_deref(), Some("Bearer tok"));
    }
    let _ = w.root;
}
