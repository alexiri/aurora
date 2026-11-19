//! (phase 5, P5-WEBHOOK-1, D-320, C-909, SEC-127) The send mode of `secret_link`: the manifest
//! rules and their linter codes, and the preset files of the archive.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::collections::BTreeSet;

use aurora_registry::manifest::SecretLinkMode;
use aurora_registry::webhook::{archive_rules, presets};
use aurora_registry::{Manifest, Package, Violation, validate};

fn manifest(credentials: &str, section: &str, tools: &str) -> String {
    format!(
        r#"package: {{ id: hook-pkg, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: aurora, name: Hook }}
apps:
  - about: {{ id: hook, name: Hook }}
    credentials:
{credentials}
    metadata:
      - {{ name: host }}
      - {{ name: name }}
    connect:
      - id: link
        kind: fields
        fields:
          - {{ name: name, label: Name, type: text, required: true, store: metadata }}
          - {{ name: url, label: URL, type: secret, required: true, store: credentials }}
{section}
    tools:
{tools}
"#
    )
}

const CREDS: &str = "      - { name: url, secret: true }";
const SECTION: &str = "    secret_link: { credential: url, mode: send, presets: [slack.json] }";
const TOOLS: &str = "      write:\n        items:\n          - { id: send, name: Send, host: \"*.link.aurora.internal\", path: /, methods: [POST] }";

fn run(y: &str) -> Vec<Violation> {
    match Manifest::parse(y, "0.1.0") {
        Err(v) => v,
        Ok(m) => validate::manifest(&m),
    }
}

fn codes(y: &str) -> BTreeSet<&'static str> {
    run(y).iter().map(|v| v.code).collect()
}

#[test]
fn a_well_formed_send_mode_app_is_valid_and_reaches_the_snapshot_model() {
    let y = manifest(CREDS, SECTION, TOOLS);
    let v = run(&y);
    assert!(v.is_empty(), "{v:#?}");
    let m = Manifest::parse(&y, "0.1.0").unwrap();
    let s = m.apps[0].secret_link.as_ref().unwrap();
    assert_eq!(s.mode(), SecretLinkMode::Send);
    assert_eq!(s.presets.as_deref(), Some(&["slack.json".to_owned()][..]));
    // PUT is fine, and so are both.
    let both = TOOLS.replace("[POST]", "[POST, PUT]");
    assert!(run(&manifest(CREDS, SECTION, &both)).is_empty());
}

#[test]
fn fetch_mode_is_the_default_and_unchanged() {
    let y = manifest(
        CREDS,
        "    secret_link: { credential: url, expect_prefix: \"BEGIN:VCALENDAR\" }",
        "      read:\n        items:\n          - { id: read, name: Read, host: \"*.link.aurora.internal\", path: /, methods: [GET] }",
    );
    assert!(run(&y).is_empty(), "{:?}", run(&y));
    let m = Manifest::parse(&y, "0.1.0").unwrap();
    assert_eq!(
        m.apps[0].secret_link.as_ref().unwrap().mode(),
        SecretLinkMode::Fetch
    );
    // `mode: fetch` spelled out is the same thing.
    let y2 = y.replace("credential: url,", "credential: url, mode: fetch,");
    assert!(run(&y2).is_empty());
}

#[test]
fn each_c909_linter_code_is_refused() {
    // secret_link_mode: another declared credential beside the URL.
    let two = format!("{CREDS}\n      - {{ name: key, secret: true }}");
    assert!(codes(&manifest(&two, SECTION, TOOLS)).contains("secret_link_mode"));

    // secret_link_send_expect_prefix.
    let sec = "    secret_link: { credential: url, mode: send, expect_prefix: \"x\" }";
    assert!(codes(&manifest(CREDS, sec, TOOLS)).contains("secret_link_send_expect_prefix"));

    // secret_link_presets_mode: presets in fetch mode.
    let sec = "    secret_link: { credential: url, presets: [slack.json] }";
    let read = "      read:\n        items:\n          - { id: read, name: Read, host: \"*.link.aurora.internal\", path: /, methods: [GET] }";
    assert!(codes(&manifest(CREDS, sec, read)).contains("secret_link_presets_mode"));

    // secret_link_send_tool: a read tool, GET, a wrong host or path, two tools, no tool.
    let sec = "    secret_link: { credential: url, mode: send }";
    for bad in [
        read.to_owned(),
        TOOLS.replace("[POST]", "[GET]"),
        TOOLS.replace("[POST]", "[POST, DELETE]"),
        TOOLS.replace("*.link.aurora.internal", "example.com"),
        TOOLS.replace("path: /,", "path: /x,"),
        format!(
            "{TOOLS}\n          - {{ id: send2, name: Send 2, host: \"*.link.aurora.internal\", path: /, methods: [POST] }}"
        ),
    ] {
        assert!(
            codes(&manifest(CREDS, sec, &bad)).contains("secret_link_send_tool"),
            "{bad}"
        );
    }
    let none = manifest(CREDS, sec, "      write:\n        items: []");
    assert!(codes(&none).contains("secret_link_send_tool"));

    // webhook_preset_invalid: bad names, duplicates, too many.
    for list in [
        "[Slack.json]",
        "[slack]",
        "[a/b.json]",
        "[slack.json, slack.json]",
        "[../x.json]",
    ] {
        let sec = format!("    secret_link: {{ credential: url, mode: send, presets: {list} }}");
        assert!(
            codes(&manifest(CREDS, &sec, TOOLS)).contains("webhook_preset_invalid"),
            "{list}"
        );
    }
    let many: Vec<String> = (0..21).map(|i| format!("p{i}.json")).collect();
    let sec = format!(
        "    secret_link: {{ credential: url, mode: send, presets: [{}] }}",
        many.join(", ")
    );
    assert!(codes(&manifest(CREDS, &sec, TOOLS)).contains("webhook_preset_invalid"));

    // An unknown mode is a parse error, not a silent fetch.
    let sec = "    secret_link: { credential: url, mode: post }";
    assert!(Manifest::parse(&manifest(CREDS, sec, TOOLS), "0.1.0").is_err());
}

#[test]
fn the_send_tool_does_not_need_a_read_tool_shape_and_fetch_rules_do_not_apply() {
    // In fetch mode a `write` tool is refused as before.
    let sec = "    secret_link: { credential: url }";
    assert!(codes(&manifest(CREDS, sec, TOOLS)).contains("secret_link_tool"));
}

// ---- preset files --------------------------------------------------------------------------

fn preset(id: &str, template: &str, slots: &str, secrets: &str) -> String {
    format!(
        r#"{{"id":"{id}","name":"Slack","suggest_for_hosts":["hooks.slack.com"],"secret_value_names":{secrets},
"config":{{"version":1,"method":"POST","content_type":"application/json","template":{template:?},"slots":{slots}}}}}"#
    )
}

const SLACK_SLOTS: &str =
    r#"[{"name":"text","type":"string","required":true,"description":"The message"}]"#;

fn lint(files: &[(&str, String)]) -> Vec<Violation> {
    let m = Manifest::parse(&manifest(CREDS, SECTION, TOOLS), "0.1.0").unwrap();
    let mut pkg = Package {
        files: Default::default(),
        signature: None,
    };
    for (f, c) in files {
        pkg.files
            .insert(format!("presets/{f}"), c.clone().into_bytes());
    }
    let mut v = Vec::new();
    archive_rules(&m, &pkg, &mut v);
    v
}

#[test]
fn a_valid_preset_passes_and_is_loaded() {
    let good = preset("slack", r#"{"text":{{text}}}"#, SLACK_SLOTS, "[]");
    assert!(lint(&[("slack.json", good.clone())]).is_empty());
    let m = Manifest::parse(&manifest(CREDS, SECTION, TOOLS), "0.1.0").unwrap();
    let mut pkg = Package {
        files: Default::default(),
        signature: None,
    };
    pkg.files
        .insert("presets/slack.json".into(), good.into_bytes());
    let loaded = presets(&m, &pkg);
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].0, "hook");
    assert_eq!(loaded[0].1[0].id, "slack");
    assert_eq!(loaded[0].1[0].suggest_for_hosts, ["hooks.slack.com"]);
}

#[test]
fn a_bad_preset_is_webhook_preset_invalid() {
    let cases = [
        // The config fails C-909 validation: an unknown placeholder, an unused slot, bad JSON.
        preset("slack", r#"{"text":{{nope}}}"#, SLACK_SLOTS, "[]"),
        preset("slack", r#"{"text":"x"}"#, SLACK_SLOTS, "[]"),
        preset("slack", r#"{"text":"#, SLACK_SLOTS, "[]"),
        // A secret value name the template does not use.
        preset("slack", r#"{"text":{{text}}}"#, SLACK_SLOTS, r#"["topic"]"#),
        // Shape: id grammar, not JSON, an unknown field.
        preset("Slack!", r#"{"text":{{text}}}"#, SLACK_SLOTS, "[]"),
        "not json".to_owned(),
        r#"{"id":"slack","name":"S","secret_value_names":[],"extra":1,"config":{}}"#.to_owned(),
        preset("slack", r#"{"text":{{text}}}"#, SLACK_SLOTS, "[]")
            .replace("hooks.slack.com", "Not A Host"),
    ];
    for c in cases {
        let v = lint(&[("slack.json", c.clone())]);
        assert_eq!(v.len(), 1, "{c}: {v:?}");
        assert_eq!(v[0].code, "webhook_preset_invalid", "{c}");
        assert_eq!(v[0].path, "presets/slack.json");
    }
}

#[test]
fn listed_and_present_must_agree_and_ids_are_unique() {
    // Listed but absent.
    let v = lint(&[]);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].code, "webhook_preset_invalid");
    // Present but unlisted.
    let good = preset("slack", r#"{"text":{{text}}}"#, SLACK_SLOTS, "[]");
    let v = lint(&[("slack.json", good.clone()), ("other.json", good)]);
    assert_eq!(v.len(), 1, "{v:?}");
    assert_eq!(v[0].path, "presets/other.json");
    // Two listed with one id.
    let y = manifest(
        CREDS,
        "    secret_link: { credential: url, mode: send, presets: [a.json, b.json] }",
        TOOLS,
    );
    let m = Manifest::parse(&y, "0.1.0").unwrap();
    let mut pkg = Package {
        files: Default::default(),
        signature: None,
    };
    let p = preset("same", r#"{"text":{{text}}}"#, SLACK_SLOTS, "[]").into_bytes();
    pkg.files.insert("presets/a.json".into(), p.clone());
    pkg.files.insert("presets/b.json".into(), p);
    let mut v = Vec::new();
    archive_rules(&m, &pkg, &mut v);
    assert_eq!(v.len(), 1, "{v:?}");
    assert!(v[0].message.contains("duplicate"));
}
