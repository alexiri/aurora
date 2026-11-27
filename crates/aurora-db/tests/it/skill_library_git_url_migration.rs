#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-SKLIB-1 (D-448(e), C-1127): `20261201000270_skill_library_git_url.sql` on a database that
//! already has libraries. GitHub and GitLab libraries get their clone URL on the host their
//! connection used (a registered instance's host and base path, else github.com / gitlab.com), a
//! duplicate url + ref + path keeps the url only on the oldest, and every other library (a third
//! app, a GitLab library whose connection is gone) becomes an `error` row asking for an address.
//! The unique index exists and the token columns keep both-or-neither.
use aurora_db::PgPool;
use aurora_testkit::TestDb;
use sqlx::migrate::Migrator;
use uuid::Uuid;

const VERSION: i64 = 20_261_201_000_270;

struct Lib {
    id: Uuid,
}

#[allow(clippy::too_many_arguments)]
async fn lib(
    pool: &PgPool,
    org: Uuid,
    label: &str,
    conn: Option<Uuid>,
    app: &str,
    repo: &str,
    reference: Option<&str>,
    created_offset: i32,
) -> Lib {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO skill_libraries (id, org_id, label, connection_id, app_id, repo, ref, path, created_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,'skills', now() + make_interval(secs => $8))",
    )
    .bind(id)
    .bind(org)
    .bind(label)
    .bind(conn)
    .bind(app)
    .bind(repo)
    .bind(reference)
    .bind(f64::from(created_offset))
    .execute(pool)
    .await
    .unwrap();
    Lib { id }
}

async fn conn(pool: &PgPool, org: Uuid, app: &str, instance: Option<Uuid>) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref, instance_id) \
         VALUES ($1,$2,NULL,$3,$3,'1.0.0','c','active','x',$4)",
    )
    .bind(id)
    .bind(org)
    .bind(app)
    .bind(instance)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn instance(pool: &PgPool, org: Uuid, app: &str, host: &str, base: Option<&str>) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO app_instances (id, org_id, package_id, app_id, host, base_path, label) \
         VALUES ($1,$2,$3,$3,$4,$5,'inst')",
    )
    .bind(id)
    .bind(org)
    .bind(app)
    .bind(host)
    .bind(base)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn row(pool: &PgPool, id: Uuid) -> (Option<String>, Option<String>, String, Option<String>) {
    sqlx::query_as("SELECT url, repo, status, last_error FROM skill_libraries WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn libraries_are_backfilled_with_their_git_url_and_the_rest_asks_for_an_address() {
    let (_db, pool) = TestDb::unmigrated_with_pool().await;
    let before: Vec<_> = aurora_db::MIGRATOR
        .iter()
        .filter(|m| m.version < VERSION)
        .cloned()
        .collect();
    assert!(
        aurora_db::MIGRATOR.iter().any(|m| m.version == VERSION),
        "the migration exists"
    );
    Migrator::with_migrations(before).run(&pool).await.unwrap();
    let org = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, slug, name, external_id) VALUES ($1, $2, 'o', $2)")
        .bind(org)
        .bind(format!("o-{org}"))
        .execute(&pool)
        .await
        .unwrap();
    let gh_conn = conn(&pool, org, "github", None).await;
    let ghes_inst = instance(&pool, org, "github", "ghe.example.org", None).await;
    let ghes_conn = conn(&pool, org, "github", Some(ghes_inst)).await;
    let gl_inst = instance(&pool, org, "gitlab", "git.example.org", Some("/gl")).await;
    let gl_inst_conn = conn(&pool, org, "gitlab", Some(gl_inst)).await;
    let gl_com_conn = conn(&pool, org, "gitlab", None).await;
    let forge_conn = conn(&pool, org, "forge", None).await;
    let github = lib(
        &pool,
        org,
        "gh",
        Some(gh_conn),
        "github",
        "acme/skills",
        Some("main"),
        0,
    )
    .await;
    // the same repository, ref and folder again (another label): only the oldest keeps the url
    let github_dup = lib(
        &pool,
        org,
        "gh2",
        Some(gh_conn),
        "github",
        "acme/skills",
        Some("main"),
        1,
    )
    .await;
    let ghes = lib(
        &pool,
        org,
        "ghes",
        Some(ghes_conn),
        "github",
        "team/skills",
        None,
        2,
    )
    .await;
    let gitlab_inst = lib(
        &pool,
        org,
        "gl",
        Some(gl_inst_conn),
        "gitlab",
        "grp/sub/proj",
        None,
        3,
    )
    .await;
    let gitlab_com = lib(
        &pool,
        org,
        "glcom",
        Some(gl_com_conn),
        "gitlab",
        "grp/proj",
        None,
        4,
    )
    .await;
    let gitlab_gone = lib(&pool, org, "glgone", None, "gitlab", "grp/gone", None, 5).await;
    let forge = lib(
        &pool,
        org,
        "forge",
        Some(forge_conn),
        "forge",
        "acme/skills",
        None,
        6,
    )
    .await;

    aurora_db::migrate(&pool).await.unwrap();

    let (url, repo, status, err) = row(&pool, github.id).await;
    assert_eq!(url.as_deref(), Some("https://github.com/acme/skills.git"));
    assert_eq!(repo.as_deref(), Some("github.com/acme/skills"));
    assert_eq!((status.as_str(), err), ("active", None));
    let (url, _, status, err) = row(&pool, github_dup.id).await;
    assert_eq!(url, None, "a duplicate keeps no url (the unique index)");
    assert_eq!(status, "error");
    assert_eq!(err.as_deref(), Some("Enter the repository's git address"));
    let (url, repo, ..) = row(&pool, ghes.id).await;
    assert_eq!(
        url.as_deref(),
        Some("https://ghe.example.org/team/skills.git")
    );
    assert_eq!(repo.as_deref(), Some("ghe.example.org/team/skills"));
    let (url, repo, ..) = row(&pool, gitlab_inst.id).await;
    assert_eq!(
        url.as_deref(),
        Some("https://git.example.org/gl/grp/sub/proj.git"),
        "a registered instance's host and base path"
    );
    assert_eq!(repo.as_deref(), Some("git.example.org/gl/grp/sub/proj"));
    let (url, ..) = row(&pool, gitlab_com.id).await;
    assert_eq!(url.as_deref(), Some("https://gitlab.com/grp/proj.git"));
    // a GitLab library whose connection is gone has no known host; a third app has no address
    for id in [gitlab_gone.id, forge.id] {
        let (url, _, status, err) = row(&pool, id).await;
        assert_eq!(url, None);
        assert_eq!(status, "error");
        assert_eq!(err.as_deref(), Some("Enter the repository's git address"));
    }

    // the unique index: the same organization, url, ref and folder cannot be added twice
    let dup = sqlx::query(
        "INSERT INTO skill_libraries (id, org_id, label, url, ref, path) \
         VALUES (gen_random_uuid(), $1, 'again', 'https://github.com/acme/skills.git', 'main', 'skills')",
    )
    .bind(org)
    .execute(&pool)
    .await;
    assert!(dup.is_err(), "skill_libraries_url_uq refuses a duplicate");
    // the token columns are both set or both null
    let half = sqlx::query("UPDATE skill_libraries SET token_ref = 'x' WHERE id = $1")
        .bind(github.id)
        .execute(&pool)
        .await;
    assert!(half.is_err(), "a token ref needs its timestamp");
    // the display form is longer than the old limit
    let long = format!("git.example.org/{}", "a".repeat(300));
    sqlx::query("UPDATE skill_libraries SET repo = $2 WHERE id = $1")
        .bind(github.id)
        .bind(&long)
        .execute(&pool)
        .await
        .unwrap();
}
