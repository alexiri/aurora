#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P6-TPL-2 (C-1107, C-1108): agent template sources against a real git server
//! (`aurora_testkit::git_server`), read by the API's own git reader through the guarded
//! client's test override; the packages are served by the same server on plain paths (the
//! override maps every origin to it, so the token-origin rule stays testable). Real Postgres.

use aurora_api::domains::agent_templates::poll;
use aurora_api::domains::auth::mail::{Mail, MailError, Mailer};
use aurora_template::TreeFile;
use aurora_testkit::git_server::{Change, GitServer};
use base64::Engine as _;
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::*;

/// The https origin every source URL uses; the test override maps it to the git server.
const GIT_ORIGIN: &str = "https://git.example";
/// A second logical origin (the override still dials the git server): packages that do not
/// belong to the repository's origin.
const CDN_ORIGIN: &str = "https://cdn.example";
/// The `packages.url` of the test manifest.
const PKG_PATH: &str = "pkg/{id}-{version}.zip";

struct W {
    h: Harness,
    git: GitServer,
    root: String,
}

async fn world() -> W {
    world_with(|_| {}).await
}

/// [`world`] with an instance-config mutation (the kill switch, the library budgets).
async fn world_with(mutate: impl FnOnce(&mut aurora_config::ApiConfig)) -> W {
    let mut h = Harness::with_config(mutate).await;
    let git = GitServer::start().await;
    // Every origin is mapped to the git server (the reader still decides by the logical URL
    // which origin gets the token and which addresses are allowed).
    let state = h
        .state
        .clone()
        .with_connect_override(&git.base())
        .with_library_fetch_timeout(std::time::Duration::from_secs(5));
    h.app = test_router(state.clone());
    h.state = state;
    let root = h.user("root@x.io", true).await;
    let root_cookie = cookie(&h.good_session(root).await.1);
    W {
        h,
        git,
        root: root_cookie,
    }
}

// ---------------------------------------------------------------- template fixtures

/// A minimal valid `template.yaml` (id = the folder name).
fn template_yaml(id: &str, version: &str) -> String {
    format!(
        "format: 1\nid: {id}\nversion: {version}\nname: T\nsummary: A test template.\n\
         inputs:\n  - id: zone\n    label: Zone\n    pattern: \"^[a-z]+$\"\n"
    )
}

const AGENT_YAML: &str =
    "format: 1\nname: T\npurpose: monitor\nmodel: default\nmay_message_first: false\n";
const INSTRUCTIONS: &str = "Watch {input.zone}.\n";

fn manifest_yaml() -> String {
    format!(
        "api: 1\nrepo:\n  name: Acme templates\n  description: Test repo\n\
         packages:\n  url: {GIT_ORIGIN}/{PKG_PATH}\n"
    )
}

/// The files of one template directory, as they are committed to git and packed into the
/// package (the same bytes and modes, so the hashes match).
fn files(dir: &str, version: &str) -> Vec<(String, Vec<u8>)> {
    vec![
        (
            format!("{dir}/template.yaml"),
            template_yaml(dir, version).into_bytes(),
        ),
        (format!("{dir}/agent.yaml"), AGENT_YAML.as_bytes().to_vec()),
        (
            format!("{dir}/instructions.md"),
            INSTRUCTIONS.as_bytes().to_vec(),
        ),
        (
            format!("{dir}/README.md"),
            format!("# {dir}\nA test template.\n").into_bytes(),
        ),
    ]
}

fn tree(dir: &str, version: &str) -> Vec<TreeFile> {
    files(dir, version)
        .into_iter()
        .map(|(path, bytes)| TreeFile {
            path,
            bytes,
            mode: 0o100644,
        })
        .collect()
}

/// The package zip of `dir` at `version`, exactly as the tree at that commit reads.
fn pack(dir: &str, version: &str) -> Vec<u8> {
    aurora_template::pack(dir, &tree(dir, version))
}

/// The package zip of the rich template's `dir` at `version`, exactly as the tree at that
/// commit reads.
fn rich_pack(dir: &str, version: &str) -> Vec<u8> {
    let tree: Vec<TreeFile> = rich_files(dir, version)
        .into_iter()
        .map(|(path, bytes)| TreeFile {
            path,
            bytes,
            mode: 0o100644,
        })
        .collect();
    aurora_template::pack(dir, &tree)
}

fn put(path: String, content: Vec<u8>) -> Change {
    Change::Put(path, content)
}

/// A template that needs an app and references a library skill and an organization skill
/// (C-1110's resolution).
fn rich_yaml(id: &str, version: &str) -> String {
    format!(
        "format: 1\nid: {id}\nversion: {version}\nname: T2\nsummary: Apps and skills.\n\
         inputs:\n  - id: zone\n    label: Zone\n    pattern: \"^[a-z]+$\"\n\
         apps:\n  - app: com-example-thing\n    required: true\n    level: act\n    why: Needs the thing.\n\
         skills:\n  - library: {GIT_ORIGIN}/acme/skills\n    skill: alpha\n    why: Does the work.\n  \
         - organization: notes\n    why: Org knowledge.\n"
    )
}

/// The files of the rich template (same byte-for-byte rule as [`files`]).
fn rich_files(dir: &str, version: &str) -> Vec<(String, Vec<u8>)> {
    vec![
        (
            format!("{dir}/template.yaml"),
            rich_yaml(dir, version).into_bytes(),
        ),
        (format!("{dir}/agent.yaml"), AGENT_YAML.as_bytes().to_vec()),
        (
            format!("{dir}/instructions.md"),
            INSTRUCTIONS.as_bytes().to_vec(),
        ),
        (
            format!("{dir}/README.md"),
            format!("# {dir}\nRich.\n").into_bytes(),
        ),
    ]
}

impl W {
    async fn call(&self, m: Method, uri: &str, body: Option<Value>) -> (u16, Value) {
        self.h.call(&self.root, m, uri, body).await
    }

    /// A repository with the manifest and one template `dir` at `version`; returns the head.
    fn seed(&self, repo: &str, dir: &str, version: &str) -> String {
        self.git.add_repo(repo);
        let mut changes = vec![put(
            "aurora-templates.yaml".into(),
            manifest_yaml().into_bytes(),
        )];
        for (p, b) in files(dir, version) {
            changes.push(put(p, b));
        }
        self.git.commit(repo, "main", &changes)
    }

    /// Move the head of `repo` to a new commit (the manifest stays).
    fn bump(&self, repo: &str, changes: &[Change]) -> String {
        self.git.commit(repo, "main", changes)
    }

    async fn add_source(&self, repo: &str, body: Value) -> Value {
        let (s, v) = self
            .call(
                Method::POST,
                "/v1/admin/agent-template-sources",
                Some(json!({"url": format!("{GIT_ORIGIN}/{repo}")}).merge(body)),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        v
    }

    async fn check(&self, id: Uuid) -> Value {
        let (s, v) = self
            .call(
                Method::POST,
                &format!("/v1/admin/agent-template-sources/{id}/check"),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }

    async fn source(&self, id: Uuid) -> Value {
        let (s, v) = self
            .call(
                Method::GET,
                &format!("/v1/admin/agent-template-sources/{id}"),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
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

    /// The version rows of the template `t`, newest last.
    async fn versions(&self) -> Vec<(String, String, Option<String>)> {
        sqlx::query_as(
            "SELECT v.version, v.status, v.status_reason FROM agent_template_versions v
             JOIN agent_templates t ON t.id = v.agent_template_id
             WHERE t.template_id = 't' ORDER BY v.created_at, v.id",
        )
        .fetch_all(self.h.pool())
        .await
        .unwrap()
    }

    /// The stored content hash of a version row.
    async fn content_sha(&self, version: &str) -> Option<String> {
        sqlx::query_scalar(
            "SELECT v.content_sha256 FROM agent_template_versions v
             JOIN agent_templates t ON t.id = v.agent_template_id
             WHERE t.template_id = 't' AND v.version = $1",
        )
        .bind(version)
        .fetch_one(self.h.pool())
        .await
        .unwrap()
    }

    /// Approve the current `pending_approval` version (as the reviewing admin).
    async fn approve(&self, template: &str, version: &str) {
        let (s, v) = self
            .call(
                Method::GET,
                &format!("/v1/admin/agent-templates/{template}/versions/{version}"),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        let sha = v["content_sha256"]
            .as_str()
            .unwrap_or_else(|| panic!("no content_sha256 for {template}@{version}: {v}"))
            .to_owned();
        let (s, v) = self
            .call(
                Method::POST,
                &format!("/v1/admin/agent-templates/{template}/versions/{version}/approve"),
                Some(json!({"content_sha256": sha})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
    }

    /// An organization with an owner and a workspace the owner can use; returns the ids and
    /// the owner's cookie.
    async fn ws_owner(&self) -> (Uuid, Uuid, String) {
        let org = self.h.org().await;
        let owner = self.h.user("owner@x.io", false).await;
        self.h.member(org, owner, "owner", "active").await;
        let ws = self.h.workspace(org).await;
        self.h.bind_ws(ws, owner, "owner").await;
        let cookie = cookie(&self.h.good_session(owner).await.1);
        (org, ws, cookie)
    }
}

trait Merge {
    fn merge(self, other: Value) -> Value;
}

impl Merge for Value {
    fn merge(mut self, other: Value) -> Value {
        if let (Value::Object(a), Value::Object(b)) = (&mut self, other) {
            for (k, v) in b {
                a.insert(k, v);
            }
        }
        self
    }
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

// ---- the source lifecycle (C-1107) -------------------------------------------------------------

#[tokio::test]
async fn non_admins_are_refused_and_the_lifecycle_is_audited() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    let member = w.h.user("m@x.io", false).await;
    let member = cookie(&w.h.good_session(member).await.1);
    for m in [Method::GET, Method::POST] {
        let (s, v) =
            w.h.call(&member, m, "/v1/admin/agent-template-sources", None)
                .await;
        assert_eq!((s, code(&v)), (403, "instance_admin_required"), "{v}");
    }
    let (s, v) = w
        .call(
            Method::POST,
            "/v1/admin/agent-template-sources",
            Some(json!({"url": format!("{GIT_ORIGIN}/acme/tpl")})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(v["name"], "Acme templates");
    assert_eq!(v["repo"], "git.example/acme/tpl");
    assert_eq!(v["packages_url"], format!("{GIT_ORIGIN}/{PKG_PATH}"));
    assert_eq!(v["token_set"], false);
    assert_eq!(v["status"], "ok");
    assert_eq!(v["update_interval"], "1d");
    assert!(v["last_head_sha"].is_null(), "{v}");
    assert!(
        w.audit_critical("agent_template_source.add").await == 1,
        "{v}"
    );

    // The same url + ref + path again: 409, also with a trailing slash.
    for body in [
        json!({"url": format!("{GIT_ORIGIN}/acme/tpl")}),
        json!({"url": format!("{GIT_ORIGIN}/acme/tpl/")}),
    ] {
        let (s, v) = w
            .call(Method::POST, "/v1/admin/agent-template-sources", Some(body))
            .await;
        assert_eq!((s, code(&v)), (409, "template_source_exists"), "{v}");
    }

    // A bad schedule is refused; a good one moves the next check.
    let one = format!("/v1/admin/agent-template-sources/{id}");
    let (s, v) = w
        .call(Method::PATCH, &one, Some(json!({"update_interval": "2d"})))
        .await;
    assert_eq!((s, code(&v)), (400, "invalid_request"), "{v}");
    let (s, v) = w
        .call(Method::PATCH, &one, Some(json!({"update_interval": "1w"})))
        .await;
    assert_eq!((s, v["update_interval"].as_str()), (200, Some("1w")), "{v}");
    assert_eq!(w.audit_critical("agent_template_source.update").await, 1);

    // Removing: 204, the rows go with it, audited with the template count.
    let (s, v) = w.call(Method::DELETE, &one, None).await;
    assert_eq!(s, 204, "{v}");
    let (s, v) = w.call(Method::GET, &one, None).await;
    assert_eq!(s, 404, "{v}");
    assert_eq!(w.audit_critical("agent_template_source.remove").await, 1);
}

#[tokio::test]
async fn preview_answers_the_read_without_storing_anything() {
    let w = world().await;
    let head = w.seed("acme/tpl", "t", "1.0.0");
    let (s, v) = w
        .call(
            Method::POST,
            "/v1/admin/agent-template-sources/preview",
            Some(json!({"url": format!("{GIT_ORIGIN}/acme/tpl")})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["url"], format!("{GIT_ORIGIN}/acme/tpl"));
    assert_eq!(v["repo"], "git.example/acme/tpl");
    assert_eq!(v["commit_sha"], head);
    assert_eq!(
        v["resolved"],
        json!({"kind": "branch", "name": "main", "default": true})
    );
    assert_eq!(v["manifest"]["name"], "Acme templates");
    // The manifest's `packages.url` pattern rides along as text (C-1114's step 2 shows it).
    assert_eq!(
        v["manifest"]["packages_url"],
        format!("{GIT_ORIGIN}/{PKG_PATH}")
    );
    assert_eq!(v["truncated"], false);
    assert_eq!(
        v["templates"][0],
        json!({
            "template_id": "t", "dir": "t", "name": "T", "summary": "A test template.",
            "version": "1.0.0", "state": "new", "problems": []
        }),
        "{v}"
    );
    // Nothing was stored and no audit row was written.
    assert_eq!(
        w.count("SELECT count(*) FROM agent_template_sources").await,
        0
    );
    assert_eq!(w.count("SELECT count(*) FROM audit_log").await, 0);
    // A bad manifest is 422 with the fixed code.
    w.bump(
        "acme/tpl",
        &[put("aurora-templates.yaml".into(), b"api: 2\n".to_vec())],
    );
    let (s, v) = w
        .call(
            Method::POST,
            "/v1/admin/agent-template-sources/preview",
            Some(json!({"url": format!("{GIT_ORIGIN}/acme/tpl")})),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "template_manifest_invalid"), "{v}");
}

/// (C-1107) Every preview refusal carries its contract code: the address parser's
/// `template_url_invalid` (with `details.reason`), `template_unreachable` for a repository
/// that cannot be read, 409 `template_source_exists` for a stored address, and the
/// 10-a-minute per-person allowance as 429 `rate_limited`.
#[tokio::test]
async fn the_preview_refusals_carry_their_codes() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    let preview = |body: Value| {
        w.call(
            Method::POST,
            "/v1/admin/agent-template-sources/preview",
            Some(body),
        )
    };

    // Another scheme than https: 422 with the parser's reason.
    let (s, v) = preview(json!({"url": "http://git.example/acme/tpl"})).await;
    assert_eq!((s, code(&v)), (422, "template_url_invalid"), "{v}");
    assert!(
        v["error"]["details"]["reason"]
            .as_str()
            .is_some_and(|r| !r.is_empty()),
        "{v}"
    );

    // A repository that is not there, and a branch that does not exist: 422, the reason in
    // words, nothing stored.
    for body in [
        json!({"url": format!("{GIT_ORIGIN}/acme/missing")}),
        json!({"url": format!("{GIT_ORIGIN}/acme/tpl"), "ref": "nope"}),
    ] {
        let (s, v) = preview(body).await;
        assert_eq!((s, code(&v)), (422, "template_unreachable"), "{v}");
        assert!(
            v["error"]["details"]["reason"]
                .as_str()
                .is_some_and(|r| !r.is_empty()),
            "{v}"
        );
    }
    assert_eq!(
        w.count("SELECT count(*) FROM agent_template_sources").await,
        0
    );

    // 10 a minute per person (a token bucket: a slow machine may earn one back); the two
    // unreachable reads above drew on the same allowance.
    let mut limited = false;
    let mut attempted = 2;
    for _ in 0..12 {
        let (s, v) = preview(json!({"url": format!("{GIT_ORIGIN}/acme/missing")})).await;
        if s == 429 {
            assert_eq!(code(&v), "rate_limited", "{v}");
            limited = true;
            break;
        }
        assert_eq!((s, code(&v)), (422, "template_unreachable"), "{v}");
        attempted += 1;
    }
    assert!(
        limited,
        "a preview beyond the allowance is refused with 429"
    );
    assert!(
        (10..=11).contains(&attempted),
        "{attempted} previews went through"
    );

    // The other administrator still has their own allowance: they add the source, and a
    // preview of a stored address is 409 `template_source_exists` (this check runs before
    // the read and the allowance), while a free address is read normally.
    let other = w.h.user("other-root@x.io", true).await;
    let other = cookie(&w.h.good_session(other).await.1);
    let (s, v) =
        w.h.call(
            &other,
            Method::POST,
            "/v1/admin/agent-template-sources",
            Some(json!({"url": format!("{GIT_ORIGIN}/acme/tpl")})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = preview(json!({"url": format!("{GIT_ORIGIN}/acme/tpl")})).await;
    assert_eq!((s, code(&v)), (409, "template_source_exists"), "{v}");
    let (s, v) =
        w.h.call(
            &other,
            Method::POST,
            "/v1/admin/agent-template-sources/preview",
            Some(json!({"url": format!("{GIT_ORIGIN}/acme/missing")})),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "template_unreachable"), "{v}");
}

/// (C-1107) At most `TEMPLATE_SOURCES_MAX` (20) sources: the 20th add works, the 21st is
/// 422 `template_source_limit` before any repository read.
#[tokio::test]
async fn the_twentieth_source_is_the_last() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    for i in 0..19 {
        sqlx::query(
            "INSERT INTO agent_template_sources (id, name, url, packages_url, manifest)
             VALUES ($1, 'Filler', $2, 'https://git.example/pkg/{id}-{version}.zip', '{}')",
        )
        .bind(Uuid::now_v7())
        .bind(format!("{GIT_ORIGIN}/acme/filler{i}"))
        .execute(w.h.pool())
        .await
        .unwrap();
    }
    // The 20th is a real add.
    w.add_source("acme/tpl", json!({})).await;
    // A 21st is refused before the read (the repository does not exist, so a read would fail
    // differently).
    let (s, v) = w
        .call(
            Method::POST,
            "/v1/admin/agent-template-sources",
            Some(json!({"url": format!("{GIT_ORIGIN}/acme/onemore")})),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "template_source_limit"), "{v}");
    assert_eq!(
        w.count("SELECT count(*) FROM agent_template_sources").await,
        20
    );
}

// ---- the check path (C-1108) -------------------------------------------------------------------

#[tokio::test]
async fn the_first_check_ingests_the_current_head() {
    let w = world().await;
    let head = w.seed("acme/tpl", "t", "1.0.0");
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();

    // The first check runs the full pass (nothing is stored yet): the head's version is
    // taken, the package downloads and the row waits for approval.
    let out = w.check(id).await;
    assert_eq!(out["status"], "updated", "{out}");
    assert_eq!(out["commit_sha"], head);
    assert_eq!(
        out["ingested"],
        json!([{"template_id": "t", "version": "1.0.0", "status": "pending_approval",
                "status_reason": null}]),
        "{out}"
    );
    assert_eq!(out["problems"], json!([]), "{out}");
    assert_eq!(
        w.versions().await,
        vec![("1.0.0".into(), "pending_approval".into(), None)]
    );
    let row = w.source(id).await;
    assert_eq!(row["last_head_sha"], head, "{row}");
    assert_eq!(row["templates"][0]["state"], "ingested", "{row}");
    assert_eq!(row["templates"][0]["head_version"], "1.0.0", "{row}");
    // The stored content: hashes and size, the parsed content, one critical ingest row.
    assert!(w.content_sha("1.0.0").await.is_some());
    let (sha, arch, size, content): (Option<String>, Option<String>, Option<i32>, Option<Value>) =
        sqlx::query_as(
            "SELECT content_sha256, archive_sha256, size_bytes, content
             FROM agent_template_versions v JOIN agent_templates t ON t.id = v.agent_template_id
             WHERE t.template_id = 't' AND v.version = '1.0.0'",
        )
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert!(sha.is_some() && arch.is_some() && size.unwrap() > 0 && content.is_some());
    assert_eq!(w.audit_critical("agent_template.ingest").await, 1);

    // The same head again: up_to_date, nothing else is read (no new versions, no events).
    let out = w.check(id).await;
    assert_eq!(out["status"], "up_to_date", "{out}");
    assert_eq!(out["ingested"], json!([]), "{out}");
    assert_eq!(w.versions().await.len(), 1);
    assert_eq!(w.audit_critical("agent_template.ingest").await, 1);
}

#[tokio::test]
async fn an_unreadable_stored_content_cannot_be_reviewed_or_approved() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(w.check(id).await["status"], "updated");

    // Corrupt the stored content column: one helper file whose bytes are not base64 (a shape
    // the encoder never writes — the point of this test is that the API refuses it).
    sqlx::query(
        "UPDATE agent_template_versions SET content = jsonb_set(content, '{helpers}',
            '[{\"path\":\"h/run.sh\",\"size\":3,\"executable\":true,\"sha256\":\"aa\",\"bytes\":\"!!!not base64!!!\"}]')
         WHERE agent_template_id = (SELECT id FROM agent_templates WHERE template_id = 't')
            AND version = '1.0.0'",
    )
    .execute(w.h.pool())
    .await
    .unwrap();

    // The detail cannot render the content: it shows null, not a file with empty bytes.
    let (s, v) = w
        .call(
            Method::GET,
            "/v1/admin/agent-templates/t/versions/1.0.0",
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(v["content"].is_null(), "{v}");

    // Approve is refused even though the quoted hash matches the stored one: the admin has
    // not reviewed (cannot have reviewed) what is stored.
    let sha = w.content_sha("1.0.0").await.unwrap();
    let (s, v) = w
        .call(
            Method::POST,
            "/v1/admin/agent-templates/t/versions/1.0.0/approve",
            Some(json!({"content_sha256": sha})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "template_content_changed"), "{v}");
    assert_eq!(
        w.versions().await,
        vec![("1.0.0".into(), "pending_approval".into(), None)]
    );
    assert_eq!(w.audit_critical("agent_template.approve").await, 0);
}

/// (SEC-146) A tree at the recorded commit with more files than a template may carry is
/// refused after at most `TEMPLATE_FILES_MAX + 1` blobs were fetched: the version is
/// `invalid` `template_too_large`, and a blob sorted past the stop point is never fetched
/// (the old code fetched every blob and only the parse then refused the tree).
#[tokio::test]
async fn a_tree_with_more_files_than_the_cap_stops_reading_at_the_cap() {
    let w = world().await;
    w.git.add_repo("acme/tpl");
    let mut changes = vec![put(
        "aurora-templates.yaml".into(),
        manifest_yaml().into_bytes(),
    )];
    for (p, b) in files("t", "1.0.0") {
        changes.push(put(p, b));
    }
    // 198 distinct fillers: the tree holds TEMPLATE_FILES_MAX + 2 files in total, the
    // package (the four plain files only) stays well under every cap.
    let fillers = aurora_core::limits::TEMPLATE_FILES_MAX + 2 - files("t", "1.0.0").len();
    for i in 0..fillers {
        changes.push(put(
            format!("t/f{i:05}.txt"),
            format!("filler {i}\n").into_bytes(),
        ));
    }
    // Sorted last: its blob lies past the stop point.
    changes.push(put("t/zzsentinel.txt".into(), b"never read\n".to_vec()));
    w.git.commit("acme/tpl", "main", &changes);
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();

    let out = w.check(id).await;
    assert_eq!(out["status"], "updated", "{out}");
    assert_eq!(
        w.versions().await,
        vec![(
            "1.0.0".into(),
            "invalid".into(),
            Some("template_too_large".into())
        )]
    );

    // The early stop: no request ever wanted the sentinel's blob.
    let oid = w.git.blob_id("acme/tpl", "main", "t/zzsentinel.txt");
    assert!(
        !w.git
            .requests()
            .iter()
            .any(|r| r.body.windows(oid.len()).any(|w| w == oid.as_bytes())),
        "the blob past the stop point was fetched"
    );
}

#[tokio::test]
async fn a_missing_package_waits_and_is_retried_on_the_package_retry() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();

    // No package yet: the row stays awaiting_package with its reason, the source is due
    // again after TEMPLATE_PACKAGE_RETRY (15 min), not at the full interval.
    let out = w.check(id).await;
    assert_eq!(out["status"], "up_to_date", "{out}");
    assert_eq!(out["waiting"][0]["template_id"], "t", "{out}");
    assert_eq!(out["waiting"][0]["version"], "1.0.0", "{out}");
    assert_eq!(
        out["waiting"][0]["last_reason"], "package_not_found",
        "{out}"
    );
    assert!(out["waiting"][0]["since"].is_string(), "{out}");
    assert_eq!(
        w.versions().await,
        vec![(
            "1.0.0".into(),
            "awaiting_package".into(),
            Some("package_not_found".into())
        )]
    );
    let (next, attempted): (
        chrono::DateTime<chrono::Utc>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as(
        "SELECT next_check_at, last_checked_at FROM agent_template_sources WHERE id = $1",
    )
    .bind(id)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(attempted.is_some());
    let secs = (next - chrono::Utc::now()).num_seconds();
    assert!((880..=920).contains(&secs), "next in {secs}s");

    // The package appears: the retry ingests it.
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let out = w.check(id).await;
    assert_eq!(
        out["ingested"],
        json!([{"template_id": "t", "version": "1.0.0", "status": "pending_approval",
                "status_reason": null}]),
        "{out}"
    );
    assert_eq!(out["waiting"], json!([]), "{out}");
}

#[tokio::test]
async fn an_expired_wait_turns_invalid_with_its_last_reason() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    let out = w.check(id).await;
    assert_eq!(out["waiting"].as_array().unwrap().len(), 1, "{out}");

    // Past TEMPLATE_PACKAGE_WAIT (24 h) from created_at: invalid with the reason it last
    // failed with, and never tried again.
    sqlx::query("UPDATE agent_template_versions SET created_at = now() - interval '25 hours'")
        .execute(w.h.pool())
        .await
        .unwrap();
    let out = w.check(id).await;
    assert_eq!(
        out["ingested"],
        json!([{"template_id": "t", "version": "1.0.0", "status": "invalid",
                "status_reason": "package_not_found"}]),
        "{out}"
    );
    assert_eq!(
        w.versions().await,
        vec![(
            "1.0.0".into(),
            "invalid".into(),
            Some("package_not_found".into())
        )]
    );
    assert_eq!(w.audit_critical("agent_template.ingest").await, 1);

    // The package appears anyway: the head is unchanged, nothing is retried.
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let out = w.check(id).await;
    assert_eq!(out["status"], "up_to_date", "{out}");
    assert_eq!(w.versions().await.len(), 1);
    assert_eq!(w.audit_critical("agent_template.ingest").await, 1);
}

/// One template whose version row cannot be recorded (a row-level database failure, the way
/// the SEC-146 D fixture refuses a commit) must not block the source's other templates: the
/// healthy ones are still ingested in the same check, the broken one is listed as a problem
/// and offered again on the next check.
#[tokio::test]
async fn one_broken_template_does_not_block_the_others() {
    let w = world().await;
    // Three template directories in one commit.
    w.git.add_repo("acme/tpl");
    let mut changes = vec![put(
        "aurora-templates.yaml".into(),
        manifest_yaml().into_bytes(),
    )];
    for dir in ["a", "b", "broken"] {
        for (p, b) in files(dir, "1.0.0") {
            changes.push(put(p, b));
        }
    }
    w.git.commit("acme/tpl", "main", &changes);
    for dir in ["a", "b", "broken"] {
        w.git
            .serve_file(&format!("pkg/{dir}-1.0.0.zip"), pack(dir, "1.0.0"));
    }
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(w.check(id).await["status"], "updated");
    assert_eq!(
        w.count("SELECT count(*) FROM agent_template_versions")
            .await,
        3
    );

    // The database refuses every new version row of `broken`.
    sqlx::query(
        "CREATE OR REPLACE FUNCTION aurora_tpl_fail_version() RETURNS trigger AS $$
         BEGIN IF (SELECT template_id FROM agent_templates WHERE id = NEW.agent_template_id) = 'broken'
         THEN RAISE EXCEPTION 'test: refused version write'; END IF; RETURN NEW; END;
         $$ LANGUAGE plpgsql",
    )
    .execute(w.h.pool())
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER aurora_tpl_ver_trg BEFORE INSERT ON agent_template_versions
         FOR EACH ROW EXECUTE FUNCTION aurora_tpl_fail_version()",
    )
    .execute(w.h.pool())
    .await
    .unwrap();

    // All three templates bump to 2.0.0 in one commit.
    let mut bump = Vec::new();
    for dir in ["a", "b", "broken"] {
        bump.push(put(
            format!("{dir}/template.yaml"),
            template_yaml(dir, "2.0.0").into_bytes(),
        ));
        w.git
            .serve_file(&format!("pkg/{dir}-2.0.0.zip"), pack(dir, "2.0.0"));
    }
    w.bump("acme/tpl", &bump);
    let out = w.check(id).await;
    assert_eq!(out["status"], "updated", "{out}");
    let ingested = out["ingested"].as_array().unwrap();
    assert_eq!(ingested.len(), 2, "{out}");
    assert!(
        ingested.iter().all(|i| i["template_id"] != json!("broken")),
        "{out}"
    );
    assert_eq!(out["problems"][0]["template_id"], "broken", "{out}");
    let new_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM agent_template_versions WHERE version = '2.0.0'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(new_rows, 2, "the healthy two are recorded, broken is not");
}

/// A version whose last attempt failed transiently gets ONE more retry window before it
/// expires (the comparison uses the database's clock): a network failure in the very last
/// attempt must not retire a version for good when an equal version is ignored afterwards
/// and only a version bump would recover it.
#[tokio::test]
async fn a_transient_last_attempt_gets_one_more_retry_window() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    // The package host answers 500 — the classic transient download failure.
    w.git.override_status("pkg/t-1.0.0.zip", 500, None);
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    let out = w.check(id).await;
    assert_eq!(out["waiting"][0]["last_reason"], "download_failed", "{out}");

    // Past the 24 h window: the transient reason earns one more window (the version keeps
    // waiting and is retried), where a non-transient reason is already invalid.
    sqlx::query("UPDATE agent_template_versions SET created_at = now() - interval '25 hours'")
        .execute(w.h.pool())
        .await
        .unwrap();
    let out = w.check(id).await;
    assert_eq!(out["status"], "up_to_date", "{out}");
    assert_eq!(
        w.versions().await,
        vec![(
            "1.0.0".into(),
            "awaiting_package".into(),
            Some("download_failed".into())
        )]
    );

    // Past the extra window too: invalid, with the transient reason it last failed with.
    sqlx::query("UPDATE agent_template_versions SET created_at = now() - interval '49 hours'")
        .execute(w.h.pool())
        .await
        .unwrap();
    let out = w.check(id).await;
    assert_eq!(out["ingested"][0]["status"], "invalid", "{out}");
    assert_eq!(
        out["ingested"][0]["status_reason"], "download_failed",
        "{out}"
    );
}

#[tokio::test]
async fn bad_packages_turn_invalid_with_the_right_reason() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();

    // A package that does not match the repository at the recorded commit: source_mismatch.
    let mut wrong = files("t", "1.0.0");
    wrong[2] = ("t/instructions.md".into(), b"Changed content.\n".to_vec());
    let zip: Vec<u8> = aurora_template::pack(
        "t",
        &wrong
            .into_iter()
            .map(|(path, bytes)| TreeFile {
                path,
                bytes,
                mode: 0o100644,
            })
            .collect::<Vec<_>>(),
    );
    w.git.serve_file("pkg/t-1.0.0.zip", zip);
    let out = w.check(id).await;
    assert_eq!(
        out["ingested"],
        json!([{"template_id": "t", "version": "1.0.0", "status": "invalid",
                "status_reason": "source_mismatch"}]),
        "{out}"
    );

    // A package whose template.yaml does not match the row: identity_mismatch.
    w.bump(
        "acme/tpl",
        &[put(
            "t/template.yaml".into(),
            template_yaml("t", "1.1.0").into_bytes(),
        )],
    );
    let head2 = w.git.head("acme/tpl", "main");
    let mut lying = tree("t", "1.1.0");
    lying[0] = TreeFile {
        path: "t/template.yaml".into(),
        bytes: template_yaml("t", "9.9.9").into_bytes(),
        mode: 0o100644,
    };
    w.git
        .serve_file("pkg/t-1.1.0.zip", aurora_template::pack("t", &lying));
    let out = w.check(id).await;
    assert_eq!(
        out["ingested"],
        json!([{"template_id": "t", "version": "1.1.0", "status": "invalid",
                "status_reason": "identity_mismatch"}]),
        "{out}"
    );
    assert_eq!(out["commit_sha"], head2, "{out}");

    // A package over TEMPLATE_ARCHIVE_MAX (5 MiB) is too_large at once.
    w.bump(
        "acme/tpl",
        &[put(
            "t/template.yaml".into(),
            template_yaml("t", "1.2.0").into_bytes(),
        )],
    );
    w.git
        .serve_file("pkg/t-1.2.0.zip", vec![0u8; 5 * 1024 * 1024 + 1]);
    let out = w.check(id).await;
    assert_eq!(
        out["ingested"],
        json!([{"template_id": "t", "version": "1.2.0", "status": "invalid",
                "status_reason": "too_large"}]),
        "{out}"
    );

    // Every one of them left awaiting_package exactly once: three rows, three events.
    assert_eq!(
        w.versions().await,
        vec![
            (
                "1.0.0".into(),
                "invalid".into(),
                Some("source_mismatch".into())
            ),
            (
                "1.1.0".into(),
                "invalid".into(),
                Some("identity_mismatch".into())
            ),
            ("1.2.0".into(), "invalid".into(), Some("too_large".into())),
        ]
    );
    assert_eq!(w.audit_critical("agent_template.ingest").await, 3);

    // A broken package is stored invalid once and never retried: the head is stored, the
    // next check is up_to_date and touches nothing.
    let out = w.check(id).await;
    assert_eq!(out["status"], "up_to_date", "{out}");
    assert_eq!(out["ingested"], json!([]), "{out}");
    assert_eq!(w.versions().await.len(), 3);
    assert_eq!(w.audit_critical("agent_template.ingest").await, 3);
}

#[tokio::test]
async fn a_broken_head_marks_the_template_invalid_and_versions_survive() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    let out = w.check(id).await;
    assert_eq!(out["ingested"].as_array().unwrap().len(), 1, "{out}");

    // The head's template.yaml no longer parses: the problem is listed, the stored template
    // becomes invalid and keeps its versions, nothing new is stored.
    w.bump(
        "acme/tpl",
        &[put(
            "t/template.yaml".into(),
            b"format: 1\nid: t\n".to_vec(),
        )],
    );
    let out = w.check(id).await;
    assert_eq!(out["status"], "up_to_date", "{out}");
    assert_eq!(out["ingested"], json!([]), "{out}");
    assert_eq!(out["problems"].as_array().unwrap().len(), 1, "{out}");
    assert_eq!(out["problems"][0]["code"], "template_yaml", "{out}");
    let row = w.source(id).await;
    assert_eq!(row["templates"][0]["head_state"], "invalid", "{row}");
    assert!(row["templates"][0]["state"].is_string());
    assert_eq!(w.versions().await.len(), 1);
    let problems: Option<Value> =
        sqlx::query_scalar("SELECT head_problems FROM agent_templates WHERE template_id = 't'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(problems.unwrap()[0]["code"], "template_yaml");

    // The head is healthy again: the state is cleared, the version stays.
    w.bump(
        "acme/tpl",
        &[put(
            "t/template.yaml".into(),
            template_yaml("t", "1.0.0").into_bytes(),
        )],
    );
    let out = w.check(id).await;
    assert_eq!(out["problems"], json!([]), "{out}");
    let row = w.source(id).await;
    assert_eq!(row["templates"][0]["head_state"], "ok", "{row}");
    assert_eq!(w.versions().await.len(), 1);
}

#[tokio::test]
async fn a_head_that_is_not_newer_is_not_recorded() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(w.check(id).await["status"], "updated");

    // A lower head version: the problem version_not_newer, no row.
    w.bump(
        "acme/tpl",
        &[put(
            "t/template.yaml".into(),
            template_yaml("t", "0.9.0").into_bytes(),
        )],
    );
    let out = w.check(id).await;
    assert_eq!(out["status"], "up_to_date", "{out}");
    assert_eq!(out["problems"][0]["code"], "version_not_newer", "{out}");
    assert_eq!(out["problems"][0]["template_id"], "t", "{out}");
    assert_eq!(w.versions().await.len(), 1);

    // The same version again (a new commit): ignored, no new row either.
    w.bump(
        "acme/tpl",
        &[put(
            "t/template.yaml".into(),
            template_yaml("t", "1.0.0").into_bytes(),
        )],
    );
    let out = w.check(id).await;
    assert_eq!(out["problems"], json!([]), "{out}");
    assert_eq!(w.versions().await.len(), 1);
}

/// A new pre-release version is a new version (real semver precedence): `1.0.0-rc.2` is
/// ingested after `1.0.0-rc.1`, not silently ignored as an equal version.
#[tokio::test]
async fn a_new_prerelease_version_is_ingested() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0-rc.1");
    w.git
        .serve_file("pkg/t-1.0.0-rc.1.zip", pack("t", "1.0.0-rc.1"));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(w.check(id).await["status"], "updated");
    assert_eq!(w.versions().await.len(), 1);

    // The head moves to 1.0.0-rc.2: semver-newer than the stored rc.1, so it is offered.
    w.bump(
        "acme/tpl",
        &[put(
            "t/template.yaml".into(),
            template_yaml("t", "1.0.0-rc.2").into_bytes(),
        )],
    );
    w.git
        .serve_file("pkg/t-1.0.0-rc.2.zip", pack("t", "1.0.0-rc.2"));
    let out = w.check(id).await;
    assert_eq!(out["status"], "updated", "{out}");
    assert_eq!(
        w.versions().await,
        vec![
            ("1.0.0-rc.1".into(), "pending_approval".into(), None),
            ("1.0.0-rc.2".into(), "pending_approval".into(), None),
        ]
    );
}

#[tokio::test]
async fn a_removed_directory_comes_back() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(w.check(id).await["status"], "updated");

    // The directory vanished upstream: flagged, the template stays as it is.
    w.bump("acme/tpl", &[Change::DeleteDir("t".into())]);
    let out = w.check(id).await;
    assert_eq!(out["status"], "up_to_date", "{out}");
    let row = w.source(id).await;
    assert_eq!(row["templates"][0]["state"], "ingested", "{row}");
    let removed: bool =
        sqlx::query_scalar("SELECT removed_upstream FROM agent_templates WHERE template_id = 't'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(removed);

    // The directory is back: the flag clears, the version stays.
    w.bump(
        "acme/tpl",
        &files("t", "1.0.0")
            .into_iter()
            .map(|(p, b)| put(p, b))
            .collect::<Vec<_>>(),
    );
    let out = w.check(id).await;
    assert_eq!(out["problems"], json!([]), "{out}");
    let removed: bool =
        sqlx::query_scalar("SELECT removed_upstream FROM agent_templates WHERE template_id = 't'")
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(!removed);
    assert_eq!(w.versions().await.len(), 1);
}

#[tokio::test]
async fn an_id_held_by_another_source_is_id_taken() {
    let w = world().await;
    w.seed("acme/one", "t", "1.0.0");
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let first = w.add_source("acme/one", json!({})).await;
    let first_id: Uuid = first["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(w.check(first_id).await["status"], "updated");

    // A second source holding the same template id: its check lists id_taken and stores
    // nothing (no template row, no version).
    w.seed("acme/two", "t", "1.0.0");
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let second = w.add_source("acme/two", json!({})).await;
    let second_id: Uuid = second["id"].as_str().unwrap().parse().unwrap();
    let out = w.check(second_id).await;
    assert_eq!(out["status"], "up_to_date", "{out}");
    assert_eq!(out["problems"][0]["code"], "id_taken", "{out}");
    let row = w.source(second_id).await;
    assert_eq!(row["templates"], json!([]), "{row}");
    assert_eq!(w.count("SELECT count(*) FROM agent_templates").await, 1);
    assert_eq!(
        w.count("SELECT count(*) FROM agent_template_versions")
            .await,
        1
    );
}

#[tokio::test]
async fn a_min_core_above_the_core_is_needs_newer_aurora() {
    let w = world().await;
    w.git.add_repo("acme/tpl");
    let changes = vec![
        put("aurora-templates.yaml".into(), manifest_yaml().into_bytes()),
        put(
            "t/template.yaml".into(),
            format!("{}min_core: 99.0.0\n", template_yaml("t", "1.0.0")).into_bytes(),
        ),
        put("t/agent.yaml".into(), AGENT_YAML.as_bytes().to_vec()),
        put("t/instructions.md".into(), INSTRUCTIONS.as_bytes().to_vec()),
        put("t/README.md".into(), b"# t\nA test template.\n".to_vec()),
    ];
    w.git.commit("acme/tpl", "main", &changes);
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    let out = w.check(id).await;
    assert_eq!(out["status"], "up_to_date", "{out}");
    let row = w.source(id).await;
    assert_eq!(
        row["templates"][0]["head_state"], "needs_newer_aurora",
        "{row}"
    );
    assert_eq!(
        w.count("SELECT count(*) FROM agent_template_versions")
            .await,
        0
    );
}

// ---- failures, the lease and the poll ----------------------------------------------------------

#[tokio::test]
async fn a_read_failure_is_a_failed_result_and_backs_off() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();

    w.git.override_status("acme/tpl", 500, None);
    let out = w.check(id).await;
    assert_eq!(out["status"], "failed", "{out}");
    let reason = out["check_failed_reason"].as_str().unwrap();
    assert!(!reason.is_empty() && !reason.contains("sekret"), "{out}");
    assert_eq!(out["ingested"], json!([]), "{out}");
    let row = w.source(id).await;
    assert_eq!(row["status"], "check_failed", "{row}");
    assert_eq!(row["check_failed_reason"], reason, "{row}");
    // The failure backs off 1 h and writes no per-check audit row (C-1113).
    let next: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT next_check_at FROM agent_template_sources WHERE id = $1")
            .bind(id)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    let mins = (next - chrono::Utc::now()).num_minutes();
    assert!((55..=65).contains(&mins), "next in {mins}m");
    assert_eq!(w.audit_critical("agent_template.ingest").await, 0);

    // check-all answers one failed item, and a recovery turns the source ok again.
    w.git.override_status("acme/tpl", 500, None);
    let (s, v) = w
        .call(
            Method::POST,
            "/v1/admin/agent-template-sources/check-all",
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"][0]["status"], "failed", "{v}");
    w.git.clear_overrides();
    let out = w.check(id).await;
    assert_eq!(out["status"], "updated", "{out}");
    let row = w.source(id).await;
    assert_eq!(row["status"], "ok", "{row}");
    assert!(row["check_failed_reason"].is_null(), "{row}");
}

#[tokio::test]
async fn a_busy_source_is_refused_409_and_skipped_in_check_all() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    // Someone else holds the lease.
    sqlx::query(
        "UPDATE agent_template_sources SET check_lease_until = now() + interval '5 minutes',
            lease_owner = '00000000-0000-0000-0000-000000000001'",
    )
    .bind(id)
    .execute(w.h.pool())
    .await
    .unwrap();
    let (s, v) = w
        .call(
            Method::POST,
            &format!("/v1/admin/agent-template-sources/{id}/check"),
            None,
        )
        .await;
    assert_eq!((s, code(&v)), (409, "template_check_in_progress"), "{v}");
    let (s, v) = w
        .call(
            Method::POST,
            "/v1/admin/agent-template-sources/check-all",
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"][0]["status"], "failed", "{v}");
    assert_eq!(v["items"][0]["check_failed_reason"], "busy", "{v}");
}

/// A check that loses its lease mid-way (another replica claimed the source — with many
/// templates downloading packages the 5-minute lease can expire) aborts cleanly: it answers
/// the failed outcome, writes nothing of its own (no head, no check state) and leaves the
/// source with the lease of whoever holds it now.
#[tokio::test]
async fn a_check_that_loses_the_lease_aborts_cleanly() {
    let w = world().await;
    // Two fresh templates: the first template row hands the lease to the "thief", the
    // renewal before the second one must fail.
    w.git.add_repo("acme/tpl");
    let mut changes = vec![put(
        "aurora-templates.yaml".into(),
        manifest_yaml().into_bytes(),
    )];
    for dir in ["a", "b"] {
        for (p, b) in files(dir, "1.0.0") {
            changes.push(put(p, b));
        }
    }
    w.git.commit("acme/tpl", "main", &changes);
    for dir in ["a", "b"] {
        w.git
            .serve_file(&format!("pkg/{dir}-1.0.0.zip"), pack(dir, "1.0.0"));
    }
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();

    // The "thief" replica claims the source the moment the check writes its first template
    // row (as if the lease had expired during a long check).
    sqlx::query(
        "CREATE OR REPLACE FUNCTION aurora_tpl_steal_lease() RETURNS trigger AS $$
         BEGIN UPDATE agent_template_sources
               SET lease_owner = '00000000-0000-0000-0000-000000000002'::uuid,
                   check_lease_until = now() + interval '5 minutes'
               WHERE id = NEW.source_id;
         RETURN NEW; END;
         $$ LANGUAGE plpgsql",
    )
    .execute(w.h.pool())
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER aurora_tpl_lease_trg AFTER INSERT ON agent_templates
         FOR EACH ROW EXECUTE FUNCTION aurora_tpl_steal_lease()",
    )
    .execute(w.h.pool())
    .await
    .unwrap();

    let out = w.check(id).await;
    assert_eq!(out["status"], "failed", "{out}");
    assert!(
        out["check_failed_reason"]
            .as_str()
            .unwrap()
            .contains("changed while this check was running"),
        "{out}"
    );

    // The check wrote nothing of its own: no head sha, no finished check — the source
    // serves its last known state and keeps the lease of whoever holds it now.
    let row = w.source(id).await;
    assert!(row["last_head_sha"].is_null(), "{row}");
    let stolen: i64 = w
        .count(&format!(
            "SELECT count(*) FROM agent_template_sources
             WHERE id = '{id}' AND lease_owner = '00000000-0000-0000-0000-000000000002'::uuid
               AND check_lease_until > now()"
        ))
        .await;
    assert_eq!(stolen, 1, "the thief's lease was cleared");
}

#[tokio::test]
async fn the_poll_claims_due_sources_only() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();

    // Not due yet (the default interval): the poll checks nothing.
    assert_eq!(poll::poll_once(&w.h.state).await.unwrap(), 0);
    let checked: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT last_checked_at FROM agent_template_sources WHERE id = $1")
            .bind(id)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(checked.is_none());

    // Due: the poll runs the shared check path and the lease is released.
    sqlx::query("UPDATE agent_template_sources SET next_check_at = now() - interval '1 second'")
        .bind(id)
        .execute(w.h.pool())
        .await
        .unwrap();
    assert_eq!(poll::poll_once(&w.h.state).await.unwrap(), 1);
    assert_eq!(poll::poll_once(&w.h.state).await.unwrap(), 0);
    assert_eq!(
        w.versions().await,
        vec![("1.0.0".into(), "pending_approval".into(), None)]
    );
    let (checked, lease, owner): (
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<Uuid>,
    ) = sqlx::query_as(
        "SELECT last_checked_at, check_lease_until, lease_owner
         FROM agent_template_sources WHERE id = $1",
    )
    .bind(id)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(checked.is_some() && lease.is_none() && owner.is_none());
}

/// (C-1107, C-598's shape) Two replicas polling at once claim the due source once: one
/// ingests, the other finds the lease taken; the versions and the audit rows are written once.
#[tokio::test]
async fn two_replicas_polling_at_once_claim_a_due_source_once() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("UPDATE agent_template_sources SET next_check_at = now() - interval '1 second'")
        .bind(id)
        .execute(w.h.pool())
        .await
        .unwrap();
    let state = w.h.state.clone();
    let (a, b) = tokio::join!(poll::poll_once(&state), poll::poll_once(&state));
    let claimed = a.unwrap() + b.unwrap();
    assert_eq!(claimed, 1, "the two replicas together claim it once");
    assert_eq!(
        w.versions().await,
        vec![("1.0.0".into(), "pending_approval".into(), None)]
    );
    assert_eq!(w.audit_critical("agent_template.ingest").await, 1);
    // The lease is released, so the next poll finds nothing due.
    assert_eq!(poll::poll_once(&state).await.unwrap(), 0);
}

#[tokio::test]
async fn the_interval_never_is_never_due() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    let src = w
        .add_source("acme/tpl", json!({"update_interval": "never"}))
        .await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    let next: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT next_check_at FROM agent_template_sources WHERE id = $1")
            .bind(id)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    let days = (next - chrono::Utc::now()).num_days();
    assert!((360..=366).contains(&days), "next in {days}d");
    assert_eq!(poll::poll_once(&w.h.state).await.unwrap(), 0);
    // The manual check still works.
    let out = w.check(id).await;
    assert_eq!(out["status"], "up_to_date", "{out}");
    assert_eq!(out["waiting"].as_array().unwrap().len(), 1, "{out}");
}

/// A check that dies without writing its outcome (here: a database-level failure, the way
/// the SEC-146 D fixture refuses a commit) must not leave the source due on the next tick:
/// the lease is released AND the failure backs off 1 h, like every other failure.
#[tokio::test]
async fn a_check_that_errors_backs_off_instead_of_being_retried_every_tick() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    // The database refuses the check's head write (`set_head_sha`, only written by the
    // check's final transaction), so `check_source` returns Err — past the reads, before
    // any outcome was written.
    sqlx::query(
        "CREATE OR REPLACE FUNCTION aurora_tpl_fail_head() RETURNS trigger AS $$
         BEGIN IF NEW.last_head_sha IS DISTINCT FROM OLD.last_head_sha
         THEN RAISE EXCEPTION 'test: refused head write'; END IF; RETURN NEW; END;
         $$ LANGUAGE plpgsql",
    )
    .execute(w.h.pool())
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER aurora_tpl_head_trg BEFORE UPDATE ON agent_template_sources
         FOR EACH ROW EXECUTE FUNCTION aurora_tpl_fail_head()",
    )
    .execute(w.h.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE agent_template_sources SET next_check_at = now() - interval '1 second'")
        .bind(id)
        .execute(w.h.pool())
        .await
        .unwrap();

    assert_eq!(poll::poll_once(&w.h.state).await.unwrap(), 1);
    let row = w.source(id).await;
    assert_eq!(row["status"], "check_failed", "{row}");
    assert!(
        !row["check_failed_reason"].as_str().unwrap().is_empty(),
        "{row}"
    );
    let (lease, owner): (Option<chrono::DateTime<chrono::Utc>>, Option<Uuid>) = sqlx::query_as(
        "SELECT check_lease_until, lease_owner FROM agent_template_sources WHERE id = $1",
    )
    .bind(id)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(lease.is_none() && owner.is_none(), "lease still held");
    // Not due again for the backoff: the next tick claims nothing.
    let next: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT next_check_at FROM agent_template_sources WHERE id = $1")
            .bind(id)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    let mins = (next - chrono::Utc::now()).num_minutes();
    assert!((55..=65).contains(&mins), "next in {mins}m");
    assert_eq!(poll::poll_once(&w.h.state).await.unwrap(), 0);
}

/// A `never` source that errors keeps having no schedule: the backoff must not turn a
/// manual-only source into a scheduled one.
#[tokio::test]
async fn a_failed_check_leaves_a_never_source_unscheduled() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    let src = w
        .add_source("acme/tpl", json!({"update_interval": "never"}))
        .await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    sqlx::query(
        "CREATE OR REPLACE FUNCTION aurora_tpl_fail_head() RETURNS trigger AS $$
         BEGIN IF NEW.last_head_sha IS DISTINCT FROM OLD.last_head_sha
         THEN RAISE EXCEPTION 'test: refused head write'; END IF; RETURN NEW; END;
         $$ LANGUAGE plpgsql",
    )
    .execute(w.h.pool())
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER aurora_tpl_head_trg BEFORE UPDATE ON agent_template_sources
         FOR EACH ROW EXECUTE FUNCTION aurora_tpl_fail_head()",
    )
    .execute(w.h.pool())
    .await
    .unwrap();

    let out = w.check(id).await;
    assert_eq!(out["status"], "failed", "{out}");
    // Still ~a year out (`never`), not the 1 h failure backoff.
    let next: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT next_check_at FROM agent_template_sources WHERE id = $1")
            .bind(id)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    let days = (next - chrono::Utc::now()).num_days();
    assert!((360..=366).contains(&days), "next in {days}d");
    let (lease, owner): (Option<chrono::DateTime<chrono::Utc>>, Option<Uuid>) = sqlx::query_as(
        "SELECT check_lease_until, lease_owner FROM agent_template_sources WHERE id = $1",
    )
    .bind(id)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(lease.is_none() && owner.is_none(), "lease still held");
    assert_eq!(poll::poll_once(&w.h.state).await.unwrap(), 0);
}

// ---- the token (C-1125/C-1126, SEC-146) --------------------------------------------------------

#[tokio::test]
async fn the_token_goes_to_the_repository_origin_only() {
    let w = world().await;
    w.git.require_token("bot", "sekret");
    w.seed("acme/tpl", "t", "1.0.0");
    let zip = pack("t", "1.0.0");
    w.git.serve_file("pkg/t-1.0.0.zip", zip);
    let src = w
        .add_source(
            "acme/tpl",
            json!({"token": "sekret", "token_username": "bot"}),
        )
        .await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(src["token_set"], true, "{src}");

    // The git reads and the package download carry the Basic header (same origin).
    let out = w.check(id).await;
    assert_eq!(out["status"], "updated", "{out}");
    let reqs = w.git.requests();
    let pkg = reqs
        .iter()
        .filter(|r| r.target.contains("pkg/t-1.0.0.zip"))
        .collect::<Vec<_>>();
    assert!(!pkg.is_empty(), "{reqs:?}");
    let want = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("bot:sekret")
    );
    assert_eq!(
        pkg[0].authorization.as_deref(),
        Some(want.as_str()),
        "{pkg:?}"
    );

    // A redirect to another origin drops the header (C-1125's origin rule): the redirected
    // request arrives without any Authorization.
    w.git.serve_file("files/t-1.0.0.zip", pack("t", "1.0.0"));
    w.git.redirect(
        "pkg/t-1.0.0.zip",
        &format!("{CDN_ORIGIN}/files/t-1.0.0.zip"),
    );
    w.git.clear_requests();
    sqlx::query("DELETE FROM agent_template_versions")
        .execute(w.h.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE agent_template_sources SET last_head_sha = NULL WHERE id = $1")
        .bind(id)
        .execute(w.h.pool())
        .await
        .unwrap();
    let out = w.check(id).await;
    assert_eq!(
        w.versions().await,
        vec![(
            "1.0.0".into(),
            "awaiting_package".into(),
            Some("download_failed".into())
        )],
        "{out}"
    );
    let reqs = w.git.requests();
    let pkg = reqs
        .iter()
        .filter(|r| r.target.contains("pkg/t-1.0.0.zip"))
        .collect::<Vec<_>>();
    assert_eq!(pkg.len(), 1, "{reqs:?}");
    assert!(pkg[0].authorization.is_some(), "{pkg:?}");
    let files = reqs
        .iter()
        .filter(|r| r.target.contains("files/t-1.0.0.zip"))
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 1, "{reqs:?}");
    assert!(files[0].authorization.is_none(), "{:?}", files[0]);
}

// ---- the organization switches (C-1109) ---------------------------------------------------------

async fn org_list(w: &W, org: Uuid, who: &str) -> (u16, Value) {
    w.h.call(
        who,
        Method::GET,
        &format!("/v1/orgs/{org}/agent-templates"),
        None,
    )
    .await
}

async fn org_put(w: &W, org: Uuid, who: &str, template: &str, body: Value) -> (u16, Value) {
    w.h.call(
        who,
        Method::PUT,
        &format!("/v1/orgs/{org}/agent-templates/{template}"),
        Some(body),
    )
    .await
}

#[tokio::test]
async fn org_switches_turn_templates_off_and_on() {
    let w = world().await;
    let org = w.h.org().await;
    let owner = w.h.user("owner@x.io", false).await;
    w.h.member(org, owner, "owner", "active").await;
    let member = w.h.user("member@x.io", false).await;
    w.h.member(org, member, "member", "active").await;
    let owner_cookie = cookie(&w.h.good_session(owner).await.1);
    let member_cookie = cookie(&w.h.good_session(member).await.1);
    w.seed("acme/tpl", "t", "1.0.0");
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    w.check(id).await;

    // Every offered template is on by default; nothing is approved yet, so the version is null.
    let (s, v) = org_list(&w, org, &owner_cookie).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["items"],
        json!([{
            "template_id": "t",
            "name": "T",
            "summary": "A test template.",
            "version": null,
            "enabled": true,
        }]),
        "{v}"
    );

    // A plain member is refused (403); a non-member sees 404 (no bypass, not even for
    // instance administrators).
    let (s, v) = org_put(&w, org, &member_cookie, "t", json!({"enabled": false})).await;
    assert_eq!(s, 403, "{v}");
    let (s, v) = org_list(&w, org, &w.root).await;
    assert_eq!(s, 404, "{v}");

    // Turn it off and back on.
    let (s, v) = org_put(&w, org, &owner_cookie, "t", json!({"enabled": false})).await;
    assert_eq!(s, 204, "{v}");
    let (_, v) = org_list(&w, org, &owner_cookie).await;
    assert_eq!(v["items"][0]["enabled"], false, "{v}");
    let (s, v) = org_put(&w, org, &owner_cookie, "t", json!({"enabled": true})).await;
    assert_eq!(s, 204, "{v}");
    let (_, v) = org_list(&w, org, &owner_cookie).await;
    assert_eq!(v["items"][0]["enabled"], true, "{v}");

    // A switch for an unknown template is never stored (404).
    let (s, v) = org_put(&w, org, &owner_cookie, "nope", json!({"enabled": false})).await;
    assert_eq!(s, 404, "{v}");
    assert_eq!(code(&v), "not_found", "{v}");
    let n: i64 = w
        .count("SELECT count(*) FROM org_disabled_agent_templates")
        .await;
    assert_eq!(n, 0);

    // Ordinary audit, two writes (the on and the off); the write is fire-and-forget, so
    // poll briefly for it.
    let mut rows: Vec<(String, bool)> = Vec::new();
    for _ in 0..100 {
        rows = sqlx::query_as(
            "SELECT action, critical FROM audit_log WHERE action = 'org.template_availability'",
        )
        .fetch_all(w.h.pool())
        .await
        .unwrap();
        if rows.len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        rows,
        vec![("org.template_availability".into(), false); 2],
        "the ordinary audit rows"
    );
}

#[tokio::test]
async fn the_kill_switch_hides_the_org_routes_but_not_the_admin_ones() {
    let mut h = Harness::with_config(|c| c.agents.templates.enabled = false).await;
    let git = GitServer::start().await;
    let state = h
        .state
        .clone()
        .with_connect_override(&git.base())
        .with_library_fetch_timeout(std::time::Duration::from_secs(5));
    h.app = test_router(state.clone());
    h.state = state;
    let root = h.user("root@x.io", true).await;
    let root_cookie = cookie(&h.good_session(root).await.1);
    let org = h.org().await;
    git.add_repo("acme/tpl");
    git.commit(
        "acme/tpl",
        "main",
        &[
            put("aurora-templates.yaml".into(), manifest_yaml().into_bytes()),
            put(
                "t/template.yaml".into(),
                template_yaml("t", "1.0.0").into_bytes(),
            ),
            put("t/agent.yaml".into(), AGENT_YAML.as_bytes().to_vec()),
            put("t/instructions.md".into(), INSTRUCTIONS.as_bytes().to_vec()),
        ],
    );
    let (s, src) = h
        .call(
            &root_cookie,
            Method::POST,
            "/v1/admin/agent-template-sources",
            Some(json!({"url": format!("{GIT_ORIGIN}/acme/tpl")})),
        )
        .await;
    assert_eq!(s, 201, "{src}");
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    let (s, out) = h
        .call(
            &root_cookie,
            Method::POST,
            &format!("/v1/admin/agent-template-sources/{id}/check"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{out}");

    // The org routes answer 404 templates_disabled; the admin routes keep working.
    let (s, v) = h
        .call(
            &root_cookie,
            Method::GET,
            &format!("/v1/orgs/{org}/agent-templates"),
            None,
        )
        .await;
    assert_eq!(s, 404, "{v}");
    assert_eq!(code(&v), "templates_disabled", "{v}");
    let (s, v) = h
        .call(
            &root_cookie,
            Method::PUT,
            &format!("/v1/orgs/{org}/agent-templates/t"),
            Some(json!({"enabled": false})),
        )
        .await;
    assert_eq!(s, 404, "{v}");
    assert_eq!(code(&v), "templates_disabled", "{v}");
    let (s, v) = h
        .call(&root_cookie, Method::GET, "/v1/admin/agent-templates", None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"][0]["template_id"], "t", "{v}");

    // The workspace routes (C-1110) answer 404 templates_disabled too — before any
    // authority check or catalogue work.
    let owner = h.user("owner@x.io", false).await;
    h.member(org, owner, "owner", "active").await;
    let ws = h.workspace(org).await;
    h.bind_ws(ws, owner, "owner").await;
    let owner_cookie = cookie(&h.good_session(owner).await.1);
    for path in [
        format!("/v1/workspaces/{ws}/agent-templates"),
        format!("/v1/workspaces/{ws}/agent-templates/t"),
        format!("/v1/workspaces/{ws}/agent-templates/t/avatar"),
    ] {
        let (s, v) = h.call(&owner_cookie, Method::GET, &path, None).await;
        assert_eq!(s, 404, "{path}: {v}");
        assert_eq!(code(&v), "templates_disabled", "{path}: {v}");
    }
}

// ---- the workspace catalogue (C-1110) -----------------------------------------------------------

async fn ws_get(w: &W, ws: Uuid, who: &str, path: &str) -> (u16, Value) {
    w.h.call(
        who,
        Method::GET,
        &format!("/v1/workspaces/{ws}/agent-templates{path}"),
        None,
    )
    .await
}

#[tokio::test]
async fn the_workspace_catalogue_shows_available_templates() {
    let w = world().await;
    let (_, ws, owner) = w.ws_owner().await;
    w.seed("acme/tpl", "t", "1.0.0");
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    w.check(id).await;
    w.approve("t", "1.0.0").await;

    let (s, v) = ws_get(&w, ws, &owner, "").await;
    assert_eq!(s, 200, "{v}");
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{v}");
    assert_eq!(items[0]["template_id"], "t", "{v}");
    assert_eq!(items[0]["version"], "1.0.0", "{v}");
    assert_eq!(items[0]["source_name"], "Acme templates", "{v}");
    assert!(
        items[0]["readme"]
            .as_str()
            .unwrap()
            .contains("A test template."),
        "{v}"
    );
    assert_eq!(items[0]["has_avatar"], false, "{v}");
    assert_eq!(items[0]["has_helpers"], false, "{v}");
    assert_eq!(items[0]["apps"], json!([]), "{v}");
    assert_eq!(items[0]["skill_refs"], json!([]), "{v}");
    assert_eq!(items[0]["automations"], json!([]), "{v}");
    assert_eq!(items[0]["ready"], true, "{v}");

    // The plan: the reviewed content in full.
    let (s, v) = ws_get(&w, ws, &owner, "/t").await;
    assert_eq!(s, 200, "{v}");
    assert!(v["content_sha256"].is_string(), "{v}");
    assert_eq!(v["content"]["name"], "T", "{v}");
    assert_eq!(v["source_name"], "Acme templates", "{v}");
    assert_eq!(v["entries"], json!([]), "{v}");
    assert_eq!(v["skill_refs"], json!([]), "{v}");

    // No avatar in the content.
    let (s, _) = ws_get(&w, ws, &owner, "/t/avatar").await;
    assert_eq!(s, 404);

    // A person who cannot use the workspace is 404 (no bypass, not even for instance
    // administrators).
    let stranger = w.h.user("stranger@x.io", false).await;
    let stranger_cookie = cookie(&w.h.good_session(stranger).await.1);
    let (s, v) =
        w.h.call(
            &stranger_cookie,
            Method::GET,
            &format!("/v1/workspaces/{ws}/agent-templates"),
            None,
        )
        .await;
    assert_eq!(s, 404, "{v}");
    let (s, v) =
        w.h.call(
            &w.root,
            Method::GET,
            &format!("/v1/workspaces/{ws}/agent-templates"),
            None,
        )
        .await;
    assert_eq!(s, 404, "{v}");

    // Withdrawn: gone from the catalogue, the plan and the avatar 404.
    let (s, v) = w
        .call(
            Method::PATCH,
            "/v1/admin/agent-templates/t",
            Some(json!({"offered": false})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, v) = ws_get(&w, ws, &owner, "").await;
    assert_eq!(v["items"].as_array().unwrap().len(), 0, "{v}");
    let (s, _) = ws_get(&w, ws, &owner, "/t").await;
    assert_eq!(s, 404);
    let (s, _) = ws_get(&w, ws, &owner, "/t/avatar").await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn the_plan_resolves_apps_and_skill_references() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;

    // The skill library the template references, with one valid skill.
    w.git.add_repo("acme/skills");
    w.git.commit(
        "acme/skills",
        "main",
        &[put(
            "alpha/SKILL.md".into(),
            b"---\ndescription: Alpha skill\n---\n\nAlpha body.\n".to_vec(),
        )],
    );
    let (s, lib) =
        w.h.call(
            &owner,
            Method::POST,
            &format!("/v1/orgs/{org}/skill-libraries"),
            Some(json!({"label": "lib", "url": format!("{GIT_ORIGIN}/acme/skills")})),
        )
        .await;
    assert_eq!(s, 201, "{lib}");
    assert_eq!(lib["status"], "active", "{lib}");

    // The template: one required app (nothing installed) and two skill references.
    w.git.add_repo("acme/tpl2");
    let mut changes = vec![put(
        "aurora-templates.yaml".into(),
        manifest_yaml().into_bytes(),
    )];
    for (p, b) in rich_files("t2", "1.0.0") {
        changes.push(put(p, b));
    }
    w.git.commit("acme/tpl2", "main", &changes);
    w.git
        .serve_file("pkg/t2-1.0.0.zip", rich_pack("t2", "1.0.0"));
    let src = w.add_source("acme/tpl2", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    w.check(id).await;
    w.approve("t2", "1.0.0").await;

    let (s, v) = ws_get(&w, ws, &owner, "/t2").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ready"], false, "{v}");
    let entries = v["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{v}");
    assert_eq!(entries[0]["index"], 0, "{v}");
    assert_eq!(entries[0]["required"], true, "{v}");
    assert_eq!(entries[0]["why"], "Needs the thing.", "{v}");
    let choice = &entries[0]["choices"][0];
    assert_eq!(choice["app_id"], "com-example-thing", "{v}");
    assert_eq!(choice["app_name"], "com-example-thing", "{v}");
    assert_eq!(choice["available"], false, "{v}");
    assert_eq!(choice["unavailable_reason"], "not_installed", "{v}");
    assert_eq!(choice["level"], "act", "{v}");
    assert_eq!(choice["level_note"], json!(null), "{v}");
    assert_eq!(choice["custom"], json!(null), "{v}");
    assert_eq!(choice["accounts"], json!([]), "{v}");
    let refs = v["skill_refs"].as_array().unwrap();
    assert_eq!(refs.len(), 2, "{v}");
    assert_eq!(refs[0]["kind"], "library", "{v}");
    assert_eq!(refs[0]["name"], "alpha", "{v}");
    assert_eq!(refs[0]["available"], true, "{v}");
    assert_eq!(refs[0]["unavailable_reason"], json!(null), "{v}");
    // The reference's own library address, in the C-1123 display form.
    assert_eq!(refs[0]["repo"], "git.example/acme/skills", "{v}");
    assert!(refs[0]["library"]["id"].is_string(), "{v}");
    assert_eq!(refs[0]["library"]["scope"], "organization", "{v}");
    assert!(refs[0]["library"]["commit_sha"].is_string(), "{v}");
    assert_eq!(refs[0]["content"]["use_it_when"], "Alpha skill", "{v}");
    assert_eq!(refs[0]["content"]["body"], "Alpha body.\n", "{v}");
    // The SKILL.md is the body; the skill has no other files.
    assert_eq!(refs[0]["content"]["files"].as_array().unwrap().len(), 0);
    assert_eq!(refs[1]["kind"], "organization", "{v}");
    assert_eq!(refs[1]["name"], "notes", "{v}");
    assert_eq!(refs[1]["available"], false, "{v}");
    assert_eq!(refs[1]["unavailable_reason"], "not_in_organization", "{v}");
    assert_eq!(refs[1]["repo"], json!(null), "{v}");
    assert_eq!(v["source_name"], "Acme templates", "{v}");
    // The card carries the same availability.
    let (s, v) = ws_get(&w, ws, &owner, "").await;
    assert_eq!(s, 200, "{v}");
    let item = &v["items"][0];
    assert_eq!(item["apps"][0]["available"], false, "{v}");
    assert_eq!(item["apps"][0]["skills"], json!([]), "{v}");
    assert_eq!(item["skill_refs"][0]["available"], true, "{v}");
    assert_eq!(
        item["skill_refs"][0]["repo"], "git.example/acme/skills",
        "{v}"
    );
    assert_eq!(item["skill_refs"][1]["available"], false, "{v}");
    assert_eq!(item["skill_refs"][1]["repo"], json!(null), "{v}");
    assert_eq!(item["source_name"], "Acme templates", "{v}");
    assert_eq!(item["ready"], false, "{v}");

    // The organization-tier skill exists → the reference is available.
    let (s, v) =
        w.h.call(
            &owner,
            Method::POST,
            &format!("/v1/orgs/{org}/skills"),
            Some(json!({"name": "notes", "description": "Org notes", "body": "How we note."})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = ws_get(&w, ws, &owner, "/t2").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["skill_refs"][1]["available"], true, "{v}");
    assert_eq!(v["skill_refs"][1]["library"], json!(null), "{v}");
    assert_eq!(v["skill_refs"][1]["content"], json!(null), "{v}");
    // Still not ready: the app is not installed.
    assert_eq!(v["ready"], false, "{v}");

    // The organization turned the template off: gone from the catalogue, 404 elsewhere.
    let (s, v) =
        w.h.call(
            &owner,
            Method::PUT,
            &format!("/v1/orgs/{org}/agent-templates/t2"),
            Some(json!({"enabled": false})),
        )
        .await;
    assert_eq!(s, 204, "{v}");
    let (_, v) = ws_get(&w, ws, &owner, "").await;
    assert_eq!(v["items"].as_array().unwrap().len(), 0, "{v}");
    let (s, _) = ws_get(&w, ws, &owner, "/t2").await;
    assert_eq!(s, 404);
}

// ---- the catalogue availability reasons (C-1110, C-1142) ----------------------------------------

#[tokio::test]
async fn the_plan_names_why_an_app_or_a_skill_is_not_available() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;

    // The rich template: one required app (nothing installed), a library reference and an
    // organization reference.
    w.git.add_repo("acme/tpl2");
    let mut changes = vec![put(
        "aurora-templates.yaml".into(),
        manifest_yaml().into_bytes(),
    )];
    for (p, b) in rich_files("t2", "1.0.0") {
        changes.push(put(p, b));
    }
    w.git.commit("acme/tpl2", "main", &changes);
    w.git
        .serve_file("pkg/t2-1.0.0.zip", rich_pack("t2", "1.0.0"));
    let src = w.add_source("acme/tpl2", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    w.check(id).await;
    w.approve("t2", "1.0.0").await;

    // No library is added yet, the organization skill is missing, the app is not installed.
    let (s, v) = ws_get(&w, ws, &owner, "/t2").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["skill_refs"][0]["unavailable_reason"], "library_not_added",
        "{v}"
    );
    // The repository is named whether the library is added or not.
    assert_eq!(v["skill_refs"][0]["repo"], "git.example/acme/skills", "{v}");
    assert_eq!(
        v["skill_refs"][1]["unavailable_reason"], "not_in_organization",
        "{v}"
    );
    assert_eq!(
        v["entries"][0]["choices"][0]["unavailable_reason"], "not_installed",
        "{v}"
    );
    assert_eq!(v["ready"], false, "{v}");
    let (_, v) = ws_get(&w, ws, &owner, "").await;
    assert_eq!(v["items"][0]["skill_refs"][0]["available"], false, "{v}");
    assert_eq!(
        v["items"][0]["skill_refs"][0]["repo"], "git.example/acme/skills",
        "{v}"
    );

    // A library that does not carry the skill: skill_not_in_library.
    w.git.add_repo("acme/skills");
    w.git.commit(
        "acme/skills",
        "main",
        &[put(
            "beta/SKILL.md".into(),
            b"---\ndescription: Beta skill\n---\n\nBeta body.\n".to_vec(),
        )],
    );
    let (s, lib) =
        w.h.call(
            &owner,
            Method::POST,
            &format!("/v1/orgs/{org}/skill-libraries"),
            Some(json!({"label": "lib", "url": format!("{GIT_ORIGIN}/acme/skills")})),
        )
        .await;
    assert_eq!(s, 201, "{lib}");
    let lib: Uuid = lib["id"].as_str().unwrap().parse().unwrap();
    let (s, v) = ws_get(&w, ws, &owner, "/t2").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["skill_refs"][0]["unavailable_reason"], "skill_not_in_library",
        "{v}"
    );
    assert_eq!(v["skill_refs"][0]["library"], json!(null), "{v}");
    assert_eq!(v["skill_refs"][0]["repo"], "git.example/acme/skills", "{v}");

    // The skill appears at the library's head; any patch forgets the browse cache: available,
    // with the content the plan shows.
    w.git.commit(
        "acme/skills",
        "main",
        &[put(
            "alpha/SKILL.md".into(),
            b"---\ndescription: Alpha skill\n---\n\nAlpha body.\n".to_vec(),
        )],
    );
    let (s, v) =
        w.h.call(
            &owner,
            Method::PATCH,
            &format!("/v1/skill-libraries/{lib}"),
            Some(json!({"label": "lib"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = ws_get(&w, ws, &owner, "/t2").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["skill_refs"][0]["available"], true, "{v}");
    assert_eq!(v["skill_refs"][0]["unavailable_reason"], json!(null), "{v}");
    assert_eq!(
        v["skill_refs"][0]["content"]["use_it_when"], "Alpha skill",
        "{v}"
    );

    // A paused library is unreadable (the card says the same).
    let (s, v) =
        w.h.call(
            &owner,
            Method::PATCH,
            &format!("/v1/skill-libraries/{lib}"),
            Some(json!({"status": "paused"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = ws_get(&w, ws, &owner, "/t2").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["skill_refs"][0]["unavailable_reason"], "library_unreadable",
        "{v}"
    );
    assert_eq!(v["skill_refs"][0]["content"], json!(null), "{v}");
    assert_eq!(v["skill_refs"][0]["repo"], "git.example/acme/skills", "{v}");
    let (_, v) = ws_get(&w, ws, &owner, "").await;
    assert_eq!(v["items"][0]["skill_refs"][0]["available"], false, "{v}");

    // Resume, install the app, and restrict the organization's app availability without an
    // allow entry: the installed app is not_available_here.
    let (s, v) =
        w.h.call(
            &owner,
            Method::PATCH,
            &format!("/v1/skill-libraries/{lib}"),
            Some(json!({"status": "active"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let admin = crate::plugins_lifecycle::Admin::trust(&w.h, w.root.clone()).await;
    admin.install(APP_PKG_YAML).await;
    sqlx::query("UPDATE organizations SET app_posture = 'restricted' WHERE id = $1")
        .bind(org)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, v) = ws_get(&w, ws, &owner, "/t2").await;
    assert_eq!(s, 200, "{v}");
    let choice = &v["entries"][0]["choices"][0];
    assert_eq!(choice["available"], false, "{v}");
    assert_eq!(choice["unavailable_reason"], "not_available_here", "{v}");
    assert_eq!(v["ready"], false, "{v}");

    // An org-wide allow entry and the organization skill make it ready.
    sqlx::query(
        "INSERT INTO org_app_availability (org_id, app_id) VALUES ($1, 'com-example-thing')",
    )
    .bind(org)
    .execute(w.h.pool())
    .await
    .unwrap();
    let (s, v) =
        w.h.call(
            &owner,
            Method::POST,
            &format!("/v1/orgs/{org}/skills"),
            Some(json!({"name": "notes", "description": "Org notes", "body": "How we note."})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = ws_get(&w, ws, &owner, "/t2").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["entries"][0]["choices"][0]["available"], true, "{v}");
    assert_eq!(
        v["entries"][0]["choices"][0]["unavailable_reason"],
        json!(null),
        "{v}"
    );
    assert_eq!(v["skill_refs"][1]["available"], true, "{v}");
    assert_eq!(v["ready"], true, "{v}");
}

/// A skill that does not fit the agent tier's library budget is `over_budget` (C-1142), in
/// the plan and on the card.
#[tokio::test]
async fn a_library_skill_that_does_not_fit_the_agent_budget_is_over_budget() {
    // A tiny per-agent budget (the operator may only lower it).
    let w = world_with(|c| c.skills_library.bytes_per_agent = 10).await;
    let (org, ws, owner) = w.ws_owner().await;

    w.git.add_repo("acme/skills");
    w.git.commit(
        "acme/skills",
        "main",
        &[put(
            "alpha/SKILL.md".into(),
            b"---\ndescription: Alpha skill\n---\n\nAlpha body.\n".to_vec(),
        )],
    );
    let (s, lib) =
        w.h.call(
            &owner,
            Method::POST,
            &format!("/v1/orgs/{org}/skill-libraries"),
            Some(json!({"label": "lib", "url": format!("{GIT_ORIGIN}/acme/skills")})),
        )
        .await;
    assert_eq!(s, 201, "{lib}");

    w.git.add_repo("acme/tpl");
    let mut changes = vec![put(
        "aurora-templates.yaml".into(),
        manifest_yaml().into_bytes(),
    )];
    for (p, b) in rich_files("t2", "1.0.0") {
        changes.push(put(p, b));
    }
    w.git.commit("acme/tpl", "main", &changes);
    w.git
        .serve_file("pkg/t2-1.0.0.zip", rich_pack("t2", "1.0.0"));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    w.check(id).await;
    w.approve("t2", "1.0.0").await;

    let (s, v) = ws_get(&w, ws, &owner, "/t2").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["skill_refs"][0]["available"], false, "{v}");
    assert_eq!(
        v["skill_refs"][0]["unavailable_reason"], "over_budget",
        "{v}"
    );
    assert_eq!(v["skill_refs"][0]["content"], json!(null), "{v}");
    assert_eq!(v["skill_refs"][0]["repo"], "git.example/acme/skills", "{v}");
    assert_eq!(v["ready"], false, "{v}");
    let (_, v) = ws_get(&w, ws, &owner, "").await;
    assert_eq!(v["items"][0]["skill_refs"][0]["available"], false, "{v}");
}

// ---- the acceptance gaps: decide, offer, attention, accounts, custom_only -----------------------

/// A template whose `template.yaml` is `yaml` (the rest as [`files`]), committed to `repo`.
fn seed_yaml(w: &W, repo: &str, dir: &str, yaml: String) {
    w.git.add_repo(repo);
    let mut changes = vec![put(
        "aurora-templates.yaml".into(),
        manifest_yaml().into_bytes(),
    )];
    for (p, b) in [
        (format!("{dir}/template.yaml"), yaml.into_bytes()),
        (format!("{dir}/agent.yaml"), AGENT_YAML.as_bytes().to_vec()),
        (
            format!("{dir}/instructions.md"),
            INSTRUCTIONS.as_bytes().to_vec(),
        ),
        (
            format!("{dir}/README.md"),
            format!("# {dir}\nA test template.\n").into_bytes(),
        ),
    ] {
        changes.push(put(p, b));
    }
    w.git.commit(repo, "main", &changes);
}

/// The package zip matching [`seed_yaml`].
fn pack_yaml(dir: &str, yaml: String) -> Vec<u8> {
    let tree: Vec<TreeFile> = [
        (format!("{dir}/template.yaml"), yaml.into_bytes()),
        (format!("{dir}/agent.yaml"), AGENT_YAML.as_bytes().to_vec()),
        (
            format!("{dir}/instructions.md"),
            INSTRUCTIONS.as_bytes().to_vec(),
        ),
        (
            format!("{dir}/README.md"),
            format!("# {dir}\nA test template.\n").into_bytes(),
        ),
    ]
    .into_iter()
    .map(|(path, bytes)| TreeFile {
        path,
        bytes,
        mode: 0o100644,
    })
    .collect();
    aurora_template::pack(dir, &tree)
}

/// `template.yaml` naming one app (the plan's accounts and `custom_only` tests).
fn app_yaml(id: &str, version: &str, tools: &str) -> String {
    format!(
        "format: 1\nid: {id}\nversion: {version}\nname: App template\nsummary: One app.\n\
         inputs:\n  - id: zone\n    label: Zone\n    pattern: \"^[a-z]+$\"\n\
         apps:\n  - app: com-example-thing\n    required: true\n    level: {level}\n    why: Needs the thing.{tools}",
        level = if tools.is_empty() { "read" } else { "act" },
    )
}

/// The active package the templates' app lives on (installed through the plugins lifecycle):
/// `com-example-thing` with a two-tool catalogue and `custom_only`, plus an app no template
/// names (an account that must never show in the plan's choices).
const APP_PKG_YAML: &str = r#"package: { id: acme, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: P }
apps:
  - about: { id: com-example-thing, name: Thing, description: D, category: dev }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    grants: { custom_only: true, block_untooled: true }
    tools:
      read:
        items:
          - { id: look, name: Look, host: api.thing.example, path: /x, methods: [GET] }
      write:
        items:
          - { id: change, name: Change, host: api.thing.example, path: /x, methods: [POST] }
    hosts:
      - match: { host: "api.thing.example" }
        inject:
          - bearer: "{credentials.api_token}"
  - about: { id: com-example-other, name: Other, description: D, category: dev }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    hosts:
      - match: { host: "api.other.example" }
        inject:
          - bearer: "{credentials.api_token}"
"#;

/// A real app connection of `app` (the org when `ws` is None) with `access`.
async fn conn(w: &W, org: Uuid, app: &str, ws: Option<Uuid>, access: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version,
            label, status, credentials_ref, access)
         VALUES ($1,$2,$3,$4,'acme','1.0.0',$5,'active','x',$6)",
    )
    .bind(id)
    .bind(org)
    .bind(ws)
    .bind(app)
    .bind(format!("{app} account"))
    .bind(access)
    .execute(w.h.pool())
    .await
    .unwrap();
    id
}

/// The attention items about agent template versions.
async fn template_attention(w: &W) -> Vec<Value> {
    let (s, v) = w.call(Method::GET, "/v1/admin/attention", None).await;
    assert_eq!(s, 200, "{v}");
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"].as_str() == Some("agent_template_version"))
        .cloned()
        .collect()
}

#[tokio::test]
async fn approve_is_bound_to_the_reviewed_hash() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    w.check(id).await;

    // A hash that is not the one the review showed (a lie, a wrong one, the empty string):
    // 409, the version stays pending, no audit row.
    let long = "f".repeat(64);
    for sha in ["", "00", long.as_str()] {
        let (s, v) = w
            .call(
                Method::POST,
                "/v1/admin/agent-templates/t/versions/1.0.0/approve",
                Some(json!({"content_sha256": sha})),
            )
            .await;
        assert_eq!((s, code(&v)), (409, "template_content_changed"), "{v}");
    }
    assert_eq!(
        w.versions().await,
        vec![("1.0.0".into(), "pending_approval".into(), None)]
    );
    assert_eq!(w.audit_critical("agent_template.approve").await, 0);

    // The hash the detail showed approves; a second decide is a conflict, not a rewrite.
    w.approve("t", "1.0.0").await;
    assert_eq!(
        w.versions().await,
        vec![("1.0.0".into(), "approved".into(), None)]
    );
    assert_eq!(w.audit_critical("agent_template.approve").await, 1);
    let sha = w.content_sha("1.0.0").await.unwrap();
    let (s, v) = w
        .call(
            Method::POST,
            "/v1/admin/agent-templates/t/versions/1.0.0/approve",
            Some(json!({"content_sha256": sha})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "invalid_state"), "{v}");
    assert_eq!(w.audit_critical("agent_template.approve").await, 1);
}

#[tokio::test]
async fn a_rejected_version_is_never_ingested_again() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    w.check(id).await;

    let (s, v) = w
        .call(
            Method::POST,
            "/v1/admin/agent-templates/t/versions/1.0.0/reject",
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "rejected", "{v}");
    assert_eq!(w.audit_critical("agent_template.reject").await, 1);

    // The same head again: up_to_date, nothing is re-ingested even though the package is
    // still served.
    let out = w.check(id).await;
    assert_eq!(out["status"], "up_to_date", "{out}");
    assert_eq!(out["ingested"], json!([]), "{out}");

    // Deciding a rejected version is a conflict in both directions.
    for path in ["reject", "approve"] {
        let (s, v) = w
            .call(
                Method::POST,
                &format!("/v1/admin/agent-templates/t/versions/1.0.0/{path}"),
                Some(json!({"content_sha256": "x"})),
            )
            .await;
        assert_eq!((s, code(&v)), (409, "invalid_state"), "{v}");
    }

    // A newer head comes in normally; the rejected one keeps its status.
    w.bump(
        "acme/tpl",
        &files("t", "1.1.0")
            .into_iter()
            .map(|(p, b)| put(p, b))
            .collect::<Vec<_>>(),
    );
    w.git.serve_file("pkg/t-1.1.0.zip", pack("t", "1.1.0"));
    let out = w.check(id).await;
    assert_eq!(out["status"], "updated", "{out}");
    assert_eq!(
        w.versions().await,
        vec![
            ("1.0.0".into(), "rejected".into(), None),
            ("1.1.0".into(), "pending_approval".into(), None),
        ]
    );

    // And the head moving back to the rejected version is version_not_newer: still never
    // ingested again.
    w.bump(
        "acme/tpl",
        &files("t", "1.0.0")
            .into_iter()
            .map(|(p, b)| put(p, b))
            .collect::<Vec<_>>(),
    );
    let out = w.check(id).await;
    assert_eq!(out["problems"][0]["code"], "version_not_newer", "{out}");
    assert_eq!(out["ingested"], json!([]), "{out}");
    assert_eq!(
        w.versions().await,
        vec![
            ("1.0.0".into(), "rejected".into(), None),
            ("1.1.0".into(), "pending_approval".into(), None),
        ]
    );
}

#[tokio::test]
async fn the_previous_version_stays_offered_until_a_newer_one_is_approved() {
    let w = world().await;
    let (_, ws, owner) = w.ws_owner().await;
    w.seed("acme/tpl", "t", "1.0.0");
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    w.check(id).await;
    w.approve("t", "1.0.0").await;

    // The list's and the catalogue's offered version.
    let current = || async {
        let (s, v) = w.call(Method::GET, "/v1/admin/agent-templates", None).await;
        assert_eq!(s, 200, "{v}");
        v["items"][0]["current_version"].clone()
    };
    let catalogue = || async {
        let (_, v) = ws_get(&w, ws, &owner, "").await;
        v["items"][0]["version"].clone()
    };
    assert_eq!(current().await, json!("1.0.0"));
    assert_eq!(catalogue().await, json!("1.0.0"));

    // A newer head whose package never appeared: awaiting, the previous version keeps
    // serving (the plan still says 1.0.0).
    w.bump(
        "acme/tpl",
        &files("t", "1.1.0")
            .into_iter()
            .map(|(p, b)| put(p, b))
            .collect::<Vec<_>>(),
    );
    assert_eq!(w.check(id).await["status"], "up_to_date");
    assert_eq!(
        w.versions().await,
        vec![
            ("1.0.0".into(), "approved".into(), None),
            (
                "1.1.0".into(),
                "awaiting_package".into(),
                Some("package_not_found".into())
            ),
        ]
    );
    assert_eq!(current().await, json!("1.0.0"));
    let (s, v) = ws_get(&w, ws, &owner, "/t").await;
    assert_eq!((s, v["version"].as_str()), (200, Some("1.0.0")), "{v}");

    // The package arrives: pending, still 1.0.0.
    w.git.serve_file("pkg/t-1.1.0.zip", pack("t", "1.1.0"));
    assert_eq!(w.check(id).await["status"], "updated");
    assert_eq!(current().await, json!("1.0.0"));

    // Rejected: still 1.0.0.
    let (s, _) = w
        .call(
            Method::POST,
            "/v1/admin/agent-templates/t/versions/1.1.0/reject",
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(current().await, json!("1.0.0"));

    // Invalid (a broken package): still 1.0.0.
    w.bump(
        "acme/tpl",
        &files("t", "1.2.0")
            .into_iter()
            .map(|(p, b)| put(p, b))
            .collect::<Vec<_>>(),
    );
    w.git
        .serve_file("pkg/t-1.2.0.zip", vec![0x50, 0x4b, 0x00, 0x00]);
    assert_eq!(w.check(id).await["status"], "updated");
    assert_eq!(current().await, json!("1.0.0"));

    // A failed check changes nothing either.
    w.git.override_status("acme/tpl", 500, None);
    assert_eq!(w.check(id).await["status"], "failed");
    assert_eq!(current().await, json!("1.0.0"));
    w.git.clear_overrides();

    // After approve the newer one is offered — and withdraw falls back to the newest
    // approved, offer again serves it.
    w.bump(
        "acme/tpl",
        &files("t", "1.3.0")
            .into_iter()
            .map(|(p, b)| put(p, b))
            .collect::<Vec<_>>(),
    );
    w.git.serve_file("pkg/t-1.3.0.zip", pack("t", "1.3.0"));
    assert_eq!(w.check(id).await["status"], "updated");
    w.approve("t", "1.3.0").await;
    assert_eq!(current().await, json!("1.3.0"));
    assert_eq!(catalogue().await, json!("1.3.0"));
    let (s, _) = w
        .call(
            Method::PATCH,
            "/v1/admin/agent-templates/t",
            Some(json!({"offered": false})),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(current().await, json!(null));
    let (_, v) = ws_get(&w, ws, &owner, "").await;
    assert_eq!(v["items"].as_array().unwrap().len(), 0, "{v}");
    let (s, _) = w
        .call(
            Method::PATCH,
            "/v1/admin/agent-templates/t",
            Some(json!({"offered": true})),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(current().await, json!("1.3.0"));
    assert_eq!(catalogue().await, json!("1.3.0"));
    assert_eq!(w.audit_critical("agent_template.update").await, 2);
}

#[tokio::test]
async fn template_attention_items_appear_and_disappear() {
    let w = world().await;
    w.seed("acme/tpl", "t", "1.0.0");
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();

    // Nothing before the first check.
    assert!(template_attention(&w).await.is_empty());

    // A pending version: one decision item linking the review.
    w.check(id).await;
    let items = template_attention(&w).await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["decision"], true, "{items:?}");
    // (FIX-245) The topic is the page that resolves the item: What agents may do, not Apps.
    assert_eq!(items[0]["topic"], "internet-access", "{items:?}");
    assert_eq!(
        items[0]["href"], "/admin/internet-access/templates/t/1.0.0",
        "{items:?}"
    );
    assert!(
        items[0]["title"]
            .as_str()
            .unwrap()
            .contains("waiting for review"),
        "{items:?}"
    );
    // The notice row (the one e-mail per admin) was written with it.
    let notices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM admin_attention_notices WHERE kind = 'agent_template_version'",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(notices, 1);

    // Approved: the item disappears.
    w.approve("t", "1.0.0").await;
    assert!(template_attention(&w).await.is_empty());

    // A version waiting for its package is not an item yet (younger than the hour).
    w.bump(
        "acme/tpl",
        &files("t", "1.1.0")
            .into_iter()
            .map(|(p, b)| put(p, b))
            .collect::<Vec<_>>(),
    );
    assert_eq!(w.check(id).await["status"], "up_to_date");
    assert!(template_attention(&w).await.is_empty());

    // Older than an hour: the informative item appears (not a decision).
    sqlx::query("UPDATE agent_template_versions SET created_at = now() - interval '2 hours' WHERE version = '1.1.0'")
        .execute(w.h.pool())
        .await
        .unwrap();
    let items = template_attention(&w).await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(
        items[0]["title"]
            .as_str()
            .unwrap()
            .contains("waiting for its package"),
        "{items:?}"
    );
    assert_eq!(items[0]["topic"], "internet-access", "{items:?}");
    assert_eq!(
        items[0]["href"], "/admin/internet-access/templates/t/1.1.0",
        "{items:?}"
    );

    // The package arrives: the waiting item is gone, the decision item is back.
    w.git.serve_file("pkg/t-1.1.0.zip", pack("t", "1.1.0"));
    assert_eq!(w.check(id).await["status"], "updated");
    let items = template_attention(&w).await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["decision"], true, "{items:?}");
    assert!(
        items[0]["title"]
            .as_str()
            .unwrap()
            .contains("waiting for review"),
        "{items:?}"
    );
    let notices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM admin_attention_notices WHERE kind = 'agent_template_version'",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(notices, 2);

    // Rejected: gone again.
    let (s, _) = w
        .call(
            Method::POST,
            "/v1/admin/agent-templates/t/versions/1.1.0/reject",
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert!(template_attention(&w).await.is_empty());
}

#[tokio::test]
async fn the_plan_offers_only_the_accounts_the_caller_may_give() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;
    let member = w.h.user("member@x.io", false).await;
    w.h.member(org, member, "member", "active").await;
    w.h.bind_ws(ws, member, "member").await;
    let member_cookie = cookie(&w.h.good_session(member).await.1);

    // The app is really installed (an active package with its two-tool catalogue).
    let admin = crate::plugins_lifecycle::Admin::trust(&w.h, w.root.clone()).await;
    admin.install(APP_PKG_YAML).await;

    seed_yaml(&w, "acme/tpl", "t5", app_yaml("t5", "1.0.0", ""));
    w.git.serve_file(
        "pkg/t5-1.0.0.zip",
        pack_yaml("t5", app_yaml("t5", "1.0.0", "")),
    );
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    w.check(id).await;
    w.approve("t5", "1.0.0").await;

    // Real connections: two on the template's app (one org-wide, one this workspace), one
    // on the app no template names.
    let mine = conn(&w, org, "com-example-thing", Some(ws), "write").await;
    let org_wide = conn(&w, org, "com-example-thing", None, "write").await;
    let _other = conn(&w, org, "com-example-other", Some(ws), "write").await;

    // The owner's plan: exactly the app's two accounts, each givable.
    let (s, v) = ws_get(&w, ws, &owner, "/t5").await;
    assert_eq!(s, 200, "{v}");
    let accounts = v["entries"][0]["choices"][0]["accounts"]
        .as_array()
        .unwrap();
    let ids: Vec<Uuid> = accounts
        .iter()
        .map(|a| a["ref"]["id"].as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(ids.len(), 2, "{v}");
    assert!(ids.contains(&mine) && ids.contains(&org_wide), "{v}");
    assert!(
        accounts.iter().all(|a| a["can"]["give_access"] == true),
        "{v}"
    );
    assert_eq!(v["entries"][0]["choices"][0]["available"], true, "{v}");

    // A plain member of the same workspace may not give any account: the choices list none.
    let (s, v) = ws_get(&w, ws, &member_cookie, "/t5").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["entries"][0]["choices"][0]["accounts"], json!([]), "{v}");
}

#[tokio::test]
async fn custom_only_caps_the_level_and_drops_unknown_tools() {
    let w = world().await;
    let (_org, ws, owner) = w.ws_owner().await;
    let admin = crate::plugins_lifecycle::Admin::trust(&w.h, w.root.clone()).await;
    admin.install(APP_PKG_YAML).await;

    let yaml = app_yaml(
        "t6",
        "1.0.0",
        "\n    tools:\n      allow: [look, gone]\n      ask: [change, peek]\n",
    );
    seed_yaml(&w, "acme/tpl", "t6", yaml.clone());
    w.git.serve_file("pkg/t6-1.0.0.zip", pack_yaml("t6", yaml));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    w.check(id).await;
    w.approve("t6", "1.0.0").await;

    // The plan: `act` capped to `ask` (the app is custom_only), the template's tools cut to
    // the catalogue, the unknown ones named as dropped.
    let (s, v) = ws_get(&w, ws, &owner, "/t6").await;
    assert_eq!(s, 200, "{v}");
    let choice = &v["entries"][0]["choices"][0];
    assert_eq!(choice["app_id"], "com-example-thing", "{v}");
    assert_eq!(choice["available"], true, "{v}");
    assert_eq!(choice["level"], "ask", "{v}");
    assert_eq!(choice["level_note"], "custom_only", "{v}");
    assert_eq!(
        choice["custom"],
        json!({"allow_tools": ["look"], "ask_tools": ["change"], "dropped_tools": ["gone", "peek"]}),
        "{v}"
    );
    // The required app is installed and available: ready.
    assert_eq!(v["ready"], true, "{v}");
    // The card says the same.
    let (_, v) = ws_get(&w, ws, &owner, "").await;
    assert_eq!(v["items"][0]["apps"][0]["available"], true, "{v}");
    assert_eq!(v["items"][0]["ready"], true, "{v}");
}

// ---- the updates (C-1112, P6-TPL-3) -------------------------------------------------------------

fn skill_md(description: &str, body: &str) -> Vec<u8> {
    format!("---\ndescription: {description}\n---\n\n{body}\n").into_bytes()
}

/// A template's own skill (C-1110): `name` must equal the directory, `use-it-when` one line.
fn tpl_skill_md(name: &str, use_it_when: &str, body: &str) -> Vec<u8> {
    format!("---\nname: {name}\nuse-it-when: {use_it_when}\n---\n\n{body}\n").into_bytes()
}

/// The updates fixture: instructions, the own skills `watch` and `backup`, the helper file
/// `report.sh` and the memory `notes.md`. `2.0.0` changes the instructions and `watch`, adds
/// `extra` and rewrites the helper; `3.0.0` adds a required input and drops the helper files.
fn upd_files(dir: &str, version: &str) -> Vec<(String, Vec<u8>)> {
    let mut inputs = "  - id: zone\n    label: Zone\n    pattern: \"^[a-z]+$\"\n".to_owned();
    if version == "3.0.0" {
        inputs.push_str("  - id: region\n    label: Region\n    pattern: \"^[a-z]{2}$\"\n");
    }
    let instructions = match version {
        "1.0.0" => "Watch {input.zone}.\n",
        "2.0.0" => "Watch {input.zone} closely.\n",
        _ => "Watch {input.zone} in {input.region}.\n",
    };
    let watch = match version {
        "1.0.0" => ("Something is off.", "Watch body v1."),
        "2.0.0" => ("Something is off.", "Watch body v2."),
        _ => ("Something is off.", "Watch body v3."),
    };
    let mut fs = vec![
        (
            format!("{dir}/template.yaml"),
            format!(
                "format: 1\nid: {dir}\nversion: {version}\nname: T\nsummary: Updates fixture.\n\
                 inputs:\n{inputs}"
            )
            .into_bytes(),
        ),
        (format!("{dir}/agent.yaml"), AGENT_YAML.as_bytes().to_vec()),
        (
            format!("{dir}/instructions.md"),
            instructions.as_bytes().to_vec(),
        ),
        (
            format!("{dir}/README.md"),
            format!("# {dir}\nUpdates.\n").into_bytes(),
        ),
        (
            format!("{dir}/skills/watch/SKILL.md"),
            tpl_skill_md("watch", watch.0, watch.1),
        ),
        (
            format!("{dir}/skills/backup/SKILL.md"),
            tpl_skill_md("backup", "Backing up.", "Backup body."),
        ),
        (
            format!("{dir}/memory/notes.md"),
            b"---\nkey: notes\nupdated: 2026-11-20\n---\nTemplate notes.\n".to_vec(),
        ),
    ];
    match version {
        "1.0.0" => fs.push((
            format!("{dir}/helpers/report.sh"),
            b"#!/bin/sh\necho v1\n".to_vec(),
        )),
        "2.0.0" => fs.push((
            format!("{dir}/helpers/report.sh"),
            b"#!/bin/sh\necho v2\n".to_vec(),
        )),
        _ => {}
    }
    if version != "1.0.0" {
        fs.push((
            format!("{dir}/skills/extra/SKILL.md"),
            tpl_skill_md("extra", "Extra work.", "Extra body."),
        ));
    }
    fs
}

/// A template with `refs` as its `skills:` section (the reference fixtures).
fn ref_files(dir: &str, version: &str, refs: &str) -> Vec<(String, Vec<u8>)> {
    vec![
        (
            format!("{dir}/template.yaml"),
            format!(
                "format: 1\nid: {dir}\nversion: {version}\nname: R\nsummary: Refs.\n\
                 inputs:\n  - id: zone\n    label: Zone\n    pattern: \"^[a-z]+$\"\n{refs}"
            )
            .into_bytes(),
        ),
        (format!("{dir}/agent.yaml"), AGENT_YAML.as_bytes().to_vec()),
        (
            format!("{dir}/instructions.md"),
            INSTRUCTIONS.as_bytes().to_vec(),
        ),
        (
            format!("{dir}/README.md"),
            format!("# {dir}\nRefs.\n").into_bytes(),
        ),
    ]
}

fn pack_tree(dir: &str, fs: &[(String, Vec<u8>)]) -> Vec<u8> {
    let tree: Vec<TreeFile> = fs
        .iter()
        .map(|(p, b)| TreeFile {
            path: p.clone(),
            bytes: b.clone(),
            mode: 0o100644,
        })
        .collect();
    aurora_template::pack(dir, &tree)
}

/// Seed `repo` with the manifest and `fs`, serve the package, check and approve; returns the
/// source id.
async fn seed_fs(w: &W, repo: &str, dir: &str, version: &str, fs: &[(String, Vec<u8>)]) -> Uuid {
    w.git.add_repo(repo);
    let mut changes = vec![put(
        "aurora-templates.yaml".into(),
        manifest_yaml().into_bytes(),
    )];
    for (p, b) in fs {
        changes.push(put(p.clone(), b.clone()));
    }
    w.git.commit(repo, "main", &changes);
    w.git
        .serve_file(&format!("pkg/{dir}-{version}.zip"), pack_tree(dir, fs));
    let src = w.add_source(repo, json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    let r = w.check(id).await;
    assert_eq!(r["status"], "updated", "{r}");
    w.approve(dir, version).await;
    id
}

/// Commit `fs` as one more template `dir` on `repo`'s existing source (a second source for
/// the same repo would be 409 `template_source_exists`), check and approve it.
async fn add_template_to_source(
    w: &W,
    id: Uuid,
    repo: &str,
    dir: &str,
    version: &str,
    fs: &[(String, Vec<u8>)],
) {
    let changes: Vec<Change> = fs.iter().map(|(p, b)| put(p.clone(), b.clone())).collect();
    w.bump(repo, &changes);
    w.git
        .serve_file(&format!("pkg/{dir}-{version}.zip"), pack_tree(dir, fs));
    let r = w.check(id).await;
    assert_eq!(r["status"], "updated", "{r}");
    w.approve(dir, version).await;
}

/// Move `repo`'s head to `version` of `fs` (with `deletions` applied), check and approve it.
async fn advance_fs(
    w: &W,
    id: Uuid,
    repo: &str,
    dir: &str,
    version: &str,
    fs: &[(String, Vec<u8>)],
    deletions: &[String],
) {
    let mut changes: Vec<Change> = fs.iter().map(|(p, b)| put(p.clone(), b.clone())).collect();
    for d in deletions {
        changes.push(Change::DeleteDir(d.clone()));
    }
    w.bump(repo, &changes);
    w.git
        .serve_file(&format!("pkg/{dir}-{version}.zip"), pack_tree(dir, fs));
    let r = w.check(id).await;
    assert_eq!(r["status"], "updated", "{r}");
    w.approve(dir, version).await;
}

impl W {
    /// The plan of `template` in `ws` (its content hash included).
    async fn plan(&self, ws: Uuid, who: &str, template: &str) -> Value {
        let (s, v) = ws_get(self, ws, who, &format!("/{template}")).await;
        assert_eq!(s, 200, "{v}");
        v
    }

    /// Create an agent from `template@version` (the happy path; `extra` carries accounts,
    /// skill_refs and the like).
    async fn create_agent(
        &self,
        ws: Uuid,
        who: &str,
        template: &str,
        version: &str,
        extra: Value,
    ) -> Value {
        let plan = self.plan(ws, who, template).await;
        let body = json!({
            "template_id": template,
            "version": version,
            "content_sha256": plan["content_sha256"],
            "name": "Monitor",
            "inputs": {"zone": "eu"},
        })
        .merge(extra);
        let (s, v) = self
            .h
            .call(
                who,
                Method::POST,
                &format!("/v1/workspaces/{ws}/agents/from-template"),
                Some(body),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        v
    }

    async fn tpl_status(&self, who: &str, agent: Uuid) -> (u16, Value) {
        self.h
            .call(
                who,
                Method::GET,
                &format!("/v1/agents/{agent}/template"),
                None,
            )
            .await
    }

    async fn apply(&self, who: &str, agent: Uuid, body: Value) -> (u16, Value) {
        self.h
            .call(
                who,
                Method::POST,
                &format!("/v1/agents/{agent}/template/apply"),
                Some(body),
            )
            .await
    }
}

/// The agent's brief.
async fn brief_of(w: &W, agent: Uuid) -> String {
    sqlx::query_scalar("SELECT brief FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_one(w.h.pool())
        .await
        .unwrap()
}

/// The agent's stored input values.
async fn inputs_of(w: &W, agent: Uuid) -> Value {
    sqlx::query_scalar("SELECT template_inputs FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_one(w.h.pool())
        .await
        .unwrap()
}

/// The agent's own (agent-tier) skills as `(name, library_id)`, ordered by name.
async fn skills_of(w: &W, agent: Uuid) -> Vec<(String, Option<Uuid>)> {
    sqlx::query_as(
        "SELECT name, library_id FROM skills WHERE agent_id = $1
         ORDER BY name",
    )
    .bind(agent)
    .fetch_all(w.h.pool())
    .await
    .unwrap()
}

/// The agent's recorded template parts as `(kind, key)`, ordered.
async fn parts_of(w: &W, agent: Uuid) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT kind, key FROM agent_template_parts WHERE agent_id = $1 ORDER BY kind, key",
    )
    .bind(agent)
    .fetch_all(w.h.pool())
    .await
    .unwrap()
}

/// The agent's helper file paths.
async fn helpers_of(w: &W, agent: Uuid) -> Vec<String> {
    sqlx::query_scalar("SELECT path FROM agent_helper_files WHERE agent_id = $1 ORDER BY path")
        .bind(agent)
        .fetch_all(w.h.pool())
        .await
        .unwrap()
}

/// The agent's memories as `(key, content, locked)`, ordered by key.
async fn memories_of(w: &W, agent: Uuid) -> Vec<(String, String, bool)> {
    sqlx::query_as("SELECT key, content, locked FROM memories WHERE agent_id = $1 ORDER BY key")
        .bind(agent)
        .fetch_all(w.h.pool())
        .await
        .unwrap()
}

/// The state of one part of an update offer.
fn part_state<'a>(v: &'a Value, kind: &str, key: &str) -> Option<&'a str> {
    v["update"]["parts"]
        .as_array()?
        .iter()
        .find(|p| p["kind"] == kind && p["key"] == key)
        .and_then(|p| p["state"].as_str())
}

/// The `merge` block of one part of an update offer.
fn part_merge<'a>(v: &'a Value, kind: &str, key: &str) -> &'a Value {
    v["update"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == kind && p["key"] == key)
        .unwrap()
        .get("merge")
        .unwrap()
}

/// The world of the update tests: the fixture `t` approved at 1.0.0 and one agent created
/// from it whose own skill `watch` was edited and whose own skill `backup` was deleted.
struct Upd {
    w: W,
    id: Uuid,
    repo: &'static str,
    ws: Uuid,
    owner: String,
    agent: Uuid,
    watch: Uuid,
}

async fn upd_world() -> Upd {
    let w = world().await;
    let (_, ws, owner) = w.ws_owner().await;
    let id = seed_fs(&w, "acme/tpl", "t", "1.0.0", &upd_files("t", "1.0.0")).await;
    let created = w.create_agent(ws, &owner, "t", "1.0.0", json!({})).await;
    let agent: Uuid = created["agent"]["id"].as_str().unwrap().parse().unwrap();
    let watch: Uuid = created["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "watch")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let backup: Uuid = created["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "backup")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    // The agent's own edits: `watch` rewritten, `backup` deleted. Helpers and the brief stay
    // as the template wrote them.
    let (s, v) =
        w.h.call(
            &owner,
            Method::PATCH,
            &format!("/v1/skills/{watch}"),
            Some(json!({"body": "Watch body: mine."})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) =
        w.h.call(
            &owner,
            Method::DELETE,
            &format!("/v1/skills/{backup}"),
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");
    Upd {
        w,
        id,
        repo: "acme/tpl",
        ws,
        owner,
        agent,
        watch,
    }
}

/// Move the fixture to `version`, check and approve it.
async fn upd_advance(u: &Upd, version: &str) {
    let deletions: Vec<String> = if version == "3.0.0" {
        // `3.0.0` drops the helper files from the repository.
        vec!["t/helpers".to_owned()]
    } else {
        Vec::new()
    };
    advance_fs(
        &u.w,
        u.id,
        u.repo,
        "t",
        version,
        &upd_files("t", version),
        &deletions,
    )
    .await;
}

#[tokio::test]
async fn the_offer_lists_every_part_state_for_every_kind() {
    let u = upd_world().await;
    upd_advance(&u, "2.0.0").await;

    let (s, v) = u.w.tpl_status(&u.owner, u.agent).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["available"], true, "{v}");
    assert_eq!(v["template"]["template_id"], "t", "{v}");
    assert_eq!(v["template"]["version"], "1.0.0", "{v}");
    let upd = &v["update"];
    assert_eq!(upd["version"], "2.0.0", "{v}");
    assert!(upd["content_sha256"].is_string(), "{v}");
    assert!(upd["readme"].as_str().unwrap().contains("Updates."), "{v}");
    assert_eq!(upd["changelog"], json!(null), "{v}");
    // Every state, one per kind where possible: the brief is untouched (the template changed
    // it), `watch` was edited here and meets the template's own edit — a conflict part with
    // one hunk (the body; the three-way merge, FIX-251), `backup` deleted here, `extra` is
    // new, the helpers are untouched (the template rewrote them).
    assert_eq!(part_state(&v, "instructions", ""), Some("unchanged_here"));
    assert_eq!(part_state(&v, "skill", "watch"), Some("conflict"));
    let watch = v["update"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "skill" && p["key"] == "watch")
        .unwrap();
    assert_eq!(watch["merge"]["state"], "conflict", "{watch}");
    assert_eq!(watch["merge"]["conflicts"][0]["unit"], "body", "{watch}");
    assert!(watch["merge"]["merge_sha256"].is_string(), "{watch}");
    assert_eq!(part_state(&v, "skill", "backup"), Some("removed_here"));
    assert_eq!(part_state(&v, "skill", "extra"), Some("new"));
    assert_eq!(part_state(&v, "helpers", ""), Some("unchanged_here"));
    // Memories and automations are never offered again.
    assert!(
        upd["parts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["kind"] != "memory" && p["kind"] != "automation")
    );
    // The full texts: the proposed ones are rendered with the stored inputs.
    let instructions = upd["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "instructions")
        .unwrap();
    assert_eq!(instructions["current"], "Watch eu.\n", "{v}");
    assert_eq!(instructions["proposed"], "Watch eu closely.\n", "{v}");
    let watch = upd["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["key"] == "watch")
        .unwrap();
    assert!(
        watch["current"]
            .as_str()
            .unwrap()
            .contains("Watch body: mine."),
        "{v}"
    );
    assert!(
        watch["proposed"]
            .as_str()
            .unwrap()
            .contains("Watch body v2."),
        "{v}"
    );
    assert_eq!(watch["skill_ref"], json!(null), "{v}");
    // Nothing changed yet: the brief, the skills, the helpers and the parts are as they were.
    assert_eq!(brief_of(&u.w, u.agent).await, "Watch eu.\n");
    assert_eq!(
        u.w.count("SELECT count(*) FROM agent_template_parts WHERE agent_id IS NOT NULL")
            .await,
        4
    );
}

#[tokio::test]
async fn applying_a_subset_changes_only_those_parts() {
    let u = upd_world().await;
    upd_advance(&u, "2.0.0").await;
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();

    // Take the instructions, the new skill and the helpers — leave `watch` and `backup` out.
    let (s, v) =
        u.w.apply(
            &u.owner,
            u.agent,
            json!({
                "version": "2.0.0",
                "content_sha256": sha,
                "parts": [
                    {"kind": "instructions", "key": ""},
                    {"kind": "skill", "key": "extra"},
                    {"kind": "helpers", "key": ""},
                ],
            }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["update"], json!(null), "{v}");
    assert_eq!(v["template"]["version"], "2.0.0", "{v}");
    assert_eq!(v["notes"], json!([]), "{v}");

    // The brief and the helper file were replaced; `extra` was created.
    assert_eq!(brief_of(&u.w, u.agent).await, "Watch eu closely.\n");
    let helpers = helpers_of(&u.w, u.agent).await;
    assert_eq!(helpers, vec!["report.sh"], "{helpers:?}");
    let content: Vec<u8> = sqlx::query_scalar(
        "SELECT content FROM agent_helper_files WHERE agent_id = $1 AND path = 'report.sh'",
    )
    .bind(u.agent)
    .fetch_one(u.w.h.pool())
    .await
    .unwrap();
    assert_eq!(content, b"#!/bin/sh\necho v2\n");
    // The edited `watch` was untouched, `backup` stayed deleted.
    let body: String = sqlx::query_scalar("SELECT body FROM skills WHERE id = $1")
        .bind(u.watch)
        .fetch_one(u.w.h.pool())
        .await
        .unwrap();
    assert_eq!(body, "Watch body: mine.");
    assert_eq!(
        skills_of(&u.w, u.agent).await,
        vec![("extra".to_owned(), None), ("watch".to_owned(), None),]
    );
    // The parts: `extra` recorded, the others re-recorded or untouched, `backup`'s row still
    // there (it was not taken).
    assert_eq!(
        parts_of(&u.w, u.agent).await,
        vec![
            ("helpers".to_owned(), String::new()),
            ("instructions".to_owned(), String::new()),
            ("skill".to_owned(), "backup".to_owned()),
            ("skill".to_owned(), "extra".to_owned()),
            ("skill".to_owned(), "watch".to_owned()),
        ]
    );
    // The applied hash is the rendered brief's (the diff base of the next offer).
    let rec: String = sqlx::query_scalar(
        "SELECT content_sha256 FROM agent_template_parts
         WHERE agent_id = $1 AND kind = 'instructions'",
    )
    .bind(u.agent)
    .fetch_one(u.w.h.pool())
    .await
    .unwrap();
    assert_eq!(
        rec,
        aurora_template::instructions_hash("Watch eu closely.\n")
    );
    // The apply is audited (ordinary).
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'agent.template_apply'
         AND NOT critical AND target_id = $1",
    )
    .bind(u.agent.to_string())
    .fetch_one(u.w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn applying_a_skill_keeps_its_enabled_choice() {
    let u = upd_world().await;
    upd_advance(&u, "2.0.0").await;
    // The person switched the agent's own skill off; the offer lists it as `edited_here`.
    sqlx::query("UPDATE skills SET enabled = false WHERE id = $1")
        .bind(u.watch)
        .execute(u.w.h.pool())
        .await
        .unwrap();
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();
    // The body edit meets the template's own body edit: the offer shows one conflict hunk
    // (the body), and the apply echoes the merge's hash and resolves it — Take the new one,
    // which is the template's text; the enabled choice is what this test is about.
    let merge = v["update"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "skill" && p["key"] == "watch")
        .unwrap()["merge"]
        .clone();
    assert_eq!(merge["state"], "conflict", "{merge}");
    assert_eq!(merge["conflicts"][0]["unit"], "body", "{merge}");

    let (s, v) =
        u.w.apply(
            &u.owner,
            u.agent,
            json!({
                "version": "2.0.0",
                "content_sha256": sha,
                "parts": [{
                    "kind": "skill",
                    "key": "watch",
                    "merge_sha256": merge["merge_sha256"],
                    "resolutions": [{"unit": "body", "choice": "take_theirs"}],
                }],
            }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // The template's text was applied, but the skill stays off (SEC-146 E).
    let (enabled, body): (bool, String) =
        sqlx::query_as("SELECT enabled, body FROM skills WHERE id = $1")
            .bind(u.watch)
            .fetch_one(u.w.h.pool())
            .await
            .unwrap();
    assert!(!enabled);
    assert_eq!(body, "\nWatch body v2.\n");
}

/// An edit the new version does not touch merges cleanly and applies without resolutions:
/// the person's line stays and the template's change arrives, and the memories are never
/// touched (FIX-251, D-488).
#[tokio::test]
async fn an_edited_part_merges_and_applies_without_resolutions() {
    let u = upd_world().await;
    // The person appended a line to the brief; the template changed its first line.
    sqlx::query("UPDATE agents SET brief = $2 WHERE id = $1")
        .bind(u.agent)
        .bind("Watch eu.\nAlso feed the cat.\n")
        .execute(u.w.h.pool())
        .await
        .unwrap();
    let memories: i64 = sqlx::query_scalar("SELECT count(*) FROM memories WHERE agent_id = $1")
        .bind(u.agent)
        .fetch_one(u.w.h.pool())
        .await
        .unwrap();
    upd_advance(&u, "2.0.0").await;

    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    assert_eq!(part_state(&v, "instructions", ""), Some("merged"), "{v}");
    let merge = part_merge(&v, "instructions", "");
    assert_eq!(merge["state"], "merged", "{merge}");
    assert_eq!(
        merge["result"], "Watch eu closely.\nAlso feed the cat.\n",
        "{merge}"
    );
    // No conflict hunks (an empty list is omitted altogether).
    assert!(
        merge["conflicts"].is_null() || merge["conflicts"].as_array().unwrap().is_empty(),
        "{merge}"
    );
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();
    let (s, v) =
        u.w.apply(
            &u.owner,
            u.agent,
            json!({
                "version": "2.0.0",
                "content_sha256": sha,
                "parts": [{
                    "kind": "instructions",
                    "key": "",
                    "merge_sha256": merge["merge_sha256"],
                }],
            }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // The merged brief: the template's change and the person's line.
    let brief: String = sqlx::query_scalar("SELECT brief FROM agents WHERE id = $1")
        .bind(u.agent)
        .fetch_one(u.w.h.pool())
        .await
        .unwrap();
    assert_eq!(brief, "Watch eu closely.\nAlso feed the cat.\n", "{brief}");
    // The memories are never changed by an update.
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM memories WHERE agent_id = $1")
        .bind(u.agent)
        .fetch_one(u.w.h.pool())
        .await
        .unwrap();
    assert_eq!(after, memories);
}

/// A conflicting hunk resolved with Keep mine: the person's text stays, and the apply writes
/// exactly what the resolution composed (FIX-251).
#[tokio::test]
async fn a_conflicting_hunk_resolved_with_keep_mine() {
    let u = upd_world().await;
    upd_advance(&u, "2.0.0").await;
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    let merge = part_merge(&v, "skill", "watch").clone();
    assert_eq!(merge["conflicts"][0]["unit"], "body", "{merge}");
    let ours = merge["conflicts"][0]["ours"].as_str().unwrap().to_owned();
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();
    let (s, v) =
        u.w.apply(
            &u.owner,
            u.agent,
            json!({
                "version": "2.0.0",
                "content_sha256": sha,
                "parts": [{
                    "kind": "skill",
                    "key": "watch",
                    "merge_sha256": merge["merge_sha256"],
                    "resolutions": [{"unit": "body", "choice": "keep_mine"}],
                }],
            }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let body: String = sqlx::query_scalar("SELECT body FROM skills WHERE id = $1")
        .bind(u.watch)
        .fetch_one(u.w.h.pool())
        .await
        .unwrap();
    assert_eq!(body, ours, "{body}");
}

/// A merge review that no longer matches is refused with the plain 409: the merge is the
/// server's own, and a stale hash is never silently re-merged (FIX-251).
#[tokio::test]
async fn a_stale_merge_review_is_refused() {
    let u = upd_world().await;
    upd_advance(&u, "2.0.0").await;
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();
    let merge = part_merge(&v, "skill", "watch").clone();
    // The agent's skill moves under the review: the merge the person answered to is gone.
    sqlx::query("UPDATE skills SET body = $2 WHERE id = $1")
        .bind(u.watch)
        .bind("Watch body: changed under the review.")
        .execute(u.w.h.pool())
        .await
        .unwrap();
    let (s, v) =
        u.w.apply(
            &u.owner,
            u.agent,
            json!({
                "version": "2.0.0",
                "content_sha256": sha,
                "parts": [{
                    "kind": "skill",
                    "key": "watch",
                    "merge_sha256": merge["merge_sha256"],
                    "resolutions": [{"unit": "body", "choice": "take_theirs"}],
                }],
            }),
        )
        .await;
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["code"], "template_version_changed", "{v}");
}

/// Resolutions that do not match the offer's conflict hunks are refused, and so is a part
/// applied without its merge hash (FIX-251).
#[tokio::test]
async fn resolutions_that_do_not_match_the_hunks_are_refused() {
    let u = upd_world().await;
    upd_advance(&u, "2.0.0").await;
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();
    let merge = part_merge(&v, "skill", "watch").clone();
    let part = |resolutions: Value| {
        json!({
            "version": "2.0.0",
            "content_sha256": sha,
            "parts": [{
                "kind": "skill",
                "key": "watch",
                "merge_sha256": merge["merge_sha256"],
                "resolutions": resolutions,
            }],
        })
    };
    // A resolution for a hunk the offer does not have.
    let (s, v) =
        u.w.apply(
            &u.owner,
            u.agent,
            part(json!([{"unit": "description", "choice": "take_theirs"}])),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "template_merge_unresolved", "{v}");
    // And a missing resolution for the one hunk it does have.
    let (s, v) = u.w.apply(&u.owner, u.agent, part(json!([]))).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "template_merge_unresolved", "{v}");
    // And no merge hash at all: the review did not come from the offer's merge.
    let (s, v) =
        u.w.apply(
            &u.owner,
            u.agent,
            json!({
                "version": "2.0.0",
                "content_sha256": sha,
                "parts": [{"kind": "skill", "key": "watch"}],
            }),
        )
        .await;
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["code"], "template_version_changed", "{v}");
}

#[tokio::test]
async fn a_needed_input_blocks_the_apply_until_given() {
    let u = upd_world().await;
    upd_advance(&u, "3.0.0").await;
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    // The stored inputs do not cover the new required `region`.
    let needed = v["update"]["inputs_needed"].as_array().unwrap();
    assert_eq!(needed.len(), 1, "{v}");
    assert_eq!(needed[0]["id"], "region", "{v}");
    assert_eq!(needed[0]["required"], true, "{v}");
    // The proposed instructions render the needed input as its placeholder.
    let instructions = v["update"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "instructions")
        .unwrap();
    assert_eq!(
        instructions["proposed"], "Watch eu in {input.region}.\n",
        "{v}"
    );

    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();
    // Without the input the apply is refused, all problems at once.
    let (s, v) =
        u.w.apply(
            &u.owner,
            u.agent,
            json!({
                "version": "3.0.0",
                "content_sha256": sha,
                "parts": [{"kind": "instructions", "key": ""}],
            }),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "template_inputs_invalid"), "{v}");
    assert!(
        v["error"]["details"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == "/inputs/region"),
        "{v}"
    );
    // With it: the brief is rendered with the merged values (the stored `zone` kept) and the
    // inputs are stored. The dropped helpers are taken too.
    let (s, v) =
        u.w.apply(
            &u.owner,
            u.agent,
            json!({
                "version": "3.0.0",
                "content_sha256": sha,
                "inputs": {"region": "eu"},
                "parts": [
                    {"kind": "instructions", "key": ""},
                    {"kind": "helpers", "key": ""},
                ],
            }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(brief_of(&u.w, u.agent).await, "Watch eu in eu.\n");
    assert_eq!(
        inputs_of(&u.w, u.agent).await,
        json!({"zone": "eu", "region": "eu"}),
        "{v}"
    );
    assert_eq!(helpers_of(&u.w, u.agent).await, Vec::<String>::new());
    // The taken removed_upstream part lost its recorded hash.
    assert!(
        !parts_of(&u.w, u.agent)
            .await
            .iter()
            .any(|(k, _)| k == "helpers"),
        "{v}"
    );
}

#[tokio::test]
async fn an_apply_is_bound_to_the_offered_version_and_hash() {
    let u = upd_world().await;
    upd_advance(&u, "2.0.0").await;

    // A stale hash.
    let (s, v) =
        u.w.apply(
            &u.owner,
            u.agent,
            json!({
                "version": "2.0.0",
                "content_sha256": "0".repeat(64),
                "parts": [],
            }),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "template_version_changed"), "{v}");
    // A stale version.
    let (_, st) = u.w.tpl_status(&u.owner, u.agent).await;
    let sha = st["update"]["content_sha256"].as_str().unwrap().to_owned();
    let (s, v) =
        u.w.apply(
            &u.owner,
            u.agent,
            json!({
                "version": "9.9.9",
                "content_sha256": sha,
                "parts": [],
            }),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "template_version_changed"), "{v}");
    // Nothing moved.
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    assert_eq!(v["template"]["version"], "1.0.0", "{v}");
    assert_eq!(brief_of(&u.w, u.agent).await, "Watch eu.\n");
}

#[tokio::test]
async fn a_part_not_in_the_offer_is_refused() {
    let u = upd_world().await;
    upd_advance(&u, "2.0.0").await;
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();

    for parts in [
        json!([{"kind": "skill", "key": "ghost"}]),
        // Memories and automations are never parts.
        json!([{"kind": "memory", "key": "notes.md"}]),
    ] {
        let (s, v) =
            u.w.apply(
                &u.owner,
                u.agent,
                json!({
                    "version": "2.0.0",
                    "content_sha256": sha,
                    "parts": parts,
                }),
            )
            .await;
        assert_eq!((s, code(&v)), (422, "template_part_unknown"), "{v}");
    }
    assert_eq!(brief_of(&u.w, u.agent).await, "Watch eu.\n");
}

#[tokio::test]
async fn a_library_skill_of_the_same_name_cannot_be_replaced() {
    let u = upd_world().await;

    // A library with a skill of the same name, added to the agent after its own `watch` was
    // deleted (the name would collide the other way round).
    u.w.git.add_repo("acme/skills");
    u.w.git.commit(
        "acme/skills",
        "main",
        &[put(
            "watch/SKILL.md".into(),
            skill_md("The library one.", "Library watch body."),
        )],
    );
    let (s, lib) =
        u.w.h
            .call(
                &u.owner,
                Method::POST,
                &format!("/v1/orgs/{}/skill-libraries", {
                    sqlx::query_scalar::<_, Uuid>("SELECT org_id FROM workspaces WHERE id = $1")
                        .bind(u.ws)
                        .fetch_one(u.w.h.pool())
                        .await
                        .unwrap()
                }),
                Some(json!({"label": "lib", "url": format!("{GIT_ORIGIN}/acme/skills")})),
            )
            .await;
    assert_eq!(s, 201, "{lib}");
    let lib: Uuid = lib["id"].as_str().unwrap().parse().unwrap();
    let (s, v) =
        u.w.h
            .call(
                &u.owner,
                Method::DELETE,
                &format!("/v1/skills/{}", u.watch),
                None,
            )
            .await;
    assert_eq!(s, 204, "{v}");
    let head = u.w.git.head("acme/skills", "main");
    let (s, v) =
        u.w.h
            .call(
                &u.owner,
                Method::POST,
                &format!("/v1/agents/{}/skills", u.agent),
                Some(json!({
                    "from_library": {
                        "library_id": lib, "path": "watch", "commit_sha": head,
                    }
                })),
            )
            .await;
    assert_eq!(s, 201, "{v}");
    // The template's part is now shadowed by a library skill of the same name: the offer
    // still lists it (the agent has no own skill of the name), the apply refuses.
    upd_advance(&u, "2.0.0").await;
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    assert_eq!(part_state(&v, "skill", "watch"), Some("removed_here"));
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();
    let (s, v) =
        u.w.apply(
            &u.owner,
            u.agent,
            json!({
                "version": "2.0.0",
                "content_sha256": sha,
                "parts": [{"kind": "skill", "key": "watch"}],
            }),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "library_skill_readonly"), "{v}");
    // The library skill is untouched.
    let body: String =
        sqlx::query_scalar("SELECT body FROM skills WHERE agent_id = $1 AND name = 'watch'")
            .bind(u.agent)
            .fetch_one(u.w.h.pool())
            .await
            .unwrap();
    assert_eq!(body, "Library watch body.\n");
}

#[tokio::test]
async fn skip_this_version_moves_the_version_and_touches_nothing() {
    let u = upd_world().await;
    upd_advance(&u, "2.0.0").await;
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();
    let before_parts = parts_of(&u.w, u.agent).await;
    let before_skills = skills_of(&u.w, u.agent).await;
    let before_helpers = helpers_of(&u.w, u.agent).await;

    let (s, v) =
        u.w.apply(
            &u.owner,
            u.agent,
            json!({"version": "2.0.0", "content_sha256": sha, "parts": []}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["template"]["version"], "2.0.0", "{v}");
    assert_eq!(v["update"], json!(null), "{v}");
    assert_eq!(v["notes"], json!([]), "{v}");
    // Nothing but the version moved.
    assert_eq!(brief_of(&u.w, u.agent).await, "Watch eu.\n");
    assert_eq!(skills_of(&u.w, u.agent).await, before_skills);
    assert_eq!(helpers_of(&u.w, u.agent).await, before_helpers);
    assert_eq!(parts_of(&u.w, u.agent).await, before_parts);
}

#[tokio::test]
async fn detach_forgets_the_provenance_and_stops_the_offers() {
    let u = upd_world().await;
    upd_advance(&u, "2.0.0").await;

    let (s, v) =
        u.w.h
            .call(
                &u.owner,
                Method::DELETE,
                &format!("/v1/agents/{}/template", u.agent),
                None,
            )
            .await;
    assert_eq!(s, 204, "{v}");
    // The provenance is forgotten: the columns are null, the parts are gone.
    let (tid, ver): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT template_id, template_version FROM agents WHERE id = $1")
            .bind(u.agent)
            .fetch_one(u.w.h.pool())
            .await
            .unwrap();
    assert_eq!(tid, None);
    assert_eq!(ver, None);
    assert_eq!(
        parts_of(&u.w, u.agent).await,
        Vec::<(String, String)>::new()
    );
    // Helper files and the agent's skills stay.
    assert_eq!(
        helpers_of(&u.w, u.agent).await,
        vec!["report.sh".to_owned()]
    );
    assert_eq!(skills_of(&u.w, u.agent).await.len(), 1);
    // No template section any more, and no offer even though 2.0.0 is offered.
    let (s, v) = u.w.tpl_status(&u.owner, u.agent).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["template"], json!(null), "{v}");
    assert_eq!(v["available"], false, "{v}");
    assert_eq!(v["update"], json!(null), "{v}");
    // The detach is audited (ordinary).
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'agent.template_detach'
         AND NOT critical AND target_id = $1",
    )
    .bind(u.agent.to_string())
    .fetch_one(u.w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1);
}

// ---- the per-agent writer lock (SEC-146 A) -----------------------------------------------------

/// The request of a spawned task: a clone of the app router and the cookie (`Harness::call`
/// for a task the harness itself cannot move into).
async fn app_call(
    app: axum::Router,
    cookie: String,
    m: Method,
    uri: String,
    body: Option<Value>,
) -> (u16, Value) {
    use http_body_util::BodyExt;
    use tower::ServiceExt as _;
    let mut b = http::Request::builder().method(m).uri(uri);
    b = b.header("cookie", cookie).header("origin", ORIGIN);
    let req = match body {
        Some(v) => b
            .header("content-type", "application/json")
            .body(axum::body::Body::from(v.to_string())),
        None => b.body(axum::body::Body::empty()),
    }
    .unwrap();
    let res = app.oneshot(req).await.unwrap();
    let (parts, body) = res.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    (
        parts.status.as_u16(),
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Hold the agent's advisory writer lock (`repo::memory::lock_agent`) inside an explicit
/// transaction: the tests below hold it, spawn the request, move the state under it and
/// release, so the interleaving of two writers is deterministic.
async fn hold_agent_lock(
    pool: &sqlx::PgPool,
    agent: Uuid,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut tx = pool.begin().await.unwrap();
    aurora_db::repo::memory::lock_agent(&mut tx, agent)
        .await
        .unwrap();
    tx
}

/// Wait until a session of this test's database waits for a lock: the spawned request is
/// blocked at the writer lock inside its transaction (`SEC-146 A/B`). Transaction-level waits
/// (`transactionid`) carry no database in `pg_locks`, so the waiting session scopes them.
async fn wait_blocked(pool: &sqlx::PgPool) {
    for _ in 0..250 {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_locks l
             JOIN pg_stat_activity a ON a.pid = l.pid
             WHERE NOT l.granted AND a.datname = current_database()",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        if n > 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the request was never blocked on the agent lock");
}

/// The apply request body for the instructions part of the fixture's offer.
fn apply_instructions(sha: &str) -> Value {
    json!({
        "version": "2.0.0",
        "content_sha256": sha,
        "parts": [{"kind": "instructions", "key": ""}],
    })
}

#[tokio::test]
async fn an_apply_rechecks_the_offer_under_the_agent_lock_after_a_withdrawal() {
    let u = upd_world().await;
    upd_advance(&u, "2.0.0").await;
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();
    let brief_before = brief_of(&u.w, u.agent).await;
    let agent = u.agent;
    let pool = u.w.h.pool().clone();
    let app = u.w.h.app.clone();

    let lock = hold_agent_lock(&pool, agent).await;
    let task = tokio::spawn(app_call(
        app,
        u.owner.clone(),
        Method::POST,
        format!("/v1/agents/{agent}/template/apply"),
        Some(apply_instructions(&sha)),
    ));
    wait_blocked(&pool).await;

    // The offer is withdrawn while the apply waits behind the lock.
    sqlx::query("UPDATE agent_templates SET offered = false")
        .execute(&pool)
        .await
        .unwrap();
    lock.rollback().await.unwrap();

    let (s, v) = task.await.unwrap();
    assert_eq!((s, code(&v)), (409, "template_version_changed"), "{v}");
    // Nothing was applied: the brief, the version and the recorded parts are as they were.
    assert_eq!(brief_of(&u.w, u.agent).await, brief_before);
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    assert_eq!(v["template"]["version"], "1.0.0", "{v}");
    assert_eq!(
        parts_of(&u.w, agent).await.len(),
        4,
        "no part row was written"
    );
}

#[tokio::test]
async fn an_apply_refuses_when_a_listed_part_changed_under_the_lock() {
    let u = upd_world().await;
    upd_advance(&u, "2.0.0").await;
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();
    let agent = u.agent;
    let pool = u.w.h.pool().clone();
    let app = u.w.h.app.clone();

    let lock = hold_agent_lock(&pool, agent).await;
    let task = tokio::spawn(app_call(
        app,
        u.owner.clone(),
        Method::POST,
        format!("/v1/agents/{agent}/template/apply"),
        Some(apply_instructions(&sha)),
    ));
    wait_blocked(&pool).await;

    // The brief (the listed `unchanged_here` part) is edited while the apply waits: the
    // recomputed state under the lock is `edited_here`, not what the person answered to.
    sqlx::query("UPDATE agents SET brief = 'Watch eu: raced.' WHERE id = $1")
        .bind(agent)
        .execute(&pool)
        .await
        .unwrap();
    lock.rollback().await.unwrap();

    let (s, v) = task.await.unwrap();
    assert_eq!((s, code(&v)), (409, "template_version_changed"), "{v}");
    // The edit was not overwritten with the template's text.
    assert_eq!(brief_of(&u.w, u.agent).await, "Watch eu: raced.");
}

#[tokio::test]
async fn a_second_apply_of_the_same_version_does_not_duplicate_the_parts() {
    let u = upd_world().await;
    upd_advance(&u, "2.0.0").await;
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();
    let proposed = "Watch eu closely.\n";
    let agent = u.agent;
    let pool = u.w.h.pool().clone();
    let app = u.w.h.app.clone();

    let lock = hold_agent_lock(&pool, agent).await;
    let task = tokio::spawn(app_call(
        app,
        u.owner.clone(),
        Method::POST,
        format!("/v1/agents/{agent}/template/apply"),
        Some(apply_instructions(&sha)),
    ));
    wait_blocked(&pool).await;

    // The first apply commits while this one waits behind the lock: the part is written and
    // the version moves. The second apply must refuse, not write the part again.
    sqlx::query("UPDATE agents SET brief = $2, template_version = '2.0.0' WHERE id = $1")
        .bind(agent)
        .bind(proposed)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE agent_template_parts SET content_sha256 = $2
         WHERE agent_id = $1 AND kind = 'instructions'",
    )
    .bind(agent)
    .bind(aurora_template::instructions_hash(proposed))
    .execute(&pool)
    .await
    .unwrap();
    lock.rollback().await.unwrap();

    let (s, v) = task.await.unwrap();
    assert_eq!((s, code(&v)), (409, "template_version_changed"), "{v}");
    // One part row, and the applied text is the first apply's.
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM agent_template_parts
         WHERE agent_id = $1 AND kind = 'instructions'",
    )
    .bind(agent)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
    assert_eq!(brief_of(&u.w, agent).await, proposed);
}

/// Applying after the agent was renamed: the offer renders the proposed texts with the
/// agent's **current** name (the recorded hashes were made with the name at create time), so
/// a rename alone puts every part that says `{agent.name}` in the next offer — the diff is
/// the name swap — and the apply re-records it with the new name. Memories are never
/// offered: the stored one keeps the name it was created with.
#[tokio::test]
async fn an_apply_after_a_rename_renders_with_the_new_name() {
    let w = world().await;
    let (_, ws, owner) = w.ws_owner().await;
    // The instructions (and the memory) carry the agent's name.
    let mut fs = upd_files("t", "1.0.0");
    fs[2] = (
        "t/instructions.md".into(),
        b"Watch {input.zone} as {agent.name}.\n".to_vec(),
    );
    fs[6] = (
        "t/memory/notes.md".into(),
        b"---\nkey: notes\nupdated: 2026-11-20\n---\nNotes for {agent.name}.\n".to_vec(),
    );
    let id = seed_fs(&w, "acme/tpl", "t", "1.0.0", &fs).await;
    let created = w.create_agent(ws, &owner, "t", "1.0.0", json!({})).await;
    let agent: Uuid = created["agent"]["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(brief_of(&w, agent).await, "Watch eu as Monitor.\n");

    // Rename the agent; nothing else changes.
    let (s, v) =
        w.h.call(
            &owner,
            Method::PATCH,
            &format!("/v1/agents/{agent}"),
            Some(json!({"name": "Rex"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    // 2.0.0 keeps the instructions byte-for-byte: the only difference the offer can show is
    // the name.
    let mut fs2 = upd_files("t", "2.0.0");
    fs2[2] = (
        "t/instructions.md".into(),
        b"Watch {input.zone} as {agent.name}.\n".to_vec(),
    );
    advance_fs(&w, id, "acme/tpl", "t", "2.0.0", &fs2, &[]).await;

    let (_, v) = w.tpl_status(&owner, agent).await;
    assert_eq!(
        part_state(&v, "instructions", ""),
        Some("unchanged_here"),
        "{v}"
    );
    let instructions = v["update"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "instructions")
        .unwrap();
    assert_eq!(instructions["current"], "Watch eu as Monitor.\n", "{v}");
    assert_eq!(instructions["proposed"], "Watch eu as Rex.\n", "{v}");

    // Applying takes the new name; the memory keeps the one it was created with.
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();
    let (s, v) = w
        .apply(
            &owner,
            agent,
            json!({
                "version": "2.0.0",
                "content_sha256": sha,
                "parts": [{"kind": "instructions", "key": ""}],
            }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(brief_of(&w, agent).await, "Watch eu as Rex.\n");
    assert_eq!(
        memories_of(&w, agent).await,
        vec![("notes".into(), "Notes for Monitor.\n".into(), false)]
    );
}

/// Applying never touches the agent's memories — a locked one keeps its lock and its
/// content — and the apply succeeds while the agent is at the memory cap: the cap is about
/// memory writes, and the apply makes none.
#[tokio::test]
async fn an_apply_leaves_the_memories_alone_locked_or_at_the_cap() {
    let u = upd_world().await;
    // Lock the template's memory (C-1151).
    let (s, v) =
        u.w.h
            .call(
                &u.owner,
                Method::PUT,
                &format!("/v1/agents/{}/memories/notes/locked", u.agent),
                Some(json!({"locked": true})),
            )
            .await;
    assert_eq!(s, 200, "{v}");
    // Fill the agent to the memory cap (100 per agent; the SQL write is the cap's own
    // business — the point is that the apply starts from a full agent).
    sqlx::query(
        "INSERT INTO memories (id, agent_id, key, content, revision)
         SELECT gen_random_uuid(), $1, 'm' || g, 'filler', 0 FROM generate_series(0, 98) g",
    )
    .bind(u.agent)
    .execute(u.w.h.pool())
    .await
    .unwrap();
    let before = memories_of(&u.w, u.agent).await;
    assert_eq!(before.len(), 100);

    upd_advance(&u, "2.0.0").await;
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();
    let (s, v) =
        u.w.apply(
            &u.owner,
            u.agent,
            json!({
                "version": "2.0.0",
                "content_sha256": sha,
                "parts": [
                    {"kind": "instructions", "key": ""},
                    {"kind": "skill", "key": "extra"},
                    {"kind": "helpers", "key": ""},
                ],
            }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // The memories are exactly as they were: same rows, same content, `notes` still locked.
    assert_eq!(memories_of(&u.w, u.agent).await, before);
}

/// The helper totals cap is about the set an apply writes (the version's, which the template
/// validator keeps within it), never about the agent's current one: an agent whose helper
/// files sit past the cap can still apply, and the apply replaces the whole set.
#[tokio::test]
async fn an_apply_at_the_helper_totals_cap() {
    let u = upd_world().await;
    // 8 × 256 KiB is exactly the 2 MiB an agent's helper files may total — with the
    // template's report.sh the agent's set is over it, as a cap-sitting agent's would be.
    let chunk = vec![b'x'; 256 * 1024];
    sqlx::query(
        "INSERT INTO agent_helper_files (agent_id, path, content, executable, sha256, size)
         SELECT $1, 'big' || g || '.sh', $2, false, '0', $3 FROM generate_series(0, 7) g",
    )
    .bind(u.agent)
    .bind(&chunk)
    .bind(chunk.len() as i32)
    .execute(u.w.h.pool())
    .await
    .unwrap();
    assert_eq!(helpers_of(&u.w, u.agent).await.len(), 9);
    // The agent's set is the recorded one (the part's hash says so): the helpers are
    // `unchanged_here` and the apply takes the version's set wholesale — the cap judges the
    // set an apply writes (the version's), never the agent's current one. (A set the agent
    // edited is offered as a three-way merge instead, and the merged set respects the cap.)
    let live: Vec<(String, Vec<u8>, bool)> = sqlx::query_as(
        "SELECT path, content, executable FROM agent_helper_files
         WHERE agent_id = $1 ORDER BY path",
    )
    .bind(u.agent)
    .fetch_all(u.w.h.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE agent_template_parts SET content_sha256 = $2
         WHERE agent_id = $1 AND kind = 'helpers'",
    )
    .bind(u.agent)
    .bind(aurora_template::helpers_hash(&live))
    .execute(u.w.h.pool())
    .await
    .unwrap();

    upd_advance(&u, "2.0.0").await;
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();

    // The apply takes the helpers past the agent's current total: the cap judges the
    // version's set (one small file), and the whole set is replaced by it.
    let (s, v) =
        u.w.apply(
            &u.owner,
            u.agent,
            json!({
                "version": "2.0.0",
                "content_sha256": sha,
                "parts": [{"kind": "helpers", "key": ""}],
            }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let helpers = helpers_of(&u.w, u.agent).await;
    assert_eq!(helpers, vec!["report.sh"], "{helpers:?}");
    let content: Vec<u8> = sqlx::query_scalar(
        "SELECT content FROM agent_helper_files WHERE agent_id = $1 AND path = 'report.sh'",
    )
    .bind(u.agent)
    .fetch_one(u.w.h.pool())
    .await
    .unwrap();
    assert_eq!(content, b"#!/bin/sh\necho v2\n");
}

/// Deleting a source: its templates and versions are gone (the open decision item with
/// them), while the agents made from them keep everything they were given — the provenance
/// is text, so it stays and answers `available: false` with no offer.
#[tokio::test]
async fn deleting_a_source_frees_the_agents_and_closes_their_decision_items() {
    let w = world().await;
    let (_, ws, owner) = w.ws_owner().await;
    let id = seed_fs(&w, "acme/tpl", "t", "1.0.0", &upd_files("t", "1.0.0")).await;
    let created = w.create_agent(ws, &owner, "t", "1.0.0", json!({})).await;
    let agent: Uuid = created["agent"]["id"].as_str().unwrap().parse().unwrap();

    // A version waiting for review is a decision item: one notice per admin, and an open
    // item in the attention list. Bump and check, and leave 2.0.0 unreviewed.
    let fs2 = upd_files("t", "2.0.0");
    let changes: Vec<Change> = fs2.iter().map(|(p, b)| put(p.clone(), b.clone())).collect();
    w.bump("acme/tpl", &changes);
    w.git.serve_file("pkg/t-2.0.0.zip", pack_tree("t", &fs2));
    let r = w.check(id).await;
    assert_eq!(r["ingested"][0]["status"], "pending_approval", "{r}");
    let (s, att) = w.call(Method::GET, "/v1/admin/attention", None).await;
    assert_eq!(s, 200, "{att}");
    assert!(
        att["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["kind"] == "agent_template_version"),
        "{att}"
    );
    let notices: i64 = w
        .count("SELECT count(*) FROM admin_attention_notices WHERE kind = 'agent_template_version'")
        .await;
    assert!(notices >= 1, "one notice per instance admin: {notices}");

    // Delete the source.
    let (s, v) = w
        .call(
            Method::DELETE,
            &format!("/v1/admin/agent-template-sources/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");
    assert_eq!(
        w.count("SELECT count(*) FROM agent_template_sources").await,
        0
    );
    assert_eq!(w.count("SELECT count(*) FROM agent_templates").await, 0);
    assert_eq!(
        w.count("SELECT count(*) FROM agent_template_versions")
            .await,
        0
    );

    // The agent keeps everything it was given, and its provenance (text) answers.
    assert_eq!(brief_of(&w, agent).await, "Watch eu.\n");
    assert_eq!(skills_of(&w, agent).await.len(), 2);
    assert_eq!(helpers_of(&w, agent).await, vec!["report.sh"]);
    assert_eq!(
        parts_of(&w, agent).await.len(),
        4,
        "the recorded parts stay"
    );
    let (s, v) = w.tpl_status(&owner, agent).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["template"]["template_id"], "t", "{v}");
    assert_eq!(v["template"]["version"], "1.0.0", "{v}");
    assert_eq!(v["available"], false, "{v}");
    assert_eq!(v["update"], json!(null), "{v}");

    // The decision item is gone with the version. The once-only notice row stays: it is the
    // e-mail registry (one row per decision item and admin), not an open item.
    let (s, att) = w.call(Method::GET, "/v1/admin/attention", None).await;
    assert_eq!(s, 200, "{att}");
    assert!(
        !att["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["kind"] == "agent_template_version"),
        "{att}"
    );
    assert_eq!(
        w.count(
            "SELECT count(*) FROM admin_attention_notices WHERE kind = 'agent_template_version'"
        )
        .await,
        notices
    );
}

#[tokio::test]
async fn concurrent_detaches_serialize_and_the_second_is_a_404() {
    let u = upd_world().await;
    let agent = u.agent;
    let pool = u.w.h.pool().clone();
    let app = u.w.h.app.clone();

    let lock = hold_agent_lock(&pool, agent).await;
    let uri = format!("/v1/agents/{agent}/template");
    let t1 = tokio::spawn(app_call(
        app.clone(),
        u.owner.clone(),
        Method::DELETE,
        uri.clone(),
        None,
    ));
    let t2 = tokio::spawn(app_call(app, u.owner.clone(), Method::DELETE, uri, None));
    wait_blocked(&pool).await;
    lock.rollback().await.unwrap();

    // The lock serializes them: exactly one detach succeeds, the other finds the provenance
    // gone (either in its pre-check or in its re-read under the lock).
    let (s1, v1) = t1.await.unwrap();
    let (s2, v2) = t2.await.unwrap();
    assert!(
        (s1 == 204 && s2 == 404) || (s1 == 404 && s2 == 204),
        "{s1} {v1} / {s2} {v2}"
    );
    // One audit row, and the provenance is gone either way.
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'agent.template_detach'
         AND NOT critical AND target_id = $1",
    )
    .bind(agent.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
    let (tid, ver): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT template_id, template_version FROM agents WHERE id = $1")
            .bind(agent)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((tid, ver), (None, None));
}

#[tokio::test]
async fn the_agent_is_unchanged_while_an_update_waits() {
    let u = upd_world().await;
    let brief = brief_of(&u.w, u.agent).await;
    let skills = skills_of(&u.w, u.agent).await;
    let helpers = helpers_of(&u.w, u.agent).await;
    let parts = parts_of(&u.w, u.agent).await;

    // A newer version is approved: the agent runs its own content.
    upd_advance(&u, "2.0.0").await;
    let (s, v) = u.w.tpl_status(&u.owner, u.agent).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["update"]["version"], "2.0.0", "{v}");
    assert_eq!(brief_of(&u.w, u.agent).await, brief);
    assert_eq!(skills_of(&u.w, u.agent).await, skills);
    assert_eq!(helpers_of(&u.w, u.agent).await, helpers);
    assert_eq!(parts_of(&u.w, u.agent).await, parts);

    // The template is withdrawn: still nothing changes; the offer is gone.
    let (s, v) =
        u.w.call(
            Method::PATCH,
            "/v1/admin/agent-templates/t",
            Some(json!({"offered": false})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    assert_eq!(v["available"], false, "{v}");
    assert_eq!(v["update"], json!(null), "{v}");
    assert_eq!(brief_of(&u.w, u.agent).await, brief);
    assert_eq!(parts_of(&u.w, u.agent).await, parts);

    // The source is deleted: the agent keeps its provenance and its content.
    let (s, v) =
        u.w.call(
            Method::DELETE,
            &format!("/v1/admin/agent-template-sources/{}", u.id),
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    assert_eq!(v["available"], false, "{v}");
    assert_eq!(v["template"]["template_id"], "t", "{v}");
    assert_eq!(brief_of(&u.w, u.agent).await, brief);
    assert_eq!(skills_of(&u.w, u.agent).await, skills);
    assert_eq!(helpers_of(&u.w, u.agent).await, helpers);
    assert_eq!(parts_of(&u.w, u.agent).await, parts);
}

#[tokio::test]
async fn a_source_deleted_and_added_again_finds_the_agent_by_template_id() {
    let mut u = upd_world().await;
    let (s, v) =
        u.w.call(
            Method::DELETE,
            &format!("/v1/admin/agent-template-sources/{}", u.id),
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");
    let (_, v) = u.w.tpl_status(&u.owner, u.agent).await;
    assert_eq!(v["available"], false, "{v}");

    // The source comes back (a fresh row, the same template id) and offers 2.0.0: the agent
    // is found by the stored template id text.
    u.repo = "acme/tpl2";
    u.id = seed_fs(&u.w, u.repo, "t", "1.0.0", &upd_files("t", "1.0.0")).await;
    upd_advance(&u, "2.0.0").await;
    let (s, v) = u.w.tpl_status(&u.owner, u.agent).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["available"], true, "{v}");
    assert_eq!(v["update"]["version"], "2.0.0", "{v}");
}

// ---- the reference updates (C-1112's library_skill parts) ---------------------------------------

/// The skill library with `alpha` and `beta`, added to `org`; returns its id.
async fn seed_library(w: &W, org: Uuid, owner: &str) -> Uuid {
    w.git.add_repo("acme/skills");
    w.git.commit(
        "acme/skills",
        "main",
        &[
            put(
                "alpha/SKILL.md".into(),
                skill_md("Alpha skill", "Alpha body."),
            ),
            put("beta/SKILL.md".into(), skill_md("Beta skill", "Beta body.")),
        ],
    );
    let (s, lib) =
        w.h.call(
            owner,
            Method::POST,
            &format!("/v1/orgs/{org}/skill-libraries"),
            Some(json!({"label": "lib", "url": format!("{GIT_ORIGIN}/acme/skills")})),
        )
        .await;
    assert_eq!(s, 201, "{lib}");
    lib["id"].as_str().unwrap().parse().unwrap()
}

#[tokio::test]
async fn a_removed_reference_deletes_the_library_skill_only_while_sourced() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;
    let lib = seed_library(&w, org, &owner).await;

    // v1 references `alpha`; two agents take it.
    let refs = format!(
        "skills:\n  - library: {GIT_ORIGIN}/acme/skills\n    skill: alpha\n    why: Does the work.\n"
    );
    let id = seed_fs(
        &w,
        "acme/tpl",
        "ref1",
        "1.0.0",
        &ref_files("ref1", "1.0.0", &refs),
    )
    .await;
    let plan = w.plan(ws, &owner, "ref1").await;
    let item = json!([{
        "index": 0,
        "library_id": plan["skill_refs"][0]["library"]["id"],
        "commit_sha": plan["skill_refs"][0]["library"]["commit_sha"],
    }]);
    let a = w
        .create_agent(ws, &owner, "ref1", "1.0.0", json!({"skill_refs": item}))
        .await;
    let agent_a: Uuid = a["agent"]["id"].as_str().unwrap().parse().unwrap();
    let b = w
        .create_agent(ws, &owner, "ref1", "1.0.0", json!({"skill_refs": item}))
        .await;
    let agent_b: Uuid = b["agent"]["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        skills_of(&w, agent_a).await,
        vec![("alpha".to_owned(), Some(lib))]
    );

    // v2 drops the reference: `removed_upstream` for both agents.
    advance_fs(
        &w,
        id,
        "acme/tpl",
        "ref1",
        "2.0.0",
        &ref_files("ref1", "2.0.0", ""),
        &[],
    )
    .await;
    let (_, v) = w.tpl_status(&owner, agent_a).await;
    let key = v["update"]["parts"][0]["key"].as_str().unwrap().to_owned();
    assert_eq!(v["update"]["parts"][0]["kind"], "library_skill", "{v}");
    assert_eq!(v["update"]["parts"][0]["state"], "removed_upstream", "{v}");
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();

    // Agent A takes it: the library skill goes.
    let (s, v) = w
        .apply(
            &owner,
            agent_a,
            json!({
                "version": "2.0.0",
                "content_sha256": sha,
                "parts": [{"kind": "library_skill", "key": key}],
            }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["notes"], json!([]), "{v}");
    assert_eq!(
        skills_of(&w, agent_a).await,
        Vec::<(String, Option<Uuid>)>::new()
    );
    // The recorded part is gone with it.
    assert_eq!(parts_of(&w, agent_a).await.len(), 1); // instructions only

    // Agent B first detaches the skill from its library: no longer sourced from the library
    // the template referenced, it stays (`skill_kept`).
    let skill_b: Uuid = b["skills"][0]["id"].as_str().unwrap().parse().unwrap();
    let (s, v) =
        w.h.call(
            &owner,
            Method::POST,
            &format!("/v1/skills/{skill_b}/library/detach"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .apply(
            &owner,
            agent_b,
            json!({
                "version": "2.0.0",
                "content_sha256": sha,
                "parts": [{"kind": "library_skill", "key": key}],
            }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let notes = v["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1, "{v}");
    assert_eq!(notes[0]["code"], "skill_kept", "{v}");
    assert_eq!(
        skills_of(&w, agent_b).await,
        vec![("alpha".to_owned(), None)]
    );
}

#[tokio::test]
async fn a_new_library_reference_needs_its_skill_refs_item() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;
    let lib = seed_library(&w, org, &owner).await;

    // v1 has no references; v2 adds `beta`.
    let id = seed_fs(
        &w,
        "acme/tpl",
        "ref2",
        "1.0.0",
        &ref_files("ref2", "1.0.0", ""),
    )
    .await;
    let created = w.create_agent(ws, &owner, "ref2", "1.0.0", json!({})).await;
    let agent: Uuid = created["agent"]["id"].as_str().unwrap().parse().unwrap();

    let refs = format!(
        "skills:\n  - library: {GIT_ORIGIN}/acme/skills\n    skill: beta\n    why: Does the other work.\n"
    );
    advance_fs(
        &w,
        id,
        "acme/tpl",
        "ref2",
        "2.0.0",
        &ref_files("ref2", "2.0.0", &refs),
        &[],
    )
    .await;
    let (_, v) = w.tpl_status(&owner, agent).await;
    let part = &v["update"]["parts"][0];
    assert_eq!(part["kind"], "library_skill", "{v}");
    assert_eq!(part["state"], "new", "{v}");
    assert_eq!(part["skill_ref"]["name"], "beta", "{v}");
    assert_eq!(part["skill_ref"]["available"], true, "{v}");
    let key = part["key"].as_str().unwrap().to_owned();
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();

    // Without the skill_refs item the part cannot be taken.
    let (s, v) = w
        .apply(
            &owner,
            agent,
            json!({
                "version": "2.0.0",
                "content_sha256": sha,
                "parts": [{"kind": "library_skill", "key": key}],
            }),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "template_skill_missing"), "{v}");
    // With it: the library skill is added pinned and read-only.
    let plan = w.plan(ws, &owner, "ref2").await;
    let (s, v) = w
        .apply(
            &owner,
            agent,
            json!({
                "version": "2.0.0",
                "content_sha256": sha,
                "skill_refs": [{
                    "index": 0,
                    "library_id": plan["skill_refs"][0]["library"]["id"],
                    "commit_sha": plan["skill_refs"][0]["library"]["commit_sha"],
                }],
                "parts": [{"kind": "library_skill", "key": key}],
            }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        skills_of(&w, agent).await,
        vec![("beta".to_owned(), Some(lib))]
    );
    // Pinned to the commit the person's item named, linked to the library.
    let (pinned, linked): (String, Option<Uuid>) = sqlx::query_as(
        "SELECT library_commit_sha, library_id FROM skills
         WHERE agent_id = $1 AND name = 'beta'",
    )
    .bind(agent)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(pinned, plan["skill_refs"][0]["library"]["commit_sha"]);
    assert_eq!(linked, Some(lib));
    // The part is recorded with the reference key's hash (the crate's rule).
    let rec: String = sqlx::query_scalar(
        "SELECT content_sha256 FROM agent_template_parts
         WHERE agent_id = $1 AND kind = 'library_skill'",
    )
    .bind(agent)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(key.as_bytes());
    assert_eq!(rec, hex::encode(h.finalize()));
}

#[tokio::test]
async fn a_create_refuses_a_library_commit_the_plan_did_not_resolve() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;
    seed_library(&w, org, &owner).await;
    // Move the library's head twice: the plan resolves at the new head, so the intermediate
    // commit is a valid but wrong pin.
    let stale = w.git.commit(
        "acme/skills",
        "main",
        &[put(
            "alpha/SKILL.md".into(),
            skill_md("Alpha skill", "Alpha body v2."),
        )],
    );
    w.git.commit(
        "acme/skills",
        "main",
        &[put(
            "alpha/SKILL.md".into(),
            skill_md("Alpha skill", "Alpha body v3."),
        )],
    );
    let refs = format!(
        "skills:\n  - library: {GIT_ORIGIN}/acme/skills\n    skill: beta\n    why: Does the other work.\n"
    );
    seed_fs(
        &w,
        "acme/tpl",
        "ref1",
        "1.0.0",
        &ref_files("ref1", "1.0.0", &refs),
    )
    .await;
    let plan = w.plan(ws, &owner, "ref1").await;
    assert_eq!(plan["skill_refs"][0]["available"], true, "{plan}");
    assert_ne!(
        plan["skill_refs"][0]["library"]["commit_sha"], stale,
        "{plan}"
    );
    let lib_id = plan["skill_refs"][0]["library"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // A body-chosen commit — even a valid one — is not what the plan resolved: 422 at the
    // reference's index, nothing created.
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            json!({
                "template_id": "ref1", "version": "1.0.0",
                "content_sha256": plan["content_sha256"],
                "name": "Monitor", "inputs": {"zone": "eu"},
                "skill_refs": [{
                    "index": 0,
                    "library_id": plan["skill_refs"][0]["library"]["id"],
                    "commit_sha": stale,
                }],
            }),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "template_commit_changed"), "{v}");
    assert_eq!(
        v["error"]["details"]["fields"][0]["path"], "/skill_refs",
        "{v}"
    );
    assert_eq!(w.count("SELECT count(*) FROM agents").await, 0);

    // A reference the plan no longer offers (its library is gone) is refused too.
    let (s, v) =
        w.h.call(
            &owner,
            Method::DELETE,
            &format!("/v1/skill-libraries/{lib_id}"),
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");
    let plan = w.plan(ws, &owner, "ref1").await;
    assert_eq!(plan["skill_refs"][0]["available"], false, "{plan}");
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            json!({
                "template_id": "ref1", "version": "1.0.0",
                "content_sha256": plan["content_sha256"],
                "name": "Monitor", "inputs": {"zone": "eu"},
                "skill_refs": [{"index": 0, "library_id": lib_id, "commit_sha": stale}],
            }),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "template_skill_unavailable"), "{v}");
    assert_eq!(w.count("SELECT count(*) FROM agents").await, 0);
}

#[tokio::test]
async fn an_apply_refuses_a_library_commit_the_offer_did_not_resolve() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;
    seed_library(&w, org, &owner).await;
    let stale = w.git.commit(
        "acme/skills",
        "main",
        &[put(
            "alpha/SKILL.md".into(),
            skill_md("Alpha skill", "Alpha body v2."),
        )],
    );
    // The offer resolves at the current head, so the intermediate commit is a valid but wrong
    // pin for it.
    w.git.commit(
        "acme/skills",
        "main",
        &[put(
            "alpha/SKILL.md".into(),
            skill_md("Alpha skill", "Alpha body v3."),
        )],
    );

    // v1 has no references; v2 adds `beta`.
    let id = seed_fs(
        &w,
        "acme/tpl",
        "ref2",
        "1.0.0",
        &ref_files("ref2", "1.0.0", ""),
    )
    .await;
    let created = w.create_agent(ws, &owner, "ref2", "1.0.0", json!({})).await;
    let agent: Uuid = created["agent"]["id"].as_str().unwrap().parse().unwrap();
    let refs = format!(
        "skills:\n  - library: {GIT_ORIGIN}/acme/skills\n    skill: beta\n    why: Does the other work.\n"
    );
    advance_fs(
        &w,
        id,
        "acme/tpl",
        "ref2",
        "2.0.0",
        &ref_files("ref2", "2.0.0", &refs),
        &[],
    )
    .await;
    let (_, v) = w.tpl_status(&owner, agent).await;
    let key = v["update"]["parts"][0]["key"].as_str().unwrap().to_owned();
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();

    // A body-chosen commit — even a valid one — is not what the offer resolved: 422.
    let (s, v) = w
        .apply(
            &owner,
            agent,
            json!({
                "version": "2.0.0",
                "content_sha256": sha,
                "skill_refs": [{
                    "index": 0,
                    "library_id": v["update"]["parts"][0]["skill_ref"]["library"]["id"],
                    "commit_sha": stale,
                }],
                "parts": [{"kind": "library_skill", "key": key}],
            }),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "template_commit_changed"), "{v}");
    // The skill was not added.
    assert_eq!(skills_of(&w, agent).await.len(), 0);

    // With the offer's commit the part is taken (the happy path stands).
    let plan = w.plan(ws, &owner, "ref2").await;
    let (s, v) = w
        .apply(
            &owner,
            agent,
            json!({
                "version": "2.0.0",
                "content_sha256": sha,
                "skill_refs": [{
                    "index": 0,
                    "library_id": plan["skill_refs"][0]["library"]["id"],
                    "commit_sha": plan["skill_refs"][0]["library"]["commit_sha"],
                }],
                "parts": [{"kind": "library_skill", "key": key}],
            }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(skills_of(&w, agent).await.len(), 1);
}

// ---- a failed commit cleans up (SEC-146 D) -----------------------------------------------------

/// A 4×4 PNG (the avatar part of the SEC-146 D fixture).
fn avatar_png() -> Vec<u8> {
    let img =
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([10, 20, 30])));
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::write_to(&img, &mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
}

/// The SEC-146 D fixture: a webhook (its secret is sealed), an avatar (its blob is put) and a
/// required library skill (the critical audit commit) — no apps.
fn d_files(dir: &str, version: &str) -> Vec<(String, Vec<u8>)> {
    vec![
        (
            format!("{dir}/template.yaml"),
            format!(
                "format: 1\nid: {dir}\nversion: {version}\nname: D\nsummary: Commit failure.\n\
                 inputs:\n  - id: zone\n    label: Zone\n    pattern: \"^[a-z]+$\"\n\
                 skills:\n  - library: {GIT_ORIGIN}/acme/skills\n    skill: alpha\n    why: Does the work.\n"
            )
            .into_bytes(),
        ),
        (format!("{dir}/agent.yaml"), AGENT_YAML.as_bytes().to_vec()),
        (
            format!("{dir}/instructions.md"),
            INSTRUCTIONS.as_bytes().to_vec(),
        ),
        (format!("{dir}/README.md"), format!("# {dir}\nD.\n").into_bytes()),
        (
            format!("{dir}/automations/hook.yaml"),
            b"kind: webhook\nname: Escalate\ninstructions: Page for {input.zone}.\nverification: signed\n"
                .to_vec(),
        ),
        (format!("{dir}/avatar.png"), avatar_png()),
    ]
}

/// [`world`] over a memory-secret-store vault (the handle comes back for the seal assertions).
async fn d_world() -> (W, std::sync::Arc<aurora_secrets::MemoryStore>) {
    let mut h = Harness::new().await;
    let git = GitServer::start().await;
    let state = h
        .state
        .clone()
        .with_connect_override(&git.base())
        .with_library_fetch_timeout(std::time::Duration::from_secs(5));
    let mem = std::sync::Arc::new(aurora_secrets::MemoryStore::new("memory"));
    let keyring = std::sync::Arc::new(
        aurora_crypto::Keyring::parse(
            &format!(
                "k1:{}",
                base64::engine::general_purpose::STANDARD.encode([b'a'; 32])
            ),
            "k1",
        )
        .unwrap(),
    );
    let mut sr =
        aurora_secrets::StoreRegistry::from_config(&state.config.secrets, keyring).unwrap();
    sr.register(mem.clone());
    sr.set_default("memory").unwrap();
    let state = state.with_vault(std::sync::Arc::new(aurora_secrets::SecretVault::new(sr)));
    h.state = state.clone();
    h.app = test_router(state);
    let root = h.user("root@x.io", true).await;
    let root_cookie = cookie(&h.good_session(root).await.1);
    (
        W {
            h,
            git,
            root: root_cookie,
        },
        mem,
    )
}

#[tokio::test]
async fn a_failed_commit_cleans_up_the_avatar_blob_and_the_sealed_secret() {
    let (w, mem) = d_world().await;
    let (org, ws, owner) = w.ws_owner().await;
    seed_library(&w, org, &owner).await;
    seed_fs(&w, "acme/tpl", "d", "1.0.0", &d_files("d", "1.0.0")).await;
    let plan = w.plan(ws, &owner, "d").await;

    // The commit is made to fail: a deferred constraint trigger raises when the transaction
    // commits (the agent's insert fires it) — after the blob put and the vault seals.
    sqlx::query(
        "CREATE OR REPLACE FUNCTION aurora_tpl_fail_commit() RETURNS trigger AS $$
         BEGIN RAISE EXCEPTION 'test: refused commit'; END;
         $$ LANGUAGE plpgsql",
    )
    .execute(w.h.pool())
    .await
    .unwrap();
    sqlx::query(
        "CREATE CONSTRAINT TRIGGER aurora_tpl_fail_trg AFTER INSERT ON agents
         DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION aurora_tpl_fail_commit()",
    )
    .execute(w.h.pool())
    .await
    .unwrap();

    let (s, v) = w
        .create_body(
            ws,
            &owner,
            json!({
                "template_id": "d", "version": "1.0.0",
                "content_sha256": plan["content_sha256"],
                "name": "Monitor", "inputs": {"zone": "eu"},
                "skill_refs": [{
                    "index": 0,
                    "library_id": plan["skill_refs"][0]["library"]["id"],
                    "commit_sha": plan["skill_refs"][0]["library"]["commit_sha"],
                }],
            }),
        )
        .await;
    assert!(s >= 500, "the commit refusal is an internal error: {s} {v}");

    // Nothing was created, nothing was left outside the rolled-back transaction: no agent, no
    // agent-create audit rows (the seeding's own template events remain), no orphan avatar
    // blob, and the sealed webhook secret was discarded.
    assert_eq!(w.count("SELECT count(*) FROM agents").await, 0);
    assert_eq!(
        w.count("SELECT count(*) FROM audit_log WHERE action LIKE 'agent.%'")
            .await,
        0
    );
    assert_eq!(w.count("SELECT count(*) FROM blobs").await, 0);
    assert_eq!(mem.managed_count(), 0);
}

// ---- the creation (C-1111, C-1119's create list) ------------------------------------------------

/// The user id of a test user (by email).
async fn user_id(w: &W, email: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(w.h.pool())
        .await
        .unwrap()
}

impl W {
    /// The create call without status assertions (the refusals read the error).
    async fn create_body(&self, ws: Uuid, who: &str, body: Value) -> (u16, Value) {
        self.h
            .call(
                who,
                Method::POST,
                &format!("/v1/workspaces/{ws}/agents/from-template"),
                Some(body),
            )
            .await
    }
}

/// The full-creation fixture (the service-monitor shape): own skills `watch` and `backup`, a
/// helper file, two memories (the policy one locked), a schedule and a webhook, one required
/// app, a required and an optional library reference. The input has no pattern, so a hostile
/// value can be sent (one-pass substitution, C-1111).
fn full_files(dir: &str, version: &str) -> Vec<(String, Vec<u8>)> {
    vec![
        (
            format!("{dir}/template.yaml"),
            format!(
                "format: 1\nid: {dir}\nversion: {version}\nname: Monitor\nsummary: The full fixture.\n\
                 inputs:\n  - id: zone\n    label: Zone\n\
                 apps:\n  - app: com-example-thing\n    required: true\n    level: ask\n    why: Needs the thing.\n\
                 skills:\n  - library: {GIT_ORIGIN}/acme/skills\n    skill: alpha\n    why: Does the work.\n  \
                 - library: {GIT_ORIGIN}/acme/skills\n    skill: beta\n    required: false\n    why: Optional help.\n"
            )
            .into_bytes(),
        ),
        (format!("{dir}/agent.yaml"), AGENT_YAML.as_bytes().to_vec()),
        (
            format!("{dir}/instructions.md"),
            b"Watch {input.zone}.\n".to_vec(),
        ),
        (
            format!("{dir}/README.md"),
            format!("# {dir}\nFull.\n").into_bytes(),
        ),
        (
            format!("{dir}/skills/watch/SKILL.md"),
            tpl_skill_md("watch", "Something is off.", "Watch body."),
        ),
        (
            format!("{dir}/skills/backup/SKILL.md"),
            tpl_skill_md("backup", "Backing up.", "Backup body."),
        ),
        (
            format!("{dir}/helpers/report.sh"),
            b"#!/bin/sh\necho v1\n".to_vec(),
        ),
        (
            format!("{dir}/memory/notes.md"),
            b"---\nkey: notes\nupdated: 2026-11-20\n---\nTemplate notes.\n".to_vec(),
        ),
        (
            format!("{dir}/memory/policy.md"),
            "---\nkey: policy\nupdated: 2026-11-20\nlocked: true\n---\nPolicy {{input.zone}}.\n"
                .to_owned()
                .into_bytes(),
        ),
        (
            format!("{dir}/automations/daily.yaml"),
            b"kind: schedule\nname: Daily check\nprompt: Check the {input.zone} dashboards.\ncadence:\n  cron: \"0 8 * * 1-5\"\nenabled: true\n"
                .to_vec(),
        ),
        (
            format!("{dir}/automations/weekly.yaml"),
            b"kind: schedule\nname: Weekly report\nprompt: Write the weekly report for {input.zone}.\ncadence:\n  cron: \"0 9 * * 1\"\n"
                .to_vec(),
        ),
        (
            format!("{dir}/automations/hook.yaml"),
            b"kind: webhook\nname: Escalate\ninstructions: Page for {input.zone}.\nverification: signed\n"
                .to_vec(),
        ),
    ]
}

/// SEC-146: the tables the create route must never write. (`derived_rules` is not in the list:
/// the one grant writer compiles the created grant's rules, which the full test checks is the
/// only kind present.)
async fn guard_counts(w: &W) -> Vec<(&'static str, i64)> {
    let mut out = Vec::new();
    for table in [
        "policy_rules",
        "channel_integrations",
        "channel_presences",
        "channel_cards",
        "action_approvals",
        "secrets",
        "skill_libraries",
        "app_connections",
    ] {
        let n = w.count(&format!("SELECT count(*) FROM {table}")).await;
        out.push((table, n));
    }
    out
}

/// A library skill whose body is `n` characters (a known budget size for the two-skill
/// refusal).
fn big_skill_md(n: usize) -> Vec<u8> {
    format!("---\ndescription: Big skill\n---\n\n{}\n", "b".repeat(n)).into_bytes()
}

#[tokio::test]
async fn a_full_creation_writes_every_kind_with_its_provenance() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;
    let owner_id = user_id(&w, "owner@x.io").await;
    let admin = crate::plugins_lifecycle::Admin::trust(&w.h, w.root.clone()).await;
    admin.install(APP_PKG_YAML).await;
    let lib = seed_library(&w, org, &owner).await;

    // A starter grant on an organization secret (the ordinary new-agent step, D-414).
    let starter = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO secrets (id, org_id, name, kind, host_pattern, injection, value_source, value_ref)
         VALUES ($1,$2,'starter','generic','x','{}','managed','x')",
    )
    .bind(starter)
    .bind(org)
    .execute(w.h.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO instance_placement (singleton, mode, default_org_id, default_org_role,
            shared_workspaces, personal_enabled, personal_role, name_template, starter_grants, updated_at)
         VALUES (true, 'shared_org', $1, 'member', '[]'::jsonb, true, 'owner', '{name}', $2, now())
         ON CONFLICT (singleton) DO UPDATE SET starter_grants = $2",
    )
    .bind(org)
    .bind(json!([{"kind": "secret", "id": starter}]))
    .execute(w.h.pool())
    .await
    .unwrap();

    seed_fs(
        &w,
        "acme/tpl",
        "full",
        "1.0.0",
        &full_files("full", "1.0.0"),
    )
    .await;
    let plan = w.plan(ws, &owner, "full").await;
    let mine = conn(&w, org, "com-example-thing", Some(ws), "write").await;
    let before = guard_counts(&w).await;
    let (governance_before, byo_before): (Value, Option<String>) =
        sqlx::query_as("SELECT governance, app_posture FROM organizations WHERE id = $1")
            .bind(org)
            .fetch_one(w.h.pool())
            .await
            .unwrap();

    // A hostile input value: it becomes the value, it is never read as a placeholder.
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            json!({
                "template_id": "full", "version": "1.0.0",
                "content_sha256": plan["content_sha256"],
                "name": "Monitor", "handle": "monitor",
                "inputs": {"zone": "{agent.name}"},
                "accounts": [{"entry": 0, "app_id": "com-example-thing", "connection_id": mine}],
                "skill_refs": [{"index": 0, "library_id": plan["skill_refs"][0]["library"]["id"],
                                "commit_sha": plan["skill_refs"][0]["library"]["commit_sha"]}],
                "automations": [{"kind": "schedule", "name": "Daily check", "enabled": false}],
            }),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let agent: Uuid = v["agent"]["id"].as_str().unwrap().parse().unwrap();

    // The optional reference the person left out is noted, nothing else.
    let notes = v["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1, "{v}");
    assert_eq!(notes[0]["code"], "skill_skipped", "{v}");
    assert_eq!(notes[0]["index"], 1, "{v}");

    // The agent: hosted, the rendered brief (the value is data, never a placeholder again),
    // the provenance on the response, the template's proactive and effort.
    assert_eq!(v["agent"]["kind"], "hosted", "{v}");
    assert_eq!(v["agent"]["proactive_messaging"], "ask", "{v}");
    assert_eq!(v["agent"]["template"]["template_id"], "full", "{v}");
    assert_eq!(v["agent"]["template"]["version"], "1.0.0", "{v}");
    assert_eq!(brief_of(&w, agent).await, "Watch {agent.name}.\n");
    let inputs: Value = inputs_of(&w, agent).await;
    assert_eq!(inputs, json!({"zone": "{agent.name}"}), "{inputs}");
    let (policy_locked, policy_by): (bool, Option<Uuid>) = sqlx::query_as(
        "SELECT locked, locked_by FROM memories WHERE agent_id = $1 AND key = 'policy'",
    )
    .bind(agent)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(policy_locked);
    assert_eq!(policy_by, Some(owner_id));
    let (notes_content, prov): (String, Value) = sqlx::query_as(
        "SELECT r.content, r.provenance FROM memory_revisions r
         JOIN memories m ON m.id = r.memory_id
         WHERE m.agent_id = $1 AND m.key = 'notes' ORDER BY r.revision DESC LIMIT 1",
    )
    .bind(agent)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(
        notes_content.starts_with("Template notes."),
        "{notes_content}"
    );
    assert_eq!(prov["source"], "template", "{prov}");
    assert_eq!(prov["template"], "Monitor", "{prov}");
    assert_eq!(prov["template_version"], "1.0.0", "{prov}");

    // The skills: the two own ones (editable) and the library skill, pinned and read-only.
    assert_eq!(
        skills_of(&w, agent).await,
        vec![
            ("alpha".to_owned(), Some(lib)),
            ("backup".to_owned(), None),
            ("watch".to_owned(), None),
        ]
    );
    let (pinned, linked): (String, Option<Uuid>) = sqlx::query_as(
        "SELECT library_commit_sha, library_id FROM skills WHERE agent_id = $1 AND name = 'alpha'",
    )
    .bind(agent)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(pinned, plan["skill_refs"][0]["library"]["commit_sha"]);
    assert_eq!(linked, Some(lib));
    let alpha: Uuid = v["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "alpha")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let (ps, pv) =
        w.h.call(
            &owner,
            Method::PATCH,
            &format!("/v1/skills/{alpha}"),
            Some(json!({"body": "x"})),
        )
        .await;
    assert_eq!((ps, code(&pv)), (409, "library_skill_readonly"), "{pv}");

    // The helper file, written through C-1143's writer.
    assert_eq!(helpers_of(&w, agent).await, vec!["report.sh".to_owned()]);
    let content: Vec<u8> = sqlx::query_scalar(
        "SELECT content FROM agent_helper_files WHERE agent_id = $1 AND path = 'report.sh'",
    )
    .bind(agent)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(content, b"#!/bin/sh\necho v1\n");

    // The schedules: the chosen one paused, the other at the template's enabled, both with
    // the caller as their creator (the fire-time authority, 04 §Fire-time authority).
    let rows: Vec<(String, bool, Option<Uuid>)> = sqlx::query_as(
        "SELECT name, enabled, creator_user_id FROM schedules WHERE agent_id = $1 ORDER BY name",
    )
    .bind(agent)
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            ("Daily check".into(), false, Some(owner_id)),
            ("Weekly report".into(), true, Some(owner_id)),
        ]
    );

    // The webhook: created paused, its secret shown once in the answer.
    let hooks: Vec<(bool,)> = sqlx::query_as("SELECT enabled FROM webhooks WHERE agent_id = $1")
        .bind(agent)
        .fetch_all(w.h.pool())
        .await
        .unwrap();
    assert_eq!(hooks.len(), 1);
    assert!(!hooks[0].0, "{hooks:?}");
    assert!(v["webhooks"][0]["secret"].is_string(), "{v}");
    assert!(
        !v["webhooks"][0]["webhook"]["enabled"]
            .as_bool()
            .unwrap_or(true),
        "{v}"
    );

    // The access: the capped level of the plan (custom_only: ask over the read tools, the
    // write tools ask), through the one grant writer with `via: "template"`.
    assert_eq!(v["grants"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["grants"][0]["connection_id"], json!(mine), "{v}");
    assert_eq!(v["grants"][0]["mode"], "custom", "{v}");
    assert_eq!(v["grants"][0]["allow_tools"], json!(["look"]), "{v}");
    assert_eq!(v["grants"][0]["ask_tools"], json!(["change"]), "{v}");
    // The starter grant of a new hosted agent (D-414) is there too.
    let starter_row: Option<Uuid> = sqlx::query_scalar(
        "SELECT secret_id FROM grants WHERE agent_id = $1 AND secret_id IS NOT NULL",
    )
    .bind(agent)
    .fetch_optional(w.h.pool())
    .await
    .unwrap();
    assert_eq!(starter_row, Some(starter));
    let vias: Vec<String> = sqlx::query_scalar(
        "SELECT metadata->>'via' FROM audit_log WHERE action = 'grant.create' AND critical
         AND target_id = $1 ORDER BY created_at, id",
    )
    .bind(agent.to_string())
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    assert_eq!(vias, vec!["new_agent".to_owned(), "template".to_owned()]);

    // The provenance rows: all four kinds (instructions, each own skill, the library
    // reference's key, the helpers).
    assert_eq!(
        parts_of(&w, agent).await,
        vec![
            ("helpers".to_owned(), String::new()),
            ("instructions".to_owned(), String::new()),
            (
                "library_skill".to_owned(),
                "library|https://git.example/acme/skills|||alpha".to_owned()
            ),
            ("skill".to_owned(), "backup".to_owned()),
            ("skill".to_owned(), "watch".to_owned()),
        ]
    );

    // The audit: the ordinary agent.create with the template metadata.
    let (n, template_meta): (i64, Option<String>) = sqlx::query_as(
        "SELECT count(*), min(metadata->>'template') FROM audit_log
         WHERE action = 'agent.create' AND NOT critical AND target_id = $1",
    )
    .bind(agent.to_string())
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(n, 1);
    assert_eq!(template_meta.as_deref(), Some("full@1.0.0"));

    // SEC-146: nothing was written to the tables the route must never touch, no library was
    // created or changed, and the organization's governance and BYO posture are untouched.
    assert_eq!(guard_counts(&w).await, before, "the guarded tables changed");
    // The only derived rules are the ones the grant writer compiled for the created grant
    // (never `blocklist`, never another source).
    let (rule_kinds, stray_rules): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE source_kind = 'grant'), count(*) FILTER (WHERE source_kind <> 'grant')
         FROM derived_rules",
    )
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(stray_rules, 0);
    assert!(rule_kinds > 0);
    let (governance_after, byo_after): (Value, Option<String>) =
        sqlx::query_as("SELECT governance, app_posture FROM organizations WHERE id = $1")
            .bind(org)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(
        (governance_before, byo_before),
        (governance_after, byo_after)
    );
}

#[tokio::test]
async fn a_custom_choice_is_cut_to_the_catalogue_and_an_empty_one_gives_no_access() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;
    let admin = crate::plugins_lifecycle::Admin::trust(&w.h, w.root.clone()).await;
    admin.install(APP_PKG_YAML).await;
    let yaml = app_yaml(
        "t6n",
        "1.0.0",
        "\n    tools:\n      allow: [look, gone]\n      ask: [change, peek]\n",
    );
    seed_yaml(&w, "acme/tpl", "t6n", yaml.clone());
    w.git
        .serve_file("pkg/t6n-1.0.0.zip", pack_yaml("t6n", yaml));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    w.check(id).await;
    w.approve("t6n", "1.0.0").await;
    let mine = conn(&w, org, "com-example-thing", Some(ws), "write").await;
    let plan = w.plan(ws, &owner, "t6n").await;
    let base = json!({
        "template_id": "t6n", "version": "1.0.0",
        "content_sha256": plan["content_sha256"],
        "name": "Monitor", "inputs": {"zone": "eu"},
        "accounts": [{"entry": 0, "app_id": "com-example-thing", "connection_id": mine}],
    });

    // The tools not in the catalogue are dropped and named; the rest is the grant.
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            base.clone().merge(json!({
                "accounts": [{"entry": 0, "app_id": "com-example-thing", "connection_id": mine,
                              "custom": {"allow_tools": ["look", "gone"], "ask_tools": ["change", "peek"]}}],
            })),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let notes = v["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1, "{v}");
    assert_eq!(notes[0]["code"], "tools_dropped", "{v}");
    assert_eq!(notes[0]["entry"], 0, "{v}");
    let agent: Uuid = v["agent"]["id"].as_str().unwrap().parse().unwrap();
    let (mode, allow, ask): (String, Vec<String>, Vec<String>) = sqlx::query_as(
        "SELECT mode, allow_tools, ask_tools FROM grants WHERE agent_id = $1 AND connection_id = $2",
    )
    .bind(agent)
    .bind(mine)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(
        (mode.as_str(), allow, ask),
        ("custom", vec!["look".to_owned()], vec!["change".to_owned()])
    );

    // A custom choice left with no tool gives no access at all (SEC-146).
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            base.merge(json!({
                "name": "Monitor 2",
                "accounts": [{"entry": 0, "app_id": "com-example-thing", "connection_id": mine,
                              "custom": {"allow_tools": ["gone"], "ask_tools": ["peek"]}}],
            })),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    // Every tool was dropped, so the entry's access is refused entirely: both notes, and
    // neither a grant row nor a write anywhere.
    let notes = v["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 2, "{v}");
    assert_eq!(notes[0]["code"], "tools_dropped", "{v}");
    assert_eq!(notes[1]["code"], "access_not_given", "{v}");
    let agent: Uuid = v["agent"]["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(v["grants"], json!([]), "{v}");
    assert_eq!(
        w.count(&format!(
            "SELECT count(*) FROM grants WHERE agent_id = '{agent}'"
        ))
        .await,
        0
    );
}

#[tokio::test]
async fn the_create_accepts_what_the_plan_showed() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;
    let admin = crate::plugins_lifecycle::Admin::trust(&w.h, w.root.clone()).await;
    admin.install(APP_PKG_YAML).await;
    seed_library(&w, org, &owner).await;
    let yaml = app_yaml(
        "tagree",
        "1.0.0",
        &format!(
            "\n    tools:\n      allow: [look]\n      ask: [change, peek]\n\
             skills:\n  - library: {GIT_ORIGIN}/acme/skills\n    skill: alpha\n    why: Does the work.\n"
        ),
    );
    seed_yaml(&w, "acme/tpl", "tagree", yaml.clone());
    w.git
        .serve_file("pkg/tagree-1.0.0.zip", pack_yaml("tagree", yaml));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    w.check(id).await;
    w.approve("tagree", "1.0.0").await;
    let _ = conn(&w, org, "com-example-thing", Some(ws), "write").await;
    let org_wide = conn(&w, org, "com-example-thing", None, "write").await;

    // The plan's choice: the level (capped), the custom tool split cut to the catalogue, the
    // givable accounts, the resolved library.
    let plan = w.plan(ws, &owner, "tagree").await;
    let choice = &plan["entries"][0]["choices"][0];
    assert_eq!(choice["level"], "ask", "{plan}");
    assert_eq!(choice["level_note"], "custom_only", "{plan}");
    assert_eq!(choice["custom"]["allow_tools"], json!(["look"]), "{plan}");
    assert_eq!(choice["custom"]["ask_tools"], json!(["change"]), "{plan}");
    assert_eq!(choice["custom"]["dropped_tools"], json!(["peek"]), "{plan}");
    let accounts = choice["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 2, "{plan}");

    // Create takes exactly the plan's custom and one of the plan's accounts, and the
    // plan's resolved library id and commit: all of it accepted, the grant equal to the
    // plan's custom.
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            json!({
                "template_id": "tagree", "version": "1.0.0",
                "content_sha256": plan["content_sha256"],
                "name": "Monitor", "inputs": {"zone": "eu"},
                "accounts": [{"entry": 0, "app_id": choice["app_id"],
                              "connection_id": org_wide,
                              "custom": {"allow_tools": choice["custom"]["allow_tools"],
                                         "ask_tools": choice["custom"]["ask_tools"]}}],
                "skill_refs": [{"index": 0, "library_id": plan["skill_refs"][0]["library"]["id"],
                                "commit_sha": plan["skill_refs"][0]["library"]["commit_sha"]}],
            }),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    assert_eq!(v["notes"], json!([]), "{v}");
    let agent: Uuid = v["agent"]["id"].as_str().unwrap().parse().unwrap();
    let (mode, allow, ask): (String, Vec<String>, Vec<String>) =
        sqlx::query_as("SELECT mode, allow_tools, ask_tools FROM grants WHERE agent_id = $1")
            .bind(agent)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(
        (mode.as_str(), allow, ask),
        ("custom", vec!["look".to_owned()], vec!["change".to_owned()])
    );
    assert_eq!(
        skills_of(&w, agent).await,
        vec![(
            "alpha".to_owned(),
            Some(
                plan["skill_refs"][0]["library"]["id"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap()
            )
        )]
    );
}

// ---- the create's refusals (C-1111, C-1119's create list) ----------------------------------------

#[tokio::test]
async fn a_stale_plan_is_a_409_with_the_fresh_plan() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;
    let admin = crate::plugins_lifecycle::Admin::trust(&w.h, w.root.clone()).await;
    admin.install(APP_PKG_YAML).await;
    seed_library(&w, org, &owner).await;
    let id = seed_fs(
        &w,
        "acme/tpl",
        "full",
        "1.0.0",
        &full_files("full", "1.0.0"),
    )
    .await;
    let plan = w.plan(ws, &owner, "full").await;
    let stale = plan["content_sha256"].clone();

    // A hash the plan did not show (a lie, the empty string): 409 with the plan in details,
    // nothing created.
    for sha in [json!("0".repeat(64)), json!("")] {
        let (s, v) = w
            .create_body(
                ws,
                &owner,
                json!({
                    "template_id": "full", "version": "1.0.0", "content_sha256": sha,
                    "name": "Monitor", "inputs": {"zone": "eu"},
                }),
            )
            .await;
        assert_eq!((s, code(&v)), (409, "template_version_changed"), "{v}");
        assert_eq!(v["error"]["details"]["plan"]["version"], "1.0.0", "{v}");
    }
    assert_eq!(w.count("SELECT count(*) FROM agents").await, 0);

    // After the template moved on, the stale hash still answers 409 — and the details carry
    // the *fresh* plan to re-plan from.
    advance_fs(
        &w,
        id,
        "acme/tpl",
        "full",
        "2.0.0",
        &full_files("full", "2.0.0"),
        &[],
    )
    .await;
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            json!({
                "template_id": "full", "version": "1.0.0", "content_sha256": stale,
                "name": "Monitor", "inputs": {"zone": "eu"},
            }),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "template_version_changed"), "{v}");
    assert_eq!(v["error"]["details"]["plan"]["version"], "2.0.0", "{v}");
    assert_ne!(
        v["error"]["details"]["plan"]["content_sha256"], stale,
        "{v}"
    );
    assert_eq!(w.count("SELECT count(*) FROM agents").await, 0);
}

#[tokio::test]
async fn a_create_rechecks_the_offered_version_under_the_row_lock() {
    let w = world().await;
    let (_, ws, owner) = w.ws_owner().await;
    let id = seed_fs(&w, "acme/tpl", "t", "1.0.0", &files("t", "1.0.0")).await;
    // 2.0.0 is approved but held back, so the offer (and the plan) is 1.0.0.
    advance_fs(&w, id, "acme/tpl", "t", "2.0.0", &files("t", "2.0.0"), &[]).await;
    let tid: Uuid = sqlx::query_scalar("SELECT id FROM agent_templates WHERE template_id = 't'")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE agent_template_versions SET status = 'pending_approval'
         WHERE agent_template_id = $1 AND version = '2.0.0'",
    )
    .bind(tid)
    .execute(w.h.pool())
    .await
    .unwrap();
    let plan = w.plan(ws, &owner, "t").await;
    assert_eq!(plan["version"], "1.0.0", "{plan}");

    let app = w.h.app.clone();
    let pool = w.h.pool().clone();
    let body = json!({
        "template_id": "t",
        "version": plan["version"],
        "content_sha256": plan["content_sha256"],
        "name": "Monitor",
        "inputs": {"zone": "eu"},
    });

    // Hold the template row: the create's transaction blocks re-reading it FOR SHARE.
    let mut lock = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM agent_templates WHERE id = $1 FOR UPDATE")
        .bind(tid)
        .execute(&mut *lock)
        .await
        .unwrap();
    let task = tokio::spawn(app_call(
        app,
        owner.clone(),
        Method::POST,
        format!("/v1/workspaces/{ws}/agents/from-template"),
        Some(body),
    ));
    wait_blocked(&pool).await;

    // A newer version is approved while the create waits behind the row lock; the re-read in
    // the transaction must see it and refuse.
    sqlx::query(
        "UPDATE agent_template_versions SET status = 'approved', decided_at = now()
         WHERE agent_template_id = $1 AND version = '2.0.0'",
    )
    .bind(tid)
    .execute(&pool)
    .await
    .unwrap();
    lock.rollback().await.unwrap();

    let (s, v) = task.await.unwrap();
    assert_eq!((s, code(&v)), (409, "template_version_changed"), "{v}");
    assert_eq!(v["error"]["details"]["plan"]["version"], "1.0.0", "{v}");
    // No agent was created, and none is stamped with the stale version.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM agents WHERE workspace_id = $1")
        .bind(ws)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn a_create_refuses_bad_account_choices() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;
    let member = w.h.user("member@x.io", false).await;
    w.h.member(org, member, "member", "active").await;
    w.h.bind_ws(ws, member, "member").await;
    let member_cookie = cookie(&w.h.good_session(member).await.1);
    let admin = crate::plugins_lifecycle::Admin::trust(&w.h, w.root.clone()).await;
    admin.install(APP_PKG_YAML).await;
    seed_library(&w, org, &owner).await;
    seed_fs(
        &w,
        "acme/tpl",
        "full",
        "1.0.0",
        &full_files("full", "1.0.0"),
    )
    .await;
    let plan = w.plan(ws, &owner, "full").await;
    let mine = conn(&w, org, "com-example-thing", Some(ws), "write").await;
    let other = conn(&w, org, "com-example-other", Some(ws), "write").await;
    let base = json!({
        "template_id": "full", "version": "1.0.0",
        "content_sha256": plan["content_sha256"],
        "name": "Monitor", "inputs": {"zone": "eu"},
        "skill_refs": [{"index": 0, "library_id": plan["skill_refs"][0]["library"]["id"],
                        "commit_sha": plan["skill_refs"][0]["library"]["commit_sha"]}],
    });

    // The required entry without an account: 422 naming the entry, all problems at once is
    // the inputs' rule, the accounts name their entry.
    let (s, v) = w
        .create_body(ws, &owner, base.clone().merge(json!({"accounts": []})))
        .await;
    assert_eq!((s, code(&v)), (422, "template_account_missing"), "{v}");
    assert_eq!(
        v["error"]["details"]["fields"],
        json!([{"path": "/accounts", "message": "entry 0"}]),
        "{v}"
    );

    // Two accounts on one entry.
    let acct = json!({"entry": 0, "app_id": "com-example-thing", "connection_id": mine});
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            base.clone()
                .merge(json!({"accounts": [acct.clone(), acct]})),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "template_account_invalid"), "{v}");

    // An app the entry does not name.
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            base.clone().merge(json!({"accounts": [acct.clone()
                .merge(json!({"app_id": "com-example-other"}))]})),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "template_app_unavailable"), "{v}");

    // A connection of another app (the plan never listed it for this entry): the same 403
    // the C-504 check gives anything it did not list.
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            base.clone().merge(json!({"accounts": [acct
                .merge(json!({"connection_id": other}))]})),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    assert_eq!(
        v["error"]["details"]["who_can"]["role"], "workspace_owner",
        "{v}"
    );

    // A connection that is not `active` (the plan still lists it: it lists accounts by whom
    // they may be given): the same 403.
    sqlx::query("UPDATE app_connections SET status = 'disabled' WHERE id = $1")
        .bind(mine)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            base.clone().merge(json!({"accounts": [{"entry": 0,
                "app_id": "com-example-thing", "connection_id": mine}]})),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    sqlx::query("UPDATE app_connections SET status = 'active' WHERE id = $1")
        .bind(mine)
        .execute(w.h.pool())
        .await
        .unwrap();

    // A plain member: the plan lists no accounts for them, so the create is the same 403
    // (the route's one C-504 check, SEC-67's shape).
    let (s, v) = w
        .create_body(
            ws,
            &member_cookie,
            base.merge(
                json!({"accounts": [{"entry": 0, "app_id": "com-example-thing",
                                            "connection_id": mine}]}),
            ),
        )
        .await;
    assert_eq!((s, code(&v)), (403, "forbidden"), "{v}");
    assert_eq!(
        v["error"]["details"]["who_can"]["role"], "workspace_owner",
        "{v}"
    );

    // Nothing was created by any of the refusals.
    assert_eq!(w.count("SELECT count(*) FROM agents").await, 0);
}

#[tokio::test]
async fn a_create_refuses_levels_above_the_caps() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;
    let admin = crate::plugins_lifecycle::Admin::trust(&w.h, w.root.clone()).await;
    admin.install(APP_PKG_YAML).await;
    seed_library(&w, org, &owner).await;
    seed_fs(
        &w,
        "acme/tpl",
        "full",
        "1.0.0",
        &full_files("full", "1.0.0"),
    )
    .await;
    let plan = w.plan(ws, &owner, "full").await;
    let mine = conn(&w, org, "com-example-thing", Some(ws), "write").await;
    let base = json!({
        "template_id": "full", "version": "1.0.0",
        "content_sha256": plan["content_sha256"],
        "name": "Monitor", "inputs": {"zone": "eu"},
        "skill_refs": [{"index": 0, "library_id": plan["skill_refs"][0]["library"]["id"],
                        "commit_sha": plan["skill_refs"][0]["library"]["commit_sha"]}],
    });

    // `act` on a custom_only app: the plan capped it, but a body that insists is refused by
    // the one grant writer (SEC-67: the cap is not the plan's politeness).
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            base.clone().merge(json!({"accounts": [{"entry": 0,
                "app_id": "com-example-thing", "connection_id": mine, "level": "act"}]})),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "full_grant_forbidden"), "{v}");

    // An organization's account with a `read` limit: an `ask` choice is above it, with the
    // limit in the details (P5-UX-4, whichever path asked).
    let org_acct = conn(&w, org, "com-example-thing", None, "write").await;
    sqlx::query("UPDATE app_connections SET limit_level = 'read' WHERE id = $1")
        .bind(org_acct)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            base.merge(json!({"accounts": [{"entry": 0,
                "app_id": "com-example-thing", "connection_id": org_acct, "level": "ask"}]})),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "above_account_limit"), "{v}");
    assert_eq!(v["error"]["details"]["limit"], "read", "{v}");
    assert_eq!(v["error"]["details"]["who_can"]["role"], "org_admin", "{v}");

    // Nothing was created by either refusal.
    assert_eq!(w.count("SELECT count(*) FROM agents").await, 0);
}

#[tokio::test]
async fn a_create_enforces_the_agent_rules() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;
    let admin = crate::plugins_lifecycle::Admin::trust(&w.h, w.root.clone()).await;
    admin.install(APP_PKG_YAML).await;
    seed_library(&w, org, &owner).await;
    let id = seed_fs(
        &w,
        "acme/tpl",
        "full",
        "1.0.0",
        &full_files("full", "1.0.0"),
    )
    .await;
    let plan = w.plan(ws, &owner, "full").await;
    let mine = conn(&w, org, "com-example-thing", Some(ws), "write").await;
    let acct = json!({"entry": 0, "app_id": "com-example-thing", "connection_id": mine});
    let skill_refs = json!([{"index": 0, "library_id": plan["skill_refs"][0]["library"]["id"],
                             "commit_sha": plan["skill_refs"][0]["library"]["commit_sha"]}]);

    // A hostile input value is still a value: it must match the pattern (all problems at
    // once, C-1104). The pattern comes from the reference fixture's `zone` (the full fixture
    // has no pattern on purpose). A second source for the same repo would be 409, so the
    // template rides on the same source.
    add_template_to_source(
        &w,
        id,
        "acme/tpl",
        "refin",
        "1.0.0",
        &ref_files("refin", "1.0.0", ""),
    )
    .await;
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            json!({
                "template_id": "refin", "version": "1.0.0",
                "content_sha256": w.plan(ws, &owner, "refin").await["content_sha256"],
                "name": "Monitor", "handle": "monitor",
                "inputs": {"zone": "EU1 {agent.name}"},
            }),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "template_inputs_invalid"), "{v}");
    assert_eq!(
        v["error"]["details"]["fields"][0]["path"], "/inputs/zone",
        "{v}"
    );

    // A handle that is not `[a-z][a-z0-9-]{0,39}`.
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            json!({
                "template_id": "full", "version": "1.0.0",
                "content_sha256": plan["content_sha256"],
                "name": "Monitor", "handle": "Bad_Handle",
                "inputs": {"zone": "eu"}, "accounts": [acct], "skill_refs": skill_refs,
            }),
        )
        .await;
    assert_eq!((s, code(&v)), (400, "invalid_request"), "{v}");
    assert_eq!(v["error"]["details"]["fields"][0]["path"], "/handle", "{v}");

    // A taken handle: the agent rules' 409.
    let taken = w
        .create_agent(ws, &owner, "full", "1.0.0",
            json!({"handle": "monitor", "accounts": [acct.clone()], "skill_refs": skill_refs.clone()}))
        .await;
    assert_eq!(taken["agent"]["handle"], "monitor", "{taken}");
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            json!({
                "template_id": "full", "version": "1.0.0",
                "content_sha256": plan["content_sha256"],
                "name": "Monitor 2", "handle": "monitor",
                "inputs": {"zone": "eu"}, "accounts": [acct],
                "skill_refs": skill_refs,
            }),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "handle_taken"), "{v}");

    // The organization's max_agents: the agent-limit rule of ordinary creation applies.
    sqlx::query("UPDATE organizations SET settings = jsonb_set(settings, '{max_agents}', '1')")
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            json!({
                "template_id": "full", "version": "1.0.0",
                "content_sha256": plan["content_sha256"],
                "name": "Monitor 3", "handle": "other",
                "inputs": {"zone": "eu"}, "accounts": [acct],
                "skill_refs": skill_refs,
            }),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "agent_limit_reached"), "{v}");

    // A rendered brief above BRIEF_MAX never even becomes a version: the source check stores
    // it invalid with `template_too_large`, so nothing can be approved (the create's own
    // BRIEF_MAX check is the belt for already-stored content).
    let big = "b".repeat(20_001);
    let fs = vec![
        (
            "tbig/template.yaml".to_owned(),
            "format: 1\nid: tbig\nversion: 1.0.0\nname: Big\nsummary: Too big.\n"
                .to_owned()
                .into_bytes(),
        ),
        ("tbig/agent.yaml".to_owned(), AGENT_YAML.as_bytes().to_vec()),
        ("tbig/instructions.md".to_owned(), big.into_bytes()),
        ("tbig/README.md".to_owned(), b"# tbig\nBig.\n".to_vec()),
    ];
    let changes: Vec<Change> = fs.iter().map(|(p, b)| put(p.clone(), b.clone())).collect();
    w.bump("acme/tpl", &changes);
    w.git
        .serve_file("pkg/tbig-1.0.0.zip", pack_tree("tbig", &fs));
    w.check(id).await;
    let (s, v) = w
        .call(
            Method::GET,
            "/v1/admin/agent-templates/tbig/versions/1.0.0",
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"], "invalid", "{v}");
    assert_eq!(v["status_reason"], "template_too_large", "{v}");
    // No offered version: the plan (and so the create) cannot reach it at all.
    let (s, _) = ws_get(&w, ws, &owner, "/tbig").await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn a_create_refuses_bad_library_and_organization_references() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;
    let lib = seed_library(&w, org, &owner).await;

    // A template whose only reference is the required library skill `alpha`.
    let refs = format!(
        "skills:\n  - library: {GIT_ORIGIN}/acme/skills\n    skill: alpha\n    why: Does the work.\n"
    );
    let id = seed_fs(
        &w,
        "acme/tpl",
        "libref",
        "1.0.0",
        &ref_files("libref", "1.0.0", &refs),
    )
    .await;
    let plan = w.plan(ws, &owner, "libref").await;
    let base = json!({
        "template_id": "libref", "version": "1.0.0",
        "content_sha256": plan["content_sha256"],
        "name": "Monitor", "inputs": {"zone": "eu"},
    });

    // The required reference without a skill_refs item.
    let (s, v) = w
        .create_body(ws, &owner, base.clone().merge(json!({"skill_refs": []})))
        .await;
    assert_eq!((s, code(&v)), (422, "template_skill_missing"), "{v}");
    assert_eq!(
        v["error"]["details"]["fields"],
        json!([{"path": "/skill_refs", "message": "entry 0"}]),
        "{v}"
    );

    // A library that is not the one the plan resolved.
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            base.clone().merge(json!({"skill_refs": [{"index": 0,
                "library_id": Uuid::now_v7(),
                "commit_sha": plan["skill_refs"][0]["library"]["commit_sha"]}]})),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "template_skill_unavailable"), "{v}");

    // A commit the plan did not resolve (head2, where `alpha` was deleted): the plan's-commit
    // rule refuses before the add path is even reached (SEC-146 C), nothing written.
    let head1 = plan["skill_refs"][0]["library"]["commit_sha"]
        .as_str()
        .unwrap()
        .to_owned();
    let head2 = w
        .git
        .commit("acme/skills", "main", &[Change::DeleteDir("alpha".into())]);
    assert_ne!(head1, head2);
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            base.merge(json!({"skill_refs": [{"index": 0, "library_id": lib,
                                              "commit_sha": head2}]})),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "template_commit_changed"), "{v}");
    assert_eq!(w.count("SELECT count(*) FROM agents").await, 0);

    // A required organization skill the workspace does not have (nothing is ever written
    // for one, but a required one must still be there).
    let org_refs = "skills:\n  - organization: notes\n    why: Org knowledge.\n".to_owned();
    add_template_to_source(
        &w,
        id,
        "acme/tpl",
        "orgref",
        "1.0.0",
        &ref_files("orgref", "1.0.0", &org_refs),
    )
    .await;
    let plan = w.plan(ws, &owner, "orgref").await;
    assert_eq!(plan["skill_refs"][0]["available"], false, "{plan}");
    assert_eq!(
        plan["skill_refs"][0]["unavailable_reason"], "not_in_organization",
        "{plan}"
    );
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            json!({
                "template_id": "orgref", "version": "1.0.0",
                "content_sha256": plan["content_sha256"],
                "name": "Monitor", "inputs": {"zone": "eu"},
            }),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "template_skill_missing"), "{v}");
    assert_eq!(w.count("SELECT count(*) FROM agents").await, 0);
}

#[tokio::test]
async fn two_library_skills_above_the_agent_budget_roll_back_together() {
    let w = world_with(|c| c.skills_library.bytes_per_agent = 300).await;
    let (org, ws, owner) = w.ws_owner().await;

    // A library whose two skills each fit the 300-byte budget alone, together not.
    w.git.add_repo("acme/skills");
    w.git.commit(
        "acme/skills",
        "main",
        &[
            put("alpha/SKILL.md".into(), big_skill_md(200)),
            put("beta/SKILL.md".into(), big_skill_md(200)),
        ],
    );
    let (s, lib) =
        w.h.call(
            &owner,
            Method::POST,
            &format!("/v1/orgs/{org}/skill-libraries"),
            Some(json!({"label": "lib", "url": format!("{GIT_ORIGIN}/acme/skills")})),
        )
        .await;
    assert_eq!(s, 201, "{lib}");
    let lib: Uuid = lib["id"].as_str().unwrap().parse().unwrap();

    let refs = format!(
        "skills:\n  - library: {GIT_ORIGIN}/acme/skills\n    skill: alpha\n    why: One.\n  \
         - library: {GIT_ORIGIN}/acme/skills\n    skill: beta\n    why: Two.\n"
    );
    seed_fs(
        &w,
        "acme/tpl",
        "twobig",
        "1.0.0",
        &ref_files("twobig", "1.0.0", &refs),
    )
    .await;
    let plan = w.plan(ws, &owner, "twobig").await;
    assert_eq!(plan["skill_refs"][0]["available"], true, "{plan}");
    assert_eq!(plan["skill_refs"][1]["available"], true, "{plan}");

    // The first skill is written inside the transaction, the second passes the budget: the
    // whole creation rolls back, the written one included.
    let (s, v) = w
        .create_body(
            ws,
            &owner,
            json!({
                "template_id": "twobig", "version": "1.0.0",
                "content_sha256": plan["content_sha256"],
                "name": "Monitor", "inputs": {"zone": "eu"},
                "skill_refs": [
                    {"index": 0, "library_id": lib,
                     "commit_sha": plan["skill_refs"][0]["library"]["commit_sha"]},
                    {"index": 1, "library_id": lib,
                     "commit_sha": plan["skill_refs"][1]["library"]["commit_sha"]},
                ],
            }),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "library_budget_exceeded"), "{v}");
    assert_eq!(w.count("SELECT count(*) FROM agents").await, 0);
    assert_eq!(w.count("SELECT count(*) FROM skills").await, 0);
}

#[tokio::test]
async fn a_create_that_fails_late_leaves_nothing_behind() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;
    let admin = crate::plugins_lifecycle::Admin::trust(&w.h, w.root.clone()).await;
    admin.install(APP_PKG_YAML).await;
    seed_library(&w, org, &owner).await;
    seed_fs(
        &w,
        "acme/tpl",
        "full",
        "1.0.0",
        &full_files("full", "1.0.0"),
    )
    .await;
    let plan = w.plan(ws, &owner, "full").await;

    // An organization account whose limit refuses the template's `ask` level: the failure
    // happens in the grant writer, after the skills, helpers, memories, schedules and the
    // webhook were written in the same transaction.
    let org_acct = conn(&w, org, "com-example-thing", None, "write").await;
    sqlx::query("UPDATE app_connections SET limit_level = 'read' WHERE id = $1")
        .bind(org_acct)
        .execute(w.h.pool())
        .await
        .unwrap();

    let tables = [
        "agents",
        "skills",
        "agent_helper_files",
        "memories",
        "memory_revisions",
        "schedules",
        "webhooks",
        "grants",
        "agent_template_parts",
        "audit_log",
        "secrets",
    ];
    let mut before = Vec::new();
    for t in tables {
        before.push(w.count(&format!("SELECT count(*) FROM {t}")).await);
    }

    let (s, v) = w
        .create_body(
            ws,
            &owner,
            json!({
                "template_id": "full", "version": "1.0.0",
                "content_sha256": plan["content_sha256"],
                "name": "Monitor", "handle": "monitor",
                "inputs": {"zone": "eu"},
                "accounts": [{"entry": 0, "app_id": "com-example-thing",
                              "connection_id": org_acct}],
                "skill_refs": [{"index": 0, "library_id": plan["skill_refs"][0]["library"]["id"],
                                "commit_sha": plan["skill_refs"][0]["library"]["commit_sha"]}],
            }),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "above_account_limit"), "{v}");

    // All-or-nothing: not one row of any kind remains, the library skill and the helpers
    // included, and no audit row either.
    for (t, b) in tables.iter().zip(before) {
        let after = w.count(&format!("SELECT count(*) FROM {t}")).await;
        assert_eq!((t, after), (t, b), "{t} changed");
    }
}

#[tokio::test]
async fn admin_actions_never_change_a_created_agent() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;
    let admin = crate::plugins_lifecycle::Admin::trust(&w.h, w.root.clone()).await;
    admin.install(APP_PKG_YAML).await;
    seed_library(&w, org, &owner).await;
    let id = seed_fs(
        &w,
        "acme/tpl",
        "full",
        "1.0.0",
        &full_files("full", "1.0.0"),
    )
    .await;
    let plan = w.plan(ws, &owner, "full").await;
    let mine = conn(&w, org, "com-example-thing", Some(ws), "write").await;
    let created = w
        .create_agent(
            ws,
            &owner,
            "full",
            "1.0.0",
            json!({
                "accounts": [{"entry": 0, "app_id": "com-example-thing", "connection_id": mine}],
                "skill_refs": [{"index": 0, "library_id": plan["skill_refs"][0]["library"]["id"],
                                "commit_sha": plan["skill_refs"][0]["library"]["commit_sha"]}],
            }),
        )
        .await;
    let agent: Uuid = created["agent"]["id"].as_str().unwrap().parse().unwrap();

    let snapshot = async |w: &W, agent: Uuid| -> Vec<i64> {
        let mut out = Vec::new();
        for sql in [
            format!("SELECT count(*) FROM agent_template_parts WHERE agent_id = '{agent}'"),
            format!("SELECT count(*) FROM skills WHERE agent_id = '{agent}'"),
            format!("SELECT count(*) FROM schedules WHERE agent_id = '{agent}'"),
            format!("SELECT count(*) FROM webhooks WHERE agent_id = '{agent}'"),
            format!("SELECT count(*) FROM grants WHERE agent_id = '{agent}'"),
            format!("SELECT count(*) FROM memories WHERE agent_id = '{agent}'"),
        ] {
            out.push(w.count(&sql).await);
        }
        out
    };
    let (brief_before, status_before) = (
        brief_of(&w, agent).await,
        w.tpl_status(&owner, agent).await.1,
    );
    let parts_before = snapshot(&w, agent).await;

    // The reviewing admin withdraws the template, rejects the next pending version and
    // deletes the source: none of it touches the created agent.
    let (s, v) = w
        .call(
            Method::PATCH,
            "/v1/admin/agent-templates/full",
            Some(json!({"offered": false})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // A pending 2.0.0 (checked, not approved), which the admin then rejects. The 1.0.0 files
    // that the reference fixture does not have are deleted from the tree, or the source
    // comparison would fail.
    let refs = format!(
        "skills:\n  - library: {GIT_ORIGIN}/acme/skills\n    skill: beta\n    why: New work.\n"
    );
    let fs = ref_files("full", "2.0.0", &refs);
    let mut changes: Vec<Change> = fs.iter().map(|(p, b)| put(p.clone(), b.clone())).collect();
    for d in [
        "full/skills",
        "full/helpers",
        "full/memory",
        "full/automations",
    ] {
        changes.push(Change::DeleteDir(d.into()));
    }
    w.bump("acme/tpl", &changes);
    w.git
        .serve_file("pkg/full-2.0.0.zip", pack_tree("full", &fs));
    let r = w.check(id).await;
    assert_eq!(r["status"], "updated", "{r}");
    let (s, v) = w
        .call(
            Method::POST,
            "/v1/admin/agent-templates/full/versions/2.0.0/reject",
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .call(
            Method::DELETE,
            &format!("/v1/admin/agent-template-sources/{id}"),
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");

    // The agent: exactly what it was, and its status still answers with the version it took.
    assert_eq!(brief_of(&w, agent).await, brief_before);
    assert_eq!(snapshot(&w, agent).await, parts_before);
    let (s, status_after) = w.tpl_status(&owner, agent).await;
    assert_eq!(s, 200, "{status_after}");
    assert_eq!(
        status_after["version"], status_before["version"],
        "{status_after}"
    );
    assert_eq!(
        status_after["update"], status_before["update"],
        "{status_after}"
    );
}

// ---- the copied skill rules (C-1130 → C-1141) ---------------------------------------------------

/// `crates/aurora-template/src/skill_rules.rs` repeats the platform's skill rules (the one
/// validator, `aurora-registry/src/skill_format.rs`) because the template crate may depend
/// only on `aurora-core`. This drift test feeds both the same table of names and paths —
/// accepted and refused, hostile shapes included — and fails when the copy and the source of
/// truth ever disagree. No behaviour lives here.
#[test]
fn the_copied_skill_rules_match_the_platform_validator() {
    use aurora_core::limits;
    use aurora_registry::skill_format as validator;
    use aurora_template as copy;

    // The numbers the copy repeats.
    assert_eq!(copy::FILES_PER_SKILL_MAX, validator::FILES_PER_SKILL_MAX);
    assert_eq!(copy::PATH_MAX_DEPTH, validator::PATH_MAX_DEPTH);
    assert_eq!(copy::PATH_MAX_LEN, validator::PATH_MAX_LEN);

    // Verdicts as words, so the two crates' identical-but-distinct `NameError` types compare.
    fn verdict<R, E: std::fmt::Debug>(r: Result<R, E>) -> String {
        match r {
            Ok(_) => "ok".to_owned(),
            Err(e) => format!("{e:?}"),
        }
    }

    // Accepted names: the grammar's corners (one part, digits only, 64 characters) included.
    let accepted = [
        "a",
        "0",
        "0-0",
        "a-b2",
        "skills-1",
        "auroral",
        "a0-9z",
        "aurora9",
        &"a".repeat(limits::SKILL_NAME_MAX),
    ];
    // Refused names, with the reason both must give: the grammar's refusals, the reserved
    // prefix, and hostile shapes (a separator, a space, a dot, non-ASCII, over the limit).
    let refused: &[(&str, &str)] = &[
        ("", "Grammar"),
        ("A", "Grammar"),
        ("a--b", "Grammar"),
        ("-a", "Grammar"),
        ("a-", "Grammar"),
        ("a/b", "Grammar"),
        ("a b", "Grammar"),
        ("a..b", "Grammar"),
        ("café", "Grammar"),
        (&"a".repeat(limits::SKILL_NAME_MAX + 1), "Grammar"),
        ("aurora", "Reserved"),
        ("aurora-x", "Reserved"),
        ("Aurora-x", "Grammar"),
    ];
    for n in accepted {
        let (v, c) = (
            verdict(validator::check_name(n)),
            verdict(copy::check_name(n)),
        );
        assert_eq!((v.as_str(), c.as_str()), ("ok", "ok"), "{n}");
    }
    for (n, want) in refused {
        let (v, c) = (
            verdict(validator::check_name(n)),
            verdict(copy::check_name(n)),
        );
        assert_eq!((v.as_str(), c.as_str()), (*want, *want), "{n}");
    }

    // Accepted paths: the depth cap (4 segments) and the character set's corners.
    let accepted = [
        "run.sh",
        "scripts/run.sh",
        "a.b_c-d/e.txt",
        "x/y/z/w.txt",
        "SKILLX.md",
        "x/y/z/0.txt",
    ];
    // Refused paths: traversal, empty and dot segments, one too deep, the reserved `SKILL.md`
    // in any case, spaces, a backslash, non-ASCII, and one over the 200-character cap.
    let refused = [
        "",
        "../x",
        "./x",
        "a//b",
        "a/",
        "/a",
        "a/b/c/d/e",
        "Skill.md",
        "skill.MD/x",
        "x/SKILL.md/y",
        "a b",
        "a\\b",
        "café/x",
        &"a".repeat(201),
    ];
    for p in accepted {
        assert!(validator::path_ok(p), "{p}");
        assert!(copy::path_ok(p), "{p}");
    }
    for p in refused {
        assert!(!validator::path_ok(p), "{p}");
        assert!(!copy::path_ok(p), "{p}");
    }
}

// ---- the whole journey on the checked-in example (the service-monitor) --------------------------

/// The instance's catalogue for the example's apps: `grafana` (read tools) and `atlassian`
/// (the read and write tools the template's `tools:` name). The optional `gitlab` entry gets
/// no package and no account (optional means optional).
const EXAMPLE_APPS_YAML: &str = r#"package: { id: acme, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: P }
apps:
  - about: { id: grafana, name: Grafana, description: D, category: dev }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    tools:
      read:
        items:
          - { id: read_dashboard, name: Read, host: grafana.example, path: /d, methods: [GET] }
    hosts:
      - match: { host: "grafana.example" }
        inject:
          - bearer: "{credentials.api_token}"
  - about: { id: atlassian, name: Atlassian, description: D, category: dev }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    tools:
      read:
        items:
          - { id: search_issues, name: Search, host: jira.example, path: /s, methods: [GET] }
          - { id: get_issue, name: Get, host: jira.example, path: /i, methods: [GET] }
          - { id: list_issue_comments, name: Comments, host: jira.example, path: /c, methods: [GET] }
      write:
        items:
          - { id: create_issue, name: Create, host: jira.example, path: /i, methods: [POST] }
          - { id: add_issue_comment, name: Comment, host: jira.example, path: /c, methods: [POST] }
    hosts:
      - match: { host: "jira.example" }
        inject:
          - bearer: "{credentials.api_token}"
"#;

/// The checked-in examples repository (the publisher's checkout): the real manifest and the
/// real template files with their real modes.
fn examples_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates/examples")
}

/// The checked-in example `dir` as the publisher commits it: the real bytes from
/// `templates/examples` and the real modes (the helper keeps its executable bit). The paths
/// are `dir/...`, exactly like the git tree's.
fn example_files(dir: &str) -> Vec<(String, Vec<u8>, bool)> {
    use std::os::unix::fs::PermissionsExt as _;

    fn walk(rel: &str, out: &mut Vec<(String, Vec<u8>, bool)>) {
        let base = examples_root().join(rel);
        let mut es: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .map(|e| e.unwrap())
            .collect();
        es.sort_by_key(|e| e.file_name());
        for e in es {
            let p = format!("{rel}/{}", e.file_name().to_string_lossy());
            let m = e.metadata().unwrap();
            if m.is_dir() {
                walk(&p, out);
            } else {
                out.push((
                    p,
                    std::fs::read(e.path()).unwrap(),
                    m.permissions().mode() & 0o111 != 0,
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out
}

/// The publisher's next version of the example: `version:` bumped, the instructions changed
/// and the helper rewritten (the same files and modes otherwise).
fn example_files_bumped(dir: &str, to: &str) -> Vec<(String, Vec<u8>, bool)> {
    example_files(dir)
        .into_iter()
        .map(|(p, b, x)| {
            let b = match p.as_str() {
                p if p == format!("{dir}/template.yaml") => std::str::from_utf8(&b)
                    .unwrap()
                    .replace("version: 1.0.0", &format!("version: {to}"))
                    .into_bytes(),
                p if p == format!("{dir}/instructions.md") => {
                    let mut s = String::from_utf8(b).unwrap();
                    s.push_str("\nEvery morning report ends with the words: checked by 1.1.0.\n");
                    s.into_bytes()
                }
                p if p == format!("{dir}/helpers/alert-summary.sh") => {
                    let mut s = String::from_utf8(b).unwrap();
                    s.push_str("\necho 'summary 1.1.0'\n");
                    s.into_bytes()
                }
                _ => b,
            };
            (p, b, x)
        })
        .collect()
}

/// The git commit of `files` (the executable bit as `Exec`, like the publisher's tree).
fn example_changes(files: &[(String, Vec<u8>, bool)]) -> Vec<Change> {
    files
        .iter()
        .map(|(p, b, x)| {
            if *x {
                Change::Exec(p.clone(), b.clone())
            } else {
                Change::Put(p.clone(), b.clone())
            }
        })
        .collect()
}

/// The package zip of `files`, built with the real packer from the real tree (the modes
/// included, so the zip's tree matches the git tree's hashes).
fn example_zip(dir: &str, files: &[(String, Vec<u8>, bool)]) -> Vec<u8> {
    let tree: Vec<TreeFile> = files
        .iter()
        .map(|(p, b, x)| TreeFile {
            path: p.clone(),
            bytes: b.clone(),
            mode: if *x { 0o100755 } else { 0o100644 },
        })
        .collect();
    aurora_template::pack(dir, &tree)
}

/// The checked-in manifest's `packages.url` with `{id}` and `{version}` filled: the path the
/// package fetch dials (the override maps the host to the git server, the path stays).
fn example_pkg_path(dir: &str, version: &str) -> String {
    format!("example/agent-templates/releases/download/{dir}-v{version}/{dir}-{version}.zip")
}

/// The skill library the example references (its real address; the override dials the git
/// server), with `incident-writeup`; added to `org`; returns its id.
async fn seed_example_library(w: &W, org: Uuid, owner: &str) -> Uuid {
    w.git.add_repo("agents/skills.git");
    w.git.commit(
        "agents/skills.git",
        "main",
        &[put(
            "incident-writeup/SKILL.md".into(),
            skill_md("Incident write-up", "The service desk's format."),
        )],
    );
    let (s, lib) =
        w.h.call(
            owner,
            Method::POST,
            &format!("/v1/orgs/{org}/skill-libraries"),
            Some(json!({"label": "skills", "url": "https://gitlab.cern.ch/agents/skills.git"})),
        )
        .await;
    assert_eq!(s, 201, "{lib}");
    lib["id"].as_str().unwrap().parse().unwrap()
}

/// The real home-sync renderer's parts for `agent` (the supervisor's view).
async fn home_sync_parts(w: &W, agent: Uuid) -> Vec<aurora_protocol::runner::HomeSync> {
    use aurora_db::repo::hosted::HomeSyncClaim;
    let mut conn = w.h.pool().acquire().await.unwrap();
    let claim = HomeSyncClaim {
        sandbox_id: Uuid::now_v7(),
        boot_id: Uuid::now_v7(),
        agent_id: agent,
        generation: 7,
        harness_id: None,
    };
    aurora_api::domains::hosted::home::render(&w.h.state, &mut conn, &claim)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_checked_in_example_walks_the_whole_journey() {
    let w = world().await;
    let (org, ws, owner) = w.ws_owner().await;
    let admin = crate::plugins_lifecycle::Admin::trust(&w.h, w.root.clone()).await;
    admin.install(EXAMPLE_APPS_YAML).await;
    let lib = seed_example_library(&w, org, &owner).await;

    // The publisher: the examples repository in git — the real manifest and the real
    // service-monitor tree — and the package at the manifest's packages.url.
    let repo = "acme/examples";
    w.git.add_repo(repo);
    let mut changes = vec![put(
        "aurora-templates.yaml".into(),
        std::fs::read(examples_root().join("aurora-templates.yaml")).unwrap(),
    )];
    let fs = example_files("service-monitor");
    changes.extend(example_changes(&fs));
    w.git.commit(repo, "main", &changes);
    w.git.serve_file(
        &example_pkg_path("service-monitor", "1.0.0"),
        example_zip("service-monitor", &fs),
    );

    // The admin's preview reads the real manifest and the real folder without storing.
    let (s, v) = w
        .call(
            Method::POST,
            "/v1/admin/agent-template-sources/preview",
            Some(json!({"url": format!("{GIT_ORIGIN}/{repo}")})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["manifest"]["name"], "Example agent templates", "{v}");
    assert_eq!(
        v["manifest"]["packages_url"],
        "https://github.com/example/agent-templates/releases/download/{id}-v{version}/{id}-{version}.zip",
        "{v}"
    );
    let t = &v["templates"][0];
    assert_eq!(t["template_id"], "service-monitor", "{v}");
    assert_eq!(t["name"], "Service monitor", "{v}");
    assert_eq!(t["version"], "1.0.0", "{v}");
    assert_eq!(
        w.count("SELECT count(*) FROM agent_template_sources").await,
        0
    );

    // The source is added, the check fetches the real package and stores the version, the
    // pending review approves it.
    let src = w.add_source(repo, json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    let r = w.check(id).await;
    assert_eq!(r["status"], "updated", "{r}");
    w.approve("service-monitor", "1.0.0").await;

    // The workspace catalogue offers the real card (the helper files make `has_helpers`).
    let (s, v) = ws_get(&w, ws, &owner, "").await;
    assert_eq!(s, 200, "{v}");
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{v}");
    assert_eq!(items[0]["template_id"], "service-monitor", "{v}");
    assert_eq!(items[0]["version"], "1.0.0", "{v}");
    assert_eq!(items[0]["source_name"], "Example agent templates", "{v}");
    assert_eq!(items[0]["has_helpers"], true, "{v}");
    assert_eq!(items[0]["ready"], true, "{v}");

    // The plan: the reviewed content with the real app entries and the resolved library
    // reference (the atlassian entry's `tools:` cut to the catalogue without drops).
    let plan = w.plan(ws, &owner, "service-monitor").await;
    assert_eq!(plan["entries"].as_array().unwrap().len(), 3, "{plan}");
    let grafana = &plan["entries"][0];
    assert_eq!(grafana["required"], true, "{plan}");
    assert_eq!(grafana["choices"][0]["app_id"], "grafana", "{plan}");
    assert_eq!(grafana["choices"][0]["available"], true, "{plan}");
    assert_eq!(grafana["choices"][0]["level"], "read", "{plan}");
    let atlassian = &plan["entries"][1];
    assert_eq!(atlassian["choices"][0]["app_id"], "atlassian", "{plan}");
    assert_eq!(atlassian["choices"][0]["level"], "ask", "{plan}");
    assert_eq!(
        atlassian["choices"][0]["custom"],
        json!({
            "allow_tools": ["search_issues", "get_issue", "list_issue_comments"],
            "ask_tools": ["create_issue", "add_issue_comment"],
            "dropped_tools": [],
        }),
        "{plan}"
    );
    assert_eq!(plan["entries"][2]["required"], false, "{plan}");
    let refr = &plan["skill_refs"][0];
    assert_eq!(refr["kind"], "library", "{plan}");
    assert_eq!(refr["name"], "incident-writeup", "{plan}");
    assert_eq!(refr["required"], false, "{plan}");
    assert_eq!(refr["available"], true, "{plan}");
    assert!(
        refr["repo"]
            .as_str()
            .unwrap()
            .contains("gitlab.cern.ch/agents/skills"),
        "{plan}"
    );
    assert_eq!(refr["library"]["id"], json!(lib), "{plan}");

    // The creation: the accounts for the two required entries, the library skill taken, the
    // inputs given.
    let g = conn(&w, org, "grafana", Some(ws), "read").await;
    let a = conn(&w, org, "atlassian", Some(ws), "write").await;
    let (s, created) = w
        .create_body(
            ws,
            &owner,
            json!({
                "template_id": "service-monitor",
                "version": "1.0.0",
                "content_sha256": plan["content_sha256"],
                "name": "Service monitor",
                "inputs": {"jira_project": "OPS", "grafana_folder": "Checkout"},
                "accounts": [
                    {"entry": 0, "app_id": "grafana", "connection_id": g},
                    {"entry": 1, "app_id": "atlassian", "connection_id": a},
                ],
                "skill_refs": [{
                    "index": 0,
                    "library_id": refr["library"]["id"],
                    "commit_sha": refr["library"]["commit_sha"],
                }],
            }),
        )
        .await;
    assert_eq!(s, 201, "{created}");
    let agent: Uuid = created["agent"]["id"].as_str().unwrap().parse().unwrap();

    // The grants the template asked for: grafana read, atlassian ask (the read tools allow,
    // the write tools ask).
    let rows: Vec<(String, String, Vec<String>, Vec<String>)> = sqlx::query_as(
        "SELECT c.app_id, gr.mode, gr.allow_tools, gr.ask_tools FROM grants gr
         JOIN app_connections c ON c.id = gr.connection_id
         WHERE gr.agent_id = $1 ORDER BY c.app_id",
    )
    .bind(agent)
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            (
                "atlassian".to_owned(),
                "custom".to_owned(),
                vec![
                    "search_issues".to_owned(),
                    "get_issue".to_owned(),
                    "list_issue_comments".to_owned()
                ],
                vec!["create_issue".to_owned(), "add_issue_comment".to_owned()],
            ),
            (
                "grafana".to_owned(),
                "custom".to_owned(),
                vec!["read_dashboard".to_owned()],
                Vec::<String>::new(),
            ),
        ],
        "{rows:?}"
    );

    // The helper file landed through C-1143's writer, the provenance records every part.
    assert_eq!(
        helpers_of(&w, agent).await,
        vec!["alert-summary.sh".to_owned()]
    );
    assert_eq!(
        parts_of(&w, agent).await,
        vec![
            ("helpers".to_owned(), String::new()),
            ("instructions".to_owned(), String::new()),
            (
                "library_skill".to_owned(),
                "library|https://gitlab.cern.ch/agents/skills.git|||incident-writeup".to_owned()
            ),
            ("skill".to_owned(), "open-incident".to_owned()),
            ("skill".to_owned(), "triage-alert".to_owned()),
        ]
    );

    // The home-sync's `helpers` part carries the helper at `.aurora/helpers/` with its
    // executable bit, and `render_inputs.helpers` names the dir and the count.
    let parts = home_sync_parts(&w, agent).await;
    let entries: Vec<&Value> = parts
        .iter()
        .filter(|p| p.part.kind == aurora_protocol::runner::HomeSyncKind::Helpers)
        .flat_map(|p| p.part.entries.iter())
        .collect();
    assert_eq!(entries.len(), 1, "{parts:?}");
    assert_eq!(
        entries[0]["path"], ".aurora/helpers/alert-summary.sh",
        "{v}"
    );
    assert_eq!(entries[0]["exec"], true, "{v}");
    assert!(
        entries[0]["content"].as_str().unwrap().contains("summary"),
        "{v}"
    );
    let inputs = parts
        .iter()
        .find(|p| p.part.kind == aurora_protocol::runner::HomeSyncKind::RenderInputs)
        .unwrap();
    assert_eq!(
        inputs.part.entries[0]["helpers"],
        json!({"dir": ".aurora/helpers", "count": 1}),
        "{parts:?}"
    );

    // The provenance the agent's Template section shows.
    let (s, v) = w.tpl_status(&owner, agent).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["template"]["template_id"], "service-monitor", "{v}");
    assert_eq!(v["template"]["name"], "Service monitor", "{v}");
    assert_eq!(v["template"]["version"], "1.0.0", "{v}");
    assert_eq!(v["available"], true, "{v}");
    assert_eq!(v["update"], json!(null), "{v}");

    // The publisher publishes 1.1.0: the bumped tree committed, the new package served, the
    // check ingests it and the review approves it. The catalogue now offers 1.1.0.
    let fs2 = example_files_bumped("service-monitor", "1.1.0");
    w.bump(repo, &example_changes(&fs2));
    w.git.serve_file(
        &example_pkg_path("service-monitor", "1.1.0"),
        example_zip("service-monitor", &fs2),
    );
    let r = w.check(id).await;
    assert_eq!(r["status"], "updated", "{r}");
    w.approve("service-monitor", "1.1.0").await;
    let (s, v) = ws_get(&w, ws, &owner, "").await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"][0]["version"], "1.1.0", "{v}");

    // The offer lists exactly the two changed parts, the helper's text with its executable
    // bit and the rendered instructions.
    let (s, v) = w.tpl_status(&owner, agent).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["update"]["version"], "1.1.0", "{v}");
    let kinds: Vec<&str> = v["update"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["instructions", "helpers"], "{v}");
    assert_eq!(part_state(&v, "instructions", ""), Some("unchanged_here"));
    assert_eq!(part_state(&v, "helpers", ""), Some("unchanged_here"));
    let instructions = v["update"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "instructions")
        .unwrap();
    assert!(
        !instructions["current"].as_str().unwrap().contains("1.1.0"),
        "{v}"
    );
    assert!(
        instructions["proposed"]
            .as_str()
            .unwrap()
            .contains("checked by 1.1.0"),
        "{v}"
    );
    let helpers = v["update"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "helpers")
        .unwrap();
    assert!(
        helpers["proposed"]
            .as_str()
            .unwrap()
            .starts_with("- alert-summary.sh ("),
        "{v}"
    );
    assert!(
        helpers["proposed"]
            .as_str()
            .unwrap()
            .contains(", executable"),
        "{v}"
    );

    // The apply takes both: the brief and the helper content move, nothing else does.
    let sha = v["update"]["content_sha256"].as_str().unwrap().to_owned();
    let (s, v) = w
        .apply(
            &owner,
            agent,
            json!({
                "version": "1.1.0",
                "content_sha256": sha,
                "parts": [
                    {"kind": "instructions", "key": ""},
                    {"kind": "helpers", "key": ""},
                ],
            }),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["template"]["version"], "1.1.0", "{v}");
    assert_eq!(v["update"], json!(null), "{v}");
    assert!(
        brief_of(&w, agent).await.contains("checked by 1.1.0"),
        "{}",
        brief_of(&w, agent).await
    );
    assert_eq!(
        helpers_of(&w, agent).await,
        vec!["alert-summary.sh".to_owned()]
    );
    let content: Vec<u8> = sqlx::query_scalar(
        "SELECT content FROM agent_helper_files WHERE agent_id = $1 AND path = 'alert-summary.sh'",
    )
    .bind(agent)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(
        String::from_utf8_lossy(&content).contains("summary 1.1.0"),
        "{content:?}"
    );

    // The home-sync's helpers part still carries the one helper, executable, with the new
    // content.
    let parts = home_sync_parts(&w, agent).await;
    let entries: Vec<&Value> = parts
        .iter()
        .filter(|p| p.part.kind == aurora_protocol::runner::HomeSyncKind::Helpers)
        .flat_map(|p| p.part.entries.iter())
        .collect();
    assert_eq!(entries.len(), 1, "{parts:?}");
    assert_eq!(entries[0]["exec"], true, "{v}");
    assert!(
        entries[0]["content"]
            .as_str()
            .unwrap()
            .contains("summary 1.1.0"),
        "{v}"
    );

    // The detach: the provenance is forgotten, the helper files and the skills stay.
    let (s, v) =
        w.h.call(
            &owner,
            Method::DELETE,
            &format!("/v1/agents/{agent}/template"),
            None,
        )
        .await;
    assert_eq!(s, 204, "{v}");
    let (s, v) = w.tpl_status(&owner, agent).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["template"], json!(null), "{v}");
    assert_eq!(v["available"], false, "{v}");
    assert_eq!(v["update"], json!(null), "{v}");
    assert_eq!(
        helpers_of(&w, agent).await,
        vec!["alert-summary.sh".to_owned()]
    );
    assert_eq!(skills_of(&w, agent).await.len(), 3);
}

// ---------------------------------------------------------------------------------------------
// (FIX-251, D-488(a)) The update notice on an approval and a re-offer.
// ---------------------------------------------------------------------------------------------

struct FailMailer;

#[async_trait::async_trait]
impl Mailer for FailMailer {
    async fn send(&self, _: Mail) -> Result<(), MailError> {
        Err(MailError("smtp down".into()))
    }
}

/// Seed, ingest and approve nothing: the source is checked, `t`@`1.0.0` waits for review.
async fn template_ingested(w: &W) {
    w.seed("acme/tpl", "t", "1.0.0");
    w.git.serve_file("pkg/t-1.0.0.zip", pack("t", "1.0.0"));
    let src = w.add_source("acme/tpl", json!({})).await;
    let id: Uuid = src["id"].as_str().unwrap().parse().unwrap();
    w.check(id).await;
}

/// An agent made from `t`, recorded as coming from `version` (behind the offered one when that
/// is newer).
async fn agent_from_template(
    w: &W,
    org: Uuid,
    ws: Uuid,
    handle: &str,
    version: Option<&str>,
) -> Uuid {
    let id = w.h.hosted_agent(org, ws, handle).await;
    sqlx::query(
        "UPDATE agents SET template_id = 't', template_name = 'T', template_version = $2 WHERE id = $1",
    )
    .bind(id)
    .bind(version)
    .execute(w.h.pool())
    .await
    .unwrap();
    id
}

/// An active org member with their own workspace binding (the apply right) and their cookie.
async fn bound_member(w: &W, org: Uuid, ws: Uuid, email: &str, role: &str) -> (Uuid, String) {
    let u = w.h.user(email, false).await;
    w.h.member(org, u, "member", "active").await;
    w.h.bind_ws(ws, u, role).await;
    let cookie = cookie(&w.h.good_session(u).await.1);
    (u, cookie)
}

/// An approval tells exactly the people who can apply the update — the active members with an
/// effective workspace access (the right the apply route requires): the bound owner and member
/// get it, a bound-but-disabled person and a workspace-less org member do not, the approver
/// never does; one notice per workspace and version (the dedup row); the bodies name nobody by
/// e-mail address; the workspace attention feed carries the in-app notice.
#[tokio::test]
async fn an_approval_tells_exactly_the_people_who_can_apply() {
    let w = world().await;
    template_ingested(&w).await;
    let org = w.h.org().await;
    let ws = w.h.workspace(org).await;
    let (_, owner_cookie) = bound_member(&w, org, ws, "owner@x.io", "owner").await;
    let _ = bound_member(&w, org, ws, "member@x.io", "member").await;
    // A bound person whose account is disabled: never told.
    let gone = w.h.user("gone@x.io", false).await;
    w.h.member(org, gone, "member", "active").await;
    w.h.bind_ws(ws, gone, "member").await;
    w.h.disable_user(gone).await;
    // An active org member with no workspace binding: no apply right, never told.
    let outsider = w.h.user("outsider@x.io", false).await;
    w.h.member(org, outsider, "member", "active").await;

    agent_from_template(&w, org, ws, "behind", Some("0.9.0")).await;
    w.approve("t", "1.0.0").await;

    let mail = w.h.mail.sent();
    let tos: Vec<&str> = mail.iter().map(|m| m.to.as_str()).collect();
    assert_eq!(tos, vec!["owner@x.io", "member@x.io"], "{mail:?}");
    for m in &mail {
        assert!(
            m.body.contains("T 1.0.0 is available for 1 agent in"),
            "{m:?}"
        );
        assert!(m.body.contains("memories are never changed"), "{m:?}");
        // Never an e-mail address as a label in the body.
        assert!(!m.body.contains("@x.io"), "{m:?}");
    }
    // The dedup row: exactly one, for this workspace and version.
    let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT workspace_id, template_id, version FROM agent_template_update_notices",
    )
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    assert_eq!(rows, vec![(ws, "t".into(), "1.0.0".into())], "{rows:?}");

    // The in-app notice: the workspace attention feed shows it to a member with the apply
    // right, with the link to the Agents page filtered to the agents that are behind.
    let (s, v) =
        w.h.call(
            &owner_cookie,
            Method::GET,
            &format!("/v1/workspaces/{ws}/attention"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let items: Vec<&Value> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"].as_str() == Some("template_update"))
        .collect();
    assert_eq!(items.len(), 1, "{v}");
    assert!(
        items[0]["title"]
            .as_str()
            .unwrap()
            .starts_with("T 1.0.0 is available for 1 agent in "),
        "{v}"
    );
    assert!(
        items[0]["action"]["href"]
            .as_str()
            .unwrap()
            .ends_with("/agents?needs_update=1")
            && items[0]["action"]["href"]
                .as_str()
                .unwrap()
                .contains(&format!("/w/{}/agents", ws.simple())),
        "{v}"
    );
    assert_eq!(items[0]["action"]["label"], json!("Review updates"), "{v}");

    // The rails' number counts it under the agents topic.
    let (s, v) =
        w.h.call(
            &owner_cookie,
            Method::GET,
            &format!("/v1/attention/counts?workspace={ws}"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(
        v["workspace"]["topics"]["agents"].as_i64().unwrap_or(0) >= 1,
        "{v}"
    );

    // A re-approval path (the version is approved; a second decide is refused) — the dedup is
    // proven by the re-offer test; here: a withdrawal and a re-offer keep it silent.
    let (s, v) = w
        .call(
            Method::PATCH,
            "/v1/admin/agent-templates/t",
            Some(json!({"offered": false})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .call(
            Method::PATCH,
            "/v1/admin/agent-templates/t",
            Some(json!({"offered": true})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(w.h.mail.sent().len(), 2, "no second notice");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM agent_template_update_notices")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(rows, 1);
}

/// A re-offer of the same version is silent (the dedup), a new version tells again, and every
/// workspace with agents from the template gets its own notice.
#[tokio::test]
async fn the_notice_is_sent_once_per_workspace_and_version() {
    let w = world().await;
    template_ingested(&w).await;
    let org = w.h.org().await;
    let ws1 = w.h.workspace(org).await;
    let ws2 = w.h.workspace(org).await;
    let _ = bound_member(&w, org, ws1, "one@x.io", "owner").await;
    let _ = bound_member(&w, org, ws2, "two@x.io", "owner").await;
    agent_from_template(&w, org, ws1, "a1", Some("0.9.0")).await;
    agent_from_template(&w, org, ws2, "a2", None).await;

    w.approve("t", "1.0.0").await;
    let mail = w.h.mail.sent();
    let tos: Vec<&str> = mail.iter().map(|m| m.to.as_str()).collect();
    assert_eq!(tos, vec!["one@x.io", "two@x.io"], "{mail:?}");
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT workspace_id, version FROM agent_template_update_notices ORDER BY workspace_id",
    )
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");

    // Withdraw and offer again: the same version is silent everywhere.
    let (s, v) = w
        .call(
            Method::PATCH,
            "/v1/admin/agent-templates/t",
            Some(json!({"offered": false})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .call(
            Method::PATCH,
            "/v1/admin/agent-templates/t",
            Some(json!({"offered": true})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(w.h.mail.sent().len(), 2, "the re-offer told nobody again");

    // A new version tells again, once per workspace and version.
    w.seed("acme/tpl", "t", "1.1.0");
    w.git.serve_file("pkg/t-1.1.0.zip", pack("t", "1.1.0"));
    let sid: Uuid = sqlx::query_scalar("SELECT id FROM agent_template_sources LIMIT 1")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    let (s, v) = w
        .call(
            Method::POST,
            &format!("/v1/admin/agent-template-sources/{sid}/check"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    w.approve("t", "1.1.0").await;
    let mail = w.h.mail.sent();
    assert_eq!(mail.len(), 4, "{mail:?}");
    assert!(mail.iter().any(|m| m.body.contains("1.1.0")), "{mail:?}");
}

/// A failing mailer never fails or undoes the approval: the version is approved, the critical
/// audit row stands and the in-app notice (the dedup row) is there.
#[tokio::test]
async fn a_failing_notice_does_not_fail_the_approval() {
    let mut w = world().await;
    template_ingested(&w).await;
    let org = w.h.org().await;
    let ws = w.h.workspace(org).await;
    let _ = bound_member(&w, org, ws, "owner@x.io", "owner").await;
    agent_from_template(&w, org, ws, "behind", Some("0.9.0")).await;

    w.h.state =
        w.h.state
            .clone()
            .with_mailer(std::sync::Arc::new(FailMailer));
    w.h.app = test_router(w.h.state.clone());

    let (s, v) = w
        .call(
            Method::POST,
            "/v1/admin/agent-templates/t/versions/1.0.0/approve",
            Some(json!({"content_sha256": w.content_sha("1.0.0").await.unwrap()})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        w.versions().await,
        vec![("1.0.0".into(), "approved".into(), None)]
    );
    assert_eq!(w.audit_critical("agent_template.approve").await, 1);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM agent_template_update_notices")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    assert_eq!(rows, 1, "the in-app notice is there");
}

// ---------------------------------------------------------------------------------------------
// (FIX-251, D-488(a)) The Update available mark on the agents list and the agent read.
// ---------------------------------------------------------------------------------------------

fn needs_update_of(v: &Value, id: Uuid) -> bool {
    v["items"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|a| a["id"].as_str() == Some(id.to_string().as_str()))
                .map(|a| a["needs_update"].as_bool().unwrap())
        })
        .unwrap_or_else(|| panic!("agent {id} not in the list: {v}"))
}

/// The workspace agents list and the single-agent read mark the agents whose template offers a
/// semver-newer version than the stored one (`needs_update`): behind at 0.9.0 against an
/// approved 1.0.0, an agent already on it is not, an agent without a template never is; a newer
/// approval marks both; a withdrawn offer marks nobody.
#[tokio::test]
async fn the_agents_list_marks_the_ones_behind() {
    let w = world().await;
    template_ingested(&w).await;
    let org = w.h.org().await;
    let ws = w.h.workspace(org).await;
    let (_, cookie) = bound_member(&w, org, ws, "owner@x.io", "owner").await;
    let behind = agent_from_template(&w, org, ws, "behind", Some("0.9.0")).await;
    let current = agent_from_template(&w, org, ws, "current", Some("1.0.0")).await;
    let plain = w.h.hosted_agent(org, ws, "plain").await;

    w.approve("t", "1.0.0").await;

    let (s, v) = w
        .h
        .call(
            &cookie,
            Method::GET,
            &format!("/v1/workspaces/{ws}/agents"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(needs_update_of(&v, behind), "{v}");
    assert!(!needs_update_of(&v, current), "{v}");
    assert!(!needs_update_of(&v, plain), "{v}");
    let (s, v) = w.h.call(&cookie, Method::GET, &format!("/v1/agents/{behind}"), None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["needs_update"], json!(true), "{v}");

    // A newer approved version marks the stored one behind too.
    w.seed("acme/tpl", "t", "1.1.0");
    w.git.serve_file("pkg/t-1.1.0.zip", pack("t", "1.1.0"));
    let sid: Uuid = sqlx::query_scalar("SELECT id FROM agent_template_sources LIMIT 1")
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    w.check(sid).await;
    w.approve("t", "1.1.0").await;
    let (s, v) = w
        .h
        .call(
            &cookie,
            Method::GET,
            &format!("/v1/workspaces/{ws}/agents"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(needs_update_of(&v, behind), "{v}");
    assert!(needs_update_of(&v, current), "{v}");

    // The offer withdrawn: nobody is behind.
    let (s, v) = w
        .call(
            Method::PATCH,
            "/v1/admin/agent-templates/t",
            Some(json!({"offered": false})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .h
        .call(
            &cookie,
            Method::GET,
            &format!("/v1/workspaces/{ws}/agents"),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(!needs_update_of(&v, behind), "{v}");
    assert!(!needs_update_of(&v, current), "{v}");
}
