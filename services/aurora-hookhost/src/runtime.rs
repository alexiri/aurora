//! wasmtime runtime: one engine, components compiled once per digest, a fresh
//! `Store` and instance per call, memory limiter, epoch-enforced wall clock and a
//! WASI 0.2 subset linked with empty sandboxes (decision I-22).

use std::sync::Arc;
use std::time::{Duration, Instant};

use aurora_core::limits::{HOOK_FETCH_MAX, HOOK_FETCH_TIMEOUT, HOOK_MEMORY};
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, ResourceLimiter, Store, Trap, UpdateDeadline};
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::fetch::{Egress, FetchFail, FetchPolicy};
use crate::redact::redact;

wasmtime::component::bindgen!({
    path: "../../crates/aurora-hook-wit/wit",
    world: "plugin",
    imports: { default: async },
    exports: { default: async },
    additional_derives: [serde::Serialize, serde::Deserialize],
});

pub use aurora::plugin::types;
pub use exports::aurora::plugin::hooks;

pub const LOG_LINES_MAX: usize = 100;
pub const LOG_LINE_MAX: usize = 1024;
const STDIO_CAP: usize = 64 * 1024;
const TICK: Duration = Duration::from_millis(10);

/// Stable error code plus a short message, as returned on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallError {
    pub code: &'static str,
    pub message: String,
}

impl CallError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub enum Invocation {
    Authorize(types::AuthorizeInput),
    Exchange(types::ExchangeInput),
    Mint(types::MintInput),
    Summarize(types::SummarizeInput),
}

pub enum Outcome {
    Authorize(types::AuthorizeOutput),
    Exchange(types::ExchangeOutput),
    Mint(types::MintOutput),
    Summarize(types::Summary),
}

/// Per-call parameters taken from the snapshot, never from the caller.
pub struct CallSpec {
    pub package_id: String,
    pub app_id: String,
    pub hook: &'static str,
    /// Egress hosts for this hook (empty for `approval.summarize`).
    pub egress: Vec<String>,
    /// Literal secret values to mask in logs.
    pub secrets: Vec<String>,
    pub wall: Duration,
}

/// Per-store limiter; `exceeded` records why a memory request was refused.
struct Limiter {
    exceeded: bool,
}

impl ResourceLimiter for Limiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > HOOK_MEMORY {
            self.exceeded = true;
            return Err(wasmtime::format_err!("hook memory limit exceeded"));
        }
        Ok(true)
    }
    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > 100_000 {
            self.exceeded = true;
            return Err(wasmtime::format_err!("hook table limit exceeded"));
        }
        Ok(true)
    }
    fn instances(&self) -> usize {
        8
    }
    fn tables(&self) -> usize {
        8
    }
    fn memories(&self) -> usize {
        4
    }
}

/// Store data.
pub struct Ctx {
    wasi: WasiCtx,
    table: ResourceTable,
    limiter: Limiter,
    egress: Egress,
    deadline: Instant,
    fetches: u32,
    denied: u32,
    logs: Vec<String>,
    log_overflow: bool,
    /// The OS random source could not be read during this call (FIX-107).
    rng_failed: bool,
    secrets: Vec<String>,
    package_id: String,
    app_id: String,
    hook: &'static str,
}

impl WasiView for Ctx {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl types::Host for Ctx {}

impl aurora::plugin::host::Host for Ctx {
    async fn fetch(
        &mut self,
        req: types::HttpRequest,
    ) -> Result<types::HttpResponse, types::FetchError> {
        if self.fetches >= HOOK_FETCH_MAX {
            self.denied += 1;
            return Err(types::FetchError::Denied("fetch limit reached".into()));
        }
        self.fetches += 1;
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        let timeout = HOOK_FETCH_TIMEOUT.min(remaining);
        if timeout.is_zero() {
            return Err(types::FetchError::Timeout);
        }
        let headers: Vec<(String, String)> =
            req.headers.into_iter().map(|k| (k.key, k.value)).collect();
        match self
            .egress
            .fetch(&req.method, &req.url, &headers, req.body, timeout)
            .await
        {
            Ok(r) => Ok(types::HttpResponse {
                status: r.status,
                headers: r
                    .headers
                    .into_iter()
                    .map(|(key, value)| types::Kv { key, value })
                    .collect(),
                body: r.body,
            }),
            Err(FetchFail::Denied(m)) => {
                self.denied += 1;
                Err(types::FetchError::Denied(m))
            }
            Err(FetchFail::Timeout) => Err(types::FetchError::Timeout),
            Err(FetchFail::TooLarge) => Err(types::FetchError::TooLarge),
            Err(FetchFail::Network(m)) => Err(types::FetchError::Network(m)),
        }
    }

    async fn log(&mut self, level: types::LogLevel, message: String) {
        if self.logs.len() >= LOG_LINES_MAX {
            self.log_overflow = true;
            return;
        }
        let line = redact(&message, &self.secrets, LOG_LINE_MAX);
        self.emit(level_str(level), &line);
        self.logs.push(line);
    }

    async fn random_bytes(&mut self, len: u32) -> Vec<u8> {
        let len = (len as usize).min(64 * 1024);
        let mut v = vec![0u8; len];
        // Same OS source the WASI random interface uses. (FIX-107) A source that cannot be read
        // fails the whole call: all-zero "random" bytes would make OAuth state/nonce values
        // predictable.
        let ok = std::fs::File::open("/dev/urandom").is_ok_and(|mut f| {
            use std::io::Read;
            f.read_exact(&mut v).is_ok()
        });
        if !ok {
            self.rng_failed = true;
        }
        v
    }

    async fn now_unix_seconds(&mut self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }
}

fn level_str(l: types::LogLevel) -> &'static str {
    match l {
        types::LogLevel::Debug => "debug",
        types::LogLevel::Info => "info",
        types::LogLevel::Warn => "warn",
        types::LogLevel::Error => "error",
    }
}

impl Ctx {
    fn emit(&self, level: &str, line: &str) {
        tracing::info!(
            target: "hook_log",
            package_id = %self.package_id,
            app_id = %self.app_id,
            hook = self.hook,
            level,
            "{line}"
        );
    }
}

/// A compiled, pre-linked component.
pub type Module = PluginPre<Ctx>;

pub struct Runtime {
    engine: Engine,
    linker: Linker<Ctx>,
    policy: FetchPolicy,
    ticker: tokio::task::JoinHandle<()>,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.ticker.abort();
    }
}

impl Runtime {
    /// Must be called inside a tokio runtime (starts the epoch ticker).
    pub fn new(policy: FetchPolicy) -> wasmtime::Result<Self> {
        let mut cfg = Config::new();
        cfg.wasm_component_model(true);
        cfg.epoch_interruption(true);
        cfg.max_wasm_stack(512 * 1024);
        let engine = Engine::new(&cfg)?;
        let mut linker = Linker::<Ctx>::new(&engine);
        link_wasi(&mut linker)?;
        Plugin::add_to_linker::<Ctx, wasmtime::component::HasSelf<Ctx>>(&mut linker, |c| c)?;
        let e = engine.clone();
        let ticker = tokio::spawn(async move {
            let mut iv = tokio::time::interval(TICK);
            iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                iv.tick().await;
                e.increment_epoch();
            }
        });
        Ok(Self {
            engine,
            linker,
            policy,
            ticker,
        })
    }

    /// CPU-heavy; call from `spawn_blocking`.
    pub fn compile(&self, bytes: &[u8]) -> wasmtime::Result<Module> {
        let component = Component::new(&self.engine, bytes)?;
        PluginPre::new(self.linker.instantiate_pre(&component)?)
    }

    /// Run one hook in a fresh store. Never panics on guest behaviour.
    pub async fn call(
        &self,
        module: &Module,
        spec: CallSpec,
        inv: Invocation,
    ) -> (Result<Outcome, CallError>, CallStats) {
        let started = Instant::now();
        let deadline = started + spec.wall;
        let stdout = MemoryOutputPipe::new(STDIO_CAP);
        let stderr = MemoryOutputPipe::new(STDIO_CAP);
        let wasi = WasiCtx::builder()
            .stdout(stdout.clone())
            .stderr(stderr.clone())
            .build();
        let mut store = Store::new(
            &self.engine,
            Ctx {
                wasi,
                table: ResourceTable::new(),
                limiter: Limiter { exceeded: false },
                egress: Egress::new(&spec.egress, self.policy.clone()),
                deadline,
                fetches: 0,
                denied: 0,
                logs: Vec::new(),
                log_overflow: false,
                rng_failed: false,
                secrets: spec.secrets,
                package_id: spec.package_id,
                app_id: spec.app_id,
                hook: spec.hook,
            },
        );
        store.limiter(|c| &mut c.limiter);
        store.set_epoch_deadline(1);
        store.epoch_deadline_callback(move |_| {
            if Instant::now() >= deadline {
                Err(Trap::Interrupt.into())
            } else {
                Ok(UpdateDeadline::Yield(1))
            }
        });

        // The outer timeout covers time spent blocked in host calls, where the
        // epoch cannot fire; a small margin lets the epoch win for busy loops.
        let run = run_hook(module, &mut store, inv);
        let res = tokio::time::timeout(spec.wall + 2 * TICK, run).await;

        let mut stats = CallStats {
            epoch_kill: false,
            denied: store.data().denied,
            fetches: store.data().fetches,
            elapsed: started.elapsed(),
        };
        // Captured stdio goes to the capped, redacted log.
        for (name, pipe) in [("stdout", &stdout), ("stderr", &stderr)] {
            let c = pipe.contents();
            if !c.is_empty() {
                let text = String::from_utf8_lossy(&c);
                let line = redact(&text, &store.data().secrets, LOG_LINE_MAX);
                store.data().emit(name, &line);
            }
        }
        let oom = store.data().limiter.exceeded;
        let out = match res {
            Err(_) => {
                stats.epoch_kill = true;
                Err(CallError::new("timeout", "hook exceeded its time limit"))
            }
            Ok(Err(e)) => {
                if oom {
                    Err(CallError::new("memory", "hook exceeded its memory limit"))
                } else if e
                    .downcast_ref::<Trap>()
                    .is_some_and(|t| *t == Trap::Interrupt)
                {
                    stats.epoch_kill = true;
                    Err(CallError::new("timeout", "hook exceeded its time limit"))
                } else {
                    tracing::warn!(error = %e, "hook trapped");
                    Err(CallError::new("internal", "hook failed"))
                }
            }
            Ok(Ok(Ok(o))) => Ok(o),
            Ok(Ok(Err(he))) => Err(map_hook_error(he, store.data(), &stats)),
        };
        let out = if out.is_ok() && store.data().rng_failed {
            tracing::error!("hook call used an unreadable random source");
            Err(CallError::new("internal", "random source unavailable"))
        } else {
            out
        };
        (out, stats)
    }
}

#[derive(Debug, Clone)]
pub struct CallStats {
    pub epoch_kill: bool,
    pub denied: u32,
    pub fetches: u32,
    pub elapsed: Duration,
}

fn map_hook_error(e: types::HookError, ctx: &Ctx, stats: &CallStats) -> CallError {
    let (code, msg) = match e {
        types::HookError::Unsupported => ("unsupported", String::new()),
        types::HookError::InvalidInput(m) => ("invalid_input", m),
        types::HookError::Upstream(m) => ("upstream", m),
        types::HookError::ReauthRequired(m) => ("reauth_required", m),
        types::HookError::Internal(m) => ("internal", m),
    };
    let code = if stats.denied > 0 && matches!(code, "upstream" | "internal") {
        "egress_denied"
    } else {
        code
    };
    CallError::new(code, redact(&msg, &ctx.secrets, 500))
}

async fn run_hook(
    module: &Module,
    store: &mut Store<Ctx>,
    inv: Invocation,
) -> wasmtime::Result<Result<Outcome, types::HookError>> {
    let instance = module.instantiate_async(&mut *store).await?;
    let h = instance.aurora_plugin_hooks();
    Ok(match inv {
        Invocation::Authorize(i) => h
            .call_connect_authorize(&mut *store, &i)
            .await?
            .map(Outcome::Authorize),
        Invocation::Exchange(i) => h
            .call_connect_exchange(&mut *store, &i)
            .await?
            .map(Outcome::Exchange),
        Invocation::Mint(i) => h.call_token_mint(&mut *store, &i).await?.map(Outcome::Mint),
        Invocation::Summarize(i) => h
            .call_approval_summarize(&mut *store, &i)
            .await?
            .map(Outcome::Summarize),
    })
}

/// Link only the WASI 0.2 interfaces permitted by I-22 (`cli/*` stdio and env,
/// `io/*`, `clocks/*`, `random/random`). No filesystem, no sockets.
fn link_wasi(l: &mut Linker<Ctx>) -> wasmtime::Result<()> {
    use wasmtime_wasi::cli::{WasiCli, WasiCliView};
    use wasmtime_wasi::p2::bindings::cli;
    // io + clocks + random + stdin/stdout/stderr
    wasmtime_wasi::p2::add_to_linker_proxy_interfaces_async(l)?;
    cli::exit::add_to_linker::<Ctx, WasiCli>(l, Ctx::cli)?;
    cli::environment::add_to_linker::<Ctx, WasiCli>(l, Ctx::cli)?;
    cli::terminal_input::add_to_linker::<Ctx, WasiCli>(l, Ctx::cli)?;
    cli::terminal_output::add_to_linker::<Ctx, WasiCli>(l, Ctx::cli)?;
    cli::terminal_stdin::add_to_linker::<Ctx, WasiCli>(l, Ctx::cli)?;
    cli::terminal_stdout::add_to_linker::<Ctx, WasiCli>(l, Ctx::cli)?;
    cli::terminal_stderr::add_to_linker::<Ctx, WasiCli>(l, Ctx::cli)?;
    Ok(())
}

pub type SharedRuntime = Arc<Runtime>;
