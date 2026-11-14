#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-123 (D-195(a), D-214): connect methods of one app. A tool restricted by `connect_methods`
//! is classified only for connections made with those methods (the `check-snapshot` runner uses
//! `identify_tool`, the function of the live pipeline), and the refresh/mint paths resolve the
//! token kind per connection (`TokenManager::applies`, `AppEntry::token_for`).
use aurora_gateway::check_snapshot::run;
use aurora_gateway::refresh::TokenManager;
use aurora_gateway::snapshot::parse;
use aurora_registry::Manifest;
use aurora_registry::snapshot::{ActivePackage, compile};

/// One service with an access-token method (static) and an installation method that mints
/// one-hour tokens through a hook (`token` on the method, none on the app).
const MANIFEST: &str = r#"
package: { id: svc-pkg, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: aurora, name: Svc }
permissions:
  hooks: [connect.exchange, token.mint]
  egress:
    connect.exchange: [api.example.com]
    token.mint: [api.example.com]
apps:
  - about: { id: svc, name: Service }
    credentials:
      - { name: access_token, secret: true }
      - { name: private_key, secret: true, optional: true }
    connect:
      - id: token
        kind: fields
        fields: [ { name: access_token, label: T, type: secret, required: true } ]
      - id: installation
        kind: hook
        token: { kind: hook, max_lifetime_secs: 3600 }
        fields: [ { name: private_key, label: K, type: file, required: true } ]
    hosts:
      - match: { host: api.example.com }
        inject: [ { bearer: "{credentials.access_token}" } ]
    tools:
      read:
        items:
          - { id: get_user, name: Get the user, host: api.example.com, path: /user, methods: [GET], connect_methods: [token] }
          - { id: list_installation, name: List the installation, host: api.example.com, path: /installation, methods: [GET], connect_methods: [installation] }
          - { id: get_repo, name: Get a repo, host: api.example.com, path: "/repos/*", methods: [GET] }
"#;

fn snapshot_json() -> Vec<u8> {
    let p = ActivePackage {
        manifest: Manifest::parse(MANIFEST, "0.1.0").unwrap(),
        digest: "d".into(),
        publisher: "aurora".into(),
        hooks_digest: None,
    };
    compile(&[p], &[], &[], 1).unwrap().json
}

fn case(method: Option<&str>, path: &str, expect: &str) -> String {
    let m = method.map_or(String::new(), |m| format!(", method: {m}"));
    format!(
        "- name: {path} as {}\n  connection: {{ credentials: {{ access_token: tok }}{m} }}\n  request: {{ method: GET, url: \"https://api.example.com{path}\" }}\n  expect: {expect}\n",
        method.unwrap_or("unknown")
    )
}

#[test]
fn a_method_restricted_tool_is_classified_only_for_its_methods() {
    let y = [
        case(Some("token"), "/user", "{ app: svc, tool: get_user }"),
        case(Some("installation"), "/user", "{ app: svc, no_tool: true }"),
        case(
            Some("installation"),
            "/installation",
            "{ app: svc, tool: list_installation }",
        ),
        case(
            Some("token"),
            "/installation",
            "{ app: svc, no_tool: true }",
        ),
        // A tool without a list applies to every method.
        case(Some("token"), "/repos/a", "{ app: svc, tool: get_repo }"),
        case(
            Some("installation"),
            "/repos/a",
            "{ app: svc, tool: get_repo }",
        ),
        // A connection with no recorded method hides nothing (legacy rows of a multi-method app).
        case(None, "/user", "{ app: svc, tool: get_user }"),
        case(
            None,
            "/installation",
            "{ app: svc, tool: list_installation }",
        ),
    ]
    .concat();
    let r = run(&snapshot_json(), &[("m.yaml".into(), y)]).unwrap();
    assert!(r.ok, "{}", serde_json::to_string_pretty(&r.json).unwrap());
    assert_eq!(r.json["passed"].as_u64(), Some(8));
}

#[test]
fn the_token_kind_is_resolved_per_connection_method() {
    let body: serde_json::Value = serde_json::from_slice(&snapshot_json()).unwrap();
    let loaded = parse("h", body).unwrap();
    let app = loaded.snapshot.app("svc").unwrap();
    // The app has no `token` of its own; the installation method's hook token applies to
    // connections made with it, and only to them.
    assert!(app.token.is_none());
    assert!(TokenManager::applies(app, Some("installation")));
    assert!(!TokenManager::applies(app, Some("token")));
    assert!(!TokenManager::applies(app, None));
    assert_eq!(
        app.token_for(Some("installation"))
            .and_then(|t| t.max_lifetime_secs),
        Some(3600)
    );
}
