//! aurora: operator / developer CLI (docs/16 §W12).
#![forbid(unsafe_code)]

mod approvals;
mod forward;
mod plugin;
mod run;
mod token;

use std::io::{IsTerminal, Write};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use aurora_client::Client;
use aurora_client::config::{self, Config};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "aurora", version, about = "Aurora command-line client")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Log in with the device flow and store an API key.
    Login {
        /// Aurora external URL (default: the stored one).
        #[arg(long, env = "AURORA_URL")]
        url: Option<String>,
        /// Hostname shown on the approval page (default: this machine).
        #[arg(long)]
        hostname: Option<String>,
    },
    /// Remove the stored credentials.
    Logout,
    /// Show the authenticated identity.
    Whoami,
    /// Manage agents.
    Agents {
        #[command(subcommand)]
        cmd: AgentsCmd,
    },
    /// Run a command with an agent's proxy and CA environment.
    Run {
        #[arg(long)]
        agent: String,
        /// Environment variable holding the agent token.
        #[arg(long, default_value = token::TOKEN_ENV)]
        token_env: String,
        /// Run a loopback forwarder (plain HTTP on 127.0.0.1) that speaks TLS to the gateway, for
        /// clients that cannot use an HTTPS proxy (e.g. stock Java).
        #[arg(long)]
        forward: bool,
        /// Lifetime of the context credential minted for this run, in seconds (60..86400).
        /// A context credential is always used when the agent requires one (RFE-025).
        #[arg(long, default_value_t = run::DEFAULT_TTL_SECS, value_parser = clap::value_parser!(u32).range(run::TTL_RANGE))]
        ttl: u32,
        /// Attribute this run with a labelled context credential even when the agent does not
        /// require one.
        #[arg(long)]
        context: bool,
        #[arg(trailing_var_arg = true, required = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Plugin package development: new, build, validate, compile, test, sign, keygen, docs.
    Plugin {
        #[command(subcommand)]
        cmd: plugin::PluginCmd,
    },
    /// Approvals.
    Approvals {
        #[command(subcommand)]
        cmd: ApprovalsCmd,
    },
}

#[derive(Subcommand)]
enum AgentsCmd {
    List,
    /// Store a BYO agent token copied from the dashboard ("Connect" tab).
    /// Reads stdin (no echo on a terminal) or `--from-env`.
    Token {
        handle: String,
        /// Read the token from `$AURORA_AGENT_TOKEN`.
        #[arg(long)]
        from_env: bool,
    },
}

#[derive(Subcommand)]
enum ApprovalsCmd {
    Watch {
        /// Only this workspace (slug in your org).
        #[arg(long)]
        workspace: Option<String>,
        /// Stream JSON lines instead of prompting.
        #[arg(long)]
        json: bool,
    },
}

fn load() -> Result<(Config, std::path::PathBuf)> {
    let p = config::default_path()?;
    Ok((Config::load(&p)?, p))
}

/// The same server: equal after dropping a trailing `/` and ignoring case (scheme and host).
fn same_server(a: &str, b: &str) -> bool {
    a.trim_end_matches('/')
        .eq_ignore_ascii_case(b.trim_end_matches('/'))
}

fn default_hostname() -> Option<String> {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[tokio::main]
async fn main() {
    let code = match real_main().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

async fn real_main() -> Result<i32> {
    match Cli::parse().cmd {
        Cmd::Plugin { cmd } => plugin::run(cmd),
        Cmd::Login { url, hostname } => {
            let path = config::default_path()?;
            let url = match url {
                Some(u) => u,
                None => Config::load(&path)
                    .map(|c| c.url)
                    .map_err(|_| anyhow!("pass --url <external_url> (or set AURORA_URL)"))?,
            };
            let url = url.trim_end_matches('/').to_string();
            // https:// (or loopback) only: refused here, before any request is made.
            let client = Client::new(&url, None)?;
            let host = hostname.or_else(default_hostname);
            let st = client.auth_cli_start(host.as_deref()).await?;
            println!(
                "Open {} and enter the code:\n\n    {}\n",
                aurora_client::clean(&st.verification_url),
                aurora_client::clean(&st.user_code)
            );
            println!("Waiting for approval...");
            let t = client
                .poll_cli_token(
                    &st.device_code,
                    Duration::from_secs(st.interval.max(1)),
                    Duration::from_secs(5),
                    Duration::from_secs(st.expires_in),
                )
                .await?;
            // Stored agent tokens belong to the server that issued them: a login to a different
            // server drops them instead of sending them to the new URL later (FIX-118, PKG-6).
            let agents = match Config::load(&path) {
                Ok(c) if same_server(&c.url, &url) => c.agents,
                Ok(c) => {
                    if !c.agents.is_empty() {
                        println!(
                            "Dropped {} stored agent token(s) of {}.",
                            c.agents.len(),
                            aurora_client::clean(&c.url)
                        );
                    }
                    Default::default()
                }
                Err(_) => Default::default(),
            };
            Config {
                url,
                api_key: t.api_key,
                workspace: t.workspace.clone(),
                agents,
            }
            .save(&path)?;
            println!(
                "Logged in to workspace {}/{}",
                aurora_client::clean(&t.workspace.org_slug),
                aurora_client::clean(&t.workspace.slug)
            );
            Ok(0)
        }
        Cmd::Logout => {
            let p = config::default_path()?;
            let stored = Config::load(&p).ok();
            match std::fs::remove_file(&p) {
                Ok(()) => {
                    println!("Logged out: the local credentials file is deleted.");
                    // There is no endpoint to revoke the key a device login created (the CLI does
                    // not know its id), so say so instead of implying it is gone (FIX-118, PKG-7).
                    if let Some(c) = stored {
                        println!(
                            "The API key and {} stored agent token(s) were NOT revoked on {}: \
                             revoke them there (Workspace, API keys; the agent's token) if the \
                             file may have been copied.",
                            c.agents.len(),
                            aurora_client::clean(&c.url)
                        );
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => println!("Not logged in."),
                Err(e) => return Err(e.into()),
            }
            Ok(0)
        }
        Cmd::Whoami => {
            let (cfg, _) = load()?;
            let me = Client::new(&cfg.url, Some(cfg.api_key.clone()))?
                .me()
                .await?;
            println!(
                "user:      {}",
                aurora_client::clean(me.user.email.as_deref().unwrap_or(&me.user.id))
            );
            println!("url:       {}", cfg.url);
            println!(
                "workspace: {}/{} ({})",
                aurora_client::clean(&cfg.workspace.org_slug),
                aurora_client::clean(&cfg.workspace.slug),
                aurora_client::clean(&cfg.workspace.id)
            );
            Ok(0)
        }
        Cmd::Agents {
            cmd: AgentsCmd::List,
        } => {
            let (cfg, _) = load()?;
            let client = Client::new(&cfg.url, Some(cfg.api_key.clone()))?;
            for a in client.list_agents(&cfg.workspace.id).await? {
                let tok = if cfg.agents.contains_key(&a.handle) {
                    "token stored"
                } else {
                    "-"
                };
                println!(
                    "{:<24} {:<7} {:<14} {}",
                    aurora_client::clean(&a.handle),
                    aurora_client::clean(&a.kind),
                    tok,
                    aurora_client::clean(&a.name)
                );
            }
            Ok(0)
        }
        Cmd::Agents {
            cmd: AgentsCmd::Token { handle, from_env },
        } => {
            let (mut cfg, path) = load()?;
            let tok = if from_env {
                std::env::var(token::TOKEN_ENV)
                    .map_err(|_| anyhow!("{} is not set", token::TOKEN_ENV))?
            } else {
                token::read_token_interactive()?
            };
            token::store(&mut cfg, &path, &handle, tok.trim()).await?;
            println!(
                "Token for `{handle}` verified and stored in {}.",
                path.display()
            );
            Ok(0)
        }
        Cmd::Run {
            agent,
            token_env,
            forward,
            ttl,
            context,
            command,
        } => {
            let (cfg, _) = load()?;
            let agent_tok = token::resolve_for_run(std::env::var(&token_env).ok(), &cfg, &agent)?;
            let client = Client::new(&cfg.url, Some(agent_tok.clone()))?;
            let cc = client.agent_container_config().await?;
            for w in run::warnings(&cc) {
                eprintln!("aurora: {w}");
            }
            // (phase 2, RFE-025) One labelled context credential per run, used as the proxy
            // credential, when the agent requires one (or the caller asks for attribution).
            let tok = if run::needs_context(&cc) || context {
                client
                    .create_context_credential(&run::run_label(&command), ttl)
                    .await
                    .context("could not mint a context credential for this run")?
                    .credential
            } else {
                agent_tok
            };
            let dir = run::make_temp_dir()?;
            let result = async {
                let mut env = run::build_env(&cc, Some(&tok), &dir, &run::system_roots_pem())?;
                // Held for the child's lifetime; dropping it stops the forwarder.
                let _forwarder = if forward {
                    let url = forward::ProxyUrl::parse(&run::resolved_proxy_url(&cc, &tok))?;
                    let operator = cc.proxy_tls.as_deref() == Some("operator_certificate");
                    let up = forward::Upstream::new(&url, &cc.ca_pem, operator)?;
                    let f = forward::spawn(up).await?;
                    run::apply_forwarder(&mut env, &f.url());
                    Some(f)
                } else {
                    None
                };
                let (prog, args) = command.split_first().context("no command")?;
                let status = tokio::process::Command::new(prog)
                    .args(args)
                    .envs(&env)
                    .status()
                    .await
                    .with_context(|| format!("failed to run `{prog}`"))?;
                Ok::<i32, anyhow::Error>(run::exit_code(status))
            }
            .await;
            let _ = std::fs::remove_dir_all(&dir);
            result
        }
        Cmd::Approvals {
            cmd: ApprovalsCmd::Watch { workspace, json },
        } => {
            let (cfg, _) = load()?;
            let client = Client::new(&cfg.url, Some(cfg.api_key.clone()))?;
            let workspace_id = match workspace {
                Some(slug) => {
                    let r = client.resolve(&cfg.workspace.org_slug, Some(&slug)).await?;
                    Some(
                        r.workspace
                            .ok_or_else(|| anyhow!("workspace `{slug}` not found"))?
                            .id,
                    )
                }
                None => None,
            };
            if !json && !std::io::stdin().is_terminal() {
                bail!("interactive mode needs a terminal; use --json for streaming");
            }
            let opts = approvals::WatchOpts {
                json,
                workspace_id,
                max_rounds: None,
                wait_secs: 25,
                retry_delay: Duration::from_secs(3),
            };
            let stdin = std::io::stdin();
            let mut input = stdin.lock();
            let mut out = std::io::stdout();
            approvals::watch(&client, &opts, &mut input, &mut out).await?;
            out.flush()?;
            Ok(0)
        }
    }
}
