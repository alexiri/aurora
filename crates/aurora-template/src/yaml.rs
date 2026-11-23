//! Text handling for untrusted repository bytes: cleaning, strict-YAML refusals and front
//! matter. Modelled on `aurora-registry`'s skill format guard (FIX-096): anchors and aliases are
//! refused before the parser sees them; tags and multiple documents are refused too (C-1141).

use aurora_core::sanitize::is_stripped;

/// Control and format characters (bidi, zero-width, …) are removed from repository text before
/// it can become part of what an agent is told; CRLF becomes LF. Everything else, including
/// indentation, is kept exactly (as for library skills, `10 §Git skill library`).
#[must_use]
pub fn clean_text(s: &str) -> String {
    s.replace("\r\n", "\n")
        .chars()
        .filter(|c| !is_stripped(*c))
        .collect()
}

/// Strips a byte-order mark, if any.
#[must_use]
pub fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

/// Whether any YAML token starts with `&` (anchor), `*` (alias) or `!` (tag): at the start of a
/// line (after block-sequence dashes), after `:`/`,`/`[`/`{` and spaces. Conservative on
/// purpose: a plain scalar that merely *contains* `*emphasis*` is fine, one that *starts* with
/// `*` is an alias.
#[must_use]
pub fn has_anchor_alias_or_tag(yaml: &str) -> bool {
    yaml.lines().any(|line| {
        let mut l = line.trim_start();
        while let Some(r) = l.strip_prefix("- ") {
            l = r.trim_start();
        }
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
                '&' | '*' | '!' if token_start => return true,
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

/// The strict pre-checks every template YAML file runs before the parser sees it, then the
/// single-document parse into a `serde_yaml::Value`. A refusal carries one fixed sentence.
pub fn parse_strict_yaml(text: &str) -> Result<serde_yaml::Value, String> {
    let text = strip_bom(text);
    if text.contains('\0') {
        return Err("YAML must not contain NUL bytes".into());
    }
    if has_anchor_alias_or_tag(text) {
        return Err("YAML must not use anchors, aliases or tags".into());
    }
    // Multiple documents are refused (serde_yaml refuses them itself; this also bounds the
    // typed parse below to the first document's shape).
    let value: serde_yaml::Value = match serde_yaml::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("multiple documents") {
                return Err("YAML must hold exactly one document".into());
            }
            return Err("YAML is not valid".into());
        }
    };
    Ok(value)
}

/// Splits `---\n…\n---\n` front matter from the body on an owned string
/// (`(front_matter_yaml, body)`); `None` when there is no closed front matter.
pub fn split_front_matter_owned(raw: String) -> Option<(String, String)> {
    let text = strip_bom(&raw).to_owned();
    let text = text.replace("\r\n", "\n");
    let rest = text.strip_prefix("---\n")?;
    let mut offset = 0;
    let mut end = None;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches('\n') == "---" {
            end = Some((offset, offset + line.len()));
            break;
        }
        offset += line.len();
    }
    let (fm_end, body_start) = end?;
    Some((rest[..fm_end].to_owned(), rest[body_start..].to_owned()))
}

/// The problem path of a typed-parse refusal: `<file>#/apps/1/level` (C-1141's shape).
pub fn typed_error_path(file: &str, e: &serde_path_to_error::Error<serde_yaml::Error>) -> String {
    use serde_path_to_error::Segment;
    let mut out = format!("{file}#");
    for segment in e.path() {
        match segment {
            Segment::Map { key } => out.push_str(&format!("/{key}")),
            Segment::Seq { index } => out.push_str(&format!("/{index}")),
            Segment::Enum { variant } => out.push_str(&format!("/{variant}")),
            Segment::Unknown => out.push_str("/?"),
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn cleaning() {
        assert_eq!(clean_text("a\r\nb\u{200B}c\u{202E}d"), "a\nbcd");
        assert_eq!(clean_text("a\n\n\n\nb"), "a\n\n\n\nb"); // newlines kept as typed
        assert_eq!(strip_bom("\u{feff}x"), "x");
    }

    #[test]
    fn anchor_alias_tag_refusals() {
        for bad in [
            "a: &x 1",
            "a: *x",
            "a: !tag 1",
            "&anchor\na: 1",
            "- &a 1",
            "a:\n  - *b",
        ] {
            assert!(has_anchor_alias_or_tag(bad), "{bad}");
        }
        for good in [
            "name: Service monitor",
            "why: Important! Really.",
            "summary: A *fine* agent",
            "why: 'quoted *value'",
            "apps:\n  - app: atlassian\n    level: ask",
        ] {
            assert!(!has_anchor_alias_or_tag(good), "{good}");
        }
    }

    #[test]
    fn strict_parse() {
        assert!(parse_strict_yaml("a: 1").is_ok());
        assert!(parse_strict_yaml("a: &x 1").is_err());
        assert!(parse_strict_yaml("a: 1\n---\nb: 2").is_err());
        assert!(parse_strict_yaml("a: [1,2").is_err());
    }

    #[test]
    fn front_matter() {
        let raw = "---\nkey: notes\nlocked: true\n---\nbody text\n";
        let (fm, body) = split_front_matter_owned(raw.to_owned()).unwrap();
        assert_eq!(fm, "key: notes\nlocked: true\n");
        assert_eq!(body, "body text\n");
        assert!(split_front_matter_owned("no front matter".to_owned()).is_none());
        assert!(split_front_matter_owned("---\nkey: x\n".to_owned()).is_none());
        let (fm, body) =
            split_front_matter_owned("---\r\nkey: x\r\n---\r\nbody\r\n".to_owned()).unwrap();
        assert_eq!(fm, "key: x\n");
        assert_eq!(body, "body\n");
    }
}
