//! Neutral Markdown → Mattermost Markdown: the conversion table of `docs/20 §Rendering`
//! (FIX-142 step 4, D-227(c)).
//!
//! Native constructs pass through **byte for byte**: this is a pass over the pulldown-cmark
//! offset iterator that edits only the byte ranges of constructs Mattermost lacks — footnotes
//! (`[1]` + a Notes list), alerts (a bold-labelled quote), display math and ```` ```math ````
//! (```` ```latex ````) and the raw HTML the core lets through (to Markdown). Nothing is
//! dropped. Code spans, fenced blocks and the mention tokens are never touched.

use pulldown_cmark::{BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag};
use std::ops::Range;

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
    /// Emphasis markers whose opening tag was converted (so its closing tag is too).
    open: Vec<String>,
    lists: Vec<Option<u32>>,
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

/// Characters that start Markdown in text that is not Markdown (the text of an HTML block).
fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        if matches!(
            c,
            '\\' | '`' | '*' | '_' | '[' | ']' | '(' | ')' | '<' | '>' | '!' | '|' | '~' | '$'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Break every `![` of Markdown that is carried over as it is (a footnote's text): what the core
/// refused to let form must not form here either.
fn break_image_openers(s: &str) -> String {
    s.replace("![", "!\\[")
}

/// A code span that holds `inner` literally: the delimiter is longer than any backtick run in it.
fn code_span(inner: &str) -> String {
    let run = inner
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    let d = "`".repeat(run + 1);
    let pad = if inner.starts_with('`') || inner.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{d}{pad}{inner}{pad}{d}")
}

/// A fenced block that holds `inner` literally: the fence is longer than any backtick run in it.
fn code_fence(inner: &str) -> String {
    let run = inner
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    let f = "`".repeat((run + 1).max(3));
    format!("\n\n{f}\n{}\n{f}\n\n", inner.trim_matches('\n'))
}

/// Link destinations that cannot end the destination early.
fn dest(href: &str) -> String {
    href.replace('(', "%28").replace(')', "%29")
}

/// `text` as a link label: brackets cannot close it (nor open an image after a `!`).
fn escape_label(s: &str) -> String {
    s.replace('[', "\\[").replace(']', "\\]")
}

/// The name of a tag (`<B class=x>` → `b`) and whether it is a closing tag.
fn tag_name(raw: &str) -> (bool, String) {
    let inner = raw.trim_start_matches('<');
    let (closing, inner) = inner
        .strip_prefix('/')
        .map_or((false, inner), |r| (true, r));
    let name = inner
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    (closing, name)
}

/// The marker an inline emphasis tag converts to.
fn mark(name: &str) -> Option<&'static str> {
    match name {
        "b" | "strong" => Some("**"),
        "i" | "em" => Some("_"),
        "s" | "del" => Some("~~"),
        _ => None,
    }
}

impl Html {
    fn tag(&mut self, raw: &str, rest: &str, out: &mut String) {
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
            // Emphasis only when the tag is paired: an unpaired marker would reach out into
            // the Markdown around it.
            (n, false) if mark(n).is_some() => {
                let closes = rest.to_ascii_lowercase().contains(&format!("</{n}"));
                if closes && let Some(m) = mark(n) {
                    out.push_str(m);
                    self.open.push(n.to_owned());
                }
            }
            (n, true) if mark(n).is_some() => {
                if let Some(at) = self.open.iter().rposition(|o| o == n) {
                    self.open.remove(at);
                    out.push_str(mark(n).unwrap_or_default());
                }
            }
            ("br", _) => out.push_str("  \n"),
            ("hr", _) => out.push_str("\n\n---\n\n"),
            ("a", false) => {
                // The core keeps an `href` only for http, https and mailto; without one the
                // label stays and the tag goes. Only a paired tag becomes a link.
                if rest.to_ascii_lowercase().contains("</a") {
                    let href = attr(inner, "href");
                    if href.is_some() {
                        out.push('[');
                    }
                    self.hrefs.push(href);
                }
            }
            ("a", true) => {
                if let Some(Some(h)) = self.hrefs.pop() {
                    out.push_str(&format!("]({})", dest(&h)));
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
        // The text of an HTML block is not Markdown: it is shown as typed.
        let t = escape_text(t);
        if self.in_cell {
            out.push_str(&t.replace('\n', " "));
        } else {
            out.push_str(&t);
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
                    let raw = &tail[..=e];
                    let after = &tail[e + 1..];
                    let (closing, name) = tag_name(raw);
                    // Code is literal: `<code>…</code>`, `<kbd>…</kbd>` and `<pre>…</pre>` hold their
                    // text as typed (tags inside are dropped); an unpaired opener is dropped.
                    if !closing && matches!(name.as_str(), "code" | "kbd" | "pre") {
                        let lower = after.to_ascii_lowercase();
                        match lower.find(&format!("</{name}")) {
                            Some(c) => {
                                let literal = strip_tags(&after[..c]);
                                if name == "pre" {
                                    out.push_str(&code_fence(&literal));
                                } else {
                                    out.push_str(&code_span(&literal));
                                }
                                rest = after[c..].find('>').map_or("", |g| &after[c + g + 1..]);
                            }
                            None => rest = after,
                        }
                        continue;
                    }
                    // A stray closing tag of code is dropped.
                    if closing && matches!(name.as_str(), "code" | "kbd" | "pre") {
                        rest = after;
                        continue;
                    }
                    self.tag(raw, after, &mut out);
                    rest = after;
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

/// The last-line check of `render_text`: no image, no link outside `http`/`https`/`mailto`, and
/// no image opener (`![`, which Mattermost's own dialect — `![x](url =10x10)`, a definition
/// that interrupts a paragraph — can complete) outside code.
pub fn is_safe(text: &str) -> bool {
    use pulldown_cmark::{LinkType, TagEnd};
    let mut in_code = false;
    for (e, r) in Parser::new_ext(text, options()).into_offset_iter() {
        match e {
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::End(TagEnd::CodeBlock) => in_code = false,
            Event::Start(Tag::Image { .. }) => return false,
            Event::Start(Tag::Link {
                dest_url,
                link_type,
                ..
            }) if !matches!(link_type, LinkType::Email) => {
                let l = dest_url.to_ascii_lowercase();
                if !(l.starts_with("http:") || l.starts_with("https:") || l.starts_with("mailto:"))
                {
                    return false;
                }
            }
            Event::Text(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
                if !in_code && text[r.clone()].contains("![") =>
            {
                return false;
            }
            _ => {}
        }
    }
    true
}

/// The index of the closing tag that pairs with the opening tag `name` at `evs[i]`, within the
/// same inline run (a paragraph, a cell): nesting of the same name counts.
fn find_close(evs: &[(Event<'_>, Range<usize>)], i: usize, name: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut nest = 0usize;
    for (k, (e, _)) in evs.iter().enumerate().skip(i + 1) {
        match e {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            _ => {
                if depth == 0
                    && let Event::InlineHtml(h) = e
                    && let (closing, n) = tag_name(h)
                    && n == name
                {
                    if !closing {
                        nest += 1;
                    } else if nest == 0 {
                        return Some(k);
                    } else {
                        nest -= 1;
                    }
                }
            }
        }
    }
    None
}

/// `s` without its tags (the text of a stretch of HTML).
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
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
    // Closing tags of converted emphasis/link tags, by source offset → what they become.
    let mut closers: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    let evs: Vec<(Event<'_>, Range<usize>)> = Parser::new_ext(neutral, options())
        .into_offset_iter()
        .collect();
    for (i, (ev, r)) in evs.iter().enumerate() {
        let (ev, r) = (ev.clone(), r.clone());
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
                // The text is flattened into a list item: nothing in it may open an image.
                defs.push((label.to_string(), break_image_openers(&text)));
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
                let raw = &neutral[r.clone()];
                let (closing, name) = tag_name(raw);
                if closing {
                    // A closer that belongs to an opener we converted is handled with it
                    // (`closers`); a stray one is dropped.
                    let rep = closers.remove(&r.start).unwrap_or_default();
                    edits.push((r.start, r.end, rep));
                } else if matches!(name.as_str(), "code" | "kbd") {
                    // Code is literal: the pair becomes one code span around the source text
                    // between the tags, however many backticks it holds. Unpaired: dropped.
                    match find_close(&evs, i, &name) {
                        Some(k) => {
                            let end = evs[k].1.end;
                            let inner = strip_tags(&neutral[r.end..evs[k].1.start]);
                            edits.push((r.start, end, code_span(&inner)));
                            consumed = end;
                        }
                        None => edits.push((r.start, r.end, String::new())),
                    }
                } else if name == "a" {
                    match (find_close(&evs, i, &name), attr(raw, "href")) {
                        (Some(k), Some(href)) => {
                            let end = evs[k].1.end;
                            let inner = &neutral[r.end..evs[k].1.start];
                            // After a `!` a link would be an image: keep them apart.
                            let apart = if neutral[..r.start].ends_with('!') {
                                "\u{200B}"
                            } else {
                                ""
                            };
                            edits.push((
                                r.start,
                                end,
                                format!(
                                    "{apart}[{}]({})",
                                    escape_label(&convert(inner)),
                                    dest(&href)
                                ),
                            ));
                            consumed = end;
                        }
                        (Some(k), None) => {
                            // No usable href: the label stays, the tags go.
                            closers.insert(evs[k].1.start, String::new());
                            edits.push((r.start, r.end, String::new()));
                        }
                        (None, _) => edits.push((r.start, r.end, String::new())),
                    }
                } else if let Some(m) = mark(&name) {
                    match find_close(&evs, i, &name) {
                        Some(k) => {
                            closers.insert(evs[k].1.start, m.to_owned());
                            edits.push((r.start, r.end, m.to_owned()));
                        }
                        None => edits.push((r.start, r.end, String::new())),
                    }
                } else {
                    let c = html.convert(raw);
                    edits.push((r.start, r.end, c));
                }
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

    /// FIX-142 review: the converter alone (the last-line check not involved) must not turn text
    /// the core let through into an image or a bad link.
    #[test]
    fn conversion_never_manufactures_an_image_or_a_bad_link() {
        let s = "https://evil.example/?d=S";
        for src in [
            format!("!<a href=\"{s}\">x</a>"),
            format!("<div>\n![x]({s})\n</div>"),
            format!("<span>![x]({s})</span>"),
            format!("x<code>`![a]({s})`"),
            format!("<pre>\n```\n![x]({s})\n</pre>"),
            "<a href=\"https://ok.example\">x](javascript:alert(1))</a>".to_owned(),
            format!("see[^a]\n\n[^a]: note\n\n        ![x]({s})"),
            format!("<b>![x]({s})"),
            format!("<a href=\"{s}\">![x]({s})</a>"),
        ] {
            let neutral = aurora_core::sanitize::markdown_safe(&src, 10_000);
            let out = convert(&neutral);
            assert!(super::is_safe(&out), "{src:?} -> {neutral:?} -> {out:?}");
        }
    }

    #[test]
    fn html_pairs_convert_and_unpaired_tags_are_dropped() {
        assert_eq!(convert("a <code>x</code> b\n"), "a `x` b\n");
        assert_eq!(convert("a <code>x`y</code> b\n"), "a ``x`y`` b\n");
        assert_eq!(convert("a <code>x b\n"), "a x b\n");
        assert_eq!(convert("a <b>x b\n"), "a x b\n");
        assert_eq!(convert("a </b>x b\n"), "a x b\n");
        assert_eq!(convert("a <b>x</b> <i>y</i>\n"), "a **x** _y_\n");
        assert_eq!(
            convert("<a href=\"https://x.example/a(b)\">[l]</a>\n"),
            "[\\[l\\]](https://x.example/a%28b%29)\n"
        );
        assert_eq!(
            convert("!<a href=\"https://x.example\">x</a>\n"),
            "!\u{200B}[x](https://x.example)\n"
        );
    }

    #[test]
    fn the_text_of_an_html_block_is_shown_as_typed() {
        let out = convert("<div>*not emphasis* and [not](a link)</div>\n");
        assert!(out.contains("\\*not emphasis\\*"), "{out}");
        assert!(
            !out.contains("](a link)") || out.contains("\\]\\(a link\\)"),
            "{out}"
        );
    }

    #[test]
    fn code_is_never_converted() {
        let s = "`<b>x</b>` and\n\n```\n<b>x</b>[^1]\n$$\n```\n";
        assert_eq!(convert(s), s);
    }
}
