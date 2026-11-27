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
    // command family only (`mysql -p …`, `curl -u …`): the bare `-p` of `ls -p` is not a password.
    const SECRET_FLAGS: [&str; 7] = [
        "--token",
        "--password",
        "--passwd",
        "--secret",
        "--api-key",
        "--apikey",
        "--passphrase",
    ];
    const P_IS_PASSWORD: [&str; 5] = ["mysql", "mariadb", "mysqldump", "mongo", "mongosh"];
    fn secret_name(k: &str, keys: &[&str; 8]) -> bool {
        keys.iter()
            .any(|s| k == *s || k.ends_with(&format!("_{s}")) || k.ends_with(&format!("-{s}")))
    }
    let line = one_line(s, max.saturating_mul(2).max(64));
    // The command families the short flags are read in: a `-p` is a password only after `mysql`
    // and friends, `-u` a user only after `curl`. The command may appear anywhere (`&& mysql …`).
    let mut p_is_password = false;
    let mut u_is_user = false;
    let mut out: Vec<String> = Vec::new();
    let mut mask_next = false;
    for word in line.split(' ') {
        let lower = word.to_ascii_lowercase();
        let base = lower.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
        if P_IS_PASSWORD.contains(&base) {
            p_is_password = true;
        }
        if base == "curl" {
            u_is_user = true;
        }
        if mask_next {
            // `Authorization: Basic x`: the scheme word is shown, its credential stays masked.
            if lower == "bearer" || lower == "basic" {
                out.push(word.to_owned());
                continue;
            }
            mask_next = false;
            out.push("***".into());
            continue;
        }
        if lower == "bearer" || lower == "basic" {
            mask_next = true;
            out.push(word.to_owned());
            continue;
        }
        if SECRET_FLAGS.contains(&lower.as_str())
            || (lower == "-p" && p_is_password)
            || (lower == "-u" && u_is_user)
        {
            mask_next = true;
            out.push(word.to_owned());
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
                if !w[i + 1..].is_empty() && secret_name(k, &SECRET_KEYS) {
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
            mask_next = true;
        }
        out.push(w);
    }
    clamp(out.join(" "), max)
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
