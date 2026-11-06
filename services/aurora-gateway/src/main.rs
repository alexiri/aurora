//! aurora-gateway: proxy (4102, placeholder until M2) and internal/ops (4103).
#![forbid(unsafe_code)]

use aurora_config::GatewayConfig;
use aurora_core::service::{self, exit_config, ready_check};
use aurora_gateway::ca::{self, Ca};
use aurora_gateway::decide::{PolicyHandler, PolicyHandlerParts};
use aurora_gateway::governance::{DbGovSource, Governance, instance_from_config};
use aurora_gateway::guard::Guard;
use aurora_gateway::hold::Holds;
use aurora_gateway::instance;
use aurora_gateway::invalidation::Hub;
use aurora_gateway::plaintext::PlaintextCache;
use aurora_gateway::proxy::{Proxy, ProxyParts};
use aurora_gateway::reqlog::{DbWriter, FLUSH_INTERVAL, RequestLog};
use aurora_gateway::resolve::{DbSource, Resolver};
use aurora_gateway::resolver::DbCredentialResolver;
use aurora_gateway::snapshot::SnapshotStore;
use base64::Engine;
use clap::{Parser, Subcommand};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

const SERVICE: &str = "aurora-gateway";
const PROXY_PORT: u16 = 4102;
const INTERNAL_PORT: u16 = 4103;

#[derive(Parser)]
#[command(name = "aurora-gateway", version, about = "Aurora egress gateway")]
struct Cli {
    /// Config file (else `AURORA_CONFIG`, else /etc/aurora/aurora.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the gateway.
    Serve,
    /// Mint the gateway CA (ca.pem + ca.key); refuses to overwrite.
    GenerateCa {
        #[arg(long)]
        out: PathBuf,
    },
    /// Offline fixture run against a compiled snapshot; prints JSON results.
    CheckSnapshot {
        #[arg(long)]
        snapshot: PathBuf,
        /// Directory of `*.yaml` fixture files.
        #[arg(long)]
        fixtures: PathBuf,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Command::GenerateCa { out } = &cli.command {
        let instance = uuid::Uuid::now_v7().simple().to_string();
        let r = ca::generate(&instance).and_then(|g| ca::write_new(out, &g));
        return match r {
            Ok(()) => {
                println!(
                    "wrote {}/ca.pem and {}/ca.key",
                    out.display(),
                    out.display()
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if let Command::CheckSnapshot { snapshot, fixtures } = &cli.command {
        return match aurora_gateway::check_snapshot::run_paths(snapshot, fixtures) {
            Ok(r) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&r.json).unwrap_or_default()
                );
                if r.ok {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(2)
            }
        };
    }
    let cfg: GatewayConfig = aurora_config::load(cli.config.as_deref(), std::env::vars())
        .and_then(|raw| raw.gateway_config())
        .unwrap_or_else(|e| exit_config(e));
    let instance = service::init_tracing(SERVICE, &cfg.telemetry.log_level);
    tracing::info!(instance_id = %instance, "starting");
    let Command::Serve = cli.command else {
        return ExitCode::FAILURE;
    };
    match serve(cfg).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "fatal");
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(cfg: GatewayConfig) -> Result<(), Box<dyn std::error::Error>> {
    let pool = PgPoolOptions::new()
        .max_connections(cfg.max_connections)
        .acquire_timeout(Duration::from_secs(3))
        .connect_lazy(cfg.database_url.expose())?;
    let metrics = service::install_metrics(SERVICE);
    let hub = Hub::new();
    let resolver = Arc::new(DbCredentialResolver::new(pool.clone()));
    hub.register(resolver.cache());
    let llm = Arc::new(aurora_llm::ProviderRegistry::from_config(&cfg.llm)?);
    let keys = cfg
        .keyring
        .keys
        .iter()
        .map(|k| {
            format!(
                "{}:{}",
                k.id,
                base64::engine::general_purpose::STANDARD.encode(k.key.expose())
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let keyring = Arc::new(aurora_crypto::Keyring::parse(&keys, &cfg.keyring.active)?);
    let vault = Arc::new(aurora_secrets::SecretVault::new(
        aurora_secrets::StoreRegistry::from_config(&cfg.secrets, keyring)?,
    ));
    let governance = Arc::new(
        Governance::new(
            instance_from_config(&cfg.governance, &llm),
            llm.clone(),
            Arc::new(DbGovSource(pool.clone())),
        )
        .with_recheck(Duration::from_secs(cfg.governance.origin_recheck_secs)),
    );
    // Registered before the resolver cache so a reload never sees stale governance.
    hub.register(governance.clone());
    let snapshots = SnapshotStore::new();
    let policy_resolver = Arc::new(
        Resolver::new(Arc::new(DbSource(pool.clone())), llm)
            .with_governance(governance.clone())
            .with_snapshots(snapshots.clone()),
    );
    hub.register(policy_resolver.cache());
    // `gateway.hookhost_url` (docs/15); bearer = the configured internal secret (W9).
    let hookhost_url = cfg.gateway.hookhost_url.clone();
    let hook_bearer =
        base64::engine::general_purpose::STANDARD.encode(cfg.internal_secret.expose());
    let minter: Arc<dyn aurora_gateway::mint::Minter> = Arc::new(
        aurora_gateway::mint::HttpMinter::new(&hookhost_url, &hook_bearer),
    );
    let summarizer: Arc<dyn aurora_gateway::summarize_client::Summarizer> = Arc::new(
        aurora_gateway::summarize_client::HttpSummarizer::new(&hookhost_url, &hook_bearer),
    );
    let tokens = Arc::new(aurora_gateway::refresh::TokenManager::new(
        pool.clone(),
        vault.clone(),
        Some(minter),
    ));
    let plaintext = Arc::new(PlaintextCache::new(vault));
    hub.register(plaintext.clone());
    let reqlog = RequestLog::new(Arc::new(DbWriter(pool.clone())));
    let (tx, rx) = tokio::sync::watch::channel(false);
    let instance_id = uuid::Uuid::now_v7();
    let holds = Arc::new(Holds::new(
        pool.clone(),
        hub.clone(),
        instance_id,
        Duration::from_secs(cfg.approvals.timeout_secs),
        Some(rx.clone()),
    ));
    let handler = Arc::new(
        PolicyHandler::new(PolicyHandlerParts {
            resolver: policy_resolver,
            plaintext,
            log: reqlog.clone(),
            external_url: cfg.external_url.clone(),
            holds: Some(holds.clone()),
            touch_pool: Some(pool.clone()),
        })
        .with_governance(governance)
        .with_tokens(tokens)
        .with_summarizer(summarizer),
    );
    let reqlog_task = tokio::spawn(reqlog.run(FLUSH_INTERVAL, rx.clone()));
    let snapshot_task = tokio::spawn(snapshots.clone().run(pool.clone(), rx.clone()));
    let hub_task = tokio::spawn(hub.clone().run(pool.clone(), rx.clone()));
    let instance_task = tokio::spawn(instance::run(
        pool.clone(),
        instance_id,
        instance::HEARTBEAT_INTERVAL,
        rx.clone(),
    ));
    let listener_hub = hub.clone();
    let listener_check = ready_check(move || {
        let connected = listener_hub.is_connected();
        async move {
            if connected {
                Ok(())
            } else {
                Err("invalidation listener not connected".to_owned())
            }
        }
    });
    let snapshot_check = ready_check(move || {
        let loaded = snapshots.is_loaded();
        async move {
            if loaded {
                Ok(())
            } else {
                Err("registry snapshot not loaded".to_owned())
            }
        }
    });
    let db_check = ready_check(move || {
        let pool = pool.clone();
        async move {
            sqlx::query("SELECT 1")
                .execute(&pool)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    });
    // Health and metrics are never served on the agent-reachable proxy port.
    let internal = service::ops_router(
        metrics,
        vec![
            ("database", db_check),
            ("invalidation", listener_check),
            ("registry", snapshot_check),
        ],
    );
    let ca = Arc::new(Ca::load(Path::new(&cfg.gateway.ca_dir))?);
    let bind = cfg.server.bind.as_str();
    let bind_ip: IpAddr = bind.parse()?;
    let mut own = vec![
        SocketAddr::new(bind_ip, PROXY_PORT),
        SocketAddr::new(bind_ip, INTERNAL_PORT),
    ];
    let db: PgConnectOptions = cfg.database_url.expose().parse()?;
    if let Ok(addrs) = tokio::net::lookup_host((db.get_host(), db.get_port())).await {
        own.extend(addrs);
    }
    let guard = Arc::new(Guard::new(&cfg.gateway.allowed_private, own)?);
    let extra_ca_pem = match &cfg.tls.extra_ca_bundle {
        Some(p) => Some(std::fs::read(p)?),
        None => None,
    };
    let proxy = Proxy::new(ProxyParts {
        ca,
        resolver,
        handler,
        guard,
        header_timeout: Duration::from_secs(cfg.gateway.upstream_header_timeout_secs),
        extra_ca_pem,
        egress_proxy: cfg.gateway.egress_proxy.clone(),
    })?;
    let proxy_listener = service::bind(bind, PROXY_PORT).await?;
    let internal_listener = service::bind(bind, INTERNAL_PORT).await?;
    tokio::spawn(async move {
        service::shutdown_signal().await;
        let _ = tx.send(true);
    });
    let mut rx2 = rx.clone();
    let internal_task = tokio::spawn(async move {
        axum::serve(internal_listener, internal)
            .with_graceful_shutdown(async move {
                let _ = rx2.wait_for(|s| *s).await;
            })
            .await
    });
    let mut rx3 = rx;
    proxy
        .run(proxy_listener, async move {
            let _ = rx3.wait_for(|s| *s).await;
        })
        .await;
    holds.abandon_all().await;
    internal_task.await??;
    let _ = hub_task.await;
    let _ = snapshot_task.await;
    let _ = instance_task.await;
    let _ = reqlog_task.await;
    Ok(())
}
