#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use aurora_gateway::check_snapshot::run;
use aurora_registry::Manifest;
use aurora_registry::snapshot::{ActivePackage, compile};

const MANIFEST: &str = r#"
package: { id: todoist, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: aurora, name: Todoist }
apps:
  - about: { id: todoist, name: Todoist }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    hosts:
      - match: { host: api.todoist.com }
        inject:
          - bearer: "{credentials.api_token}"
    tools:
      write:
        items:
          - { id: close_task, name: Close, host: api.todoist.com, path: "/rest/v2/tasks/*/close", methods: [POST] }
"#;

fn snapshot() -> Vec<u8> {
    let p = ActivePackage {
        manifest: Manifest::parse(MANIFEST, "0.1.0").unwrap(),
        digest: "d".into(),
        publisher: "aurora".into(),
        hooks_digest: None,
    };
    compile(&[p], &[], &[], 1).unwrap().json
}

fn files(y: &str) -> Vec<(String, String)> {
    vec![("t.yaml".into(), y.into())]
}

#[test]
fn bearer_tool_and_generic_summary_pass() {
    let y = r#"
- name: close
  connection: { credentials: { api_token: tok } }
  request: { method: POST, url: "https://api.todoist.com/rest/v2/tasks/123/close" }
  expect:
    app: todoist
    tool: close_task
    upstream: { headers: { Authorization: "Bearer tok" } }
    summary: { fields: { Host: api.todoist.com } }
- name: unmatched host
  request: { method: GET, url: "https://other.example.com/x" }
  expect: { unmatched: true }
- name: hook case
  request: { method: GET, url: "https://api.todoist.com/x" }
  hook: { name: approval.summarize }
"#;
    let r = run(&snapshot(), &files(y)).unwrap();
    assert!(r.ok, "{}", r.json);
    assert_eq!(
        (r.json["passed"].as_u64(), r.json["skipped"].as_u64()),
        (Some(2), Some(1))
    );
}

#[test]
fn mismatch_fails_and_missing_credential_is_reported() {
    let y = r#"
- name: wrong
  connection: { credentials: { api_token: tok } }
  request: { method: GET, url: "https://api.todoist.com/rest/v2/tasks" }
  expect: { upstream: { headers: { authorization: "Bearer other" } } }
- name: no creds
  request: { method: GET, url: "https://api.todoist.com/rest/v2/tasks" }
"#;
    let r = run(&snapshot(), &files(y)).unwrap();
    assert!(!r.ok);
    assert_eq!(r.json["failed"], 2);
}

const GMAIL: &str = r#"
package: { id: mailapp, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: aurora, name: Mail }
apps:
  - about: { id: mailapp, name: Mail, web_hosts: [mail.example.com] }
    credentials: [ { name: api_token, secret: true } ]
    connect:
      - id: api_key
        kind: fields
        fields: [ { name: api_token, label: T, type: secret, required: true } ]
    hosts:
      - match: { host: api.mail.example.com }
        inject:
          - bearer: "{credentials.api_token}"
    tools:
      write:
        items:
          - { id: send_message, name: Send, host: api.mail.example.com, path: "/v1/send", methods: [POST] }
    approvals:
      send_message:
        title: Send email
        fields:
          - { label: To, source: body, steps: [ { json: /raw }, { base64url: {} }, { mime_header: To } ] }
          - { label: Subject, source: body, steps: [ { json: /raw }, { base64url: {} }, { mime_header: Subject } ] }
          - { label: Auth, source: header, name: authorization }
        link: "https://mail.example.com/mail/#{field.Subject}"
"#;

#[test]
fn body_source_template_summary_passes() {
    use base64::Engine;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode("To: bob@example.com\r\nSubject: Hi\r\n\r\nbody");
    let body = serde_json::json!({ "raw": raw }).to_string();
    let y = format!(
        r#"
- name: send
  connection: {{ credentials: {{ api_token: tok }} }}
  request:
    method: POST
    url: "https://api.mail.example.com/v1/send"
    headers: {{ authorization: "Bearer leak" }}
    body: '{body}'
  expect:
    tool: send_message
    summary: {{ title: Send email, fields: {{ To: bob@example.com, Subject: Hi }} }}
"#
    );
    let p = ActivePackage {
        manifest: Manifest::parse(GMAIL, "0.1.0").unwrap(),
        digest: "d".into(),
        publisher: "aurora".into(),
        hooks_digest: None,
    };
    let snap = compile(&[p], &[], &[], 1).unwrap().json;
    let r = run(&snap, &files(&y)).unwrap();
    assert!(r.ok, "{}", r.json);
    let s = &r.json["results"][0]["observed"]["summary"];
    assert_eq!(s["source"], "template");
    assert!(!s.to_string().contains("leak"), "{s}");
    assert_eq!(s["link"], "https://mail.example.com/mail/#Hi");
}
