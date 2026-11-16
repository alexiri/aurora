//! (FIX-155) The web terminal's `claude` and `codex` wrappers (`images/sandbox/bin`) read
//! `/run/aurora/harness.env` as DATA, never as shell, and call their own helpers by absolute
//! path. The real wrapper text runs under `sh` in a temp dir with its three hard-coded paths
//! (the pinned binary, the env file, the state directory) pointed at the dir; the "binary" is a
//! script that records its environment and arguments. Not a security boundary: the terminal is
//! the agent's own environment (docs/17 residual); these tests pin the behaviour that a tampered
//! file does not execute as shell when someone runs `claude`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Output;

use crate::common::TempDir;
use aurora_protocol::runner::{ModelChoice, TerminalModel, TerminalModels};
use aurora_supervisor::harness_env::{HarnessEnvConfig, render};
// The only way a process is created in this crate (SEC-34, I-23): the scrubbed environment plus
// the `PATH` a test chooses.
use aurora_supervisor::spawn::command;

const CLAUDE: &str = include_str!("../../../../images/sandbox/bin/claude");
const CODEX: &str = include_str!("../../../../images/sandbox/bin/codex");

struct Rig {
    dir: TempDir,
    wrapper: PathBuf,
    envfile: PathBuf,
    state: PathBuf,
}

fn exe(p: &Path, body: &str) {
    std::fs::write(p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

impl Rig {
    /// `which` = "claude" | "codex".
    fn new(which: &str) -> Self {
        let dir = TempDir::new();
        let d = dir.path().to_owned();
        let (text, bin_line, state) = match which {
            "claude" => (
                CLAUDE,
                "bin=/opt/aurora/harness/claude-code/node_modules/.bin/claude",
                "/workspace/.aurora/terminal/claude",
            ),
            _ => (
                CODEX,
                "bin=/opt/aurora/harness/codex/codex",
                "/workspace/.aurora/terminal/codex",
            ),
        };
        let fake = d.join("fake-bin");
        let out = d.join("out");
        // The "pinned binary": records its environment (`env -0`-free: one `NAME=value` per line is
        // enough for the values under test) and its arguments, one per line.
        exe(
            &fake,
            &format!(
                "/usr/bin/env > '{0}.env'\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{0}.args'",
                out.display()
            ),
        );
        let envfile = d.join("harness.env");
        let state_dir = d.join("state").join(which);
        let wrapper_text = text
            .replace(bin_line, &format!("bin={}", fake.display()))
            .replace(
                "envfile=/run/aurora/harness.env",
                &format!("envfile={}", envfile.display()),
            )
            .replace(state, &state_dir.display().to_string());
        assert!(
            wrapper_text.contains(&fake.display().to_string())
                && wrapper_text.contains(&envfile.display().to_string())
                && wrapper_text.contains(&state_dir.display().to_string()),
            "the wrapper's three paths were not rewritten"
        );
        let wrapper = d.join(which);
        std::fs::write(&wrapper, wrapper_text).unwrap();
        Self {
            dir,
            wrapper,
            envfile,
            state: state_dir,
        }
    }

    fn out(&self, ext: &str) -> PathBuf {
        self.dir.path().join(format!("out.{ext}"))
    }

    fn run_with_path(&self, path: &str) -> Output {
        command("sh", [("PATH", path)])
            .arg(&self.wrapper)
            .arg("--flag")
            .output()
            .unwrap()
    }

    fn run(&self) -> Output {
        self.run_with_path("/usr/bin:/bin")
    }

    fn write_env(&self, text: &str) {
        std::fs::write(&self.envfile, text).unwrap();
    }

    /// The environment the "binary" saw (`None` when it never ran).
    fn seen_env(&self) -> Option<BTreeMap<String, String>> {
        let t = std::fs::read_to_string(self.out("env")).ok()?;
        Some(parse_env(&t))
    }

    fn seen_args(&self) -> Option<Vec<String>> {
        let t = std::fs::read_to_string(self.out("args")).ok()?;
        Some(t.lines().map(str::to_owned).collect())
    }
}

fn parse_env(t: &str) -> BTreeMap<String, String> {
    t.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

fn choice(model: &str) -> ModelChoice {
    ModelChoice {
        provider: "litellm".into(),
        endpoint_id: "e".into(),
        base_url: "http://llm.example/v1".into(),
        dialect: "anthropic_messages".into(),
        model: model.into(),
        effort: String::new(),
        client_env: vec![],
    }
}

fn models(claude: bool, model: &str) -> TerminalModels {
    let t = Some(TerminalModel {
        choice: Some(choice(model)),
        unavailable: None,
    });
    if claude {
        TerminalModels {
            claude_code: t,
            codex: None,
        }
    } else {
        TerminalModels {
            claude_code: None,
            codex: t,
        }
    }
}

/// What the supervisor writes (the real writer), for a model id with an embedded quote.
fn golden(claude: bool) -> String {
    let cfg = HarnessEnvConfig {
        path: PathBuf::from("/run/aurora/harness.env"),
        enabled: vec!["claude-code".into(), "codex".into()],
        proxy_url: "http://127.0.0.1:3128".into(),
        ca_path: "/run/aurora/ca.pem".into(),
    };
    render(&cfg, &models(claude, "m'1 \"q\" $(x) `y` \\z"))
}

fn names(file: &str) -> Vec<String> {
    file.lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .filter_map(|l| l.split_once('=').map(|(k, _)| k.to_owned()))
        .collect()
}

#[test]
fn a_golden_harness_env_exports_what_sourcing_it_did() {
    for (which, is_claude) in [("claude", true), ("codex", false)] {
        let rig = Rig::new(which);
        let file = golden(is_claude);
        rig.write_env(&file);
        let out = rig.run();
        assert!(
            out.status.success(),
            "{which}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let seen = rig.seen_env().expect("the binary ran");
        // The reference: the file sourced by a shell with `set -a`, as the old wrapper did.
        let sourced = command("sh", [("PATH", "/usr/bin:/bin")])
            .arg("-c")
            .arg("set -a; . \"$1\"; set +a; /usr/bin/env")
            .arg("sh")
            .arg(&rig.envfile)
            .output()
            .unwrap();
        assert!(sourced.status.success());
        let reference = parse_env(&String::from_utf8_lossy(&sourced.stdout));
        // What each wrapper unsets before the exec (not part of the comparison).
        let unset: &[&str] = if is_claude {
            &[
                "HARNESSES_ENABLED",
                "AURORA_CLAUDE_UNAVAILABLE",
                "AURORA_CODEX_UNAVAILABLE",
                "AURORA_CODEX_MODEL",
                "AURORA_CODEX_BASE_URL",
            ]
        } else {
            &[
                "HARNESSES_ENABLED",
                "AURORA_CODEX_MODEL",
                "AURORA_CODEX_BASE_URL",
                "AURORA_CLAUDE_UNAVAILABLE",
                "AURORA_CODEX_UNAVAILABLE",
                "ANTHROPIC_MODEL",
                "ANTHROPIC_BASE_URL",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_API_KEY",
            ]
        };
        let mut compared = 0;
        for n in names(&file) {
            if unset.contains(&n.as_str()) {
                continue;
            }
            assert_eq!(
                seen.get(&n),
                reference.get(&n),
                "{which}: {n} differs from sourcing"
            );
            compared += 1;
        }
        assert!(compared >= 8, "{which}: only {compared} variables compared");
        // The model id with every shell metacharacter survived as data.
        if which == "claude" {
            assert_eq!(
                seen.get("ANTHROPIC_MODEL").map(String::as_str),
                Some("m'1 \"q\" $(x) `y` \\z")
            );
        } else {
            let args = rig.seen_args().unwrap();
            assert!(
                args.iter().any(|a| a == "model=\"m'1 \"q\" $(x) `y` \\z\""),
                "{args:?}"
            );
        }
        // The wrapper's own state directory exists and CLAUDE_CONFIG_DIR / CODEX_HOME point at it.
        assert!(rig.state.is_dir(), "{which}: state dir not created");
        let key = if is_claude {
            "CLAUDE_CONFIG_DIR"
        } else {
            "CODEX_HOME"
        };
        assert_eq!(
            seen.get(key).map(String::as_str),
            Some(rig.state.to_str().unwrap())
        );
    }
}

const GOOD: &str =
    "# Written by aurora-supervisor.\nHARNESSES_ENABLED='claude-code codex'\nFOO='bar'\n";

#[test]
fn a_file_with_a_shell_line_makes_the_wrapper_refuse_and_the_line_never_runs() {
    for which in ["claude", "codex"] {
        for (label, extra) in [
            ("command line", "touch MARKER"),
            ("command substitution", "X=$(touch MARKER)"),
            ("assignment then command", "X='1'; touch MARKER"),
            ("backticks in an unquoted value", "X=`touch MARKER`"),
            ("redirect", "echo hi > MARKER"),
        ] {
            let rig = Rig::new(which);
            let marker = rig.dir.path().join("MARKER");
            let extra = extra.replace("MARKER", &marker.display().to_string());
            // The bad line last, so a parser that applies line by line would already have exported.
            rig.write_env(&format!("{GOOD}{extra}\n"));
            let out = rig.run();
            assert_eq!(out.status.code(), Some(1), "{which} / {label}");
            assert!(
                String::from_utf8_lossy(&out.stderr).contains(&format!("{which}: "))
                    && String::from_utf8_lossy(&out.stderr)
                        .contains("is not in the expected format"),
                "{which} / {label}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(!marker.exists(), "{which} / {label}: the line ran");
            assert!(
                rig.seen_env().is_none(),
                "{which} / {label}: the binary ran (something was applied)"
            );
        }
    }
}

#[test]
fn grammar_breaks_make_the_wrapper_refuse_to_start() {
    for which in ["claude", "codex"] {
        for (label, line) in [
            ("unquoted value", "X=abc"),
            ("double quotes", "X=\"abc\""),
            ("bytes after the closing quote", "X='a'b"),
            ("a second word", "X='a' 'b'"),
            ("unterminated", "X='a"),
            ("a bare escape", "X='a'\\'b'"),
            ("a name with a dash", "X-Y='a'"),
            ("a name starting with a digit", "1X='a'"),
            ("an empty name", "='a'"),
            ("no equals sign", "FOO"),
            ("a leading space", " X='a'"),
            ("a carriage return", "X='a'\r"),
        ] {
            let rig = Rig::new(which);
            rig.write_env(&format!("{GOOD}{line}\n"));
            let out = rig.run();
            assert_eq!(out.status.code(), Some(1), "{which} / {label}");
            assert!(
                rig.seen_env().is_none(),
                "{which} / {label}: the binary ran"
            );
        }
    }
}

#[test]
fn quoting_that_the_writer_produces_parses_exactly() {
    for which in ["claude", "codex"] {
        let rig = Rig::new(which);
        rig.write_env(&format!(
            "{GOOD}EMPTY=''\nQ='it'\\''s'\nTWO='a'\\'''\\''b'\nSPACES='  x  y  '\nGLOB='*'\n_UNDER9='ok'\n"
        ));
        let out = rig.run();
        assert!(out.status.success(), "{which}: {out:?}");
        let seen = rig.seen_env().unwrap();
        assert_eq!(seen.get("EMPTY").map(String::as_str), Some(""));
        assert_eq!(seen.get("Q").map(String::as_str), Some("it's"));
        assert_eq!(seen.get("TWO").map(String::as_str), Some("a''b"));
        assert_eq!(seen.get("SPACES").map(String::as_str), Some("  x  y  "));
        assert_eq!(seen.get("GLOB").map(String::as_str), Some("*"));
        assert_eq!(seen.get("_UNDER9").map(String::as_str), Some("ok"));
    }
}

#[test]
fn a_hostile_mkdir_on_path_does_not_run_and_the_state_dir_is_still_made() {
    for which in ["claude", "codex"] {
        let rig = Rig::new(which);
        rig.write_env(GOOD);
        let hostile = rig.dir.path().join("hostile");
        std::fs::create_dir_all(&hostile).unwrap();
        let marker = rig.dir.path().join("HOSTILE-RAN");
        exe(
            &hostile.join("mkdir"),
            &format!("touch '{}'\nexit 0", marker.display()),
        );
        let path = format!("{}:/usr/bin:/bin", hostile.display());
        let out = rig.run_with_path(&path);
        assert!(out.status.success(), "{which}: {out:?}");
        assert!(!marker.exists(), "{which}: the hostile mkdir ran");
        assert!(rig.state.is_dir(), "{which}: the state dir was not created");
    }
}

#[test]
fn the_unchanged_refusals_still_hold() {
    for (which, harness) in [("claude", "claude-code"), ("codex", "codex")] {
        // A missing file.
        let rig = Rig::new(which);
        let out = rig.run();
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&out.stderr).contains("gateway settings"));
        // The harness is not enabled on this instance.
        rig.write_env("HARNESSES_ENABLED='other'\n");
        assert_eq!(rig.run().status.code(), Some(127), "{which}");
        // An agent with no model for it: the fixed sentence, exit 1.
        let key = if which == "claude" {
            "AURORA_CLAUDE_UNAVAILABLE"
        } else {
            "AURORA_CODEX_UNAVAILABLE"
        };
        rig.write_env(&format!(
            "HARNESSES_ENABLED='{harness}'\n{key}='no model is chosen'\n"
        ));
        let out = rig.run();
        assert_eq!(out.status.code(), Some(1), "{which}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("no model is chosen"));
    }
}
