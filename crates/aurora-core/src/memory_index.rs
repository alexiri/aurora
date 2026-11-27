//! The one index line of a memory (`10 §Memory`, C-1149): `memory_list`, `memory/index.md` and
//! the turn-start recall index all render it here.

/// Characters of the text after the key.
const LINE_MAX: usize = 120;

fn filled(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

/// `key: line`, or `key (locked): line` for a memory a person locked. The line is the
/// description, else the title, else the first content line, on one line and clamped.
///
/// The lock marker comes before everything the agent can write: a key has no space or
/// parenthesis, so no description, title or content can produce the `key (locked):` prefix.
#[must_use]
pub fn index_line(
    key: &str,
    title: Option<&str>,
    description: Option<&str>,
    content: &str,
    locked: bool,
) -> String {
    let line = filled(description)
        .or_else(|| filled(title))
        .or_else(|| content.lines().find(|l| !l.trim().is_empty()))
        .unwrap_or("");
    let one = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let clamped: String = one.chars().take(LINE_MAX).collect();
    let marker = if locked { " (locked)" } else { "" };
    format!("{key}{marker}: {clamped}")
}

#[cfg(test)]
mod tests {
    use super::index_line;

    #[test]
    fn the_marker_follows_the_key_and_text_stays_after_the_colon() {
        assert_eq!(
            index_line("a", None, Some("Rules"), "", true),
            "a (locked): Rules"
        );
        assert_eq!(
            index_line("a", None, Some("Rules (locked)"), "", false),
            "a: Rules (locked)"
        );
        // One line only, whatever the text holds.
        assert_eq!(
            index_line("a", Some(" "), None, "\nx\r\u{2028}b (locked): y", false),
            "a: x b (locked): y"
        );
        let long = "é".repeat(200);
        assert_eq!(
            index_line("a", Some(&long), None, "", true),
            format!("a (locked): {}", "é".repeat(120))
        );
    }
}
