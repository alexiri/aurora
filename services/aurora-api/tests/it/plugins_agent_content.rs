//! (P5-APPSKILL-1, C-1130, C-1132, C-1136, C-1137, SEC-150) Ingest of agent files: one `agent_file`
//! blob per `SKILL.md`, skill file and operations file of a package that is not `invalid`, readable
//! by digest only; old guide files are accepted and stored nowhere; the compiled snapshot carries
//! the skills with digests and executable bits; a package that breaks a skill rule is `invalid`
//! with the code; the admin detail and file routes; `changed` and the `agent_changed` audit key,
//! also when only a script's executable bit flips.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use crate::home_app_skills::{RawGet, wait_audit_meta};
use crate::plugins_lifecycle::setup;
use aurora_db::repo::plugins;
use http::Method;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

fn yaml(host: &str, extra_agent: &str) -> String {
    format!(
        "package: {{ id: gdemo, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: P }}\napps:\n  - about: {{ id: gdemo-app, name: A }}\n    credentials: [ {{ name: api_token, secret: true }} ]\n    connect:\n      - id: api_key\n        kind: fields\n        fields: [ {{ name: api_token, label: T, type: secret, required: true }} ]\n    hosts:\n      - match: {{ host: \"{host}\" }}\n        inject:\n          - bearer: \"{{credentials.api_token}}\"\n    agent:\n      summary: \"Demo things.\"\n      instructions: \"Be careful.\"\n{extra_agent}"
    )
}

const SKILL: &str = "skills/gdemo-app/gdemo-app-steps/SKILL.md";
const SCRIPT: &str = "skills/gdemo-app/gdemo-app-steps/scripts/run.sh";
const REFERENCE: &str = "skills/gdemo-app/gdemo-app-steps/reference/api.md";
const SKILL2: &str = "skills/gdemo-app/gdemo-app-api/SKILL.md";

fn skill_md(name: &str, description: &str, body: &str) -> Vec<u8> {
    format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}").into_bytes()
}

fn hex(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

async fn blob_kind(h: &Harness, digest: &str) -> Option<String> {
    sqlx::query_scalar("SELECT kind FROM plugin_blobs WHERE digest = $1")
        .bind(digest)
        .fetch_optional(h.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn ingest_stores_agent_file_blobs_and_the_snapshot_carries_the_skills() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let md = skill_md("gdemo-app-steps", "Do the thing", "# Steps\n1. go\n");
    let script: &[u8] = b"#!/bin/sh\necho hi\n";
    let reference: &[u8] = b"# API\nreference\n";
    // A blob of another kind with the same bytes as a skill file already exists: it keeps its kind.
    let shared = hex(reference);
    plugins::put_blob(h.pool(), &shared, "setup_guide", reference)
        .await
        .unwrap();
    let v = a
        .upload_with_exec(
            &yaml("api.gdemo.example", ""),
            &[(SKILL, &md), (SCRIPT, script), (REFERENCE, reference)],
            &[SCRIPT],
        )
        .await;
    assert_eq!(v["status"], "pending_approval", "{v}");
    assert_eq!(
        blob_kind(&h, &hex(&md)).await.as_deref(),
        Some("agent_file")
    );
    assert_eq!(
        blob_kind(&h, &hex(script)).await.as_deref(),
        Some("agent_file")
    );
    // Same bytes as an existing blob of another kind: the row is untouched ...
    assert_eq!(blob_kind(&h, &shared).await.as_deref(), Some("setup_guide"));
    // ... and the file is readable through the by-digest function, not through the kind.
    assert_eq!(
        plugins::get_agent_blob(h.pool(), &shared)
            .await
            .unwrap()
            .as_deref(),
        Some(reference)
    );
    assert!(
        plugins::get_blob(h.pool(), &shared, "agent_file")
            .await
            .unwrap()
            .is_none()
    );

    // Approval compiles the snapshot with the digests and the bits of the stored archive.
    let (s, p) = a.act(&v["id"], "approve").await;
    assert_eq!(s, 200, "{p}");
    let snap: Value = sqlx::query_scalar(
        "SELECT snapshot FROM registry_snapshots ORDER BY activated_at DESC LIMIT 1",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    let ag = &snap["apps"][0]["agent"];
    assert_eq!(ag["summary"], "Demo things.");
    assert!(ag.get("guide").is_none(), "{ag}");
    let sk = &ag["skills"][0];
    assert_eq!(sk["name"], "gdemo-app-steps");
    assert_eq!(sk["description"], "Do the thing");
    assert_eq!(sk["sha256"], hex(&md));
    assert_eq!(sk["size"], md.len());
    assert_eq!(
        sk["files"],
        json!([
            {"path": "reference/api.md", "sha256": shared, "size": reference.len(), "executable": false},
            {"path": "scripts/run.sh", "sha256": hex(script), "size": script.len(), "executable": true},
        ])
    );
    // No content in the snapshot.
    assert!(!snap.to_string().contains("echo hi"));
}

#[tokio::test]
async fn an_old_package_with_a_guide_is_accepted_and_nothing_of_it_is_stored() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let api: &[u8] = b"# old guide file\n";
    let steps: &[u8] = b"# old recipe\n";
    let legacy = yaml(
        "api.gdemo.example",
        "      guide:\n        - { file: api.md, title: \"API\" }\n        - { file: steps.md, title: \"Do it\", kind: recipe }\n",
    );
    let v = a
        .upload_with(
            &legacy,
            &[
                ("agent/gdemo-app/api.md", api),
                ("agent/gdemo-app/steps.md", steps),
            ],
        )
        .await;
    assert_eq!(v["status"], "pending_approval", "{v}");
    assert!(blob_kind(&h, &hex(api)).await.is_none());
    assert!(blob_kind(&h, &hex(steps)).await.is_none());
    let (s, p) = a.act(&v["id"], "approve").await;
    assert_eq!(s, 200, "{p}");
    let snap: Value = sqlx::query_scalar(
        "SELECT snapshot FROM registry_snapshots ORDER BY activated_at DESC LIMIT 1",
    )
    .fetch_one(h.pool())
    .await
    .unwrap();
    let ag = &snap["apps"][0]["agent"];
    assert!(
        ag.get("guide").is_none() && ag.get("skills").is_none(),
        "{ag}"
    );
    let d = a.status(&v["id"]).await;
    assert_eq!(d["agent"][0]["skills"], json!([]), "{d}");
}

#[tokio::test]
async fn an_invalid_package_stores_no_agent_blob() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let md = skill_md("gdemo-app-steps", "never stored", "# never stored\n");
    // A reserved host makes the row `invalid` (the skill itself is fine).
    let v = a
        .upload_with(&yaml("x.aurora.internal", ""), &[(SKILL, &md)])
        .await;
    assert_eq!(v["status"], "invalid", "{v}");
    assert!(blob_kind(&h, &hex(&md)).await.is_none());
}

/// (SEC-150) A skill that breaks a rule of the shared validator makes the package `invalid` with
/// the code, and none of its files is stored.
#[tokio::test]
async fn a_package_that_breaks_a_skill_rule_is_invalid_with_the_code() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    type Case = (&'static str, Vec<(&'static str, Vec<u8>)>);
    let cases: Vec<Case> = vec![
        (
            "app_skill_name",
            vec![(
                "skills/gdemo-app/other-steps/SKILL.md",
                skill_md("other-steps", "d", "b"),
            )],
        ),
        (
            "app_skill_not_text",
            vec![
                (SKILL, skill_md("gdemo-app-steps", "d", "b")),
                (SCRIPT, b"echo\0hi".to_vec()),
            ],
        ),
        (
            "app_skill_frontmatter",
            vec![(SKILL, b"# no frontmatter\n".to_vec())],
        ),
        (
            "app_skill_unknown_app",
            vec![("skills/nope/nope-x/SKILL.md", skill_md("nope-x", "d", "b"))],
        ),
    ];
    for (code, files) in cases {
        let refs: Vec<(&str, &[u8])> = files.iter().map(|(p, c)| (*p, c.as_slice())).collect();
        let v = a.upload_with(&yaml("api.gdemo.example", ""), &refs).await;
        assert_eq!(v["status"], "invalid", "{code}: {v}");
        assert!(
            v["violations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x["code"] == code),
            "{code}: {}",
            v["violations"]
        );
        for (_, c) in &files {
            assert!(blob_kind(&h, &hex(c)).await.is_none(), "{code}");
        }
    }
}

/// The admin detail carries `agent` per app; the file routes serve the bytes by digest with the safe
/// headers; unknown packages, apps, skills and files are 404, a bad path 400, a non-admin 403.
#[tokio::test]
async fn the_admin_routes_serve_the_skills() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let md = skill_md("gdemo-app-steps", "Do the thing", "# Steps\n");
    let script: &[u8] = b"#!/bin/sh\necho hi\n";
    let ops = crate::home_app_skills::OPERATIONS_JSON;
    let y = yaml(
        "api.gdemo.example",
        "      operations: operations.json\n    tools:\n      read:\n        items:\n          - { id: list_things, name: List things, host: api.gdemo.example, path: /things, methods: [GET] }\n      write:\n        items:\n          - { id: create_thing, name: Create a thing, host: api.gdemo.example, path: /things, methods: [POST] }\n",
    );
    let v = a
        .upload_with_exec(
            &y,
            &[
                (SKILL, &md),
                (SCRIPT, script),
                ("agent/gdemo-app/operations.json", ops),
            ],
            &[SCRIPT],
        )
        .await;
    assert_eq!(v["status"], "pending_approval", "{}", v["violations"]);
    let id = v["id"].as_str().unwrap().to_owned();
    let d = a.status(&v["id"]).await;
    let g = &d["agent"][0];
    assert_eq!(g["app_id"], "gdemo-app");
    assert_eq!(g["summary"], "Demo things.");
    assert_eq!(g["instructions"], "Be careful.");
    assert_eq!(g["operations"]["file"], "operations.json");
    assert_eq!(g["operations"]["count"], 2);
    let sk = &g["skills"][0];
    assert_eq!(sk["name"], "gdemo-app-steps");
    assert_eq!(sk["size"], md.len() + script.len());
    assert_eq!(
        sk["files"],
        json!([
            {"path": "SKILL.md", "size": md.len(), "sha256": hex(&md), "executable": false},
            {"path": "scripts/run.sh", "size": script.len(), "sha256": hex(script), "executable": true},
        ])
    );
    assert_eq!(
        g["changed"],
        Value::Null,
        "no active version to compare with"
    );
    assert!(d.get("agent_guides").is_none());
    // A list does not read archives.
    let (s, list) = a.json(Method::GET, "/v1/admin/plugins", None).await;
    assert_eq!(s, 200);
    assert_eq!(list["items"][0]["agent"], json!([]));

    let base = format!("/v1/admin/plugins/{id}/apps/gdemo-app");
    let res = h
        .get_raw(
            &format!("{base}/skills/gdemo-app-steps/file"),
            &[("cookie", &a.cookie)],
        )
        .await;
    assert_eq!(res.0, 200);
    assert_eq!(res.2, md, "no path = SKILL.md as shipped");
    let hd = |r: &(u16, Vec<(String, String)>, Vec<u8>), n: &str| {
        r.1.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone())
    };
    assert_eq!(
        hd(&res, "content-type").as_deref(),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(
        hd(&res, "x-content-type-options").as_deref(),
        Some("nosniff")
    );
    assert_eq!(
        hd(&res, "content-disposition").as_deref(),
        Some("attachment; filename=\"SKILL.md\"")
    );
    assert_eq!(
        hd(&res, "cache-control").as_deref(),
        Some("private, no-store")
    );
    let res = h
        .get_raw(
            &format!("{base}/skills/gdemo-app-steps/file?path=scripts/run.sh"),
            &[("cookie", &a.cookie)],
        )
        .await;
    assert_eq!((res.0, res.2.as_slice()), (200, script));
    let res = h
        .get_raw(&format!("{base}/operations"), &[("cookie", &a.cookie)])
        .await;
    assert_eq!((res.0, res.2.as_slice()), (200, ops));
    assert_eq!(
        hd(&res, "content-type").as_deref(),
        Some("text/plain; charset=utf-8")
    );
    for bad in [
        format!(
            "/v1/admin/plugins/{}/apps/gdemo-app/operations",
            Uuid::now_v7()
        ),
        format!("/v1/admin/plugins/{id}/apps/nope/operations"),
        format!("{base}/skills/nope/file"),
        format!("{base}/skills/gdemo-app-steps/file?path=scripts/other.sh"),
        // a file of the package that is not a skill file of this skill
        format!("{base}/skills/gdemo-app-steps/file?path=plugin.yaml"),
        format!("/v1/admin/plugins/{id}/apps/nope/skills/gdemo-app-steps/file"),
    ] {
        let r = h.get_raw(&bad, &[("cookie", &a.cookie)]).await;
        assert_eq!(r.0, 404, "{bad}");
    }
    for bad in ["../../x", "a b", "a/b/c/d/e/f"] {
        let r = h
            .get_raw(
                &format!(
                    "{base}/skills/gdemo-app-steps/file?path={}",
                    bad.replace(' ', "%20")
                ),
                &[("cookie", &a.cookie)],
            )
            .await;
        assert_eq!(r.0, 400, "{bad}");
    }
    // The old route is gone.
    let r = h
        .get_raw(
            &format!("/v1/admin/plugins/{id}/agent-guides/gdemo-app/api.md"),
            &[("cookie", &a.cookie)],
        )
        .await;
    assert_eq!(r.0, 404);
    // Not an instance admin: refused, for both routes.
    let user = h.user("member@x.io", false).await;
    let (_, tok) = h.good_session(user).await;
    for uri in [
        format!("{base}/skills/gdemo-app-steps/file"),
        format!("{base}/operations"),
    ] {
        let r = h.get_raw(&uri, &[("cookie", &cookie(&tok))]).await;
        assert_eq!(r.0, 403, "{uri}");
    }
}

fn v2_yaml() -> String {
    // A new host claim is a widening, so the new version waits for approval and can be compared.
    yaml("api.gdemo.example", "")
        .replace("version: 1.0.0", "version: 1.1.0")
        .replace(
            "    hosts:\n",
            "    hosts:\n      - match: { host: \"api2.gdemo.example\" }\n        inject:\n          - bearer: \"{credentials.api_token}\"\n",
        )
}

/// `changed` against the active version, and `agent_changed` in the approve and activate audit rows
/// (the apps whose summary, essentials, operations file or skills differ), when a file changes, a
/// skill is added or removed, and when only a script's executable bit flips.
#[tokio::test]
async fn a_new_version_reports_what_changed_and_audits_it() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let md = skill_md("gdemo-app-steps", "Do the thing", "# Steps\n");
    let script: &[u8] = b"#!/bin/sh\necho hi\n";
    let v1 = a
        .upload_with(
            &yaml("api.gdemo.example", ""),
            &[(SKILL, &md), (SCRIPT, script)],
        )
        .await;
    let (s, p) = a.act(&v1["id"], "approve").await;
    assert_eq!(s, 200, "{p}");

    // 1.1.0: the script's bit flips, nothing else (a skill change is not a widening, so the host
    // claim is what keeps this one pending).
    let v = a
        .upload_with_exec(&v2_yaml(), &[(SKILL, &md), (SCRIPT, script)], &[SCRIPT])
        .await;
    assert_eq!(v["status"], "pending_approval", "{v}");
    let d = a.status(&v["id"]).await;
    let c = &d["agent"][0]["changed"];
    assert_eq!(c["summary"], false, "{d}");
    assert_eq!(c["instructions"], false);
    assert_eq!(c["operations"], false);
    assert_eq!(
        c["skills"],
        json!([{"name": "gdemo-app-steps", "change": "changed",
                "files": [{"path": "scripts/run.sh", "change": "mode"}]}])
    );
    let (s, p) = a.act(&v["id"], "approve").await;
    assert_eq!(s, 200, "{p}");
    for action in ["plugin.approve", "plugin.activate"] {
        let meta: Value = wait_audit_meta(&h, action, v["id"].as_str().unwrap()).await;
        assert_eq!(
            meta["agent_changed"],
            json!(["gdemo-app"]),
            "{action}: {meta}"
        );
        assert!(meta.get("agent_guide_changed").is_none());
    }

    // 1.2.0 (automatic: no widening): a file's content changes, a skill is added.
    let v3 = v2_yaml().replace("version: 1.1.0", "version: 1.2.0");
    let md2 = skill_md("gdemo-app-api", "The API", "# API\n");
    let v = a
        .upload_with_exec(
            &v3,
            &[
                (SKILL, &md),
                (SCRIPT, b"#!/bin/sh\necho changed\n"),
                (SKILL2, &md2),
            ],
            &[SCRIPT],
        )
        .await;
    assert_eq!(v["status"], "active", "{v}");
    let meta = wait_audit_meta(&h, "plugin.activate", v["id"].as_str().unwrap()).await;
    assert_eq!(meta["auto"], true);
    assert_eq!(meta["agent_changed"], json!(["gdemo-app"]), "{meta}");

    // 1.3.0 (automatic): a skill is removed.
    let v4 = v2_yaml().replace("version: 1.1.0", "version: 1.3.0");
    let v = a
        .upload_with_exec(
            &v4,
            &[(SKILL, &md), (SCRIPT, b"#!/bin/sh\necho changed\n")],
            &[SCRIPT],
        )
        .await;
    assert_eq!(v["status"], "active", "{v}");
    let meta = wait_audit_meta(&h, "plugin.activate", v["id"].as_str().unwrap()).await;
    assert_eq!(meta["agent_changed"], json!(["gdemo-app"]), "{meta}");

    // 1.4.0 (automatic): nothing about the agent files differs: no key.
    let v5 = v2_yaml().replace("version: 1.1.0", "version: 1.4.0");
    let v = a
        .upload_with_exec(
            &v5,
            &[(SKILL, &md), (SCRIPT, b"#!/bin/sh\necho changed\n")],
            &[SCRIPT],
        )
        .await;
    assert_eq!(v["status"], "active", "{v}");
    let meta = wait_audit_meta(&h, "plugin.activate", v["id"].as_str().unwrap()).await;
    assert!(meta.get("agent_changed").is_none(), "{meta}");
}
