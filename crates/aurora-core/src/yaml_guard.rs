//! The strictness pre-check for untrusted YAML, run before the parser sees it: whether any token
//! starts with an indicator the caller refuses (`&` anchor, `*` alias, `!` tag). The one
//! implementation: the skill validator (`aurora-registry`) and the template format
//! (`aurora-template`) both call it. A heuristic, not a parser: it is conservative on purpose,
//! and the parser's own alias limits still apply behind it.

/// Whether any YAML token starts with one of `indicators`: at the start of a line (after
/// block-sequence dashes), after `:`/`?`/`,`/`[`/`{` and spaces. A plain scalar that merely
/// *contains* `*emphasis*` is fine, one that *starts* with `*` is an alias.
///
/// Two passes, either of which refuses: each line on its own, and one that follows `[…]` / `{…}`
/// collections over line breaks.
#[must_use]
pub fn has_indicator_token(yaml: &str, indicators: &[char]) -> bool {
    in_a_line(yaml, indicators) || in_a_flow_collection(yaml, indicators)
}

/// Each line on its own.
fn in_a_line(yaml: &str, indicators: &[char]) -> bool {
    yaml.lines().any(|line| {
        let mut l = line.trim_start();
        while let Some(r) = l.strip_prefix("- ") {
            l = r.trim_start();
        }
        // A token can start here, or after an indicator (`: `, `,`, `[`, `{`, `? `) outside
        // quotes.
        let chars: Vec<char> = l.chars().collect();
        let mut quote: Option<char> = None;
        let mut token_start = true;
        let mut flow = false;
        for (i, &ch) in chars.iter().enumerate() {
            if let Some(q) = quote {
                if ch == q {
                    quote = None;
                }
                continue;
            }
            let next_blank = chars.get(i + 1).is_none_or(|n| matches!(n, ' ' | '\t'));
            match ch {
                c if token_start && indicators.contains(&c) => return true,
                '"' | '\'' if token_start => quote = Some(ch),
                ' ' | '\t' => continue,
                '[' | '{' if token_start => flow = true,
                '[' | '{' => {}
                ':' | '?' if next_blank => {
                    token_start = true;
                    continue;
                }
                ',' if flow => {
                    token_start = true;
                    continue;
                }
                _ => {}
            }
            token_start = false;
            if matches!(ch, '[' | '{') {
                // a flow collection opens a value position
                token_start = true;
            }
        }
        false
    })
}

/// Follows the nesting of flow collections (and the quoted scalars in them) across lines, and
/// looks only inside them. Comments and the content of block scalars (`|`, `>`) don't open one.
fn in_a_flow_collection(yaml: &str, indicators: &[char]) -> bool {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    // The indentation of the line that opened a block scalar.
    let mut block: Option<usize> = None;
    for line in yaml.lines() {
        let indent = line.len() - line.trim_start_matches(' ').len();
        if let Some(opened_at) = block {
            if line.trim().is_empty() || indent > opened_at {
                continue;
            }
            block = None;
        }
        let mut l = line.trim_start();
        while quote.is_none()
            && let Some(r) = l.strip_prefix("- ")
        {
            l = r.trim_start();
        }
        let chars: Vec<char> = l.chars().collect();
        let mut token_start = quote.is_none();
        // Whether the last token was a closed quoted scalar (`{"a":*x}` needs no space).
        let mut after_quoted = false;
        let mut i = 0;
        while i < chars.len() {
            let ch = chars[i];
            i += 1;
            if let Some(q) = quote {
                if q == '"' && ch == '\\' {
                    i += 1;
                } else if ch == q {
                    if q == '\'' && chars.get(i) == Some(&'\'') {
                        i += 1;
                    } else {
                        quote = None;
                        after_quoted = true;
                    }
                }
                continue;
            }
            let next_blank = chars.get(i).is_none_or(|n| matches!(n, ' ' | '\t'));
            match ch {
                ' ' | '\t' => continue,
                '#' if i == 1 || matches!(chars[i - 2], ' ' | '\t') => break,
                c if token_start && depth > 0 && indicators.contains(&c) => return true,
                '"' | '\'' if token_start => {
                    quote = Some(ch);
                    token_start = false;
                }
                '[' | '{' if token_start || depth > 0 => {
                    depth += 1;
                    token_start = true;
                }
                ']' | '}' if depth > 0 => {
                    depth -= 1;
                    token_start = false;
                }
                ',' if depth > 0 => token_start = true,
                ':' if next_blank || (depth > 0 && after_quoted) => token_start = true,
                '?' if next_blank => token_start = true,
                '|' | '>' if token_start && depth == 0 && is_block_header_rest(&chars[i..]) => {
                    block = Some(indent);
                    break;
                }
                // A tag or anchor this pass doesn't refuse (outside a collection the first
                // pass judges it) leaves the value position open: `!t [` still opens one.
                '&' | '!' if token_start => {
                    while chars
                        .get(i)
                        .is_some_and(|n| !matches!(n, ' ' | '\t' | ',' | '[' | ']' | '{' | '}'))
                    {
                        i += 1;
                    }
                }
                _ => token_start = false,
            }
            after_quoted = false;
        }
    }
    false
}

/// What may follow `|` or `>` on a block scalar's header line: chomping and indentation
/// indicators, then nothing but a comment.
fn is_block_header_rest(rest: &[char]) -> bool {
    let mut it = rest
        .iter()
        .skip_while(|c| matches!(c, '+' | '-' | '0'..='9'));
    match it.next() {
        None => true,
        Some(' ' | '\t') => it
            .find(|c| !matches!(c, ' ' | '\t'))
            .is_none_or(|c| *c == '#'),
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[char] = &['&', '*', '!'];

    #[test]
    fn tokens_on_one_line() {
        for bad in ["a: &x 1", "a: *x", "a: !t 1", "- &a 1", "a: [x, *y]"] {
            assert!(has_indicator_token(bad, ALL), "{bad}");
        }
        assert!(!has_indicator_token("a: !t 1", &['&', '*']));
    }

    #[test]
    fn flow_collections_over_several_lines() {
        for bad in [
            "a: [x,\n  y, &z w]",
            "[x,\n  y, *z]",
            "a: {k: v,\n  l: m, n: *z}",
            "a: [[x],\n  y, *z]",
            "a: [x, # ] a comment\n  y, *z]",
            "a: [\"x\n  ]\", y, *z]",
            "a: ['it''s ]',\n  y, *z]",
            "a: [\"\\\" ]\",\n  y, *z]",
            "a: !!seq [x,\n  y, *z]",
            "a: {\"k\":1,\n  l: 2, \"n\":*z}",
        ] {
            assert!(has_indicator_token(bad, &['&', '*']), "{bad}");
        }
    }

    #[test]
    fn ordinary_text_is_not_refused() {
        for good in [
            "a: [x,\n  y, \"*z\"]",
            "a: [x,\n  y]\nb: one, *two*",
            "a: 1 # [see\nb: one, *two*",
            "# [x\nb: one, *two*",
            "a: |\n  [open\n  one, *two*\nb: one, *two*",
            "a: >-\n  { open\n\n  one, *two*\nb: three, *four*",
            "a: Use [x] here\nb: one, *two*",
        ] {
            assert!(!has_indicator_token(good, ALL), "{good}");
        }
        // Where tags are allowed, one right before the closing bracket still lets the
        // collection close.
        assert!(!has_indicator_token(
            "a: [x, !t]\nb: one, *two*",
            &['&', '*']
        ));
    }
}
