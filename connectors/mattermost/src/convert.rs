//! Neutral Markdown → Mattermost Markdown: the conversion table of `docs/20 §Rendering`
//! (FIX-142 step 4, D-227(c)).
//!
//! Native constructs pass through **byte for byte**: this is a pass over the pulldown-cmark
//! offset iterator that edits only the byte ranges of constructs Mattermost lacks — footnotes
//! (`[1]` + a Notes list), alerts (a bold-labelled quote), display math and ```` ```math ````
//! (```` ```latex ````) and the raw HTML the core lets through (to Markdown). Nothing is
//! dropped. Code spans, fenced blocks and the mention tokens are never touched.

use pulldown_cmark::{BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag};

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM
}

fn alert_label(k: BlockQuoteKind) -> &'static str {
    match k {
        BlockQuoteKind::Note => "ℹ️ **Note**",
        BlockQuoteKind::Tip => "💡 **Tip**",
        BlockQuoteKind::Important => "❗ **Important**",
        BlockQuoteKind::Warning => "⚠️ **Warning**",
        BlockQuoteKind::Caution => "🛑 **Caution**",
    }
}

/// Raw HTML (allowlisted by the core) → Markdown, with the state a stream of tags needs.
#[derive(Default)]
struct Html {
    hrefs: Vec<Option<String>>,
    lists: Vec<Option<u32>>,
    in_pre: bool,
    in_cell: bool,
    in_table: bool,
    rows: u32,
    cells: u32,
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let l = tag.to_ascii_lowercase();
    let at = l.find(&format!("{name}="))? + name.len() + 1;
    let rest = &tag[at..];
    let q = rest.chars().next()?;
    if q == '"' || q == '\'' {
        let inner = &rest[1..];
        Some(inner[..inner.find(q)?].to_owned())
    } else {
        Some(
            rest.split(|c: char| c.is_whitespace() || c == '>')
                .next()?
                .to_owned(),
        )
    }
}

impl Html {
    fn tag(&mut self, raw: &str, out: &mut String) {
        let inner = raw
            .trim_start_matches('<')
            .trim_end_matches('>')
            .trim_end_matches('/');
        let (closing, inner) = inner
            .strip_prefix('/')
            .map_or((false, inner), |r| (true, r));
        let name: String = inner
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        match (name.as_str(), closing) {
            ("b" | "strong", _) => out.push_str("**"),
            ("i" | "em", _) => out.push('_'),
            ("s" | "del", _) => out.push_str("~~"),
            ("code" | "kbd", _) if !self.in_pre => out.push('`'),
            ("pre", false) => {
                self.in_pre = true;
                out.push_str("\n\n```\n");
            }
            ("pre", true) => {
                self.in_pre = false;
                out.push_str("\n```\n\n");
            }
            ("br", _) => out.push_str("  \n"),
            ("hr", _) => out.push_str("\n\n---\n\n"),
            ("a", false) => {
                // The core keeps an `href` only for http, https and mailto; without one the
                // label stays and the tag goes.
                let href = attr(inner, "href");
                if href.is_some() {
                    out.push('[');
                }
                self.hrefs.push(href);
            }
            ("a", true) => {
                if let Some(Some(h)) = self.hrefs.pop() {
                    out.push_str(&format!("]({h})"));
                }
            }
            ("summary", false) => out.push_str("\n\n**"),
            ("summary", true) => out.push_str("**\n\n"),
            ("details" | "p" | "div" | "blockquote" | "caption", _) => out.push_str("\n\n"),
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                out.push_str("\n\n");
                out.push_str(&"#".repeat(usize::from(name.as_bytes()[1] - b'0')));
                out.push(' ');
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => out.push_str("\n\n"),
            ("ul", false) => {
                self.lists.push(None);
                out.push('\n');
            }
            ("ol", false) => {
                self.lists.push(Some(
                    attr(inner, "start")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(1),
                ));
                out.push('\n');
            }
            ("ul" | "ol", true) => {
                self.lists.pop();
                out.push('\n');
            }
            ("li", false) => {
                out.push('\n');
                out.push_str(&"  ".repeat(self.lists.len().saturating_sub(1)));
                match self.lists.last_mut() {
                    Some(Some(n)) => {
                        out.push_str(&format!("{n}. "));
                        *n += 1;
                    }
                    _ => out.push_str("- "),
                }
            }
            ("table", false) => {
                self.rows = 0;
                self.in_table = true;
                out.push_str("\n\n");
            }
            ("table", true) => {
                self.in_table = false;
                out.push_str("\n\n");
            }
            ("tr", false) => {
                self.cells = 0;
                out.push_str("\n| ");
            }
            ("tr", true) => {
                self.rows += 1;
                if self.rows == 1 {
                    out.push('\n');
                    out.push('|');
                    out.push_str(&"---|".repeat(self.cells.max(1) as usize));
                }
            }
            ("td" | "th", false) => self.in_cell = true,
            ("td" | "th", true) => {
                self.in_cell = false;
                self.cells += 1;
                out.push_str(" | ");
            }
            // sub, sup, ins, mark, small, span, thead, tbody, …: the content stays, the tag goes.
            _ => {}
        }
    }

    fn text(&self, t: &str, out: &mut String) {
        // Whitespace between list tags would make the list loose.
        if (!self.lists.is_empty() || self.in_table) && t.trim().is_empty() {
            return;
        }
        if self.in_cell {
            out.push_str(&t.replace('\n', " ").replace('|', "\\|"));
        } else {
            out.push_str(t);
        }
    }

    /// Convert a stretch of raw HTML (a block, or one inline tag).
    fn convert(&mut self, s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(p) = rest.find('<') {
            self.text(&rest[..p], &mut out);
            let tail = &rest[p..];
            match tail.find('>') {
                Some(e) if tail[1..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '/') => {
                    self.tag(&tail[..=e], &mut out);
                    rest = &tail[e + 1..];
                }
                _ => {
                    self.text("<", &mut out);
                    rest = &tail[1..];
                }
            }
        }
        self.text(rest, &mut out);
        out
    }
}

fn first_line_end(s: &str) -> usize {
    s.find('\n').unwrap_or(s.len())
}

/// Convert neutral Markdown for Mattermost. Text with nothing to convert comes back unchanged.
pub fn convert(neutral: &str) -> String {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut refs: Vec<String> = Vec::new();
    let mut defs: Vec<(String, String)> = Vec::new();
    let mut html = Html::default();
    let mut consumed = 0usize;
    for (ev, r) in Parser::new_ext(neutral, options()).into_offset_iter() {
        if r.start < consumed {
            continue;
        }
        match ev {
            Event::FootnoteReference(label) => {
                let label = label.to_string();
                let n = refs.iter().position(|l| *l == label).unwrap_or_else(|| {
                    refs.push(label);
                    refs.len() - 1
                }) + 1;
                edits.push((r.start, r.end, format!("[{n}]")));
            }
            Event::Start(Tag::FootnoteDefinition(label)) => {
                let raw = &neutral[r.clone()];
                let body = raw.split_once("]:").map_or(raw, |(_, b)| b);
                let mut lines = body.lines();
                let mut text = lines.next().unwrap_or("").trim().to_owned();
                for l in lines {
                    let l = l.strip_prefix("    ").unwrap_or(l.trim_start());
                    if !l.trim().is_empty() {
                        text.push(' ');
                        text.push_str(l.trim());
                    }
                }
                defs.push((label.to_string(), text));
                // Remove the whole definition (and the blank line after it).
                edits.push((r.start, r.end, String::new()));
                consumed = r.end;
            }
            Event::Start(Tag::BlockQuote(Some(kind))) => {
                let raw = &neutral[r.clone()];
                let line = &raw[..first_line_end(raw)];
                if let Some(a) = line.find("[!")
                    && let Some(b) = line[a..].find(']')
                {
                    let prefix = line[..a].trim_end();
                    edits.push((
                        r.start + a,
                        r.start + a + b + 1,
                        format!("{}\n{prefix}", alert_label(kind)),
                    ));
                }
            }
            Event::DisplayMath(t) => {
                let raw = &neutral[r.clone()];
                let at_line_start = r.start == 0 || neutral.as_bytes()[r.start - 1] == b'\n';
                let at_line_end = neutral[r.end..].starts_with('\n') || r.end == neutral.len();
                if raw.starts_with("$$") && at_line_start && at_line_end {
                    edits.push((r.start, r.end, format!("```latex\n{}\n```", t.trim())));
                }
            }
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) if info.trim() == "math" => {
                let raw = &neutral[r.clone()];
                let line = &raw[..first_line_end(raw)];
                if let Some(i) = line.rfind("math") {
                    edits.push((r.start + i, r.start + i + 4, "latex".to_owned()));
                }
            }
            Event::Start(Tag::HtmlBlock) => {
                let converted = html.convert(&neutral[r.clone()]);
                let mut c = converted.trim_matches('\n').to_owned();
                while c.contains("\n\n\n") {
                    c = c.replace("\n\n\n", "\n\n");
                }
                // The converted block must not fuse with its neighbours (`---` after a
                // paragraph is a heading underline; a list swallows the next line).
                let before_blank = r.start == 0 || neutral[..r.start].ends_with("\n\n");
                let after_blank = r.end >= neutral.len() || neutral[r.end..].starts_with('\n');
                let c = format!(
                    "{}{c}\n{}",
                    if before_blank { "" } else { "\n" },
                    if after_blank { "" } else { "\n" }
                );
                edits.push((r.start, r.end, c));
                consumed = r.end;
            }
            Event::InlineHtml(_) => {
                let c = html.convert(&neutral[r.clone()]);
                edits.push((r.start, r.end, c));
            }
            _ => {}
        }
    }
    if edits.is_empty() && refs.is_empty() {
        return neutral.to_owned();
    }
    edits.sort_by_key(|e| (e.0, e.1));
    let mut out = String::with_capacity(neutral.len() + 64);
    let mut at = 0;
    for (a, b, rep) in edits {
        if a < at {
            continue;
        }
        out.push_str(&neutral[at..a]);
        out.push_str(&rep);
        at = b;
    }
    out.push_str(&neutral[at..]);
    if !refs.is_empty() {
        let out_trim = out.trim_end().len();
        out.truncate(out_trim);
        out.push_str("\n\n**Notes**\n\n");
        for (i, label) in refs.iter().enumerate() {
            let text = defs
                .iter()
                .find(|(l, _)| l == label)
                .map_or("", |(_, t)| t.as_str());
            out.push_str(&format!("{}. {}\n", i + 1, text));
        }
        out.truncate(out.trim_end().len());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::convert;

    #[test]
    fn native_constructs_pass_through_byte_for_byte() {
        for s in [
            "# H\n\n**b** _i_ ~~s~~ `c` and a [link](https://x.example) <https://y.example>\n",
            "> quote\n>\n> - nested\n>   - deeper\n\n1. one\n2. two\n\n---\n",
            "| a | b |\n|:--|--:|\n| **x** | `y` |\n",
            "```rust\nfn main() {}\n```\n\n    indented code\n",
            "- [ ] open\n- [x] done\n",
            ":tada: ok  \nhard break\n",
            "inline $x^2$ stays, mermaid:\n\n```mermaid\ngraph TD\n  A-->B\n```\n",
            "mention <@user:U1> and <@here> and <#space:C1> stay for the token pass\n",
        ] {
            assert_eq!(convert(s), s, "{s}");
        }
    }

    #[test]
    fn footnotes_become_numbers_and_a_notes_list() {
        let out = convert(
            "One[^a] and two[^b] and one again[^a].\n\n[^a]: First note.\n[^b]: Second\n    continues.\n",
        );
        assert_eq!(
            out,
            "One[1] and two[2] and one again[1].\n\n**Notes**\n\n1. First note.\n2. Second continues."
        );
    }

    #[test]
    fn alerts_become_labelled_quotes() {
        for (kind, label) in [
            ("NOTE", "ℹ️ **Note**"),
            ("TIP", "💡 **Tip**"),
            ("IMPORTANT", "❗ **Important**"),
            ("WARNING", "⚠️ **Warning**"),
            ("CAUTION", "🛑 **Caution**"),
        ] {
            assert_eq!(
                convert(&format!("> [!{kind}]\n> body text\n")),
                format!("> {label}\n>\n> body text\n"),
                "{kind}"
            );
        }
        // An ordinary quote is not an alert.
        assert_eq!(convert("> [!OTHER]\n> x\n"), "> [!OTHER]\n> x\n");
    }

    #[test]
    fn math_blocks_become_latex_fences_and_inline_math_stays() {
        assert_eq!(convert("$$\nE = mc^2\n$$\n"), "```latex\nE = mc^2\n```\n");
        assert_eq!(convert("```math\nx^2\n```\n"), "```latex\nx^2\n```\n");
        assert_eq!(convert("so $a+b$ holds\n"), "so $a+b$ holds\n");
    }

    #[test]
    fn allowlisted_html_becomes_markdown() {
        assert_eq!(
            convert(
                "a <b>bold</b> <i>it</i> <s>gone</s> <kbd>Ctrl</kbd> <em>e</em> <sub>2</sub> x<br>y\n"
            ),
            "a **bold** _it_ ~~gone~~ `Ctrl` _e_ 2 x  \ny\n"
        );
        assert_eq!(
            convert("see <a href=\"https://x.example\">docs</a> now\n"),
            "see [docs](https://x.example) now\n"
        );
        assert_eq!(convert("<a>plain</a>\n"), "plain\n");
        let d = convert("<details>\n<summary>More</summary>\n\nhidden *text*\n\n</details>\n");
        assert!(
            d.contains("**More**") && d.contains("hidden *text*") && !d.contains('<'),
            "{d}"
        );
        assert_eq!(
            convert("before\n\n<hr>\n\nafter\n").trim(),
            "before\n\n---\n\nafter"
        );
        let l = convert("<ul>\n<li>one</li>\n<li>two</li>\n</ul>\n");
        assert_eq!(l.trim(), "- one\n- two");
        let l = convert("<ol start=\"3\">\n<li>c</li>\n<li>d</li>\n</ol>\n");
        assert_eq!(l.trim(), "3. c\n4. d");
        let t = convert(
            "<table>\n<tr><th>A</th><th>B</th></tr>\n<tr><td>1</td><td>x|y</td></tr>\n</table>\n",
        );
        assert_eq!(t.trim(), "| A | B | \n|---|---|\n| 1 | x\\|y |");
    }

    #[test]
    fn code_is_never_converted() {
        let s = "`<b>x</b>` and\n\n```\n<b>x</b>[^1]\n$$\n```\n";
        assert_eq!(convert(s), s);
    }
}
