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
        "<https://x.example>",
        "<a@x.example>",
    ] {
        assert_eq!(md(ok), ok, "{ok}");
    }
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
        "<https://cern.ch>",
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

#[test]
fn broadcast_here_only_in_prose_at_word_boundaries() {
    for (src, want) in [
        ("@here the build is red", "<@here> the build is red"),
        ("ok @Here, look", "ok <@here>, look"),
        ("**@here** now", "**<@here>** now"),
        ("see @here.", "see <@here>."),
        ("a `@here` b", "a `@here` b"),
        ("```\n@here\n```", "```\n@here\n```"),
        ("mail foo@here.org", "mail foo@here.org"),
        ("@hereby", "@hereby"),
        ("x@here", "x@here"),
        ("@channel @all @alex", "@channel @all @alex"),
        ("[@here](https://x.example)", "[<@here>](https://x.example)"),
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
        prop_assert!(!has_token(&out), "token formed: {out:?}");
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
