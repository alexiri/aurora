//! Process manager: background processes, watches and schedule gates
//! (`docs/12-supervisor-harness.md` §Process manager).
//!
//! Every process runs as `sh -c <command>` in its own process group (uid/gid `cfg.uid` when the
//! supervisor runs as root) with the container env plus the machine-proxy variables. stdout and
//! stderr are interleaved into a ring buffer of `PROCESS_TAIL` bytes. `process.state` frames are
//! debounced (2 s). Watches (`pattern` substring, `silence`, `exit`) are evaluated in memory and
//! fire at most once, sending `watch.fired`.
//!
//! Known gap: "busy" is output within the busy window (60 s); the CPU > 5 % clause is not measured.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aurora_core::limits;
use aurora_protocol::runner::{
    Code, GateOutcome, GateResult, MAX_GATE_OUTPUT, MAX_WATCH_DETAIL, ProcessState, ProcessStates,
    ProcessStatus, WatchFired,
};
use aurora_protocol::supervisor::SupervisorToRunner;
use rustix::process::{Pid, Signal, kill_process_group};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::Notify;
use tokio::time::{Instant, sleep, timeout};
use uuid::Uuid;

use crate::channel::ChannelSender;

/// Captured gate stdout bound (`12 §Process manager`).
pub const GATE_CAPTURE: usize = 16 * 1024;
const MAX_NAME: usize = 64;
/// (FIX-114) Exited process records kept (beyond those still waiting to be reported).
pub const MAX_EXITED_RECORDS: usize = 64;
const MAX_COMMAND: usize = 4096;
const MAX_PATTERN: usize = aurora_core::limits::WATCH_PATTERN_MAX;
const MAX_WATCHES: usize = 8;

#[derive(Debug, Clone)]
pub struct ProcessConfig {
    pub working_dir: PathBuf,
    /// Run children as this uid/gid, only effective when the supervisor itself is root.
    pub uid: Option<u32>,
    /// Extra env (machine proxy, app env) added to the allow-listed container env.
    pub env: Vec<(String, String)>,
    pub tail: usize,
    pub debounce: Duration,
    pub tick: Duration,
    pub busy_window: Duration,
    pub max_running: usize,
    pub stop_grace: Duration,
}

impl ProcessConfig {
    pub fn new(working_dir: PathBuf) -> Self {
        let proxy = "http://127.0.0.1:3128";
        Self {
            working_dir,
            uid: Some(1000),
            env: ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"]
                .iter()
                .map(|k| ((*k).to_owned(), proxy.to_owned()))
                .collect(),
            tail: limits::PROCESS_TAIL,
            debounce: Duration::from_secs(2),
            tick: Duration::from_secs(1),
            busy_window: Duration::from_secs(60),
            max_running: 16,
            stop_grace: Duration::from_secs(3),
        }
    }
}

enum WatchKind {
    Pattern(String),
    Silence(Duration),
    Exit,
}

struct Watch {
    id: String,
    kind: WatchKind,
}

struct Proc {
    name: String,
    command: String,
    pid: Option<i32>,
    state: ProcessStatus,
    exit_code: Option<i32>,
    ring: VecDeque<u8>,
    last_output: Instant,
    busy: bool,
    watches: Vec<Watch>,
}

#[derive(Default)]
struct Inner {
    procs: BTreeMap<String, Proc>,
    dirty: BTreeSet<String>,
}

pub struct ProcessManager {
    cfg: ProcessConfig,
    out: ChannelSender,
    inner: Mutex<Inner>,
    dirty: Notify,
}

/// (FIX-114) Drop the oldest exited records beyond `MAX_EXITED_RECORDS`, keeping any the API has
/// not been told about yet (`dirty`). Ids are UUID v7: the map is ordered oldest first.
fn prune_exited(procs: &mut BTreeMap<String, Proc>, dirty: &BTreeSet<String>) {
    let exited: Vec<String> = procs
        .iter()
        .filter(|(id, p)| p.state != ProcessStatus::Running && !dirty.contains(*id))
        .map(|(id, _)| id.clone())
        .collect();
    let excess = (exited.len() + 1).saturating_sub(MAX_EXITED_RECORDS);
    for id in exited.into_iter().take(excess) {
        procs.remove(&id);
    }
}

fn tail_string(ring: &VecDeque<u8>, max: usize) -> String {
    let bytes: Vec<u8> = ring.iter().copied().collect();
    let mut s = String::from_utf8_lossy(&bytes).into_owned();
    if s.len() > max {
        let mut cut = s.len() - max;
        while !s.is_char_boundary(cut) {
            cut += 1;
        }
        s.drain(..cut);
    }
    s
}

fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

/// `(state, ppid)` from the text of `/proc/<pid>/stat` (the command name may contain spaces and
/// parentheses, so the fields start after the **last** `)`).
pub fn parse_proc_stat(stat: &str) -> Option<(char, i32)> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut f = rest.split_whitespace();
    let state = f.next()?.chars().next()?;
    let ppid = f.next()?.parse().ok()?;
    Some((state, ppid))
}

/// Zombie children of this process, from `/proc`.
fn zombie_children(me: i32) -> Vec<i32> {
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    rd.filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|s| parse_proc_stat(&s))
                .is_some_and(|(state, ppid)| state == 'Z' && ppid == me)
        })
        .collect()
}

/// (FIX-114, `12` §Process model) The supervisor runs as PID 1 of the sandbox: processes whose
/// parent died (tool subprocesses of an aborted turn, `sh -c 'server &'`) are reparented to it
/// and stay zombies until someone waits for them, eating the sandbox's pid budget.
///
/// Tokio reaps only the children it spawned, and does so itself at SIGCHLD; a zombie child of
/// ours that is **still there on the next scan** (`interval` later, long after tokio would have
/// reaped its own) is an orphan nobody owns, so it is reaped here. Returns a task handle.
pub fn spawn_reaper(interval: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let me = i32::try_from(std::process::id()).unwrap_or(1);
        let mut seen: std::collections::BTreeSet<i32> = std::collections::BTreeSet::new();
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut chld = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::child()).ok();
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                Some(()) = async { match chld.as_mut() { Some(s) => s.recv().await, None => None } } => {}
            }
            let now: std::collections::BTreeSet<i32> = zombie_children(me).into_iter().collect();
            for pid in now.intersection(&seen) {
                if let Some(p) = Pid::from_raw(*pid) {
                    let _ = rustix::process::waitpid(Some(p), rustix::process::WaitOptions::NOHANG);
                }
            }
            // Zombies seen for the first time get one more interval to be reaped by their owner.
            seen = now.difference(&seen).copied().collect();
        }
    })
}

fn signal_group(pid: i32, sig: Signal) {
    if let Some(p) = Pid::from_raw(pid) {
        let _ = kill_process_group(p, sig);
    }
}

/// Builds `sh -c <command>` with the shared posture (own group, uid, container env).
fn shell(cfg: &ProcessConfig, command: &str, cwd: &Path) -> Command {
    let mut c = crate::spawn::tokio_command("sh", cfg.env.iter().map(|(k, v)| (k, v)));
    c.arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .process_group(0);
    if let Some(uid) = cfg.uid
        && rustix::process::getuid().is_root()
    {
        c.uid(uid).gid(uid);
    }
    c
}

/// Resolves `cwd` against the working dir; it must stay inside it.
fn resolve_cwd(root: &Path, cwd: Option<&str>) -> Result<PathBuf, String> {
    let Some(cwd) = cwd.filter(|c| !c.is_empty()) else {
        return Ok(root.to_path_buf());
    };
    let p = Path::new(cwd);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    };
    if joined
        .components()
        .any(|c| matches!(c, Component::ParentDir))
        || !joined.starts_with(root)
    {
        return Err("cwd must be inside the workspace".into());
    }
    Ok(joined)
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

impl ProcessManager {
    pub fn new(cfg: ProcessConfig, out: ChannelSender) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            out,
            inner: Mutex::new(Inner::default()),
            dirty: Notify::new(),
        })
    }

    /// Starts the debounced state reporter and the silence/busy ticker. Call once, inside a runtime.
    pub fn spawn_background(self: &Arc<Self>) {
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                me.dirty.notified().await;
                sleep(me.cfg.debounce).await;
                me.flush_state().await;
            }
        });
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                sleep(me.cfg.tick).await;
                me.tick().await;
            }
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    async fn flush_state(&self) {
        let processes: Vec<ProcessState> = {
            let mut g = self.lock();
            let ids = std::mem::take(&mut g.dirty);
            ids.iter()
                .filter_map(|id| g.procs.get(id).map(|p| (id, p)))
                .map(|(id, p)| ProcessState {
                    process_id: id.clone(),
                    state: p.state.clone(),
                    command: Some(truncate_bytes(&p.command, 512)),
                    busy: Some(p.busy),
                    exit_code: p.exit_code,
                    tail: Some(tail_string(&p.ring, self.cfg.tail)),
                })
                .collect()
        };
        if !processes.is_empty() {
            let _ = self
                .out
                .send(SupervisorToRunner::ProcessState(ProcessStates {
                    processes,
                }))
                .await;
        }
    }

    async fn tick(&self) {
        let mut fires = Vec::new();
        {
            let mut g = self.lock();
            let Inner { procs, dirty, .. } = &mut *g;
            for (id, p) in procs.iter_mut() {
                if p.state != ProcessStatus::Running {
                    continue;
                }
                let quiet = p.last_output.elapsed();
                let busy = quiet < self.cfg.busy_window;
                if busy != p.busy {
                    p.busy = busy;
                    dirty.insert(id.clone());
                    self.dirty.notify_one();
                }
                let tail = tail_string(&p.ring, self.cfg.tail);
                let name = p.name.clone();
                p.watches.retain(|w| match w.kind {
                    WatchKind::Silence(d) if quiet >= d => {
                        fires.push(watch_fired(
                            &w.id,
                            &format!("process `{name}` produced no output for {}s", d.as_secs()),
                            &tail,
                        ));
                        false
                    }
                    _ => true,
                });
            }
        }
        for f in fires {
            let _ = self.out.send(SupervisorToRunner::WatchFired(f)).await;
        }
    }

    fn append(&self, id: &str, chunk: &[u8]) -> Vec<WatchFired> {
        let mut g = self.lock();
        let Inner { procs, dirty, .. } = &mut *g;
        let Some(p) = procs.get_mut(id) else {
            return Vec::new();
        };
        p.ring.extend(chunk);
        while p.ring.len() > self.cfg.tail {
            p.ring.pop_front();
        }
        p.last_output = Instant::now();
        if !p.busy {
            p.busy = true;
        }
        dirty.insert(id.to_owned());
        self.dirty.notify_one();
        let mut fires = Vec::new();
        let name = p.name.clone();
        let tail = tail_string(&p.ring, self.cfg.tail);
        let ring = &p.ring;
        p.watches.retain(|w| {
            let WatchKind::Pattern(pat) = &w.kind else {
                return true;
            };
            let take = (chunk.len() + pat.len()).min(ring.len());
            let window: Vec<u8> = ring.iter().skip(ring.len() - take).copied().collect();
            if String::from_utf8_lossy(&window).contains(pat.as_str()) {
                fires.push(watch_fired(
                    &w.id,
                    &format!("process `{name}` output matched `{pat}`"),
                    &tail,
                ));
                false
            } else {
                true
            }
        });
        fires
    }

    async fn pump<R: AsyncRead + Unpin>(self: Arc<Self>, id: String, mut r: R) {
        let mut buf = [0u8; 4096];
        loop {
            let n = match r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            for f in self.append(&id, &buf[..n]) {
                let _ = self.out.send(SupervisorToRunner::WatchFired(f)).await;
            }
        }
    }

    /// Ids of the processes that are running now.
    pub fn running_ids(&self) -> Vec<String> {
        self.lock()
            .procs
            .iter()
            .filter(|(_, p)| p.state == ProcessStatus::Running)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Number of process records held (running plus remembered exits).
    pub fn record_count(&self) -> usize {
        self.lock().procs.len()
    }

    /// Starts a process; returns its id.
    pub fn start(
        self: &Arc<Self>,
        name: &str,
        command: &str,
        cwd: Option<&str>,
    ) -> Result<String, String> {
        if name.is_empty() || name.chars().count() > MAX_NAME {
            return Err(format!("name must be 1..={MAX_NAME} characters"));
        }
        if command.trim().is_empty() || command.len() > MAX_COMMAND || command.contains('\0') {
            return Err(format!("command must be 1..={MAX_COMMAND} bytes, no NUL"));
        }
        let cwd = resolve_cwd(&self.cfg.working_dir, cwd)?;
        // (FIX-114) The slot is reserved under the same lock as the `max_running` check, so N+1
        // concurrent starts yield at most N; exited records are capped so the table stays bounded.
        let id = {
            let mut g = self.lock();
            let running = g
                .procs
                .values()
                .filter(|p| p.state == ProcessStatus::Running)
                .count();
            if running >= self.cfg.max_running {
                return Err(format!(
                    "at most {} processes may run at once",
                    self.cfg.max_running
                ));
            }
            // (P4-PRV-2) Supervisor-assigned UUID (`03 §processes`): the API's `process.state`
            // handler parses the id as a UUID — a `p{n}` string would invalidate the report.
            let id = Uuid::now_v7().to_string();
            let Inner { procs, dirty, .. } = &mut *g;
            prune_exited(procs, dirty);
            procs.insert(
                id.clone(),
                Proc {
                    name: name.to_owned(),
                    command: command.to_owned(),
                    pid: None,
                    state: ProcessStatus::Running,
                    exit_code: None,
                    ring: VecDeque::new(),
                    last_output: Instant::now(),
                    busy: false,
                    watches: Vec::new(),
                },
            );
            id
        };
        let mut child = match shell(&self.cfg, command, &cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                self.lock().procs.remove(&id);
                return Err(format!("could not start process: {e}"));
            }
        };
        {
            let mut g = self.lock();
            if let Some(p) = g.procs.get_mut(&id) {
                p.pid = child.id().and_then(|p| i32::try_from(p).ok());
            }
            g.dirty.insert(id.clone());
        }
        self.dirty.notify_one();
        let readers: Vec<_> = [
            child
                .stdout
                .take()
                .map(|s| tokio::spawn(self.clone().pump(id.clone(), s))),
            child
                .stderr
                .take()
                .map(|s| tokio::spawn(self.clone().pump(id.clone(), s))),
        ]
        .into_iter()
        .flatten()
        .collect();
        let me = self.clone();
        let pid = id.clone();
        tokio::spawn(async move {
            let status = child.wait().await.ok();
            for r in readers {
                let _ = timeout(Duration::from_millis(500), r).await;
            }
            me.on_exit(&pid, status.and_then(|s| s.code())).await;
        });
        Ok(id)
    }

    async fn on_exit(&self, id: &str, code: Option<i32>) {
        let mut fires = Vec::new();
        {
            let mut g = self.lock();
            let Inner { procs, dirty, .. } = &mut *g;
            if let Some(p) = procs.get_mut(id) {
                p.state = ProcessStatus::Exited;
                p.exit_code = code;
                p.pid = None;
                p.busy = false;
                let tail = tail_string(&p.ring, self.cfg.tail);
                let name = p.name.clone();
                p.watches.retain(|w| {
                    if matches!(w.kind, WatchKind::Exit) {
                        fires.push(watch_fired(
                            &w.id,
                            &format!("process `{name}` exited with code {code:?}"),
                            &tail,
                        ));
                        false
                    } else {
                        true
                    }
                });
                p.watches.clear();
                dirty.insert(id.to_owned());
            }
        }
        self.dirty.notify_one();
        for f in fires {
            let _ = self.out.send(SupervisorToRunner::WatchFired(f)).await;
        }
    }

    /// (FIX-114) SIGTERM to the process group of every running process (shutdown, `docs/12`
    /// §Process model); returns how many groups were signalled.
    pub fn terminate_all(&self) -> usize {
        let pids: Vec<i32> = self
            .lock()
            .procs
            .values()
            .filter(|p| p.state == ProcessStatus::Running)
            .filter_map(|p| p.pid)
            .collect();
        for pid in &pids {
            signal_group(*pid, Signal::TERM);
        }
        pids.len()
    }

    /// SIGTERM to the group, SIGKILL after the grace period.
    pub fn stop(self: &Arc<Self>, id: &str) -> Result<(), String> {
        let pid = {
            let g = self.lock();
            let p = g.procs.get(id).ok_or("unknown process id")?;
            if p.state != ProcessStatus::Running {
                return Err("process is not running".into());
            }
            p.pid.ok_or("process is not running")?
        };
        signal_group(pid, Signal::TERM);
        let me = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            sleep(me.cfg.stop_grace).await;
            let alive = me
                .lock()
                .procs
                .get(&id)
                .is_some_and(|p| p.state == ProcessStatus::Running);
            if alive {
                signal_group(pid, Signal::KILL);
            }
        });
        Ok(())
    }

    pub fn watch(
        &self,
        id: &str,
        kind: &str,
        pattern: Option<&str>,
        silence_secs: Option<u64>,
    ) -> Result<String, String> {
        let kind = match kind {
            "pattern" => {
                let pat = pattern
                    .filter(|p| !p.is_empty())
                    .ok_or("pattern required")?;
                if pat.len() > MAX_PATTERN {
                    return Err(format!("pattern must be at most {MAX_PATTERN} bytes"));
                }
                WatchKind::Pattern(pat.to_owned())
            }
            "silence" => match silence_secs {
                Some(s @ 1..=86_400) => WatchKind::Silence(Duration::from_secs(s)),
                _ => return Err("silence_secs must be 1..=86400".into()),
            },
            "exit" => WatchKind::Exit,
            _ => return Err("kind must be pattern, silence or exit".into()),
        };
        let mut g = self.lock();
        // (P4-PRV-2) Supervisor-assigned UUID (`03 §watches`): the API parses `watch_id` as a
        // UUID when a `watch.fired` report lands — a `w{n}` string would invalidate the report.
        let wid = Uuid::now_v7().to_string();
        let p = g.procs.get_mut(id).ok_or("unknown process id")?;
        if p.state != ProcessStatus::Running {
            return Err("process is not running".into());
        }
        if p.watches.len() >= MAX_WATCHES {
            return Err(format!("at most {MAX_WATCHES} watches per process"));
        }
        if matches!(kind, WatchKind::Silence(_)) {
            p.last_output = Instant::now();
        }
        p.watches.push(Watch {
            id: wid.clone(),
            kind,
        });
        Ok(wid)
    }

    pub fn status(&self, id: Option<&str>) -> Result<Value, String> {
        let g = self.lock();
        let one = |id: &String, p: &Proc| {
            json!({
                "id": id,
                "name": p.name,
                "command": truncate_bytes(&p.command, 512),
                "state": p.state.as_str(),
                "busy": p.busy,
                "exit_code": p.exit_code,
                "tail": tail_string(&p.ring, self.cfg.tail),
            })
        };
        match id {
            Some(id) => {
                let (k, p) = g.procs.get_key_value(id).ok_or("unknown process id")?;
                Ok(one(k, p))
            }
            None => Ok(
                json!({ "processes": g.procs.iter().map(|(k, p)| one(k, p)).collect::<Vec<_>>() }),
            ),
        }
    }

    /// Executes a local `process_*` tool; `Err` is shown to the model.
    pub fn call(self: &Arc<Self>, tool: &str, args: &Value) -> Result<Value, String> {
        let id = || str_arg(args, "id").ok_or_else(|| "id required".to_owned());
        match tool {
            "process_start" => {
                let name = str_arg(args, "name").ok_or("name required")?;
                let command = str_arg(args, "command").ok_or("command required")?;
                let id = self.start(name, command, str_arg(args, "cwd"))?;
                Ok(json!({ "id": id, "state": "running" }))
            }
            "process_status" => self.status(str_arg(args, "id")),
            "process_stop" => {
                self.stop(id()?)?;
                Ok(json!({ "stopping": true }))
            }
            "process_watch" => {
                let w = self.watch(
                    id()?,
                    str_arg(args, "kind").ok_or("kind required")?,
                    str_arg(args, "pattern"),
                    args.get("silence_secs").and_then(Value::as_u64),
                )?;
                Ok(json!({ "watch_id": w }))
            }
            _ => Err("unknown process tool".into()),
        }
    }

    /// Descriptors of the local `process_*` tools (merged into `tools/list`; the API catalogue
    /// holds remote tools only).
    pub fn local_descriptors() -> Vec<aurora_protocol::tools::ToolDescriptor> {
        let d = |name: &str, description: &str, input_schema: Value| {
            aurora_protocol::tools::ToolDescriptor {
                name: name.to_owned(),
                description: Some(description.to_owned()),
                input_schema,
            }
        };
        vec![
            d(
                "process_start",
                "Start a background process (`sh -c command`) in the workspace; returns its id.",
                json!({"type":"object","additionalProperties":false,"required":["name","command"],"properties":{
                    "name":{"type":"string","maxLength":MAX_NAME},
                    "command":{"type":"string","maxLength":MAX_COMMAND},
                    "cwd":{"type":"string","description":"Directory inside the workspace"}}}),
            ),
            d(
                "process_status",
                "Status and output tail of one background process, or of all when `id` is omitted.",
                json!({"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"}}}),
            ),
            d(
                "process_stop",
                "Stop a background process (SIGTERM, then SIGKILL after a grace period).",
                json!({"type":"object","additionalProperties":false,"required":["id"],"properties":{"id":{"type":"string"}}}),
            ),
            d(
                "process_watch",
                "Wake the agent when a process prints a pattern, is silent for a while, or exits. Fires once.",
                json!({"type":"object","additionalProperties":false,"required":["id","kind"],"properties":{
                    "id":{"type":"string"},
                    "kind":{"type":"string","enum":["pattern","silence","exit"]},
                    "pattern":{"type":"string","maxLength":MAX_PATTERN},
                    "silence_secs":{"type":"integer","minimum":1}}}),
            ),
        ]
    }

    pub fn is_local_tool(name: &str) -> bool {
        matches!(
            name,
            "process_start" | "process_status" | "process_stop" | "process_watch"
        )
    }

    /// Runs a schedule gate and returns the `gate.result` body. Empty stdout or
    /// `{"wake_agent": false}` is quiet, a non-zero exit or timeout is failed, other stdout wakes.
    pub async fn run_gate(&self, turn_id: Uuid, command: &str, timeout_secs: u32) -> GateResult {
        let limit = Duration::from_secs(u64::from(timeout_secs)).min(limits::GATE_TIMEOUT);
        let done = |outcome, output: Option<String>| GateResult {
            turn_id,
            outcome,
            output: output.map(|o| truncate_bytes(&o, MAX_GATE_OUTPUT)),
        };
        let mut child = match shell(&self.cfg, command, &self.cfg.working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                return done(
                    GateOutcome::Failed,
                    Some(format!("gate did not start: {e}")),
                );
            }
        };
        let pid = child.id().and_then(|p| i32::try_from(p).ok());
        let Some(mut stdout) = child.stdout.take() else {
            return done(GateOutcome::Failed, None);
        };
        let run = async {
            let mut buf = Vec::new();
            let _ = (&mut stdout)
                .take(GATE_CAPTURE as u64)
                .read_to_end(&mut buf)
                .await;
            // Past the capture bound the rest is discarded; the gate still has to finish.
            let _ = tokio::io::copy(&mut stdout, &mut tokio::io::sink()).await;
            let status = child.wait().await;
            (buf, status)
        };
        match timeout(limit, run).await {
            Err(_) => {
                if let Some(p) = pid {
                    signal_group(p, Signal::KILL);
                }
                done(GateOutcome::Failed, Some("gate timed out".into()))
            }
            Ok((buf, Ok(st))) if st.success() => {
                let text = String::from_utf8_lossy(&buf).trim().to_owned();
                let quiet = text.is_empty()
                    || serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|v| v.get("wake_agent").and_then(Value::as_bool))
                        == Some(false);
                if quiet {
                    done(GateOutcome::Quiet, None)
                } else {
                    done(GateOutcome::Wake, Some(text))
                }
            }
            Ok((buf, _)) => done(
                GateOutcome::Failed,
                Some(String::from_utf8_lossy(&buf).trim().to_owned()).filter(|s| !s.is_empty()),
            ),
        }
    }
}

fn watch_fired(watch_id: &str, what: &str, tail: &str) -> WatchFired {
    WatchFired {
        watch_id: watch_id.to_owned(),
        detail: truncate_bytes(&format!("{what}\n{tail}"), MAX_WATCH_DETAIL),
    }
}

/// Short code helper for tool errors.
pub fn code(s: &str) -> Code {
    Code::new(s).unwrap_or_else(|_| Code::new("error").unwrap_or_else(|_| unreachable_code()))
}

#[allow(clippy::panic)]
fn unreachable_code() -> Code {
    panic!("fixed code vocabulary exceeds bound")
}
