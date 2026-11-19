//! Concrete harness adapters (the vendor-neutral traits live in [`crate::harness`]) and the
//! compile-time registry the supervisor instantiates them from (`docs/12-supervisor-harness.md`
//! §Harness selection and adapter contract, D-122, SEC-66).

pub mod claude_code;
pub mod codex;
pub mod goose;
pub mod opencode;
pub mod pi;
pub(crate) mod provider_error;
pub mod render;

use std::sync::Arc;

use crate::fake::FakeHarness;
use crate::harness::{HarnessAdapter, HarnessError};

/// (FIX-151, FIX-176) Registers a harness child for its conversation: platform-tool calls from the
/// process's descendants act for that conversation, and the returned guard ends the registration
/// when it is dropped (hold it until the process exits). Every launcher calls this right after
/// spawning, so a new harness cannot forget it.
pub fn register_harness(
    child: &tokio::process::Child,
    conversation: uuid::Uuid,
) -> Option<crate::proc_ancestry::Registration> {
    child
        .id()
        .and_then(|pid| crate::proc_ancestry::HarnessRegistry::global().register(pid, conversation))
}

/// Error code of a boot (or purge) that names a harness this build or image cannot run.
pub const UNSUPPORTED: &str = "harness_unsupported";

/// Ids of the compiled-in adapters.
pub const KNOWN: [&str; 6] = ["claude-code", "codex", "pi", "goose", "opencode", "fake"];

fn unsupported(msg: String) -> HarnessError {
    HarnessError::new(UNSUPPORTED, msg)
}

/// The adapter for harness `id`, by the boot document's `harness.id`. An id this build does not
/// know, or a runtime that is not in the image, is [`UNSUPPORTED`]: there is no fallback to
/// another harness, ever (SEC-66).
pub fn adapter_for(id: &str) -> Result<Arc<dyn HarnessAdapter>, HarnessError> {
    adapter_with(id, |p| p.exists())
}

/// [`adapter_for`] with the "is this runtime installed" probe injected (tests).
pub fn adapter_with(
    id: &str,
    exists: impl Fn(&std::path::Path) -> bool,
) -> Result<Arc<dyn HarnessAdapter>, HarnessError> {
    match id {
        "fake" => Ok(Arc::new(FakeHarness)),
        "claude-code" => {
            let l = claude_code::launcher::ProcessLauncher::default();
            if !exists(&l.program) {
                return Err(unsupported(format!(
                    "the claude-code runtime is not in this image ({})",
                    l.program.display()
                )));
            }
            Ok(Arc::new(claude_code::ClaudeCodeAdapter::with_launcher(
                Arc::new(l),
            )))
        }
        "codex" => {
            let l = codex::launcher::ProcessLauncher::default();
            if !exists(&l.program) {
                return Err(unsupported(format!(
                    "the codex runtime is not in this image ({})",
                    l.program.display()
                )));
            }
            Ok(Arc::new(codex::CodexAdapter::with_launcher(Arc::new(l))))
        }
        "pi" => {
            let l = pi::launcher::ProcessLauncher::default();
            if !exists(&l.program) {
                return Err(unsupported(format!(
                    "the pi runtime is not in this image ({})",
                    l.program.display()
                )));
            }
            Ok(Arc::new(pi::PiAdapter::with_launcher(Arc::new(l))))
        }
        "goose" => {
            let l = goose::launcher::ProcessLauncher::default();
            if !exists(&l.program) {
                return Err(unsupported(format!(
                    "the goose runtime is not in this image ({})",
                    l.program.display()
                )));
            }
            Ok(Arc::new(goose::GooseAdapter::with_launcher(Arc::new(l))))
        }
        "opencode" => {
            let l = opencode::launcher::ProcessLauncher::default();
            if !exists(&l.program) {
                return Err(unsupported(format!(
                    "the opencode runtime is not in this image ({})",
                    l.program.display()
                )));
            }
            Ok(Arc::new(opencode::OpenCodeAdapter::with_launcher(
                Arc::new(l),
            )))
        }
        other => Err(unsupported(format!(
            "harness `{other}` is not available in this build"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A harness stand-in: the child's own child reports its pid on stdout and then waits, so the
    /// pid read back is that of a *descendant* of the process the launcher started.
    fn descendant_script() -> Vec<String> {
        vec![
            "-c".into(),
            "/bin/sh -c 'echo $$; exec /bin/sleep 30'; true".into(),
        ]
    }

    async fn first_pid(rx: &mut tokio::sync::mpsc::Receiver<String>) -> u32 {
        let line = tokio::time::timeout(std::time::Duration::from_secs(20), rx.recv())
            .await
            .expect("the stand-in harness printed no pid")
            .expect("stdout closed");
        line.trim().parse().expect("a pid")
    }

    /// (FIX-176) The conformance case for the launchers: a process the harness starts is admitted
    /// for its conversation, and one outside the harness is not (`session_not_caller`). Every
    /// launcher in the registry is in this list; a new harness that forgets to register fails here.
    #[tokio::test]
    async fn every_launcher_registers_the_harness_process_for_its_conversation() {
        use std::path::PathBuf;
        let reg = crate::proc_ancestry::HarnessRegistry::global();
        let outside = std::process::id();
        let sh = PathBuf::from("/bin/sh");
        let cwd = std::env::temp_dir();
        let mut seen = Vec::new();

        macro_rules! check {
            ($name:literal, $m:ident, $spec:expr, $io:ident, $kill:expr) => {{
                let conv = uuid::Uuid::now_v7();
                let l = $m::launcher::ProcessLauncher {
                    program: sh.clone(),
                };
                let mut $io = $m::launcher::Launcher::launch(&l, ($spec)(conv))
                    .await
                    .unwrap();
                let pid = first_pid(&mut $io.lines).await;
                assert_eq!(
                    reg.conversation_of(pid),
                    Some(conv),
                    "{}: a descendant of the harness process is not admitted",
                    $name
                );
                assert_eq!(
                    reg.conversation_of(outside),
                    None,
                    "{}: a process outside the harness is admitted",
                    $name
                );
                ($kill)($io.kill);
                seen.push($name);
            }};
        }
        let kill = |k: Box<dyn FnOnce() + Send>| k();

        check!(
            "claude-code",
            claude_code,
            |c| claude_code::config::LaunchSpec {
                args: descendant_script(),
                env: vec![],
                cwd: cwd.clone(),
                conversation_id: c,
            },
            io,
            kill
        );
        check!(
            "codex",
            codex,
            |c| codex::config::LaunchSpec {
                args: descendant_script(),
                env: vec![],
                cwd: cwd.clone(),
                conversation_id: c,
                stdin: String::new(),
            },
            io,
            kill
        );
        check!(
            "pi",
            pi,
            |c| pi::config::LaunchSpec {
                args: descendant_script(),
                env: vec![],
                cwd: cwd.clone(),
                conversation_id: c,
            },
            io,
            kill
        );
        check!(
            "goose",
            goose,
            |c| goose::config::LaunchSpec {
                args: descendant_script(),
                env: vec![],
                cwd: cwd.clone(),
                conversation_id: c,
                stdin: String::new(),
            },
            io,
            kill
        );
        check!(
            "opencode",
            opencode,
            |c| opencode::config::LaunchSpec {
                args: descendant_script(),
                env: vec![],
                cwd: cwd.clone(),
                conversation_id: c,
                stdin: String::new(),
            },
            io,
            kill
        );
        assert_eq!(
            seen,
            ["claude-code", "codex", "pi", "goose", "opencode"],
            "every adapter in KNOWN (but fake) is covered"
        );
    }

    /// (FIX-176) claude-code's stdout is read with the shared bounded reader: a line over 16 MiB is
    /// replaced by the over-long marker (never buffered whole) and the stream continues.
    #[tokio::test]
    async fn claude_code_cuts_an_overlong_line_and_the_stream_continues() {
        let l = claude_code::launcher::ProcessLauncher {
            program: "/bin/sh".into(),
        };
        let spec = claude_code::config::LaunchSpec {
            args: vec![
                "-c".into(),
                "/usr/bin/head -c 17000000 /dev/zero | /usr/bin/tr '\\0' a; echo; echo after"
                    .into(),
            ],
            env: vec![],
            cwd: std::env::temp_dir(),
            conversation_id: uuid::Uuid::now_v7(),
        };
        let mut io = claude_code::launcher::Launcher::launch(&l, spec)
            .await
            .unwrap();
        async fn recv(rx: &mut tokio::sync::mpsc::Receiver<String>) -> String {
            tokio::time::timeout(std::time::Duration::from_secs(60), rx.recv())
                .await
                .expect("no line")
                .expect("closed")
        }
        assert_eq!(
            recv(&mut io.lines).await,
            claude_code::launcher::OVERLONG_LINE
        );
        assert_eq!(recv(&mut io.lines).await, "after");
        let mut m = claude_code::stream::StreamMapper::new();
        let mapped = m.map_line(claude_code::launcher::OVERLONG_LINE);
        assert!(matches!(
            mapped.terminal,
            Some(crate::harness::HarnessEvent::Error { .. })
        ));
    }

    #[test]
    fn known_ids_resolve_when_the_runtime_is_present() {
        for id in KNOWN {
            let a = adapter_with(id, |_| true).unwrap();
            assert_eq!(a.id(), id);
        }
    }

    #[test]
    fn unknown_id_or_missing_runtime_is_harness_unsupported_never_a_fallback() {
        for id in ["", "gemini", "CODEX", "claude_code"] {
            assert_eq!(adapter_with(id, |_| true).err().unwrap().code, UNSUPPORTED);
        }
        for id in ["claude-code", "codex", "pi", "goose", "opencode"] {
            let e = adapter_with(id, |_| false).err().unwrap();
            assert_eq!(e.code, UNSUPPORTED, "{id}");
        }
        // `fake` has no runtime to miss.
        assert!(adapter_with("fake", |_| false).is_ok());
    }
}
