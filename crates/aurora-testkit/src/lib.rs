//! aurora-testkit — shared test utilities (docs/tasks/BE-007.md, decision I-06).
//!
//! - [`TestDb`]: a uniquely named, migrated Postgres database per instance.
//!   Server resolution: `AURORA_TEST_DATABASE_URL`, then `DATABASE_URL`
//!   (process env, then repo-root `.env`). If neither is set, the machine's one
//!   shared test server is used (`scripts/dev/test-pg.sh`, FIX-255, D-491),
//!   started on first use. This crate never starts a server per process, and it
//!   refuses a URL on port 55432 (the owner's dev database).
//! - [`MockUpstream`]: wiremock-backed HTTP upstream. `wiremock` is re-exported.
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
use sqlx::Connection;
use sqlx::PgConnection;
use sqlx::PgPool;
use tokio::sync::OnceCell;

pub mod git_server;
pub mod harness_upstream;
pub mod llm;
pub mod oauth;
pub mod oidc;
pub mod push;
pub mod responses;
pub mod template;

pub use wiremock;

/// (FIX-101) The skip policy of env-gated suites. A suite that needs a service the developer may not have
/// (podman, Redis, Vault, the strict front doors) names its gate variable and calls this first:
///
/// - gate set (non-empty, not `0`): returns `true`, the suite runs;
/// - gate unset, `CI` not `true`: prints a `SKIPPED:` line and returns `false`, the caller returns early;
/// - gate unset and `CI=true`: **panics**. A suite the specifications cite as evidence must run in CI or
///   fail there, never pass by skipping (REVIEW-5 pattern 2).
///
/// The e2e scripts additionally fail on a `SKIPPED:` line when `CI=true` (`scripts/e2e/lib-skip.sh`).
#[allow(clippy::panic)]
pub fn require_or_skip(gate: &str) -> bool {
    match gate_decision(
        gate,
        std::env::var(gate).ok().as_deref(),
        std::env::var("CI").ok().as_deref(),
    ) {
        Ok(true) => true,
        Ok(false) => {
            eprintln!(
                "SKIPPED: {gate} is not set (this suite needs it; CI sets it and fails without it)"
            );
            false
        }
        Err(msg) => panic!("{msg}"),
    }
}

/// The pure decision behind [`require_or_skip`]: `Ok(true)` run, `Ok(false)` skip, `Err` fail.
pub fn gate_decision(gate: &str, value: Option<&str>, ci: Option<&str>) -> Result<bool, String> {
    if value.is_some_and(|v| !v.is_empty() && v != "0") {
        return Ok(true);
    }
    if ci == Some("true") {
        return Err(format!(
            "{gate} is not set but CI=true: this env-gated suite must run in CI, not skip. Set {gate} (and its service variables) in the workflow job, or take the suite out of the CI run on purpose (docs/18 §Skip policy)"
        ));
    }
    Ok(false)
}

/// Name of the env var that overrides the test server.
pub const TEST_DB_ENV: &str = "AURORA_TEST_DATABASE_URL";

/// How long a connection attempt keeps waiting for a free server slot (SQLSTATE 53300).
const CONNECT_SLOT_DEADLINE: Duration = Duration::from_secs(60);

/// Whether `e` is Postgres refusing a connection because `max_connections` is used up
/// (`53300 too_many_connections`, or `53400` when the role's own limit is).
fn is_no_free_slot(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if matches!(d.code().as_deref(), Some("53300" | "53400")))
}

/// Retry `attempt` while the server has no free connection slot, polling until
/// [`CONNECT_SLOT_DEADLINE`]. The test server is shared by every lane and by every nextest
/// process (each opens its own pool), so under load a connect can be refused for a moment
/// although nothing is wrong; any other error is returned at once.
async fn retry_no_free_slot<T, F, Fut>(mut attempt: F) -> sqlx::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = sqlx::Result<T>>,
{
    let deadline = tokio::time::Instant::now() + CONNECT_SLOT_DEADLINE;
    let mut wait = Duration::from_millis(25);
    loop {
        match attempt().await {
            Err(e) if is_no_free_slot(&e) && tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(Duration::from_millis(500));
            }
            other => return other,
        }
    }
}

/// `PgConnection::connect` that waits out a full server (see [`retry_no_free_slot`]).
pub async fn connect_admin(url: &str) -> sqlx::Result<PgConnection> {
    retry_no_free_slot(|| PgConnection::connect(url)).await
}

/// Env var overriding the per-`TestDb` pool size (VEL-005).
pub const TEST_POOL_SIZE_ENV: &str = "AURORA_TEST_POOL_SIZE";

/// Default connections per `TestDb` pool. Every test (and every nextest process) opens its own
/// pool against the shared test Postgres, so a smaller pool would allow
/// more parallel tests — but the `aurora-api` harness hands this same pool to the app under test
/// (workers, SSE, LISTEN, long polls), and at 2 connections 20 of its 899 tests starve and fail
/// (VEL-005 measurement, `docs/ops/build-performance.md` §7). So the default stays 5; set
/// `AURORA_TEST_POOL_SIZE` to experiment (e.g. a thread-count sweep on an idle machine).
const DEFAULT_TEST_POOL_SIZE: u32 = 5;

fn parse_pool_size(raw: Option<&str>) -> u32 {
    raw.and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(DEFAULT_TEST_POOL_SIZE)
}

fn test_pool_size() -> u32 {
    parse_pool_size(std::env::var(TEST_POOL_SIZE_ENV).ok().as_deref())
}

/// A uniquely named, migrated Postgres database. Dropped on `cleanup()` or `Drop`.
pub struct TestDb {
    server_url: String,
    name: String,
    url: String,
    pool: PgPool,
    cleaned: bool,
    /// Gateway login role created by [`TestDb::gateway_role_url`]; dropped with the database.
    gateway_role: Option<String>,
}

impl TestDb {
    /// Create and migrate a fresh database. Panics on failure (test helper).
    #[allow(clippy::panic)]
    pub async fn new() -> TestDb {
        match Self::try_new().await {
            Ok(db) => db,
            Err(e) => panic!("aurora-testkit: failed to create test database: {e:#}"),
        }
    }

    /// [`TestDb::new`] plus an owned handle to its pool: what a test that used
    /// `#[sqlx::test(migrator = "aurora_db::MIGRATOR")]` starts with. That attribute is banned
    /// (`scripts/guards/no-sqlx-test.py`): it reads `DATABASE_URL` and `.env` itself, past the
    /// URL rules of this crate. Keep the `TestDb` bound until the test ends
    /// (`let (_db, pool) = …`, never `let (_, pool)`): dropping it drops the database.
    pub async fn with_pool() -> (TestDb, PgPool) {
        let db = TestDb::new().await;
        let pool = db.pool.clone();
        (db, pool)
    }

    /// Like [`TestDb::with_pool`], but the database is empty: no migration is applied
    /// (`#[sqlx::test(migrations = false)]`). For tests that run migrations themselves.
    #[allow(clippy::panic)]
    pub async fn unmigrated_with_pool() -> (TestDb, PgPool) {
        match Self::try_create(false).await {
            Ok(db) => {
                let pool = db.pool.clone();
                (db, pool)
            }
            Err(e) => panic!("aurora-testkit: failed to create test database: {e:#}"),
        }
    }

    /// Fallible variant of [`TestDb::new`].
    pub async fn try_new() -> anyhow::Result<TestDb> {
        Self::try_create(true).await
    }

    async fn try_create(with_migrations: bool) -> anyhow::Result<TestDb> {
        let server_url = server_url().await?;
        let name = format!("aurora_test_{}", uuid::Uuid::now_v7().simple());

        let mut admin = connect_admin(&server_url)
            .await
            .context("connecting to test server")?;
        // VEL-003: clone the per-server template (migrated once) at
        // filesystem speed instead of replaying all migrations per test.
        // `AURORA_TEST_TEMPLATE_DB=0` reverts to the legacy path below.
        let migrated = if with_migrations && template::template_enabled() {
            let template = template::ensure_template(&server_url).await?;
            template::clone_from_template(&mut admin, template, &name).await?;
            true
        } else {
            sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\"")))
                .execute(&mut admin)
                .await
                .context("creating test database")?;
            false
        };
        admin.close().await.ok();

        let url = replace_db_name(&server_url, &name)?;
        let pool = retry_no_free_slot(|| {
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(test_pool_size())
                .connect(&url)
        })
        .await
        .context("connecting to test database")?;
        if with_migrations && !migrated {
            template::migrate_pool(&pool).await?;
        }
        Ok(TestDb {
            server_url,
            name,
            url,
            pool,
            cleaned: false,
            gateway_role: None,
        })
    }

    /// FIX-016: create a throwaway least-privilege login role, apply the gateway grants to it
    /// (`aurora_db::grants::apply_gateway_grants`, exactly what `aurora-api migrate` does) and
    /// return a connection URL for this database that logs in as that role. The role (cluster-wide)
    /// is dropped with the database. Needs `CREATEROLE` on the test server user.
    pub async fn gateway_role_url(&mut self) -> anyhow::Result<String> {
        let role = format!("aurora_test_gw_{}", uuid::Uuid::now_v7().simple());
        let password = uuid::Uuid::now_v7().simple().to_string();
        let mut admin = connect_admin(&self.server_url)
            .await
            .context("connecting to test server")?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE ROLE \"{role}\" LOGIN PASSWORD '{password}' NOSUPERUSER NOCREATEDB NOCREATEROLE"
        )))
        .execute(&mut admin)
        .await
        .context("creating gateway test role")?;
        admin.close().await.ok();
        self.gateway_role = Some(role.clone());
        aurora_db::grants::apply_gateway_grants(&self.pool, &role)
            .await
            .context("applying gateway grants")?;
        // postgres://user:pass@host... -> swap the credentials.
        let Some(scheme_end) = self.url.find("://").map(|i| i + 3) else {
            bail!("test database URL has no scheme");
        };
        let rest = &self.url[scheme_end..];
        let host_part = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
        Ok(format!(
            "{}{role}:{password}@{host_part}",
            &self.url[..scheme_end]
        ))
    }

    /// Pool connected to this database (migrations applied).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Connection URL of this database.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Database name (`aurora_test_<uuid simple>`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// URL of the server hosting this database (used for admin operations).
    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    /// Close the pool and drop the database (`WITH (FORCE)`).
    pub async fn cleanup(mut self) -> anyhow::Result<()> {
        self.cleaned = true;
        self.pool.close().await;
        let r = drop_database(&self.server_url, &self.name).await;
        if let Some(role) = self.gateway_role.take() {
            drop_role(&self.server_url, &role).await?;
        }
        r
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        if self.cleaned {
            return;
        }
        // Best effort: run the DROP on a dedicated thread with its own runtime,
        // so this works whether or not the caller's runtime is still alive.
        // (FIX-160) Wait until the DROP has finished, unless it is stuck on Postgres's
        // ProcSignalBarrier. While this blocks, the test's (current-thread) runtime is frozen, and
        // a pool connection it was opening stays in Postgres's `authentication` phase. Such a
        // backend never absorbs the barrier `DROP DATABASE` waits on, so a plain join stalled
        // every such test until `authentication_timeout` (60 s) with sqlx 0.9. When the barrier
        // wait is detected we return; the test's runtime ends, the half-open connection closes
        // and the DROP completes in the background (a DROP still pending when the process exits
        // is lost).
        let server_url = self.server_url.clone();
        let name = self.name.clone();
        let role = self.gateway_role.clone();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok()?;
            let r = rt.block_on(async {
                let r = drop_database_watched(&server_url, &name, tx.clone())
                    .await
                    .ok();
                if let Some(role) = role {
                    drop_role(&server_url, &role).await.ok();
                }
                r
            });
            let _ = tx.send(());
            r
        });
        // Done, or blocked on the barrier; the bound is only a safety net.
        let _ = rx.recv_timeout(std::time::Duration::from_secs(120));
    }
}

/// [`drop_database`], signalling `blocked` once the DROP has waited on `ProcSignalBarrier` for a
/// full second (it then still runs to completion).
async fn drop_database_watched(
    server_url: &str,
    name: &str,
    blocked: std::sync::mpsc::Sender<()>,
) -> anyhow::Result<()> {
    let mut admin = connect_admin(server_url)
        .await
        .context("connecting to test server for DROP")?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut admin)
        .await
        .context("reading the DROP session's pid")?;
    let watch = async {
        let Ok(mut probe) = connect_admin(server_url).await else {
            return std::future::pending::<()>().await;
        };
        let mut waited = 0;
        while waited < 10 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let wait: Option<String> =
                sqlx::query_scalar("SELECT wait_event FROM pg_stat_activity WHERE pid = $1")
                    .bind(pid)
                    .fetch_optional(&mut probe)
                    .await
                    .ok()
                    .flatten()
                    .flatten();
            waited = if wait.as_deref() == Some("ProcSignalBarrier") {
                waited + 1
            } else {
                0
            };
        }
        let _ = blocked.send(());
        probe.close().await.ok();
    };
    {
        let drop = sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"
        )))
        .execute(&mut admin);
        tokio::pin!(drop);
        tokio::select! {
            r = &mut drop => { r.context("dropping test database")?; }
            () = watch => { drop.await.context("dropping test database")?; }
        }
    }
    admin.close().await.ok();
    Ok(())
}

async fn drop_database(server_url: &str, name: &str) -> anyhow::Result<()> {
    let mut admin = connect_admin(server_url)
        .await
        .context("connecting to test server for DROP")?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"
    )))
    .execute(&mut admin)
    .await
    .context("dropping test database")?;
    admin.close().await.ok();
    Ok(())
}

/// Drop a role created by [`TestDb::gateway_role_url`] (after its database is gone).
async fn drop_role(server_url: &str, role: &str) -> anyhow::Result<()> {
    let mut admin = connect_admin(server_url)
        .await
        .context("connecting to test server for DROP ROLE")?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP ROLE IF EXISTS \"{role}\""
    )))
    .execute(&mut admin)
    .await
    .context("dropping gateway test role")?;
    admin.close().await.ok();
    Ok(())
}

/// Resolve the server URL: env var, then `.env`, then the machine's shared test server.
async fn server_url() -> anyhow::Result<String> {
    let dotenv = read_dotenv()?;
    resolve_server_url(|key| std::env::var(key).ok(), &dotenv, shared_server_url).await
}

/// The URL rules of [`server_url`]: the process environment wins over `.env`, and `shared` is
/// only called when neither names a server. A resolved URL on the owner's dev database port is
/// refused, wherever it came from.
async fn resolve_server_url<F, Fut>(
    env: impl Fn(&str) -> Option<String>,
    dotenv: &[(String, String)],
    shared: F,
) -> anyhow::Result<String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<String>>,
{
    const KEYS: [&str; 2] = [TEST_DB_ENV, "DATABASE_URL"];
    let from_env = KEYS.iter().find_map(|key| {
        env(key)
            .filter(|v| !v.is_empty())
            .map(|v| (format!("${key}"), v))
    });
    let from_dotenv = || {
        KEYS.iter().find_map(|key| {
            dotenv
                .iter()
                .find(|(k, _)| k == key)
                .filter(|(_, v)| !v.is_empty())
                .map(|(_, v)| (format!("{key} in .env"), v.clone()))
        })
    };
    let (source, url) = match from_env.or_else(from_dotenv) {
        Some(found) => found,
        None => ("the shared test server".to_owned(), shared().await?),
    };
    if is_dev_database(&url) {
        // The URL itself is not echoed: it may carry a password.
        bail!(
            "refusing the test database URL from {source}: port {DEV_DATABASE_PORT} is the \
             owner's dev database. Run tests with scripts/dev/test.sh (it uses the shared test \
             server of scripts/dev/test-pg.sh), or remove that setting."
        );
    }
    Ok(url)
}

/// Port of the owner's dev database (`just dev-db`), which no test may touch.
const DEV_DATABASE_PORT: u16 = 55432;

/// Whether `url` points at [`DEV_DATABASE_PORT`], by authority port or `?port=`. A URL that
/// does not parse counts if it mentions the port at all.
fn is_dev_database(url: &str) -> bool {
    let wanted = DEV_DATABASE_PORT.to_string();
    match url::Url::parse(url) {
        Ok(u) => {
            u.port() == Some(DEV_DATABASE_PORT)
                || u.query_pairs().any(|(k, v)| k == "port" && v == wanted)
        }
        Err(_) => url.contains(&wanted),
    }
}

/// The machine's one shared test Postgres (`scripts/dev/test-pg.sh`, D-491): started if it is
/// not running, under a machine-wide lock, so any number of test processes get the same server.
/// Resolved once per process.
async fn shared_server_url() -> anyhow::Result<String> {
    static SHARED_URL: OnceCell<String> = OnceCell::const_new();
    SHARED_URL
        .get_or_try_init(|| async {
            run_test_pg("up").await?;
            let url = run_test_pg("url").await?;
            Ok::<_, anyhow::Error>(url.trim().to_owned())
        })
        .await
        .cloned()
}

/// Run `scripts/dev/test-pg.sh <command>` and return what it prints.
async fn run_test_pg(command: &str) -> anyhow::Result<String> {
    let script: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "..",
        "scripts",
        "dev",
        "test-pg.sh",
    ]
    .iter()
    .collect();
    let out = tokio::process::Command::new("bash")
        .arg(&script)
        .arg(command)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .with_context(|| format!("running {} {command}", script.display()))?;
    if !out.status.success() {
        bail!(
            "neither {TEST_DB_ENV} nor DATABASE_URL is set, and the shared test server is not \
             available (`{} {command}`: {}): {}",
            script.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Parse the repo-root `.env` (KEY=VALUE lines; `#` comments; optional quotes).
fn read_dotenv() -> anyhow::Result<Vec<(String, String)>> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", "..", ".env"]
        .iter()
        .collect();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Ok(Vec::new()),
    };
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| {
            let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
            (k.trim().to_owned(), v.to_owned())
        })
        .collect())
}

/// Replace the database path segment of `base` with `name`, keeping any query.
pub(crate) fn replace_db_name(base: &str, name: &str) -> anyhow::Result<String> {
    let (main, query) = match base.split_once('?') {
        Some((m, q)) => (m, Some(q)),
        None => (base, None),
    };
    let Some(scheme_end) = main.find("://").map(|i| i + 3) else {
        bail!("test database URL has no scheme");
    };
    let host_end = main[scheme_end..]
        .find('/')
        .map(|i| scheme_end + i)
        .unwrap_or(main.len());
    let mut out = format!("{}/{}", &main[..host_end], name);
    if let Some(q) = query {
        out.push('?');
        out.push_str(q);
    }
    Ok(out)
}

/// Mock HTTP upstream backed by `wiremock::MockServer`.
pub struct MockUpstream {
    server: wiremock::MockServer,
}

impl MockUpstream {
    /// Start a new mock server on a random local port.
    pub async fn start() -> MockUpstream {
        MockUpstream {
            server: wiremock::MockServer::start().await,
        }
    }

    /// Base URI, e.g. `http://127.0.0.1:PORT`.
    pub fn uri(&self) -> String {
        self.server.uri()
    }

    /// Mount a route answering `method` `path` with `status` and a JSON `body`.
    pub async fn json(&self, method: &str, path: &str, status: u16, body: serde_json::Value) {
        wiremock::Mock::given(wiremock::matchers::method(method))
            .and(wiremock::matchers::path(path))
            .respond_with(
                wiremock::ResponseTemplate::new(status)
                    .set_body_json(body)
                    .insert_header("content-type", "application/json"),
            )
            .mount(&self.server)
            .await;
    }

    /// Requests received so far, in arrival order.
    pub async fn received(&self) -> Vec<wiremock::Request> {
        self.server.received_requests().await.unwrap_or_default()
    }
}

#[cfg(test)]
mod server_url_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{TEST_DB_ENV, resolve_server_url};

    const SHARED: &str = "postgres://aurora:aurora@127.0.0.1:55990/aurora";

    /// Resolve with a fake environment and `.env`; also returns how often the shared server
    /// was asked for. Never touches the real environment or podman.
    async fn resolve(
        env: &[(&str, &str)],
        dotenv: &[(&str, &str)],
    ) -> (anyhow::Result<String>, usize) {
        let calls = AtomicUsize::new(0);
        let dotenv: Vec<(String, String)> = dotenv
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        let result = resolve_server_url(
            |key| {
                env.iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| (*v).to_owned())
            },
            &dotenv,
            || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(SHARED.to_owned())
            },
        )
        .await;
        (result, calls.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn the_environment_wins_and_the_test_variable_comes_first() {
        let dotenv = [(TEST_DB_ENV, "postgres://h:5432/dotenv")];
        let (url, calls) = resolve(
            &[
                ("DATABASE_URL", "postgres://h:5432/plain"),
                (TEST_DB_ENV, "postgres://h:5432/test"),
            ],
            &dotenv,
        )
        .await;
        assert_eq!(url.expect("url"), "postgres://h:5432/test");
        assert_eq!(calls, 0);
        let (url, _) = resolve(&[("DATABASE_URL", "postgres://h:5432/plain")], &dotenv).await;
        assert_eq!(url.expect("url"), "postgres://h:5432/plain");
    }

    #[tokio::test]
    async fn dotenv_comes_next_and_empty_values_count_as_unset() {
        let (url, calls) = resolve(
            &[("DATABASE_URL", ""), (TEST_DB_ENV, "")],
            &[
                (TEST_DB_ENV, ""),
                ("DATABASE_URL", "postgres://h:5432/dotenv"),
            ],
        )
        .await;
        assert_eq!(url.expect("url"), "postgres://h:5432/dotenv");
        assert_eq!(calls, 0);
    }

    #[tokio::test]
    async fn without_any_url_the_shared_server_is_used() {
        let (url, calls) = resolve(&[], &[("OTHER", "x")]).await;
        assert_eq!(url.expect("url"), SHARED);
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn the_dev_database_is_refused_from_every_source() {
        for bad in [
            "postgres://aurora:s3cret@127.0.0.1:55432/aurora",
            "postgres://aurora:s3cret@localhost:55432/aurora",
            "postgresql://aurora:s3cret@[::1]:55432/aurora?sslmode=disable",
            "postgres://aurora:s3cret@localhost/aurora?port=55432",
            "not a url, port 55432, s3cret",
        ] {
            for (env, dotenv) in [
                (vec![(TEST_DB_ENV, bad)], vec![]),
                (vec![("DATABASE_URL", bad)], vec![]),
                (vec![], vec![("DATABASE_URL", bad)]),
            ] {
                let (result, calls) = resolve(&env, &dotenv).await;
                let message = result.expect_err(bad).to_string();
                assert!(message.contains("scripts/dev/test.sh"), "{message}");
                assert!(message.contains("55432"), "{message}");
                assert!(!message.contains("s3cret"), "{message}");
                assert_eq!(calls, 0, "a refused URL must not fall back");
            }
        }
    }

    #[tokio::test]
    async fn other_ports_and_lookalikes_are_accepted() {
        for good in [
            "postgres://aurora:aurora@127.0.0.1:5432/aurora",
            "postgres://aurora:aurora@127.0.0.1:55990/aurora",
            "postgres://aurora:pw55432@db.example/aurora_55432",
        ] {
            let (result, _) = resolve(&[("DATABASE_URL", good)], &[]).await;
            assert_eq!(result.expect(good), good);
        }
    }
}

#[cfg(test)]
mod pool_size_tests {
    use super::parse_pool_size;

    #[test]
    fn pool_size_defaults_to_five_and_honours_a_valid_override() {
        assert_eq!(parse_pool_size(None), 5);
        assert_eq!(parse_pool_size(Some("2")), 2);
        assert_eq!(parse_pool_size(Some(" 3 ")), 3);
        // Unusable values fall back to the default instead of a zero/garbage pool.
        assert_eq!(parse_pool_size(Some("0")), 5);
        assert_eq!(parse_pool_size(Some("many")), 5);
    }
}
