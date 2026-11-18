#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! (P5-GUIDE-5, C-650) `aurora plugin import-openapi`: golden output, determinism, the limits and
//! their drop report, the order of the skeleton, hostile documents, and the output pasted into a
//! package that `validate` accepts.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn aurora(args: &[&str]) -> Output {
    let bin = std::env::var_os("NEXTEST_BIN_EXE_aurora")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_aurora")));
    Command::new(bin).args(args).output().unwrap()
}

fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!("aurora-cli-oa-{}", uuid::Uuid::now_v7().simple()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn doc(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/openapi")
        .join(name)
}

struct Imported {
    stdout: String,
    stderr: String,
    files: BTreeMap<String, String>,
}

fn import_in(file: &Path, app: &str, dir: &Path) -> Output {
    aurora(&[
        "plugin",
        "import-openapi",
        s(file),
        "--app",
        app,
        "--host",
        "api.example.com",
        "--dir",
        s(dir),
    ])
}

fn import(file: &Path, app: &str) -> Imported {
    let d = tmp();
    let o = import_in(file, app, &d);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    read(&d, app, &o)
}

fn read(d: &Path, app: &str, o: &Output) -> Imported {
    let mut files = BTreeMap::new();
    for e in std::fs::read_dir(d.join("agent").join(app)).unwrap() {
        let e = e.unwrap();
        files.insert(
            e.file_name().to_string_lossy().into_owned(),
            std::fs::read_to_string(e.path()).unwrap(),
        );
    }
    Imported {
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        files,
    }
}

/// The package the output is pasted into: the scaffold's connection and host, the printed `guide:`
/// entries and `tools:` skeleton.
fn compose(app: &str, stdout: &str) -> String {
    let mut guide = Vec::new();
    let mut tools = Vec::new();
    let mut section = "";
    let mut operations = String::new();
    for l in stdout.lines() {
        if l.starts_with('#') {
            continue;
        }
        if l.starts_with("operations: ") {
            operations = format!("\n      {l}");
            continue;
        }
        if l == "guide:" {
            section = "guide";
        } else if l == "tools:" {
            section = "tools";
        } else if l.is_empty() {
            section = "";
        }
        match section {
            "guide" => guide.push(format!("      {l}")),
            "tools" => tools.push(format!("    {l}")),
            _ => {}
        }
    }
    format!(
        "package: {{ id: {app}, version: 0.1.0, contract: 1, min_core: 0.1.0, publisher: example, name: {app} }}\n\
apps:\n  - about: {{ id: {app}, name: {app} }}\n    credentials: [ {{ name: api_token, secret: true }} ]\n    connect:\n      - id: api_key\n        kind: fields\n        fields: [ {{ name: api_token, label: API token, type: secret, required: true }} ]\n    hosts:\n      - match: {{ host: api.example.com }}\n        inject:\n          - bearer: \"{{credentials.api_token}}\"\n{}\n    agent:\n      summary: \"Imported.\"\n      instructions: \"Imported.\"{}\n{}\n",
        tools.join("\n"),
        operations,
        guide.join("\n")
    )
}

/// Scaffolds a package, imports into it, replaces its manifest with the pasted output, validates.
fn validates_when_pasted(file: &Path, app: &str) -> Imported {
    let d = tmp();
    let o = aurora(&["plugin", "new", app, "--dir", s(&d)]);
    assert!(o.status.success());
    let pkg = d.join(app);
    // The scaffold's own guide file is not part of the pasted manifest.
    let _ = std::fs::remove_file(pkg.join("agent").join(app).join("api.md"));
    let o = import_in(file, app, &pkg);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let got = read(&pkg, app, &o);
    std::fs::write(pkg.join("plugin.yaml"), compose(app, &got.stdout)).unwrap();
    let v = aurora(&["plugin", "validate", s(&pkg), "--allow-unsigned"]);
    let out = String::from_utf8_lossy(&v.stdout).into_owned();
    assert!(
        v.status.success() && out.contains("\"ok\": true"),
        "{out}{}",
        String::from_utf8_lossy(&v.stderr)
    );
    got
}

fn check_golden(name: &str, app: &str, got: &Imported) {
    let golden = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/openapi")
        .join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        let _ = std::fs::remove_dir_all(&golden);
        std::fs::create_dir_all(&golden).unwrap();
        for (n, c) in &got.files {
            std::fs::write(golden.join(n), c).unwrap();
        }
        std::fs::write(golden.join("stdout.txt"), &got.stdout).unwrap();
        return;
    }
    let mut want: BTreeMap<String, String> = BTreeMap::new();
    for e in std::fs::read_dir(&golden).unwrap() {
        let e = e.unwrap();
        want.insert(
            e.file_name().to_string_lossy().into_owned(),
            std::fs::read_to_string(e.path()).unwrap(),
        );
    }
    let stdout = want.remove("stdout.txt").unwrap();
    assert_eq!(stdout, got.stdout, "{app}: stdout differs from golden");
    assert_eq!(
        want.keys().collect::<Vec<_>>(),
        got.files.keys().collect::<Vec<_>>(),
        "{app}: file set differs from golden"
    );
    for (n, c) in want {
        assert_eq!(c, got.files[&n], "{app}/{n} differs from golden");
    }
}

#[test]
fn small_and_tagged_documents_match_their_golden_output_and_are_deterministic() {
    for (file, app, golden) in [
        ("small.json", "widgets", "small"),
        ("tagged.yaml", "tagged", "tagged"),
    ] {
        let a = import(&doc(file), app);
        let b = import(&doc(file), app);
        assert_eq!(a.files, b.files, "{file}: two runs differ");
        assert_eq!(a.stdout, b.stdout, "{file}: two runs differ");
        check_golden(golden, app, &a);
    }
}

#[test]
fn the_output_pasted_into_a_package_passes_validate() {
    validates_when_pasted(&doc("small.json"), "widgets");
    validates_when_pasted(&doc("tagged.yaml"), "tagged");
}

/// `/a/search` is a literal path and `/a/{id}` a wildcard one: the literal one comes first, and
/// the linter's shadowing proof accepts the result (`validates_when_pasted`).
#[test]
fn the_skeleton_puts_the_more_specific_path_first() {
    let got = validates_when_pasted(&doc("tagged.yaml"), "tagged");
    let search = got.stdout.find("\"/a/search\"").unwrap();
    let wild = got.stdout.find("\"/a/*\"").unwrap();
    assert!(search < wild, "{}", got.stdout);
    // Reads before writes; ids are lower snake case from `operationId`; one without one is derived.
    assert!(got.stdout.contains("id: get_a,") && got.stdout.contains("id: get_reports_daily,"));
    let reads = got.stdout.find("  read:").unwrap();
    let writes = got.stdout.find("  write:").unwrap();
    assert!(reads < writes);
}

fn large_document() -> String {
    let mut paths = String::new();
    for t in 0..12 {
        for i in 0..125 {
            if !paths.is_empty() {
                paths.push(',');
            }
            paths.push_str(&format!(
                r#""/t{t}/res{i}/{{id}}": {{ "post": {{ "operationId": "op{t}x{i}", "summary": "Operation number {i} of the tag number {t}, with a summary",
 "tags": ["Tag{t}"],
 "parameters": [
  {{"name": "id", "in": "path", "required": true, "schema": {{"type": "string"}}, "description": "{d}"}},
  {{"name": "a", "in": "query", "schema": {{"type": "string"}}, "description": "{d}"}},
  {{"name": "b", "in": "query", "schema": {{"type": "integer"}}, "description": "{d}"}}],
 "requestBody": {{"content": {{"application/json": {{"schema": {{"properties": {{
  "f1": {{"type": "string", "description": "{d}"}}, "f2": {{"type": "string", "description": "{d}"}},
  "f3": {{"type": "string", "description": "{d}"}}, "f4": {{"type": "string", "description": "{d}"}}}}}}}}}}}} }} }}"#,
                d = "d".repeat(150)
            ));
        }
    }
    format!(
        r#"{{"openapi": "3.0.0", "info": {{"title": "L", "version": "1"}}, "servers": [{{"url": "https://l.example.com"}}], "paths": {{{paths}}}}}"#
    )
}

#[test]
fn a_large_document_drops_detail_reports_it_and_stays_within_every_limit() {
    let d = tmp();
    let f = d.join("large.json");
    std::fs::write(&f, large_document()).unwrap();
    let o = import_in(&f, "large", &d.join("p1"));
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let a = read(&d.join("p1"), "large", &o);
    assert!(a.stderr.contains("import-openapi: dropped"), "{}", a.stderr);
    // (C-656) The operations file has its own limit and is not a guide file.
    let ops = a.files.get("operations.json").expect("operations.json");
    assert!(ops.len() <= 512 * 1024, "{}", ops.len());
    assert!(
        a.stderr.contains("operations.json: dropped"),
        "{}",
        a.stderr
    );
    let guide_files: Vec<&String> = a
        .files
        .iter()
        .filter(|(n, _)| *n != "operations.json")
        .map(|(_, c)| c)
        .collect();
    assert!(guide_files.len() <= 24, "{}", guide_files.len());
    assert!(guide_files.iter().all(|c| c.len() <= 64 * 1024));
    assert!(guide_files.iter().map(|c| c.len()).sum::<usize>() <= 256 * 1024);
    // Every operation is still in the index.
    let index: String = a
        .files
        .iter()
        .filter(|(n, _)| n.starts_with("operations"))
        .map(|(_, c)| c.as_str())
        .collect();
    assert_eq!(index.matches("\n- POST ").count(), 12 * 125);
    // Deterministic, and it validates once pasted (the skeleton has 1 500 tools).
    let o2 = import_in(&f, "large", &d.join("p2"));
    let b = read(&d.join("p2"), "large", &o2);
    assert_eq!(a.files, b.files);
    assert_eq!(a.stdout, b.stdout);
    validates_when_pasted(&f, "large");
}

#[test]
fn bad_inputs_exit_non_zero_with_one_line_reasons() {
    let d = tmp();
    let cases = [
        (
            r##"{"openapi":"3.0.0","paths":{"/x":{"get":{"parameters":[{"$ref":"other.yaml#/p"}]}}}}"##,
            "external $ref",
        ),
        (r#"{"swagger":"2.0","paths":{}}"#, "Swagger 2.0"),
        (r#"{"paths":{}}"#, "not an OpenAPI 3"),
        ("{ not: [valid", "not JSON or YAML"),
        (r#"{"openapi":"3.0.0","paths":{}}"#, "no operations"),
    ];
    for (i, (text, reason)) in cases.iter().enumerate() {
        let f = d.join(format!("bad{i}.json"));
        std::fs::write(&f, text).unwrap();
        let o = import_in(&f, "bad", &d.join(format!("out{i}")));
        assert!(!o.status.success(), "{text}");
        let e = String::from_utf8_lossy(&o.stderr);
        assert!(e.contains(reason), "{reason}: {e}");
        assert!(e.trim().lines().count() <= 3, "{e}");
    }
    let o = import_in(&d.join("missing.json"), "bad", &d.join("x"));
    assert!(!o.status.success());
    let o = import_in(&doc("small.json"), "Bad_ID", &d.join("y"));
    assert!(!o.status.success());
}

/// HTML, backticks, control characters, a heading smuggled into a summary, a 5 000-character
/// summary: the files stay text on their lines, pass `agent_guide_not_text` and validate.
#[test]
fn hostile_strings_stay_inside_their_lines() {
    let d = tmp();
    let text = std::fs::read_to_string(doc("hostile.json"))
        .unwrap()
        .replace("SUMMARY_PLACEHOLDER", &"x".repeat(5_000));
    let f = d.join("hostile.json");
    std::fs::write(&f, text).unwrap();
    let got = validates_when_pasted(&f, "hostile");
    for (n, c) in &got.files {
        assert!(
            !c.contains('\r') && !c.contains('\u{7}') && !c.contains('\u{202e}'),
            "{n}"
        );
        assert!(
            !c.contains("<script") && !c.contains("<b>") && !c.contains("<img"),
            "{n}\n{c}"
        );
        for l in c.lines() {
            assert!(!l.starts_with("# Instructions"), "{n}: {l}");
            assert!(l.len() < 600, "{n}: line of {} bytes", l.len());
            // Backticks only where the generator writes its own code spans: balanced per line.
            assert_eq!(l.matches('`').count() % 2, 0, "{n}: {l}");
        }
    }
}
