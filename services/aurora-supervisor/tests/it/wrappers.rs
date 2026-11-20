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
const PI: &str = include_str!("../../../../images/sandbox/bin/pi");
const GOOSE: &str = include_str!("../../../../images/sandbox/bin/goose");
const OPENCODE: &str = include_str!("../../../../images/sandbox/bin/opencode");

/// Every terminal wrapper (C-1015): the wrapper's file name and its harness id.
const ALL: [(&str, &str); 5] = [
    ("claude", "claude-code"),
    ("codex", "codex"),
    ("pi", "pi"),
    ("goose", "goose"),
    ("opencode", "opencode"),
];

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
            "pi" => (
                PI,
                "bin=/opt/aurora/harness/pi/node_modules/.bin/pi",
                "/workspace/.aurora/terminal/pi",
            ),
            "goose" => (
                GOOSE,
                "bin=/opt/aurora/harness/goose/goose",
                "/workspace/.aurora/terminal/goose",
            ),
            "opencode" => (
                OPENCODE,
                "bin=/opt/aurora/harness/opencode/opencode",
                "/workspace/.aurora/terminal/opencode",
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

    /// The wrapper with the person's own arguments and environment (FIX-169).
    fn run_as(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut vars = vec![("PATH", "/usr/bin:/bin")];
        vars.extend_from_slice(env);
        command("sh", vars)
            .arg(&self.wrapper)
            .args(args)
            .output()
            .unwrap()
    }

    /// A rig with `file` as harness.env, run as `run_as`, returning the "binary"'s arguments.
    fn started(
        which: &str,
        file: &str,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> (Self, Vec<String>) {
        let rig = Self::new(which);
        rig.write_env(file);
        let out = rig.run_as(args, env);
        assert!(
            out.status.success(),
            "{which} {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let seen = rig.seen_args().expect("the binary ran");
        (rig, seen)
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
        roles: None,
        subagent_role: None,
        model_meta: None,
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
            ..Default::default()
        }
    } else {
        TerminalModels {
            codex: t,
            ..Default::default()
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
    render(&cfg, &models(claude, GOLDEN_MODEL))
}

/// (SEC-122) The most punctuation a model id may carry (`MODEL_ID_PATTERN`); shell metacharacters never reach
/// the file, because the supervisor refuses such an id before writing it (see the next test).
const GOLDEN_MODEL: &str = "m.1:q/r@s-t_u";

/// A model id with shell metacharacters is refused: nothing of it is written, and no model is set for the
/// harness (`harness.env` keeps only the placeholder key).
#[test]
fn a_model_id_with_shell_metacharacters_never_reaches_harness_env() {
    let cfg = HarnessEnvConfig {
        path: PathBuf::from("/run/aurora/harness.env"),
        enabled: vec!["claude-code".into(), "codex".into()],
        proxy_url: "http://127.0.0.1:3128".into(),
        ca_path: "/run/aurora/ca.pem".into(),
    };
    for bad in ["m'1 \"q\" $(x) `y` \\z", "m$(id)", "a b", "x=y", "m\n1"] {
        let file = render(&cfg, &models(true, bad));
        assert!(!file.contains("ANTHROPIC_MODEL"), "{bad:?}: {file}");
        assert!(
            !file.contains("$(") && !file.contains("`y`"),
            "{bad:?}: {file}"
        );
    }
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
        // The model id, with the most punctuation an id may carry, survived as data.
        if which == "claude" {
            assert_eq!(
                seen.get("ANTHROPIC_MODEL").map(String::as_str),
                Some(GOLDEN_MODEL)
            );
        } else {
            let args = rig.seen_args().unwrap();
            assert!(
                args.iter()
                    .any(|a| a == &format!("model=\"{GOLDEN_MODEL}\"")),
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

const GOOD: &str = "# Written by aurora-supervisor.\nHARNESSES_ENABLED='claude-code codex pi goose opencode'\nFOO='bar'\n";

#[test]
fn a_file_with_a_shell_line_makes_the_wrapper_refuse_and_the_line_never_runs() {
    for (which, _) in ALL {
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
    for (which, _) in ALL {
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
    for (which, _) in ALL {
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
    for (which, _) in ALL {
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
    for (which, harness) in ALL {
        // A missing file.
        let rig = Rig::new(which);
        let out = rig.run();
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&out.stderr).contains("gateway settings"));
        // The harness is not enabled on this instance.
        rig.write_env("HARNESSES_ENABLED='other'\n");
        assert_eq!(rig.run().status.code(), Some(127), "{which}");
        // An agent with no model for it: the fixed sentence, exit 1.
        let key = format!("AURORA_{}_UNAVAILABLE", which.to_ascii_uppercase());
        rig.write_env(&format!(
            "HARNESSES_ENABLED='{harness}'\n{key}='no model is chosen'\n"
        ));
        let out = rig.run();
        assert_eq!(out.status.code(), Some(1), "{which}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("no model is chosen"));
    }
}

/// What the supervisor writes for the agent's model on `harness` (the real writer).
fn terminal_file(harness: &str, model: &str, dialect: &str) -> String {
    let cfg = HarnessEnvConfig {
        path: PathBuf::from("/run/aurora/harness.env"),
        enabled: ALL.iter().map(|(_, h)| (*h).to_owned()).collect(),
        proxy_url: "http://127.0.0.1:3128".into(),
        ca_path: "/run/aurora/ca.pem".into(),
    };
    let mut c = choice(model);
    c.dialect = dialect.into();
    let mut m = TerminalModels::default();
    m.set(
        harness,
        Some(TerminalModel {
            choice: Some(c),
            unavailable: None,
        }),
    );
    render(&cfg, &m)
}

/// (P5-HARN, C-1015) `pi`: the wrapper writes the rendered `models.json` into its own state directory,
/// points `PI_CODING_AGENT_DIR` there, names the placeholder through the key variable, turns the vendor
/// network off, and hands Pi the provider and the model; no `AURORA_PI_*` reaches Pi.
#[test]
fn the_pi_wrapper_configures_pi_from_the_file_and_keeps_it_off_the_vendor_network() {
    for (dialect, api, key) in [
        (
            "anthropic_messages",
            "anthropic-messages",
            "ANTHROPIC_API_KEY",
        ),
        ("openai_chat", "openai-completions", "OPENAI_API_KEY"),
    ] {
        let rig = Rig::new("pi");
        rig.write_env(&terminal_file("pi", GOLDEN_MODEL, dialect));
        let out = rig.run();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let seen = rig.seen_env().unwrap();
        assert_eq!(
            seen.get("PI_CODING_AGENT_DIR").map(String::as_str),
            Some(rig.state.to_str().unwrap())
        );
        for (k, v) in [
            ("PI_OFFLINE", "1"),
            ("PI_TELEMETRY", "0"),
            ("PI_SKIP_VERSION_CHECK", "1"),
        ] {
            assert_eq!(seen.get(k).map(String::as_str), Some(v), "{k}");
        }
        assert_eq!(
            seen.get(key).map(String::as_str),
            Some("aurora-placeholder")
        );
        assert!(
            !seen
                .keys()
                .any(|k| k.starts_with("AURORA_PI_") && k != "AURORA_PI_BIN"),
            "{seen:?}"
        );
        assert_eq!(
            rig.seen_args().unwrap(),
            ["--provider", "aurora", "--model", GOLDEN_MODEL, "--flag"]
        );
        let models: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(rig.state.join("models.json")).unwrap())
                .unwrap();
        let p = &models["providers"]["aurora"];
        assert_eq!(p["api"], api);
        assert_eq!(
            p["apiKey"],
            format!("${key}"),
            "a reference, never a literal"
        );
        assert_eq!(p["models"][0]["id"], GOLDEN_MODEL);
        assert!(!models.to_string().contains("sk-"));
    }
}

fn json_file(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// (FIX-190, D-404) `pi`'s first start writes the turn's `settings.json` (no project-trust question,
/// no install telemetry, a quiet start) and nothing secret.
#[test]
fn the_pi_wrapper_writes_the_turns_settings_on_first_start() {
    let (rig, _) = Rig::started(
        "pi",
        &terminal_file("pi", GOLDEN_MODEL, "anthropic_messages"),
        &[],
        &[],
    );
    let path = rig.state.join("settings.json");
    let want: serde_json::Value =
        serde_json::from_str(&aurora_supervisor::adapters::pi::config::terminal_settings())
            .unwrap();
    let v = json_file(&path);
    assert_eq!(v, want);
    assert_eq!(v["defaultProjectTrust"], "never");
    assert_eq!(v["enableInstallTelemetry"], false);
    assert_eq!(v["quietStartup"], true);
    assert_eq!(v["collapseChangelog"], true);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains("aurora-placeholder") && !text.contains("apiKey"),
        "{text}"
    );
    assert!(
        !rig.seen_env()
            .unwrap()
            .contains_key("AURORA_PI_SETTINGS_JSON")
    );
    let names: Vec<_> = std::fs::read_dir(&rig.state)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with('.'))
        .collect();
    assert!(names.is_empty(), "temp files left: {names:?}");
}

/// (FIX-190) An existing `settings.json` (Pi's own, or the person's) is merged, never replaced: Aurora's
/// keys are added where missing, every value there is kept, and a file that has them all, or is not
/// a JSON object, stays byte-identical.
#[test]
fn the_pi_wrapper_keeps_the_persons_settings() {
    let file = terminal_file("pi", GOLDEN_MODEL, "openai_chat");
    let turn: serde_json::Value =
        serde_json::from_str(&aurora_supervisor::adapters::pi::config::render_settings()).unwrap();
    let start = |existing: &str| {
        let rig = Rig::new("pi");
        rig.write_env(&file);
        std::fs::create_dir_all(&rig.state).unwrap();
        let path = rig.state.join("settings.json");
        std::fs::write(&path, existing).unwrap();
        let out = rig.run_as(&[], &[]);
        assert!(out.status.success(), "{out:?}");
        assert!(rig.seen_args().is_some(), "the binary ran");
        (rig, path)
    };
    // What Pi writes on its first start: its key is kept, Aurora's are added.
    let (_rig, path) = start("{\n  \"lastChangelogVersion\": \"1.5.2\"\n}\n");
    let v = json_file(&path);
    assert_eq!(v["lastChangelogVersion"], "1.5.2");
    for k in [
        "retry",
        "defaultProjectTrust",
        "enableInstallTelemetry",
        "quietStartup",
    ] {
        assert_eq!(v[k], turn[k], "{k}");
    }
    // A choice of the person's wins, also inside a nested object.
    let (_rig, path) = start(
        "{\"defaultProjectTrust\":\"ask\",\"quietStartup\":false,\"retry\":{\"maxRetries\":7}}",
    );
    let v = json_file(&path);
    assert_eq!(v["defaultProjectTrust"], "ask");
    assert_eq!(v["quietStartup"], false);
    assert_eq!(v["retry"]["maxRetries"], 7);
    assert_eq!(v["retry"]["enabled"], turn["retry"]["enabled"]);
    assert_eq!(v["enableInstallTelemetry"], false);
    // Every key present (with other values and an extra key): untouched, byte for byte.
    let full = "{ \"retry\": {\"enabled\": false, \"maxRetries\": 0, \"baseDelayMs\": 1},\n\"defaultProjectTrust\": \"always\", \"enableInstallTelemetry\": true, \"quietStartup\": false, \"collapseChangelog\": false, \"theme\": \"light\" }\n";
    let (_rig, path) = start(full);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), full);
    // Not JSON, or not an object: the person's to fix, untouched.
    for bad in ["{ not json", "[1, 2]\n"] {
        let (_rig, path) = start(bad);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bad);
    }
}

/// (FIX-190, D-404) No model for Pi: a bare `pi` says so instead of opening Pi's provider setup; with
/// arguments the binary still runs, unchanged.
#[test]
fn the_pi_wrapper_without_a_model_explains_instead_of_opening_its_setup() {
    let rig = Rig::new("pi");
    rig.write_env("HARNESSES_ENABLED='pi'\n");
    let out = rig.run_as(&[], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr).trim_end(),
        "pi: no model is set up for this agent's terminal; check the agent's model on its Behaviour page"
    );
    assert!(rig.seen_env().is_none(), "the binary ran");
    for args in [vec!["--version"], vec!["--help"]] {
        let (_, seen) = Rig::started("pi", "HARNESSES_ENABLED='pi'\n", &args, &[]);
        assert_eq!(seen, args);
    }
}

/// `goose`: configured by environment, its own root, the right provider for the dialect.
#[test]
fn the_goose_wrapper_sets_the_provider_for_the_dialect_and_its_own_root() {
    for (dialect, provider, host_var) in [
        ("anthropic_messages", "anthropic", "ANTHROPIC_HOST"),
        ("openai_chat", "litellm", "LITELLM_HOST"),
    ] {
        let rig = Rig::new("goose");
        rig.write_env(&terminal_file("goose", GOLDEN_MODEL, dialect));
        let out = rig.run();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let seen = rig.seen_env().unwrap();
        let g = |k: &str| seen.get(k).map(String::as_str);
        assert_eq!(g("GOOSE_PATH_ROOT"), Some(rig.state.to_str().unwrap()));
        assert_eq!(g("GOOSE_PROVIDER"), Some(provider));
        assert_eq!(g("GOOSE_MODEL"), Some(GOLDEN_MODEL));
        assert_eq!(g(host_var), Some("http://llm.example"));
        assert_eq!(g("GOOSE_TELEMETRY_ENABLED"), Some("false"));
        assert_eq!(g("GOOSE_DISABLE_KEYRING"), Some("1"));
        assert_eq!(g("GOOSE_DISABLE_SESSION_NAMING"), Some("true"));
        assert_eq!(g("GOOSE_CA_CERT_PATH"), Some("/run/aurora/ca.pem"));
        if provider == "litellm" {
            assert_eq!(g("LITELLM_BASE_PATH"), Some("v1/chat/completions"));
            assert_eq!(g("LITELLM_API_KEY"), Some("aurora-placeholder"));
        } else {
            assert_eq!(g("ANTHROPIC_API_KEY"), Some("aurora-placeholder"));
        }
        assert!(
            !seen
                .keys()
                .any(|k| k.starts_with("AURORA_GOOSE_") && k != "AURORA_GOOSE_BIN"),
            "{seen:?}"
        );
        assert_eq!(rig.seen_args().unwrap(), ["--flag"]);
    }
}

/// `opencode`: the rendered config inline, its own XDG directories, the vendor switches.
#[test]
fn the_opencode_wrapper_passes_the_rendered_config_inline_and_keeps_it_off_the_vendor_network() {
    for (dialect, key) in [
        ("anthropic_messages", "ANTHROPIC_API_KEY"),
        ("openai_responses", "OPENAI_API_KEY"),
    ] {
        let rig = Rig::new("opencode");
        rig.write_env(&terminal_file("opencode", GOLDEN_MODEL, dialect));
        let out = rig.run();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let seen = rig.seen_env().unwrap();
        let g = |k: &str| seen.get(k).map(String::as_str);
        let state = rig.state.to_str().unwrap();
        assert_eq!(g("XDG_DATA_HOME"), Some(format!("{state}/data").as_str()));
        assert_eq!(
            g("XDG_CONFIG_HOME"),
            Some(format!("{state}/config").as_str())
        );
        for k in [
            "OPENCODE_DISABLE_MODELS_FETCH",
            "OPENCODE_DISABLE_DEFAULT_PLUGINS",
            "OPENCODE_DISABLE_LSP_DOWNLOAD",
            "OPENCODE_DISABLE_AUTOUPDATE",
        ] {
            assert_eq!(g(k), Some("1"), "{k}");
        }
        assert_eq!(g(key), Some("aurora-placeholder"));
        let cfg: serde_json::Value =
            serde_json::from_str(g("OPENCODE_CONFIG_CONTENT").expect("inline config")).unwrap();
        assert_eq!(cfg["model"], format!("aurora/{GOLDEN_MODEL}"));
        assert_eq!(cfg["autoupdate"], false);
        assert_eq!(
            cfg["provider"]["aurora"]["options"]["apiKey"],
            format!("{{env:{key}}}")
        );
        assert!(
            cfg.get("mcp").is_none(),
            "no platform tools in the terminal"
        );
        assert!(
            !seen
                .keys()
                .any(|k| k.starts_with("AURORA_OPENCODE_") && k != "AURORA_OPENCODE_BIN"),
            "{seen:?}"
        );
    }
}

/// A dialect the harness cannot speak (goose has no Responses path) or an id that fails the re-check
/// becomes the fixed refusal, never a half-configured CLI.
#[test]
fn a_model_a_harness_cannot_use_is_a_refusal_and_not_a_configuration() {
    for (which, dialect, model) in [
        ("goose", "openai_responses", "m"),
        ("pi", "anthropic_bedrock", "m"),
        ("opencode", "anthropic_vertex", "m"),
        ("pi", "anthropic_messages", "m$(id)"),
    ] {
        let rig = Rig::new(which);
        let file = terminal_file(which, model, dialect);
        assert!(file.contains("_UNAVAILABLE='"), "{which}: {file}");
        assert!(!file.contains("$(id)"), "{file}");
        rig.write_env(&file);
        let out = rig.run();
        assert_eq!(out.status.code(), Some(1), "{which}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("cannot be used with"));
        assert!(rig.seen_env().is_none(), "{which}: the CLI ran");
    }
}

/// (FIX-169, D-371, C-1025) `claude` starts in bypass mode like a turn, unless the person chose a mode.
#[test]
fn the_claude_wrapper_defaults_to_bypass_mode_and_keeps_the_persons_choice() {
    let file = golden(true);
    let bypass = ["--permission-mode", "bypassPermissions"];
    // No choice: bypass first, then exactly the person's arguments (prompt, -p, -c, --resume, a
    // subcommand, --version, --help, and a prompt after `--`).
    for args in [
        vec![],
        vec!["write a file"],
        vec!["-p", "hi"],
        vec!["-c"],
        vec!["--resume", "abc"],
        vec!["mcp", "list"],
        vec!["doctor"],
        vec!["--version"],
        vec!["--help"],
        vec!["--", "--permission-mode"],
    ] {
        let (_, seen) = Rig::started("claude", &file, &args, &[]);
        let mut want: Vec<String> = bypass.iter().map(|s| (*s).to_owned()).collect();
        want.extend(args.iter().map(|s| (*s).to_owned()));
        assert_eq!(seen, want, "{args:?}");
    }
    // An explicit choice passes through unchanged: no second mode, no default.
    for args in [
        vec!["--permission-mode", "default"],
        vec!["--permission-mode=plan", "-p", "x"],
        vec!["-p", "x", "--permission-mode", "acceptEdits"],
        vec!["--dangerously-skip-permissions"],
        vec!["--allow-dangerously-skip-permissions", "-c"],
    ] {
        let (_, seen) = Rig::started("claude", &file, &args, &[]);
        assert_eq!(seen, args, "{args:?}");
    }
}

/// `codex`: the turn's approval and sandbox settings in both branches (with and without a gateway
/// endpoint); the person's dedicated flags and later `-c` keep what they chose.
#[test]
fn the_codex_wrapper_defaults_to_no_approvals_and_no_own_sandbox() {
    let ask = "approval_policy=\"never\"";
    let boxed = "sandbox_mode=\"danger-full-access\"";
    let with_base = golden(false);
    let without_base = "HARNESSES_ENABLED='codex'\n";
    for (label, file) in [
        ("with a gateway endpoint", with_base.as_str()),
        ("without", without_base),
    ] {
        let (_, seen) = Rig::started("codex", file, &["exec", "make a file"], &[]);
        let has = |v: &str| seen.windows(2).any(|w| w[0] == "-c" && w[1] == v);
        assert!(has(ask) && has(boxed), "{label}: {seen:?}");
        assert!(
            seen.ends_with(&["exec".into(), "make a file".into()]),
            "{label}: {seen:?}"
        );
        // Our defaults come before the person's arguments, so a later `-c` of theirs wins.
        let (_, seen) = Rig::started("codex", file, &["-c", "approval_policy=\"untrusted\""], &[]);
        let ours = seen.iter().position(|a| a == ask).expect("default present");
        let theirs = seen
            .iter()
            .position(|a| a == "approval_policy=\"untrusted\"")
            .unwrap();
        assert!(ours < theirs, "{label}: {seen:?}");
    }
    // A dedicated flag: the default it covers is not added.
    let count = |seen: &[String], v: &str| seen.iter().filter(|a| a.as_str() == v).count();
    for (args, want_ask, want_box) in [
        (vec!["-a", "on-request"], 0, 1),
        (vec!["--ask-for-approval=untrusted"], 0, 1),
        (vec!["-s", "read-only"], 1, 0),
        (vec!["--sandbox=workspace-write"], 1, 0),
        (vec!["--full-auto"], 0, 0),
        (vec!["--dangerously-bypass-approvals-and-sandbox"], 0, 0),
        (vec!["-a", "never", "-s", "read-only"], 0, 0),
    ] {
        let (_, seen) = Rig::started("codex", without_base, &args, &[]);
        assert_eq!(count(&seen, ask), want_ask, "{args:?}: {seen:?}");
        assert_eq!(count(&seen, boxed), want_box, "{args:?}: {seen:?}");
        assert!(
            seen.ends_with(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()),
            "{args:?}: {seen:?}"
        );
    }
}

/// `goose`: `GOOSE_MODE=auto` like a turn; a mode in the environment the wrapper received is kept, and
/// harness.env never supplies one.
#[test]
fn the_goose_wrapper_defaults_to_auto_mode_and_keeps_the_persons_choice() {
    let file = terminal_file("goose", GOLDEN_MODEL, "anthropic_messages");
    let mode = |rig: &Rig| rig.seen_env().unwrap().get("GOOSE_MODE").cloned();
    let (rig, _) = Rig::started("goose", &file, &[], &[]);
    assert_eq!(mode(&rig).as_deref(), Some("auto"));
    let (rig, _) = Rig::started("goose", &file, &[], &[("GOOSE_MODE", "approve")]);
    assert_eq!(mode(&rig).as_deref(), Some("approve"));
    // The file has no such key and must not be able to set one: the default still applies.
    let (rig, _) = Rig::started("goose", &format!("{file}GOOSE_MODE='chat'\n"), &[], &[]);
    assert_eq!(mode(&rig).as_deref(), Some("auto"));
    // A mode saved with `goose configure` is not overridden by an environment default.
    let rig = Rig::new("goose");
    rig.write_env(&file);
    let cfg = rig.state.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("config.yaml"),
        "GOOSE_PROVIDER: x\nGOOSE_MODE: smart_approve\n",
    )
    .unwrap();
    assert!(rig.run_as(&[], &[]).status.success());
    assert_eq!(mode(&rig), None);
}

/// The goose config file the wrapper manages (FIX-189).
fn goose_config(rig: &Rig) -> PathBuf {
    rig.state.join("config").join("config.yaml")
}

fn yaml(path: &Path) -> BTreeMap<String, serde_yaml::Value> {
    serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// (FIX-189, D-404) goose opens its wizard when its config file is missing: the first start writes a
/// minimal one (non-secret keys only, mode 0600), and nothing else.
#[test]
fn the_goose_wrapper_writes_a_minimal_config_on_first_start() {
    for (dialect, provider, hosts) in [
        (
            "anthropic_messages",
            "anthropic",
            &[("ANTHROPIC_HOST", "http://llm.example")][..],
        ),
        (
            "openai_chat",
            "litellm",
            &[
                ("LITELLM_HOST", "http://llm.example"),
                ("LITELLM_BASE_PATH", "v1/chat/completions"),
            ][..],
        ),
    ] {
        let (rig, args) = Rig::started(
            "goose",
            &terminal_file("goose", GOLDEN_MODEL, dialect),
            &[],
            &[],
        );
        assert!(args.is_empty(), "{args:?}");
        let path = goose_config(&rig);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{dialect}");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("API_KEY") && !text.contains("aurora-placeholder"),
            "{text}"
        );
        assert!(!text.contains("GOOSE_MODE:"), "{text}");
        let map = yaml(&path);
        let mut want: BTreeMap<String, serde_yaml::Value> = BTreeMap::new();
        want.insert("GOOSE_PROVIDER".into(), provider.into());
        want.insert("GOOSE_MODEL".into(), GOLDEN_MODEL.into());
        want.insert("GOOSE_TELEMETRY_ENABLED".into(), false.into());
        for (k, v) in hosts {
            want.insert((*k).into(), (*v).into());
        }
        assert_eq!(map, want, "{dialect}");
        // No temp file left beside it.
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["config.yaml"], "{dialect}");
        // The default mode is still `auto` (FIX-169): the written file has no mode.
        let (rig2, _) = Rig::started(
            "goose",
            &terminal_file("goose", GOLDEN_MODEL, dialect),
            &[],
            &[],
        );
        assert_eq!(
            rig2.seen_env()
                .unwrap()
                .get("GOOSE_MODE")
                .map(String::as_str),
            Some("auto")
        );
    }
}

/// (FIX-189) A file that exists is the person's: a later start leaves it byte-identical.
#[test]
fn the_goose_wrapper_leaves_an_existing_config_alone() {
    let file = terminal_file("goose", GOLDEN_MODEL, "openai_chat");
    let (rig, _) = Rig::started("goose", &file, &[], &[]);
    let path = goose_config(&rig);
    let edited = "GOOSE_PROVIDER: openai\nGOOSE_MODEL: mine\nextensions: {}\n";
    std::fs::write(&path, edited).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let out = rig.run_as(&[], &[]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o644
    );
    // The environment still names the agent's provider and model (goose reads it first).
    let seen = rig.seen_env().unwrap();
    assert_eq!(
        seen.get("GOOSE_PROVIDER").map(String::as_str),
        Some("litellm")
    );
    assert_eq!(
        seen.get("GOOSE_MODEL").map(String::as_str),
        Some(GOLDEN_MODEL)
    );
}

/// (FIX-189) Every value is written as a YAML string: YAML's special characters stay data.
#[test]
fn the_goose_config_quotes_yaml_special_characters() {
    let model = "a: b # c 'd' \"e\" {f} [g] &h *i !j %k @l `m` |n >o ~ null";
    let host = "http://h.example:8080/#x'y";
    let rig = Rig::new("goose");
    let q = |v: &str| v.replace('\'', "'\\''");
    rig.write_env(&format!(
        "HARNESSES_ENABLED='goose'\nAURORA_GOOSE_MODEL='{}'\nAURORA_GOOSE_PROVIDER='litellm'\nAURORA_GOOSE_HOST='{}'\n",
        q(model),
        q(host)
    ));
    let out = rig.run_as(&[], &[]);
    assert!(out.status.success(), "{out:?}");
    let map = yaml(&goose_config(&rig));
    assert_eq!(map["GOOSE_MODEL"], serde_yaml::Value::from(model));
    assert_eq!(map["LITELLM_HOST"], serde_yaml::Value::from(host));
    assert_eq!(map.len(), 5, "{map:?}");
    // A one-word YAML keyword (`null`, `true`) is still a string.
    for (word, name) in [("null", "n1"), ("true", "n2"), ("1.5", "n3")] {
        let rig = Rig::new("goose");
        rig.write_env(&format!(
            "HARNESSES_ENABLED='goose'\nAURORA_GOOSE_MODEL='{word}'\nAURORA_GOOSE_PROVIDER='anthropic'\nAURORA_GOOSE_HOST='http://h'\n"
        ));
        assert!(rig.run_as(&[], &[]).status.success(), "{name}");
        assert_eq!(
            yaml(&goose_config(&rig))["GOOSE_MODEL"],
            serde_yaml::Value::from(word),
            "{name}"
        );
    }
}

/// (FIX-189, D-404) No model for goose: a bare `goose` says so instead of opening the wizard; with
/// arguments (`configure`, `--version`) the binary still runs, unchanged.
#[test]
fn the_goose_wrapper_without_a_model_explains_instead_of_opening_the_wizard() {
    let rig = Rig::new("goose");
    rig.write_env("HARNESSES_ENABLED='goose'\n");
    let out = rig.run_as(&[], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr).trim_end(),
        "goose: no model is set up for this agent's terminal; check the agent's model on its Behaviour page"
    );
    assert!(rig.seen_env().is_none(), "the binary ran");
    assert!(!goose_config(&rig).exists());
    for args in [vec!["configure"], vec!["--version"]] {
        let (rig, seen) = Rig::started("goose", "HARNESSES_ENABLED='goose'\n", &args, &[]);
        assert_eq!(seen, args);
        assert!(!goose_config(&rig).exists(), "{args:?}");
    }
}

/// (FIX-191) `opencode` writes the turn's plugin files (so its start never asks the npm registry), each
/// only when missing.
#[test]
fn the_opencode_wrapper_writes_the_turns_plugin_files_when_missing() {
    let file = terminal_file("opencode", GOLDEN_MODEL, "anthropic_messages");
    assert!(file.contains("AURORA_OPENCODE_PLUGIN_VERSION='"), "{file}");
    let (rig, _) = Rig::started("opencode", &file, &[], &[]);
    for (rel, content) in aurora_supervisor::adapters::opencode::config::plugin_files() {
        // The turn's paths are relative to its root, whose `config` is XDG_CONFIG_HOME.
        let path = rig.state.join(rel);
        let want: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(json_file(&path), want, "{rel}");
    }
    assert!(
        !rig.seen_env()
            .unwrap()
            .keys()
            .any(|k| k.starts_with("AURORA_OPENCODE_")),
    );
    // A package.json the person (or OpenCode, for their plugins) changed is kept.
    let pkg = rig.state.join("config/opencode/package.json");
    let theirs =
        "{\"dependencies\":{\"@opencode-ai/plugin\":\"1.19.6\",\"my-plugin\":\"1.0.0\"}}\n";
    std::fs::write(&pkg, theirs).unwrap();
    assert!(rig.run_as(&[], &[]).status.success());
    assert_eq!(std::fs::read_to_string(&pkg).unwrap(), theirs);
    // A version that is not a plain version string writes nothing.
    let rig = Rig::new("opencode");
    rig.write_env(&file.replace(
        "AURORA_OPENCODE_PLUGIN_VERSION='1.19.6'",
        "AURORA_OPENCODE_PLUGIN_VERSION='1\"}x'",
    ));
    assert!(rig.run_as(&[], &[]).status.success());
    assert!(!rig.state.join("config/opencode/package.json").exists());
}

/// (FIX-191) OpenCode's errors go to its log, behind its full-screen interface: after a non-zero exit
/// the wrapper says where that log is, and keeps the status; a clean exit says nothing.
#[test]
fn the_opencode_wrapper_names_the_log_after_a_failed_start() {
    let file = terminal_file("opencode", GOLDEN_MODEL, "anthropic_messages");
    for (code, says) in [(0, false), (3, true)] {
        let rig = Rig::new("opencode");
        rig.write_env(&file);
        exe(
            &rig.dir.path().join("fake-bin"),
            &format!("/usr/bin/mkdir -p \"$XDG_DATA_HOME/opencode/log\"\nexit {code}"),
        );
        let out = rig.run_as(&[], &[]);
        assert_eq!(out.status.code(), Some(code));
        let err = String::from_utf8_lossy(&out.stderr);
        let log = rig.state.join("data/opencode/log");
        assert_eq!(
            err.contains(&format!("see {} for details", log.display())),
            says,
            "{code}: {err}"
        );
    }
}

/// `opencode` already behaves like a turn: its terminal config allows every tool.
#[test]
fn the_opencode_terminal_config_allows_every_tool() {
    let rig = Rig::new("opencode");
    rig.write_env(&terminal_file(
        "opencode",
        GOLDEN_MODEL,
        "anthropic_messages",
    ));
    assert!(rig.run().status.success());
    let seen = rig.seen_env().unwrap();
    let cfg: serde_json::Value =
        serde_json::from_str(seen.get("OPENCODE_CONFIG_CONTENT").expect("inline config")).unwrap();
    assert_eq!(cfg["permission"]["*"], "allow");
}
