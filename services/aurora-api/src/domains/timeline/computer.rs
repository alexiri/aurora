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
        "aug_", "aub_", "auc_", "sk-", "sk_", "ghp_", "gho_", "ghs_", "ghu_", "ghr_", "glpat-",
        "xox", "AKIA", "AIza", "npm_", "pypi-", "eyJ",
    ]
    .iter()
    .any(|p| core.starts_with(p) && core.len() > p.len() + 4);
    // `NAME=value` (and `--flag=value`): a name that ends like a credential's. The value may be
    // a quoted multi-word string (`TOKEN="a b"`: the tokenizer keeps it in one word).
    let assignment = core.split_once('=').is_some_and(|(k, v)| {
        !v.is_empty() && {
            let k = k.trim_start_matches('-').to_ascii_lowercase();
            SECRET_NAME_ENDS.iter().any(|s| k.ends_with(s))
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

/// The end of a variable or flag name that holds a credential (`API_TOKEN`, `DB_PASS`, `GH_PAT`,
/// `--client-secret`, `SSH_KEY`, ...). Compared in lower case.
const SECRET_NAME_ENDS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "pass",
    "pw",
    "pwd",
    "pat",
    "apikey",
    "api_key",
    "api-key",
    "key",
    "credential",
    "credentials",
    "auth",
];

/// Flags whose next word is a credential (or carries one): users with passwords, tokens,
/// cookies, request bodies.
const SECRET_FLAGS: &[&str] = &[
    "-u",
    "--user",
    "--proxy-user",
    "--password",
    "--passwd",
    "--pass",
    "--token",
    "--secret",
    "--api-key",
    "--apikey",
    "--auth",
    "--bearer",
    "--oauth2-bearer",
    "--cookie",
    "--data",
    "--data-raw",
    "--data-binary",
    "--data-urlencode",
    "--data-ascii",
    "--json",
    "--form",
    "--form-string",
];

/// Short flags that are a cookie or a request body for an HTTP client (`curl -b`, `curl -d`,
/// `curl -F`) and mean something else elsewhere (`docker run -d`, `rm -f`): redacted only after
/// one of these commands.
const HTTP_CLIENTS: &[&str] = &["curl", "wget", "http", "https", "xh", "httpie", "hurl"];
const HTTP_SHORT_FLAGS: &[&str] = &["-b", "-d", "-F"];

/// Commands whose `-p` glues a password to the flag (`mysql -pSECRET`).
const PASSWORD_CLIENTS: &[&str] = &[
    "mysql",
    "mysqldump",
    "mysqladmin",
    "mariadb",
    "mongosh",
    "mongo",
];

/// HTTP header names (lower case, with the colon) whose value is a credential.
const SECRET_HEADERS: &[&str] = &[
    "authorization:",
    "proxy-authorization:",
    "cookie:",
    "set-cookie:",
    "x-api-key:",
    "api-key:",
    "x-auth-token:",
    "x-access-token:",
    "x-amz-security-token:",
    "private-token:",
];

/// The words of a command line, each kept as written (quotes included): whitespace splits words
/// except inside single or double quotes, so `TOKEN="a b"` and `-H "Authorization: Token x"` are
/// one word each. An unterminated quote runs to the end of the line.
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' {
                    cur.extend(chars.next());
                }
            }
            None if c == '\'' || c == '"' => {
                cur.push(c);
                quote = Some(c);
            }
            None if c == '\\' => {
                cur.push(c);
                cur.extend(chars.next());
            }
            None if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            None => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// What the next word is, after a word that introduces a credential.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Next {
    Normal,
    /// The next word is the credential.
    Secret,
    /// Every word up to the next flag is (an unquoted `Authorization: Bearer <token>`).
    UntilFlag,
}

const REDACTED: &str = "[redacted]";

/// Redact credential-looking words (SEC-143: a command shown on the timeline never carries a
/// credential): the value of `NAME=value` with a secret-looking name (quoted values with spaces
/// too), the words after `-u`/`--user`/`--password`/`-b`/`--cookie`/`-d`/`--data*` style flags,
/// after `Bearer`/`Basic`, `sshpass -p`, the value of `Authorization:`/`Cookie:`-style headers,
/// `-pSECRET` (a password glued to the flag), URLs with user information and long tokens.
fn redact_words(line: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut next = Next::Normal;
    // What the current command (up to a `|`, `&&`, `;`) is, for the flags that depend on it.
    let (mut sshpass, mut http, mut pwd_client) = (false, false, false);
    for w in words(line) {
        let unquoted = w.trim_matches(|c: char| "\"'".contains(c));
        let lower = unquoted.to_ascii_lowercase();
        match next {
            Next::Secret => {
                out.push(REDACTED.to_owned());
                next = Next::Normal;
                continue;
            }
            Next::UntilFlag if !(lower.starts_with('-') && lower.len() > 1) => {
                out.push(REDACTED.to_owned());
                continue;
            }
            _ => next = Next::Normal,
        }
        // A new command in the line starts afresh.
        if matches!(lower.as_str(), "|" | "||" | "&&" | ";" | "&") {
            (sshpass, http, pwd_client) = (false, false, false);
            out.push(w);
            continue;
        }
        sshpass |= lower == "sshpass";
        http |= HTTP_CLIENTS.contains(&lower.as_str());
        pwd_client |= PASSWORD_CLIENTS.contains(&lower.as_str());
        // `Authorization: Bearer x`, `Cookie: a=b` (quoted as one word, or split on the colon).
        if let Some(h) = SECRET_HEADERS.iter().find(|h| lower.starts_with(**h)) {
            let value = lower[h.len()..].trim();
            let name = &unquoted[..h.len()];
            if value.is_empty() {
                out.push(w);
                next = Next::UntilFlag;
            } else {
                out.push(format!("{name} {REDACTED}"));
                // `Authorization:Bearer` with the token in the next word.
                if matches!(value, "bearer" | "basic" | "token" | "digest" | "negotiate") {
                    next = Next::Secret;
                }
            }
            continue;
        }
        // `--password=x`, `--data={"a":1}`: a flag with its value attached.
        if let Some((flag, _)) = lower.split_once('=')
            && flag.starts_with('-')
            && (SECRET_FLAGS.contains(&flag) || secretish(unquoted))
        {
            out.push(format!("{}={REDACTED}", &unquoted[..flag.len()]));
            continue;
        }
        if SECRET_FLAGS.contains(&lower.as_str()) || (http && HTTP_SHORT_FLAGS.contains(&unquoted))
        {
            out.push(w);
            next = Next::Secret;
            continue;
        }
        // `sshpass -p x`: a lone `-p` is a port elsewhere (`ssh -p 22`), a password after sshpass.
        if lower == "-p" && sshpass {
            out.push(w);
            next = Next::Secret;
            continue;
        }
        // `mysql -pSECRET`: a password glued to the flag. After a client that takes one, anything;
        // elsewhere (`mkdir -pv`, `find -print`) only what looks like a password.
        if let Some(rest) = unquoted.strip_prefix("-p")
            && !rest.is_empty()
            && !rest.starts_with('-')
            && (pwd_client
                || (rest.len() >= 4
                    && rest
                        .chars()
                        .any(|c| c.is_ascii_digit() || c.is_ascii_uppercase())))
        {
            out.push(format!("-p{REDACTED}"));
            continue;
        }
        out.push(if secretish(&w) {
            REDACTED.to_owned()
        } else {
            w
        });
        if matches!(lower.as_str(), "bearer" | "basic") {
            next = Next::Secret;
        }
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

    /// SEC-143: one canary per way a credential can sit in a command; it never appears in the
    /// output. (Each line is a form the review named.)
    #[test]
    fn a_credential_in_any_common_form_never_reaches_the_row() {
        const C: &str = "cAnArY9z";
        let forms: &[&str] = &[
            "curl -H 'Authorization: Bearer cAnArY9z' https://x.example/a",
            "curl -H \"Authorization:Bearer cAnArY9z\" https://x.example/a",
            "curl -H 'Authorization: Token cAnArY9z' https://x.example/a",
            "curl -H Authorization:Bearer cAnArY9z https://x.example/a",
            "curl -H Authorization: Token cAnArY9z https://x.example/a",
            "curl -H 'Proxy-Authorization: Basic cAnArY9z' https://x.example/a",
            "curl -H 'X-Api-Key: cAnArY9z' https://x.example/a",
            "curl -H 'Cookie: session=cAnArY9z; a=b' https://x.example/a",
            "curl -b session=cAnArY9z https://x.example/a",
            "curl --cookie session=cAnArY9z https://x.example/a",
            "curl --cookie=session=cAnArY9z https://x.example/a",
            "curl -u alice:cAnArY9z https://x.example/a",
            "curl --user alice:cAnArY9z https://x.example/a",
            "curl --user=alice:cAnArY9z https://x.example/a",
            "curl -X POST -d '{\"password\":\"cAnArY9z\"}' https://x.example/a",
            "curl --data-raw '{\"password\": \"cAnArY9z\"}' https://x.example/a",
            "curl --data-binary @- --data-urlencode pw=cAnArY9z https://x.example/a",
            "curl --json '{\"token\":\"cAnArY9z\"}' https://x.example/a",
            "curl -F password=cAnArY9z https://x.example/a",
            "curl https://alice:cAnArY9z@host.example/a",
            "git clone https://cAnArY9z@host.example/a.git",
            "mysql -u root -pcAnArY9z db",
            "mysql -pcanary9zlower db",
            "sshpass -p cAnArY9z ssh host",
            "sshpass -p cAnArY9z ssh -p 22 host",
            "export DB_PASS=cAnArY9z && run",
            "DB_PW=cAnArY9z run",
            "GH_PAT=cAnArY9z run",
            "DB_PWD=cAnArY9z run",
            "SVC_CREDENTIALS=cAnArY9z run",
            "REG_AUTH=cAnArY9z run",
            "API_TOKEN=cAnArY9z run",
            "TOKEN=\"cAnArY9z and more\" run",
            "SECRET='two cAnArY9z words' run",
            "run --password cAnArY9z",
            "run --client-secret=cAnArY9z",
            "echo hi | curl -H 'authorization: bearer cAnArY9z' https://x.example/a",
        ];
        for f in forms {
            let c = command(f).unwrap();
            assert!(!c.contains(C), "{f:?} leaked: {c}");
            assert!(!c.contains("canary9zlower"), "{f:?} leaked: {c}");
            assert!(c.contains(REDACTED), "{f:?} was not redacted: {c}");
        }
    }

    #[test]
    fn ordinary_commands_keep_their_flags() {
        for ok in [
            "ls -la",
            "mkdir -pv a/b",
            "ssh -p 22 host",
            "find . -print",
            "docker run -d nginx",
            "git push -u origin main",
            "tail -f app.log",
            "rm -f old.txt",
            "curl -s -o out.json https://x.example/a",
            "cargo test -p aurora-api",
        ] {
            // `-u` is redacted on purpose (curl's user:password); the rest must be left alone.
            if ok.contains(" -u ") {
                continue;
            }
            assert_eq!(command(ok).unwrap(), ok);
        }
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
