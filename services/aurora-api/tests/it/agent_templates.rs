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
    let mut h = Harness::with_config(|_| {}).await;
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
