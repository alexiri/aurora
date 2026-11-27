//! (FIX-265) Everything one harness child started, found in `/proc` and ended.
//!
//! A process belongs to the child when it is the child, is in the child's process group, carries
//! the child's own [`TAG_ENV`] value in its environment, or descends from such a process. The tag
//! is what finds a process that left the group and lost its parent (`setsid -f`, a double fork):
//! it was reparented to the supervisor, so no parent chain leads to it any more. (The sandbox has
//! no delegated cgroup.) A process that left the group and the tree **and** dropped the variable,
//! hides its environment (not dumpable) or holds it past the first [`ENVIRON_MAX`] bytes, is not
//! found: the end of the sandbox ends it.
//!
//! Every signal goes to one process instance: through a pidfd where the kernel has them, and in
//! any case only while the pid still has the start time the scan saw. Never to the supervisor.

#[cfg(not(target_os = "linux"))]
compile_error!(
    "the supervisor runs in a Linux sandbox: it needs /proc to end what a harness starts"
);

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read as _;
use std::sync::{Arc, Mutex, TryLockError};
use std::time::Duration;

use rustix::io::Errno;
use rustix::process::{
    Pid, PidfdFlags, Signal, kill_process, kill_process_group, pidfd_open, pidfd_send_signal,
};
use tokio::time::{Instant, timeout_at};

/// The environment variable that marks what one harness child started; its value is that
/// child's own.
pub(super) const TAG_ENV: &str = "AURORA_HARNESS_CHILD";
/// How much of a process's environment is searched for the tag. The harness child gets its
/// variables sorted by name, so the tag is near the start; a shell below it may reorder them.
const ENVIRON_MAX: u64 = 128 * 1024;

/// Grace between the HUP and the KILL (the same as `ExecProc` in `crate::exec`).
const KILL_GRACE: Duration = Duration::from_secs(2);
/// The one deadline of a whole end: the HUP, the grace, the KILLs and every scan.
const SWEEP_LIMIT: Duration = Duration::from_secs(7);
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
    exited_from(std::fs::read(format!("/proc/{pid}/stat")))
}

/// What reading the leader's `stat` says about it. Only "there is no such process" and the
/// zombie state say that it exited; any other failure (out of descriptors or memory) says
/// nothing, and the next poll asks again.
fn exited_from(stat: std::io::Result<Vec<u8>>) -> bool {
    match stat {
        Ok(s) => parse_stat(0, &s).is_some_and(|p| p.zombie),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// Signals the instance `p`, never a later process with its pid.
fn signal_instance(p: &Proc, signal: Signal) {
    let Some(pid) = Pid::from_raw(p.pid) else {
        return;
    };
    let same = || stat_of(p.pid).is_some_and(|now| now.start == p.start);
    // The pidfd names whatever has the pid when it is opened; the start time read after that
    // says that it is ours, and the descriptor cannot move to another process afterwards.
    match pidfd_open(pid, PidfdFlags::empty()) {
        Ok(fd) => {
            if same()
                && matches!(
                    pidfd_send_signal(&fd, signal),
                    Err(Errno::NOSYS | Errno::PERM)
                )
                && same()
            {
                let _ = kill_process(pid, signal);
            }
        }
        Err(Errno::SRCH) => {}
        // No pidfds here (an old kernel, a seccomp filter): the start time alone.
        Err(_) => {
            if same() {
                let _ = kill_process(pid, signal);
            }
        }
    }
}

/// The scanning half, which reads `/proc` and so runs off the async threads.
struct Scanner {
    root: i32,
    root_start: Option<u64>,
    /// `AURORA_HARNESS_CHILD=<this child's value>`.
    tag: Vec<u8>,
    /// Every instance a scan has found: one that later drops the tag and loses its parent is
    /// still known.
    seen: HashSet<(i32, u64)>,
    /// Instances whose environment was read and holds no tag: not read again in this end.
    untagged: HashSet<(i32, u64)>,
}

impl Scanner {
    fn tagged(&mut self, p: &Proc) -> bool {
        let id = (p.pid, p.start);
        if self.untagged.contains(&id) {
            return false;
        }
        let mut env = Vec::new();
        let read = std::fs::File::open(format!("/proc/{}/environ", p.pid))
            .and_then(|f| f.take(ENVIRON_MAX).read_to_end(&mut env));
        let tagged = env.split(|b| *b == 0).any(|v| v == self.tag);
        // An empty answer may be a process in mid-`exec`: ask again next time.
        if read.is_ok() && !env.is_empty() && !tagged {
            self.untagged.insert(id);
        }
        tagged
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
        let mut queue: VecDeque<(Proc, usize)> = VecDeque::new();
        for p in snap.iter().filter(|p| signallable(p)) {
            if self.seen.contains(&(p.pid, p.start))
                || (own_group && (p.pid == self.root || p.group == self.root))
                || self.tagged(p)
            {
                queue.push_back((*p, 0));
            }
        }
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

    /// One scan, then `signal` to what it found; returns how many that was.
    fn pass(&mut self, signal: Option<Signal>) -> usize {
        let (found, own_group) = self.running();
        if let Some(signal) = signal {
            // The group is the child's own for as long as the child is unwaited, which it is
            // until its owner has seen this end succeed (the reaper leaves owned children).
            if own_group
                && found.iter().any(|p| p.group == self.root)
                && let Some(g) = Pid::from_raw(self.root)
            {
                let _ = kill_process_group(g, signal);
            }
            for p in &found {
                signal_instance(p, signal);
            }
        }
        found.len()
    }
}

/// What one harness child started. `root` is the child, which leads its own process group.
pub(super) struct ChildTree(Arc<Mutex<Scanner>>);

impl ChildTree {
    /// Call while the child is unreaped, so that the start time read here is its own.
    pub(super) fn new(root: i32, tag_value: &str) -> Self {
        Self(Arc::new(Mutex::new(Scanner {
            root,
            root_start: stat_of(root).map(|p| p.start),
            tag: format!("{TAG_ENV}={tag_value}").into_bytes(),
            seen: HashSet::new(),
            untagged: HashSet::new(),
        })))
    }

    /// [`Scanner::pass`] on a blocking thread, given up at `deadline`. A scan that hangs keeps
    /// the scanner, so a later one says so instead of piling up behind it.
    async fn pass(&self, signal: Option<Signal>, deadline: Instant) -> Result<usize, String> {
        let scanner = self.0.clone();
        let scan = tokio::task::spawn_blocking(move || match scanner.try_lock() {
            Ok(mut s) => Some(s.pass(signal)),
            Err(TryLockError::Poisoned(p)) => Some(p.into_inner().pass(signal)),
            Err(TryLockError::WouldBlock) => None,
        });
        match timeout_at(deadline, scan).await {
            Ok(Ok(Some(n))) => Ok(n),
            Ok(Ok(None)) => Err("an earlier scan of the processes is still running".into()),
            Ok(Err(e)) => Err(format!("the scan of the processes failed: {e}")),
            Err(_) => Err("the scan of the processes did not finish in time".into()),
        }
    }

    /// Ends the tree: with `hup`, a HUP and a grace for a clean exit first, which ends as soon
    /// as nothing runs; then KILL, repeated until nothing is found. `Err` says in words that
    /// something may still be running.
    pub(super) async fn end(&mut self, hup: bool) -> Result<(), String> {
        let deadline = Instant::now() + SWEEP_LIMIT;
        if let Ok(mut s) = self.0.try_lock() {
            s.untagged.clear();
        }
        if hup {
            let mut left = self.pass(Some(Signal::HUP), deadline).await?;
            let grace = Instant::now() + KILL_GRACE;
            while left > 0 && Instant::now() < grace {
                tokio::time::sleep(SCAN_EVERY).await;
                left = self.pass(None, deadline).await?;
            }
        }
        let mut empty = 0;
        loop {
            let found = self.pass(Some(Signal::KILL), deadline).await?;
            if found == 0 {
                empty += 1;
                if empty >= EMPTY_SCANS {
                    return Ok(());
                }
            } else {
                empty = 0;
                if Instant::now() >= deadline {
                    return Err(format!(
                        "{found} processes that the harness started did not end"
                    ));
                }
            }
            tokio::time::sleep(SCAN_EVERY).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (SEC-165) Only "no such process" or a zombie says that the leader exited: a read that
    /// fails for want of a file descriptor must not start the kill of a live child.
    #[test]
    fn a_failed_read_does_not_say_that_the_leader_exited() {
        let err = |code: Errno| Err(std::io::Error::from(code));
        assert!(!exited_from(err(Errno::MFILE)));
        assert!(!exited_from(err(Errno::NOMEM)));
        assert!(exited_from(err(Errno::NOENT)));
        let stat = |text: &str| Ok(text.as_bytes().to_vec());
        assert!(exited_from(stat(
            "7 (sh) Z 1 7 7 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 5"
        )));
        assert!(!exited_from(stat(
            "7 (s) h) S 1 7 7 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 5"
        )));
    }
}
