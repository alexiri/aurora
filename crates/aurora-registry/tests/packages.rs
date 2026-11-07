#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::collections::HashMap;

use aurora_crypto::{SigningKey, generate_signing_key};
use aurora_policy::HostPattern;
use aurora_registry::builder::{PackageBuilder, png_stub};
use aurora_registry::signature::TrustedKey;
use aurora_registry::validate::{self, ValidationContext};
use aurora_registry::{Limits, Package, Signer, TrustStore};
use tar::EntryType;

const BASE: &str = r#"
package: { id: todoist, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: PUB, name: Todoist }
apps:
  - about: { id: todoist, name: Todoist }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: API token, type: secret, required: true } ]
    hosts:
      - match: { host: HOST, path_prefix: /rest }
        inject:
          - bearer: "{credentials.api_token}"
"#;

fn manifest(publisher: &str, host: &str) -> String {
    BASE.replace("PUB", publisher).replace("HOST", host)
}

fn key() -> SigningKey {
    generate_signing_key().unwrap()
}

fn ctx() -> ValidationContext {
    ValidationContext {
        running_version: "0.1.0".into(),
        reserved_hosts: [
            "api.anthropic.com",
            "aurora.example.com",
            "*.aurora.internal",
        ]
        .iter()
        .map(|h| HostPattern::parse(h).unwrap())
        .collect(),
        active_app_ids: HashMap::new(),
    }
}

fn trust(k: &SigningKey) -> TrustStore {
    TrustStore {
        project_key: Some(k.verifying_key()),
        keys: vec![],
    }
}

fn read(b: &PackageBuilder) -> Package {
    Package::read(&b.build().unwrap(), &Limits::default()).unwrap()
}

fn read_codes(b: &PackageBuilder) -> Vec<&'static str> {
    match Package::read(&b.build().unwrap(), &Limits::default()) {
        Ok(_) => vec![],
        Err(v) => v.iter().map(|x| x.code).collect(),
    }
}

fn codes(p: &Package, c: &ValidationContext, t: &TrustStore, unsigned: bool) -> Vec<&'static str> {
    validate::package(p, c, t, unsigned)
        .iter()
        .map(|v| v.code)
        .collect()
}

fn signed(yaml: &str, k: &SigningKey) -> PackageBuilder {
    let mut b = PackageBuilder::new(yaml);
    b.sign(k, "builtin");
    b
}

#[test]
fn valid_signed_package_passes() {
    let k = key();
    let p = read(&signed(&manifest("aurora", "api.todoist.com"), &k));
    assert!(validate::package(&p, &ctx(), &trust(&k), false).is_empty());
    assert_eq!(
        aurora_registry::signature::verify(&p, &trust(&k), false).unwrap(),
        Signer::Project
    );
}

#[test]
fn trusted_third_party_key_ok_and_revoked_or_wrong_publisher_refused() {
    let k = key();
    let y = manifest("acme", "api.todoist.com");
    let mut b = PackageBuilder::new(&y);
    b.sign(&k, &PackageBuilder::key_id(&k));
    let p = read(&b);
    let mk = |publisher: &str, revoked| TrustStore {
        project_key: None,
        keys: vec![TrustedKey {
            publisher: publisher.into(),
            key: k.verifying_key(),
            revoked,
        }],
    };
    assert!(validate::package(&p, &ctx(), &mk("acme", false), false).is_empty());
    assert_eq!(codes(&p, &ctx(), &mk("acme", true), false), ["key_revoked"]);
    assert_eq!(
        codes(&p, &ctx(), &mk("other", false), false),
        ["publisher_mismatch"]
    );
    assert_eq!(
        codes(&p, &ctx(), &TrustStore::default(), false),
        ["signature_untrusted"]
    );
}

#[test]
fn unsigned_refused_or_allowed() {
    let b = PackageBuilder::new(&manifest("aurora", "api.todoist.com"));
    let p = read(&b);
    let t = TrustStore::default();
    assert_eq!(codes(&p, &ctx(), &t, false), ["signature_missing"]);
    assert!(codes(&p, &ctx(), &t, true).is_empty());
}

#[test]
fn builtin_key_requires_publisher_aurora() {
    let k = key();
    let p = read(&signed(&manifest("acme", "api.todoist.com"), &k));
    assert_eq!(codes(&p, &ctx(), &trust(&k), false), ["publisher_mismatch"]);
}

#[test]
fn tampered_file_is_digest_mismatch() {
    let k = key();
    let mut b = signed(&manifest("aurora", "api.todoist.com"), &k);
    b.set_file("docs/setup.md", b"changed\n");
    let p = read(&b);
    assert_eq!(codes(&p, &ctx(), &trust(&k), false), ["digest_mismatch"]);
}

#[test]
fn archive_rules() {
    let y = manifest("aurora", "api.todoist.com");
    let mut b = PackageBuilder::new(&y);
    b.push_symlink("docs/link", "/etc/passwd");
    assert!(read_codes(&b).contains(&"archive_link"));

    let mut b = PackageBuilder::new(&y);
    b.push_raw("../evil", EntryType::Regular, b"x");
    assert!(read_codes(&b).contains(&"path_traversal"));

    let mut b = PackageBuilder::new(&y);
    b.push_raw("/abs", EntryType::Regular, b"x");
    assert!(read_codes(&b).contains(&"path_absolute"));

    let mut b = PackageBuilder::new(&y);
    b.push_raw("docs/setup.md", EntryType::Regular, b"x");
    assert!(read_codes(&b).contains(&"path_duplicate"));

    let mut b = PackageBuilder::new(&y);
    b.set_file("extra.txt", b"x");
    assert!(read_codes(&b).contains(&"path_unknown"));

    let mut b = PackageBuilder::new(&y);
    b.remove_file("assets/icon.png");
    assert!(read_codes(&b).contains(&"file_missing"));
}

#[test]
fn oversize() {
    let b = PackageBuilder::new(&manifest("aurora", "api.todoist.com"));
    let bytes = b.build().unwrap();
    let small = Limits {
        package_max: 10,
        ..Limits::default()
    };
    assert_eq!(
        Package::read(&bytes, &small).unwrap_err()[0].code,
        "package_too_large"
    );
    let small = Limits {
        decompressed_max: 100,
        ..Limits::default()
    };
    assert_eq!(
        Package::read(&bytes, &small).unwrap_err()[0].code,
        "package_too_large"
    );
}

#[test]
fn icon_rules() {
    let y = manifest("aurora", "api.todoist.com");
    let mut b = PackageBuilder::new(&y);
    b.set_file("assets/icon.png", b"GIF89a not a png at all......");
    assert_eq!(read_codes(&b), ["icon_not_png"]);
    let mut b = PackageBuilder::new(&y);
    b.set_file("assets/icon.png", &png_stub(64, 128));
    assert_eq!(read_codes(&b), ["icon_not_square"]);
    let mut b = PackageBuilder::new(&y);
    b.set_file("assets/icon.png", &png_stub(32, 32));
    assert_eq!(read_codes(&b), ["icon_too_small"]);
    let mut b = PackageBuilder::new(&y);
    let mut big = png_stub(64, 64);
    big.resize(300 * 1024, 0);
    b.set_file("assets/icon.png", &big);
    assert_eq!(read_codes(&b), ["icon_too_large"]);
}

#[test]
fn guide_rules() {
    let y = manifest("aurora", "api.todoist.com");
    let mut b = PackageBuilder::new(&y);
    b.set_file("docs/setup.md", b"# Hi\n\n<script>x</script>\n");
    assert_eq!(read_codes(&b), ["guide_raw_html"]);
    let mut b = PackageBuilder::new(&y);
    b.set_file("docs/setup.md", b"text <b>bold</b>\n");
    assert_eq!(read_codes(&b), ["guide_raw_html"]);
    let mut b = PackageBuilder::new(&y);
    b.set_file("docs/setup.md", &vec![b'a'; 70 * 1024]);
    assert_eq!(read_codes(&b), ["guide_too_large"]);
}

fn unsigned_codes(y: &str) -> Vec<&'static str> {
    let p = read(&PackageBuilder::new(y));
    codes(&p, &ctx(), &TrustStore::default(), true)
}

#[test]
fn zone_breadth() {
    for (zone, bad) in [
        ("*.example.com", false),
        ("*.co.uk", true),
        ("*.com", true),
        ("*", true),
    ] {
        let y = manifest("aurora", "api.todoist.com").replace(
            "        inject:",
            &format!("        tenant: {{ field: connection.tenant_host, zone: \"{zone}\" }}\n        inject:"),
        );
        let c = unsigned_codes(&y);
        assert_eq!(c.contains(&"zone_too_broad"), bad, "{zone}: {c:?}");
    }
}

#[test]
fn reserved_hosts() {
    for h in ["api.anthropic.com", "x.aurora.internal", "\"*.internal\""] {
        let c = unsigned_codes(&manifest("aurora", h));
        assert!(c.contains(&"reserved_host"), "{h}: {c:?}");
    }
    assert!(unsigned_codes(&manifest("aurora", "api.todoist.com")).is_empty());
}

#[test]
fn egress_private_and_reserved() {
    for (h, code) in [
        ("127.0.0.1", "egress_private"),
        ("localhost", "egress_private"),
        ("10.0.0.5", "egress_private"),
        ("api.anthropic.com", "egress_reserved"),
    ] {
        let y = manifest("aurora", "api.todoist.com")
            .replacen("apps:", &format!("permissions:\n  hooks: [connect.exchange]\n  egress:\n    connect.exchange: [\"{h}\"]\napps:"), 1)
            .replace(
                "      - id: api_key\n        kind: fields\n        fields: [ { name: api_token, label: API token, type: secret, required: true } ]",
                "      - id: api_key\n        kind: hook\n        fields: []",
            );
        let mut b = PackageBuilder::new(&y);
        b.set_file("hooks.wasm", MIN_HOOKS_COMPONENT);
        let p = read(&b);
        let c = codes(&p, &ctx(), &TrustStore::default(), true);
        assert_eq!(c, [code], "{h}");
    }
}

#[test]
fn duplicate_app_id_with_active_package() {
    let p = read(&PackageBuilder::new(&manifest("aurora", "api.todoist.com")));
    let mut c = ctx();
    c.active_app_ids.insert("todoist".into(), "todoist".into());
    assert!(
        codes(&p, &c, &TrustStore::default(), true).is_empty(),
        "same package upgrade"
    );
    c.active_app_ids
        .insert("todoist".into(), "other-pkg".into());
    assert_eq!(
        codes(&p, &c, &TrustStore::default(), true),
        ["duplicate_app_id"]
    );
}

#[test]
fn hooks_wasm_presence() {
    let mut b = PackageBuilder::new(&manifest("aurora", "api.todoist.com"));
    b.set_file("hooks.wasm", MIN_HOOKS_COMPONENT);
    let p = read(&b);
    assert_eq!(
        codes(&p, &ctx(), &TrustStore::default(), true),
        ["hooks_wasm_unexpected"]
    );
}

#[test]
fn wildcard_superset() {
    let with = |wild: &str, items: &str| {
        let mut y = manifest("aurora", "api.todoist.com");
        y.push_str(&format!(
            "    tools:\n      read:\n        {wild}\n        items: [{items}]\n"
        ));
        y
    };
    let w = "wildcard: { id: all_read, name: All }";
    let good = "{ id: a, name: A, host: api.todoist.com, path: /rest/v2/tasks, methods: [GET] }";
    let outside = "{ id: a, name: A, host: api.todoist.com, path: /other, methods: [GET] }";
    assert!(unsigned_codes(&with(w, good)).is_empty());
    assert!(unsigned_codes(&with(w, "")).contains(&"wildcard_not_superset"));
    assert!(unsigned_codes(&with(w, outside)).contains(&"wildcard_not_superset"));
}

/// Smallest component satisfying the plugin world shape: exports an (empty)
/// instance named `aurora:plugin/hooks@1.0.0`, imports nothing.
const MIN_HOOKS_COMPONENT: &[u8] =
    b"\0asm\x0d\0\x01\0\x05\x03\x01\x01\0\x0b\x1f\x01\0\x19aurora:plugin/hooks@1.0.0\x05\0\0";
