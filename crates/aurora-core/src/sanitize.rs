//! Sanitizers for untrusted text (`02 §Untrusted text`). The only sanitizers
//! to use before placing model/user/plugin/connector text in platform voice.
//!
//! Handling: C0/C1 controls, DEL, bidi controls and Unicode `Cf` format
//! characters (zero-width etc.) are removed. Whitespace (including `\t`,
//! `\r`, U+0085, Unicode spaces) collapses to a single space; `\n` is kept
//! by [`block`] only.

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
    let line = one_line(s, max.saturating_mul(2).max(64));
    let mut out: Vec<String> = Vec::new();
    let mut mask_next = false;
    for word in line.split(' ') {
        if mask_next {
            mask_next = false;
            out.push("***".into());
            continue;
        }
        let lower = word.to_ascii_lowercase();
        if lower == "bearer" || lower == "basic" {
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
        // key=value / key:value pairs.
        for sep in ['=', ':'] {
            if let Some(i) = w.find(sep) {
                let k = w[..i]
                    .trim_start_matches(['"', '\'', '{', '(', '['])
                    .to_ascii_lowercase();
                let k = k.trim_end_matches(['"', '\'']);
                if !w[i + 1..].is_empty()
                    && SECRET_KEYS.iter().any(|s| {
                        k == *s || k.ends_with(&format!("_{s}")) || k.ends_with(&format!("-{s}"))
                    })
                {
                    w = format!("{}{sep}***", &w[..i]);
                }
            }
        }
        if SECRET_KEYS
            .iter()
            .any(|s| lower.trim_end_matches(':') == *s)
            && word.ends_with(':')
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
