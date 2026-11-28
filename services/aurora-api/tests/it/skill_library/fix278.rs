//! FIX-278: a library says what can really be added (the add's own rules on the real content),
//! names its default branch, suggests a label that names the repository, and follows a changed
//! source: pending proposals of the old source are dropped, in the change's transaction, and can
//! never be approved afterwards.

use std::time::{Duration, Instant};

use sqlx::PgConnection;

use super::*;

// ---- helpers --------------------------------------------------------------------------------

fn item<'a>(v: &'a Value, path: &str) -> &'a Value {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["path"] == path)
        .unwrap_or_else(|| panic!("{path} missing in {v}"))
}

impl W {
    async fn browse_agent(&self, lib: Uuid) -> Value {
        let (s, v) = self
            .call(
                &self.member,
                Method::GET,
                &format!(
                    "/v1/skill-libraries/{lib}/skills?tier=agent&agent={}",
                    self.agent
                ),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }

    async fn preview(&self, lib: Uuid, dir: &str) -> Value {
        let (s, v) = self
            .call(
                &self.admin,
                Method::GET,
                &format!("/v1/skill-libraries/{lib}/skills/{dir}"),
                None,
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }

    async fn check_repo(&self, body: Value) -> Value {
        let (s, v) = self
            .call(
                &self.admin,
                Method::POST,
                &format!("/v1/orgs/{}/skill-libraries/check", self.org),
                Some(body),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        v
    }

    async fn patch_library(&self, lib: Uuid, body: Value) -> (u16, Value) {
        self.call(
            &self.admin,
            Method::PATCH,
            &format!("/v1/skill-libraries/{lib}"),
            Some(body),
        )
        .await
    }

    async fn approve(&self, skill: Uuid, update: &str, hash: &str) -> (u16, Value) {
        self.call(
            &self.admin,
            Method::POST,
            &format!("/v1/skills/{skill}/library/updates/{update}/approve"),
            Some(json!({"content_sha256": hash})),
        )
        .await
    }

    async fn stored(&self, skill: Uuid) -> (String, Option<String>) {
        sqlx::query_as("SELECT body, library_commit_sha FROM skills WHERE id = $1")
            .bind(skill)
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }

    async fn pending_rows(&self, lib: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM skill_library_updates WHERE library_id = $1 AND status = 'pending'",
        )
        .bind(lib)
        .fetch_one(self.h.pool())
        .await
        .unwrap()
    }
}

/// Waits (without sleeping) until at least `n` backends of this test's database wait for a lock.
async fn lock_waiters(pool: &sqlx::PgPool, n: i64) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity
             WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        if waiting >= n {
            return;
        }
        assert!(Instant::now() < deadline, "{n} lock waiters never appeared");
        tokio::task::yield_now().await;
    }
}

async fn exec_in(conn: &mut PgConnection, sql: &str) {
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .execute(conn)
        .await
        .unwrap();
}

// ---- C-1: the listing judges the real content ------------------------------------------------

#[tokio::test]
async fn fix278_browse_judges_the_real_content_like_the_add() {
    let w = world().await;
    w.git.add_repo("acme/skills");
    let sha = w.git.commit(
        "acme/skills",
        "main",
        &[
            put("ok/SKILL.md", md("Fine", "Body")),
            put("ok/notes.txt", b"some notes".to_vec()),
            // A small SKILL.md; the other files pass the 300,000-character total together.
            put("over/SKILL.md", md("Over", "Body")),
            put("over/a.txt", vec![b'a'; 150_000]),
            put("over/b.txt", vec![b'b'; 151_000]),
            // One file over the per-file limit.
            put("bigfile/SKILL.md", md("Big", "Body")),
            put("bigfile/data.bin", vec![b'x'; 170 * 1024]),
        ],
    );
    let lib = w.library().await;
    let v = w.browse_agent(lib).await;

    let body_chars = "Body\n".len() as u64;
    let over = item(&v, "over");
    assert_eq!(over["valid"], false, "{v}");
    assert!(
        over["reason"]
            .as_str()
            .unwrap()
            .contains("total more than 300000"),
        "{over}"
    );
    assert_eq!(over["reason_code"], "too_large", "{over}");
    assert_eq!(over["blocked"]["reason"], "invalid", "{over}");
    assert_eq!(over["size"].as_u64(), Some(body_chars + 301_000), "{over}");
    assert_eq!(over["files"], 3, "{over}");

    let big = item(&v, "bigfile");
    assert_eq!(big["valid"], false, "{v}");
    assert!(
        big["reason"]
            .as_str()
            .unwrap()
            .contains("larger than 163840 bytes"),
        "{big}"
    );
    assert_eq!(big["reason_code"], "file_too_large", "{big}");
    assert_eq!(big["blocked"]["reason"], "invalid", "{big}");
    assert_eq!(big["size"].as_u64(), Some(body_chars + 170 * 1024), "{big}");

    // The sizes are the ones the add computes.
    let ok = item(&v, "ok");
    assert_eq!(ok["valid"], true, "{ok}");
    assert_eq!(ok["reason_code"], Value::Null, "{ok}");
    assert_eq!(ok["blocked"], Value::Null, "{ok}");
    assert_eq!(ok["size"], w.preview(lib, "ok").await["size"]);
    assert_eq!(ok["size"].as_u64(), Some(body_chars + 10));
    assert_eq!(ok["files"], 2);
    let (s, a) = w.add_agent(lib, "ok", &sha).await;
    assert_eq!(s, 201, "{a}");
    for dir in ["over", "bigfile"] {
        let p = w.preview(lib, dir).await;
        assert_eq!(p["valid"], false, "{p}");
        assert_eq!(p["reason_code"], item(&v, dir)["reason_code"], "{p}");
        let (s, e) = w.add_agent(lib, dir, &sha).await;
        assert_eq!((s, code(&e)), (422, "library_skill_invalid"), "{e}");
    }
    assert_eq!(w.preview(lib, "ok").await["reason_code"], Value::Null);

    // The card's counts and the check dialog follow the same judgement.
    let row = w.library_row(lib).await;
    assert_eq!(
        (
            row["skills_offered"].as_i64(),
            row["skills_unavailable"].as_i64()
        ),
        (Some(1), Some(2)),
        "{row}"
    );
    let c = w
        .check_repo(json!({"url": url("acme/skills"), "path": ""}))
        .await;
    assert_eq!(c["skills"]["found"], 3, "{c}");
    assert_eq!(c["skills"]["valid"], 1, "{c}");
    assert_eq!(c["skills"]["names"], json!(["ok"]), "{c}");
}

#[tokio::test]
async fn fix278_every_invalid_cause_has_a_reason_code() {
    let w = world().await;
    w.git.add_repo("acme/skills");
    let many: Vec<Change> = (0..51)
        .map(|i| put(&format!("many/f{i}.txt"), b"x".to_vec()))
        .collect();
    let mut changes = vec![
        put("toolarge/SKILL.md", md("d", "b")),
        put("toolarge/a.txt", vec![b'a'; 150_000]),
        put("toolarge/b.txt", vec![b'a'; 151_000]),
        put("longbody/SKILL.md", md("d", &"b".repeat(48_100))),
        put("bigfile/SKILL.md", md("d", "b")),
        put("bigfile/x.bin", vec![b'x'; 161 * 1024]),
        put("bigskillmd/SKILL.md", md("d", &"b".repeat(170 * 1024))),
        put("many/SKILL.md", md("d", "b")),
        put("badpath/SKILL.md", md("d", "b")),
        put("badpath/a b.txt", b"x".to_vec()),
        put("linked/SKILL.md", md("d", "b")),
        Change::Symlink("linked/evil".into(), "/etc/passwd".into()),
        put("submodule/SKILL.md", md("d", "b")),
        Change::Submodule("submodule/dep".into()),
        put("nofm/SKILL.md", b"no frontmatter\n".to_vec()),
        put("misnamed/skill.md", md("d", "b")),
        put("nomd/Skill.md", md("d", "b")),
        put("Bad_Name/SKILL.md", md("d", "b")),
        // Too large to judge: the blob passes the cap of one git answer.
        put("huge/SKILL.md", md("d", "b")),
        put("huge/zeros.bin", vec![0u8; 65 * 1024 * 1024]),
        put(
            "zfine/SKILL.md",
            md("Judged although a neighbour is huge", "b"),
        ),
    ];
    changes.extend(many);
    w.git.commit("acme/skills", "main", &changes);
    let lib = w.library().await;
    let v = w.browse_agent(lib).await;
    for (dir, want) in [
        ("toolarge", "too_large"),
        ("longbody", "body_too_large"),
        ("bigfile", "file_too_large"),
        ("bigskillmd", "file_too_large"),
        ("many", "too_many_files"),
        ("badpath", "unsupported_path"),
        ("linked", "unsupported_entry"),
        ("submodule", "unsupported_entry"),
        ("nofm", "skill_md"),
        ("misnamed", "skill_md"),
        ("nomd", "skill_md"),
        ("Bad_Name", "bad_name"),
        ("huge", "unknown_size"),
    ] {
        let i = item(&v, dir);
        assert_eq!(i["valid"], false, "{dir}: {i}");
        assert_eq!(i["reason_code"], want, "{dir}: {i}");
        assert!(
            i["reason"].as_str().is_some_and(|r| !r.is_empty()),
            "{dir}: {i}"
        );
        assert_eq!(i["blocked"]["reason"], "invalid", "{dir}: {i}");
    }
    // Only the skill that could not be judged has no size, and says so in plain words.
    assert_eq!(item(&v, "huge")["size"], Value::Null, "{v}");
    assert!(
        item(&v, "huge")["blocked"]["message"]
            .as_str()
            .unwrap()
            .contains("could not be checked"),
        "{v}"
    );
    assert!(item(&v, "toolarge")["size"].is_u64(), "{v}");
    // A huge neighbour does not stop the others from being judged.
    assert_eq!(item(&v, "zfine")["valid"], true, "{v}");
    assert_eq!(item(&v, "zfine")["reason_code"], Value::Null);
    let row = w.library_row(lib).await;
    assert_eq!(row["skills_offered"], 1, "{row}");
    // The content route says the same.
    for (dir, want) in [("toolarge", "too_large"), ("nofm", "skill_md")] {
        assert_eq!(w.preview(lib, dir).await["reason_code"], want);
    }
}

#[tokio::test]
async fn fix278_an_invalid_proposal_has_a_reason_code() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    let alpha = add_org_id(&w, lib, "alpha", &sha).await;
    w.git.commit(
        "acme/skills",
        "main",
        &[
            put("alpha/big.txt", vec![b'a'; 150_000]),
            put("alpha/big2.txt", vec![b'a'; 151_000]),
        ],
    );
    w.refresh(lib).await;
    let p = w.pending(alpha).await;
    assert_eq!(p["update"]["kind"], "invalid", "{p}");
    assert_eq!(p["update"]["reason_code"], "too_large", "{p}");
    w.git.commit(
        "acme/skills",
        "main",
        &[put("alpha/big.txt", b"small again".to_vec())],
    );
    w.refresh(lib).await;
    let p = w.pending(alpha).await;
    assert_eq!(p["update"]["kind"], "changed", "{p}");
    assert_eq!(p["update"]["reason_code"], Value::Null, "{p}");
}

// ---- C-6: the default branch -----------------------------------------------------------------

#[tokio::test]
async fn fix278_a_library_without_a_branch_names_the_default_branch() {
    let w = world().await;
    w.seed();
    w.git.commit("acme/skills", "trunk", &[]);
    w.git.set_default_branch("acme/skills", "trunk");
    let lib = w.library().await;
    assert_eq!(w.library_row(lib).await["default_branch"], "trunk");
    // A library with a branch is unchanged.
    let (s, v) = w.patch_library(lib, json!({"ref": "main"})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["default_branch"], Value::Null, "{v}");
    assert_eq!(v["ref"], "main", "{v}");
    let (s, v) = w.patch_library(lib, json!({"ref": ""})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["default_branch"], "trunk", "{v}");
    // A refresh follows a renamed default branch, even at the same commit.
    let head = w.git.head("acme/skills", "trunk");
    add_org_id(&w, lib, "alpha", &head).await;
    w.git.set_default_branch("acme/skills", "main");
    w.refresh(lib).await;
    assert_eq!(w.library_row(lib).await["default_branch"], "main");
    // A browse fills it for a library from before the field existed.
    sqlx::query("UPDATE skill_libraries SET default_branch = NULL")
        .execute(w.h.pool())
        .await
        .unwrap();
    w.browse_agent(lib).await;
    assert_eq!(w.library_row(lib).await["default_branch"], "main");
    // The members' picker says nothing about the repository.
    let (_, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!("/v1/orgs/{}/skill-libraries", w.org),
            None,
        )
        .await;
    assert_eq!(v["items"][0]["default_branch"], Value::Null, "{v}");
}

// ---- C-7: the suggested label ----------------------------------------------------------------

#[tokio::test]
async fn fix278_the_suggested_label_names_the_repository() {
    let w = world().await;
    w.git.add_repo("anthropics/skills");
    w.git.commit(
        "anthropics/skills",
        "main",
        &[
            put("skills/pdf/SKILL.md", md("d", "b")),
            put("skills/docx/SKILL.md", md("d", "b")),
            // One stray skill folder at the root is not another library.
            put("template/SKILL.md", md("d", "b")),
        ],
    );
    for address in [
        "https://git.example/anthropics/skills",
        "https://gitlab.example/anthropics/skills",
    ] {
        let c = w
            .check_repo(json!({"url": address, "path": "skills"}))
            .await;
        assert_eq!(c["suggested_label"], "anthropics/skills", "{address}: {c}");
    }
    // A nested group: the group that holds the repository.
    w.git.add_repo("top/team/tools");
    w.git.commit(
        "top/team/tools",
        "main",
        &[put("alpha/SKILL.md", md("d", "b"))],
    );
    let c = w
        .check_repo(json!({"url": url("top/team/tools"), "path": ""}))
        .await;
    assert_eq!(c["suggested_label"], "team/tools", "{c}");
    // Two folders that each hold several skills: the label says which one.
    w.git.add_repo("acme/many");
    w.git.commit(
        "acme/many",
        "main",
        &[
            put("a/one/SKILL.md", md("d", "b")),
            put("a/two/SKILL.md", md("d", "b")),
            put("b/three/SKILL.md", md("d", "b")),
            put("b/four/SKILL.md", md("d", "b")),
        ],
    );
    let c = w
        .check_repo(json!({"url": url("acme/many"), "path": "a"}))
        .await;
    assert_eq!(c["suggested_label"], "acme/many/a", "{c}");
    // The 80-character limit holds.
    let long = format!("acme/{}", "r".repeat(120));
    w.git.add_repo(&long);
    w.git
        .commit(&long, "main", &[put("alpha/SKILL.md", md("d", "b"))]);
    let c = w.check_repo(json!({"url": url(&long), "path": ""})).await;
    assert_eq!(
        c["suggested_label"].as_str().unwrap().chars().count(),
        80,
        "{c}"
    );
}

// ---- B-1: a changed source -------------------------------------------------------------------

/// `main` holds the pinned content; `next` differs in `alpha`. Returns `(lib, alpha, main sha)`.
async fn two_branches(w: &W) -> (Uuid, Uuid, String) {
    let sha = w.seed();
    w.git.commit(
        "acme/skills",
        "next",
        &[put("alpha/SKILL.md", md("Alpha next", "Alpha body next"))],
    );
    let lib = w.library().await;
    let alpha = add_org_id(w, lib, "alpha", &sha).await;
    // One pass so the head of the old source is the compared one.
    w.refresh(lib).await;
    assert_eq!(w.pending_rows(lib).await, 0);
    (lib, alpha, sha)
}

/// Both directions: switching to a branch where a skill differs proposes the update although
/// that head was seen at the change; switching back to the pinned commit drops the proposal at
/// once, it cannot be approved, and nothing is proposed again.
#[tokio::test]
async fn fix278_a_changed_source_is_compared_again_and_drops_old_proposals() {
    let w = world().await;
    let (lib, alpha, pinned) = two_branches(&w).await;

    let (s, v) = w.patch_library(lib, json!({"ref": "next"})).await;
    assert_eq!(s, 200, "{v}");
    w.refresh(lib).await;
    let p = w.pending(alpha).await;
    assert_eq!(
        p["update"]["kind"], "changed",
        "the new branch is compared: {p}"
    );
    assert_eq!(p["update"]["diff"]["body"]["new"], "Alpha body next\n");
    let uid = p["update"]["id"].as_str().unwrap().to_owned();
    let hash = p["update"]["content_sha256"].as_str().unwrap().to_owned();

    let before = w.stored(alpha).await;
    let (s, v) = w.patch_library(lib, json!({"ref": "main"})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["pending_updates"], 0, "{v}");
    // Gone as soon as the change commits, before any refresh.
    assert_eq!(w.pending(alpha).await["update"], Value::Null);
    let (s, v) = w.approve(alpha, &uid, &hash).await;
    assert_eq!((s, code(&v)), (409, "update_superseded"), "{v}");
    assert_eq!(
        w.stored(alpha).await,
        before,
        "a dropped proposal changes nothing"
    );
    assert_eq!(before.1.as_deref(), Some(pinned.as_str()));
    assert_eq!(w.audit_critical("skill.library.update_apply").await, 0);
    w.refresh(lib).await;
    assert_eq!(w.pending(alpha).await["update"], Value::Null);

    // The drop is audited, naming what was dropped.
    let meta: Vec<Value> = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'skill_library.update'
         AND metadata ? 'proposals_dropped' ORDER BY id",
    )
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    assert_eq!(meta.len(), 1, "{meta:?}");
    assert_eq!(meta[0]["proposals_dropped"], 1, "{meta:?}");
    assert_eq!(meta[0]["dropped"][0]["update_id"], uid.as_str(), "{meta:?}");
    assert_eq!(
        meta[0]["dropped"][0]["skill_id"],
        alpha.to_string(),
        "{meta:?}"
    );
}

/// A change of folder is a change of source; a rename, a pause or the same ref again is not.
#[tokio::test]
async fn fix278_only_a_change_of_what_is_read_drops_proposals() {
    let w = world().await;
    let (lib, alpha, _) = two_branches(&w).await;
    w.git.commit(
        "acme/skills",
        "main",
        &[put("alpha/SKILL.md", md("Alpha v2", "Alpha body v2"))],
    );
    w.refresh(lib).await;
    let uid = w.pending(alpha).await["update"]["id"].clone();
    assert!(uid.is_string());
    for body in [
        json!({"label": "renamed"}),
        json!({"status": "paused"}),
        json!({"status": "active"}),
        json!({"ref": ""}),
        json!({"path": ""}),
    ] {
        let (s, v) = w.patch_library(lib, body.clone()).await;
        assert_eq!(s, 200, "{body}: {v}");
        assert_eq!(w.pending(alpha).await["update"]["id"], uid, "{body}");
    }
    let (s, v) = w.patch_library(lib, json!({"path": "alpha"})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(w.pending(alpha).await["update"], Value::Null);
}

/// Instance libraries: each organization whose proposal was dropped gets its own audit row.
#[tokio::test]
async fn fix278_a_changed_instance_source_audits_the_drop_in_each_organization() {
    let w = world().await;
    let sha = w.seed();
    w.git.commit(
        "acme/skills",
        "next",
        &[put("alpha/SKILL.md", md("Alpha next", "Alpha body next"))],
    );
    let (s, v) = w
        .call(
            &w.root,
            Method::POST,
            "/v1/admin/skill-libraries",
            Some(json!({"label": "shared", "url": url("acme/skills")})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let lib: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let alpha = add_org_id(&w, lib, "alpha", &sha).await;
    let (s, v) = w
        .call(
            &w.root,
            Method::PATCH,
            &format!("/v1/skill-libraries/{lib}"),
            Some(json!({"ref": "next"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    w.refresh(lib).await;
    assert_eq!(w.pending_rows(lib).await, 1);
    let (s, v) = w
        .call(
            &w.root,
            Method::PATCH,
            &format!("/v1/skill-libraries/{lib}"),
            Some(json!({"ref": "main"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(w.pending_rows(lib).await, 0);
    let rows: Vec<(Option<Uuid>, Value)> = sqlx::query_as(
        "SELECT org_id, metadata FROM audit_log WHERE action = 'skill_library.update'
         AND metadata ? 'proposals_dropped' ORDER BY org_id NULLS FIRST",
    )
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].0, None, "the instance row: {rows:?}");
    assert_eq!(rows[0].1["proposals_dropped"], 1);
    assert!(rows[0].1.get("dropped").is_none(), "no organization's ids");
    assert_eq!(rows[1].0, Some(w.org), "{rows:?}");
    assert_eq!(rows[1].1["dropped"][0]["skill_id"], alpha.to_string());
}

/// Commits made reachable before a change of source are no longer accepted by an add.
#[tokio::test]
async fn fix278_a_changed_source_forgets_the_old_sources_heads() {
    let w = world().await;
    let sha = w.seed();
    let next = w.git.commit(
        "acme/skills",
        "next",
        &[put("alpha/SKILL.md", md("Alpha next", "Alpha body next"))],
    );
    let newer = w.git.commit(
        "acme/skills",
        "next",
        &[put("beta/SKILL.md", md("Beta next", "Beta body next"))],
    );
    let lib = w.library().await;
    let (s, v) = w.patch_library(lib, json!({"ref": "next"})).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w.patch_library(lib, json!({"ref": "main"})).await;
    assert_eq!(s, 200, "{v}");
    for old in [&next, &newer] {
        let (s, v) = w.add_org(lib, "alpha", old).await;
        assert_eq!((s, code(&v)), (422, "commit_not_in_library"), "{old}: {v}");
    }
    let (s, v) = w.add_org(lib, "alpha", &sha).await;
    assert_eq!(s, 201, "{v}");
}

// ---- B-1: races ------------------------------------------------------------------------------

/// An old proposal pending on `next`, ready to approve. `(lib, alpha, update id, hash)`.
async fn pending_on_next(w: &W) -> (Uuid, Uuid, String, String) {
    let (lib, alpha, _) = two_branches(w).await;
    let (s, v) = w.patch_library(lib, json!({"ref": "next"})).await;
    assert_eq!(s, 200, "{v}");
    w.refresh(lib).await;
    let p = w.pending(alpha).await;
    (
        lib,
        alpha,
        p["update"]["id"].as_str().unwrap().to_owned(),
        p["update"]["content_sha256"].as_str().unwrap().to_owned(),
    )
}

/// PATCH first: it has dropped the proposal but not committed when the approval arrives. The
/// approval waits on the proposal's row lock and then sees it dropped. The test holds the
/// organization-state table, which the PATCH writes after dropping and the approval never uses.
#[tokio::test]
async fn fix278_an_approval_that_waits_for_a_source_change_is_refused() {
    let w = world().await;
    let (lib, alpha, uid, hash) = pending_on_next(&w).await;
    let before = w.stored(alpha).await;
    let mut gate = w.h.pool().begin().await.unwrap();
    exec_in(
        &mut gate,
        "LOCK TABLE skill_library_org_state IN SHARE MODE",
    )
    .await;
    let patch = async { w.patch_library(lib, json!({"ref": "main"})).await };
    let rest = async {
        lock_waiters(w.h.pool(), 1).await;
        let approve = w.approve(alpha, &uid, &hash);
        let release = async {
            lock_waiters(w.h.pool(), 2).await;
            gate.commit().await.unwrap();
        };
        tokio::join!(approve, release).0
    };
    let ((ps, pv), (s, v)) = tokio::join!(patch, rest);
    assert_eq!(ps, 200, "{pv}");
    assert_eq!((s, code(&v)), (409, "update_superseded"), "{v}");
    assert_eq!(w.stored(alpha).await, before);
}

/// Approval first: it holds the skill and the proposal when the change arrives. The change waits
/// for it, and the next pass proposes the way back from the new source. The test holds the tier's
/// budget lock, which the approval takes after the proposal's row lock.
#[tokio::test]
async fn fix278_a_source_change_waits_for_an_approval_that_holds_the_proposal() {
    let w = world().await;
    let (lib, alpha, uid, hash) = pending_on_next(&w).await;
    let mut gate = w.h.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("skill_library_budget:{}:None:None", w.org))
        .execute(&mut *gate)
        .await
        .unwrap();
    let approve = w.approve(alpha, &uid, &hash);
    let rest = async {
        lock_waiters(w.h.pool(), 1).await;
        let patch = w.patch_library(lib, json!({"ref": "main"}));
        let release = async {
            lock_waiters(w.h.pool(), 2).await;
            gate.commit().await.unwrap();
        };
        tokio::join!(patch, release).0
    };
    let ((s, v), (ps, pv)) = tokio::join!(approve, rest);
    assert_eq!(s, 200, "the approval was first: {v}");
    assert_eq!(ps, 200, "{pv}");
    assert_eq!(w.stored(alpha).await.0, "Alpha body next\n");
    assert_eq!(w.pending_rows(lib).await, 0);
    w.refresh(lib).await;
    let p = w.pending(alpha).await;
    assert_eq!(
        p["update"]["diff"]["body"]["new"], "Alpha body v1\n",
        "the way back to what the library follows: {p}"
    );
}

/// A pass that read the old source must not write a proposal after the source changed. The test
/// holds the organization's proposal lock, which the pass takes right before it inserts.
#[tokio::test]
async fn fix278_a_refresh_of_the_old_source_proposes_nothing_after_a_change() {
    let w = world().await;
    let (lib, alpha, _) = two_branches(&w).await;
    w.git.commit(
        "acme/skills",
        "main",
        &[put("alpha/SKILL.md", md("Alpha v2", "Alpha body v2"))],
    );
    let mut gate = w.h.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("skill_library_pending:{}", w.org))
        .execute(&mut *gate)
        .await
        .unwrap();
    let pass = w.refresh(lib);
    let rest = async {
        lock_waiters(w.h.pool(), 1).await;
        // `next` is where the library now looks; the pass in flight read `main`.
        let (s, v) = w.patch_library(lib, json!({"ref": "next"})).await;
        assert_eq!(s, 200, "{v}");
        gate.commit().await.unwrap();
    };
    tokio::join!(pass, rest);
    assert_eq!(w.pending_rows(lib).await, 0, "nothing from the old source");
    // The change is still compared at once: the old pass did not postpone it or mark it done.
    let due: bool = sqlx::query_scalar(
        "SELECT next_refresh_at <= now() AND last_head_sha IS NULL FROM skill_libraries
         WHERE id = $1",
    )
    .bind(lib)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert!(due);
    refresh::run_once(&w.h.state).await.unwrap();
    assert_eq!(
        w.pending(alpha).await["update"]["diff"]["body"]["new"],
        "Alpha body next\n"
    );
}
