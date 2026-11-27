//! Which harness process a tools-socket caller descends from (FIX-151, D-301, SEC-121).
//!
//! The tools socket cannot trust the `session` a request names: the sandbox runs one uid, so any
//! process can write any conversation id, and any process can read another's environment. What
//! agent code cannot forge is the kernel's view: the peer pid of the connection (`SO_PEERCRED`)
//! and the parent chain in `/proc`. A harness adapter registers the process it starts
//! ([`HarnessRegistry::register`]); a call is accepted for a conversation only when the caller's
//! parent chain reaches a registered process of that conversation. This module knows nothing about
//! any particular harness: every adapter's launcher calls the same `register`.
//!
//! A process that daemonizes away from the harness (reparented to the subreaper) is refused by
//! design.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use uuid::Uuid;

/// Parent chains longer than this are refused (a loop or a very deep tree is not a harness's
/// bridge).
pub const MAX_DEPTH: usize = 64;

/// The two fields of `/proc/<pid>/stat` the walk needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    pub ppid: u32,
    /// Field 22: clock ticks after boot at which the process started. Identifies a process
    /// across pid reuse.
    pub start_time: u64,
}

/// Parses the content of `/proc/<pid>/stat`. `comm` (field 2) sits in parentheses and may itself
/// hold spaces and parentheses, so the fields are counted from the **last** `)`.
pub fn parse_stat(content: &str) -> Option<Stat> {
    let rest = &content[content.rfind(')')? + 1..];
    // After the `)`: state(3) ppid(4) ... starttime(22): ppid is index 1, starttime index 19.
    let mut fields = rest.split_whitespace();
    let ppid = fields.nth(1)?.parse().ok()?;
    let start_time = fields.nth(17)?.parse().ok()?;
    Some(Stat { ppid, start_time })
}

/// Reads one process's [`Stat`]; `None` when it does not exist (any more).
pub fn read_stat(pid: u32) -> Option<Stat> {
    // Bytes, not text: the kernel cuts the name to 15 bytes, also inside a character.
    let stat = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    parse_stat(&String::from_utf8_lossy(&stat))
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    conversation: Uuid,
    start_time: u64,
}

/// `harness pid -> conversation` for the harness processes this supervisor started and that are
/// still running.
#[derive(Debug, Default)]
pub struct HarnessRegistry {
    inner: Mutex<HashMap<u32, Entry>>,
}

/// Keeps a harness registered; dropping it (the process exited, or its turn ended) removes the
/// entry, unless the pid has been registered again since.
#[derive(Debug)]
pub struct Registration {
    registry: Arc<HarnessRegistry>,
    pid: u32,
    start_time: u64,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut g = self.registry.lock();
        if g.get(&self.pid)
            .is_some_and(|e| e.start_time == self.start_time)
        {
            g.remove(&self.pid);
        }
    }
}

impl HarnessRegistry {
    /// The registry the real launchers and the supervisor's tools hub share.
    pub fn global() -> Arc<Self> {
        static GLOBAL: OnceLock<Arc<HarnessRegistry>> = OnceLock::new();
        GLOBAL.get_or_init(Arc::default).clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u32, Entry>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Registers a running harness process for a conversation, with its start time read now.
    /// `None` when the process is already gone (nothing to register).
    pub fn register(self: &Arc<Self>, pid: u32, conversation: Uuid) -> Option<Registration> {
        let start_time = read_stat(pid)?.start_time;
        Some(self.register_with_start(pid, conversation, start_time))
    }

    /// [`Self::register`] with a given start time (tests simulate a recycled pid with it).
    pub fn register_with_start(
        self: &Arc<Self>,
        pid: u32,
        conversation: Uuid,
        start_time: u64,
    ) -> Registration {
        self.lock().insert(
            pid,
            Entry {
                conversation,
                start_time,
            },
        );
        Registration {
            registry: self.clone(),
            pid,
            start_time,
        }
    }

    /// The conversation of the registered harness that `pid` is, or descends from. The parent
    /// chain is read from `/proc` once per call; a process that exits mid-walk is refused.
    pub fn conversation_of(&self, pid: u32) -> Option<Uuid> {
        self.conversation_with(pid, read_stat)
    }

    fn conversation_with(&self, pid: u32, stat: impl Fn(u32) -> Option<Stat>) -> Option<Uuid> {
        let entries = self.lock().clone();
        let mut cur = pid;
        for _ in 0..MAX_DEPTH {
            if cur <= 1 {
                return None;
            }
            let s = stat(cur)?;
            if let Some(e) = entries.get(&cur) {
                // The same pid with another start time is an unrelated, later process.
                return (e.start_time == s.start_time).then_some(e.conversation);
            }
            cur = s.ppid;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_with_plain_comm() {
        let s = "1234 (bash) S 1000 1234 1234 34816 1234 4194304 100 0 0 0 0 0 0 0 20 0 1 0 987654 1000 100 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0";
        assert_eq!(
            parse_stat(s),
            Some(Stat {
                ppid: 1000,
                start_time: 987_654
            })
        );
    }

    #[test]
    fn stat_with_spaces_and_parentheses_in_comm() {
        // comm is `a) (b c` : fields must be counted from the last `)`.
        let s = "77 (a) (b c) R 55 77 77 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 424242 0 0";
        assert_eq!(
            parse_stat(s),
            Some(Stat {
                ppid: 55,
                start_time: 424_242
            })
        );
    }

    #[test]
    fn truncated_or_garbled_stat_is_none() {
        assert_eq!(parse_stat(""), None);
        assert_eq!(parse_stat("1 (x)"), None);
        assert_eq!(parse_stat("1 (x) S 0 1 1"), None);
        assert_eq!(
            parse_stat("1 (x) S notanumber 1 1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 5"),
            None
        );
    }

    fn chain(pairs: &[(u32, u32, u64)]) -> impl Fn(u32) -> Option<Stat> + '_ {
        move |pid| {
            pairs
                .iter()
                .find(|(p, _, _)| *p == pid)
                .map(|&(_, ppid, start_time)| Stat { ppid, start_time })
        }
    }

    #[test]
    fn walk_reaches_the_harness_through_its_descendants() {
        let reg = Arc::new(HarnessRegistry::default());
        let conv = Uuid::from_u128(1);
        let _g = reg.register_with_start(100, conv, 5);
        // 400 (bridge) -> 300 (node) -> 100 (harness)
        let t = [(400, 300, 9), (300, 100, 7), (100, 2, 5), (2, 1, 1)];
        assert_eq!(reg.conversation_with(400, chain(&t)), Some(conv));
        assert_eq!(reg.conversation_with(100, chain(&t)), Some(conv));
    }

    #[test]
    fn walk_refuses_a_process_outside_every_harness() {
        let reg = Arc::new(HarnessRegistry::default());
        let _g = reg.register_with_start(100, Uuid::from_u128(1), 5);
        // a web-terminal shell: 500 -> 450 -> 1
        let t = [(500, 450, 9), (450, 1, 8), (100, 2, 5)];
        assert_eq!(reg.conversation_with(500, chain(&t)), None);
        // reparented to pid 1 (daemonized away from the harness)
        assert_eq!(reg.conversation_with(300, chain(&[(300, 1, 7)])), None);
    }

    #[test]
    fn a_recycled_pid_does_not_match() {
        let reg = Arc::new(HarnessRegistry::default());
        let _g = reg.register_with_start(100, Uuid::from_u128(1), 5);
        // pid 100 is now another process (start time 6)
        let t = [(400, 100, 9), (100, 2, 6)];
        assert_eq!(reg.conversation_with(400, chain(&t)), None);
    }

    #[test]
    fn walk_is_depth_bounded_and_survives_loops() {
        let reg = Arc::new(HarnessRegistry::default());
        // 3 -> 2 -> 3 -> ... never reaches a harness or pid 1
        let t = [(3, 2, 1), (2, 3, 1)];
        assert_eq!(reg.conversation_with(3, chain(&t)), None);
        // a chain of MAX_DEPTH + 5 processes ending in the harness is too deep
        let _g = reg.register_with_start(10, Uuid::from_u128(1), 1);
        let deep = |pid: u32| {
            (pid >= 10).then(|| Stat {
                ppid: if pid == 10 { 1 } else { pid - 1 },
                start_time: 1,
            })
        };
        assert_eq!(
            reg.conversation_with(10 + 5, deep),
            Some(Uuid::from_u128(1))
        );
        assert_eq!(reg.conversation_with(10 + MAX_DEPTH as u32 + 5, deep), None);
    }

    #[test]
    fn dropping_the_registration_removes_the_entry() {
        let reg = Arc::new(HarnessRegistry::default());
        let g = reg.register_with_start(100, Uuid::from_u128(1), 5);
        let t = [(100, 2, 5)];
        assert!(reg.conversation_with(100, chain(&t)).is_some());
        drop(g);
        assert_eq!(reg.conversation_with(100, chain(&t)), None);
    }
}
