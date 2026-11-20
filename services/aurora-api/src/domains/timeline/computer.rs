//! (P5-REFINE-9, C-1083; UX review 5 theme "Nothing shows what the agent did on its computer")
//! Which harness tools ran a command or wrote a file on the agent's computer, and what their
//! durable `tool.started` summary says.
//!
//! The table is per adapter (`services/aurora-supervisor/src/adapters/*`, `docs/12`). A tool that
//! is not in it is never classified (no guess): read-only tools (`Read`, `Glob`, `Grep`, `read`,
//! `list`, ...), platform tools (`mcp__aurora__*`) and unknown names give no `computer` row. The
//! summary is the adapter's `<name>: <first argument>` (≤ 200 characters, never the raw
//! arguments); a summary that does not have that shape gives a step without command or path.

/// What a step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// Ran a command.
    Shell,
    /// Wrote or edited files.
    Write,
}

/// The adapters' shell and file-writing tools.
const TABLE: &[(&str, &[&str], &[&str])] = &[
    // claude-code: `summarize` keys command, file_path (NotebookEdit's notebook_path is not one:
    // its summary is "NotebookEdit (N args)", a step without a path).
    (
        "claude-code",
        &["Bash"],
        &["Write", "Edit", "MultiEdit", "NotebookEdit"],
    ),
    // pi: `tool_summary` keys command, path, file_path.
    ("pi", &["bash"], &["write", "edit"]),
    // codex: `shell: <command>`, `apply_patch: <path>, <path>`.
    ("codex", &["shell"], &["apply_patch"]),
    // goose 1.59.2 developer extension: `shell`, `write`, `edit` (summary keys command, path).
    ("goose", &["shell"], &["write", "edit"]),
    // opencode 1.19.x built-ins: `bash`, `write`, `edit` (filePath), `patch` (no path).
    ("opencode", &["bash"], &["write", "edit", "patch"]),
];

/// The step kind of `tool` under `harness`; `None` for anything not in the table.
pub fn classify(harness: &str, tool: &str) -> Option<StepKind> {
    let (_, shell, write) = TABLE.iter().find(|(h, _, _)| *h == harness)?;
    if shell.contains(&tool) {
        Some(StepKind::Shell)
    } else if write.contains(&tool) {
        Some(StepKind::Write)
    } else {
        None
    }
}

/// The longest command shown (C-1083).
pub const COMMAND_MAX: usize = 120;
/// The most paths a row lists (C-1083).
pub const FILES_MAX: usize = 5;
/// The agent's workspace in its sandbox (`docs/12 §Home projection`, C-899).
pub const WORKSPACE: &str = "/workspace";

/// One parsed step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub kind: StepKind,
    /// `Shell`: the first line of the command (redacted, ≤ [`COMMAND_MAX`]).
    pub command: Option<String>,
    /// `Write`: the paths written, as the summary names them (redacted).
    pub paths: Vec<String>,
}

/// Parse a `tool.started` of `harness`: `None` when the tool is not a computer step.
pub fn parse(harness: &str, tool: &str, summary: Option<&str>) -> Option<Step> {
    let kind = classify(harness, tool)?;
    let rest = summary.and_then(|s| s.strip_prefix(tool)?.strip_prefix(": "));
    let mut step = Step {
        kind,
        command: None,
        paths: Vec::new(),
    };
    let Some(rest) = rest else {
        return Some(step);
    };
    match kind {
        StepKind::Shell => step.command = command(rest),
        StepKind::Write => {
            let parts: Vec<&str> = if harness == "codex" {
                rest.split(", ").collect()
            } else {
                vec![rest]
            };
            step.paths = parts.into_iter().filter_map(path).collect();
        }
    }
    Some(step)
}

/// The first line, redacted, at most [`COMMAND_MAX`] characters; `None` when empty.
pub fn command(rest: &str) -> Option<String> {
    let line = rest.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return None;
    }
    let red = redact_words(line);
    Some(clip(&red, COMMAND_MAX))
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// A path as a summary names it: one line, no control characters, credential-looking segments
/// replaced. `None` when nothing usable is left.
fn path(raw: &str) -> Option<String> {
    let p = raw.lines().next().unwrap_or("").trim();
    if p.is_empty() || p.chars().any(char::is_control) {
        return None;
    }
    let red: Vec<String> = p.split('/').map(redact_segment).collect();
    Some(clip(&red.join("/"), 300))
}

/// The path relative to the workspace for display (`/workspace/notes.txt` → `notes.txt`; a
/// relative path stays as it is; anything else is shown whole).
pub fn display_path(p: &str) -> String {
    p.strip_prefix(WORKSPACE)
        .and_then(|r| r.strip_prefix('/'))
        .filter(|r| !r.is_empty())
        .map_or_else(|| p.to_owned(), str::to_owned)
}

/// The absolute path the Files page opens, when the file is inside the workspace (a harness
/// runs in `/workspace`, so a relative path is under it); `None` otherwise.
pub fn files_page_path(p: &str) -> Option<String> {
    if p.contains("[redacted]") || p.split('/').any(|c| c == ".." || c == ".") {
        return None;
    }
    let abs = if p.starts_with('/') {
        p.to_owned()
    } else {
        format!("{WORKSPACE}/{}", p.trim_start_matches("./"))
    };
    abs.strip_prefix(WORKSPACE)
        .is_some_and(|r| r.starts_with('/') && r.len() > 1)
        .then_some(abs)
}

/// A word or path segment that looks like a credential (the gateway's `redact_path` rule plus
/// `key=value` assignments of secret-looking names) becomes `[redacted]`.
fn secretish(seg: &str) -> bool {
    let core = seg.trim_matches(|c: char| "\"'`()[]{}<>,;".contains(c));
    let long_token = core.len() >= 24
        && core.chars().any(|c| c.is_ascii_digit())
        && core
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_:.=+/".contains(c));
    let prefixed = [
        "aug_", "aub_", "auc_", "sk-", "sk_", "ghp_", "gho_", "xox", "AKIA",
    ]
    .iter()
    .any(|p| core.starts_with(p) && core.len() > p.len() + 4);
    let assignment = core.split_once('=').is_some_and(|(k, v)| {
        !v.is_empty() && {
            let k = k.to_ascii_lowercase();
            [
                "token", "secret", "password", "passwd", "apikey", "api_key", "key",
            ]
            .iter()
            .any(|s| k.ends_with(s))
        }
    });
    // A URL with user information (`scheme://user:token@host`).
    let url_userinfo = core.split_once("://").is_some_and(|(_, rest)| {
        let authority = rest.split('/').next().unwrap_or("");
        authority.contains('@')
    });
    long_token || prefixed || assignment || url_userinfo
}

fn redact_segment(seg: &str) -> String {
    if secretish(seg) {
        "[redacted]".to_owned()
    } else {
        seg.to_owned()
    }
}

/// Redact credential-looking words; a word after `Bearer`/`Basic` or an `-u`/`--user`/
/// `--password` style flag is redacted too.
fn redact_words(line: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut next_secret = false;
    for w in line.split(' ') {
        if w.is_empty() {
            out.push(String::new());
            continue;
        }
        let lw = w.to_ascii_lowercase();
        let trimmed = lw.trim_matches(|c: char| "\"'".contains(c));
        // `Authorization: Bearer <token>`: a keyword after a keyword keeps the chain going.
        let introduces = matches!(
            trimmed,
            "bearer" | "basic" | "-u" | "--user" | "--password" | "--token" | "authorization:"
        );
        out.push(if next_secret {
            "[redacted]".to_owned()
        } else {
            redact_segment(w)
        });
        next_secret = introduces;
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One fixture per adapter: the summaries its `ToolStarted` writes (see the adapter's
    /// `summarize`/`tool_summary`/`summary`).
    #[test]
    fn claude_code() {
        let s = parse("claude-code", "Bash", Some("Bash: ls -la\nsecond line")).unwrap();
        assert_eq!(s.kind, StepKind::Shell);
        assert_eq!(s.command.as_deref(), Some("ls -la"));
        let w = parse("claude-code", "Write", Some("Write: notes.txt")).unwrap();
        assert_eq!(
            (w.kind, w.paths.clone()),
            (StepKind::Write, vec!["notes.txt".to_owned()])
        );
        for t in ["Edit", "MultiEdit"] {
            let s = parse("claude-code", t, Some(&format!("{t}: /workspace/a.rs"))).unwrap();
            assert_eq!(s.paths, vec!["/workspace/a.rs"]);
        }
        let nb = parse("claude-code", "NotebookEdit", Some("NotebookEdit (3 args)")).unwrap();
        assert!(
            nb.paths.is_empty(),
            "no path in the summary: a step without files"
        );
        for t in [
            "Read",
            "Glob",
            "Grep",
            "WebFetch",
            "mcp__aurora__memory_search",
            "Task",
        ] {
            assert!(
                parse("claude-code", t, Some(&format!("{t}: x"))).is_none(),
                "{t}"
            );
        }
    }

    #[test]
    fn pi() {
        assert_eq!(
            parse("pi", "bash", Some("bash: echo hi"))
                .unwrap()
                .command
                .as_deref(),
            Some("echo hi")
        );
        assert_eq!(
            parse("pi", "write", Some("write: out.md")).unwrap().paths,
            ["out.md"]
        );
        assert_eq!(
            parse("pi", "edit", Some("edit: src/x.ts")).unwrap().paths,
            ["src/x.ts"]
        );
        assert!(parse("pi", "read", Some("read: notes.txt")).is_none());
    }

    #[test]
    fn codex() {
        assert_eq!(
            parse("codex", "shell", Some("shell: cargo test"))
                .unwrap()
                .command
                .as_deref(),
            Some("cargo test")
        );
        assert_eq!(
            parse("codex", "apply_patch", Some("apply_patch: a.rs, b/c.rs"))
                .unwrap()
                .paths,
            ["a.rs", "b/c.rs"]
        );
        assert!(parse("codex", "web_search", Some("web_search (1 args)")).is_none());
    }

    #[test]
    fn goose() {
        assert_eq!(
            parse("goose", "shell", Some("shell: echo hi"))
                .unwrap()
                .command
                .as_deref(),
            Some("echo hi")
        );
        assert_eq!(
            parse("goose", "write", Some("write: /workspace/r.md"))
                .unwrap()
                .paths,
            ["/workspace/r.md"]
        );
        assert!(parse("goose", "tree", Some("tree: /workspace")).is_none());
        assert!(parse("goose", "analyze", Some("analyze: x")).is_none());
    }

    #[test]
    fn opencode() {
        assert_eq!(
            parse("opencode", "bash", Some("bash: echo hi"))
                .unwrap()
                .command
                .as_deref(),
            Some("echo hi")
        );
        assert_eq!(
            parse("opencode", "write", Some("write: /workspace/n.txt"))
                .unwrap()
                .paths,
            ["/workspace/n.txt"]
        );
        assert!(
            parse("opencode", "patch", Some("patch (1 args)"))
                .unwrap()
                .paths
                .is_empty()
        );
        for t in ["read", "glob", "grep", "list", "webfetch", "todowrite"] {
            assert!(
                parse("opencode", t, Some(&format!("{t}: x"))).is_none(),
                "{t}"
            );
        }
    }

    #[test]
    fn unknown_names_and_harnesses_are_never_guessed() {
        assert!(
            parse("claude-code", "bash", Some("bash: ls")).is_none(),
            "case matters"
        );
        assert!(parse("claude-code", "Shell", Some("Shell: ls")).is_none());
        assert!(parse("some-new-harness", "Bash", Some("Bash: ls")).is_none());
        assert!(classify("pi", "mcp__aurora__notify").is_none());
    }

    #[test]
    fn an_unparsable_summary_still_gives_a_step() {
        let s = parse("claude-code", "Bash", Some("Bash (2 args)")).unwrap();
        assert_eq!((s.command, s.paths.len()), (None, 0));
        let s = parse("claude-code", "Bash", None).unwrap();
        assert!(s.command.is_none());
        let s = parse("claude-code", "Bash", Some("Bash: ")).unwrap();
        assert!(s.command.is_none());
    }

    #[test]
    fn commands_are_one_line_clipped_and_redacted() {
        let long = format!("Bash: echo {}", "x".repeat(300));
        let c = parse("claude-code", "Bash", Some(&long))
            .unwrap()
            .command
            .unwrap();
        assert_eq!(c.chars().count(), COMMAND_MAX);
        let c = command("curl -H 'Authorization: Bearer abc.def' https://x.example/a").unwrap();
        assert!(!c.contains("abc.def"), "{c}");
        let c = command("export API_TOKEN=s3cr3tvalue && run").unwrap();
        assert!(!c.contains("s3cr3tvalue"), "{c}");
        let c = command("git push https://ghp_abcdefghijklmnopqrstuvwxyz0123@git.example.com/x")
            .unwrap();
        assert!(!c.contains("ghp_abcdef"), "{c}");
        let c = command("echo sk-proj-1234567890abcdefghij").unwrap();
        assert!(c.contains("[redacted]"), "{c}");
        assert_eq!(command("ls -la").unwrap(), "ls -la");
    }

    #[test]
    fn paths_are_shown_relative_and_linked_only_inside_the_workspace() {
        assert_eq!(display_path("/workspace/notes.txt"), "notes.txt");
        assert_eq!(display_path("notes.txt"), "notes.txt");
        assert_eq!(display_path("/tmp/x"), "/tmp/x");
        assert_eq!(
            files_page_path("notes.txt").as_deref(),
            Some("/workspace/notes.txt")
        );
        assert_eq!(
            files_page_path("/workspace/a/b.md").as_deref(),
            Some("/workspace/a/b.md")
        );
        assert_eq!(files_page_path("/tmp/x"), None);
        assert_eq!(files_page_path("../etc/passwd"), None);
        assert_eq!(files_page_path("/workspace"), None);
        let w = parse(
            "pi",
            "write",
            Some("write: /workspace/aug_0123456789abcdefghijklmnop/k"),
        )
        .unwrap();
        assert_eq!(w.paths, ["/workspace/[redacted]/k"]);
        assert_eq!(files_page_path(&w.paths[0]), None);
    }
}
