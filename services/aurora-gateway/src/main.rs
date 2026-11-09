//! aurora-gateway: proxy (4102, placeholder until M2) and internal/ops (4103).
#![forbid(unsafe_code)]

use aurora_config::GatewayConfig;
use aurora_core::service::{self, exit_config};
use aurora_gateway::ca::{self, Ca};
use aurora_gateway::decide::{PolicyHandler, PolicyHandlerParts};
use aurora_gateway::governance::{DbGovSource, Governance, instance_from_config};
use aurora_gateway::guard::Guard;
use aurora_gateway::health::Health;
use aurora_gateway::hold::Holds;
use aurora_gateway::instance;
use aurora_gateway::invalidation::Hub;
use aurora_gateway::plaintext::PlaintextCache;
use aurora_gateway::proxy::{Proxy, ProxyParts};
use aurora_gateway::proxy_tls::{
    ProxyTls, REFRESH_INTERVAL, SanInputs, cert_names, compose_addr_is_local, detect_namespace,
    name_covers, san_set, url_host,
};
use aurora_gateway::reqlog::{DbWriter, FLUSH_INTERVAL, RequestLog};
use aurora_gateway::resolve::{DbSource, Resolver};
use aurora_gateway::resolver::DbCredentialResolver;
use aurora_gateway::snapshot::SnapshotStore;
use aurora_gateway::status::Outage;
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
    let instance = service::init_telemetry(
        SERVICE,
        &cfg.telemetry.log_level,
        cfg.telemetry.otlp_endpoint.as_deref(),
    );
    tracing::info!(instance_id = %instance, "starting");
    let Command::Serve = cli.command else {
        return ExitCode::FAILURE;
    };
    let result = serve(cfg).await;
    service::telemetry_shutdown();
    match result {
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
    // (phase 3, RFE-014) One availability state for the process: `down` until a registry snapshot
    // is loaded, `degraded` while the database or the LISTEN connection is down.
    let snapshots = SnapshotStore::new();
    let health = {
        let s = snapshots.clone();
        Health::new(move || s.is_loaded())
    };
    hub.set_health(health.clone());
    hub.set_registry_poke(snapshots.poke());
    if cfg.gateway.resolve_cache_ttl_secs > 300 {
        tracing::warn!(
            resolve_cache_ttl_secs = cfg.gateway.resolve_cache_ttl_secs,
            "gateway.resolve_cache_ttl_secs is above 300 s: cached entries (and a revocation or \
             freeze that cannot arrive during an outage) stay in force that long"
        );
    }
    let resolver = Arc::new(
        DbCredentialResolver::new(pool.clone())
            .with_byo(aurora_llm::governance::ByoInstance::from_config(
                &cfg.governance.byo,
            ))
            .with_health(health.clone()),
    );
    hub.register(resolver.cache());
    // (phase 2, RFE-025) Live BYO tunnels are re-evaluated on every invalidation that concerns
    // them; registered after the identity cache so a re-resolve never sees a stale identity.
    let tunnels = aurora_gateway::byo::TunnelRegistry::new(resolver.clone(), &cfg.external_url);
    hub.register(tunnels.clone());
    let llm = Arc::new(aurora_llm::ProviderRegistry::from_config(&cfg.llm)?);
    let llm_hosts = llm.endpoint_hosts();
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
        aurora_secrets::StoreRegistry::from_config_with_tls(
            &cfg.secrets,
            keyring,
            cfg.tls.extra_ca_bundle.as_deref(),
        )?,
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
    let policy_resolver = Arc::new(
        Resolver::with_ttl(
            Arc::new(DbSource(pool.clone())),
            llm,
            Duration::from_secs(cfg.gateway.resolve_cache_ttl_secs),
        )
        .with_governance(governance.clone())
        .with_snapshots(snapshots.clone())
        .with_health(health.clone()),
    );
    hub.register(policy_resolver.cache());
    // `gateway.hookhost_url` (docs/15); bearer = the configured internal secret (W9).
    let hookhost_url = cfg.gateway.hookhost_url.clone();
    let ring = Arc::new(cfg.internal_ring.clone());
    let hook_bearer = ring.active_bearer();
    let hook_key_id = ring.active_id().to_owned();
    let minter: Arc<dyn aurora_gateway::mint::Minter> = Arc::new(
        aurora_gateway::mint::HttpMinter::new(&hookhost_url, &hook_bearer)
            .with_key_id(&hook_key_id),
    );
    let summarizer: Arc<dyn aurora_gateway::summarize_client::Summarizer> = Arc::new(
        aurora_gateway::summarize_client::HttpSummarizer::new(&hookhost_url, &hook_bearer)
            .with_key_id(&hook_key_id),
    );
    let tokens = Arc::new(
        aurora_gateway::refresh::TokenManager::new(pool.clone(), vault.clone(), Some(minter))
            .with_health(health.clone()),
    );
    let plaintext = Arc::new(PlaintextCache::new(vault).with_health(health.clone()));
    hub.register(plaintext.clone());
    let reqlog = RequestLog::new(Arc::new(DbWriter(pool.clone())));
    reqlog.set_health(health.clone());
    let (tx, rx) = tokio::sync::watch::channel(false);
    let instance_id = uuid::Uuid::now_v7();
    let holds = Arc::new(
        Holds::new(
            pool.clone(),
            hub.clone(),
            instance_id,
            Duration::from_secs(cfg.approvals.timeout_secs),
            Some(rx.clone()),
        )
        .with_max_timeout(Duration::from_secs(cfg.approvals.max_timeout_secs))
        .with_health(health.clone()),
    );
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
        .with_health(health.clone())
        .with_tokens(tokens)
        .with_summarizer(summarizer)
        .with_condition_body_limit(cfg.gateway.condition_body_bytes as usize),
    );
    let reqlog_task = tokio::spawn(reqlog.clone().run(FLUSH_INTERVAL, rx.clone()));
    let snapshot_task = tokio::spawn(snapshots.clone().run(pool.clone(), rx.clone()));
    let hub_task = tokio::spawn(hub.clone().run(pool.clone(), rx.clone()));
    let instance_task = tokio::spawn(instance::run(
        pool.clone(),
        instance_id,
        instance::HEARTBEAT_INTERVAL,
        rx.clone(),
    ));
    let probe_task = tokio::spawn(health.clone().run_probe(pool.clone(), rx.clone()));
    // (RFE-013, C-316) Token use is written in batches off the request path.
    let last_used_task = tokio::spawn(
        resolver
            .clone()
            .run_last_used(Duration::from_secs(10), rx.clone()),
    );
    let reconcile_task = tokio::spawn(
        holds
            .clone()
            .run_reconcile(Duration::from_secs(5), rx.clone()),
    );
    // Health and metrics are never served on the agent-reachable proxy port. (RFE-014) `/readyz`
    // is the three-state answer of the outage state machine; the shared ops router (healthz,
    // metrics, 404) answers everything else.
    let outage = Outage {
        health: health.clone(),
        hub: hub.clone(),
        reqlog: Some(reqlog.clone()),
    };
    let internal = aurora_gateway::status::readyz_router(outage.clone())
        .merge(aurora_gateway::status::router_with(ring, Some(outage)))
        .fallback_service(service::ops_router(metrics, Vec::new()))
        .layer(axum::middleware::from_fn(service::trace_context));
    let ca = Arc::new(Ca::load(Path::new(&cfg.gateway.ca_dir))?);
    let bind = cfg.server.bind.as_str();
    let internal_bind = cfg.internal_bind.as_str();
    let bind_ip: IpAddr = bind.parse()?;
    let internal_ip: IpAddr = internal_bind.parse()?;
    if let Some(w) = aurora_config::bind_overlap_warning(bind, internal_bind) {
        tracing::warn!(proxy_bind = bind, internal_bind, "{w}");
    }
    let mut own = vec![
        SocketAddr::new(bind_ip, PROXY_PORT),
        SocketAddr::new(internal_ip, INTERNAL_PORT),
    ];
    let db: PgConnectOptions = cfg.database_url.expose().parse()?;
    if let Ok(addrs) = tokio::net::lookup_host((db.get_host(), db.get_port())).await {
        own.extend(addrs);
    }
    // docs/08 §6 (LLM hosts): operator-configured endpoint hosts pass the destination guard even
    // when they resolve to private addresses (an in-cluster or compose LiteLLM).
    let mut allowed_private = cfg.gateway.allowed_private.clone();
    for h in &llm_hosts {
        let bare = h.rsplit_once(':').map_or(h.as_str(), |(b, _)| b).to_owned();
        if !bare.is_empty() && !allowed_private.contains(&bare) {
            allowed_private.push(bare);
        }
    }
    let guard = Arc::new(Guard::new(&allowed_private, own)?);
    let extra_ca_pem = match &cfg.tls.extra_ca_bundle {
        Some(p) => Some(std::fs::read(p)?),
        None => None,
    };
    // (phase 1, RFE-002) The proxy listener is TLS-only; startup fails (naming the key) when the
    // certificate cannot be established.
    let proxy_tls = match (
        &cfg.gateway.proxy_tls.cert_file,
        &cfg.gateway.proxy_tls.key_file,
    ) {
        (Some(cert), Some(key)) => {
            let tls = ProxyTls::operator(Path::new(cert), Path::new(key))?;
            if let Some(host) = url_host(&cfg.advertised_proxy_url) {
                let covered = tls
                    .current_chain()
                    .first()
                    .is_some_and(|c| cert_names(c).iter().any(|n| name_covers(n, &host)));
                if !covered {
                    tracing::warn!(
                        host,
                        "the operator proxy certificate does not cover the advertised proxy host"
                    );
                }
            }
            tls
        }
        _ => {
            let namespace = detect_namespace();
            let names = san_set(&SanInputs {
                advertised_proxy_url: &cfg.advertised_proxy_url,
                sandbox_proxy_url: &cfg.sandbox_proxy_url,
                extra_names: &cfg.gateway.proxy_tls.names,
                namespace: namespace.as_deref(),
                compose_addr_local: compose_addr_is_local(),
            });
            tracing::info!(sans = ?names, "proxy listener certificate issued by the gateway CA");
            ProxyTls::from_ca(ca.clone(), names)?
        }
    };
    let tls_task = tokio::spawn(proxy_tls.clone().run_refresh(REFRESH_INTERVAL, rx.clone()));
    // `gateway.trusted_proxies` is validated as CIDRs at startup (docs/15).
    let trusted_proxies = cfg
        .gateway
        .trusted_proxies
        .iter()
        .map(|c| aurora_core::cidr::Cidr::parse(c))
        .collect::<Result<Vec<_>, _>>()?;
    let proxy = Proxy::with_byo(
        ProxyParts {
            ca,
            tls: proxy_tls,
            resolver,
            handler,
            guard,
            header_timeout: Duration::from_secs(cfg.gateway.upstream_header_timeout_secs),
            extra_ca_pem,
            egress_proxy: cfg.gateway.egress_proxy.clone(),
        },
        aurora_gateway::proxy::ByoParts {
            trusted_proxies,
            external_url: cfg.external_url.clone(),
            tunnels: Some(tunnels),
            health: Some(health.clone()),
        },
    )?;
    let proxy_listener = service::bind(bind, PROXY_PORT).await?;
    let internal_listener = service::bind(internal_bind, INTERNAL_PORT).await?;
    let drain_health = health.clone();
    tokio::spawn(async move {
        service::shutdown_signal().await;
        drain_health.set_draining();
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
    let _ = tls_task.await;
    let _ = probe_task.await;
    let _ = last_used_task.await;
    let _ = reconcile_task.await;
    Ok(())
}
