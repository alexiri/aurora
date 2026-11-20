//! `markdown_safe` (FIX-142, D-226, D-227; `13 §4.3`, `02 §Untrusted text`).
//!
//! An agent's own message body on a chat platform keeps its Markdown. The text is copied
//! through **byte for byte** and only the byte ranges of D-227's four security exceptions are
//! rewritten (never re-emitted from a parse tree):
//!
//! 1. images are never inline: `![alt](url)` → `[🖼 alt (host)](url)` (empty alt → `image`);
//! 2. link schemes: only `http`, `https` and `mailto` stay links, any other becomes its label
//!    as text; a link whose label looks like a URL or domain of another host gets ` (→ host)`;
//! 3. raw HTML outside the allowlist (`13 §4.3`) is removed, keeping its text (attributes other than
//!    the harmless ones are dropped from allowlisted tags);
//! 4. typed neutral tokens (`<@user:…>`, `<@bot:…>`, `<#space:…>`, `<@here>`) never form.
//!
//! The result is verified by re-parsing; a block that fails falls back to
//! [`markdown_as_data`](super::markdown_as_data) and bumps [`markdown_fallbacks`].

use super::{clamp, is_stripped, markdown_as_data};
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

static FALLBACKS: AtomicU64 = AtomicU64::new(0);

/// Blocks (or whole messages) that failed verification and fell back to `markdown_as_data`
/// since the process started. Normally zero; a pathological input is the only way up.
pub fn markdown_fallbacks() -> u64 {
    FALLBACKS.load(Ordering::Relaxed)
}

/// The Markdown dialect of `13 §4.3`: CommonMark 0.31 + GFM + the additions listed there.
pub(crate) fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM
}

/// The raw-HTML allowlist of `13 §4.3` (`docs/tasks/FIX-137.md` item 3), plus `a` under the link rule.
const HTML_ALLOWED: [&str; 41] = [
    "b",
    "i",
    "em",
    "strong",
    "s",
    "del",
    "ins",
    "sub",
    "sup",
    "kbd",
    "mark",
    "small",
    "br",
    "hr",
    "p",
    "div",
    "span",
    "details",
    "summary",
    "blockquote",
    "code",
    "pre",
    "ul",
    "ol",
    "li",
    "table",
    "thead",
    "tbody",
    "tr",
    "th",
    "td",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "a",
    "tfoot",
    "caption",
    "dl",
];

/// Remove typed C0/C1/bidi/format characters but keep the author's layout: spaces, tabs and
/// newlines (indentation is Markdown syntax). Line endings are normalised to `\n`; leading
/// blank lines go; the result is clamped to `max` characters.
fn clean_layout(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(16_384));
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\r' => {
                if it.peek() == Some(&'\n') {
                    it.next();
                }
                out.push('\n');
            }
            '\u{85}' | '\u{2028}' | '\u{2029}' => out.push('\n'),
            c if is_stripped(c) => {}
            c => out.push(c),
        }
    }
    // Leading blank lines carry nothing; everything else, trailing newlines and runs of blank
    // lines included, stays (they are content inside code blocks).
    clamp(without_leading_blank_lines(&out).to_owned(), max)
}

fn without_leading_blank_lines(s: &str) -> &str {
    let start = s
        .split_inclusive('\n')
        .take_while(|l| l.trim().is_empty())
        .map(str::len)
        .sum::<usize>();
    &s[start..]
}

fn escape_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        if matches!(
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
                | '!'
                | '|'
                | '~'
                | '&'
                | '$'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Break every `![` (an image opener) of prose: `!\[`. Nothing after the sanitizer may form what
/// the sanitizer refuses — a connector that converts HTML or math to Markdown, or a platform
/// whose dialect reads more as an image than CommonMark does (Mattermost's `=WxH` size syntax,
/// a definition interrupting a paragraph), must find no opener to complete.
fn break_image_openers(s: &str) -> String {
    s.replace("![", "!\\[")
}

/// Math is opaque to the Markdown parser but not to a platform with LaTeX off, where `$…$` is
/// prose: no image opener and no `](` link joint survives in it. A space does it without
/// changing the math (LaTeX ignores it; a backslash would be a LaTeX error on a chat platform with
/// inline LaTeX on — checked on the pinned dev server, `docs/20`, FIX-142 step 4).
fn tame_math(s: &str) -> String {
    let s = s.replace("![", "! [").replace("](", "] (");
    // A URL in math cannot be a link, and a server that reads raw text would fetch it (D-270):
    // a space after the colon (LaTeX ignores it) makes it none.
    break_schemes(&s, " ")
}

fn scheme_of(url: &str) -> Option<&str> {
    let i = url.find(':')?;
    let s = &url[..i];
    let mut ch = s.chars();
    (ch.next()?.is_ascii_alphabetic()
        && ch.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')))
    .then_some(s)
}

/// Only `http`, `https` and `mailto` (case-insensitive) are links.
pub(crate) fn allowed_url(url: &str) -> bool {
    scheme_of(url).is_some_and(|s| {
        ["http", "https", "mailto"]
            .iter()
            .any(|a| a.eq_ignore_ascii_case(s))
    })
}

fn host_of(url: &str) -> Option<String> {
    let l = url.to_ascii_lowercase();
    if let Some(r) = l.strip_prefix("mailto:") {
        let d = r.split_once('@')?.1;
        let d = d.split(['?', ',', ' ']).next()?;
        return (!d.is_empty()).then(|| d.to_owned());
    }
    // WHATWG rules, as a browser applies them: `http:evil.example/x` and `https://evil.example\@cern.ch`
    // both go to evil.example.
    let u = url::Url::parse(url.trim()).ok()?;
    u.host_str()
        .filter(|h| !h.is_empty())
        .map(|h| h.to_ascii_lowercase())
}

/// The host a label claims when it looks like a URL or a domain (`https://cern.ch/x`, `cern.ch`).
fn label_host(label: &str) -> Option<String> {
    let t = label.trim();
    if t.is_empty() || t.chars().any(char::is_whitespace) {
        return None;
    }
    let l = t.to_ascii_lowercase();
    let l = l
        .strip_prefix("https://")
        .or_else(|| l.strip_prefix("http://"))
        .unwrap_or(&l);
    let host = l.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?;
    let host = host.split(':').next()?;
    // Closing punctuation after the name is still the name (`https://cern.ch]`, `cern.ch.`).
    let host = host.trim_end_matches(['.', ',', ';', '!', '?', '\'', '"', ')', ']']);
    let tld = host.rsplit_once('.')?.1;
    (host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        && tld.len() >= 2
        && tld.chars().all(|c| c.is_ascii_alphabetic())
        && !host.starts_with('.')
        && !host.contains(".."))
    .then(|| host.to_owned())
}

fn bare(h: &str) -> &str {
    h.strip_prefix("www.").unwrap_or(h)
}

/// The host a link really leads to when its `label` looks like a URL or domain of another host
/// (D-227(2)); `None` for an honest or an ordinary label. The destination is read by WHATWG
/// rules, spaces and backslashes included. Also used by connectors to re-check their output.
pub fn link_host_lie(label: &str, dest: &str) -> Option<String> {
    let claimed = label_host(label)?;
    let real = host_of(dest)?;
    (bare(&claimed) != bare(&real)).then_some(real)
}

/// ` (→ host)` for a link whose label claims another host than the destination's.
fn deceptive_suffix(label: &str, dest: &str) -> Option<String> {
    link_host_lie(label, dest).map(|real| format!(" (→ {})", escape_inline(&real)))
}

/// Whether `url` may be a link target at all: `http`, `https` or `mailto` (D-227(2)).
pub fn allowed_link_scheme(url: &str) -> bool {
    allowed_url(url)
}

/// The destination of an inline link as it is written in the source (`[l](dest "t")`), before
/// Markdown unescapes it: a platform that keeps it as typed sends the browser there.
fn raw_dest(link_src: &str) -> Option<String> {
    let at = link_src.rfind("](")?;
    let rest = link_src[at + 2..].trim_start();
    if let Some(r) = rest.strip_prefix('<') {
        return Some(r[..r.find('>')?].to_owned());
    }
    let mut depth = 0i32;
    let mut end = rest.len();
    for (i, c) in rest.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => {
                end = i;
                break;
            }
            ')' => depth -= 1,
            c if c.is_whitespace() => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    Some(rest[..end].to_owned())
}

/// A link destination that survives any content: `<…>` form, the characters that would end it
/// percent-encoded.
fn dest_form(url: &str) -> String {
    let mut out = String::with_capacity(url.len() + 2);
    out.push('<');
    for c in url.chars() {
        match c {
            '\\' => out.push_str("%5C"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '\n' | '\r' => out.push_str("%0A"),
            c => out.push(c),
        }
    }
    out.push('>');
    out
}

/// The plain text of the events nested in the container at `evs[i]`.
fn plain_text(evs: &[(Event<'_>, Range<usize>)], i: usize) -> String {
    let end = evs[i].1.end;
    let mut out = String::new();
    for (e, r) in &evs[i + 1..] {
        if r.start >= end {
            break;
        }
        match e {
            Event::Text(t) | Event::Code(t) | Event::InlineMath(t) | Event::DisplayMath(t) => {
                out.push_str(t)
            }
            Event::SoftBreak | Event::HardBreak => out.push(' '),
            _ => {}
        }
    }
    out
}

fn tidy_alt(alt: &str) -> String {
    let one: String = alt.split_whitespace().collect::<Vec<_>>().join(" ");
    let one = if one.is_empty() {
        "image".to_owned()
    } else {
        one
    };
    let clipped: String = one.chars().take(200).collect();
    escape_inline(&clipped)
}

/// Attribute `(name, value)` pairs of a tag body (after the name), quote-aware.
fn parse_attrs(mut s: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    loop {
        s = s.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        if s.is_empty() {
            break;
        }
        let name_end = s
            .find(|c: char| c.is_whitespace() || matches!(c, '=' | '/' | '>'))
            .unwrap_or(s.len());
        let name = s[..name_end].to_ascii_lowercase();
        s = s[name_end..].trim_start();
        let mut val = None;
        if let Some(r) = s.strip_prefix('=') {
            let r = r.trim_start();
            let (v, rest) = match r.chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let inner = &r[1..];
                    inner
                        .find(q)
                        .map_or((inner, ""), |e| (&inner[..e], &inner[e + 1..]))
                }
                _ => {
                    let e = r
                        .find(|c: char| c.is_whitespace() || c == '>')
                        .unwrap_or(r.len());
                    (&r[..e], &r[e..])
                }
            };
            val = Some(v.to_owned());
            s = rest;
        }
        if !name.is_empty() {
            out.push((name, val));
        }
    }
    out
}

/// The end index (inclusive) of the tag starting at `s[0] == '<'`, honouring quotes.
fn tag_end(s: &str) -> Option<usize> {
    let mut q: Option<char> = None;
    for (i, c) in s.char_indices().skip(1) {
        match (q, c) {
            (Some(x), c) if c == x => q = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => q = Some(c),
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

fn safe_token(v: &str, max: usize) -> bool {
    !v.is_empty() && v.len() <= max && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn safe_href(v: &str) -> bool {
    allowed_url(v)
        && !v.chars().any(|c| {
            c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '<' | '>' | '\\' | '`')
        })
}

/// Rebuild an allowlisted tag with only its harmless attributes. `None` = the tag is not
/// allowlisted (dropped). The second value is the kept `href`, for the deceptive-label check.
fn clean_tag(tag: &str) -> Option<(String, Option<String>)> {
    let inner = tag[1..tag.len() - 1].trim_end_matches('/');
    let (closing, inner) = inner
        .strip_prefix('/')
        .map_or((false, inner), |r| (true, r));
    let name_end = inner
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(inner.len());
    let name = inner[..name_end].to_ascii_lowercase();
    if !HTML_ALLOWED.contains(&name.as_str()) {
        return None;
    }
    let attrs = parse_attrs(&inner[name_end..]);
    if attrs.is_empty() || closing {
        // Nothing to drop: an allowlisted tag without attributes passes verbatim.
        let verbatim = if closing {
            format!("</{name}>")
        } else {
            tag.to_owned()
        };
        return Some((verbatim, None));
    }
    let mut out = format!("<{name}");
    let mut href = None;
    for (an, av) in attrs {
        let v = av.as_deref();
        let keep = match (name.as_str(), an.as_str()) {
            ("details", "open") => Some(None),
            (_, "align") => v
                .filter(|x| {
                    ["left", "right", "center", "justify"]
                        .contains(&x.to_ascii_lowercase().as_str())
                })
                .map(|x| Some(x.to_ascii_lowercase())),
            ("td" | "th", "colspan" | "rowspan") => v
                .filter(|x| x.len() <= 4 && x.chars().all(|c| c.is_ascii_digit()))
                .map(|x| Some(x.to_owned())),
            ("ol", "start") => v
                .filter(|x| x.len() <= 6 && x.chars().all(|c| c.is_ascii_digit()))
                .map(|x| Some(x.to_owned())),
            ("ol" | "ul" | "li", "type") => {
                v.filter(|x| safe_token(x, 8)).map(|x| Some(x.to_owned()))
            }
            ("a", "href") => v.filter(|x| safe_href(x)).map(|x| {
                href = Some(x.to_owned());
                Some(x.to_owned())
            }),
            _ => None,
        };
        match keep {
            Some(Some(val)) => {
                out.push_str(&format!(" {an}=\"{val}\""));
            }
            Some(None) => {
                out.push(' ');
                out.push_str(&an);
            }
            None => {}
        }
    }
    out.push('>');
    Some((out, href))
}

/// Strip the markup of `s` down to its text (for a raw `<a>` label).
fn text_of(s: &str) -> String {
    let mut out = String::new();
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

/// Whether an `<a>` just opened is closed in `rest` by a `</a>` that comes before the next `<a`
/// (a nested or repeated opener would take that closer, leaving this one unpaired).
pub fn anchor_is_closed(rest: &str) -> bool {
    let low = rest.to_ascii_lowercase();
    let Some(close) = low.find("</a") else {
        return false;
    };
    let open = [
        low.find("<a "),
        low.find("<a>"),
        low.find("<a\t"),
        low.find("<a\n"),
    ]
    .into_iter()
    .flatten()
    .min();
    open.is_none_or(|o| close < o)
}

/// The text between the tags of an HTML block: image openers broken and, outside an `<a>`, bare
/// URLs made links (D-270) — as `<a href>` where the URL fits an attribute, else the colon is an
/// entity (`https&#58;//…`), which a platform's URL matcher does not read.
fn html_text(piece: &str, in_a: bool) -> String {
    let piece = break_image_openers(piece);
    if in_a {
        return piece;
    }
    let mut out = String::with_capacity(piece.len());
    let mut at = 0;
    for (u, kind) in bare_url_ranges(&piece) {
        out.push_str(&piece[at..u.start]);
        let url = &piece[u.clone()];
        let href = if kind == UrlKind::Www {
            format!("https://{url}")
        } else {
            url.to_owned()
        };
        if kind != UrlKind::Ftp && safe_href(&href) && !url.contains('&') {
            out.push_str(&format!("<a href=\"{href}\">{url}</a>"));
        } else if kind == UrlKind::Www {
            // (no colon to turn into an entity: the dot after `www` is the break)
            out.push_str(&url[..3]);
            out.push_str("&#46;");
            out.push_str(&url[4..]);
        } else {
            let colon = url.find(':').unwrap_or(0);
            out.push_str(&url[..colon]);
            out.push_str("&#58;");
            out.push_str(&url[colon + 1..]);
        }
        at = u.end;
    }
    out.push_str(&piece[at..]);
    out
}

/// Sanitize raw HTML text (an HTML block, or one inline tag): comments, doctype/CDATA/PI and
/// tags outside the allowlist are removed (their text stays), allowlisted tags lose every
/// attribute but the harmless ones, `<img>` becomes `🖼 alt`.
fn sanitize_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut open_a: Option<(usize, String)> = None;
    let mut in_a = false;
    while i < s.len() {
        let rest = &s[i..];
        let Some(p) = rest.find('<') else {
            out.push_str(&html_text(rest, in_a));
            break;
        };
        out.push_str(&html_text(&rest[..p], in_a));
        i += p;
        let rest = &s[i..];
        let next = rest[1..].chars().next();
        if rest.starts_with("<!--") {
            i = rest.find("-->").map_or(s.len(), |e| i + e + 3);
            continue;
        }
        if !next.is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?')) {
            out.push('<');
            i += 1;
            continue;
        }
        let Some(end) = tag_end(rest) else {
            // An unterminated tag: not markup any more, keep nothing of it.
            i = s.len();
            continue;
        };
        let tag = &rest[..=end];
        i += end + 1;
        if tag.starts_with("<!") || tag.starts_with("<?") {
            continue;
        }
        let inner = tag[1..tag.len() - 1].trim_start_matches('/');
        if inner
            .get(..3)
            .is_some_and(|n| n.eq_ignore_ascii_case("img"))
            && !inner[3..].starts_with(|c: char| c.is_ascii_alphanumeric())
        {
            let alt = parse_attrs(&inner[3..])
                .into_iter()
                .find(|(n, _)| n == "alt")
                .and_then(|(_, v)| v)
                .unwrap_or_default();
            out.push_str(&format!("🖼 {}", tidy_alt(&alt)));
            continue;
        }
        let Some((clean, href)) = clean_tag(tag) else {
            continue;
        };
        if clean == "</a>" {
            in_a = false;
            out.push_str(&clean);
            if let Some((at, dest)) = open_a.take()
                && let Some(suf) = deceptive_suffix(&text_of(&out[at..]), &dest)
                && !s[i..].starts_with(&suf)
            {
                out.push_str(&suf);
            }
            continue;
        }
        if clean == "<a>" || clean.starts_with("<a ") {
            // Text is inside an anchor only when the anchor is closed later: an unpaired `<a>`
            // is dropped by a connector and its text would be bare.
            in_a = anchor_is_closed(&s[i..]);
        }
        if let Some(h) = href {
            open_a = Some((out.len() + clean.len(), h));
        }
        out.push_str(&clean);
    }
    out
}

/// The href of a (clean) `<a …>` open tag.
fn href_of(tag: &str) -> Option<String> {
    let inner = tag.strip_prefix('<')?.strip_suffix('>')?;
    let rest = inner
        .strip_prefix('a')
        .filter(|r| r.starts_with(char::is_whitespace))?;
    parse_attrs(rest)
        .into_iter()
        .find(|(n, _)| n == "href")
        .and_then(|(_, v)| v)
}

/// What kind of URL a platform's preview extracts (measured on Mattermost 11.7.15: `http://`,
/// `https://`, `ftp://` and `www.` — an embed is made for each; `docs/20 §Rendering`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum UrlKind {
    /// `http://…`, `https://…`
    Web,
    /// `www.…` (the server prepends `http://`)
    Www,
    /// `ftp://…` — not a link scheme (rule 2), but the server previews it
    Ftp,
}

/// Whether `www.` at byte `i` of `raw` starts a URL: a name character follows and nothing of a
/// name, a path or an address precedes it (`awww.x`, `foo.www.x`, `x@www.y`, `…/www.x` are not).
fn www_at(raw: &str, i: usize) -> bool {
    raw[i..]
        .get(..4)
        .is_some_and(|s| s.eq_ignore_ascii_case("www."))
        && raw[i + 4..].starts_with(|c: char| c.is_ascii_alphanumeric())
        && !raw[..i].chars().next_back().is_some_and(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | '/' | ':')
        })
}

/// The URL kind starting at byte `i` of `raw` and the length of its scheme/prefix, if any.
fn url_start(raw: &str, i: usize) -> Option<(UrlKind, usize)> {
    let rest = raw.get(i..)?;
    let starts = |p: &str| {
        rest.get(..p.len())
            .is_some_and(|s| s.eq_ignore_ascii_case(p))
    };
    if starts("https://") {
        Some((UrlKind::Web, 8))
    } else if starts("http://") {
        Some((UrlKind::Web, 7))
    } else if starts("ftp://") {
        Some((UrlKind::Ftp, 6))
    } else if www_at(raw, i) {
        Some((UrlKind::Www, 4))
    } else {
        None
    }
}

/// Byte ranges of the bare URLs in `raw` (a run of prose) with their kind: to the next whitespace,
/// `<` or `>`, without the trailing punctuation and unbalanced closing brackets GFM leaves out.
fn bare_url_ranges(raw: &str) -> Vec<(Range<usize>, UrlKind)> {
    let mut out = Vec::new();
    let mut a = 0;
    while a < raw.len() {
        if !raw.is_char_boundary(a) {
            a += 1;
            continue;
        }
        let Some((kind, scheme)) = url_start(raw, a) else {
            a += 1;
            continue;
        };
        let body = a + scheme;
        let mut end = body
            + raw[body..]
                .find(|c: char| c.is_whitespace() || c == '<' || c == '>')
                .unwrap_or(raw.len() - body);
        loop {
            let u = &raw[a..end];
            let Some(c) = u.chars().next_back() else {
                break;
            };
            let unbalanced =
                |open: char, close: char| u.matches(close).count() > u.matches(open).count();
            if matches!(
                c,
                '.' | ',' | ':' | ';' | '!' | '?' | '\'' | '"' | '*' | '_' | '~'
            ) || (c == ')' && unbalanced('(', ')'))
                || (c == ']' && unbalanced('[', ']'))
            {
                end -= c.len_utf8();
            } else {
                break;
            }
        }
        if end > body {
            out.push((a..end, kind));
        }
        a = end.max(a + 1);
    }
    out
}

/// Whether `text` holds a URL a platform's preview extracts (`http(s)://`, `ftp://`, `www.`):
/// for a connector's re-check of text it has converted (D-270).
pub fn has_preview_url(text: &str) -> bool {
    !bare_url_ranges(text).is_empty()
}

/// `s` with every URL a platform's preview would extract written so it is none (`https:\//…`,
/// `ftp:\//…`, `www\.…`; they still read as the address): for a connector's plain-text fallback.
pub fn break_preview_urls(s: &str) -> String {
    break_schemes(s, "\\")
}

/// A markdown escape `\X` (X ASCII punctuation) undone.
fn unescape_md(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\'
            && let Some(n) = it.peek()
            && n.is_ascii_punctuation()
        {
            out.push(*n);
            it.next();
        } else {
            out.push(c);
        }
    }
    out
}

/// A bare URL as a labelled link (D-270): same text, same target. The destination uses the `<…>`
/// form only when it holds something that would end a plain one.
fn labelled_link(label_md: &str, dest: &str) -> String {
    // (a pipe in a destination would split a table cell)
    let dest = dest.replace('|', "%7C");
    let dest = dest.as_str();
    let plain = !dest.chars().any(|c| {
        c.is_whitespace() || c.is_control() || matches!(c, '(' | ')' | '<' | '>' | '[' | ']' | '\\')
    });
    if plain {
        format!("[{label_md}]({dest})")
    } else {
        format!("[{label_md}]({})", dest_form(dest))
    }
}

/// The label of an autolink `<url>` rewritten to a link: the URL's text, escaped.
fn autolink_label(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for c in url.chars() {
        if matches!(
            c,
            '\\' | '[' | ']' | '*' | '_' | '~' | '`' | '<' | '>' | '&'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Edits for a run of prose text `src[start..end]` (outside code): image openers broken
/// (`![` → `!\[`), link joints the parser did not form broken (`](` → `]\(`) and definition
/// look-alikes at a line start broken (`[x]:` → `[x]\:`) — a platform with a looser dialect
/// (a destination with spaces, a definition after a paragraph line: `docs/20`) would form what
/// the parser did not — and, outside links, bare URLs made labelled links (D-270).
fn prose_edits(
    src: &str,
    start: usize,
    end: usize,
    link_free: bool,
    edits: &mut Vec<(usize, usize, String)>,
) {
    let raw = &src[start..end];
    for (p, _) in raw.match_indices("![") {
        edits.push((start + p + 1, start + p + 1, "\\".to_owned()));
    }
    for (p, _) in raw.match_indices("](") {
        edits.push((start + p + 1, start + p + 1, "\\".to_owned()));
    }
    // A definition look-alike: up to three spaces, `[`, a label without `]`, `]:`.
    let mut line_at = 0usize;
    // Indentation in front of the run on its own line (the parser leaves it out of the text).
    let before = &src[src[..start].rfind('\n').map_or(0, |p| p + 1)..start];
    for line in raw.split_inclusive('\n') {
        let lead = if line_at == 0 { before } else { "" };
        if lead.chars().all(|c| c == ' ') {
            let t = line.trim_start_matches(' ');
            let indent = lead.len() + line.len() - t.len();
            if indent <= 3
                && let Some(r) = t.strip_prefix('[')
                && let Some(c) = r.find(']')
                && r[c + 1..].starts_with(':')
                && !r[..c].contains(['\n', '['])
            {
                // The colon is escaped, not the bracket: an escaped `\[` would balance an earlier
                // `[` of the paragraph and make the whole stretch a definition (no events, the
                // sanitizer blind to it).
                let at = start + line_at + (line.len() - t.len()) + 1 + c + 1;
                edits.push((at, at, "\\".to_owned()));
            }
        }
        line_at += line.len();
    }
    if link_free {
        for (u, kind) in bare_url_ranges(raw) {
            let label = &raw[u.clone()];
            match kind {
                UrlKind::Web => edits.push((
                    start + u.start,
                    start + u.end,
                    labelled_link(label, &unescape_md(label)),
                )),
                // `www.x` → a link to `https://www.x` (rule 2: a link scheme).
                UrlKind::Www => edits.push((
                    start + u.start,
                    start + u.end,
                    labelled_link(label, &format!("https://{}", unescape_md(label))),
                )),
                // `ftp://` is no link scheme, and the server previews it: the slash is escaped.
                UrlKind::Ftp => {
                    let at = start + u.start + 4;
                    edits.push((at, at, "\\".to_owned()));
                }
            }
        }
    }
}

/// The pipes of a table row line that are not backslash-escaped (the cell separators, GFM).
fn row_pipes(line: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut esc = false;
    for (i, c) in line.char_indices() {
        if esc {
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == '|' {
            out.push(i);
        }
    }
    out
}

/// GFM drops the cells of a body row beyond the header's width — the parser shows nothing of their
/// content — but a chat platform renders them (an image, a URL it fetches, an `@channel`: checked on
/// the pinned dev server, `docs/20`). The pipe that would start an extra cell, and every one after it, is escaped, so the
/// rest of the row is text of the last cell and every other rewrite reaches it.
fn table_excess_edits(
    src: &str,
    range: &Range<usize>,
    columns: usize,
    edits: &mut Vec<(usize, usize, String)>,
) {
    let from = src[..range.start].rfind('\n').map_or(0, |p| p + 1);
    let mut at = from;
    for (n, line) in src[from..range.end].split_inclusive('\n').enumerate() {
        let line_at = at;
        at += line.len();
        if n < 2 {
            continue; // the header and the delimiter row
        }
        let body = line.trim_end_matches(['\n', '\r']);
        let prefix = body.len() - body.trim_start_matches([' ', '>']).len();
        let row = &body[prefix..];
        let pipes = row_pipes(row);
        let trimmed = row.trim_end();
        let leading = row.starts_with('|');
        let trailing = pipes.last().is_some_and(|p| *p + 1 == trimmed.len());
        let cells = (pipes.len() + 1).saturating_sub(usize::from(leading) + usize::from(trailing));
        if columns == 0 || cells <= columns {
            continue;
        }
        // The separator after the `columns`-th cell.
        let first = if leading { columns } else { columns - 1 };
        for p in pipes.iter().skip(first) {
            edits.push((line_at + prefix + p, line_at + prefix + p, "\\".to_owned()));
        }
    }
}

/// `src` with every table cell beyond the header's width made text of the row's last cell (the
/// pipe that would start it, and each one after it, backslash-escaped; [`table_excess_edits`]).
/// For a converter that parses text whose structure differs from what [`markdown_safe`] saw (the
/// label of an HTML anchor, read as a document of its own, can be a table): a cell the parser
/// drops gives no event, so raw HTML in it would never be converted (FIX-195, D-270).
pub fn escape_table_excess(src: &str) -> String {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for (ev, r) in Parser::new_ext(src, options()).into_offset_iter() {
        if let Event::Start(Tag::Table(aligns)) = ev {
            table_excess_edits(src, &r, aligns.len(), &mut edits);
        }
    }
    if edits.is_empty() {
        return src.to_owned();
    }
    edits.sort_by_key(|e| e.0);
    edits.dedup_by_key(|e| e.0);
    let mut out = String::with_capacity(src.len() + edits.len());
    let mut at = 0;
    for (a, _, ins) in edits {
        out.push_str(&src[at..a]);
        out.push_str(&ins);
        at = a;
    }
    out.push_str(&src[at..]);
    out
}

/// Spaces and tabs, then at most one line break followed by the line's indentation and quote
/// markers: the gap a reference definition allows between its parts.
fn skip_def_space(b: &[u8], mut i: usize) -> usize {
    while matches!(b.get(i), Some(b' ' | b'\t')) {
        i += 1;
    }
    if b.get(i) == Some(&b'\r') {
        i += 1;
    }
    if b.get(i) == Some(&b'\n') {
        i += 1;
        while matches!(b.get(i), Some(b' ' | b'\t' | b'>')) {
            i += 1;
        }
    }
    i
}

/// The title of a reference definition at the start of `s` (`[label]: dest "title"`, `'title'`
/// or `(title)`), from the end of the destination through its closing delimiter.
fn def_title(s: &str) -> Option<Range<usize>> {
    let b = s.as_bytes();
    let mut i = 1;
    loop {
        match *b.get(i)? {
            b'\\' => i += 2,
            b']' => break,
            b'[' => return None,
            _ => i += 1,
        }
    }
    if b.get(i + 1) != Some(&b':') {
        return None;
    }
    i = skip_def_space(b, i + 2);
    // The destination, as the connector's re-check reads it: `<…>` or a run without whitespace.
    let start = i;
    if b.get(i) == Some(&b'<') {
        loop {
            i += 1;
            match *b.get(i)? {
                b'\\' => i += 1,
                b'>' => break,
                b'\n' => return None,
                _ => {}
            }
        }
        i += 1;
    } else {
        while b.get(i).is_some_and(|c| !c.is_ascii_whitespace()) {
            i += 1;
        }
    }
    if i == start {
        return None;
    }
    let dest_end = i;
    let j = skip_def_space(b, i);
    let close = match *b.get(j)? {
        b'"' => b'"',
        b'\'' => b'\'',
        b'(' => b')',
        _ => return None,
    };
    let mut k = j + 1;
    loop {
        match *b.get(k)? {
            b'\\' => k += 2,
            c if c == close => break,
            b'(' if close == b')' => return None,
            _ => k += 1,
        }
    }
    Some(dest_end..k + 1)
}

/// FIX-195 (D-270): a reference definition gives the parser no event — no other rewrite reaches
/// it — and its title is never shown, but a platform's preview, or a re-check, reads the URL in
/// it as bare text. A title that holds a preview URL (`http(s)://`, `ftp://`, `www.`, an autolink)
/// is dropped; the label and the destination stay, a title without one stays. Definitions are
/// found as text no event covers (outside containers' markers), so a repeated label — which the
/// parser's own list of definitions keeps only once — is found too.
fn def_title_edits(
    src: &str,
    evs: &[(Event<'_>, Range<usize>)],
    edits: &mut Vec<(usize, usize, String)>,
) {
    use pulldown_cmark::TagEnd;
    let covered: Vec<&Range<usize>> = evs
        .iter()
        .filter(|(e, _)| {
            !matches!(
                e,
                Event::Start(
                    Tag::BlockQuote(_)
                        | Tag::List(_)
                        | Tag::Item
                        | Tag::FootnoteDefinition(_)
                        | Tag::DefinitionList
                        | Tag::DefinitionListTitle
                        | Tag::DefinitionListDefinition
                ) | Event::End(
                    TagEnd::BlockQuote(_)
                        | TagEnd::List(_)
                        | TagEnd::Item
                        | TagEnd::FootnoteDefinition
                        | TagEnd::DefinitionList
                        | TagEnd::DefinitionListTitle
                        | TagEnd::DefinitionListDefinition
                )
            )
        })
        .map(|(_, r)| r)
        .collect();
    let in_gap = |r: &Range<usize>| !covered.iter().any(|c| c.start < r.end && r.start < c.end);
    for (p, _) in src.match_indices('[') {
        if !in_gap(&(p..p + 1)) {
            continue;
        }
        if let Some(t) = def_title(&src[p..]) {
            let t = p + t.start..p + t.end;
            if in_gap(&t) && has_preview_url(&src[t.clone()]) {
                edits.push((t.start, t.end, String::new()));
            }
        }
    }
}

/// One pass of the exceptions 1–3 (byte-range edits), then exception 4.
fn rewrite(src: &str) -> String {
    let evs: Vec<(Event<'_>, Range<usize>)> =
        Parser::new_ext(src, options()).into_offset_iter().collect();
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut consumed = 0usize;
    let mut link_ends: Vec<usize> = Vec::new();
    let mut in_code = false;
    let mut text_until = 0usize;
    for (i, (ev, r)) in evs.iter().enumerate() {
        if r.start < consumed {
            continue;
        }
        link_ends.retain(|e| *e > r.start);
        match ev {
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::End(pulldown_cmark::TagEnd::CodeBlock) => in_code = false,
            Event::Start(Tag::Table(aligns)) => {
                table_excess_edits(src, r, aligns.len(), &mut edits);
            }
            // Prose: an image opener is broken wherever it is not code (D-227(1), review rule (c)).
            Event::Text(_) if !in_code && r.start >= text_until => {
                // A run of adjacent text events (the parser splits prose at every `[`, `]`, `_`…):
                // what the platform reads is the run as written.
                let mut end = r.end;
                let mut j = i;
                // (An escaped character's event starts after its backslash: that gap is fine.)
                while let Some((Event::Text(_), rr)) = evs.get(j + 1)
                    && rr.start >= end
                    && matches!(&src[end..rr.start], "" | "\\")
                {
                    end = rr.end;
                    j += 1;
                }
                text_until = end;
                prose_edits(src, r.start, end, link_ends.is_empty(), &mut edits);
            }
            Event::InlineMath(_) | Event::DisplayMath(_) => {
                let raw = &src[r.clone()];
                let tame = tame_math(raw);
                if tame != raw {
                    edits.push((r.start, r.end, tame));
                }
                consumed = r.end;
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                let alt = tidy_alt(&plain_text(&evs, i));
                let rep = match (allowed_url(dest_url) && link_ends.is_empty())
                    .then(|| host_of(dest_url))
                {
                    Some(host) => {
                        let h = host
                            .map(|h| format!(" ({})", escape_inline(&h)))
                            .unwrap_or_default();
                        format!("[🖼 {alt}{h}]({})", dest_form(dest_url))
                    }
                    None => format!("🖼 {alt}"),
                };
                edits.push((r.start, r.end, rep));
                consumed = r.end;
            }
            Event::Start(Tag::Link {
                dest_url,
                link_type,
                ..
            }) => {
                let label = plain_text(&evs, i);
                // `<a@b.c>` is an e-mail autolink: its destination carries no scheme but is a mailto.
                if allowed_url(dest_url) && matches!(link_type, LinkType::Autolink) {
                    // `<https://x>` is fetched by a platform's preview like a bare URL (D-270).
                    edits.push((
                        r.start,
                        r.end,
                        labelled_link(&autolink_label(dest_url), dest_url),
                    ));
                    consumed = r.end;
                } else if allowed_url(dest_url) || matches!(link_type, LinkType::Email) {
                    link_ends.push(r.end);
                    if !matches!(link_type, LinkType::Autolink | LinkType::Email)
                        && let Some(suf) = deceptive_suffix(&label, dest_url).or_else(|| {
                            raw_dest(&src[r.clone()]).and_then(|d| deceptive_suffix(&label, &d))
                        })
                        && !src[r.end..].starts_with(&suf)
                    {
                        edits.push((r.end, r.end, suf));
                    }
                } else {
                    let text = if label.trim().is_empty() {
                        dest_url.to_string()
                    } else {
                        label
                    };
                    edits.push((r.start, r.end, escape_inline(&text)));
                    consumed = r.end;
                }
            }
            Event::Start(Tag::HtmlBlock) => {
                let raw = &src[r.clone()];
                let clean = sanitize_html(raw);
                if clean != raw {
                    edits.push((r.start, r.end, clean));
                }
                consumed = r.end;
            }
            Event::InlineHtml(_) => {
                let raw = &src[r.clone()];
                let clean = sanitize_html(raw);
                // A raw `<a href>` … `</a>` pair: the label check spans the events between.
                if let Some(dest) = href_of(&clean)
                    && let Some(j) = evs[i + 1..].iter().position(|(e, rr)| {
                        matches!(e, Event::InlineHtml(_))
                            && src[rr.clone()].to_ascii_lowercase().starts_with("</a")
                    })
                {
                    let close = &evs[i + 1 + j].1;
                    let label: String = evs[i + 1..i + 1 + j]
                        .iter()
                        .filter_map(|(e, _)| match e {
                            Event::Text(t) | Event::Code(t) => Some(t.to_string()),
                            _ => None,
                        })
                        .collect();
                    if let Some(suf) = deceptive_suffix(&label, &dest)
                        && !src[close.end..].starts_with(&suf)
                    {
                        edits.push((close.end, close.end, suf));
                    }
                }
                if clean != raw {
                    edits.push((r.start, r.end, clean));
                }
            }
            _ => {}
        }
    }
    def_title_edits(src, &evs, &mut edits);
    edits.sort_by_key(|e| (e.0, e.1));
    let mut out = String::with_capacity(src.len() + 64);
    let mut at = 0;
    for (a, b, rep) in edits {
        if a < at {
            continue;
        }
        out.push_str(&src[at..a]);
        out.push_str(&rep);
        at = b;
    }
    out.push_str(&src[at..]);
    defang_tokens(&out)
}

/// Removing markup (a comment, a stripped tag) can change how the rest parses, exposing text the
/// first pass saw as HTML: run the rewrite until it stops changing (a few passes at most; each
/// only escapes more).
fn rewrite_to_fixed_point(src: &str) -> String {
    let mut out = rewrite(src);
    for _ in 0..3 {
        let next = rewrite(&out);
        if next == out {
            break;
        }
        out = next;
    }
    out
}

/// Exception 4: break every typed neutral token with a zero-width space after the `<`
/// (the sanitizer's own input cleaning strips it again, so the result stays idempotent).
fn defang_tokens(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while let Some(p) = s[i..].find("<") {
        let at = i + p;
        out.push_str(&s[i..at + 1]);
        let tail = &s[at + 1..];
        // FIX-178: a later step may have backslash-escaped a character of the token (`<@here\>`,
        // `<\#space:`); the break must come back after the input cleaning stripped it, or the
        // output is not a fixed point. Matching with the backslashes left out defangs a superset.
        let low: String = tail
            .chars()
            .take(12)
            .filter(|&c| c != '\\')
            .take(8)
            .collect::<String>()
            .to_ascii_lowercase();
        if low.starts_with("@user:")
            || low.starts_with("@bot:")
            || low.starts_with("@here>")
            || low.starts_with("#space:")
        {
            out.push('\u{200B}');
        }
        i = at + 1;
    }
    out.push_str(&s[i..]);
    out
}

fn has_token(s: &str) -> bool {
    defang_tokens(s) != s
}

/// The re-parse check: no image, no link outside the schemes, and the text is a fixed point
/// of the rewrite (so no exception is left to apply).
fn verified(out: &str) -> bool {
    for (e, _) in Parser::new_ext(out, options()).into_offset_iter() {
        match e {
            Event::Start(Tag::Image { .. }) => return false,
            Event::Start(Tag::Link {
                dest_url,
                link_type,
                ..
            }) if !allowed_url(&dest_url) && !matches!(link_type, LinkType::Email) => {
                return false;
            }
            _ => {}
        }
    }
    !has_token(out) && rewrite(out) == out
}

/// Byte offsets where a top-level block starts.
fn block_starts(src: &str) -> Vec<usize> {
    let mut depth = 0usize;
    let mut starts = vec![0usize];
    for (e, r) in Parser::new_ext(src, options()).into_offset_iter() {
        match e {
            Event::Start(_) => {
                if depth == 0 {
                    starts.push(r.start);
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            _ if depth == 0 => starts.push(r.start),
            _ => {}
        }
    }
    starts.sort_unstable();
    starts.dedup();
    starts
}

/// An agent's message body for a chat platform (D-227): the text as written, except the four
/// security exceptions above. Controls, bidi and format characters are stripped, line endings
/// normalised, layout kept; clamped to `max` characters *before* rewriting.
///
/// Idempotent: `markdown_safe(markdown_safe(x)) == markdown_safe(x)`.
pub fn markdown_safe(s: &str, max: usize) -> String {
    let out = markdown_safe_inner(&clean_layout(s, max));
    // Removing markup can leave blank lines in front; a fixed point has none.
    without_leading_blank_lines(&out).to_owned()
}

fn markdown_safe_inner(clean: &str) -> String {
    let out = rewrite_to_fixed_point(clean);
    if verified(&out) {
        return out;
    }
    let clean = clean.to_owned();
    // A block that cannot be verified falls back to escaped data; the others stay Markdown.
    let mut starts = block_starts(&clean);
    starts.push(clean.len());
    let mut joined = String::with_capacity(clean.len());
    for w in starts.windows(2) {
        let block = &clean[w[0]..w[1]];
        let o = rewrite_to_fixed_point(block);
        if verified(&o) {
            joined.push_str(&o);
        } else {
            FALLBACKS.fetch_add(1, Ordering::Relaxed);
            joined.push_str(&data_fallback(block));
            joined.push('\n');
        }
    }
    if verified(&joined) {
        return joined;
    }
    FALLBACKS.fetch_add(1, Ordering::Relaxed);
    data_fallback(&clean)
}

/// A block that could not be verified, as escaped data. URLs in it are broken (`https:\//…`,
/// which still reads as the address) so no platform's preview fetches them (D-270).
fn data_fallback(block: &str) -> String {
    break_schemes(&markdown_as_data(block, usize::MAX), "\\")
}

/// `ins` after the colon of every `http://` / `https://` in `s`, wherever it sits (also inside
/// another URL: `https://web.archive.org/…/https://x`) — no platform reads the result as a URL.
fn break_schemes(s: &str, ins: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let mut at = 0;
    let mut i = 0;
    while i < s.len() {
        if !s.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let Some((kind, scheme)) = url_start(s, i) else {
            i += 1;
            continue;
        };
        // After the colon of a scheme; after `www` for a `www.` URL.
        let cut = if kind == UrlKind::Www {
            i + 3
        } else {
            i + scheme - 2
        };
        out.push_str(&s[at..cut]);
        out.push_str(ins);
        at = cut;
        i += scheme.max(1);
    }
    out.push_str(&s[at..]);
    out
}

fn is_wordish(c: char) -> bool {
    // `<`, and the zero-width space that breaks a typed `<@here>`, keep an inert token inert.
    c.is_alphanumeric()
        || matches!(
            c,
            '_' | '@' | '.' | '/' | '-' | '+' | '%' | '<' | '\u{200B}'
        )
}

/// D-226(b), D-271(b): a standalone `@here` (exactly that spelling, outside code and outside a
/// link label, at word boundaries) in prose becomes the core's `<@here>` broadcast token.
/// `@Here`, `foo@here.org`, `@hereby`, a link label, code spans and fences are left alone — the
/// connector defuses what is left. Apply it to the output of [`markdown_safe`], never to
/// untrusted data.
pub fn broadcast_here(s: &str) -> String {
    let mut edits: Vec<(usize, usize)> = Vec::new();
    let mut in_code = false;
    let mut in_link = 0usize;
    // Raw `<code>`, `<kbd>` and `<pre>` are code for this rule too (their text is shown as typed).
    let mut html_code = 0usize;
    for (e, r) in Parser::new_ext(s, options()).into_offset_iter() {
        match &e {
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::End(pulldown_cmark::TagEnd::CodeBlock) => in_code = false,
            Event::Start(Tag::Link { .. }) => in_link += 1,
            Event::End(pulldown_cmark::TagEnd::Link) => in_link = in_link.saturating_sub(1),
            Event::InlineHtml(h) => {
                let t = h.trim_start_matches('<').to_ascii_lowercase();
                let (closing, name) = t
                    .strip_prefix('/')
                    .map_or((false, t.as_str()), |n| (true, n));
                let name: String = name
                    .chars()
                    .take_while(char::is_ascii_alphanumeric)
                    .collect();
                if matches!(name.as_str(), "code" | "kbd" | "pre") {
                    if closing {
                        html_code = html_code.saturating_sub(1);
                    } else if !t.trim_end().ends_with("/>") {
                        html_code += 1;
                    }
                }
            }
            _ => {}
        }
        let Event::Text(t) = e else { continue };
        if in_code || html_code > 0 || in_link > 0 {
            continue;
        }
        if &s[r.clone()] != t.as_ref() {
            continue;
        }
        let mut from = 0;
        while let Some(p) = t[from..].find("@here") {
            let a = from + p;
            let b = a + 5;
            from = b;
            let abs = r.start + a;
            let before = s[..abs].chars().next_back();
            let mut after = s[r.start + b..].chars();
            let next = after.next();
            let next2 = after.next();
            let tail_ok = match next {
                None => true,
                Some(c) if c.is_alphanumeric() || c == '_' => false,
                Some('.' | '-' | '@') => !next2.is_some_and(char::is_alphanumeric),
                Some(_) => true,
            };
            if before.is_none_or(|c| !is_wordish(c)) && tail_ok {
                edits.push((abs, r.start + b));
            }
        }
    }
    if edits.is_empty() {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len() + edits.len() * 2);
    let mut at = 0;
    for (a, b) in edits {
        out.push_str(&s[at..a]);
        out.push_str("<@here>");
        at = b;
    }
    out.push_str(&s[at..]);
    out
}

#[cfg(test)]
mod tests;
