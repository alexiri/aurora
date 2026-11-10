//! P4-GSL (D-148, D-149; SEC-84): the `library` manifest block: linter matrix, snapshot copy,
//! re-approval on change, parameter grammar, request building and response extractors, and the
//! built-in GitHub and GitLab packages.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::collections::BTreeSet;

use aurora_registry::diff::{Widening, widens};
use aurora_registry::library::{self, Params, Step};
use aurora_registry::manifest::Library;
use aurora_registry::validate;
use aurora_registry::{Manifest, Violation};

const HEAD: &str = r#"
package: { id: forge, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: aurora, name: Forge }
apps:
  - about: { id: forge, name: Forge }
    credentials: [ { name: access_token, secret: true } ]
    connect:
      - id: token
        kind: fields
        fields: [ { name: access_token, label: Token, type: secret, required: true } ]
    hosts:
      - match: { host: api.forge.example }
        inject:
          - bearer: "{credentials.access_token}"
"#;

const BLOCK: &str = r#"    library:
      repo_pattern: "^[A-Za-z0-9_.-]{1,100}/[A-Za-z0-9_.-]{1,100}$"
      request_host: api.forge.example
      resolve_ref:
        path: "/repos/{repo}/commits/{ref}"
        default_path: "/repos/{repo}"
        sha: "$.sha"
        default_branch: "$.default_branch"
      list_tree:
        path: "/repos/{repo}/git/trees/{sha}?recursive=1"
        entries: "$.tree[*]"
        fields: { path: "$.path", type: "$.type", mode: "$.mode", blob: "$.sha", size: "$.size" }
        file_type: blob
        truncated: "$.truncated"
        paginate: none
      get_blob:
        path: "/repos/{repo}/git/blobs/{blob}"
        content: "$.content"
        encoding: "$.encoding"
      required_access: read
"#;

fn manifest_with(block: &str) -> String {
    format!("{HEAD}{block}")
}

fn codes(yaml: &str) -> BTreeSet<&'static str> {
    match Manifest::parse(yaml, "0.1.0") {
        Err(v) => v.iter().map(|x| x.code).collect(),
        Ok(m) => validate::manifest(&m).iter().map(|x| x.code).collect(),
    }
}

fn violations(yaml: &str) -> Vec<Violation> {
    match Manifest::parse(yaml, "0.1.0") {
        Err(v) => v,
        Ok(m) => validate::manifest(&m),
    }
}

fn lib() -> Library {
    Manifest::parse(&manifest_with(BLOCK), "0.1.0")
        .unwrap()
        .apps[0]
        .library
        .clone()
        .unwrap()
}

/// The block with `from` replaced by `to` (every test mutates one line).
fn mutated(from: &str, to: &str) -> String {
    assert!(BLOCK.contains(from), "{from}");
    manifest_with(&BLOCK.replacen(from, to, 1))
}

#[test]
fn a_valid_block_lints_clean_and_absent_means_no_library() {
    assert!(
        codes(&manifest_with(BLOCK)).is_empty(),
        "{:?}",
        violations(&manifest_with(BLOCK))
    );
    let m = Manifest::parse(HEAD, "0.1.0").unwrap();
    assert!(m.apps[0].library.is_none());
    assert!(codes(HEAD).is_empty());
}

#[test]
fn linter_matrix() {
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "wildcard host",
            mutated(
                "request_host: api.forge.example",
                "request_host: \"*.forge.example\"",
            ),
            "library_host_not_exact",
        ),
        (
            "host not in hosts",
            mutated(
                "request_host: api.forge.example",
                "request_host: other.example",
            ),
            "library_host_not_exact",
        ),
        (
            "host with a path",
            mutated(
                "request_host: api.forge.example",
                "request_host: api.forge.example/x",
            ),
            "library_host_not_exact",
        ),
        (
            "instance host without instances",
            mutated(
                "request_host: api.forge.example",
                "request_host: \"{instance}\"",
            ),
            "library_host_not_exact",
        ),
        (
            "free placeholder",
            mutated(
                "/repos/{repo}/commits/{ref}",
                "/repos/{repo}/commits/{ref}/{owner}",
            ),
            "library_path_placeholder",
        ),
        (
            "placeholder in the wrong step",
            mutated(
                "/repos/{repo}/git/blobs/{blob}",
                "/repos/{repo}/git/blobs/{sha}",
            ),
            "library_path_placeholder",
        ),
        (
            "path without a slash",
            mutated(
                "/repos/{repo}/git/blobs/{blob}",
                "repos/{repo}/git/blobs/{blob}",
            ),
            "library_path_placeholder",
        ),
        (
            "protocol-relative path",
            mutated(
                "/repos/{repo}/git/blobs/{blob}",
                "//evil.example/{repo}/{blob}",
            ),
            "library_path_placeholder",
        ),
        (
            "literal traversal",
            mutated("/repos/{repo}/git/blobs/{blob}", "/repos/../{repo}/{blob}"),
            "library_path_placeholder",
        ),
        (
            "fragment",
            mutated(
                "/repos/{repo}/git/blobs/{blob}",
                "/repos/{repo}/git/blobs/{blob}#x",
            ),
            "library_path_placeholder",
        ),
        (
            "templated page",
            mutated("?recursive=1", "?recursive=1&page=2"),
            "library_path_placeholder",
        ),
        (
            "missing placeholder",
            mutated(
                "/repos/{repo}/git/blobs/{blob}",
                "/repos/{repo}/git/blobs/x",
            ),
            "library_path_placeholder",
        ),
        (
            "unanchored pattern",
            mutated(
                "\"^[A-Za-z0-9_.-]{1,100}/[A-Za-z0-9_.-]{1,100}$\"",
                "\"[A-Za-z0-9_.-]{1,100}/[A-Za-z0-9_.-]{1,100}\"",
            ),
            "library_repo_pattern",
        ),
        (
            "pattern that accepts anything",
            mutated(
                "\"^[A-Za-z0-9_.-]{1,100}/[A-Za-z0-9_.-]{1,100}$\"",
                "\"^.+$\"",
            ),
            "library_repo_pattern",
        ),
        (
            "invalid regex",
            mutated(
                "\"^[A-Za-z0-9_.-]{1,100}/[A-Za-z0-9_.-]{1,100}$\"",
                "\"^(\"",
            ),
            "library_repo_pattern",
        ),
        (
            "recursive descent",
            mutated("\"$.sha\"", "\"$..sha\""),
            "library_json_path",
        ),
        (
            "filter",
            mutated("entries: \"$.tree[*]\"", "entries: \"$.tree[?(@.type)]\""),
            "library_json_path",
        ),
        (
            "not anchored",
            mutated("content: \"$.content\"", "content: \"content\""),
            "library_json_path",
        ),
        (
            "too many wildcards",
            mutated(
                "entries: \"$.tree[*]\"",
                "entries: \"$.a[*].b[*].c[*].d[*]\"",
            ),
            "library_json_path",
        ),
        (
            "pagination without max_pages",
            mutated("paginate: none", "paginate: link_header"),
            "library_json_path",
        ),
        (
            "non-GET",
            mutated(
                "        sha: \"$.sha\"",
                "        method: POST\n        sha: \"$.sha\"",
            ),
            "library_method",
        ),
        (
            "unknown key",
            mutated(
                "required_access: read",
                "required_access: read\n      headers: { x: y }",
            ),
            "unknown_field",
        ),
        (
            "unknown access",
            mutated("required_access: read", "required_access: write"),
            "unknown_variant",
        ),
    ];
    for (name, yaml, want) in cases {
        let got = codes(&yaml);
        assert!(
            got.contains(want),
            "{name}: wanted {want}, got {got:?}\n{:?}",
            violations(&yaml)
        );
    }
}

#[test]
fn the_host_needs_an_inject_stage_that_covers_the_request() {
    let no_inject = manifest_with(BLOCK).replace(
        "        inject:\n          - bearer: \"{credentials.access_token}\"\n",
        "",
    );
    assert!(
        codes(&no_inject).contains("library_no_credential"),
        "{:?}",
        codes(&no_inject)
    );
    // A rule whose path_prefix does not cover the templates does not authenticate them.
    let narrow = manifest_with(BLOCK).replace(
        "- match: { host: api.forge.example }",
        "- match: { host: api.forge.example, path_prefix: /api/v9 }",
    );
    assert!(codes(&narrow).contains("library_no_credential"));
    // rewrite_host is refused.
    let rewritten = manifest_with(BLOCK).replace(
        "- match: { host: api.forge.example }",
        "- match: { host: api.forge.example }\n        rewrite_host: { template: \"{connection.tenant_host}\", zone: \"*.forge.example\" }",
    );
    assert!(codes(&rewritten).contains("library_host_not_exact"));
}

#[test]
fn encoding_rules() {
    let raw = mutated(
        "        content: \"$.content\"\n        encoding: \"$.encoding\"",
        "        content: \"$.content\"\n        encoding: raw",
    );
    assert!(
        codes(&raw).contains("library_json_path"),
        "raw endpoint with a content path"
    );
    let missing = mutated(
        "        content: \"$.content\"\n        encoding: \"$.encoding\"",
        "        encoding: base64",
    );
    assert!(
        codes(&missing).contains("library_json_path"),
        "JSON blob without a content path"
    );
}

#[test]
fn widening_on_add_and_on_change_but_not_when_identical() {
    let old = Manifest::parse(HEAD, "0.1.0").unwrap();
    let new = Manifest::parse(&manifest_with(BLOCK), "0.1.0").unwrap();
    assert_eq!(
        widens(&old, &new),
        vec![Widening::Library {
            app: "forge".into(),
            host: "api.forge.example".into()
        }]
    );
    assert!(widens(&new, &new).is_empty());
    let changed = Manifest::parse(
        &mutated(
            "/repos/{repo}/commits/{ref}",
            "/v2/repos/{repo}/commits/{ref}",
        ),
        "0.1.0",
    )
    .unwrap();
    assert_eq!(widens(&new, &changed).len(), 1);
    // Dropping the block is narrowing.
    assert!(widens(&new, &old).is_empty());
}

#[test]
fn the_block_is_copied_into_the_snapshot() {
    use aurora_registry::snapshot::{ActivePackage, compile};
    let m = Manifest::parse(&manifest_with(BLOCK), "0.1.0").unwrap();
    let c = compile(
        &[ActivePackage {
            manifest: m,
            digest: "d".into(),
            publisher: "aurora".into(),
            hooks_digest: None,
        }],
        &[],
        &[],
        1,
    )
    .unwrap();
    let j: serde_json::Value = serde_json::from_slice(&c.json).unwrap();
    assert_eq!(j["apps"][0]["library"]["request_host"], "api.forge.example");
    let back: Library = serde_json::from_value(j["apps"][0]["library"].clone()).unwrap();
    assert_eq!(back, lib());
    // an app without the block serializes none
    let c = compile(
        &[ActivePackage {
            manifest: Manifest::parse(HEAD, "0.1.0").unwrap(),
            digest: "d".into(),
            publisher: "aurora".into(),
            hooks_digest: None,
        }],
        &[],
        &[],
        1,
    )
    .unwrap();
    let j: serde_json::Value = serde_json::from_slice(&c.json).unwrap();
    assert!(j["apps"][0].get("library").is_none());
}

// ---- runtime: parameters and request building -----------------------------------------------

fn params(repo: &str) -> Params {
    Params {
        repo: repo.into(),
        ..Params::default()
    }
}

#[test]
fn requests_are_built_from_the_template_only() {
    let l = lib();
    let mut p = params("acme/skills");
    assert_eq!(
        library::build(&l, Step::ResolveRef, &p).unwrap(),
        "/repos/acme/skills"
    );
    p.reference = Some("release/1.2".into());
    assert_eq!(
        library::build(&l, Step::ResolveRef, &p).unwrap(),
        "/repos/acme/skills/commits/release/1.2"
    );
    p.sha = Some("a".repeat(40));
    assert_eq!(
        library::build(&l, Step::ListTree, &p).unwrap(),
        format!(
            "/repos/acme/skills/git/trees/{}?recursive=1",
            "a".repeat(40)
        )
    );
    p.page = Some(3);
    assert!(
        library::build(&l, Step::ListTree, &p)
            .unwrap()
            .ends_with("?recursive=1&page=3")
    );
    p.blob = Some("b".repeat(64));
    p.page = None;
    assert_eq!(
        library::build(&l, Step::GetBlob, &p).unwrap(),
        format!("/repos/acme/skills/git/blobs/{}", "b".repeat(64))
    );
}

#[test]
fn parameter_fuzzing_is_refused() {
    let l = lib();
    for repo in [
        "",
        "acme",
        "../x",
        "acme/..",
        "acme/../x",
        "./a",
        "acme/skills/extra",
        "acme/skills?x=1",
        "acme/skills#f",
        "acme/%2e%2e",
        "acme/%2E%2E",
        "acme/sk\r\nHost: evil",
        "acme/skills ",
        "acme\\skills",
        "ac\u{43c}e/skills",
        "\u{ff0e}\u{ff0e}/x",
        "-acme/skills",
        &format!("{}/x", "a".repeat(300)),
    ] {
        assert!(
            library::build(&l, Step::ResolveRef, &params(repo)).is_err(),
            "repo {repo:?}"
        );
    }
    for r in [
        "",
        "..",
        "a..b",
        "../x",
        "-x",
        "/x",
        "x/",
        "a//b",
        "a/./b",
        "a b",
        "a?b",
        "a#b",
        "a%2e%2e",
        "a\r\nb",
        "a\u{0}b",
        "\u{ff0e}\u{ff0e}",
        &"a".repeat(201),
    ] {
        let mut p = params("acme/skills");
        p.reference = Some(r.into());
        assert!(
            library::build(&l, Step::ResolveRef, &p).is_err(),
            "ref {r:?}"
        );
    }
    for id in [
        "",
        "ABC",
        &"g".repeat(40),
        &"a".repeat(39),
        &"a".repeat(65),
        "../..",
        &format!("{}/", "a".repeat(40)),
    ] {
        let mut p = params("acme/skills");
        p.sha = Some(id.into());
        assert!(
            library::build(&l, Step::ListTree, &p).is_err(),
            "sha {id:?}"
        );
        let mut p = params("acme/skills");
        p.blob = Some(id.into());
        assert!(
            library::build(&l, Step::GetBlob, &p).is_err(),
            "blob {id:?}"
        );
    }
    for page in [0, 1, 201, u32::MAX] {
        let mut p = params("acme/skills");
        p.sha = Some("a".repeat(40));
        p.page = Some(page);
        assert!(
            library::build(&l, Step::ListTree, &p).is_err(),
            "page {page}"
        );
    }
    // A step needing a parameter that is absent fails closed.
    assert!(library::build(&l, Step::GetBlob, &params("acme/skills")).is_err());
}

#[test]
fn urlencode_placeholders_encode_everything() {
    let mut l = lib();
    l.resolve_ref.path = "/projects/{repo|urlencode}/commits/{ref|urlencode}".into();
    let mut p = params("grp/proj");
    p.reference = Some("feature/x".into());
    assert_eq!(
        library::build(&l, Step::ResolveRef, &p).unwrap(),
        "/projects/grp%2Fproj/commits/feature%2Fx"
    );
}

// ---- runtime: extractors --------------------------------------------------------------------

#[test]
fn github_shaped_responses() {
    let l = lib();
    let sha = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(
        library::resolved_sha(&l, format!(r#"{{"sha":"{sha}"}}"#).as_bytes()).unwrap(),
        sha
    );
    assert!(library::resolved_sha(&l, br#"{"sha":"../etc"}"#).is_err());
    assert!(library::resolved_sha(&l, b"<html>").is_err());
    assert_eq!(
        library::default_branch(&l, br#"{"default_branch":"main"}"#).unwrap(),
        "main"
    );
    assert!(library::default_branch(&l, br#"{"default_branch":"a b"}"#).is_err());

    let tree = br#"{"truncated":true,"tree":[
        {"path":"s/SKILL.md","type":"blob","mode":"100644","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size":12},
        {"path":"s","type":"tree","mode":"040000","sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},
        {"path":"link","type":"blob","mode":"120000","sha":"cccccccccccccccccccccccccccccccccccccccc","size":3},
        {"path":"sub","type":"commit","mode":"160000","sha":"dddddddddddddddddddddddddddddddddddddddd"},
        {"type":"blob"}]}"#;
    let page = library::tree_page(&l, tree).unwrap();
    assert!(page.truncated);
    assert_eq!(page.entries.len(), 4);
    assert!(page.entries[0].is_file_kind(&l) && page.entries[0].size == Some(12));
    assert!(!page.entries[1].is_file_kind(&l));
    assert_eq!(page.entries[2].mode, "120000");
    assert_eq!(page.entries[3].kind, "commit");

    let blob = br#"{"content":"aGVsbG8K\nd29ybGQ=","encoding":"base64"}"#;
    assert_eq!(library::blob_bytes(&l, blob).unwrap(), b"hello\nworld");
    assert!(library::blob_bytes(&l, br#"{"content":"x","encoding":"gzip"}"#).is_err());
    assert!(library::blob_bytes(&l, br#"{"content":"!!!","encoding":"base64"}"#).is_err());
}

// ---- the built-in packages ------------------------------------------------------------------

fn builtin(id: &str) -> Manifest {
    let p = format!(
        "{}/../../plugins/builtin/{id}/plugin.yaml",
        env!("CARGO_MANIFEST_DIR")
    );
    Manifest::parse(&std::fs::read_to_string(p).unwrap(), "9.9.9").unwrap()
}

#[test]
fn builtin_github_and_gitlab_declare_a_valid_library() {
    for id in ["github", "gitlab"] {
        let m = builtin(id);
        let v = validate::manifest(&m);
        assert!(v.is_empty(), "{id}: {v:?}");
        let with: Vec<_> = m
            .apps
            .iter()
            .filter(|a| a.library.is_some())
            .map(|a| a.about.id.as_str())
            .collect();
        assert!(!with.is_empty(), "{id} has no library block");
    }
    let gh = builtin("github");
    assert_eq!(
        gh.apps[0].library.as_ref().unwrap().request_host,
        "api.github.com"
    );
    assert_eq!(
        gh.apps[1].library.as_ref().unwrap().request_host,
        "{instance}"
    );
    // widening versus a package without the block
    let mut bare = gh.clone();
    for a in &mut bare.apps {
        a.library = None;
    }
    assert_eq!(widens(&bare, &gh).len(), 2);
}

#[test]
fn gitlab_shape_paginated_tree_encoded_project_and_raw_blob() {
    let l = builtin("gitlab").apps[0].library.clone().unwrap();
    let mut p = params("group/sub/project");
    p.reference = Some("main".into());
    assert_eq!(
        library::build(&l, Step::ResolveRef, &p).unwrap(),
        "/api/v4/projects/group%2Fsub%2Fproject/repository/commits/main"
    );
    assert_eq!(
        library::build(&l, Step::ResolveRef, &params("group/sub/project")).unwrap(),
        "/api/v4/projects/group%2Fsub%2Fproject"
    );
    p.sha = Some("a".repeat(40));
    p.page = Some(2);
    let t = library::build(&l, Step::ListTree, &p).unwrap();
    assert!(
        t.starts_with("/api/v4/projects/group%2Fsub%2Fproject/repository/tree?recursive=true"),
        "{t}"
    );
    assert!(t.ends_with("&page=2"), "{t}");
    let tree = br#"[{"id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","name":"SKILL.md","type":"blob","path":"s/SKILL.md","mode":"100644"},
                    {"id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","name":"s","type":"tree","path":"s","mode":"040000"}]"#;
    let page = library::tree_page(&l, tree).unwrap();
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.entries[0].blob, "a".repeat(40));
    assert!(!page.truncated);
    assert_eq!(library::blob_bytes(&l, b"raw bytes").unwrap(), b"raw bytes");
}
