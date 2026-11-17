//! aurora-api: public API (4101), internal API (4111) and `migrate`.
#![forbid(unsafe_code)]

use aurora_config::ApiConfig;
use aurora_core::service::{self, exit_config, ready_check};
use axum::Router;
use clap::{Parser, Subcommand};
use sqlx::postgres::PgPoolOptions;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const SERVICE: &str = "aurora-api";
const PUBLIC_PORT: u16 = 4101;
const INTERNAL_PORT: u16 = 4111;

#[derive(Parser)]
#[command(name = "aurora-api", version, about = "Aurora control-plane API")]
struct Cli {
    /// Config file (else `AURORA_CONFIG`, else /etc/aurora/aurora.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the API server.
    Serve,
    /// Print the OpenAPI document (or write it to `--out`).
    Openapi {
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Apply database migrations (and gateway-role grants if configured).
    Migrate,
    /// Pre-provision an instance admin (binds at their first OIDC sign-in) and print the sign-in URL.
    BootstrapAdmin {
        #[arg(long)]
        email: String,
    },
    /// Signing-key / internal-secret rings (`docs/operator/key-rotation.md`).
    Keys {
        #[command(subcommand)]
        command: KeysCommand,
    },
    /// Instance-admin management.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
    /// Secret-store tooling (`docs/operator/vault.md`).
    Secrets {
        #[command(subcommand)]
        command: SecretsCommand,
    },
    /// Instance settings (`docs/operator/upgrades.md` §Break-glass): works with invalid stored
    /// settings. Exit 0 done, 2 refused (validation), 3 locked key, 4 database unreachable.
    Settings {
        #[command(subcommand)]
        command: SettingsCommand,
    },
    /// Web Push keys (C-833): rotate the generated VAPID key, show the key and device counts.
    /// `rotate-vapid` exits 3 when the key is operator-managed.
    Push {
        #[command(subcommand)]
        command: PushCommand,
    },
}

#[derive(Subcommand)]
enum PushCommand {
    /// Generate a new active VAPID key and retire the old one (generated keys only).
    RotateVapid {
        /// Report what would happen; change nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Key mode, keys and subscription counts; never key material.
    Status {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SettingsCommand {
    /// Effective values and their sources (secrets as set/unset).
    Show {
        section: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Remove dashboard values: a section, one key (`--key`) or one keyed-list entry (`--entry`).
    Reset {
        section: String,
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        entry: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Write the dashboard values (no secrets) as YAML or JSON to stdout.
    Export {
        #[arg(long, default_value = "yaml")]
        format: String,
        /// Comma-separated section ids.
        #[arg(long)]
        sections: Option<String>,
    },
    /// Apply a settings document (one transaction).
    Import {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        dry_run: bool,
        /// `replace` (default) or `merge`.
        #[arg(long, default_value = "replace")]
        mode: String,
    },
}

#[derive(Subcommand)]
enum SecretsCommand {
    /// Move every managed secret value into another store (`inline` or a Vault store id).
    Migrate {
        /// Target store id.
        #[arg(long)]
        to: String,
        /// Only one purpose: secret.value, connection.credentials, oauth_client.secret,
        /// integration.credentials, presence.credentials, webhook.verification.
        #[arg(long, value_parser = aurora_api::secrets_migrate::parse_purpose)]
        purpose: Option<aurora_secrets::Purpose>,
        /// Report what would move; write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Seconds to wait before discarding old values (gateway caches reload on commit).
        #[arg(long, default_value_t = 2)]
        discard_grace_secs: u64,
    },
    /// Re-seal every inline value that is not under the active encryption key (key rotation,
    /// `docs/operator/key-rotation.md`). Same guarantees as `migrate --to inline`.
    Rekey {
        /// Only one purpose (see `migrate --purpose`).
        #[arg(long, value_parser = aurora_api::secrets_migrate::parse_purpose)]
        purpose: Option<aurora_secrets::Purpose>,
        /// Report what would move; write nothing.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum KeyKind {
    Signing,
    Internal,
}

#[derive(Subcommand)]
enum KeysCommand {
    /// Print a fresh `<key_id>:<base64>` ring entry to append (never activates it).
    Generate {
        #[arg(long, value_enum)]
        kind: KeyKind,
        /// Key id (`[a-z0-9]{1,16}`); default: random `k` + 8 hex chars.
        #[arg(long)]
        id: Option<String>,
    },
    /// Print the key ids, fingerprints and active id of this process's configured rings.
    Status,
}

#[derive(Subcommand)]
enum AdminCommand {
    /// Clear the instance-admin flag of a user.
    Revoke { email: String },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Command::Openapi { out } = &cli.command {
        let json = aurora_api::openapi::document_json();
        return match out {
            Some(path) => match std::fs::write(path, json) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            None => {
                print!("{json}");
                ExitCode::SUCCESS
            }
        };
    }
    if let Command::Keys { command } = &cli.command {
        return keys_command(&cli, command);
    }
    // The configuration with its presence set (which keys the deployment sets: D-236(b)).
    let source = aurora_config::ConfigSource::load(cli.config.as_deref(), std::env::vars())
        .unwrap_or_else(|e| exit_config(e));
    let cfg: ApiConfig = source
        .raw()
        .and_then(|raw| raw.api_config())
        .unwrap_or_else(|e| exit_config(e));
    if let Command::Settings { command } = &cli.command {
        return settings_command(cfg, source, command).await;
    }
    if let Command::Push { command } = &cli.command {
        return push_command(cfg, source, command).await;
    }
    let instance = service::init_telemetry(
        SERVICE,
        &cfg.telemetry.log_level,
        cfg.telemetry.otlp_endpoint.as_deref(),
        cfg.telemetry.sample_ratio,
    );
    tracing::info!(instance_id = %instance, "starting");
    let result = match cli.command {
        Command::Serve => serve(cfg, source).await,
        Command::Migrate => migrate(cfg).await,
        Command::BootstrapAdmin { email } => bootstrap_admin(cfg, &email).await,
        Command::Admin {
            command: AdminCommand::Revoke { email },
        } => revoke_admin(cfg, &email).await,
        Command::Secrets {
            command:
                SecretsCommand::Migrate {
                    to,
                    purpose,
                    dry_run,
                    discard_grace_secs,
                },
        } => secrets_migrate(cfg, source, to, purpose, dry_run, discard_grace_secs).await,
        Command::Secrets {
            command: SecretsCommand::Rekey { purpose, dry_run },
        } => secrets_rekey(cfg, source, purpose, dry_run).await,
        Command::Openapi { .. }
        | Command::Keys { .. }
        | Command::Settings { .. }
        | Command::Push { .. } => Ok(()),
    };
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

/// `aurora-api push …` (C-833). Exit 0 done, 1 error (or push off), 3 `rotate-vapid` with an
/// operator-managed key.
async fn push_command(
    cfg: ApiConfig,
    source: aurora_config::ConfigSource,
    command: &PushCommand,
) -> ExitCode {
    let run = async {
        let pool = aurora_db::connect(cfg.database_url.expose(), 4)
            .await
            .map_err(|e| e.to_string())?;
        let state = secrets_state(&pool, cfg, source)
            .await
            .map_err(|e| e.to_string())?;
        match command {
            PushCommand::RotateVapid { dry_run } => {
                aurora_api::domains::push::cli::rotate(&state, *dry_run).await
            }
            PushCommand::Status { json } => aurora_api::domains::push::cli::status(&state, *json)
                .await
                .map(|()| 0),
        }
    };
    match run.await {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn migrate(cfg: ApiConfig) -> Result<(), Box<dyn std::error::Error>> {
    let pool = aurora_db::connect(cfg.database_url.expose(), 2).await?;
    aurora_db::migrate(&pool).await?;
    tracing::info!("migrations applied");
    if let Some(role) = cfg.gateway_role.as_deref() {
        aurora_db::grants::apply_gateway_grants(&pool, role).await?;
        tracing::info!(role, "gateway grants applied");
    }
    Ok(())
}

/// The state a secrets command works on: the configuration's stores plus the ones the dashboard
/// defined (FIX-124), so `--to <a dashboard store>` works. A database that cannot be read keeps
/// the configuration's stores.
async fn secrets_state(
    pool: &aurora_db::PgPool,
    cfg: ApiConfig,
    source: aurora_config::ConfigSource,
) -> Result<aurora_api::app::AppState, Box<dyn std::error::Error>> {
    let state = aurora_api::app::AppState::try_new(pool.clone(), cfg)?.with_config_source(source);
    if let Err(e) = state.settings.reload(pool, "cli").await {
        eprintln!("warning: stored settings not loaded ({e}); using the configuration's stores");
    }
    Ok(state)
}

async fn secrets_migrate(
    cfg: ApiConfig,
    source: aurora_config::ConfigSource,
    to: String,
    purpose: Option<aurora_secrets::Purpose>,
    dry_run: bool,
    discard_grace_secs: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let pool = aurora_db::connect(cfg.database_url.expose(), 4).await?;
    let state = secrets_state(&pool, cfg, source).await?;
    let mut opts = aurora_api::secrets_migrate::Options::new(&to);
    opts.keep_instance_settings_inline = true;
    opts.purpose = purpose;
    opts.dry_run = dry_run;
    opts.discard_grace = std::time::Duration::from_secs(discard_grace_secs);
    let summary = aurora_api::secrets_migrate::run(&pool, &state.vault, &opts).await?;
    print!("{}", summary.render(dry_run));
    if summary.failed() > 0 {
        return Err(format!("{} row(s) failed; see the log", summary.failed()).into());
    }
    Ok(())
}

async fn secrets_rekey(
    cfg: ApiConfig,
    source: aurora_config::ConfigSource,
    purpose: Option<aurora_secrets::Purpose>,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let pool = aurora_db::connect(cfg.database_url.expose(), 4).await?;
    let active = cfg.keyring.active.clone();
    let state = secrets_state(&pool, cfg, source).await?;
    let mut opts = aurora_api::secrets_migrate::Options::rekey(&active);
    opts.purpose = purpose;
    opts.dry_run = dry_run;
    let summary = aurora_api::secrets_migrate::run(&pool, &state.vault, &opts).await?;
    print!("{}", summary.render(dry_run));
    if summary.failed() > 0 {
        return Err(format!("{} row(s) failed; see the log", summary.failed()).into());
    }
    Ok(())
}

async fn bootstrap_admin(cfg: ApiConfig, email: &str) -> Result<(), Box<dyn std::error::Error>> {
    let pool = aurora_db::connect(cfg.database_url.expose(), 2).await?;
    let url = aurora_api::domains::admin::service::bootstrap_admin(
        &pool,
        &cfg.external_url,
        !cfg.auth.oidc_providers.is_empty(),
        email,
    )
    .await
    .map_err(|e| format!("{}: {}", e.code(), e.message()))?;
    println!("{url}");
    eprintln!(
        "Pre-provisioned {email} as instance admin. Sign in through your OIDC provider with that verified e-mail; the first sign-in binds the identity."
    );
    Ok(())
}

async fn revoke_admin(cfg: ApiConfig, email: &str) -> Result<(), Box<dyn std::error::Error>> {
    let pool = aurora_db::connect(cfg.database_url.expose(), 2).await?;
    let found = aurora_api::domains::admin::service::revoke_admin(&pool, email)
        .await
        .map_err(|e| e.message().to_owned())?;
    if !found {
        return Err(format!("no user with email {email}").into());
    }
    println!("instance-admin flag cleared for {email}");
    Ok(())
}

/// `aurora-api settings …` (C-784): through the settings service with source `cli`. It never
/// loads the stored settings strictly, so it starts when they are invalid.
async fn settings_command(
    cfg: ApiConfig,
    source: aurora_config::ConfigSource,
    command: &SettingsCommand,
) -> ExitCode {
    use aurora_api::domains::settings::cli;
    let pool = match aurora_db::connect(cfg.database_url.expose(), 4).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: database unreachable: {e}");
            return ExitCode::from(4);
        }
    };
    let state = match aurora_api::app::AppState::try_new(pool, cfg) {
        Ok(s) => s.with_config_source(source),
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let result = match command {
        SettingsCommand::Show { section, json } => {
            cli::show(&state, section.as_deref(), *json).await
        }
        SettingsCommand::Reset {
            section,
            key,
            entry,
            dry_run,
        } => cli::reset(&state, section, key.as_deref(), entry.as_deref(), *dry_run).await,
        SettingsCommand::Export { format, sections } => {
            let only: Option<Vec<String>> = sections.as_deref().map(|s| {
                s.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect()
            });
            cli::export(&state, format, only.as_deref())
        }
        SettingsCommand::Import {
            file,
            dry_run,
            mode,
        } => match std::fs::read_to_string(file) {
            Ok(text) => {
                let format = if file.extension().is_some_and(|e| e == "json") {
                    "json"
                } else {
                    "yaml"
                };
                cli::import(&state, &text, format, mode == "merge", *dry_run).await
            }
            Err(e) => {
                eprintln!("error: cannot read {}: {e}", file.display());
                return ExitCode::from(2);
            }
        },
    };
    match result {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {}", cli::render_error(&e));
            ExitCode::from(cli::exit_code(&e))
        }
    }
}

async fn serve(
    cfg: ApiConfig,
    source: aurora_config::ConfigSource,
) -> Result<(), Box<dyn std::error::Error>> {
    // Lazy pool: the process starts without the DB and `/readyz` reports 503.
    let pool = PgPoolOptions::new()
        .max_connections(cfg.max_connections)
        .acquire_timeout(Duration::from_secs(3))
        .connect_lazy(cfg.database_url.expose())?;
    let metrics = service::install_metrics(SERVICE);
    let check_pool = pool.clone();
    let db_check = ready_check(move || {
        let pool = check_pool.clone();
        async move {
            sqlx::query("SELECT 1")
                .execute(&pool)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    });
    let ops = service::ops_router(metrics, vec![("database", db_check)]);
    if cfg.auth.dev_idp.enabled {
        tracing::warn!(
            issuer = cfg.auth.dev_idp.issuer.as_deref().unwrap_or(""),
            "DEVELOPMENT ONLY: the development identity provider `dev` is enabled (auth.dev_idp); it signs users in without a password and must never be exposed"
        );
    }
    for w in aurora_config::llm_endpoint_warnings(&cfg.llm) {
        tracing::warn!("{w}");
    }
    let state =
        aurora_api::app::AppState::try_new(pool.clone(), cfg.clone())?.with_config_source(source);
    // (phase 5, SET-1, D-236) Mirror the configuration's locked keys, load the stored settings
    // and keep them live. Invalid stored settings stop the start naming the section, the key and
    // the reset command (D-238(c)); an unreachable database only delays the first load.
    aurora_api::domains::settings::handle::start(&state.settings, &pool).await?;
    state.settings.spawn_reload(pool.clone(), "api");
    // (FIX-083) IdP-linked groups of a provider that no longer syncs them (provider ids only).
    aurora_api::domains::groups::sync::warn_orphaned_providers(&pool, &cfg.auth.oidc_providers)
        .await;
    aurora_api::http::auth::spawn_revocation_listener(pool.clone(), state.session_cache.clone());
    if let Err(e) = aurora_api::domains::runners_admin::service::bootstrap_default(&state).await {
        tracing::warn!(error = %e, "bootstrap runner token not applied");
    }
    if let Err(e) = aurora_api::domains::relay::service::bootstrap_relay_token(&state).await {
        tracing::warn!(error = %e, "bootstrap relay token not applied");
    }
    if let Err(e) = aurora_api::domains::connectors::bootstrap_tokens(&state).await {
        tracing::warn!(error = %e, "bootstrap connector tokens not applied");
    }
    if let Err(e) = aurora_api::domains::connectors::bootstrap_host_token(&state).await {
        tracing::warn!(error = %e, "bootstrap connector host token not applied");
    }
    // (RFE-013, C-315) The instance maximum agent-token age applies to existing tokens at start.
    match aurora_api::domains::governance::service::retrofit_instance(&state).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(retrofitted = n, "agent token maximum age applied"),
        Err(e) => tracing::warn!(error = %e.message(), "agent token maximum age not applied"),
    }
    // (FU-P2-015b) Quota notices ride the RFE-011a e-mail channel when it is enabled.
    let quota_mail = aurora_api::domains::attachments::sweep::QuotaMail {
        mailer: state.mailer.clone(),
        external_url: cfg.external_url.clone(),
    };
    // (P4-CNT) `counters_prune`: dead rows of the shared-counter tables, whatever the backend
    // (rows stay behind after a switch back to `memory` and are removed here).
    aurora_api::jobs::spawn_counters_prune(pool.clone());
    // (P4-PRV-1) preview housekeeping: stale wake/start stamps and the shares gauge.
    aurora_api::jobs::spawn_preview_housekeeping(pool.clone());
    // (P4-SSH-2) the stale-session sweep of the SSH terminator plane.
    aurora_api::jobs::spawn_ssh_sweep(state.clone());
    // (P4-UTOK) discard the sealed values of finished sends and ended person tokens.
    aurora_api::jobs::spawn_user_token_sweep(state.clone());
    aurora_api::domains::push::jobs::spawn(state.clone());
    aurora_api::jobs::spawn(pool, state.ops_live.clone(), quota_mail);
    aurora_api::domains::approvals::notify::spawn(state.clone());
    // (P5-UX-16, C-983) The transcript events of approvals that belong to a conversation.
    aurora_api::domains::approvals::transcript::spawn(state.clone());
    // (FIX-025, D-158) No turn waits forever for a sandbox, runner connected or not.
    aurora_api::domains::hosted::readiness::spawn(state.clone());
    aurora_api::domains::connectors::inbox::spawn(state.clone());
    aurora_api::domains::channels::cards::fanout::spawn(state.clone());
    aurora_api::domains::channels::deprovision::spawn(state.clone());
    // (P4-GSL) git skill library refresh (lease per library; proposals only) and its retention.
    aurora_api::domains::skill_libraries::refresh::spawn(state.clone());
    // (P5-ASRC, D-250) addon source poll (lease per source; shared with the manual checks).
    aurora_api::domains::addon_sources::poll::spawn(state.clone());
    aurora_api::domains::plugins::scan::spawn(state.clone());
    // (P5-CHAT-4) index pre-existing messages for search (resumable; ends when complete).
    aurora_api::domains::conversations::search::spawn_backfill(state.clone());
    aurora_api::domains::plugins::registry::spawn_initial(state.clone());
    let internal = service::traced(
        aurora_api::domains::plugins::internal::router(state.clone()).fallback(service::not_found),
        service::INTERNAL_TRUST,
    );
    let public = service::traced(
        aurora_api::app::router(state, ops),
        aurora_api::http::trace_trust::PUBLIC_TRUST,
    );
    let bind = cfg.server.bind.as_str();
    let servers = vec![
        (service::bind(bind, PUBLIC_PORT).await?, public),
        (service::bind(bind, INTERNAL_PORT).await?, internal),
    ];
    serve_all(servers).await?;
    Ok(())
}

/// Like `service::serve_all`, but exposes the peer address (`ConnectInfo`) so the
/// client-IP middleware can resolve it.
async fn serve_all(servers: Vec<(tokio::net::TcpListener, Router)>) -> std::io::Result<()> {
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        service::shutdown_signal().await;
        let _ = tx.send(true);
    });
    let mut set = tokio::task::JoinSet::new();
    for (listener, app) in servers {
        if let Ok(addr) = listener.local_addr() {
            tracing::info!(%addr, "listening");
        }
        let mut rx = rx.clone();
        set.spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                let _ = rx.wait_for(|stop| *stop).await;
            })
            .await
        });
    }
    let mut result = Ok(());
    while let Some(joined) = set.join_next().await {
        let r = joined.unwrap_or_else(|e| Err(std::io::Error::other(e)));
        if r.is_err() && result.is_ok() {
            result = r;
        }
    }
    result
}

/// `aurora-api keys {generate,status}` (docs/08 §4, RFE-007). Prints ids and fingerprints only,
/// except `generate`, whose whole purpose is to print the new entry once.
fn keys_command(cli: &Cli, command: &KeysCommand) -> ExitCode {
    use base64::Engine as _;
    match command {
        KeysCommand::Generate { id, .. } => {
            let mut raw = [0u8; 32];
            let mut idb = [0u8; 4];
            if aws_lc_rs::rand::fill(&mut raw).is_err() || aws_lc_rs::rand::fill(&mut idb).is_err()
            {
                eprintln!("error: random number generator failure");
                return ExitCode::FAILURE;
            }
            let id = id.clone().unwrap_or_else(|| {
                let hex: String = idb.iter().map(|b| format!("{b:02x}")).collect();
                format!("k{hex}")
            });
            if !aurora_crypto::valid_ring_id(&id) {
                eprintln!("error: key id must match [a-z0-9]{{1,16}}");
                return ExitCode::FAILURE;
            }
            println!(
                "{id}:{}",
                base64::engine::general_purpose::STANDARD.encode(raw)
            );
            ExitCode::SUCCESS
        }
        KeysCommand::Status => {
            let rings = aurora_config::load(cli.config.as_deref(), std::env::vars())
                .and_then(|raw| raw.rings())
                .unwrap_or_else(|e| exit_config(e));
            for (name, ring) in [("signing_key", &rings.0), ("internal_secret", &rings.1)] {
                let (active, keys) = ring.status();
                println!("{name}: active={active}");
                for (id, fp) in keys {
                    let mark = if id == active { " (active)" } else { "" };
                    println!("  {id}  fingerprint={fp}{mark}");
                }
            }
            ExitCode::SUCCESS
        }
    }
}
