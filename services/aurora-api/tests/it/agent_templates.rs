#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P6-TPL-2 (C-1107, C-1108): agent template sources against a real git server
//! (`aurora_testkit::git_server`), read by the API's own git reader through the guarded
//! client's test override; the packages are served by the same server on plain paths (the
//! override maps every origin to it, so the token-origin rule stays testable). Real Postgres.

use aurora_api::domains::agent_templates::poll;
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
        let sha = v["content_sha256"].as_str().unwrap().to_owned();
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
    // The card carries the same availability.
    let (s, v) = ws_get(&w, ws, &owner, "").await;
    assert_eq!(s, 200, "{v}");
    let item = &v["items"][0];
    assert_eq!(item["apps"][0]["available"], false, "{v}");
    assert_eq!(item["apps"][0]["skills"], json!([]), "{v}");
    assert_eq!(item["skill_refs"][0]["available"], true, "{v}");
    assert_eq!(item["skill_refs"][1]["available"], false, "{v}");
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
    assert_eq!(
        items[0]["href"], "/admin/agent-templates/t/versions/1.0.0",
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
    assert_eq!(
        items[0]["href"], "/admin/agent-templates/t/versions/1.1.0",
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
