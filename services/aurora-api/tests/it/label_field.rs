//! (phase 5, FIX-194, D-420, C-1091) A `fields` method's `store: label` field is the connection's
//! label: checked like a rename (422 on `/fields/<name>`, nothing stored), never kept in metadata or
//! credentials, ahead of the probe's label, and on a reconnect an empty field keeps the label.
//!
//! A child module of `connect` (it reuses that file's world and helpers).
use super::*;

const YAML: &str = r#"package: { id: lab, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: L }
apps:
  - about: { id: lab_plain, name: Plain app, description: D, category: dev, web_hosts: [plain.acme.example] }
    credentials:
      - { name: api_token, secret: true }
    connect:
      - id: key
        kind: fields
        fields:
          - { name: name, label: Name, type: text, store: label }
          - { name: api_token, label: Token, type: secret, required: true }
    hosts:
      - match: { host: "plain.acme.example" }
        inject:
          - bearer: "{credentials.api_token}"
  - about: { id: lab_probe, name: Probed app, description: D, category: dev, web_hosts: [api.acme.example] }
    credentials:
      - { name: api_token, secret: true }
    connect:
      - id: key
        kind: fields
        fields:
          - { name: name, label: Name, type: text, store: label }
          - { name: api_token, label: Token, type: secret, required: true }
        probe: { url: "https://api.acme.example/me", label: /email }
    hosts:
      - match: { host: "api.acme.example" }
        inject:
          - bearer: "{credentials.api_token}"
"#;

impl W {
    async fn connect_lab(&self, app: &str, fields: Value, reconnect: Option<&str>) -> (u16, Value) {
        let mut b = self.connect_body("key");
        b["fields"] = fields;
        if let Some(id) = reconnect {
            b["reconnect_connection_id"] = json!(id);
        }
        let (s, _, v) = self
            .call(
                &self.owner,
                Method::POST,
                &format!("/v1/apps/{app}/connect"),
                Some(b),
            )
            .await;
        (s, v)
    }

    async fn connection_rows(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM app_connections")
            .fetch_one(self.h.pool())
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn the_label_field_names_the_connection_and_is_stored_nowhere_else() {
    let w = world_with(None, YAML, false).await;
    let (s, v) = w
        .connect_lab(
            "lab_plain",
            json!({"name": "  Production account  ", "api_token": "tok-1"}),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["label"], "Production account");
    assert!(v["connection"]["metadata"].get("name").is_none(), "{v}");
    let id: Uuid = v["connection"]["id"].as_str().unwrap().parse().unwrap();
    let (meta, keys): (String, Option<Vec<String>>) =
        sqlx::query_as("SELECT metadata::text, credential_keys FROM app_connections WHERE id=$1")
            .bind(id)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(!meta.contains("Production account"), "{meta}");
    assert_eq!(keys.unwrap(), ["api_token"], "the name is no credential");
}

#[tokio::test]
async fn an_empty_field_gives_the_apps_name_or_the_probes_label() {
    let w = world_with(None, YAML, false).await;
    let (s, v) = w
        .connect_lab("lab_plain", json!({"api_token": "tok-1"}), None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["label"], "Plain app");
    // Blank counts as empty.
    let (s, v) = w
        .connect_lab(
            "lab_plain",
            json!({"name": "   ", "api_token": "tok-2"}),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["label"], "Plain app");
    // A method with a probe: the field wins when given, the probe's label otherwise.
    let (s, v) = w
        .connect_lab("lab_probe", json!({"api_token": "tok-3"}), None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["label"], "me@acme.io");
    let (s, v) = w
        .connect_lab(
            "lab_probe",
            json!({"name": "Mine", "api_token": "tok-4"}),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["label"], "Mine");
}

#[tokio::test]
async fn an_invisible_character_or_a_long_name_is_a_field_error_and_stores_nothing() {
    let w = world_with(None, YAML, false).await;
    for bad in ["Pro\u{200B}d".to_owned(), "x".repeat(121)] {
        let (s, v) = w
            .connect_lab(
                "lab_plain",
                json!({"name": bad, "api_token": "tok-1"}),
                None,
            )
            .await;
        // The usual connect field error (`invalid_request`, status 400, path `/fields/<name>`).
        assert_eq!(s, 400, "{v}");
        assert!(v.to_string().contains("/fields/name"), "{v}");
    }
    assert_eq!(w.connection_rows().await, 0);
}

#[tokio::test]
async fn a_reconnect_replaces_the_label_with_a_name_and_keeps_it_without() {
    let w = world_with(None, YAML, false).await;
    let (_, v) = w
        .connect_lab(
            "lab_plain",
            json!({"name": "First", "api_token": "tok-1"}),
            None,
        )
        .await;
    let id = v["connection"]["id"].as_str().unwrap().to_owned();
    let (s, v) = w
        .connect_lab("lab_plain", json!({"api_token": "tok-2"}), Some(&id))
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["label"], "First", "an empty field keeps it");
    let (s, v) = w
        .connect_lab(
            "lab_plain",
            json!({"name": "Second", "api_token": "tok-3"}),
            Some(&id),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["connection"]["label"], "Second");
    assert_eq!(w.connection_rows().await, 1);
}
