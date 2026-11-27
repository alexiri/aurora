#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! (FIX-265) The claude-code child's background work, driven end to end: a scripted child (a
//! shell script speaking stream-json, started by the real process launcher) under a real
//! supervisor. The list shrinks, Stop reaches orphans, and "released" is said only once the
//! processes are gone.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use aurora_protocol::runner::SendNowOutcome;
use aurora_protocol::supervisor::{RunnerToSupervisor, SupervisorToRunner};
use aurora_supervisor::adapters::claude_code::ClaudeCodeAdapter;
use aurora_supervisor::adapters::claude_code::launcher::ProcessLauncher;
use aurora_supervisor::channel::ChannelSender;
use aurora_supervisor::fake::FakeHarness;
use aurora_supervisor::harness::{
    BackgroundWork, HarnessAdapter, HarnessCapabilities, HarnessError, HarnessEventStream,
    HarnessSession, SessionOptions, SteerAck, SteerInput, TurnInput,
};
use aurora_supervisor::process::ProcessConfig;
use aurora_supervisor::supervisor::{Supervisor, SupervisorConfig};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::time::timeout;
use uuid::Uuid;

use crate::common::TempDir;

const T: Duration = Duration::from_secs(30);
const CONV: &str = "00000000-0000-0000-0000-0000000000c9";

const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"sess-bg","tools":["Bash","mcp__aurora__memory_save"],"mcp_servers":[{"name":"aurora","status":"connected"}]}"#;
const RESULT: &str =
    r#"{"type":"result","subtype":"success","is_error":false,"session_id":"sess-bg"}"#;

/// The lines of a `Bash` call with `run_in_background`, as the pinned CLI writes them: the call,
/// the launch's own tool result (not the end of the command) and the task's start.
fn bg_started(tool_use: &str, task: &str, command: &str) -> Vec<String> {
    vec![
        json!({"type": "assistant", "parent_tool_use_id": null, "message": {"id": format!("m-{tool_use}"),
            "role": "assistant", "content": [{"type": "tool_use", "id": tool_use, "name": "Bash",
            "input": {"command": command, "run_in_background": true}}]}})
        .to_string(),
        json!({"type": "system", "subtype": "task_started", "task_id": task,
            "tool_use_id": tool_use, "task_type": "local_bash", "is_backgrounded": true})
        .to_string(),
        json!({"type": "user", "parent_tool_use_id": null, "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": tool_use,
             "content": format!("Command running in background with ID: {task}")}]}})
        .to_string(),
    ]
}

/// The CLI's event for a background command that ended.
fn bg_finished(tool_use: &str, task: &str) -> String {
    json!({"type": "system", "subtype": "task_notification", "task_id": task,
        "tool_use_id": tool_use, "status": "completed", "output_file": "", "summary": "done"})
    .to_string()
}

/// Shell commands that print `lines` (none of which holds a single quote).
fn say(lines: &[String]) -> String {
    lines.iter().map(|l| format!("echo '{l}'\n")).collect()
}

struct Rig {
    to_sup: mpsc::Sender<RunnerToSupervisor>,
    from_sup: mpsc::Receiver<SupervisorToRunner>,
    ws: PathBuf,
}

fn frame(kind: &str, body: Value) -> RunnerToSupervisor {
    serde_json::from_value(json!({"kind": kind, "body": body})).unwrap()
}

/// The scripted child: the init line, then for the n-th user line the script `turn<n>.sh`.
fn write_child(ws: &Path, turns: &[String]) -> PathBuf {
    for (i, body) in turns.iter().enumerate() {
        fs::write(ws.join(format!("turn{}.sh", i + 1)), body).unwrap();
    }
    let child = ws.join("claude-stub");
    fs::write(
        &child,
        format!(
            "#!/bin/sh\ncd '{}'\necho '{INIT}'\nn=0\nwhile IFS= read -r line; do\n  n=$((n+1))\n  . ./turn$n.sh\ndone\n",
            ws.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&child, fs::Permissions::from_mode(0o755)).unwrap();
    child
}

async fn rig(turns: &[String]) -> Rig {
    let ws = TempDir::new().keep();
    let program = write_child(&ws, turns);
    let adapter = ClaudeCodeAdapter::with_launcher(Arc::new(ProcessLauncher { program }));
    rig_with(ws, Arc::new(adapter)).await
}

async fn rig_with(ws: PathBuf, adapter: Arc<dyn HarnessAdapter>) -> Rig {
    rig_cfg(ws, adapter, |_| {}).await
}

async fn rig_cfg(
    ws: PathBuf,
    adapter: Arc<dyn HarnessAdapter>,
    tune: impl FnOnce(&mut SupervisorConfig),
) -> Rig {
    fs::create_dir_all(ws.join(".aurora")).unwrap();
    let (out_tx, out_rx) = mpsc::channel(256);
    let (in_tx, in_rx) = mpsc::channel(256);
    let mut cfg = SupervisorConfig::new(crate::common::dead_upstream(), "aub_x");
    cfg.working_dir = ws.clone();
    cfg.process = ProcessConfig::new(ws.clone());
    cfg.harvest_interval = Duration::from_millis(100);
    cfg.mcp_bridge_command = vec!["/bin/true".into(), "mcp-bridge".into()];
    tune(&mut cfg);
    let sup = Supervisor::new(
        cfg,
        ChannelSender::from_mpsc(out_tx),
        adapter,
        aurora_supervisor::streams::StreamsHandle::noop(),
        aurora_supervisor::pty::PtyHandle::noop(),
    );
    tokio::spawn(sup.run(in_rx));
    let mut rig = Rig {
        to_sup: in_tx,
        from_sup: out_rx,
        ws,
    };
    rig.next("ready").await;
    for (i, kind) in ["manifest", "final"].into_iter().enumerate() {
        rig.send(
            "home.sync",
            json!({"generation": 1, "part_index": i, "part_count": 2,
                   "part": {"kind": kind, "entries": []}}),
        )
        .await;
    }
    rig.next("home.synced").await;
    rig
}

impl Rig {
    async fn send(&self, kind: &str, body: Value) {
        self.to_sup.send(frame(kind, body)).await.unwrap();
    }

    /// The body of the next frame of `kind`.
    async fn next(&mut self, kind: &str) -> Value {
        loop {
            let m = timeout(T, self.from_sup.recv())
                .await
                .unwrap_or_else(|_| panic!("no {kind} frame"))
                .unwrap();
            if m.kind() == kind {
                return serde_json::to_value(&m).unwrap()["body"].clone();
            }
        }
    }

    /// Runs one turn and returns the commands of its `background` event, if it sent one.
    async fn turn(&mut self) -> Option<Vec<String>> {
        self.send(
            "turn.deliver",
            json!({
                "conversation_id": CONV, "turn_id": Uuid::now_v7(), "session": {"epoch": 1},
                "model": {"provider": "p", "endpoint_id": "e", "base_url": "http://x",
                          "dialect": "anthropic_messages", "model": "m", "effort": "low", "client_env": []},
                "context_credential": "auc_turn",
                "input": {"text": "go", "speaker_label": "Ann", "door": "web",
                          "recall": {"index": "", "snippets": []}, "notices": []},
                "attachments": [],
            }),
        )
        .await;
        let mut background = None;
        loop {
            let m = timeout(T, self.from_sup.recv())
                .await
                .expect("the turn did not end")
                .unwrap();
            let v = serde_json::to_value(&m).unwrap();
            match m.kind() {
                "turn.event" if v["body"]["event"]["kind"] == "background" => {
                    background = Some(commands(&v["body"]["event"]));
                }
                "turn.result" => {
                    assert_eq!(v["body"]["status"], "done", "{v}");
                    return background;
                }
                _ => {}
            }
        }
    }

    /// Stop: `session.release`, re-sent like the API's lease does while the turn is still being
    /// wound up, until the supervisor says it released the session.
    async fn stop(&mut self) {
        let deadline = std::time::Instant::now() + T;
        loop {
            self.send("session.release", json!({"conversation_id": CONV}))
                .await;
            if self.next("session.released").await["ok"] == true {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "never released");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn pid_in(&self, file: &str) -> u32 {
        timeout(T, async {
            loop {
                if let Ok(s) = fs::read_to_string(self.ws.join(file))
                    && let Ok(pid) = s.trim().parse()
                {
                    return pid;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("no pid file")
    }
}

/// The commands of a `background` event (`{"kind": "background", "items": [...]}`).
fn commands(event: &Value) -> Vec<String> {
    event["items"]
        .as_array()
        .unwrap_or_else(|| panic!("no items: {event}"))
        .iter()
        .map(|i| i["command"].as_str().unwrap().to_owned())
        .collect()
}

/// `true` while the process runs (a zombie has ended; only its parent's wait is missing).
fn running(pid: u32) -> bool {
    std::fs::read(format!("/proc/{pid}/stat")).is_ok_and(|s| {
        String::from_utf8_lossy(&s)
            .rsplit_once(')')
            .is_some_and(|(_, f)| !f.trim_start().starts_with('Z'))
    })
}

/// Kills what a failing test would leave behind.
struct Reap(u32);

impl Drop for Reap {
    fn drop(&mut self) {
        if running(self.0)
            && let Some(p) = i32::try_from(self.0)
                .ok()
                .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process(p, rustix::process::Signal::KILL);
        }
    }
}

async fn gone(pid: u32) {
    timeout(T, async {
        while Path::new(&format!("/proc/{pid}")).exists() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("pid {pid} is still there"));
}

/// A background command that finishes leaves the list, and the one started later is the one
/// reported. The last one's end is reported too, as an empty list.
#[tokio::test]
async fn a_finished_background_command_leaves_the_list() {
    let mut one = bg_started("toolu_a", "ta", "cmd-a");
    one.push(RESULT.into());
    let mut two = vec![bg_finished("toolu_a", "ta")];
    two.extend(bg_started("toolu_b", "tb", "cmd-b"));
    two.push(RESULT.into());
    // The second way the CLI says that a task ended: by its task id only.
    let three = vec![
        json!({"type": "system", "subtype": "task_updated", "task_id": "tb",
            "patch": {"status": "completed"}})
        .to_string(),
        RESULT.into(),
    ];
    let mut r = rig(&[say(&one), say(&two), say(&three)]).await;
    assert_eq!(r.turn().await, Some(vec!["cmd-a".to_owned()]));
    assert_eq!(r.turn().await, Some(vec!["cmd-b".to_owned()]));
    assert_eq!(r.turn().await, Some(vec![]));
    r.stop().await;
}

/// A command that ends while the child is idle shrinks the list at once, as a `background` entry
/// of the idle events, and what the child then does by itself is still recorded.
#[tokio::test]
async fn a_background_command_that_ends_while_idle_is_reported() {
    let mut one = bg_started("toolu_a", "ta", "cmd-a");
    one.push(RESULT.into());
    let after = [
        bg_finished("toolu_a", "ta"),
        // The CLI's own run about it starts a moment later.
    ];
    let reaction = [
        json!({"type": "assistant", "parent_tool_use_id": null, "message": {"id": "m-late",
            "role": "assistant", "content": [{"type": "text", "text": "it finished"}]}})
        .to_string(),
        RESULT.into(),
    ];
    let script = format!(
        "{}sleep 1\n{}sleep 1\n{}",
        say(&one),
        say(&after),
        say(&reaction)
    );
    let mut r = rig(&[script]).await;
    assert_eq!(r.turn().await, Some(vec!["cmd-a".to_owned()]));
    let (mut emptied, mut told) = (false, false);
    while !(emptied && told) {
        let body = r.next("turn.idle.events").await;
        for e in body["events"].as_array().unwrap() {
            let e = &e["event"];
            if e["kind"] == "background" {
                assert_eq!(commands(e), Vec::<String>::new(), "{e}");
                emptied = true;
            }
            if e["kind"] == "text.delta" && e["delta"] == "it finished" {
                told = true;
            }
        }
    }
    r.stop().await;
}

/// `setsid -f` from a shell that then exits: the sleeper leads its own session and has lost its
/// parent, so neither the group nor the parent chain finds it. Stop ends it all the same.
#[tokio::test]
async fn stop_reaches_a_detached_process_whose_parent_exited() {
    let mut lines = bg_started("toolu_a", "ta", "cmd-a");
    lines.push(RESULT.into());
    let script = format!(
        "sh -c 'setsid -f sh -c \"echo \\$\\$ > orphan.pid; exec sleep 300\"'\n{}",
        say(&lines)
    );
    let mut r = rig(&[script]).await;
    r.turn().await;
    let orphan = r.pid_in("orphan.pid").await;
    let _reap = Reap(orphan);
    assert!(running(orphan), "the orphan is not running");
    r.stop().await;
    assert!(
        !running(orphan),
        "released was reported while the orphan {orphan} still runs"
    );
    gone(orphan).await;
}

/// (SEC-165) The kernel cuts a process's name to 15 bytes, also inside a character. Its
/// `stat` is then not UTF-8, and Stop finds it all the same.
#[tokio::test]
async fn stop_reaches_a_process_whose_name_is_cut_inside_a_character() {
    let mut lines = bg_started("toolu_a", "ta", "cmd-a");
    lines.push(RESULT.into());
    // 14 bytes, then a two-byte character: the cut falls between its bytes.
    let name = "aaaaaaaaaaaaaaé";
    let script = format!("setsid -f ./{name}\n{}", say(&lines));
    let mut r = rig(&[script]).await;
    let named = r.ws.join(name);
    fs::write(
        &named,
        // It waits on a pipe nobody writes to, with no child that Stop could see instead.
        "#!/bin/sh\nmkfifo never\necho $$ > named.pid\nread line < never\n",
    )
    .unwrap();
    fs::set_permissions(&named, fs::Permissions::from_mode(0o755)).unwrap();
    r.turn().await;
    let pid = r.pid_in("named.pid").await;
    let _reap = Reap(pid);
    assert!(
        fs::read_to_string(format!("/proc/{pid}/stat")).is_err(),
        "the name was not cut inside a character"
    );
    r.stop().await;
    assert!(
        !running(pid),
        "released was reported while {pid} still runs"
    );
    gone(pid).await;
}

/// The release is acknowledged only after the processes are gone: this background shell ignores
/// HUP, so it lives through the grace and dies only to the KILL that follows.
#[tokio::test]
async fn released_is_reported_only_after_the_processes_are_gone() {
    let mut lines = bg_started("toolu_a", "ta", "cmd-a");
    lines.push(RESULT.into());
    let script = format!(
        "sh -c 'trap \"\" HUP; echo $$ > immune.pid; exec sleep 300' &\nwhile [ ! -s immune.pid ]; do sleep 0.05; done\n{}",
        say(&lines)
    );
    let mut r = rig(&[script]).await;
    r.turn().await;
    let immune = r.pid_in("immune.pid").await;
    let _reap = Reap(immune);
    assert!(running(immune));
    r.stop().await;
    assert!(
        !running(immune),
        "released was reported while {immune} still runs"
    );
    gone(immune).await;
}

const STUCK_CONV: &str = "00000000-0000-0000-0000-0000000000ca";

/// The fake harness, but a session of [`STUCK_CONV`] never finishes its release, and every
/// other one takes this long.
struct Stuck(Duration);

struct StuckSession(Box<dyn HarnessSession>, bool, Duration);

#[async_trait]
impl HarnessAdapter for Stuck {
    fn id(&self) -> &'static str {
        "fake"
    }
    fn capabilities(&self) -> HarnessCapabilities {
        FakeHarness.capabilities()
    }
    async fn start_session(
        &self,
        opts: SessionOptions,
    ) -> Result<Box<dyn HarnessSession>, HarnessError> {
        let stuck = opts.conversation_id.to_string() == STUCK_CONV;
        let s = FakeHarness.start_session(opts).await?;
        Ok(Box::new(StuckSession(s, stuck, self.0)))
    }
    async fn purge_session(&self, dir: &Path, session_ref: &str) -> Result<(), HarnessError> {
        FakeHarness.purge_session(dir, session_ref).await
    }
}

#[async_trait]
impl HarnessSession for StuckSession {
    fn session_ref(&self) -> Option<String> {
        self.0.session_ref()
    }
    async fn run_turn(&mut self, input: TurnInput) -> Result<HarnessEventStream, HarnessError> {
        self.0.run_turn(input).await
    }
    async fn steer(&mut self, input: SteerInput) -> Result<SteerAck, HarnessError> {
        self.0.steer(input).await
    }
    async fn background_running_tool(&mut self) -> Result<SendNowOutcome, HarnessError> {
        self.0.background_running_tool().await
    }
    async fn abort(&mut self) -> Result<(), HarnessError> {
        self.0.abort().await
    }
    async fn release(&mut self) -> Result<(), HarnessError> {
        if self.1 {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep(self.2).await;
        self.0.release().await
    }
}

impl Rig {
    async fn deliver(&self, conversation: &str, turn: Uuid, epoch: u64, text: &str) {
        self.send(
            "turn.deliver",
            json!({
                "conversation_id": conversation, "turn_id": turn, "session": {"epoch": epoch},
                "model": {"provider": "p", "endpoint_id": "e", "base_url": "http://x",
                          "dialect": "anthropic_messages", "model": "m", "effort": "low", "client_env": []},
                "context_credential": "auc_turn",
                "input": {"text": text, "speaker_label": "Ann", "door": "web",
                          "recall": {"index": "", "snippets": []}, "notices": []},
                "attachments": [],
            }),
        )
        .await;
    }

    /// The bodies of the first frames that `wanted` names, in any order, all within `limit`.
    async fn within(
        &mut self,
        limit: Duration,
        what: &str,
        wanted: &[(&str, &str, Value)],
    ) -> Vec<Value> {
        let mut found: Vec<Option<Value>> = vec![None; wanted.len()];
        let got = timeout(limit, async {
            while found.iter().any(Option::is_none) {
                let m = self.from_sup.recv().await.unwrap();
                let v = serde_json::to_value(&m).unwrap();
                for (slot, (kind, key, value)) in found.iter_mut().zip(wanted) {
                    if m.kind() == *kind && v["body"][key] == *value {
                        *slot = Some(v["body"].clone());
                    }
                }
            }
        })
        .await;
        assert!(got.is_ok(), "{what}: the supervisor stalled");
        found.into_iter().flatten().collect()
    }
}

impl Rig {
    /// A whole generation whose tool list holds only `tool`; the frames that came before its
    /// `home.synced`.
    async fn sync_tool(&mut self, generation: u64, tool: &str) -> Vec<Value> {
        let parts = [
            ("tools", json!([{"name": tool}])),
            ("manifest", json!([])),
            ("final", json!([])),
        ];
        for (i, (kind, entries)) in parts.into_iter().enumerate() {
            self.send(
                "home.sync",
                json!({"generation": generation, "part_index": i, "part_count": 3,
                       "part": {"kind": kind, "entries": entries}}),
            )
            .await;
        }
        let mut before = Vec::new();
        loop {
            let m = timeout(T, self.from_sup.recv())
                .await
                .expect("no home.synced frame")
                .unwrap();
            if m.kind() == "home.synced" {
                return before;
            }
            before.push(serde_json::to_value(&m).unwrap());
        }
    }
}

/// (FIX-258) A changed tool list releases the idle sessions together: one that takes long does
/// not make the next one count as not released.
#[tokio::test]
async fn a_slow_idle_release_does_not_fail_the_next_one() {
    // Together within the one deadline of 4 s; one after the other they are not.
    let slow = Duration::from_millis(2500);
    let mut r = rig_with(TempDir::new().keep(), Arc::new(Stuck(slow))).await;
    r.sync_tool(2, "first").await;
    let conversations = [CONV, "00000000-0000-0000-0000-0000000000cb"];
    for conversation in conversations {
        let turn = Uuid::now_v7();
        r.deliver(conversation, turn, 1, "hello").await;
        r.within(T, "turn", &[("turn.result", "turn_id", json!(turn))])
            .await;
    }
    let released: Vec<Value> = r
        .sync_tool(3, "second")
        .await
        .into_iter()
        .filter(|v| v["kind"] == "session.released")
        .collect();
    assert_eq!(released.len(), conversations.len(), "{released:?}");
    assert!(
        released.iter().all(|v| v["body"]["ok"] == true),
        "{released:?}"
    );
}

/// (SEC-165) Releases that do not finish hold the supervisor's loop no longer than one
/// deadline, however many a conversation has: it reports that they did not stop, and another
/// conversation's abort is handled meanwhile.
#[tokio::test]
async fn stuck_releases_do_not_stall_the_other_conversations() {
    let mut r = rig_with(TempDir::new().keep(), Arc::new(Stuck(Duration::ZERO))).await;
    // Longer than the one deadline, far shorter than for ever.
    let limit = Duration::from_secs(10);
    let busy = Uuid::now_v7();
    r.deliver(CONV, busy, 1, "[[sleep: 120s]]").await;
    r.within(limit, "start", &[("turn.started", "turn_id", json!(busy))])
        .await;
    // A new session epoch replaces the first session, whose release is stuck.
    for epoch in [1, 2] {
        let turn = Uuid::now_v7();
        r.deliver(STUCK_CONV, turn, epoch, "hello").await;
        r.within(limit, "turn", &[("turn.result", "turn_id", json!(turn))])
            .await;
    }
    // Both of the conversation's sessions are stuck now.
    r.send("session.release", json!({"conversation_id": STUCK_CONV}))
        .await;
    r.send("turn.abort", json!({"turn_id": busy})).await;
    let got = r
        .within(
            limit,
            "release and abort",
            &[
                ("session.released", "conversation_id", json!(STUCK_CONV)),
                ("turn.result", "turn_id", json!(busy)),
            ],
        )
        .await;
    assert_eq!(got[0]["ok"], false, "{got:?}");
    assert_eq!(got[1]["status"], "aborted", "{got:?}");
}

/// What the test of a late acknowledgement decides.
#[derive(Default)]
struct Switches {
    /// The release of the first session ends once this is set.
    open: AtomicBool,
    /// It then fails, this once.
    fail: AtomicBool,
    /// What a later session says: a background command runs; a child of it is still ending.
    listing: AtomicBool,
    ending: AtomicBool,
    /// The first session's release task dies instead.
    die: AtomicBool,
    /// Later sessions' releases wait for `open` too.
    gate_all: AtomicBool,
}

/// The fake harness; only the first session's release waits.
struct Gated(Arc<Switches>, AtomicUsize);

struct GatedSession {
    inner: Box<dyn HarnessSession>,
    first: bool,
    switches: Arc<Switches>,
}

#[async_trait]
impl HarnessAdapter for Gated {
    fn id(&self) -> &'static str {
        "fake"
    }
    fn capabilities(&self) -> HarnessCapabilities {
        FakeHarness.capabilities()
    }
    async fn start_session(
        &self,
        opts: SessionOptions,
    ) -> Result<Box<dyn HarnessSession>, HarnessError> {
        Ok(Box::new(GatedSession {
            inner: FakeHarness.start_session(opts).await?,
            first: self.1.fetch_add(1, Ordering::SeqCst) == 0,
            switches: self.0.clone(),
        }))
    }
    async fn purge_session(&self, dir: &Path, session_ref: &str) -> Result<(), HarnessError> {
        FakeHarness.purge_session(dir, session_ref).await
    }
}

#[async_trait]
impl HarnessSession for GatedSession {
    fn session_ref(&self) -> Option<String> {
        self.inner.session_ref()
    }
    async fn run_turn(&mut self, input: TurnInput) -> Result<HarnessEventStream, HarnessError> {
        self.inner.run_turn(input).await
    }
    async fn steer(&mut self, input: SteerInput) -> Result<SteerAck, HarnessError> {
        self.inner.steer(input).await
    }
    async fn background_running_tool(&mut self) -> Result<SendNowOutcome, HarnessError> {
        self.inner.background_running_tool().await
    }
    async fn abort(&mut self) -> Result<(), HarnessError> {
        self.inner.abort().await
    }
    async fn release(&mut self) -> Result<(), HarnessError> {
        if self.first || self.switches.gate_all.load(Ordering::SeqCst) {
            if self.first && self.switches.die.load(Ordering::SeqCst) {
                panic!("the release task dies");
            }
            while !self.switches.open.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            if self.switches.fail.swap(false, Ordering::SeqCst) {
                return Err(HarnessError::new("stop_failed", "something still runs"));
            }
        }
        self.inner.release().await
    }
    fn background_work(&self) -> Vec<BackgroundWork> {
        let listing = !self.first && self.switches.listing.load(Ordering::SeqCst);
        listing
            .then(|| BackgroundWork {
                command: "cmd-new".into(),
                started_at: 1,
            })
            .into_iter()
            .collect()
    }
    fn still_ending(&self) -> bool {
        !self.first && self.switches.ending.load(Ordering::SeqCst)
    }
}

/// How long a test waits to say that no acknowledgement comes: several of the supervisor's
/// looks at its unfinished releases.
const QUIET: Duration = Duration::from_millis(1200);

async fn gated() -> (Rig, Arc<Switches>) {
    let switches = Arc::new(Switches::default());
    let adapter = Gated(switches.clone(), AtomicUsize::new(0));
    let rig = rig_cfg(TempDir::new().keep(), Arc::new(adapter), |cfg| {
        cfg.release_deadline = Duration::from_millis(300);
    })
    .await;
    (rig, switches)
}

impl Rig {
    /// Runs one turn of [`CONV`] to its end.
    async fn short_turn(&mut self, epoch: u64) {
        let turn = Uuid::now_v7();
        self.deliver(CONV, turn, epoch, "hello").await;
        self.within(T, "turn", &[("turn.result", "turn_id", json!(turn))])
            .await;
    }

    async fn release(&mut self) -> bool {
        self.send("session.release", json!({"conversation_id": CONV}))
            .await;
        self.next("session.released").await["ok"] == true
    }

    /// Whatever arrives within [`QUIET`], no `session.released` does.
    async fn nothing_released(&mut self, when: &str) {
        let until = tokio::time::Instant::now() + QUIET;
        while let Ok(m) = tokio::time::timeout_at(until, self.from_sup.recv()).await {
            let m = m.unwrap();
            assert_ne!(
                m.kind(),
                "session.released",
                "{when}: {}",
                serde_json::to_value(&m).unwrap()
            );
        }
    }

    /// The one late acknowledgement.
    async fn released_late(&mut self) {
        let got = timeout(Duration::from_secs(5), self.next("session.released"))
            .await
            .expect("no late session.released");
        assert_eq!(got["ok"], true, "{got}");
        self.nothing_released("after the late one").await;
    }
}

/// (FIX-258) A release that overruns the deadline answers `ok: false`; when it finishes, the
/// supervisor says so by itself, once.
#[tokio::test]
async fn a_release_that_finishes_late_is_acknowledged_once() {
    let (mut r, switches) = gated().await;
    r.short_turn(1).await;
    assert!(!r.release().await);
    r.nothing_released("while the release runs").await;
    switches.open.store(true, Ordering::SeqCst);
    r.released_late().await;
}

/// (FIX-258) A release nobody asked for (a new session epoch replaced the session) is
/// acknowledged when it finishes late too.
#[tokio::test]
async fn a_replaced_session_released_late_is_acknowledged() {
    let (mut r, switches) = gated().await;
    r.short_turn(1).await;
    r.short_turn(2).await;
    r.nothing_released("while the release runs").await;
    switches.open.store(true, Ordering::SeqCst);
    r.released_late().await;
}

/// (FIX-258) "Released" is never said early: not while a turn runs, while the live session
/// lists background work, or while a child it replaced is still ending.
#[tokio::test]
async fn the_late_acknowledgement_waits_until_nothing_runs() {
    let (mut r, switches) = gated().await;
    r.short_turn(1).await;
    assert!(!r.release().await);
    switches.listing.store(true, Ordering::SeqCst);
    let turn = Uuid::now_v7();
    r.deliver(CONV, turn, 1, "[[sleep: 120s]]").await;
    r.within(T, "start", &[("turn.started", "turn_id", json!(turn))])
        .await;
    switches.open.store(true, Ordering::SeqCst);
    r.nothing_released("while a turn runs").await;
    r.send("turn.abort", json!({"turn_id": turn})).await;
    r.within(T, "abort", &[("turn.result", "turn_id", json!(turn))])
        .await;
    r.nothing_released("while background work is listed").await;
    switches.ending.store(true, Ordering::SeqCst);
    switches.listing.store(false, Ordering::SeqCst);
    r.nothing_released("while a replaced child is ending").await;
    switches.ending.store(false, Ordering::SeqCst);
    r.released_late().await;
}

/// (FIX-258) A late release that fails is no acknowledgement: only the next request, which
/// tries again, says "released".
#[tokio::test]
async fn a_late_failure_is_not_acknowledged() {
    let (mut r, switches) = gated().await;
    r.short_turn(1).await;
    assert!(!r.release().await);
    switches.fail.store(true, Ordering::SeqCst);
    switches.open.store(true, Ordering::SeqCst);
    r.nothing_released("after a failed release").await;
    assert!(r.release().await, "the retry released it");
    r.nothing_released("after the normal acknowledgement").await;
}

/// (FIX-258) A release task that died bars the unasked acknowledgement only until the
/// conversation is acknowledged again in the normal way.
#[tokio::test]
async fn a_normal_acknowledgement_forgets_a_lost_release() {
    let (mut r, switches) = gated().await;
    switches.die.store(true, Ordering::SeqCst);
    r.short_turn(1).await;
    assert!(!r.release().await, "a dead task is not released");
    r.nothing_released("after a lost release").await;
    assert!(r.release().await, "nothing is left to release");
    // A later release that is only slow is acknowledged late as usual.
    switches.gate_all.store(true, Ordering::SeqCst);
    r.short_turn(2).await;
    assert!(!r.release().await);
    switches.open.store(true, Ordering::SeqCst);
    r.released_late().await;
}

/// (FIX-258) A release put off because a turn runs ended no session, so nothing is said when
/// the turn ends.
#[tokio::test]
async fn a_release_put_off_by_a_turn_is_not_acknowledged_by_itself() {
    let (mut r, _) = gated().await;
    let turn = Uuid::now_v7();
    r.deliver(CONV, turn, 1, "[[sleep: 120s]]").await;
    r.within(T, "start", &[("turn.started", "turn_id", json!(turn))])
        .await;
    assert!(!r.release().await);
    r.send("turn.abort", json!({"turn_id": turn})).await;
    r.within(T, "abort", &[("turn.result", "turn_id", json!(turn))])
        .await;
    r.nothing_released("after the turn").await;
}
