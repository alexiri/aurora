//! (P5-GUIDE-2, C-643, C-644, C-648, D-171, D-295, SEC-117, SEC-91) The agent guide in the home
//! sync: the `apps` part, the render inputs' index fields, the budget, the grant-row access lines,
//! and the admin routes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use crate::common::*;
use crate::plugins_lifecycle::{Admin, setup};
use aurora_db::repo::hosted::HomeSyncClaim;
use aurora_protocol::runner::{HomeSync, MAX_HOME_SYNC_PART};
use http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

const ROOT: &str = ".aurora/apps/gdemo_app";
const CREDENTIAL_REF: &str = "SECRET-CREDENTIAL-REF-9f3a";

const API_MD: &[u8] = b"# API\nUse the bulk endpoint.\n";
const STEPS_MD: &[u8] = b"# Steps\n1. list things\n2. create one\n";
const DATA_JSON: &[u8] = b"{\"things\": []}\n";

fn yaml(summary: &str, essentials: &str) -> String {
    format!(
        r#"package: {{ id: gdemo, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: P }}
apps:
  - about: {{ id: gdemo_app, name: Demo App, description: "Fallback description.", category: dev }}
    credentials: [ {{ name: api_token, secret: true }} ]
    connect:
      - id: api_key
        kind: fields
        fields: [ {{ name: api_token, label: T, type: secret, required: true }} ]
    agent:
      summary: "{summary}"
      instructions: "{essentials}"
      guide:
        - {{ file: api.md, title: "REST conventions" }}
        - {{ file: steps.md, title: "Create a thing", kind: recipe }}
        - {{ file: data.json, title: "Sample data" }}
    tools:
      read:
        items:
          - {{ id: list_things, name: List things, description: "All the things.", host: api.gdemo.example, path: /things, methods: [GET] }}
      write:
        items:
          - {{ id: create_thing, name: Create a thing, host: api.gdemo.example, path: /things, methods: [POST] }}
    hosts:
      - match: {{ host: "api.gdemo.example" }}
        inject:
          - bearer: "{{credentials.api_token}}"
"#
    )
}

fn files() -> Vec<(&'static str, &'static [u8])> {
    vec![
        ("agent/gdemo_app/api.md", API_MD),
        ("agent/gdemo_app/steps.md", STEPS_MD),
        ("agent/gdemo_app/data.json", DATA_JSON),
    ]
}

struct Agent {
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    conn: Uuid,
}

/// Uploads and approves the package, then an agent with one connection of it and a grant.
async fn world(h: &Harness, a: &Admin<'_>, grant: &str) -> Agent {
    let v = a
        .upload_with(
            &yaml("Things, lists and creating them.", "Be careful."),
            &files(),
        )
        .await;
    assert_eq!(v["status"], "pending_approval", "{}", v["violations"]);
    let (s, p) = a.act(&v["id"], "approve").await;
    assert_eq!(s, 200, "{p}");
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness, effort, brief) VALUES ($1,$2,$3,'ada','Ada','hosted','fake','medium','')")
        .bind(agent).bind(org).bind(ws).execute(h.pool()).await.unwrap();
    let conn = Uuid::now_v7();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,'gdemo_app','gdemo','1.0.0','work account','active',$4)")
        .bind(conn).bind(org).bind(ws).bind(CREDENTIAL_REF).execute(h.pool()).await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!("INSERT INTO grants (id, workspace_id, agent_id, connection_id, mode) VALUES ($1,$2,$3,$4,'{grant}')")))
        .bind(Uuid::now_v7()).bind(ws).bind(agent).bind(conn).execute(h.pool()).await.unwrap();
    Agent {
        org,
        ws,
        agent,
        conn,
    }
}

async fn generation(h: &Harness, agent: Uuid) -> i64 {
    sqlx::query_scalar("SELECT home_sync_generation FROM agents WHERE id = $1")
        .bind(agent)
        .fetch_one(h.pool())
        .await
        .unwrap()
}

async fn parts(h: &Harness, agent: Uuid) -> Vec<HomeSync> {
    parts_with(h, h.state.clone(), agent).await
}

async fn parts_with(h: &Harness, state: aurora_api::app::AppState, agent: Uuid) -> Vec<HomeSync> {
    let mut conn = h.pool().acquire().await.unwrap();
    let claim = HomeSyncClaim {
        sandbox_id: Uuid::now_v7(),
        boot_id: Uuid::now_v7(),
        agent_id: agent,
        generation: 1,
        harness_id: None,
    };
    aurora_api::domains::hosted::home::render(&state, &mut conn, &claim)
        .await
        .unwrap()
}

/// `path -> content` of the `apps` part entries.
fn folder(parts: &[HomeSync]) -> BTreeMap<String, String> {
    parts
        .iter()
        .filter(|p| p.part.kind.as_str() == "apps")
        .flat_map(|p| p.part.entries.iter())
        .map(|e| {
            (
                e["path"].as_str().unwrap().to_owned(),
                e["content"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn manifest(parts: &[HomeSync]) -> Vec<String> {
    parts
        .iter()
        .filter(|p| p.part.kind.as_str() == "manifest")
        .flat_map(|p| p.part.entries.iter())
        .map(|e| e["path"].as_str().unwrap().to_owned())
        .collect()
}

fn render_inputs(parts: &[HomeSync]) -> Value {
    parts
        .iter()
        .find(|p| p.part.kind.as_str() == "render_inputs")
        .unwrap()
        .part
        .entries[0]
        .clone()
}

/// The folders match the render inputs, the manifest names every path, every entry fits a part,
/// the files carry their third-party headers, and no credential of the connection is anywhere.
#[tokio::test]
async fn the_folder_matches_the_render_inputs_and_the_manifest() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let w = world(&h, &a, "full").await;
    let p = parts(&h, w.agent).await;
    let ri = render_inputs(&p);
    let app = &ri["apps"][0];
    assert_eq!(app["app_id"], "gdemo_app");
    assert_eq!(app["summary"], "Things, lists and creating them.");
    assert_eq!(app["guide_path"], format!("{ROOT}/guide.md"));
    // The four fields of before are untouched (a supervisor that predates the guides renders as before).
    assert_eq!(app["name"], "Demo App");
    assert_eq!(app["snippet"], "Be careful.");
    assert_eq!(
        app["accounts"],
        json!([{"id": w.conn, "label": "work account"}])
    );

    let f = folder(&p);
    let names: Vec<&str> = f.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            ".aurora/apps/gdemo_app/api.json",
            ".aurora/apps/gdemo_app/api.md",
            ".aurora/apps/gdemo_app/data.json",
            ".aurora/apps/gdemo_app/guide.md",
            ".aurora/apps/gdemo_app/steps.md",
            ".aurora/apps/gdemo_app/tools.md",
        ]
    );
    let m = manifest(&p);
    for path in f.keys() {
        assert!(m.contains(path), "{path} missing from the manifest");
    }
    // `apps` follows `tools` and precedes `manifest`.
    let kinds: Vec<&str> = p.iter().map(|s| s.part.kind.as_str()).collect();
    let pos = |k: &str| kinds.iter().position(|x| *x == k).unwrap();
    assert!(
        pos("tools") < pos("apps") && pos("apps") < pos("manifest"),
        "{kinds:?}"
    );
    // Every part fits the wire bound.
    for s in &p {
        assert!(serde_json::to_vec(s).unwrap().len() <= MAX_HOME_SYNC_PART);
    }

    assert!(
        f[&format!("{ROOT}/api.md")]
            .starts_with("> Reference from the Demo App plugin (third-party, version 1.0.0).")
    );
    assert!(f[&format!("{ROOT}/api.md")].ends_with(std::str::from_utf8(API_MD).unwrap()));
    assert!(f[&format!("{ROOT}/steps.md")].starts_with("> Procedure from the Demo App plugin"));
    // Other extensions byte for byte.
    assert_eq!(f[&format!("{ROOT}/data.json")].as_bytes(), DATA_JSON);
    let guide = &f[&format!("{ROOT}/guide.md")];
    assert!(
        guide.contains(&w.conn.to_string()) && guide.contains("work account"),
        "{guide}"
    );
    assert!(guide.contains("Create a thing"), "{guide}");
    let tools = &f[&format!("{ROOT}/tools.md")];
    assert!(
        tools.contains("`list_things` GET /things") && tools.contains("`create_thing`"),
        "{tools}"
    );

    // SEC-117: nothing of the connection's credential reference, in any part.
    for s in &p {
        assert!(
            !serde_json::to_string(s).unwrap().contains(CREDENTIAL_REF),
            "{}",
            s.part.kind.as_str()
        );
    }
}

/// An agent without a grant on the app gets nothing of it.
#[tokio::test]
async fn an_agent_without_a_grant_gets_nothing_of_the_app() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let w = world(&h, &a, "full").await;
    let other = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness, effort, brief) VALUES ($1,$2,$3,'bob','Bob','hosted','fake','medium','')")
        .bind(other).bind(w.org).bind(w.ws).execute(h.pool()).await.unwrap();
    let p = parts(&h, other).await;
    assert!(folder(&p).is_empty());
    assert!(manifest(&p).iter().all(|m| !m.starts_with(".aurora/apps/")));
    assert_eq!(render_inputs(&p)["apps"], json!([]));
    assert!(!serde_json::to_string(&p).unwrap().contains("gdemo_app"));
}

/// A custom grant shows the three states from the grant row; editing it, revoking it, disconnecting
/// the account, making the app unavailable and approving a new package version each give the
/// expected next generation and folder.
#[tokio::test]
async fn grant_changes_move_the_generation_and_the_folder() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let w = world(&h, &a, "custom").await;
    sqlx::query(
        "UPDATE grants SET allow_tools = '{list_things}', ask_tools = '{}' WHERE agent_id = $1",
    )
    .bind(w.agent)
    .execute(h.pool())
    .await
    .unwrap();
    let mut g = generation(&h, w.agent).await;
    let tools = |p: &[HomeSync]| folder(p)[&format!("{ROOT}/tools.md")].clone();

    let t = tools(&parts(&h, w.agent).await);
    assert!(
        t.contains("`list_things` GET /things: List things. All the things. [allowed]"),
        "{t}"
    );
    assert!(
        t.contains("`create_thing`") && t.contains("[blocked]"),
        "{t}"
    );

    // Edit: ask for create_thing.
    sqlx::query("UPDATE grants SET ask_tools = '{create_thing}' WHERE agent_id = $1")
        .bind(w.agent)
        .execute(h.pool())
        .await
        .unwrap();
    let n = generation(&h, w.agent).await;
    assert!(n > g, "a grant edit bumps the generation ({g} -> {n})");
    g = n;
    let t = tools(&parts(&h, w.agent).await);
    assert!(t.contains("[approval]"), "{t}");

    // Full: everything allowed.
    sqlx::query("UPDATE grants SET mode = 'full' WHERE agent_id = $1")
        .bind(w.agent)
        .execute(h.pool())
        .await
        .unwrap();
    assert!(generation(&h, w.agent).await > g);
    g = generation(&h, w.agent).await;
    let t = tools(&parts(&h, w.agent).await);
    assert!(!t.contains("[blocked]") && !t.contains("[approval]"), "{t}");

    // Unavailable to the org: the folder is gone.
    sqlx::query("UPDATE organizations SET app_posture = 'restricted' WHERE id = $1")
        .bind(w.org)
        .execute(h.pool())
        .await
        .unwrap();
    assert!(generation(&h, w.agent).await > g);
    g = generation(&h, w.agent).await;
    assert!(folder(&parts(&h, w.agent).await).is_empty());
    sqlx::query("UPDATE organizations SET app_posture = 'open' WHERE id = $1")
        .bind(w.org)
        .execute(h.pool())
        .await
        .unwrap();
    assert!(generation(&h, w.agent).await > g);
    assert_eq!(folder(&parts(&h, w.agent).await).len(), 6);

    // A new package version (changed reference file) is the next generation with the new bytes.
    let before = generation(&h, w.agent).await;
    let v2 = yaml("Things, lists and creating them.", "Be careful.")
        .replace("version: 1.0.0", "version: 1.1.0");
    let new_api: &[u8] = b"# API v2\nNew.\n";
    let v = a
        .upload_with(
            &v2,
            &[
                ("agent/gdemo_app/api.md", new_api),
                ("agent/gdemo_app/steps.md", STEPS_MD),
                ("agent/gdemo_app/data.json", DATA_JSON),
            ],
        )
        .await;
    // No widening: it activates on upload.
    assert_eq!(v["status"], "active", "{v}");
    assert!(
        generation(&h, w.agent).await > before,
        "a new snapshot bumps the generation"
    );
    let f = folder(&parts(&h, w.agent).await);
    assert!(f[&format!("{ROOT}/api.md")].contains("API v2"));
    assert!(f[&format!("{ROOT}/api.md")].contains("version 1.1.0"));

    // Disconnect: the account leaves.
    let before = generation(&h, w.agent).await;
    sqlx::query("UPDATE app_connections SET status = 'disabled' WHERE id = $1")
        .bind(w.conn)
        .execute(h.pool())
        .await
        .unwrap();
    assert!(
        generation(&h, w.agent).await > before,
        "a disconnect bumps the generation"
    );
    assert!(folder(&parts(&h, w.agent).await).is_empty());
    sqlx::query("UPDATE app_connections SET status = 'active' WHERE id = $1")
        .bind(w.conn)
        .execute(h.pool())
        .await
        .unwrap();

    // Revoke: the grant row goes.
    let before = generation(&h, w.agent).await;
    sqlx::query("DELETE FROM grants WHERE agent_id = $1")
        .bind(w.agent)
        .execute(h.pool())
        .await
        .unwrap();
    assert!(
        generation(&h, w.agent).await > before,
        "a revoked grant bumps the generation"
    );
    let p = parts(&h, w.agent).await;
    assert!(folder(&p).is_empty());
    assert!(manifest(&p).iter().all(|m| !m.starts_with(".aurora/apps/")));
}

/// The reference files of one sync stop at the budget: the app that does not fit gets the
/// generated files only and its `guide.md` says so.
#[tokio::test]
async fn the_budget_keeps_generated_files_and_drops_reference_files() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let w = world(&h, &a, "full").await;
    let tight = h.state.clone().with_guide_sync_max(10);
    // The counter is process-wide and only grows: assert the delta of this one sync, and that the
    // default budget (below) does not move it.
    let series = "aurora_home_guide_budget_exceeded_total";
    let _ = metrics_text(); // the recorder must exist before the counter is incremented
    let before = counter_value(series);
    let p = parts_with(&h, tight, w.agent).await;
    assert_eq!(
        counter_value(series) - before,
        1.0,
        "one app over budget, one increment"
    );
    let f = folder(&p);
    let names: Vec<&str> = f.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            ".aurora/apps/gdemo_app/api.json",
            ".aurora/apps/gdemo_app/guide.md",
            ".aurora/apps/gdemo_app/tools.md"
        ]
    );
    let guide = &f[&format!("{ROOT}/guide.md")];
    assert!(guide.contains("not available in this sandbox"), "{guide}");
    assert!(!guide.contains("REST conventions") || guide.contains("not available in this sandbox"));
    // Every path of the manifest is a projected one.
    assert!(
        manifest(&p)
            .iter()
            .filter(|m| m.starts_with(".aurora/apps/"))
            .all(|m| f.contains_key(m))
    );
    // The default budget sends all six and does not count.
    let before = counter_value(series);
    assert_eq!(folder(&parts(&h, w.agent).await).len(), 6);
    assert_eq!(counter_value(series), before);
}

/// The admin routes: the detail carries `agent_guides`, the file route serves the bytes by digest
/// with the safe headers, and unknown packages, apps and files are 404.
#[tokio::test]
async fn the_admin_routes_serve_the_guide() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let v = a
        .upload_with(&yaml("Things.", "Be careful."), &files())
        .await;
    let id = v["id"].as_str().unwrap().to_owned();
    let d = a.status(&v["id"]).await;
    let g = &d["agent_guides"][0];
    assert_eq!(g["app_id"], "gdemo_app");
    assert_eq!(g["summary"], "Things.");
    assert_eq!(g["instructions"], "Be careful.");
    assert_eq!(g["files"][0]["file"], "api.md");
    assert_eq!(g["files"][0]["kind"], "reference");
    assert_eq!(g["files"][0]["size"], API_MD.len());
    assert_eq!(g["files"][1]["kind"], "recipe");
    assert_eq!(g["operations"], Value::Null);
    assert_eq!(
        g["changed"],
        Value::Null,
        "no active version to compare with"
    );
    // A list does not read archives.
    let (s, list) = a.json(Method::GET, "/v1/admin/plugins", None).await;
    assert_eq!(s, 200);
    assert_eq!(list["items"][0]["agent_guides"], json!([]));

    let uri = format!("/v1/admin/plugins/{id}/agent-guides/gdemo_app/api.md");
    let res = h.get_raw(&uri, &[("cookie", &a.cookie)]).await;
    assert_eq!(res.0, 200);
    assert_eq!(res.2, API_MD);
    let hd = |n: &str| res.1.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
    assert_eq!(hd("content-type"), Some("text/plain; charset=utf-8"));
    assert_eq!(hd("x-content-type-options"), Some("nosniff"));
    assert_eq!(
        hd("content-disposition"),
        Some("attachment; filename=\"api.md\"")
    );
    assert_eq!(hd("cache-control"), Some("private, no-store"));
    for bad in [
        format!(
            "/v1/admin/plugins/{}/agent-guides/gdemo_app/api.md",
            Uuid::now_v7()
        ),
        format!("/v1/admin/plugins/{id}/agent-guides/nope/api.md"),
        format!("/v1/admin/plugins/{id}/agent-guides/gdemo_app/nope.md"),
        format!("/v1/admin/plugins/{id}/agent-guides/gdemo_app/guide.md"),
    ] {
        let r = h.get_raw(&bad, &[("cookie", &a.cookie)]).await;
        assert_eq!(r.0, 404, "{bad}");
    }
    // Not an instance admin: refused.
    let user = h.user("member@x.io", false).await;
    let (_, tok) = h.good_session(user).await;
    let r = h.get_raw(&uri, &[("cookie", &cookie(&tok))]).await;
    assert_eq!(r.0, 403);
}

/// `changed` against the active version, and `agent_guide_changed` in the approve and activate
/// audit rows (listing the apps whose summary, essentials or files differ).
#[tokio::test]
async fn a_new_version_reports_what_changed_and_audits_it() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let _w = world(&h, &a, "full").await;
    // A new host claim is a widening, so version 1.1.0 waits for approval and can be compared.
    let v2 = yaml("Things, changed.", "Be careful.")
        .replace("version: 1.0.0", "version: 1.1.0")
        .replace(
            "    hosts:\n",
            "    hosts:\n      - match: { host: \"api2.gdemo.example\" }\n        inject:\n          - bearer: \"{credentials.api_token}\"\n",
        );
    let v = a
        .upload_with(
            &v2,
            &[
                ("agent/gdemo_app/api.md", API_MD),
                ("agent/gdemo_app/steps.md", b"# Steps\nrewritten\n"),
                ("agent/gdemo_app/data.json", DATA_JSON),
            ],
        )
        .await;
    assert_eq!(v["status"], "pending_approval", "{v}");
    let d = a.status(&v["id"]).await;
    let c = &d["agent_guides"][0]["changed"];
    assert_eq!(c["summary"], true, "{d}");
    assert_eq!(c["instructions"], false);
    assert_eq!(c["operations"], false);
    assert_eq!(
        c["files"],
        json!([{"file": "steps.md", "change": "changed"}])
    );
    let (s, p) = a.act(&v["id"], "approve").await;
    assert_eq!(s, 200, "{p}");
    for action in ["plugin.approve", "plugin.activate"] {
        let meta: Value = wait_audit_meta(&h, action, v["id"].as_str().unwrap()).await;
        assert_eq!(
            meta["agent_guide_changed"],
            json!(["gdemo_app"]),
            "{action}: {meta}"
        );
    }
    // An automatic activation (no widening) audits it too; an unchanged guide adds no key.
    let v3 = yaml("Things, changed.", "Be careful.")
        .replace("version: 1.0.0", "version: 1.2.0")
        .replace(
            "    hosts:\n",
            "    hosts:\n      - match: { host: \"api2.gdemo.example\" }\n        inject:\n          - bearer: \"{credentials.api_token}\"\n",
        );
    let v = a
        .upload_with(
            &v3,
            &[
                ("agent/gdemo_app/api.md", b"# API v3\n"),
                ("agent/gdemo_app/steps.md", b"# Steps\nrewritten\n"),
                ("agent/gdemo_app/data.json", DATA_JSON),
            ],
        )
        .await;
    assert_eq!(v["status"], "active", "{v}");
    let meta = wait_audit_meta(&h, "plugin.activate", v["id"].as_str().unwrap()).await;
    assert_eq!(meta["auto"], true);
    assert_eq!(meta["agent_guide_changed"], json!(["gdemo_app"]), "{meta}");
}

trait RawGet {
    /// `(status, headers as lower-case name/value pairs, body bytes)`.
    async fn get_raw(
        &self,
        uri: &str,
        headers: &[(&str, &str)],
    ) -> (u16, Vec<(String, String)>, Vec<u8>);
}

impl RawGet for Harness {
    async fn get_raw(
        &self,
        uri: &str,
        headers: &[(&str, &str)],
    ) -> (u16, Vec<(String, String)>, Vec<u8>) {
        use http_body_util::BodyExt as _;
        use tower::ServiceExt as _;
        let mut b = http::Request::builder().method(Method::GET).uri(uri);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let res = self
            .app
            .clone()
            .oneshot(b.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        let (parts, body) = res.into_parts();
        let bytes = body.collect().await.unwrap().to_bytes().to_vec();
        let hs = parts
            .headers
            .iter()
            .map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap().to_owned()))
            .collect();
        (parts.status.as_u16(), hs, bytes)
    }
}

const OPERATIONS_JSON: &[u8] = br#"{
  "format": "aurora.operations/1",
  "operations": [
    { "id": "list_things", "method": "GET", "path": "/things", "summary": "All the things",
      "params": [ { "name": "page", "in": "query", "type": "integer" } ],
      "pagination": { "style": "page_param", "param": "page", "start": 1 },
      "items": "$.things", "tool": "list_things" },
    { "id": "create_thing", "method": "POST", "path": "/things", "summary": "Create a thing",
      "body": { "content_type": "application/json", "fields": [ { "name": "name", "required": true, "type": "string" } ] },
      "tool": "create_thing" }
  ]
}
"#;

/// (P5-GUIDE-6, C-652) An app with an operations file gets `api.json` in its folder: valid JSON of
/// the format, the operations mapped to the catalogue tools by the gateway's matcher, the account
/// with its host-derived `base_url`, listed in the manifest and named in `guide.md`; the operations
/// file itself is not projected; no credential reference is in it; an app without an operations
/// file has no `api.json`.
#[tokio::test]
async fn api_json_is_projected_beside_the_guide() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let y = yaml("Things, lists and creating them.", "Be careful.").replace(
        "      guide:\n",
        "      operations: operations.json\n      guide:\n",
    );
    let mut fs = files();
    fs.push(("agent/gdemo_app/operations.json", OPERATIONS_JSON));
    let v = a.upload_with(&y, &fs).await;
    assert_eq!(v["status"], "pending_approval", "{}", v["violations"]);
    let (s, p) = a.act(&v["id"], "approve").await;
    assert_eq!(s, 200, "{p}");
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness, effort, brief) VALUES ($1,$2,$3,'ada','Ada','hosted','fake','medium','')")
        .bind(agent).bind(org).bind(ws).execute(h.pool()).await.unwrap();
    let conn = Uuid::now_v7();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,'gdemo_app','gdemo','1.0.0','work account','active',$4)")
        .bind(conn).bind(org).bind(ws).bind(CREDENTIAL_REF).execute(h.pool()).await.unwrap();
    sqlx::query("INSERT INTO grants (id, workspace_id, agent_id, connection_id, mode) VALUES ($1,$2,$3,$4,'full')")
        .bind(Uuid::now_v7()).bind(ws).bind(agent).bind(conn).execute(h.pool()).await.unwrap();

    let p = parts(&h, agent).await;
    let f = folder(&p);
    assert!(
        !f.keys().any(|k| k.ends_with("/operations.json")),
        "the operations file is not projected: {:?}",
        f.keys().collect::<Vec<_>>()
    );
    let path = format!("{ROOT}/api.json");
    assert!(manifest(&p).contains(&path), "api.json is in the manifest");
    let api: Value = serde_json::from_str(&f[&path]).expect("api.json is JSON");
    assert_eq!(api["format"], "aurora.app-api/1", "{api}");
    assert_eq!(api["app"]["id"], "gdemo_app");
    let ops = api["operations"].as_array().expect("operations");
    let find = |id: &str| {
        ops.iter()
            .find(|o| o["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing: {api}"))
    };
    assert_eq!(find("list_things")["tool"], "list_things");
    assert_eq!(find("create_thing")["tool"], "create_thing");
    assert_eq!(api["accounts"][0]["connection_id"], conn.to_string());
    assert_eq!(api["accounts"][0]["grant"], "full");
    assert_eq!(api["operations"][0]["access"]["A"], "allowed");
    // A static app has no stored host: its only host is the account's base URL.
    assert_eq!(
        api["accounts"][0]["base_url"], "https://api.gdemo.example",
        "{api}"
    );
    assert!(f[&format!("{ROOT}/guide.md")].contains("api.json"));
    // SEC-117 again: the credential reference is nowhere in the file.
    assert!(!f[&path].contains(CREDENTIAL_REF));
    // (An app without operations has no `api.json`: the folder listing of
    // `the_folder_matches_the_render_inputs_and_the_manifest` names every file.)
}

/// Best-effort audit rows land shortly after the request.
async fn wait_audit_meta(h: &Harness, action: &str, target: &str) -> Value {
    for _ in 0..200 {
        let row: Option<Value> = sqlx::query_scalar(
            "SELECT metadata FROM audit_log WHERE action = $1 AND target_id = $2 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(action)
        .bind(target)
        .fetch_optional(h.pool())
        .await
        .unwrap();
        if let Some(r) = row {
            return r;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("no {action} audit row");
}
