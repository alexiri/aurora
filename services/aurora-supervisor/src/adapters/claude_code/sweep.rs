//! (FIX-265) Everything one harness child started, found in `/proc` and ended.
//!
//! A process belongs to the child when it is the child, is in the child's process group, carries
//! the child's own [`TAG_ENV`] value in its environment, or descends from such a process. The tag
//! is what finds a process that left the group and lost its parent (`setsid -f`, a double fork):
//! it was reparented to the supervisor, so no parent chain leads to it any more. (The sandbox has
//! no delegated cgroup.) A process that left the group and the tree **and** dropped the variable,
//! or hides its environment (not dumpable), is not found: the end of the sandbox ends it.
//!
//! Every signal goes to a process instance pinned by its pid and `/proc` start time, and never to
//! the supervisor itself.

#[cfg(not(target_os = "linux"))]
compile_error!(
    "the supervisor runs in a Linux sandbox: it needs /proc to end what a harness starts"
);

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use rustix::process::{Pid, Signal, kill_process, kill_process_group};
use tokio::time::Instant;

/// The environment variable that marks what one harness child started; its value is that
/// child's own.
pub(super) const TAG_ENV: &str = "AURORA_HARNESS_CHILD";

/// Grace between the HUP and the KILL (the same as `ExecProc` in `crate::exec`).
const KILL_GRACE: Duration = Duration::from_secs(2);
/// How long the KILLs are repeated before the end counts as failed.
const VERIFY_LIMIT: Duration = Duration::from_secs(5);
const SCAN_EVERY: Duration = Duration::from_millis(50);
/// This many scans in a row must find nothing: one scan can miss a process in mid-`exec`.
const EMPTY_SCANS: u32 = 2;
/// How deep the parent chain is followed below a seed, and how many processes one walk returns
/// (the cap is on the result, never on the `/proc` snapshot it is walked from).
const WALK_MAX_DEPTH: usize = 32;
const WALK_MAX_COUNT: usize = 4096;

#[derive(Debug, Clone, Copy)]
struct Proc {
    pid: i32,
    ppid: i32,
    group: i32,
    zombie: bool,
    start: u64,
}

fn stat_of(pid: i32) -> Option<Proc> {
    // Bytes, not text: the kernel cuts `comm` to 15 bytes, also inside a character.
    parse_stat(pid, &std::fs::read(format!("/proc/{pid}/stat")).ok()?)
}

fn parse_stat(pid: i32, stat: &[u8]) -> Option<Proc> {
    // `comm` may hold spaces and brackets, so the fields are read after the last ')'.
    let after = &stat[stat.iter().rposition(|b| *b == b')')? + 1..];
    let f: Vec<&str> = std::str::from_utf8(after)
        .ok()?
        .split_whitespace()
        .collect();
    Some(Proc {
        pid,
        zombie: matches!(*f.first()?, "Z" | "X"),
        ppid: f.get(1)?.parse().ok()?,
        group: f.get(2)?.parse().ok()?,
        start: f.get(19)?.parse().ok()?,
    })
}

fn snapshot() -> Vec<Proc> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter_map(stat_of) // `None`: gone between the listing and the read
        .collect()
}

/// `true` once the leader exited — still unreaped, or already reaped.
pub(super) fn exited(pid: i32) -> bool {
    stat_of(pid).is_none_or(|p| p.zombie)
}

/// What one harness child started. `root` is the child, which leads its own process group.
pub(super) struct ChildTree {
    root: i32,
    root_start: Option<u64>,
    /// `AURORA_HARNESS_CHILD=<this child's value>`.
    tag: Vec<u8>,
    /// Every instance a scan has found: one that later drops the tag and loses its parent is
    /// still known.
    seen: HashSet<(i32, u64)>,
}

impl ChildTree {
    /// Call while the child is unreaped, so that the start time read here is its own.
    pub(super) fn new(root: i32, tag_value: &str) -> Self {
        Self {
            root,
            root_start: stat_of(root).map(|p| p.start),
            tag: format!("{TAG_ENV}={tag_value}").into_bytes(),
            seen: HashSet::new(),
        }
    }

    fn tagged(&self, pid: i32) -> bool {
        std::fs::read(format!("/proc/{pid}/environ"))
            .is_ok_and(|env| env.split(|b| *b == 0).any(|v| v == self.tag))
    }

    /// The processes of the tree that still run, and whether the child's pid still names its
    /// own group (a reaped child's number may be reused by a stranger).
    fn running(&mut self) -> (Vec<Proc>, bool) {
        let snap = snapshot();
        let own_group = !snap
            .iter()
            .any(|p| p.pid == self.root && Some(p.start) != self.root_start);
        let me = i32::try_from(std::process::id()).unwrap_or(1);
        let signallable = |p: &Proc| p.pid > 1 && p.pid != me;
        let mut children: HashMap<i32, Vec<Proc>> = HashMap::new();
        for p in &snap {
            children.entry(p.ppid).or_default().push(*p);
        }
        let mut queue: VecDeque<(Proc, usize)> = snap
            .iter()
            .filter(|p| signallable(p))
            .filter(|p| {
                self.seen.contains(&(p.pid, p.start))
                    || (own_group && (p.pid == self.root || p.group == self.root))
                    || self.tagged(p.pid)
            })
            .map(|p| (*p, 0))
            .collect();
        let mut visited = HashSet::new();
        let mut found = Vec::new();
        while let Some((p, depth)) = queue.pop_front() {
            if found.len() >= WALK_MAX_COUNT {
                break;
            }
            if !visited.insert(p.pid) {
                continue;
            }
            self.seen.insert((p.pid, p.start));
            if !p.zombie {
                found.push(p);
            }
            if depth < WALK_MAX_DEPTH {
                for c in children.get(&p.pid).into_iter().flatten() {
                    if signallable(c) {
                        queue.push_back((*c, depth + 1));
                    }
                }
            }
        }
        (found, own_group)
    }

    fn signal(&self, found: &[Proc], own_group: bool, signal: Signal) {
        if own_group
            && found.iter().any(|p| p.group == self.root)
            && let Some(g) = Pid::from_raw(self.root)
        {
            let _ = kill_process_group(g, signal);
        }
        for p in found {
            if stat_of(p.pid).is_some_and(|now| now.start == p.start)
                && let Some(pid) = Pid::from_raw(p.pid)
            {
                let _ = kill_process(pid, signal);
            }
        }
    }

    /// Ends the tree: with `hup`, a HUP and a grace for a clean exit first, which ends as soon
    /// as nothing runs; then KILL, repeated until nothing is found. `Err` says in words that
    /// something is still running.
    pub(super) async fn end(&mut self, hup: bool) -> Result<(), String> {
        if hup {
            let (found, own_group) = self.running();
            self.signal(&found, own_group, Signal::HUP);
            let grace = Instant::now() + KILL_GRACE;
            while Instant::now() < grace && !self.running().0.is_empty() {
                tokio::time::sleep(SCAN_EVERY).await;
            }
        }
        let limit = Instant::now() + VERIFY_LIMIT;
        let mut empty = 0;
        loop {
            let (found, own_group) = self.running();
            if found.is_empty() {
                empty += 1;
                if empty >= EMPTY_SCANS {
                    return Ok(());
                }
            } else {
                empty = 0;
                if Instant::now() >= limit {
                    return Err(format!(
                        "{} processes that the harness started did not end",
                        found.len()
                    ));
                }
                self.signal(&found, own_group, Signal::KILL);
            }
            tokio::time::sleep(SCAN_EVERY).await;
        }
    }
}
