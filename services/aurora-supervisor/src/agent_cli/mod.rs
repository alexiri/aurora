//! (P5-GUIDE-6, `docs/24 §10a`, C-653, D-296) `aurora-agent`: one command in the sandbox to list,
//! search, describe and call what the agent can reach. It is the `agent` subcommand of the
//! supervisor binary (`aurora-supervisor agent …`), installed as the wrapper
//! `/opt/aurora/bin/aurora-agent`.
//!
//! It holds no credential and adds no privilege (SEC-118): an app call goes through the proxy
//! environment it inherits, exactly like `curl`, and the gateway decides. A **target** is a
//! connected app id or the reserved id `aurora` (the platform tools, P5-GUIDE-7, over the tools
//! socket: [`platform`]). The two kinds are separate paths behind the same verbs
//! ([`TargetKind`]), [`platform::call`] beside [`call::call_app`].
//!
//! Exit codes (C-653): 0 success, 1 upstream HTTP error, 2 usage or validation, 3 an Aurora
//! gateway refusal, 4 tools-socket transport failure (platform target only).
pub mod api;
pub mod call;
pub mod files;
pub mod jsonpath;
pub mod platform;
pub mod preview;
pub mod request;
pub mod show;
pub mod text;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Subcommand};

use call::{CallOpts, Outcome};

/// The reserved target id of the platform tools (C-640, C-655).
pub const PLATFORM_TARGET: &str = "aurora";
/// Where the supervisor projects the apps' folders.
pub const APPS_DIR: &str = "/workspace/.aurora/apps";
/// The wrapper the image installs (`11 §Sandbox image`); its presence is `agent_cli` (C-653).
pub const WRAPPER: &str = "/opt/aurora/bin/aurora-agent";

#[derive(Args, Debug)]
pub struct AgentArgs {
    #[command(subcommand)]
    pub verb: Verb,
}

#[derive(Subcommand, Debug)]
pub enum Verb {
    /// List connected apps (and, from P5-GUIDE-7, the platform), or the operations of one.
    List { target: Option<String> },
    /// Search operations and skills by words (no network, no model).
    Search {
        #[arg(required = true)]
        words: Vec<String>,
        /// Only this target.
        #[arg(long = "in")]
        target: Option<String>,
        #[arg(long, default_value_t = 15)]
        limit: usize,
    },
    /// Describe one operation or skill: parameters, body, access, an example call.
    Describe { target: String, name: String },
    /// Call an operation: `call <app> <operation> [name=value | name:=<json> …]`, or
    /// `call <app> --path /x [--method M]` for anything not listed. With --path, `name=value`
    /// are query parameters and `name:=<json>` are JSON body fields.
    Call {
        target: String,
        /// The operation, then `name=value` / `name:=<json>` arguments.
        rest: Vec<String>,
        /// Account letter or connection id (apps with several accounts).
        #[arg(long)]
        account: Option<String>,
        /// Follow the operation's pagination and join the pages.
        #[arg(long)]
        all: bool,
        /// Most pages with --all (default 20, at most 200).
        #[arg(long)]
        max_pages: Option<u32>,
        /// A path on the account's host for anything not listed.
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        method: Option<String>,
        /// A whole request body: @file or - (stdin).
        #[arg(long)]
        body: Option<String>,
        /// Save the response here instead of /tmp/aurora-agent/.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Print the body itself instead of the summary.
        #[arg(long)]
        raw: bool,
    },
}

/// What a target is (C-653): an app through the proxy, or the platform through the tools socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    App,
    Platform,
}

#[must_use]
pub fn kind_of(target: &str) -> TargetKind {
    if target == PLATFORM_TARGET {
        TargetKind::Platform
    } else {
        TargetKind::App
    }
}

fn app_doc<'a>(loaded: &'a [api::AppFolder], target: &str) -> Result<&'a api::ApiDoc, Outcome> {
    match loaded.iter().find(|f| f.id == target) {
        None => Err(Outcome::usage(format!(
            "unknown target {target:?}; `aurora-agent list` shows the connected apps"
        ))),
        Some(f) => f
            .api
            .as_ref()
            .map_err(|e| Outcome::usage(format!("{target}: {e}"))),
    }
}

/// Options that do not apply to a platform target are refused before anything else (exit 2).
fn platform_options(opts: &CallOpts) -> Option<&'static str> {
    if opts.account.is_some() {
        Some("--account")
    } else if opts.all {
        Some("--all")
    } else if opts.max_pages.is_some() {
        Some("--max-pages")
    } else if opts.path.is_some() {
        Some("--path")
    } else if opts.method.is_some() {
        Some("--method")
    } else {
        None
    }
}

/// Executes one verb against the apps folder `apps_dir`.
pub async fn execute(env: &call::Env, apps_dir: &Path, verb: Verb) -> Outcome {
    match verb {
        Verb::List { target: None } => {
            let mut s = show::list_apps(&api::load_all(apps_dir));
            // Inside a conversation turn the platform is a target too.
            if env.session.is_some() {
                s.push_str(
                    "aurora — the platform tools (memory, skills, messaging, …): `aurora-agent list aurora`\n",
                );
            }
            Outcome::ok(s)
        }
        Verb::List { target: Some(t) } => match kind_of(&t) {
            TargetKind::Platform => match platform::fetch_tools(env).await {
                Ok(tools) => Outcome::ok(platform::list(&tools)),
                Err(o) => o,
            },
            TargetKind::App => {
                let loaded = api::load_all(apps_dir);
                match app_doc(&loaded, &t) {
                    Ok(d) => Outcome::ok(show::list_app(d)),
                    Err(o) => o,
                }
            }
        },
        Verb::Search {
            words,
            target,
            limit,
        } => {
            // The platform tools rank beside the apps' operations. Without a session they are
            // left out of an unscoped search; `--in aurora` says why it cannot answer.
            let platform_tools = if target.as_deref().is_none_or(|t| t == PLATFORM_TARGET) {
                match platform::fetch_tools(env).await {
                    Ok(t) => t,
                    Err(o) if target.is_some() => return o,
                    Err(_) => Vec::new(),
                }
            } else {
                Vec::new()
            };
            let loaded = api::load_all(apps_dir);
            Outcome::ok(show::search_with(
                &loaded,
                &platform_tools,
                &words,
                target.as_deref(),
                limit.clamp(1, 100),
            ))
        }
        Verb::Describe { target, name } => match kind_of(&target) {
            TargetKind::Platform => match platform::fetch_tools(env).await {
                Ok(tools) => match platform::describe(&tools, &name) {
                    Ok(s) => Outcome::ok(s),
                    Err(o) => o,
                },
                Err(o) => o,
            },
            TargetKind::App => {
                let loaded = api::load_all(apps_dir);
                match app_doc(&loaded, &target)
                    .and_then(|d| show::describe(d, &name).map_err(Outcome::usage))
                {
                    Ok(s) => Outcome::ok(s),
                    Err(o) => o,
                }
            }
        },
        Verb::Call {
            target,
            rest,
            account,
            all,
            max_pages,
            path,
            method,
            body,
            out,
            raw,
        } => {
            let opts = CallOpts {
                account,
                all,
                max_pages,
                path,
                method,
                body,
                out,
                raw,
            };
            match kind_of(&target) {
                TargetKind::Platform => match platform_options(&opts) {
                    Some(o) => Outcome::usage(format!(
                        "{o} does not apply to target `aurora` (it is for apps)"
                    )),
                    None => platform::call(env, &rest, &opts).await,
                },
                TargetKind::App => {
                    let (name, args) = if opts.path.is_some() {
                        // `call app --path /x [name=value …]`; a leading plain word is an operation
                        // name (its method becomes the default).
                        match rest.split_first() {
                            Some((first, tail)) if !first.contains('=') => {
                                (Some(first.as_str()), tail)
                            }
                            _ => (None, rest.as_slice()),
                        }
                    } else {
                        match rest.split_first() {
                            Some((first, tail)) => (Some(first.as_str()), tail),
                            None => (None, rest.as_slice()),
                        }
                    };
                    call::call_app(env, apps_dir, &target, name, args, &opts).await
                }
            }
        }
    }
}

/// The `agent` subcommand: runs the verb, prints, returns the exit code.
pub async fn run(args: AgentArgs) -> ExitCode {
    use std::io::Write;
    let env = match call::Env::production() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("aurora-agent: {e}");
            return ExitCode::from(call::EXIT_USAGE);
        }
    };
    let o = execute(&env, Path::new(APPS_DIR), args.verb).await;
    let _ = std::io::stdout().write_all(&o.out);
    if !o.err.is_empty() {
        eprint!("{}", o.err);
        if !o.err.ends_with('\n') {
            eprintln!();
        }
    }
    ExitCode::from(o.code)
}

/// `agent_cli` (C-653): the wrapper exists and is executable. Computed once at supervisor start.
#[must_use]
pub fn agent_cli_available(wrapper: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(wrapper).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
