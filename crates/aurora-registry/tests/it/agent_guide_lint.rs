//! (P5-GUIDE-1, C-640) The linter matrix: one case per violation code, and the valid shapes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use aurora_registry::validate::agent_guide_rules::archive_rules;
use aurora_registry::{Manifest, Package};

use super::agent_guide::{AGENT, FILES, TOOLS, yaml};

fn lint(y: &str, files: &[(&str, &[u8])]) -> Vec<&'static str> {
    let m = Manifest::parse(y, "99.0.0").unwrap();
    let mut pkg = Package {
        files: Default::default(),
        signature: None,
    };
    for (f, c) in files {
        pkg.files.insert(format!("agent/demo-app/{f}"), c.to_vec());
    }
    let mut v = aurora_registry::validate::manifest(&m);
    archive_rules(&m, &pkg, &mut v);
    let mut codes: Vec<&'static str> = v
        .iter()
        .map(|x| x.code)
        .filter(|c| c.starts_with("agent_"))
        .collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

fn files() -> Vec<(&'static str, &'static [u8])> {
    FILES.iter().map(|(f, c)| (*f, c.as_bytes())).collect()
}

fn agent(summary: &str, guide: &str) -> String {
    format!("    agent:\n{summary}      guide:\n{guide}")
}

fn entry(file: &str, extra: &str) -> String {
    format!("        - {{ file: {file}, title: \"T\"{extra} }}\n")
}

const SUMMARY: &str = "      summary: \"Things.\"\n";

#[test]
fn a_complete_guide_is_valid() {
    assert!(lint(&yaml(TOOLS, AGENT, ""), &files()).is_empty());
}

#[test]
fn a_package_without_guide_keys_has_no_agent_violations() {
    assert!(
        lint(
            &yaml(TOOLS, "    agent:\n      instructions: hi\n", ""),
            &[]
        )
        .is_empty()
    );
    assert!(lint(&yaml(TOOLS, "", ""), &[]).is_empty());
}

#[test]
fn summary_rules() {
    let long = "x".repeat(161);
    let y = yaml(
        TOOLS,
        &format!("    agent:\n      summary: \"{long}\"\n"),
        "",
    );
    assert_eq!(lint(&y, &[]), ["agent_summary_too_long"]);
    let ok = "x".repeat(160);
    let y = yaml(TOOLS, &format!("    agent:\n      summary: \"{ok}\"\n"), "");
    assert!(lint(&y, &[]).is_empty());
    let y = yaml(TOOLS, "    agent:\n      summary: \"two\\nlines\"\n", "");
    assert_eq!(lint(&y, &[]), ["agent_summary_multiline"]);
    let y = yaml(TOOLS, &agent("", &entry("api.md", "")), "");
    assert_eq!(
        lint(&y, &[("api.md", b"# a\n")]),
        ["agent_summary_required"]
    );
}

#[test]
fn guide_entry_rules() {
    let f = |g: String| lint(&yaml(TOOLS, &agent(SUMMARY, &g), ""), &[("api.md", b"a\n")]);
    // file name grammar
    assert!(f(entry("Api.md", "")).contains(&"agent_guide_file_name"));
    assert!(f(entry("api.exe", "")).contains(&"agent_guide_file_name"));
    assert!(f(entry("-api.md", "")).contains(&"agent_guide_file_name"));
    assert!(f(entry(&format!("{}.md", "a".repeat(49)), "")).contains(&"agent_guide_file_name"));
    // reserved names
    for n in ["guide.md", "tools.md", "api.json"] {
        assert!(
            f(entry(n, "")).contains(&"agent_guide_reserved_name"),
            "{n}"
        );
    }
    // duplicates
    assert!(
        f(format!("{}{}", entry("api.md", ""), entry("api.md", "")))
            .contains(&"agent_guide_duplicate_file")
    );
    // title
    let empty = "        - { file: api.md, title: \"\" }\n".to_owned();
    assert!(f(empty).contains(&"agent_guide_title"));
    let long = format!(
        "        - {{ file: api.md, title: \"{}\" }}\n",
        "t".repeat(121)
    );
    assert!(f(long).contains(&"agent_guide_title"));
    let nl = "        - { file: api.md, title: \"a\\nb\" }\n".to_owned();
    assert!(f(nl).contains(&"agent_guide_title"));
    // kind
    assert!(f(entry("api.md", ", kind: howto")).contains(&"agent_guide_kind"));
    assert!(f(entry("api.md", ", kind: reference")).is_empty());
    // a recipe is Markdown
    assert!(f(entry("api.txt", ", kind: recipe")).contains(&"agent_recipe_not_markdown"));
    // computed fields exist only in the snapshot
    assert!(f(entry("api.md", ", sha256: abc")).contains(&"agent_guide_computed_field"));
    assert!(f(entry("api.md", ", size: 3")).contains(&"agent_guide_computed_field"));
    // too many entries
    let many: String = (0..25).map(|i| entry(&format!("f{i}.md"), "")).collect();
    assert!(f(many).contains(&"agent_guide_too_many_files"));
    // strict keys: an unknown key in an entry or in `agent` is a parse error
    let bad = yaml(
        TOOLS,
        &agent(SUMMARY, "        - { file: api.md, title: T, bogus: 1 }\n"),
        "",
    );
    assert!(Manifest::parse(&bad, "99.0.0").is_err());
    let bad = yaml(TOOLS, "    agent:\n      guides: []\n", "");
    assert!(Manifest::parse(&bad, "99.0.0").is_err());
}

#[test]
fn archive_rules_cover_listed_present_text_and_sizes() {
    let y = yaml(TOOLS, &agent(SUMMARY, &entry("api.md", "")), "");
    // listed <=> present
    assert_eq!(lint(&y, &[]), ["agent_guide_missing_file"]);
    assert_eq!(
        lint(&y, &[("api.md", b"a\n"), ("extra.md", b"b\n")]),
        ["agent_guide_unlisted_file"]
    );
    // unknown app
    let m = Manifest::parse(&y, "99.0.0").unwrap();
    let mut pkg = Package {
        files: Default::default(),
        signature: None,
    };
    pkg.files
        .insert("agent/demo-app/api.md".into(), b"a\n".to_vec());
    pkg.files
        .insert("agent/other/api.md".into(), b"a\n".to_vec());
    let mut v = Vec::new();
    archive_rules(&m, &pkg, &mut v);
    assert_eq!(
        v.iter().map(|x| x.code).collect::<Vec<_>>(),
        ["agent_guide_unknown_app"]
    );
    // text only: NUL, CR, other control characters, BOM, invalid UTF-8
    for bad in [
        &b"a\0b"[..],
        b"a\r\nb",
        b"a\x1bb",
        b"\xef\xbb\xbfa",
        b"\xff\xfe",
    ] {
        assert_eq!(
            lint(&y, &[("api.md", bad)]),
            ["agent_guide_not_text"],
            "{bad:?}"
        );
    }
    // LF and TAB are fine
    assert!(lint(&y, &[("api.md", b"a\tb\nc\n")]).is_empty());
    // sizes
    let big = vec![b'x'; 64 * 1024 + 1];
    assert_eq!(
        lint(&y, &[("api.md", &big)]),
        ["agent_guide_file_too_large"]
    );
    let exact = vec![b'x'; 64 * 1024];
    assert!(lint(&y, &[("api.md", &exact)]).is_empty());
    let g: String = (0..5).map(|i| entry(&format!("f{i}.md"), "")).collect();
    let y5 = yaml(TOOLS, &agent(SUMMARY, &g), "");
    let part = vec![b'x'; 60 * 1024];
    let five: Vec<(String, &[u8])> = (0..5).map(|i| (format!("f{i}.md"), &part[..])).collect();
    let refs: Vec<(&str, &[u8])> = five.iter().map(|(f, c)| (f.as_str(), *c)).collect();
    assert_eq!(lint(&y5, &refs), ["agent_guide_app_too_large"]);
}

#[test]
fn archive_paths_are_exactly_two_levels_deep() {
    use aurora_registry::{Limits, builder::PackageBuilder};
    let y = yaml(TOOLS, &agent(SUMMARY, &entry("api.md", "")), "");
    let mut b = PackageBuilder::new(&y);
    b.set_file("agent/demo-app/api.md", b"a\n");
    let ok = Package::read(&b.build().unwrap(), &Limits::default());
    assert!(ok.is_ok(), "{:?}", ok.err());
    for deep in ["agent/demo-app/sub/api.md", "agent/api.md"] {
        let mut b = PackageBuilder::new(&y);
        b.set_file(deep, b"a\n");
        let r = Package::read(&b.build().unwrap(), &Limits::default());
        assert!(
            r.is_err_and(|v| v.iter().any(|x| x.code == "path_unknown")),
            "{deep}"
        );
    }
}
