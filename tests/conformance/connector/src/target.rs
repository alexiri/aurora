//! The connector under test and the harness that wires it to a [`FakeCore`].

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use aurora_connector_fake::{FakeConnector, control};
use aurora_connector_sdk::{Connector, RetryPolicy, RuntimeConfig};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::driver::{PlatformDriver, Result};
use crate::fake_core::{FakeCore, eventually};
use crate::fake_driver::FakeDriver;

/// Anchor token the harness provisions (`aun_` + 32 hex).
pub const TOKEN: &str = "aun_0123456789abcdef0123456789abcdef";
/// The shared bot's platform user ref in the harness' integration.
pub const BOT_REF: &str = "B1";
/// Set to a connector binary to run the suite against it instead of in-process.
pub const BIN_ENV: &str = "CONFORMANCE_CONNECTOR_BIN";

/// How the connector under test is run.
pub enum ConnectorUnderTest {
    /// An in-process [`Connector`] driven by the SDK runtime.
    InProcess(Arc<dyn Connector>),
    /// A connector binary, configured through its environment: the harness sets
    /// `AURORA_CORE_URL` and `AURORA_CONNECTOR_TOKEN` plus `env`.
    Binary {
        path: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
}

/// Options for [`Harness::start_fake`].
#[derive(Debug, Clone, Default)]
pub struct StartOptions {
    /// Capabilities the fake connector declares (`None` = its default set).
    pub capabilities: Option<Vec<String>>,
}

/// A running fake core + connector under test + platform driver.
pub struct Harness {
    pub core: FakeCore,
    pub driver: Arc<dyn PlatformDriver>,
    /// The integration the core serves the connector.
    pub integration: Uuid,
    stop: Stopper,
}

enum Stopper {
    InProcess {
        tx: watch::Sender<bool>,
        runtime: JoinHandle<std::result::Result<(), aurora_connector_sdk::RuntimeError>>,
    },
    Binary(tokio::process::Child),
}

fn fast_runtime(url: &str) -> RuntimeConfig {
    let mut c = RuntimeConfig::new(url, TOKEN);
    c.retry = RetryPolicy {
        max_attempts: 3,
        initial: Duration::from_millis(50),
        max: Duration::from_millis(200),
    };
    c.config_interval = Some(Duration::from_millis(100));
    c.ops_wait_secs = Some(1);
    c
}

fn free_port() -> Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

impl Harness {
    /// Runs the suite's default target: `connectors/fake` in-process, or the
    /// binary named by `CONFORMANCE_CONNECTOR_BIN` (see `scripts/conformance.sh`).
    pub async fn start_fake(opts: StartOptions) -> Result<Self> {
        let core = FakeCore::start(TOKEN).await?;
        let integration = core.add_integration(BOT_REF);
        let (cut, driver): (ConnectorUnderTest, Arc<dyn PlatformDriver>) =
            match std::env::var(BIN_ENV).ok().filter(|b| !b.is_empty()) {
                Some(path) => {
                    let port = free_port()?;
                    let mut env = vec![
                        ("AURORA_FAKE_CONTROL".to_owned(), "true".to_owned()),
                        ("AURORA_FAKE_PORT".to_owned(), port.to_string()),
                    ];
                    if let Some(c) = &opts.capabilities {
                        env.push(("AURORA_FAKE_CAPABILITIES".into(), c.join(",")));
                    }
                    let driver = FakeDriver::new(&format!("http://127.0.0.1:{port}"));
                    (
                        ConnectorUnderTest::Binary {
                            path,
                            args: vec!["serve".into()],
                            env,
                        },
                        Arc::new(driver),
                    )
                }
                None => {
                    let fake = Arc::new(match &opts.capabilities {
                        Some(c) => {
                            let names: Vec<&str> = c.iter().map(String::as_str).collect();
                            FakeConnector::new(&names)
                        }
                        None => FakeConnector::default(),
                    });
                    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
                    let addr = l.local_addr()?;
                    let app = control::router(fake.clone());
                    tokio::spawn(async move {
                        let _ = axum::serve(l, app).await;
                    });
                    (
                        ConnectorUnderTest::InProcess(fake),
                        Arc::new(FakeDriver::new(&format!("http://{addr}"))),
                    )
                }
            };
        Self::start(core, integration, cut, driver).await
    }

    /// Starts any connector against `core`. `driver` is the platform driver for
    /// that connector's platform.
    pub async fn start(
        core: FakeCore,
        integration: Uuid,
        cut: ConnectorUnderTest,
        driver: Arc<dyn PlatformDriver>,
    ) -> Result<Self> {
        let stop = match cut {
            ConnectorUnderTest::InProcess(c) => {
                let (tx, rx) = watch::channel(false);
                let runtime =
                    tokio::spawn(aurora_connector_sdk::run(c, fast_runtime(&core.url()), rx));
                Stopper::InProcess { tx, runtime }
            }
            ConnectorUnderTest::Binary { path, args, env } => {
                let child = tokio::process::Command::new(&path)
                    .args(&args)
                    .env("AURORA_CORE_URL", core.url())
                    .env("AURORA_CONNECTOR_TOKEN", TOKEN)
                    .envs(env)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::inherit())
                    .kill_on_drop(true)
                    .spawn()
                    .with_context(|| format!("spawning {path}"))?;
                Stopper::Binary(child)
            }
        };
        let h = Self {
            core,
            driver,
            integration,
            stop,
        };
        if eventually(Duration::from_secs(20), || h.core.replica_id().map(|_| ()))
            .await
            .is_none()
        {
            bail!("the connector did not register within 20 s");
        }
        Ok(h)
    }

    /// Scenario 1 helper: approve the connector and wait until it serves the integration.
    pub async fn approve_and_wait(&self) -> Result<()> {
        self.core.approve();
        self.driver.wait_ready(Duration::from_secs(20)).await
    }

    /// Registers, approves, waits until serving, and seeds the bot user.
    pub async fn ready(opts: StartOptions) -> Result<Self> {
        let h = Self::start_fake(opts).await?;
        h.approve_and_wait().await?;
        h.driver
            .add_user(crate::driver::UserSpec::bot(BOT_REF))
            .await?;
        Ok(h)
    }

    /// Stops the connector cleanly (the runtime retires), then asserts the
    /// fake core saw no protocol violation. Returns the core for final checks.
    pub async fn finish(self) -> Result<FakeCore> {
        let Self { core, stop, .. } = self;
        match stop {
            Stopper::InProcess { tx, runtime } => {
                tx.send_replace(true);
                tokio::time::timeout(Duration::from_secs(10), runtime)
                    .await
                    .context("runtime did not stop")??
                    .context("runtime error")?;
            }
            Stopper::Binary(mut child) => {
                if let Some(pid) = child.id() {
                    let _ = std::process::Command::new("kill")
                        .args(["-TERM", &pid.to_string()])
                        .status();
                }
                if tokio::time::timeout(Duration::from_secs(10), child.wait())
                    .await
                    .is_err()
                {
                    let _ = child.kill().await;
                }
            }
        }
        let v = core.violations();
        if !v.is_empty() {
            bail!("protocol violations observed by the fake core: {v:#?}");
        }
        Ok(core)
    }
}
