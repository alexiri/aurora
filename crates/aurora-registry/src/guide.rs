//! Guide placeholders (P5-UX-13, D-334, C-940, C-941): `{{aurora.<name>}}` tokens a package's
//! `docs/setup.md` may carry and the guide route fills in for the viewer. The linter and the
//! route share this one scanner so they agree on what a token is.

/// The names the guide route knows how to fill (the part after `aurora.`).
pub const PLACEHOLDERS: [&str; 6] = [
    "callback_url",
    "external_id",
    "app_setup",
    "page:oauth_clients",
    "page:self_hosted",
    "page:packages",
];

const OPEN: &str = "{{aurora.";
const CLOSE: &str = "}}";

/// One `{{aurora.…}}` token: its byte range in the text and the name between the markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token<'a> {
    pub start: usize,
    pub end: usize,
    pub name: &'a str,
}

/// Every `{{aurora.…}}` token of `text`, in order. A token that is never closed on its line is
/// returned with its whole text as its name, so it reads as unknown rather than as text.
pub fn tokens(text: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = text[at..].find(OPEN) {
        let start = at + i;
        let name_start = start + OPEN.len();
        let line_end = text[name_start..]
            .find('\n')
            .map_or(text.len(), |n| name_start + n);
        match text[name_start..line_end].find(CLOSE) {
            Some(c) => {
                let name_end = name_start + c;
                out.push(Token {
                    start,
                    end: name_end + CLOSE.len(),
                    name: &text[name_start..name_end],
                });
                at = name_end + CLOSE.len();
            }
            None => {
                out.push(Token {
                    start,
                    end: line_end,
                    // The whole token text, which is never a known name.
                    name: &text[start..line_end],
                });
                at = line_end;
            }
        }
    }
    out
}

/// Names in `text` that are not in [`PLACEHOLDERS`].
pub fn unknown(text: &str) -> Vec<String> {
    tokens(text)
        .into_iter()
        .filter(|t| !PLACEHOLDERS.contains(&t.name))
        .map(|t| t.name.to_owned())
        .collect()
}

/// Replace each token with `value(name)`. Substitution is literal and one pass: a value is never
/// scanned again, so a value that contains `{{aurora.…}}` stays text. A token `value` declines
/// (`None`) stays as written.
pub fn fill(text: &str, mut value: impl FnMut(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for t in tokens(text) {
        out.push_str(&text[at..t.start]);
        match value(t.name) {
            Some(v) => out.push_str(&v),
            None => out.push_str(&text[t.start..t.end]),
        }
        at = t.end;
    }
    out.push_str(&text[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_fills_and_flags_unknown() {
        let t =
            "a {{aurora.callback_url}} b {{aurora.nope}} c {{aurora.page:packages}}\n{{aurora.open";
        let names: Vec<_> = tokens(t).iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            ["callback_url", "nope", "page:packages", "{{aurora.open"]
        );
        assert_eq!(unknown(t), ["nope", "{{aurora.open"]);
        // An unclosed token is unknown even when the name it starts is a real one.
        assert_eq!(unknown("{{aurora.external_id\n"), ["{{aurora.external_id"]);
        let f = fill(t, |n| {
            (n == "callback_url").then(|| "{{aurora.nope}}".to_owned())
        });
        assert!(f.starts_with("a {{aurora.nope}} b {{aurora.nope}} c"));
    }
}
