//! (P5-APPSKILL-1, C-1130, SEC-150) The package linter's rules for app skills and the ignored
//! guide: one case per violation code, the valid shapes, and what is ignored with a warning.
//! Each case fails if its rule is removed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use aurora_registry::validate::agent_rules::archive_rules;
use aurora_registry::{Manifest, Package};

use super::agent_files::{AGENT, LEGACY_AGENT, TOOLS, skill_md, yaml};

const APP: &str = "demo-app";

fn pkg_of(files: &[(String, Vec<u8>)]) -> Package {
    let mut pkg = Package::default();
    for (f, c) in files {
        pkg.files.insert(f.clone(), c.clone());
    }
    pkg
}

/// Codes of the agent and skill rules for `files` (full archive paths).
fn lint_y(y: &str, files: &[(String, Vec<u8>)]) -> Vec<&'static str> {
    lint_with(y, files, &[])
}

/// Same, with the apps of other active packages: `(app id, package id)`.
fn lint_with(y: &str, files: &[(String, Vec<u8>)], others: &[(&str, &str)]) -> Vec<&'static str> {
    let m = Manifest::parse(y, "99.0.0").unwrap();
    let pkg = pkg_of(files);
    let others: std::collections::HashMap<String, String> = others
        .iter()
        .map(|(a, p)| ((*a).to_owned(), (*p).to_owned()))
        .collect();
    let mut v = aurora_registry::validate::manifest(&m);
    archive_rules(&m, &pkg, &others, &mut v);
    let mut codes: Vec<&'static str> = v
        .iter()
        .map(|x| x.code)
        .filter(|c| c.starts_with("agent_") || c.starts_with("app_skill"))
        .collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

fn lint(files: &[(String, Vec<u8>)]) -> Vec<&'static str> {
    lint_y(&yaml(TOOLS, AGENT, ""), files)
}

/// `skills/demo-app/<name>/<rel>`.
fn f(name: &str, rel: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    (format!("skills/{APP}/{name}/{rel}"), bytes.to_vec())
}

fn md(name: &str) -> (String, Vec<u8>) {
    f(
        name,
        "SKILL.md",
        skill_md(name, "Do the thing.", "Steps.\n").as_bytes(),
    )
}

const N: &str = "demo-app-x";

#[test]
fn a_package_with_skills_is_valid() {
    let files = vec![
        md(N),
        f(N, "scripts/run.sh", b"#!/bin/sh\necho hi\n"),
        f(N, "reference/a/b.txt", b"nested\n"),
        md("demo-app"),
        md("demo-app-y-z"),
    ];
    assert!(lint(&files).is_empty(), "{:?}", lint(&files));
}

#[test]
fn a_package_without_agent_keys_or_skills_has_no_agent_violations() {
    let y = yaml(TOOLS, "    agent:\n      instructions: hi\n", "");
    assert!(lint_y(&y, &[]).is_empty());
    assert!(lint_y(&yaml(TOOLS, "", ""), &[]).is_empty());
}

#[test]
fn summary_rules() {
    let long = "x".repeat(161);
    let y = yaml(
        TOOLS,
        &format!("    agent:\n      summary: \"{long}\"\n"),
        "",
    );
    assert_eq!(lint_y(&y, &[]), ["agent_summary_too_long"]);
    let ok = "x".repeat(160);
    let y = yaml(TOOLS, &format!("    agent:\n      summary: \"{ok}\"\n"), "");
    assert!(lint_y(&y, &[]).is_empty());
    let y = yaml(TOOLS, "    agent:\n      summary: \"two\\nlines\"\n", "");
    assert_eq!(lint_y(&y, &[]), ["agent_summary_multiline"]);
    // required with skills, and with an operations file; not with only the essentials
    let no_summary = yaml(TOOLS, "    agent:\n      instructions: hi\n", "");
    assert_eq!(lint_y(&no_summary, &[md(N)]), ["agent_summary_required"]);
    let ops = yaml(TOOLS, "    agent:\n      operations: operations.json\n", "");
    assert!(lint_y(&ops, &[]).contains(&"agent_summary_required"));
    // a whitespace-only summary does not count
    let blank = yaml(TOOLS, "    agent:\n      summary: \"  \"\n", "");
    assert_eq!(lint_y(&blank, &[md(N)]), ["agent_summary_required"]);
}

#[test]
fn skill_names_follow_the_app_id() {
    // unknown app
    let files = vec![(
        "skills/other-app/other-app-x/SKILL.md".to_owned(),
        skill_md("other-app-x", "d", "b").into_bytes(),
    )];
    assert_eq!(lint(&files), ["app_skill_unknown_app"]);
    // not the app id nor prefixed by it
    assert_eq!(lint(&[md("other-x")]), ["app_skill_name"]);
    assert_eq!(
        lint(&[md("demo-appx")]),
        ["app_skill_name"],
        "needs the hyphen"
    );
    // grammar and reserved prefix
    for bad in ["demo-app-X", "demo-app--x", "demo-app-", "demo-app_x"] {
        assert!(lint(&[md(bad)]).contains(&"app_skill_name"), "{bad}");
    }
    for bad in ["aurora", "aurora-x"] {
        assert!(lint(&[md(bad)]).contains(&"app_skill_name"), "{bad}");
    }
    let long = format!("demo-app-{}", "a".repeat(60));
    assert!(lint(&[md(&long)]).contains(&"app_skill_name"));
    // an `indico-tools` skill named `indico-x` is refused (the app id is `indico-tools`)
    let y = "package: { id: demo, version: 1.2.3, contract: 1, min_core: 0.1.0, publisher: aurora, name: Demo }\napps:\n  - about: { id: indico-tools, name: Tools }\n    credentials: [ { name: api_token, secret: true } ]\n    connect:\n      - id: api_key\n        kind: fields\n        fields: [ { name: api_token, label: T, type: secret, required: true } ]\n    hosts:\n      - match: { host: api.example.test }\n        inject:\n          - bearer: \"{credentials.api_token}\"\n    agent:\n      summary: \"S\"\n";
    let bad = vec![(
        "skills/indico-tools/indico-x/SKILL.md".to_owned(),
        skill_md("indico-x", "d", "b").into_bytes(),
    )];
    assert_eq!(lint_y(y, &bad), ["app_skill_name"]);
    let good = vec![(
        "skills/indico-tools/indico-tools-x/SKILL.md".to_owned(),
        skill_md("indico-tools-x", "d", "b").into_bytes(),
    )];
    assert!(lint_y(y, &good).is_empty());
}

#[test]
fn the_longest_matching_app_id_owns_the_skill() {
    // `jira` and `jira-dc` in one package: `jira-dc-x` belongs under skills/jira-dc/, not skills/jira/.
    let y = "package: { id: demo, version: 1.2.3, contract: 1, min_core: 0.1.0, publisher: aurora, name: Demo }\napps:\n  - about: { id: jira, name: Jira }\n    credentials: [ { name: api_token, secret: true } ]\n    connect:\n      - id: api_key\n        kind: fields\n        fields: [ { name: api_token, label: T, type: secret, required: true } ]\n    hosts:\n      - match: { host: a.example.test }\n        inject:\n          - bearer: \"{credentials.api_token}\"\n    agent:\n      summary: \"S\"\n  - about: { id: jira-dc, name: Jira DC }\n    credentials: [ { name: api_token, secret: true } ]\n    connect:\n      - id: api_key\n        kind: fields\n        fields: [ { name: api_token, label: T, type: secret, required: true } ]\n    hosts:\n      - match: { host: b.example.test }\n        inject:\n          - bearer: \"{credentials.api_token}\"\n    agent:\n      summary: \"S\"\n";
    let under_jira = vec![(
        "skills/jira/jira-dc-x/SKILL.md".to_owned(),
        skill_md("jira-dc-x", "d", "b").into_bytes(),
    )];
    assert_eq!(lint_y(y, &under_jira), ["app_skill_name"]);
    let under_dc = vec![(
        "skills/jira-dc/jira-dc-x/SKILL.md".to_owned(),
        skill_md("jira-dc-x", "d", "b").into_bytes(),
    )];
    assert!(lint_y(y, &under_dc).is_empty());
    let plain = vec![(
        "skills/jira/jira-x/SKILL.md".to_owned(),
        skill_md("jira-x", "d", "b").into_bytes(),
    )];
    assert!(lint_y(y, &plain).is_empty());
}

/// (FIX-221, D-465) A skill name never starts with the id of an app of another active package.
#[test]
fn a_skill_named_with_another_apps_id_is_refused() {
    let y = yaml(TOOLS, AGENT, "");
    let long = vec![md("demo-app-x-thing")];
    // another active package owns `demo-app-x`: refused, at the skill's path
    let others = [("demo-app-x", "other")];
    assert_eq!(
        lint_with(&y, &long, &others),
        ["app_skill_name_claimed"],
        "the other app owns the prefix"
    );
    // with no `demo-app-x` active the same package is valid
    assert!(lint_with(&y, &long, &[]).is_empty());
    // a name equal to the other app's id is refused too
    assert_eq!(
        lint_with(&y, &[md("demo-app-x")], &others),
        ["app_skill_name_claimed"]
    );
    // an upgrade of the package that owns `demo-app-x` itself (same package id `demo`) is not
    // refused by its own old version
    assert!(lint_with(&y, &long, &[("demo-app-x", "demo")]).is_empty());
    // only a `-` boundary counts: `demo-app-xy` does not start with `demo-app-x-`
    assert!(lint_with(&y, &[md("demo-app-xy")], &others).is_empty());
    // the package's own skills are untouched by other apps that do not prefix them
    assert!(lint_with(&y, &[md(N), md("demo-app")], &[("demo", "other")]).is_empty());
    // a chain of other apps: the longest id owns the name, and the message names it and its package
    let chain = [
        ("demo-app-x", "p1"),
        ("demo-app-x-y", "p2"),
        ("demo", "p3"),
    ];
    assert_eq!(
        lint_with(&y, &[md("demo-app-x-y-z")], &chain),
        ["app_skill_name_claimed"]
    );
    let m = Manifest::parse(&y, "99.0.0").unwrap();
    let pkg = pkg_of(&[md("demo-app-x-y-z")]);
    let others: std::collections::HashMap<String, String> = chain
        .iter()
        .map(|(a, p)| ((*a).to_owned(), (*p).to_owned()))
        .collect();
    let mut v = Vec::new();
    archive_rules(&m, &pkg, &others, &mut v);
    let v: Vec<_> = v
        .iter()
        .filter(|x| x.code == "app_skill_name_claimed")
        .collect();
    assert_eq!(v.len(), 1);
    assert!(
        v[0].message.contains("`demo-app-x-y`") && v[0].message.contains("`p2`"),
        "{}",
        v[0].message
    );
    assert_eq!(v[0].path, "skills/demo-app/demo-app-x-y-z");
}

#[test]
fn skill_md_frontmatter_rules() {
    let bad = |text: &str| lint(&[f(N, "SKILL.md", text.as_bytes())]);
    assert_eq!(bad("# no frontmatter\n"), ["app_skill_frontmatter"]);
    assert_eq!(
        bad("---\nname: demo-app-x\n---\n\nb\n"),
        ["app_skill_frontmatter"],
        "description"
    );
    assert_eq!(
        bad("---\ndescription: \"\"\n---\n\nb\n"),
        ["app_skill_frontmatter"]
    );
    assert_eq!(
        bad("---\nname: other\ndescription: d\n---\n\nb\n"),
        ["app_skill_frontmatter"],
        "name != folder"
    );
    assert_eq!(
        bad("---\ndescription: d\n"),
        ["app_skill_frontmatter"],
        "unclosed"
    );
    assert_eq!(
        bad("---\nname: &n demo-app-x\ndescription: *n\n---\n\nb\n"),
        ["app_skill_frontmatter"],
        "anchors"
    );
    let long = "d".repeat(501);
    assert_eq!(
        bad(&format!("---\ndescription: {long}\n---\n\nb\n")),
        ["app_skill_frontmatter"]
    );
    // name absent is fine; a multi-line description is not a single line
    assert!(bad("---\ndescription: d\n---\n\nb\n").is_empty());
    // a skill folder without SKILL.md
    assert_eq!(
        lint(&[f(N, "scripts/a.sh", b"x\n")]),
        ["app_skill_frontmatter"]
    );
}

#[test]
fn file_paths_follow_the_shared_grammar() {
    let with = |rel: &str| lint(&[md(N), f(N, rel, b"x\n")]);
    // traversal can never be an archive path (the reader refuses it); the validator still does
    assert_eq!(with("../x.txt"), ["app_skill_path"]);
    assert_eq!(with("a/b/c/d/e.txt"), ["app_skill_path"], "depth 5");
    assert!(with("a/b/c/d.txt").is_empty(), "depth 4");
    for bad in [
        "Skill.md",
        "skill.md",
        "reference/SKILL.md",
        "skill.md/x",
        "with space.txt",
        "a//b.txt",
        "caf\u{e9}.txt",
    ] {
        assert_eq!(with(bad), ["app_skill_path"], "{bad}");
    }
    let long = format!("{}.txt", "a".repeat(200));
    assert_eq!(with(&long), ["app_skill_path"], "over 200 characters");
}

#[test]
fn files_are_utf8_text_without_nul_or_bom() {
    let with = |bytes: &[u8]| lint(&[md(N), f(N, "scripts/a.sh", bytes)]);
    for bad in [&b"a\0b"[..], b"\xef\xbb\xbfhello", b"\xff\xfe", b"\xc3\x28"] {
        assert_eq!(with(bad), ["app_skill_not_text"], "{bad:?}");
    }
    assert!(
        with(b"#!/bin/sh\r\necho hi\r\n").is_empty(),
        "CR and TAB are text"
    );
    // SKILL.md too
    let nul = lint(&[f(N, "SKILL.md", b"---\ndescription: d\n---\n\nb\0\n")]);
    assert_eq!(nul, ["app_skill_not_text"]);
    let bom = lint(&[f(
        N,
        "SKILL.md",
        "\u{feff}---\ndescription: d\n---\n\nb\n".as_bytes(),
    )]);
    assert_eq!(bom, ["app_skill_not_text"]);
}

#[test]
fn size_and_count_limits() {
    let with = |rel: &str, n: usize| lint(&[md(N), f(N, rel, &vec![b'x'; n])]);
    assert!(
        with("a.txt", 64 * 1024 - 100).contains(&"app_skill_too_large"),
        "total 32000"
    );
    assert_eq!(with("a.txt", 64 * 1024 + 1), ["app_skill_file_too_large"]);
    // body over 24000 characters
    let body = "x".repeat(24_001);
    let big = lint(&[f(N, "SKILL.md", skill_md(N, "d", &body).as_bytes())]);
    assert_eq!(big, ["app_skill_body_too_large"]);
    let body = "x".repeat(24_000);
    assert!(lint(&[f(N, "SKILL.md", skill_md(N, "d", &body).as_bytes())]).is_empty());
    // body + files over 32000 characters
    let body = "x".repeat(20_000);
    let total = lint(&[
        f(N, "SKILL.md", skill_md(N, "d", &body).as_bytes()),
        f(N, "a.txt", &vec![b'y'; 13_000]),
    ]);
    assert_eq!(total, ["app_skill_too_large"]);
    // file count: 50 fine, 51 not
    let files = |n: usize| -> Vec<(String, Vec<u8>)> {
        let mut v = vec![md(N)];
        v.extend((0..n).map(|i| f(N, &format!("r/f{i}.txt"), b"x\n")));
        v
    };
    assert!(lint(&files(50)).is_empty());
    assert_eq!(lint(&files(51)), ["app_skill_too_many_files"]);
}

#[test]
fn per_app_limits() {
    let skills = |n: usize, body: usize| -> Vec<(String, Vec<u8>)> {
        (0..n)
            .map(|i| {
                let name = format!("demo-app-s{i}");
                f(
                    &name,
                    "SKILL.md",
                    skill_md(&name, "d", &"w".repeat(body)).as_bytes(),
                )
            })
            .collect()
    };
    assert!(lint(&skills(16, 10)).is_empty(), "16 skills");
    assert_eq!(lint(&skills(17, 10)), ["app_skills_too_many"]);
    // 16 skills of ~17 KB: each under every skill limit, over 256 KiB together
    assert_eq!(lint(&skills(16, 17_000)), ["app_skills_app_too_large"]);
    assert!(
        lint(&skills(15, 17_000)).is_empty(),
        "15 x 17 KB is under 256 KiB"
    );
}

#[test]
fn old_guide_entries_and_files_are_ignored_not_errors() {
    use aurora_registry::validate::{package_warnings, warnings};
    let y = yaml(TOOLS, LEGACY_AGENT, "");
    // Any content, any names: no error, even files that would have been refused before.
    let files = vec![
        (
            "agent/demo-app/make-and-check.md".to_owned(),
            b"# Make\r\n".to_vec(),
        ),
        ("agent/demo-app/Bad_Name.md".to_owned(), vec![0u8, 1, 2]),
        ("agent/demo-app/api.md".to_owned(), vec![b'x'; 200_000]),
    ];
    assert!(lint_y(&y, &files).is_empty(), "{:?}", lint_y(&y, &files));
    // The warning is `agent_guide_ignored`, once per app, from the manifest alone...
    let m = Manifest::parse(&y, "99.0.0").unwrap();
    let ignored = |w: Vec<aurora_registry::Violation>| -> Vec<&'static str> {
        w.iter()
            .map(|x| x.code)
            .filter(|c| *c == "agent_guide_ignored")
            .collect()
    };
    assert_eq!(ignored(warnings(&m)), ["agent_guide_ignored"]);
    // ...and from files that are still in the archive of a manifest that no longer lists them.
    let y2 = yaml(TOOLS, AGENT, "");
    let m2 = Manifest::parse(&y2, "99.0.0").unwrap();
    assert!(ignored(warnings(&m2)).is_empty());
    let pkg = pkg_of(&files);
    assert_eq!(
        ignored(package_warnings(&m2, &pkg)),
        ["agent_guide_ignored"]
    );
    // The operations file is not a guide file.
    let y3 = yaml(
        TOOLS,
        "    agent:\n      summary: \"S\"\n      operations: operations.json\n",
        "",
    );
    let m3 = Manifest::parse(&y3, "99.0.0").unwrap();
    let ops = pkg_of(&[("agent/demo-app/operations.json".to_owned(), b"{}".to_vec())]);
    assert!(ignored(package_warnings(&m3, &ops)).is_empty());
    // The removed error codes are gone from the linter.
    let removed = [
        "agent_guide_file_name",
        "agent_guide_reserved_name",
        "agent_guide_unlisted_file",
        "agent_guide_missing_file",
        "agent_guide_not_text",
        "agent_guide_file_too_large",
        "agent_guide_app_too_large",
        "agent_guide_too_many_files",
        "agent_guide_kind",
        "agent_guide_title",
        "agent_guide_computed_field",
        "agent_guide_duplicate_file",
        "agent_recipe_not_markdown",
        "agent_guide_unknown_app",
    ];
    let all: Vec<&str> = lint_y(&y, &files);
    assert!(removed.iter().all(|c| !all.contains(c)));
}

#[test]
fn operations_file_rules_keep_their_own_codes() {
    let y = yaml(
        TOOLS,
        "    agent:\n      summary: \"S\"\n      operations: operations.json\n",
        "",
    );
    // missing
    assert_eq!(lint_y(&y, &[]), ["agent_operations_missing_file"]);
    // reserved and malformed names
    for n in ["tools.md", "app.md", "guide.md", "api.json"] {
        let y = yaml(
            TOOLS,
            &format!("    agent:\n      summary: \"S\"\n      operations: {n}\n"),
            "",
        );
        assert!(
            lint_y(&y, &[]).contains(&"agent_operations_reserved_name"),
            "{n}"
        );
    }
    let y = yaml(
        TOOLS,
        "    agent:\n      summary: \"S\"\n      operations: Ops.json\n",
        "",
    );
    assert!(lint_y(&y, &[]).contains(&"agent_operations_file_name"));
    // a file of an app that is not in the package
    let files = vec![("agent/other/operations.json".to_owned(), b"{}".to_vec())];
    assert!(lint_y(&y, &files).contains(&"agent_unknown_app"));
    // not text
    let y = yaml(
        TOOLS,
        "    agent:\n      summary: \"S\"\n      operations: operations.json\n",
        "",
    );
    let files = vec![(
        "agent/demo-app/operations.json".to_owned(),
        b"{\"a\":\"\x1b\"}".to_vec(),
    )];
    assert!(lint_y(&y, &files).contains(&"agent_operations_not_text"));
}

#[test]
fn archive_paths_for_skills_and_agent_files() {
    use aurora_registry::{Limits, builder::PackageBuilder};
    let y = yaml(TOOLS, AGENT, "");
    let read = |path: &str| {
        let mut b = PackageBuilder::new(&y);
        b.set_file(path, b"a\n");
        Package::read(&b.build().unwrap(), &Limits::default())
    };
    assert!(read("skills/demo-app/demo-app-x/SKILL.md").is_ok());
    assert!(read("skills/demo-app/demo-app-x/a/b/c/d.txt").is_ok());
    // agent files stay exactly two levels deep
    assert!(read("agent/demo-app/operations.json").is_ok());
    for deep in [
        "agent/demo-app/sub/api.md",
        "agent/api.md",
        "skills/demo-app/SKILL.md",
        "skills/demo-app/x",
        "skills/a.md",
        "other/x.md",
    ] {
        let r = read(deep);
        assert!(
            r.is_err_and(|v| v.iter().any(|x| x.code == "path_unknown")),
            "{deep}"
        );
    }
}
