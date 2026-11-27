//! Sanitizers for untrusted text (`02 §Untrusted text`). The only sanitizers
//! to use before placing model/user/plugin/connector text in platform voice.
//!
//! Handling: C0/C1 controls, DEL, bidi controls and Unicode `Cf` format
//! characters (zero-width etc.) are removed. Whitespace (including `\t`,
//! `\r`, U+0085, Unicode spaces) collapses to a single space; `\n` is kept
//! by [`block`] only.

mod chunk;
mod safe;
pub use chunk::markdown_chunks;
pub use safe::{
    allowed_link_scheme, anchor_is_closed, break_preview_urls, broadcast_here, defang_prose,
    escape_table_excess, has_preview_url, link_host_lie, markdown_fallbacks, markdown_safe,
};

/// `true` for characters that are removed outright: controls (C0, DEL, C1)
/// that are not whitespace, plus Unicode general category `Cf`
/// (includes bidi controls U+202A-202E, U+2066-2069, U+200E/F, U+061C).
pub fn is_stripped(c: char) -> bool {
    (c.is_control() && !c.is_whitespace()) || is_format(c)
}

/// Unicode `Cf` (Format) code points (Unicode 15/16 table).
fn is_format(c: char) -> bool {
    matches!(u32::from(c),
        0x00AD
        | 0x0600..=0x0605
        | 0x061C
        | 0x06DD
        | 0x070F
        | 0x0890..=0x0891
        | 0x08E2
        | 0x180E
        | 0x200B..=0x200F
        | 0x202A..=0x202E
        | 0x2060..=0x2064
        | 0x2066..=0x206F
        | 0xFEFF
        | 0xFFF9..=0xFFFB
        | 0x110BD
        | 0x110CD
        | 0x13430..=0x1343F
        | 0x1BCA0..=0x1BCA3
        | 0x1D173..=0x1D17A
        | 0xE0001
        | 0xE0020..=0xE007F)
}

fn is_newline(c: char) -> bool {
    matches!(c, '\n' | '\u{2028}' | '\u{2029}')
}

fn normalize(s: &str, keep_newlines: bool) -> String {
    let mut out = String::with_capacity(s.len().min(4096));
    let mut newlines = 0usize;
    let mut space = false;
    for c in s.chars() {
        if is_newline(c) {
            if keep_newlines {
                newlines += 1;
            } else {
                space = true;
            }
        } else if c.is_whitespace() {
            space = true;
        } else if is_stripped(c) {
            continue;
        } else {
            if !out.is_empty() {
                if newlines > 0 {
                    for _ in 0..newlines.min(2) {
                        out.push('\n');
                    }
                } else if space {
                    out.push(' ');
                }
            }
            newlines = 0;
            space = false;
            out.push(c);
        }
    }
    out
}

/// Clamp to at most `max` chars; if cut, the last char is `…`.
fn clamp(mut s: String, max: usize) -> String {
    if s.chars().count() <= max {
        return s;
    }
    if max == 0 {
        return String::new();
    }
    let cut = s.char_indices().nth(max - 1).map_or(s.len(), |(i, _)| i);
    s.truncate(cut);
    s.truncate(s.trim_end().len());
    s.push('…');
    s
}

/// Single line: no newlines, collapsed whitespace, trimmed, <= `max` chars.
pub fn one_line(s: &str, max: usize) -> String {
    clamp(normalize(s, false), max)
}

/// (FIX-025, D-158) A start-failure reason or substrate event shown to users and operators:
/// [`one_line`], with credentials masked: URL userinfo (`scheme://user:pass@` → `scheme://***@`),
/// Aurora tokens (`aub_…` → `aub_***`), `Bearer`/`Basic` credentials, and the values of
/// `password=`, `token=`, `secret=`, `key=` style pairs. Defence in depth: the sources (Kubernetes
/// events, scheduler and registry messages, supervisor errors) do not carry secrets by design.
pub fn reason(s: &str, max: usize) -> String {
    const SECRET_KEYS: [&str; 8] = [
        "password",
        "passwd",
        "token",
        "secret",
        "apikey",
        "api_key",
        "key",
        "authorization",
    ];
    // (FIX-253 review) Long flags whose value is a credential, and the short flags read in their
    // command family only (`mysql -p…`, `curl -u …`): the bare `-p` of `ls -p` is not a password.
    const SECRET_FLAGS: [&str; 7] = [
        "--token",
        "--password",
        "--passwd",
        "--secret",
        "--api-key",
        "--apikey",
        "--passphrase",
    ];
    // (FIX-265) Environment names end in the secret word with or without a separator
    // (`PGPASSWORD`, `MYSQL_PWD`, `GHTOKEN`); read for the `=` form only.
    const ENV_ENDINGS: [&str; 7] = [
        "password", "passwd", "pwd", "token", "secret", "key", "sshpass",
    ];
    // The words that name a scheme behind `Authorization:`; the credential follows them.
    const SCHEMES: [&str; 7] = [
        "bearer",
        "basic",
        "token",
        "apikey",
        "digest",
        "bot",
        "negotiate",
    ];
    fn secret_name(k: &str, keys: &[&str; 8]) -> bool {
        keys.iter()
            .any(|s| k == *s || k.ends_with(&format!("_{s}")) || k.ends_with(&format!("-{s}")))
    }
    let line = one_line(s, max.saturating_mul(2).max(64));
    // The command families the short flags are read in: the command may appear anywhere
    // (`&& mysql …`, `/usr/bin/curl …`).
    let mut family = Families::default();
    let mut out: Vec<String> = Vec::new();
    let mut pending = Pending::Nothing;
    // A masked value that opened a quote: the words up to the closing one belong to it.
    let mut quoted: Option<char> = None;
    for word in line.split(' ') {
        if let Some(q) = quoted {
            if word.contains(q) {
                quoted = None;
            }
            continue;
        }
        let lower = word.to_ascii_lowercase();
        family.note(&lower);
        match pending {
            Pending::Header => {
                // `Authorization: Basic x`: the scheme word is shown, its credential masked.
                if SCHEMES.contains(&lower.as_str()) {
                    out.push(word.to_owned());
                    continue;
                }
                pending = Pending::Nothing;
                quoted = open_quote(word);
                out.push("***".into());
                continue;
            }
            Pending::Value => {
                // A scheme word as a flag's value is masked too, and so is what follows it.
                if lower != "bearer" && lower != "basic" {
                    pending = Pending::Nothing;
                }
                quoted = open_quote(word);
                out.push("***".into());
                continue;
            }
            Pending::Nothing => {}
        }
        // `Authorization:` and `Proxy-Authorization:`, however they are spelled (`-H…`,
        // `--header=…`, quoted, JSON): a scheme glued to the name, or nothing after it, means
        // the credential is a following word. Anything else is masked by the `:` rule below.
        let bare: String = lower.chars().filter(|c| !matches!(c, '"' | '\'')).collect();
        if let Some((_, after)) = bare.split_once("authorization:")
            && (after.is_empty() || SCHEMES.contains(&after))
        {
            pending = Pending::Header;
            out.push(word.to_owned());
            continue;
        }
        if lower == "bearer" || lower == "basic" {
            pending = Pending::Header;
            out.push(word.to_owned());
            continue;
        }
        if SECRET_FLAGS.contains(&lower.as_str()) {
            pending = Pending::Value;
            out.push(word.to_owned());
            continue;
        }
        if let Some(short) = family.flag(word) {
            if short.value_follows {
                pending = Pending::Value;
            }
            quoted = short.quoted;
            out.push(short.shown);
            continue;
        }
        let mut w = word.to_owned();
        // URL userinfo.
        if let Some(i) = w.find("://") {
            let rest = &w[i + 3..];
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            if let Some(at) = rest[..end].rfind('@') {
                w = format!("{}***{}", &w[..i + 3], &rest[at..]);
            }
        }
        // Aurora tokens anywhere in the word.
        for p in crate::token::TokenPrefix::ALL {
            let p = p.as_str();
            if let Some(i) = w.find(p)
                && w[i + p.len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric())
                && (i == 0 || !w.as_bytes()[i - 1].is_ascii_alphanumeric())
            {
                w = format!("{}{p}***", &w[..i]);
            }
        }
        // key=value / key:value pairs (the long flags' `=` form: `--password=x`).
        for sep in ['=', ':'] {
            if let Some(i) = w.find(sep) {
                let k = w[..i]
                    .trim_start_matches(['"', '\'', '{', '(', '[', '-'])
                    .to_ascii_lowercase();
                let k = k.trim_end_matches(['"', '\'']);
                let secret = secret_name(k, &SECRET_KEYS)
                    || (sep == '='
                        && (ENV_ENDINGS.iter().any(|e| k.ends_with(e)) || k == "passphrase"));
                if !w[i + 1..].is_empty() && secret {
                    quoted = quoted.or_else(|| open_quote(&w[i + 1..]));
                    w = format!("{}{sep}***", &w[..i]);
                }
            }
        }
        // A bare header or flag name in front of its value (`Authorization: x`, `Cookie: x`,
        // `X-Api-Key: x` — header names compound: `x-api-key` ends with `-key`).
        let name = word
            .trim_matches(|c: char| c == '\'' || c == '"')
            .to_ascii_lowercase();
        if name.ends_with(':')
            && (secret_name(name.trim_end_matches(':'), &SECRET_KEYS)
                || name.trim_end_matches(':') == "cookie")
        {
            pending = Pending::Header;
        }
        out.push(w);
    }
    clamp(out.join(" "), max)
}

/// What [`reason`] still has to mask in the words that follow.
#[derive(Clone, Copy)]
enum Pending {
    Nothing,
    /// A flag's value.
    Value,
    /// The value behind a header name or a scheme word: a scheme word in front of it stays.
    Header,
}

/// The quote a masked value opened and did not close.
fn open_quote(value: &str) -> Option<char> {
    let q = value.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    (!value[1..].contains(q)).then_some(q)
}

/// The commands seen so far in a line whose short flags carry credentials.
#[derive(Default)]
struct Families {
    /// `-p`: the database clients (`-pSECRET`) and `sshpass`.
    p: bool,
    redis: bool,
    curl: bool,
}

/// A masked short flag: the word as shown, and what of its value is still to come.
struct ShortFlag {
    shown: String,
    value_follows: bool,
    quoted: Option<char>,
}

impl Families {
    fn note(&mut self, lower: &str) {
        let base = lower.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
        match base.rsplit('/').next().unwrap_or(base) {
            "mysql" | "mariadb" | "mysqldump" | "mysqladmin" | "mariadb-dump" | "mongo"
            | "mongosh" | "sshpass" => self.p = true,
            "redis-cli" => self.redis = true,
            "curl" => self.curl = true,
            _ => {}
        }
    }

    /// A credential flag of one of the seen commands, apart (`-u bob:pw`) or with its value
    /// attached (`-ubob:pw`, `--user=bob:pw`).
    fn flag(&self, word: &str) -> Option<ShortFlag> {
        let flag = word.trim_start_matches(['"', '\'']);
        let lead = &word[..word.len() - flag.len()];
        let apart = |shown: &str| {
            Some(ShortFlag {
                shown: shown.to_owned(),
                value_follows: true,
                quoted: None,
            })
        };
        // `at`: where the attached value begins in `flag`.
        let attached = |at: usize, value_follows: bool| {
            Some(ShortFlag {
                shown: format!("{lead}{}***", &flag[..at]),
                value_follows,
                quoted: open_quote(&flag[at..]),
            })
        };
        let lower = flag.to_ascii_lowercase();
        if (self.p && lower == "-p")
            || (self.redis && (flag == "-a" || lower == "--pass"))
            || (self.curl && (lower == "--user" || lower == "--proxy-user"))
        {
            return apart(word);
        }
        if self.redis && lower.starts_with("--pass=") {
            return attached("--pass=".len(), false);
        }
        if self.curl {
            for long in ["--user=", "--proxy-user="] {
                if lower.starts_with(long) && flag.len() > long.len() {
                    return attached(long.len(), false);
                }
            }
        }
        if flag.starts_with("--") {
            return None;
        }
        // (`-p` in lower case only: `-P3306` is a port.)
        if (self.p && flag.starts_with("-p") || self.redis && flag.starts_with("-a"))
            && flag.len() > 2
        {
            return attached(2, false);
        }
        // curl's short flags group (`-su bob:pw`, `-sSubob:pw`); `-H…` is a header.
        if self.curl && flag.starts_with('-') && !flag.starts_with("-H") {
            let letters = flag[1..]
                .find(|c: char| !c.is_ascii_alphabetic())
                .map_or(flag.len(), |i| i + 1);
            if let Some(u) = flag[..letters].find(['u', 'U']) {
                if u + 1 == flag.len() {
                    return apart(word);
                }
                // All letters ending in `u` reads both ways: mask both.
                let both = letters == flag.len() && flag.ends_with(['u', 'U']);
                return attached(u + 1, both);
            }
        }
        None
    }
}

/// Like [`one_line`] but keeps `\n` (runs of more than 2 collapse to 2).
pub fn block(s: &str, max: usize) -> String {
    clamp(normalize(s, true), max)
}

/// `one_line(s, 80)`: connection labels, secret names, plugin names.
pub fn label(s: &str) -> String {
    one_line(s, 80)
}

/// (P5-GUIDE-1, moved from the supervisor's instruction renderer) One line of untrusted text for
/// platform-voiced files: control characters become spaces, runs collapse, hard clamp to `max`
/// characters (no ellipsis).
pub fn inline(s: &str, max: usize) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut out = String::new();
    for w in cleaned.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out.chars().take(max).collect()
}

/// (P5-GUIDE-1, moved from the supervisor) Untrusted block text inside a code fence longer than
/// any backtick run it contains; control characters other than LF and TAB are dropped.
pub fn fenced(s: &str, max: usize) -> String {
    let body: String = s
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .take(max)
        .collect();
    let mut run = 0usize;
    let mut longest = 0usize;
    for c in body.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}text\n{}\n{fence}\n", body.trim_end())
}

/// [`block`] (clamped to `max` chars *before* escaping, so the result may be
/// longer) followed by markdown/HTML escaping so a renderer shows it as text.
///
/// Approach: backslash-escape every markdown-active ASCII character
/// (`\ ` * _ [ ] ( ) < > # + - ! | ~ = { } &`, and `.` after a digit, to stop
/// ordered lists). Because `[`, `]`, `(`, `)`, `<`, `>` are escaped, no link,
/// image, autolink or raw HTML can form; a URL in the input therefore appears
/// literally, with its target visible, and is never an active link
/// (`[text](http://x)` renders as the literal text `[text](http://x)`).
pub fn markdown_as_data(s: &str, max: usize) -> String {
    let b = block(s, max);
    let mut out = String::with_capacity(b.len() + b.len() / 8);
    let mut prev = ' ';
    for c in b.chars() {
        let active = matches!(
            c,
            '\\' | '`'
                | '*'
                | '_'
                | '['
                | ']'
                | '('
                | ')'
                | '<'
                | '>'
                | '#'
                | '+'
                | '-'
                | '!'
                | '|'
                | '~'
                | '='
                | '{'
                | '}'
                | '&'
        ) || (c == '.' && prev.is_ascii_digit());
        if active {
            out.push('\\');
        }
        out.push(c);
        prev = c;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn reason_masks_credentials() {
        let r = reason(
            "pull https://bob:hunter2@reg.example/aurora:1 failed: unauthorized\nBearer abc.def token=xyz aub_0123456789abcdef password: s3cr3t",
            500,
        );
        for leaked in ["hunter2", "abc.def", "xyz", "0123456789abcdef", "s3cr3t"] {
            assert!(!r.contains(leaked), "{leaked} leaked in {r}");
        }
        assert!(r.contains("https://***@reg.example/aurora:1"), "{r}");
        assert!(r.contains("aub_***"), "{r}");
        // Ordinary scheduler text is unchanged.
        let s = "0/3 nodes are available: 3 Insufficient memory.";
        assert_eq!(reason(s, 300), s);
        assert_eq!(
            reason("image: docker.io/x/y:1", 300),
            "image: docker.io/x/y:1"
        );
    }

    /// (FIX-253 review) The Background activity command line masks the flag forms too: long
    /// credential flags, the short password flag of the commands where `-p` is one (`mysql`),
    /// `curl -u`, header-style names and `Authorization: Basic`. A bare `-p` of `ls -p` and a
    /// path that merely contains "token" are not masked.
    #[test]
    fn reason_masks_flag_and_header_forms() {
        let r = reason(
            "curl --token t0k --password p4ss -X POST https://bob:hunter2@x.example \
             -u bob:hunter2 -H 'Cookie: session=abc123' -H 'X-Api-Key: key123' \
             -H 'Authorization: Basic dXNlcjpwYXNz' && mysql -p hush",
            500,
        );
        for leaked in [
            "t0k",
            "p4ss",
            "hunter2",
            "abc123",
            "key123",
            "dXNlcjpwYXNz",
            "hush",
        ] {
            assert!(!r.contains(leaked), "{leaked} leaked in {r}");
        }
        assert!(r.contains("https://***@x.example"), "{r}");
        // False positives stay untouched.
        assert_eq!(
            reason("ls -p src && cat /var/tokens/list.txt", 200),
            "ls -p src && cat /var/tokens/list.txt"
        );
    }

    /// (FIX-265) The common forms a command line carries a credential in. Each case is one
    /// command and the secret in it, which the masked line no longer holds.
    #[test]
    fn reason_masks_the_common_command_line_forms() {
        let cases = [
            // The short password flags, attached and apart, in their command families.
            ("mysql -u root -pS3CRET1 db", "S3CRET1"),
            ("mysqldump -h db -pS3CRET2 --all-databases", "S3CRET2"),
            ("sshpass -p S3CRET3 ssh host", "S3CRET3"),
            ("sshpass -pS3CRET4 ssh host", "S3CRET4"),
            ("redis-cli -h cache -a S3CRET5 ping", "S3CRET5"),
            ("redis-cli -aS3CRET6 ping", "S3CRET6"),
            // curl's user, in every spelling.
            ("curl --user bob:S3CRET7 https://x", "S3CRET7"),
            ("curl --user=bob:S3CRET8 https://x", "S3CRET8"),
            ("curl -ubob:S3CRET9 https://x", "S3CRET9"),
            ("curl -u bob:S3CRET10 https://x", "S3CRET10"),
            ("curl -sSubob:S3CRET11 https://x", "S3CRET11"),
            ("curl -su bob:S3CRET12 https://x", "S3CRET12"),
            ("curl --proxy-user bob:S3CRET13 https://x", "S3CRET13"),
            ("/usr/bin/curl -u bob:S3CRET14 https://x", "S3CRET14"),
            // Long flags with `=`.
            ("tool --password=S3CRET15", "S3CRET15"),
            ("tool --token=S3CRET16", "S3CRET16"),
            // Environment names: the secret word needs no separator before it.
            ("PGPASSWORD=S3CRET17 psql", "S3CRET17"),
            ("MYSQL_PWD=S3CRET18 mysql db", "S3CRET18"),
            ("SSHPASS=S3CRET19 sshpass -e ssh host", "S3CRET19"),
            ("DBPASSWD=S3CRET20 ./run", "S3CRET20"),
            ("GHTOKEN=S3CRET21 gh pr list", "S3CRET21"),
            ("CLIENTSECRET=S3CRET22 ./run", "S3CRET22"),
            ("AWS_SECRET_ACCESSKEY=S3CRET23 aws s3 ls", "S3CRET23"),
            ("export OLDPWD=S3CRET24", "S3CRET24"),
            // Header forms without the space.
            (
                "curl -H 'Authorization:Bearer S3CRET25' https://x",
                "S3CRET25",
            ),
            (
                "curl -H 'Authorization:Basic S3CRET26' https://x",
                "S3CRET26",
            ),
            (
                "curl -H 'Authorization:Token S3CRET27' https://x",
                "S3CRET27",
            ),
            (
                "curl -H \"Proxy-Authorization:Basic S3CRET28\" https://x",
                "S3CRET28",
            ),
            (
                "curl -H 'Authorization: Token S3CRET29' https://x",
                "S3CRET29",
            ),
            ("curl -HAuthorization:Bearer S3CRET30 https://x", "S3CRET30"),
            (
                "curl --header=Authorization:token S3CRET31 https://x",
                "S3CRET31",
            ),
        ];
        let leaked: Vec<String> = cases
            .iter()
            .map(|(command, secret)| (command, secret, reason(command, 500)))
            .filter(|(_, secret, masked)| masked.contains(**secret))
            .map(|(command, _, masked)| format!("{command} -> {masked}"))
            .collect();
        assert!(leaked.is_empty(), "{leaked:#?}");
        // A quoted value with a space in it is one value, to its closing quote.
        for command in [
            "curl -u \"bob:two S3CRET\" https://x",
            "mysql -p'two S3CRET' db",
            "PGPASSWORD=\"two S3CRET\" psql",
            "tool --passphrase=S3CRET",
        ] {
            let masked = reason(command, 500);
            assert!(!masked.contains("S3CRET"), "{command} -> {masked}");
        }
        // The command itself stays readable, and a word that is not a credential stays.
        assert_eq!(
            reason("redis-cli -h cache -a hunter2 ping", 200),
            "redis-cli -h cache -a *** ping"
        );
        assert_eq!(
            reason("ls -a src && ssh -p 22 host && sort -u names.txt", 200),
            "ls -a src && ssh -p 22 host && sort -u names.txt"
        );
    }

    #[test]
    fn one_line_basics() {
        assert_eq!(one_line("  a \t b\r\n c  ", 100), "a b c");
        assert_eq!(one_line("a\u{0}b\u{7}c\u{7f}d\u{85}e", 100), "abcd e");
        assert_eq!(
            one_line("a\u{200B}b\u{202E}c\u{2066}d\u{061C}e\u{FEFF}", 100),
            "abcde"
        );
        assert_eq!(one_line("abcdef", 4), "abc…");
        assert_eq!(one_line("abcd", 4), "abcd");
        assert_eq!(one_line("abc def", 5), "abc…");
        assert_eq!(one_line("abc", 0), "");
        assert_eq!(one_line("é€😀xyz", 4), "é€😀…");
    }

    #[test]
    fn block_basics() {
        assert_eq!(block("a\n\n\n\nb", 100), "a\n\nb");
        assert_eq!(block("\n a  \n  b \n", 100), "a\nb");
        assert_eq!(block("a \r\n b", 100), "a\nb");
        assert_eq!(block("a\u{200B}\nb", 100), "a\nb");
    }

    #[test]
    fn label_clamps_80() {
        assert_eq!(label(&"x".repeat(200)).chars().count(), 80);
    }

    #[test]
    fn markdown_escapes() {
        assert_eq!(
            markdown_as_data("[hi](http://evil.example/x) <b>*x*</b>", 200),
            "\\[hi\\]\\(http://evil.example/x\\) \\<b\\>\\*x\\*\\</b\\>"
        );
        assert_eq!(
            markdown_as_data("1. a\n- b\n# c", 100),
            "1\\. a\n\\- b\n\\# c"
        );
        assert_eq!(markdown_as_data("a\u{202E}b", 100), "ab");
    }

    fn clean(s: &str, allow_nl: bool) -> bool {
        s.chars().all(|c| {
            !is_stripped(c) && !(c.is_control() && !(allow_nl && c == '\n'))
                && !matches!(u32::from(c), 0x200B..=0x200F | 0x202A..=0x202E | 0x2066..=0x2069 | 0x061C | 0xFEFF | 0x2028 | 0x2029)
        })
    }

    proptest! {
        #[test]
        fn one_line_props(s in any::<String>(), max in 0usize..200) {
            let o = one_line(&s, max);
            prop_assert!(o.chars().count() <= max);
            prop_assert!(!o.contains('\n'));
            prop_assert!(clean(&o, false));
            prop_assert_eq!(o.trim(), o.as_str());
            prop_assert!(!o.contains("  "));
        }

        #[test]
        fn block_props(s in any::<String>(), max in 0usize..200) {
            let o = block(&s, max);
            prop_assert!(o.chars().count() <= max);
            prop_assert!(clean(&o, true));
            prop_assert!(!o.contains("\n\n\n"));
        }

        #[test]
        fn label_and_markdown_props(s in any::<String>(), max in 0usize..200) {
            prop_assert!(label(&s).chars().count() <= 80);
            let m = markdown_as_data(&s, max);
            prop_assert!(clean(&m, true));
            prop_assert!(!m.contains("\n\n\n"));
        }

        #[test]
        fn reason_props(s in any::<String>(), max in 0usize..300) {
            let o = reason(&s, max);
            prop_assert!(o.chars().count() <= max);
            prop_assert!(!o.contains('\n'));
            prop_assert!(clean(&o, false));
        }

        #[test]
        fn idempotent_one_line(s in any::<String>()) {
            let o = one_line(&s, 1000);
            prop_assert_eq!(one_line(&o, 1000), o);
        }
    }
}
