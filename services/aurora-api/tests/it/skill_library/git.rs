//! P5-SKLIB-1 (D-448; C-1123..C-1127; SEC-148): the API's own git reader against `git http-backend`
//! (every server configuration, every kind of ref, private repositories, redirects, the size caps,
//! corrupted answers), the check route, the token's life (sealed, replaced, removed, never shown)
//! and the ids of the blobs it pins. Child of `skill_library` so it shares the world.

use super::*;
use base64::Engine;

fn check_uri(w: &W) -> String {
    format!("/v1/orgs/{}/skill-libraries/check", w.org)
}

fn list_uri(w: &W) -> String {
    format!("/v1/orgs/{}/skill-libraries", w.org)
}

/// POST the check route as an org admin.
async fn check(w: &W, body: Value) -> (u16, Value) {
    w.call(&w.admin, Method::POST, &check_uri(w), Some(body))
        .await
}

fn message(v: &Value) -> String {
    v["error"]["message"].as_str().unwrap_or("").to_owned()
}

#[tokio::test]
async fn the_reader_works_against_every_server_configuration() {
    let w = world().await;
    let head = w.seed();
    // (protocol v2, filter, any-sha1-in-want): v2 + filter, v2 without filter, v0 + filter,
    // v0 without filter, v0 where a commit cannot be asked for by id (the whole-pack fallback)
    for (i, (v2, filter, any)) in [
        (true, true, true),
        (true, false, true),
        (false, true, true),
        (false, false, true),
        (false, true, false),
    ]
    .into_iter()
    .enumerate()
    {
        w.git.clear_overrides();
        w.git.set_v2(v2);
        w.git.set_allow_filter(filter);
        w.git.set_allow_any_sha1(any);
        w.git.clear_requests();
        let (s, v) = check(&w, json!({"url": url("acme/skills")})).await;
        assert_eq!(s, 200, "config {i}: {v}");
        assert_eq!(v["commit_sha"], head, "config {i}");
        assert_eq!(v["skills"]["found"], 2, "config {i}: {v}");
        assert_eq!(v["skills"]["valid"], 2, "config {i}: {v}");
        // the library itself, and a skill's content (blobs by id), under the same configuration
        let (s, v) = w
            .call(
                &w.admin,
                Method::POST,
                &list_uri(&w),
                Some(json!({"label": format!("c{i}"), "url": url("acme/skills")})),
            )
            .await;
        assert_eq!(s, 201, "config {i}: {v}");
        let lib = v["id"].as_str().unwrap().to_owned();
        let (s, v) = w
            .call(
                &w.admin,
                Method::GET,
                &format!("/v1/skill-libraries/{lib}/skills/alpha?commit_sha={head}"),
                None,
            )
            .await;
        assert_eq!(s, 200, "config {i}: {v}");
        assert_eq!(v["body"], "Alpha body v1\n", "config {i}");
        assert_eq!(v["files"][0]["content"], "alpha notes", "config {i}");
        let (s, _) = w
            .call(
                &w.admin,
                Method::DELETE,
                &format!("/v1/skill-libraries/{lib}"),
                None,
            )
            .await;
        assert_eq!(s, 204);
        if v2 {
            assert!(
                w.git
                    .requests()
                    .iter()
                    .all(|r| r.git_protocol.as_deref() == Some("version=2")),
                "config {i}: protocol v2 asked for every request"
            );
        }
    }
}

#[tokio::test]
async fn branches_tags_commits_and_the_default_branch_resolve() {
    let w = world().await;
    let main = w.seed();
    let dev = w.git.commit(
        "acme/skills",
        "dev",
        &[put("alpha/SKILL.md", md("Alpha dev", "dev body"))],
    );
    w.git.tag("acme/skills", "v1", &main, true); // annotated: peeled to the commit
    w.git.tag("acme/skills", "v0", &main, false); // lightweight
    let u = url("acme/skills");
    for (reference, kind, name, sha) in [
        (Some("main"), "branch", "main", &main),
        (Some("dev"), "branch", "dev", &dev),
        (Some("v1"), "tag", "v1", &main),
        (Some("v0"), "tag", "v0", &main),
        (Some(main.as_str()), "commit", main.as_str(), &main),
        (Some(dev.as_str()), "commit", dev.as_str(), &dev),
    ] {
        let (s, v) = check(&w, json!({"url": u, "ref": reference})).await;
        assert_eq!(s, 200, "{reference:?}: {v}");
        assert_eq!(v["resolved"]["kind"], kind, "{reference:?}: {v}");
        assert_eq!(v["resolved"]["name"], name, "{reference:?}: {v}");
        assert_eq!(v["commit_sha"], sha.as_str(), "{reference:?}: {v}");
    }
    // no ref: the repository's default branch (HEAD's symref), whatever it is called
    let (_, v) = check(&w, json!({"url": u})).await;
    assert_eq!(
        (
            v["resolved"]["name"].as_str(),
            v["resolved"]["default"].as_bool()
        ),
        (Some("main"), Some(true)),
        "{v}"
    );
    w.git.set_default_branch("acme/skills", "dev");
    let (_, v) = check(&w, json!({"url": u})).await;
    assert_eq!(
        (v["resolved"]["name"].as_str(), v["commit_sha"].as_str()),
        (Some("dev"), Some(dev.as_str())),
        "{v}"
    );
    // a ref that does not exist, and a commit id the server does not have
    for reference in ["nope", &"a".repeat(40)] {
        let (s, v) = check(&w, json!({"url": u, "ref": reference})).await;
        assert_eq!(
            (s, code(&v)),
            (422, "library_unreachable"),
            "{reference}: {v}"
        );
        assert!(
            message(&v).contains("The branch or tag was not found"),
            "{reference}: {v}"
        );
    }
}

#[tokio::test]
async fn a_folder_of_the_repository_is_the_library() {
    let w = world().await;
    w.git.add_repo("acme/mono");
    w.git.commit(
        "acme/mono",
        "main",
        &[
            put("top/SKILL.md", md("Top", "b")),
            put("team/agents/one/SKILL.md", md("One", "b")),
            put("team/agents/two/SKILL.md", md("Two", "b")),
        ],
    );
    let (s, v) = check(&w, json!({"url": url("acme/mono"), "path": "team/agents"})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["skills"]["found"], 2, "{v}");
    assert_eq!(v["skills"]["names"], json!(["one", "two"]), "{v}");
    assert_eq!(v["path"], "team/agents");
}

#[tokio::test]
async fn a_private_repository_needs_the_token_and_only_the_token() {
    let w = world().await;
    w.seed();
    w.git.require_token("x-access-token", "good-token");
    let u = url("acme/skills");
    let (s, v) = check(&w, json!({"url": u})).await;
    assert_eq!((s, code(&v)), (422, "library_unreachable"), "{v}");
    assert!(
        message(&v).contains("The repository needs an access token"),
        "{v}"
    );
    let (s, v) = check(&w, json!({"url": u, "token": "wrong-token"})).await;
    assert_eq!((s, code(&v)), (422, "library_unreachable"), "{v}");
    assert!(message(&v).contains("The access token was refused"), "{v}");
    assert!(!v.to_string().contains("wrong-token"), "{v}");
    let (s, v) = check(&w, json!({"url": u, "token": "good-token"})).await;
    assert_eq!(s, 200, "{v}");
    // a 403 without a token is the same fixed text; a 404 says the token may not read it
    w.git.clear_overrides();
    w.git.override_status("/info/refs", 403, None);
    let (_, v) = check(&w, json!({"url": u})).await;
    assert!(message(&v).contains("needs an access token"), "{v}");
    w.git.clear_overrides();
    w.git.override_status("/info/refs", 404, None);
    let (_, v) = check(&w, json!({"url": u, "token": "good-token"})).await;
    assert!(
        message(&v).contains("not found, or the token cannot read it"),
        "{v}"
    );
    // another Basic user name (GitLab: oauth2, Bitbucket: x-token-auth)
    w.git.clear_overrides();
    w.git.require_token("x-token-auth", "bb-token");
    let (s, _) = check(&w, json!({"url": u, "token": "bb-token"})).await;
    assert_eq!(s, 422, "the default user name is not x-token-auth");
    let (s, v) = check(
        &w,
        json!({"url": u, "token": "bb-token", "token_username": "x-token-auth"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
}

fn redirect_to(w: &W, from_repo: &str, to: &str) {
    w.git.redirect(
        &format!("{from_repo}/info/refs"),
        &format!("{GIT_ORIGIN}/{to}/info/refs?service=git-upload-pack"),
    );
}

#[tokio::test]
async fn redirects_are_followed_three_times_on_https_and_never_to_a_refused_address() {
    let w = world().await;
    w.seed();
    for h in ["h0", "h1", "h2", "h3", "h4"] {
        w.git.add_repo(&format!("acme/{h}"));
        w.git.commit(
            &format!("acme/{h}"),
            "main",
            &[put("alpha/SKILL.md", md("Alpha", "b"))],
        );
    }
    // h0 -> h1 -> h2 -> h3: three redirects, answered
    redirect_to(&w, "acme/h0", "acme/h1");
    redirect_to(&w, "acme/h1", "acme/h2");
    redirect_to(&w, "acme/h2", "acme/h3");
    let (s, v) = check(&w, json!({"url": url("acme/h0")})).await;
    assert_eq!(s, 200, "three redirects: {v}");
    // a fourth is refused with its fixed reason
    redirect_to(&w, "acme/h3", "acme/h4");
    let (s, v) = check(&w, json!({"url": url("acme/h0")})).await;
    assert_eq!((s, code(&v)), (422, "library_unreachable"), "{v}");
    assert!(message(&v).contains("redirected too many times"), "{v}");
    // an http: hop, a refused address, a private name literal, and a target that is not git
    w.git.clear_overrides();
    for (to, what) in [
        (
            "http://git.example/acme/skills/info/refs?service=git-upload-pack",
            "http:",
        ),
        (
            "https://10.0.0.5/acme/skills/info/refs?service=git-upload-pack",
            "private literal",
        ),
        (
            "https://127.0.0.1/acme/skills/info/refs?service=git-upload-pack",
            "loopback literal",
        ),
        (
            "https://169.254.169.254/latest/info/refs?service=git-upload-pack",
            "metadata address",
        ),
        (
            "https://[::1]/acme/skills/info/refs?service=git-upload-pack",
            "ipv6 loopback",
        ),
        ("https://git.example/somewhere/else", "not a git endpoint"),
        (
            "https://user:pw@git.example/acme/skills/info/refs?service=git-upload-pack",
            "credentials",
        ),
    ] {
        w.git.clear_overrides();
        w.git.redirect("acme/skills/info/refs", to);
        let (s, v) = check(&w, json!({"url": url("acme/skills")})).await;
        assert_eq!((s, code(&v)), (422, "library_unreachable"), "{what}: {v}");
        assert!(
            message(&v).contains("redirected to an address that is not allowed"),
            "{what}: {v}"
        );
        assert!(
            !v.to_string().contains("10.0.0.5") && !v.to_string().contains("pw@"),
            "{what}"
        );
    }
}

#[tokio::test]
async fn the_token_never_follows_a_redirect_to_another_origin() {
    let w = world().await;
    w.seed();
    w.git.add_repo("o/r");
    w.git
        .commit("o/r", "main", &[put("alpha/SKILL.md", md("Alpha", "b"))]);
    w.git.redirect(
        "acme/skills/info/refs",
        "https://other.example/o/r/info/refs?service=git-upload-pack",
    );
    let (s, v) = check(
        &w,
        json!({"url": url("acme/skills"), "token": "tok-for-the-library"}),
    )
    .await;
    assert_eq!(s, 200, "{v}");
    let reqs = w.git.requests();
    let want = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("x-access-token:tok-for-the-library")
    );
    let first: Vec<_> = reqs
        .iter()
        .filter(|r| r.target.starts_with("/acme/skills/"))
        .collect();
    let other: Vec<_> = reqs
        .iter()
        .filter(|r| r.target.starts_with("/o/r/"))
        .collect();
    assert_eq!(first.len(), 1, "one request to the library's own origin");
    assert_eq!(first[0].authorization.as_deref(), Some(want.as_str()));
    assert!(
        other.len() >= 2,
        "discovery and the reads happened on the other origin"
    );
    assert!(
        other.iter().all(|r| r.authorization.is_none()),
        "no Authorization crosses origins: {other:?}"
    );
}

#[tokio::test]
async fn answers_over_the_caps_or_corrupted_are_refused_with_fixed_reasons() {
    let w = world().await;
    w.seed();
    let u = url("acme/skills");
    // a pack bigger than LIBRARY_PACK_MAX (streamed, stopped at the cap)
    w.git
        .oversize(Some(aurora_core::limits::LIBRARY_PACK_MAX + 1));
    let (s, v) = check(&w, json!({"url": u})).await;
    assert_eq!((s, code(&v)), (422, "library_unreachable"), "{v}");
    assert!(message(&v).contains("larger than 32 MiB"), "{v}");
    w.git.clear_overrides();
    // objects that were not asked for (ids recomputed and compared)
    w.git.wrong_pack(true);
    let (s, v) = check(&w, json!({"url": u})).await;
    assert_eq!((s, code(&v)), (422, "library_unreachable"), "{v}");
    assert!(message(&v).contains("do not match what was asked"), "{v}");
    w.git.clear_overrides();
    // a 65 MiB file of zeros compresses to a small pack, but the whole-pack fallback (no filter)
    // inflates it past LIBRARY_INFLATE_MAX
    w.git.add_repo("acme/zeros");
    w.git.commit(
        "acme/zeros",
        "main",
        &[
            put("alpha/SKILL.md", md("Alpha", "b")),
            put("alpha/zero.bin", vec![0u8; 65 * 1024 * 1024]),
        ],
    );
    w.git.set_allow_filter(false);
    let (s, v) = check(&w, json!({"url": url("acme/zeros")})).await;
    assert_eq!((s, code(&v)), (422, "library_unreachable"), "{v}");
    assert!(message(&v).contains("too large"), "{v}");
    w.git.clear_overrides();
    // with the filter the big blob is never asked for: the skill is listed (its size unknown)
    let (s, v) = check(&w, json!({"url": url("acme/zeros")})).await;
    assert_eq!(s, 200, "{v}");
    // SHA-256 repositories
    w.git.add_repo_sha256("acme/sha256");
    let (s, v) = check(&w, json!({"url": url("acme/sha256")})).await;
    assert_eq!((s, code(&v)), (422, "library_unreachable"), "{v}");
    assert!(
        message(&v).contains("SHA-256 repositories are not supported"),
        "{v}"
    );
    // a server that is not git at all
    w.git.override_status("/info/refs", 200, None);
    let (s, _) = check(&w, json!({"url": u})).await;
    assert_eq!(s, 422);
}

#[tokio::test]
async fn a_slow_git_host_gives_up_in_time_with_a_fixed_reason() {
    let w = world().await;
    w.seed();
    w.git.slow("/info/refs", std::time::Duration::from_secs(60));
    let started = std::time::Instant::now();
    let (s, v) = check(&w, json!({"url": url("acme/skills")})).await;
    assert_eq!((s, code(&v)), (422, "library_unreachable"), "{v}");
    assert!(message(&v).contains("did not answer in time"), "{v}");
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
}

#[tokio::test]
async fn too_many_skills_in_one_library_are_cut_not_read() {
    let w = world().await;
    w.git.add_repo("acme/many");
    let n = aurora_core::limits::LIBRARY_BROWSE_MAX + 50;
    let changes: Vec<Change> = (0..n)
        .map(|i| put(&format!("s{i:03}/SKILL.md"), md(&format!("Skill {i}"), "b")))
        .collect();
    w.git.commit("acme/many", "main", &changes);
    let (s, v) = check(&w, json!({"url": url("acme/many")})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["truncated"], true, "{v}");
    assert_eq!(v["skills"]["found"], n, "{v}");
    assert!(
        v["skills"]["valid"].as_u64().unwrap() <= aurora_core::limits::LIBRARY_BROWSE_MAX as u64,
        "only the first {} are read: {v}",
        aurora_core::limits::LIBRARY_BROWSE_MAX
    );
    assert!(v["skills"]["names"].as_array().unwrap().len() <= 20);
}

#[tokio::test]
async fn the_check_stores_nothing_is_limited_and_only_admins_may_use_it() {
    let w = world().await;
    let head = w.seed();
    let (s, v) = check(&w, json!({"url": url("acme/skills")})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["url"], url("acme/skills"), "{v}");
    assert_eq!(v["repo"], "git.example/acme/skills");
    assert_eq!(v["commit_sha"], head);
    assert_eq!(v["resolved"]["default"], true);
    assert_eq!(v["suggested_label"], "skills");
    assert_eq!(v["path"], "");
    assert_eq!(v["truncated"], false);
    assert_eq!(v["skills"]["names"], json!(["alpha", "beta"]));
    // a page URL fills ref and path
    let (_, v2) = check(
        &w,
        json!({"url": format!("{GIT_ORIGIN}/acme/skills/-/tree/main/alpha")}),
    )
    .await;
    assert_eq!(
        (v2["ref"].as_str(), v2["path"].as_str()),
        (Some("main"), Some("alpha")),
        "{v2}"
    );
    for sql in [
        "SELECT count(*) FROM skill_libraries",
        "SELECT count(*) FROM skill_library_heads",
        "SELECT count(*) FROM skills",
        "SELECT count(*) FROM audit_log WHERE action LIKE 'skill_library%' OR action LIKE 'skill.library%'",
    ] {
        assert_eq!(w.count(sql).await, 0, "{sql}");
    }
    // not for members, not for strangers
    for who in [&w.member, &w.wsowner, &w.unbound] {
        let (s, _) = w
            .call(
                who,
                Method::POST,
                &check_uri(&w),
                Some(json!({"url": url("acme/skills")})),
            )
            .await;
        assert_eq!(s, 403);
    }
    let (s, _) = w
        .call(
            &w.nonmember,
            Method::POST,
            &check_uri(&w),
            Some(json!({"url": url("acme/skills")})),
        )
        .await;
    assert_eq!(s, 404);
    // 10 a minute per person (a token bucket: a slow machine may earn one back); two were used
    // above by the admin
    let mut allowed = 2;
    let mut limited = false;
    for _ in 0..12 {
        let (s, v) = check(&w, json!({"url": url("acme/skills")})).await;
        if s == 429 {
            assert_eq!(code(&v), "rate_limited", "{v}");
            limited = true;
            break;
        }
        assert_eq!(s, 200, "{v}");
        allowed += 1;
    }
    assert!(limited, "a check beyond the allowance is refused with 429");
    assert!(
        (10..=11).contains(&allowed),
        "{allowed} checks went through"
    );
    // the other admin still has their own allowance
    let (s, _) = w
        .call(
            &w.owner,
            Method::POST,
            &check_uri(&w),
            Some(json!({"url": url("acme/skills")})),
        )
        .await;
    assert_eq!(s, 200);
}

/// (SEC-148) A create is a repository read like a check, and so is a patch that re-reads: all of
/// them draw on the same per-person allowance (10 a minute), the answer is the same 429.
#[tokio::test]
async fn create_and_a_rereading_patch_share_the_checks_rate() {
    let w = world().await;
    w.seed();
    let list = list_uri(&w);
    // creates that fail at the read (the repository does not exist) still count
    let mut limited = None;
    for i in 0..13 {
        let (s, v) = w
            .call(
                &w.admin,
                Method::POST,
                &list,
                Some(json!({"label": format!("m{i}"), "url": url("acme/missing")})),
            )
            .await;
        if s == 429 {
            assert_eq!(code(&v), "rate_limited", "{v}");
            limited = Some(i);
            break;
        }
        assert_eq!((s, code(&v)), (422, "library_unreachable"), "{v}");
    }
    assert!(
        limited.is_some_and(|i| (10..=11).contains(&i)),
        "create is limited after the allowance: {limited:?}"
    );
    // a check by the same person is limited too: it is one allowance
    let (s, _) = check(&w, json!({"url": url("acme/skills")})).await;
    assert_eq!(s, 429);
    // a patch that re-reads (a new ref) draws on the owner's allowance; one that does not
    // (a rename) never does
    let owner_lib = {
        let (s, v) = w
            .call(
                &w.owner,
                Method::POST,
                &list,
                Some(json!({"label": "owned", "url": url("acme/skills"), "path": "alpha"})),
            )
            .await;
        assert_eq!(s, 201, "{v}");
        v["id"].as_str().unwrap().to_owned()
    };
    let one = format!("/v1/skill-libraries/{owner_lib}");
    let mut limited = false;
    for _ in 0..13 {
        let (s, v) = w
            .call(&w.owner, Method::PATCH, &one, Some(json!({"ref": "main"})))
            .await;
        if s == 429 {
            assert_eq!(code(&v), "rate_limited", "{v}");
            limited = true;
            break;
        }
        assert_eq!(s, 200, "{v}");
    }
    assert!(limited, "a re-reading patch is limited");
    let (s, v) = w
        .call(
            &w.owner,
            Method::PATCH,
            &one,
            Some(json!({"label": "renamed"})),
        )
        .await;
    assert_eq!(s, 200, "a patch that reads nothing is not limited: {v}");
}

/// (SEC-148) Folders nested deeper than `LIBRARY_TREE_DEPTH_MAX` refuse the read, over the wire.
#[tokio::test]
async fn a_repository_nested_too_deep_is_refused() {
    let w = world().await;
    w.git.add_repo("acme/deep");
    let deep = ["d"; aurora_core::limits::LIBRARY_TREE_DEPTH_MAX + 1].join("/");
    w.git.commit(
        "acme/deep",
        "main",
        &[
            put("alpha/SKILL.md", md("Alpha", "b")),
            put(&format!("{deep}/f.txt"), b"x".to_vec()),
        ],
    );
    let (s, v) = check(&w, json!({"url": url("acme/deep")})).await;
    assert_eq!((s, code(&v)), (422, "library_tree_too_large"), "{v}");
    // the folder that is shallow enough reads
    let (s, v) = check(&w, json!({"url": url("acme/deep"), "path": "alpha"})).await;
    assert_eq!(s, 200, "{v}");
}

#[tokio::test]
async fn the_token_is_sealed_replaced_removed_and_never_shown() {
    let w = world().await;
    let sha = w.seed();
    w.git.require_token("x-access-token", "canary-one");
    let list = list_uri(&w);
    let (s, v) = w
        .call(
            &w.admin,
            Method::POST,
            &list,
            Some(json!({"label": "priv", "url": url("acme/skills"), "token": "canary-one"})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let lib: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(v["token_set"], true);
    assert!(v["token_updated_at"].is_string());
    let one = format!("/v1/skill-libraries/{lib}");
    // sealed: the column is a reference, never the token, and the row holds no plaintext
    let token_ref: String =
        sqlx::query_scalar("SELECT token_ref FROM skill_libraries WHERE id = $1")
            .bind(lib)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(!token_ref.contains("canary-one"), "{token_ref}");
    let rowtext: String =
        sqlx::query_scalar("SELECT skill_libraries::text FROM skill_libraries WHERE id = $1")
            .bind(lib)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(!rowtext.contains("canary-one"), "{rowtext}");
    // replace with a token the server accepts: used at once (not a stale cache)
    w.git.require_token("x-access-token", "canary-two");
    let (s, v) = w
        .call(
            &w.admin,
            Method::PATCH,
            &one,
            Some(json!({"token": "canary-two"})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = w
        .call(&w.admin, Method::GET, &format!("{one}/skills"), None)
        .await;
    assert_eq!(s, 200, "the replaced token is the one in use: {v}");
    // a replacement that cannot read the repository is refused and the old one is kept
    let (s, v) = w
        .call(
            &w.admin,
            Method::PATCH,
            &one,
            Some(json!({"token": "canary-bad"})),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "library_unreachable"), "{v}");
    assert!(message(&v).contains("The access token was refused"), "{v}");
    w.refresh(lib).await;
    let row = w.library_row(lib).await;
    assert_eq!(
        (row["status"].as_str(), row["token_set"].as_bool()),
        (Some("active"), Some(true)),
        "{row}"
    );
    // an add and a refresh with it work
    let (s, v) = w.add_org(lib, "alpha", &sha).await;
    assert_eq!(s, 201, "{v}");
    // removing it (`null`): the repository is private, so the library can no longer be read
    let (s, v) = w
        .call(&w.admin, Method::PATCH, &one, Some(json!({"token": null})))
        .await;
    assert_eq!((s, code(&v)), (422, "library_unreachable"), "{v}");
    assert!(
        message(&v).contains("The repository needs an access token"),
        "{v}"
    );
    assert_eq!(
        w.library_row(lib).await["token_set"],
        true,
        "kept when the re-read fails"
    );
    // a read before saving is part of every change; the repository is public now: removed
    w.git.clear_overrides();
    let (s, v) = w
        .call(&w.admin, Method::PATCH, &one, Some(json!({"token": null})))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(w.library_row(lib).await["token_set"], false);
    let tr: Option<String> =
        sqlx::query_scalar("SELECT token_ref FROM skill_libraries WHERE id = $1")
            .bind(lib)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(tr.is_none(), "the sealed value is gone with the reference");
    // never anywhere: responses were checked above; audit rows carry set|replaced|removed only
    for canary in ["canary-one", "canary-two", "canary-bad"] {
        let hits: i64 = w
            .count(&format!(
                "SELECT count(*) FROM audit_log WHERE metadata::text LIKE '%{canary}%'"
            ))
            .await;
        assert_eq!(hits, 0, "{canary} is in no audit row");
        let hits: i64 = w
            .count(&format!(
                "SELECT count(*) FROM skill_libraries WHERE last_error LIKE '%{canary}%'"
            ))
            .await;
        assert_eq!(hits, 0, "{canary} is in no last_error");
    }
    let tokens: Vec<String> = sqlx::query_scalar(
        "SELECT metadata->>'token' FROM audit_log WHERE action IN ('skill_library.create','skill_library.update') AND metadata ? 'token' ORDER BY created_at",
    )
    .fetch_all(w.h.pool())
    .await
    .unwrap();
    assert_eq!(
        tokens,
        ["set", "replaced", "removed"],
        "audit says what happened, not the value"
    );
}

#[tokio::test]
async fn the_address_of_a_library_is_fixed_unless_it_has_none() {
    let w = world().await;
    w.seed();
    let lib = w.library().await;
    let one = format!("/v1/skill-libraries/{lib}");
    let (s, v) = w
        .call(
            &w.admin,
            Method::PATCH,
            &one,
            Some(json!({"url": url("acme/skills")})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "library_url_fixed"), "{v}");
    // a library migrated without an address: PATCH sets it once, reading the repository first
    sqlx::query("UPDATE skill_libraries SET url = NULL, repo = NULL, status = 'error', last_error = 'Enter the repository''s git address' WHERE id = $1")
        .bind(lib)
        .execute(w.h.pool())
        .await
        .unwrap();
    let (s, v) = w
        .call(
            &w.admin,
            Method::PATCH,
            &one,
            Some(json!({"url": "http://git.example/acme/skills"})),
        )
        .await;
    assert_eq!((s, code(&v)), (422, "library_url_invalid"), "{v}");
    let (s, v) = w
        .call(
            &w.admin,
            Method::PATCH,
            &one,
            Some(json!({"url": url("acme/skills")})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["url"], url("acme/skills"));
    assert_eq!(v["status"], "active", "{v}");
    assert!(v["last_error"].is_null(), "{v}");
    let (s, v) = w
        .call(
            &w.admin,
            Method::PATCH,
            &one,
            Some(json!({"url": url("acme/skills")})),
        )
        .await;
    assert_eq!((s, code(&v)), (409, "library_url_fixed"), "{v}");
}

/// The blob ids of a skill added before the switch to the reader are git's own ids: a refresh
/// compares them equal and proposes nothing.
#[tokio::test]
async fn blob_ids_are_gits_own_so_a_skill_pinned_earlier_compares_equal() {
    let w = world().await;
    let sha = w.seed();
    let lib = w.library().await;
    let (s, v) = w.add_org(lib, "alpha", &sha).await;
    assert_eq!(s, 201, "{v}");
    let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    let blobs: Value = sqlx::query_scalar("SELECT library_blobs FROM skills WHERE id = $1")
        .bind(id)
        .fetch_one(w.h.pool())
        .await
        .unwrap();
    for (file, repo_path) in [
        ("SKILL.md", "alpha/SKILL.md"),
        ("notes.txt", "alpha/notes.txt"),
    ] {
        assert_eq!(
            blobs[file].as_str(),
            Some(w.git.blob_id("acme/skills", "main", repo_path).as_str()),
            "{file}: the stored id is the id git itself serves"
        );
    }
    // a row seeded with those ids (as before the switch) is not changed by a refresh
    w.refresh(lib).await;
    assert_eq!(
        w.count("SELECT count(*) FROM skill_library_updates").await,
        0,
        "no proposal for an unchanged skill"
    );
    // a push that does not touch alpha does not change it either, but one that does, does
    w.git.commit(
        "acme/skills",
        "main",
        &[put("beta/SKILL.md", md("Beta", "v2"))],
    );
    w.refresh(lib).await;
    assert_eq!(
        w.count("SELECT count(*) FROM skill_library_updates").await,
        0
    );
    w.git.commit(
        "acme/skills",
        "main",
        &[put("alpha/notes.txt", b"changed".to_vec())],
    );
    w.refresh(lib).await;
    assert_eq!(
        w.count("SELECT count(*) FROM skill_library_updates WHERE kind = 'changed'")
            .await,
        1
    );
}

/// SEC-148: the sealed token is opened for the reader in one place; nothing else mentions the
/// purpose, and the redacted `Debug` of the token type keeps it out of a log line.
#[test]
fn the_token_is_opened_only_by_the_library_service() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&root, &mut files);
    let mut mentions = Vec::new();
    let mut openers = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        let rel = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if text.contains("Purpose::SkillLibraryToken") {
            mentions.push(rel.clone());
        }
        if text.contains("open_token(") {
            openers.push(rel);
        }
    }
    mentions.sort();
    assert_eq!(
        mentions,
        ["domains/skill_libraries/service.rs", "secrets_migrate.rs"],
        "only the library service seals/opens the token; the migration job moves it"
    );
    openers.sort();
    assert!(
        openers
            .iter()
            .all(|f| f.starts_with("domains/skill_libraries/") || f == "domains/skills/library.rs"),
        "{openers:?}"
    );
}
