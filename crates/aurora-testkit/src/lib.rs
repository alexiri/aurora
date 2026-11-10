//! aurora-testkit — shared test utilities (docs/tasks/BE-007.md, decision I-06).
//!
//! - [`TestDb`]: a uniquely named, migrated Postgres database per instance.
//!   Server resolution: `AURORA_TEST_DATABASE_URL`, then `DATABASE_URL`
//!   (process env, then repo-root `.env`). If neither is set, a podman
//!   `postgres:16` container is started once per process as a fallback.
//! - [`MockUpstream`]: wiremock-backed HTTP upstream. `wiremock` is re-exported.
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
use sqlx::Connection;
use sqlx::PgConnection;
use sqlx::PgPool;
use tokio::sync::OnceCell;

pub mod oauth;
pub mod oidc;
pub mod responses;

pub use wiremock;

/// Name of the env var that overrides the test server.
pub const TEST_DB_ENV: &str = "AURORA_TEST_DATABASE_URL";

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

    /// Fallible variant of [`TestDb::new`].
    pub async fn try_new() -> anyhow::Result<TestDb> {
        let server_url = server_url().await?;
        let name = format!("aurora_test_{}", uuid::Uuid::now_v7().simple());

        let mut admin = PgConnection::connect(&server_url)
            .await
            .context("connecting to test server")?;
        sqlx::query(&format!("CREATE DATABASE \"{name}\""))
            .execute(&mut admin)
            .await
            .context("creating test database")?;
        admin.close().await.ok();

        let url = replace_db_name(&server_url, &name)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .context("connecting to test database")?;
        aurora_db::MIGRATOR
            .run(&pool)
            .await
            .context("running migrations")?;
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
        let mut admin = PgConnection::connect(&self.server_url)
            .await
            .context("connecting to test server")?;
        sqlx::query(&format!(
            "CREATE ROLE \"{role}\" LOGIN PASSWORD '{password}' NOSUPERUSER NOCREATEDB NOCREATEROLE"
        ))
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
        let server_url = self.server_url.clone();
        let name = self.name.clone();
        let role = self.gateway_role.clone();
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok()?;
            rt.block_on(async {
                let r = drop_database(&server_url, &name).await.ok();
                if let Some(role) = role {
                    drop_role(&server_url, &role).await.ok();
                }
                r
            })
        });
        let _ = handle.join();
    }
}

async fn drop_database(server_url: &str, name: &str) -> anyhow::Result<()> {
    let mut admin = PgConnection::connect(server_url)
        .await
        .context("connecting to test server for DROP")?;
    sqlx::query(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
        .execute(&mut admin)
        .await
        .context("dropping test database")?;
    admin.close().await.ok();
    Ok(())
}

/// Drop a role created by [`TestDb::gateway_role_url`] (after its database is gone).
async fn drop_role(server_url: &str, role: &str) -> anyhow::Result<()> {
    let mut admin = PgConnection::connect(server_url)
        .await
        .context("connecting to test server for DROP ROLE")?;
    sqlx::query(&format!("DROP ROLE IF EXISTS \"{role}\""))
        .execute(&mut admin)
        .await
        .context("dropping gateway test role")?;
    admin.close().await.ok();
    Ok(())
}

/// Resolve the server URL: env var, then `.env`, then the podman fallback.
async fn server_url() -> anyhow::Result<String> {
    for key in [TEST_DB_ENV, "DATABASE_URL"] {
        if let Ok(v) = std::env::var(key)
            && !v.is_empty()
        {
            return Ok(v);
        }
    }
    let env = read_dotenv()?;
    for key in [TEST_DB_ENV, "DATABASE_URL"] {
        if let Some(v) = env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
            && !v.is_empty()
        {
            return Ok(v);
        }
    }
    container_url().await
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
fn replace_db_name(base: &str, name: &str) -> anyhow::Result<String> {
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

static CONTAINER_URL: OnceCell<String> = OnceCell::const_new();

/// Start (once per process) a podman postgres:16 container and return its URL.
async fn container_url() -> anyhow::Result<String> {
    CONTAINER_URL
        .get_or_try_init(start_container)
        .await
        .cloned()
}

async fn start_container() -> anyhow::Result<String> {
    eprintln!(
        "aurora-testkit: neither {TEST_DB_ENV} nor DATABASE_URL is set; \
         starting a podman postgres:16 container. Set {TEST_DB_ENV} to use an \
         existing server instead."
    );
    let name = format!("aurora-testkit-{}", uuid::Uuid::now_v7().simple());
    let run = tokio::process::Command::new("podman")
        .args([
            "run",
            "-d",
            "--rm",
            "--name",
            &name,
            "-p",
            "127.0.0.1::5432",
            "-e",
            "POSTGRES_USER=aurora",
            "-e",
            "POSTGRES_PASSWORD=aurora",
            "-e",
            "POSTGRES_DB=postgres",
            "docker.io/library/postgres:16",
        ])
        .output()
        .await
        .context("running podman (is podman installed?)")?;
    if !run.status.success() {
        bail!(
            "podman run failed: {}",
            String::from_utf8_lossy(&run.stderr)
        );
    }
    let port_out = tokio::process::Command::new("podman")
        .args(["port", &name, "5432/tcp"])
        .output()
        .await
        .context("podman port")?;
    let port: u16 = String::from_utf8_lossy(&port_out.stdout)
        .lines()
        .find_map(|l| l.rsplit(':').next().and_then(|p| p.trim().parse().ok()))
        .context("could not determine container host port")?;
    let url = format!("postgres://aurora:aurora@127.0.0.1:{port}/postgres");

    for _ in 0..120 {
        if PgConnection::connect(&url).await.is_ok() {
            eprintln!(
                "aurora-testkit: container {name} ready on port {port}; \
                       stop it with `podman rm -f {name}`"
            );
            return Ok(url);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    bail!("postgres container {name} did not become ready")
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

    /// Mount a route that only matches requests carrying header `name: value`.
    /// Answers 200 with an empty body; use [`MockUpstream::received`] to assert
    /// on what was sent.
    pub async fn expect_header(&self, name: &str, value: &str) {
        wiremock::Mock::given(wiremock::matchers::header(name, value))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&self.server)
            .await;
    }

    /// Requests received so far, in arrival order.
    pub async fn received(&self) -> Vec<wiremock::Request> {
        self.server.received_requests().await.unwrap_or_default()
    }
}
