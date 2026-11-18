//! (P5-GUIDE-1, C-641, C-642) Ingest stores one `agent_guide` blob per guide file of a package
//! that is not `invalid`, readable by digest only; the compiled snapshot carries the digests.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use crate::plugins_lifecycle::setup;
use aurora_db::repo::plugins;
use serde_json::Value;
use sha2::{Digest, Sha256};

fn yaml(host: &str) -> String {
    format!(
        "package: {{ id: gdemo, version: 1.0.0, contract: 1, min_core: 0.1.0, publisher: acme, name: P }}\napps:\n  - about: {{ id: gdemo_app, name: A }}\n    credentials: [ {{ name: api_token, secret: true }} ]\n    connect:\n      - id: api_key\n        kind: fields\n        fields: [ {{ name: api_token, label: T, type: secret, required: true }} ]\n    hosts:\n      - match: {{ host: \"{host}\" }}\n        inject:\n          - bearer: \"{{credentials.api_token}}\"\n    agent:\n      summary: \"Demo things.\"\n      guide:\n        - {{ file: api.md, title: \"API\" }}\n        - {{ file: steps.md, title: \"Do the thing\", kind: recipe }}\n"
    )
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
async fn ingest_stores_guide_blobs_and_the_snapshot_carries_their_digests() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let api: &[u8] = b"# API\nreference\n";
    let steps: &[u8] = b"# Steps\n1. go\n";
    // A blob of another kind with the same bytes as a guide file already exists: it keeps its kind.
    let shared_digest = hex(steps);
    plugins::put_blob(h.pool(), &shared_digest, "setup_guide", steps)
        .await
        .unwrap();

    let v = a
        .upload_with(
            &yaml("api.gdemo.example"),
            &[
                ("agent/gdemo_app/api.md", api),
                ("agent/gdemo_app/steps.md", steps),
            ],
        )
        .await;
    assert_eq!(v["status"], "pending_approval", "{v}");
    assert_eq!(
        blob_kind(&h, &hex(api)).await.as_deref(),
        Some("agent_guide")
    );
    // Same bytes as an existing blob of another kind: the row is untouched ...
    assert_eq!(
        blob_kind(&h, &shared_digest).await.as_deref(),
        Some("setup_guide")
    );
    // ... and the guide file is readable through the by-digest function, not through the kind.
    assert_eq!(
        plugins::get_guide_blob(h.pool(), &shared_digest)
            .await
            .unwrap()
            .as_deref(),
        Some(steps)
    );
    assert!(
        plugins::get_blob(h.pool(), &shared_digest, "agent_guide")
            .await
            .unwrap()
            .is_none()
    );
    let guide_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM plugin_blobs WHERE kind = 'agent_guide'")
            .fetch_one(h.pool())
            .await
            .unwrap();
    assert_eq!(
        guide_rows, 1,
        "one blob per guide file (the other already existed)"
    );

    // Approval compiles the snapshot with the digests of the stored archive.
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
    assert_eq!(ag["guide"][0]["file"], "api.md");
    assert_eq!(ag["guide"][0]["kind"], "reference");
    assert_eq!(ag["guide"][0]["sha256"], hex(api));
    assert_eq!(ag["guide"][0]["size"], api.len());
    assert_eq!(ag["guide"][1]["kind"], "recipe");
    assert_eq!(ag["guide"][1]["sha256"], shared_digest);
}

#[tokio::test]
async fn an_invalid_package_stores_no_guide_blob() {
    let h = Harness::new().await;
    let a = setup(&h).await;
    let api: &[u8] = b"# never stored\n";
    let steps: &[u8] = b"# never stored either\n";
    // A reserved host makes the row `invalid` (the guide files themselves are fine).
    let v = a
        .upload_with(
            &yaml("x.aurora.internal"),
            &[
                ("agent/gdemo_app/api.md", api),
                ("agent/gdemo_app/steps.md", steps),
            ],
        )
        .await;
    assert_eq!(v["status"], "invalid", "{v}");
    assert!(blob_kind(&h, &hex(api)).await.is_none());
    assert!(blob_kind(&h, &hex(steps)).await.is_none());
}
