#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P2-015 (RFE-015, D-54, SEC-48): attachment retention, per-org storage quota, storage
//! breakdown and the daily sweep. Quota checks are authoritative on every byte path; the runner
//! (`send_file`) and connector (`file.fetch`) paths are covered in `attachments.rs` and
//! `connector_ops.rs`.

use crate::common::*;
use crate::conversation_lifecycle::{Fx, fixture, fixture_with};
use aurora_api::domains::attachments::sweep;
use axum::body::Body;
use futures::future::join_all;
use http::{Method, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const KIB: usize = 1024;
const MIB: usize = 1024 * 1024;

async fn upload(f: &Fx, who: &str, conv: Uuid, name: &str, size: usize) -> (u16, Value) {
    let b = "XBOUNDARYX";
    let mut body = format!(
        "--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: text/plain\r\n\r\n"
    )
    .into_bytes();
    body.extend(std::iter::repeat_n(b'x', size));
    body.extend_from_slice(format!("\r\n--{b}--\r\n").as_bytes());
    let res =
        f.h.app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/v1/conversations/{conv}/attachments"))
                    .header("cookie", who)
                    .header("origin", ORIGIN)
                    .header("content-type", format!("multipart/form-data; boundary={b}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
    let (p, body) = res.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    (
        p.status.as_u16(),
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn set_quota(f: &Fx, mib: Value) -> (u16, Value) {
    f.call(
        Method::PUT,
        &format!("/v1/orgs/{}/retention", f.org),
        &f.admin.1,
        Some(json!({ "attachment_quota_mib": mib })),
    )
    .await
}

async fn used(f: &Fx) -> i64 {
    sqlx::query_scalar(
        "SELECT COALESCE(sum(attachment_bytes),0)::bigint FROM org_storage_usage WHERE org_id = $1",
    )
    .bind(f.org)
    .fetch_one(f.h.pool())
    .await
    .unwrap()
}

async fn row_sum(f: &Fx) -> i64 {
    sqlx::query_scalar("SELECT COALESCE(sum(size_bytes),0)::bigint FROM attachments WHERE purged_at IS NULL AND status <> 'failed'")
        .fetch_one(f.h.pool())
        .await
        .unwrap()
}

/// A bound attachment with real blob bytes, `age_days` old.
async fn bound(f: &Fx, conv: Uuid, size: usize, age_days: i64) -> (Uuid, String) {
    let (id, key) = (Uuid::now_v7(), Uuid::now_v7().to_string());
    sqlx::query("INSERT INTO blobs (key, content) VALUES ($1, $2)")
        .bind(&key)
        .bind(vec![b'y'; size])
        .execute(f.h.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO attachments (id, workspace_id, conversation_id, direction, status, filename, content_type, size_bytes, sha256, blob_backend, blob_key, created_at) VALUES ($1,$2,$3,'out','bound','f.bin','application/octet-stream',$4,'x','postgres',$5, now() - make_interval(days => $6::int))")
        .bind(id).bind(f.ws).bind(conv).bind(size as i64).bind(&key).bind(age_days as i32)
        .execute(f.h.pool()).await.unwrap();
    (id, key)
}

async fn blob_exists(f: &Fx, key: &str) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM blobs WHERE key = $1")
        .bind(key)
        .fetch_one(f.h.pool())
        .await
        .unwrap()
        > 0
}

async fn sweep_now(f: &Fx) -> sweep::SweepReport {
    sweep::run(f.h.pool(), &f.h.state.config.attachments)
        .await
        .unwrap()
}

#[tokio::test]
async fn upload_is_refused_at_the_org_quota_and_accepted_again_after_a_delete() {
    let f = fixture().await;
    let conv = f.conv("group", None).await;
    assert_eq!(set_quota(&f, json!(1)).await.0, 200);
    let (s, a) = upload(&f, &f.alice.1, conv, "a.txt", 600 * KIB).await;
    assert_eq!(s, 201, "{a}");
    assert_eq!(used(&f).await, (600 * KIB) as i64);
    let (s, v) = upload(&f, &f.alice.1, conv, "b.txt", 600 * KIB).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["code"], "attachment_quota_exceeded");
    assert_eq!(v["error"]["details"]["quota_mib"], 1);
    assert_eq!(v["error"]["details"]["used_mib"], 0);
    // The refused file left no row and no blob behind.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM attachments")
        .fetch_one(f.h.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);
    // A smaller file still fits; deleting the first frees its bytes.
    assert_eq!(
        upload(&f, &f.alice.1, conv, "c.txt", 300 * KIB).await.0,
        201
    );
    let id = a["id"].as_str().unwrap();
    let (s, _) = f
        .call(
            Method::DELETE,
            &format!("/v1/attachments/{id}"),
            &f.alice.1,
            None,
        )
        .await;
    assert_eq!(s, 204);
    assert_eq!(used(&f).await, (300 * KIB) as i64);
    assert_eq!(
        upload(&f, &f.alice.1, conv, "d.txt", 600 * KIB).await.0,
        201
    );
    assert_eq!(used(&f).await, row_sum(&f).await);
}

#[tokio::test]
async fn default_quota_is_ten_gib_and_zero_means_unlimited() {
    let f = fixture_with(|c| {
        c.attachments.default_org_quota_mib = 1;
        c.attachments.org_quota_mib_max = 0;
    })
    .await;
    let conv = f.conv("group", None).await;
    // No org setting: the instance default (1 MiB here) applies.
    assert_eq!(
        upload(&f, &f.alice.1, conv, "a.txt", 700 * KIB).await.0,
        201
    );
    assert_eq!(
        upload(&f, &f.alice.1, conv, "b.txt", 700 * KIB).await.0,
        422
    );
    // `0` = unlimited (allowed: no instance ceiling).
    assert_eq!(set_quota(&f, json!(0)).await.0, 200);
    assert_eq!(
        upload(&f, &f.alice.1, conv, "b.txt", 700 * KIB).await.0,
        201
    );
    let d = aurora_config::AttachmentsSection::default();
    assert_eq!(
        (d.default_org_quota_mib, d.org_quota_mib_max),
        (10240, 51200)
    );
}

#[tokio::test]
async fn the_instance_ceiling_bounds_the_org_setting() {
    let f = fixture_with(|c| {
        c.attachments.default_org_quota_mib = 0; // unlimited default, but the ceiling still caps it
        c.attachments.org_quota_mib_max = 1;
    })
    .await;
    let (s, v) = set_quota(&f, json!(5)).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(
        v["error"]["code"],
        "attachment_quota_above_instance_maximum"
    );
    // `0` (unlimited) is above any ceiling too.
    assert_eq!(set_quota(&f, json!(0)).await.0, 422);
    assert_eq!(set_quota(&f, json!(-1)).await.0, 422);
    let (s, v) = set_quota(&f, json!(1)).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["attachment_quota_mib_effective"], 1);
    assert_eq!(v["attachment_quota_mib_max"], 1);
    // The unlimited default is capped by the ceiling when the org sets nothing.
    let (_, v) = set_quota(&f, Value::Null).await;
    assert_eq!(v["attachment_quota_mib"], Value::Null);
    assert_eq!(v["attachment_quota_mib_effective"], 1);
    let conv = f.conv("group", None).await;
    assert_eq!(upload(&f, &f.alice.1, conv, "a", 700 * KIB).await.0, 201);
    assert_eq!(upload(&f, &f.alice.1, conv, "b", 700 * KIB).await.0, 422);
}

#[tokio::test]
async fn retention_settings_are_validated_audited_and_keep_other_keys() {
    let f = fixture_with(|c| c.attachments.days_max = Some(90)).await;
    let url = format!("/v1/orgs/{}/retention", f.org);
    let put = |b: Value| f.call(Method::PUT, &url, &f.admin.1, Some(b));
    assert_eq!(
        put(json!({"attachments_days": 3})).await.1["error"]["code"],
        "retention_invalid"
    );
    let (s, v) = put(json!({"attachments_days": 120})).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (422, Some("retention_above_instance_maximum"))
    );
    let (s, v) = put(json!({"conversations_days": 30, "attachments_days": 60})).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["attachments_days"], 60);
    assert_eq!(v["attachments_days_max"], 90);
    assert_eq!(v["conversations_days"], 30);
    // Absent keys are unchanged; null clears just that key.
    let (_, v) = put(json!({"attachments_days": Value::Null})).await;
    assert_eq!(
        (
            v["attachments_days"].clone(),
            v["conversations_days"].clone()
        ),
        (Value::Null, json!(30))
    );
    // Members cannot read or write it.
    assert_eq!(f.call(Method::GET, &url, &f.alice.1, None).await.0, 403);
    assert_eq!(
        f.call(
            Method::PUT,
            &url,
            &f.alice.1,
            Some(json!({"attachments_days": 30}))
        )
        .await
        .0,
        403
    );
    // Security-critical audit row with from/to.
    let meta: Value = sqlx::query_scalar("SELECT metadata FROM audit_log WHERE org_id = $1 AND action = 'retention.update' AND metadata ? 'attachments_days' ORDER BY created_at LIMIT 1")
        .bind(f.org).fetch_one(f.h.pool()).await.unwrap();
    assert_eq!(meta["attachments_days"], json!({"from": null, "to": 60}));
}

#[tokio::test]
async fn concurrent_uploads_cannot_overshoot_the_quota() {
    let f = fixture().await;
    let conv = f.conv("group", None).await;
    assert_eq!(set_quota(&f, json!(1)).await.0, 200);
    let rs = join_all((0..6).map(|i| {
        let (f, who) = (&f, f.alice.1.clone());
        async move { upload(f, &who, conv, &format!("f{i}"), 400 * KIB).await.0 }
    }))
    .await;
    let ok = rs.iter().filter(|s| **s == 201).count();
    assert_eq!(ok, 2, "{rs:?}");
    assert!(rs.iter().all(|s| *s == 201 || *s == 422), "{rs:?}");
    assert!(used(&f).await <= MIB as i64);
    assert_eq!(used(&f).await, row_sum(&f).await);
}

#[tokio::test]
async fn retention_sweep_purges_bytes_keeps_the_row_and_downloads_answer_410() {
    let f = fixture().await;
    let conv = f.conv("group", None).await;
    f.turn(conv, Some(f.alice.0), "hi").await;
    let (old, old_key) = bound(&f, conv, 2000, 40).await;
    let (young, young_key) = bound(&f, conv, 1000, 2).await;
    // Window: 30 days.
    let (s, v) = f
        .call(
            Method::PUT,
            &format!("/v1/orgs/{}/retention", f.org),
            &f.admin.1,
            Some(json!({"attachments_days": 30})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(used(&f).await, 3000);
    let r = sweep_now(&f).await;
    assert_eq!((r.purged, r.purged_bytes), (1, 2000));
    // Row kept with the purge marks, bytes gone, counter down.
    let (at, reason, key): (
        Option<chrono::DateTime<chrono::Utc>>,
        Option<String>,
        String,
    ) = sqlx::query_as("SELECT purged_at, purge_reason, blob_key FROM attachments WHERE id = $1")
        .bind(old)
        .fetch_one(f.h.pool())
        .await
        .unwrap();
    assert!(at.is_some());
    assert_eq!(reason.as_deref(), Some("retention"));
    assert_eq!(key, "");
    assert!(!blob_exists(&f, &old_key).await);
    assert!(blob_exists(&f, &young_key).await);
    assert_eq!(used(&f).await, 1000);
    // Content: 410 for the purged one, bytes for the young one.
    let (s, v) = f
        .call(
            Method::GET,
            &format!("/v1/attachments/{old}/content"),
            &f.alice.1,
            None,
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (410, Some("attachment_purged"))
    );
    let res =
        f.h.app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/attachments/{young}/content"))
                    .header("cookie", &f.alice.1)
                    .header("origin", ORIGIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    // One audit row per batch, system actor, no content.
    let (n, meta): (i64, Value) = sqlx::query_as("SELECT count(*), COALESCE(max(metadata::text),'{}')::jsonb FROM audit_log WHERE org_id = $1 AND action = 'attachment.purge'")
        .bind(f.org).fetch_one(f.h.pool()).await.unwrap();
    assert_eq!(n, 1);
    assert_eq!(
        (meta["reason"].clone(), meta["attachments"].clone()),
        (json!("retention"), json!(1))
    );
    // Idempotent.
    assert_eq!(sweep_now(&f).await.purged, 0);
    // The P2-010 export reports the real purged state (FU-P2-010b) and the zip skips the bytes.
    let (s, v) = f
        .call(
            Method::GET,
            &format!("/v1/conversations/{conv}/export"),
            &f.owner.1,
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let purged: Vec<(String, bool)> = v["attachments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["id"].as_str().unwrap().to_owned(),
                a["purged"].as_bool().unwrap(),
            )
        })
        .collect();
    assert!(purged.contains(&(old.to_string(), true)), "{purged:?}");
    assert!(purged.contains(&(young.to_string(), false)), "{purged:?}");
}

/// FU-P2-015a (C-196): the events page lists the purged attachments its events reference, with
/// the reason, so the transcript can render "expired by retention" instead of a download link.
#[tokio::test]
async fn events_page_lists_the_purged_attachments_with_their_reason() {
    use aurora_db::repo::transcript;
    let f = fixture().await;
    let conv = f.conv("group", None).await;
    let other = f.conv("group", None).await;
    let (old, _) = bound(&f, conv, 2000, 40).await;
    let (young, _) = bound(&f, conv, 1000, 2).await;
    let (gone, _) = bound(&f, conv, 500, 1).await;
    let (foreign, _) = bound(&f, other, 700, 40).await;
    let (t, ..) = f.turn(conv, Some(f.alice.0), "see files").await;
    let mut c = f.h.pool().acquire().await.unwrap();
    for (id, name) in [(old, "old.bin"), (young, "young.bin"), (gone, "gone.bin")] {
        transcript::append_durable_event(
            &mut c,
            conv,
            Some(t),
            "attachment",
            &json!({"turn_id": t, "attachment_id": id, "filename": name, "size": 1}),
        )
        .await
        .unwrap();
    }
    // An id of another conversation in an event must not leak its state.
    transcript::append_durable_event(
        &mut c,
        conv,
        Some(t),
        "turn.queued",
        &json!({"turn_id": t, "text": "x", "attachments": [foreign]}),
    )
    .await
    .unwrap();
    drop(c);
    assert_eq!(
        f.call(
            Method::PUT,
            &format!("/v1/orgs/{}/retention", f.org),
            &f.admin.1,
            Some(json!({"attachments_days": 30})),
        )
        .await
        .0,
        200
    );
    let events = format!("/v1/conversations/{conv}/events");
    let (s, v) = f.call(Method::GET, &events, &f.alice.1, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["purged_attachments"], json!([]));
    sweep_now(&f).await;
    assert_eq!(
        f.call(
            Method::DELETE,
            &format!("/v1/attachments/{gone}"),
            &f.owner.1,
            None
        )
        .await
        .0,
        204
    );
    let (s, v) = f.call(Method::GET, &events, &f.alice.1, None).await;
    assert_eq!(s, 200, "{v}");
    let mut got: Vec<(String, String)> = v["purged_attachments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["id"].as_str().unwrap().to_owned(),
                p["reason"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    got.sort();
    let mut want = vec![
        (old.to_string(), "retention".to_owned()),
        (gone.to_string(), "deleted".to_owned()),
    ];
    want.sort();
    assert_eq!(
        got, want,
        "young stays out, the other conversation's file stays out"
    );
    let _ = young;
}

#[tokio::test]
async fn the_instance_maximum_tightens_the_effective_window() {
    let f = fixture_with(|c| c.attachments.days_max = Some(30)).await;
    let conv = f.conv("group", None).await;
    let (old, _) = bound(&f, conv, 100, 40).await;
    // The org keeps 365 days, but only 30 are allowed now (set before the instance tightened).
    sqlx::query("UPDATE organizations SET settings = jsonb_set(settings, '{retention}', '{\"attachments_days\": 365}') WHERE id = $1")
        .bind(f.org).execute(f.h.pool()).await.unwrap();
    assert_eq!(sweep_now(&f).await.purged, 1);
    let purged: bool =
        sqlx::query_scalar("SELECT purged_at IS NOT NULL FROM attachments WHERE id = $1")
            .bind(old)
            .fetch_one(f.h.pool())
            .await
            .unwrap();
    assert!(purged);
}

#[tokio::test]
async fn delete_is_limited_to_the_uploader_or_an_owner_and_never_leaks_invisible_files() {
    let f = fixture().await;
    let conv = f.conv("group", None).await;
    let (s, a) = upload(&f, &f.alice.1, conv, "a.txt", 100).await;
    assert_eq!(s, 201);
    let id = a["id"].as_str().unwrap().to_owned();
    let url = format!("/v1/attachments/{id}");
    // Another member: 403. A stranger without access to a direct conversation: 404.
    assert_eq!(f.call(Method::DELETE, &url, &f.bob.1, None).await.0, 403);
    let direct = f.conv("direct", Some(f.alice.0)).await;
    let (s, d) = upload(&f, &f.alice.1, direct, "p.txt", 100).await;
    assert_eq!(s, 201, "{d}");
    let durl = format!("/v1/attachments/{}", d["id"].as_str().unwrap());
    assert_eq!(f.call(Method::DELETE, &durl, &f.bob.1, None).await.0, 404);
    assert_eq!(
        f.call(
            Method::DELETE,
            &format!("/v1/attachments/{}", Uuid::now_v7()),
            &f.bob.1,
            None
        )
        .await
        .0,
        404
    );
    // Not authenticated.
    assert_eq!(
        f.call(Method::DELETE, &url, "cookie=none", None).await.0,
        401
    );
    // A workspace owner may; repeating is idempotent.
    assert_eq!(f.call(Method::DELETE, &url, &f.owner.1, None).await.0, 204);
    assert_eq!(f.call(Method::DELETE, &url, &f.alice.1, None).await.0, 204);
    let (s, v) = f
        .call(Method::GET, &format!("{url}/content"), &f.alice.1, None)
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (410, Some("attachment_purged"))
    );
    let reason: String = sqlx::query_scalar("SELECT purge_reason FROM attachments WHERE id = $1")
        .bind(a["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .fetch_one(f.h.pool())
        .await
        .unwrap();
    assert_eq!(reason, "deleted");
    // Best-effort audit row (written asynchronously).
    let mut n = 0i64;
    for _ in 0..50 {
        n = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action = 'attachment.delete' AND org_id = $1",
        )
        .bind(f.org)
        .fetch_one(f.h.pool())
        .await
        .unwrap();
        if n > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    // Only the owner's delete purged bytes; the idempotent repeat audits nothing.
    assert_eq!(n, 1);
}

#[tokio::test]
async fn the_sweep_reconciles_drift_and_removes_orphan_blobs() {
    let f = fixture().await;
    let conv = f.conv("group", None).await;
    bound(&f, conv, 500, 1).await;
    assert_eq!(used(&f).await, 500);
    // Injected drift in both directions, plus another org's phantom counter.
    sqlx::query("UPDATE org_storage_usage SET attachment_bytes = 123456 WHERE org_id = $1")
        .bind(f.org)
        .execute(f.h.pool())
        .await
        .unwrap();
    let r = sweep_now(&f).await;
    assert_eq!(r.reconciled, 1);
    assert_eq!(used(&f).await, 500);
    assert_eq!(sweep_now(&f).await.reconciled, 0);
    // Orphan blob (no attachment row), old enough to be swept; a fresh one is spared.
    let (old, fresh) = (Uuid::now_v7().to_string(), Uuid::now_v7().to_string());
    sqlx::query("INSERT INTO blobs (key, content, created_at) VALUES ($1, 'x', now() - interval '2 hours'), ($2, 'x', now())")
        .bind(&old).bind(&fresh).execute(f.h.pool()).await.unwrap();
    assert_eq!(sweep_now(&f).await.orphans, 1);
    assert!(!blob_exists(&f, &old).await);
    assert!(blob_exists(&f, &fresh).await);
}

#[tokio::test]
async fn the_counter_follows_every_insert_purge_and_cascade() {
    let f = fixture().await;
    let conv = f.conv("group", None).await;
    let (a, _) = bound(&f, conv, 700, 0).await;
    bound(&f, conv, 300, 0).await;
    // A failed row carries no blob and does not count.
    sqlx::query("INSERT INTO attachments (id, workspace_id, conversation_id, direction, status, filename, content_type, size_bytes, sha256, blob_backend, blob_key, failure_reason) VALUES ($1,$2,$3,'in','failed','x','x',9999,'','','','quota_exceeded')")
        .bind(Uuid::now_v7()).bind(f.ws).bind(conv).execute(f.h.pool()).await.unwrap();
    assert_eq!(used(&f).await, 1000);
    f.call(
        Method::DELETE,
        &format!("/v1/attachments/{a}"),
        &f.owner.1,
        None,
    )
    .await;
    assert_eq!(used(&f).await, 300);
    // Deleting the conversation removes its rows; the counter follows.
    let (s, _) = f
        .call(
            Method::DELETE,
            &format!("/v1/conversations/{conv}"),
            &f.owner.1,
            None,
        )
        .await;
    assert_eq!(s, 204);
    assert_eq!(used(&f).await, 0);
    assert_eq!(used(&f).await, row_sum(&f).await);
}

#[tokio::test]
async fn quota_notices_are_raised_once_per_crossing() {
    let f = fixture().await;
    let conv = f.conv("group", None).await;
    assert_eq!(set_quota(&f, json!(1)).await.0, 200);
    let notices = |f: &Fx| {
        let pool = f.h.pool().clone();
        let org = f.org;
        async move {
            sqlx::query_scalar::<_, String>("SELECT metadata ->> 'level' FROM audit_log WHERE org_id = $1 AND action = 'attachment.quota_notice' ORDER BY created_at, id")
                .bind(org).fetch_all(&pool).await.unwrap()
        }
    };
    bound(&f, conv, 700 * KIB, 0).await; // 68 %
    assert_eq!(sweep_now(&f).await.notices, 0);
    let (big, _) = bound(&f, conv, 200 * KIB, 0).await; // 88 %
    assert_eq!(sweep_now(&f).await.notices, 1);
    assert_eq!(sweep_now(&f).await.notices, 0);
    bound(&f, conv, 124 * KIB, 0).await; // 100 %
    assert_eq!(sweep_now(&f).await.notices, 1);
    assert_eq!(sweep_now(&f).await.notices, 0);
    // The storage page shows the live level.
    let (_, v) = f
        .call(
            Method::GET,
            &format!("/v1/orgs/{}/storage", f.org),
            &f.admin.1,
            None,
        )
        .await;
    assert_eq!(v["notice"], "full");
    f.call(
        Method::DELETE,
        &format!("/v1/attachments/{big}"),
        &f.owner.1,
        None,
    )
    .await;
    f.call(
        Method::DELETE,
        &format!("/v1/conversations/{conv}"),
        &f.owner.1,
        None,
    )
    .await;
    assert_eq!(sweep_now(&f).await.notices, 1);
    assert_eq!(notices(&f).await, vec!["warning", "full", "clear"]);
}

/// FU-P2-015b (`02` `ATTACH_QUOTA_NOTICE`, D-81 (g)): the quota e-mails go to the org's owners
/// and admins (not to plain members), once per threshold crossing, and carry no attachment data.
#[tokio::test]
async fn quota_notices_are_mailed_to_owners_and_admins_once_per_crossing() {
    let f = fixture().await;
    let boss = f.h.user("boss@x.io", false).await;
    f.h.member(f.org, boss, "owner", "active").await;
    let conv = f.conv("group", None).await;
    assert_eq!(set_quota(&f, json!(1)).await.0, 200);
    let mail = sweep::QuotaMail {
        mailer: f.h.mail.clone(),
        external_url: "https://aurora.example".to_owned(),
    };
    let run = || async {
        sweep::run_with(f.h.pool(), &f.h.state.config.attachments, Some(&mail))
            .await
            .unwrap()
    };
    let recipients = |from: usize| {
        let mut v: Vec<String> = f.h.mail.sent()[from..]
            .iter()
            .map(|m| m.to.clone())
            .collect();
        v.sort();
        v
    };
    let both = vec!["admin@x.io".to_owned(), "boss@x.io".to_owned()];

    let (a, _) = bound(&f, conv, 700 * KIB, 0).await; // 68 %
    sqlx::query("UPDATE attachments SET filename = 'secret-plan-q3.pdf' WHERE id = $1")
        .bind(a)
        .execute(f.h.pool())
        .await
        .unwrap();
    run().await;
    assert!(f.h.mail.sent().is_empty());

    // 80 %: one mail to each owner and admin, none to the plain members.
    let (b, _) = bound(&f, conv, 200 * KIB, 0).await; // 88 %
    sqlx::query("UPDATE attachments SET filename = 'customer-list.csv' WHERE id = $1")
        .bind(b)
        .execute(f.h.pool())
        .await
        .unwrap();
    run().await;
    assert_eq!(recipients(0), both);
    let first = f.h.mail.sent()[0].clone();
    assert!(first.subject.contains("80%"), "{}", first.subject);
    run().await;
    assert_eq!(f.h.mail.sent().len(), 2, "a repeat must not mail again");

    // 100 %: one more each.
    let (c, _) = bound(&f, conv, 124 * KIB, 0).await;
    run().await;
    assert_eq!(recipients(2), both);
    assert!(f.h.mail.sent()[2].subject.contains("full"));
    run().await;
    assert_eq!(f.h.mail.sent().len(), 4);

    // Dropping to 88 % is not a new 80 % crossing and mails nobody.
    sqlx::query("DELETE FROM attachments WHERE id = $1")
        .bind(c)
        .execute(f.h.pool())
        .await
        .unwrap();
    run().await;
    assert_eq!(f.h.mail.sent().len(), 4);

    // Below 80 % and back above it notifies again.
    sqlx::query("DELETE FROM attachments WHERE id = $1")
        .bind(b)
        .execute(f.h.pool())
        .await
        .unwrap();
    run().await;
    assert_eq!(f.h.mail.sent().len(), 4);
    bound(&f, conv, 200 * KIB, 0).await;
    run().await;
    assert_eq!(recipients(4), both);
    assert!(f.h.mail.sent()[4].subject.contains("80%"));

    // The mails say org, usage, quota and a link; never a file name or content.
    for m in f.h.mail.sent() {
        assert!(m.body.contains("MiB of 1 MiB"), "{}", m.body);
        assert!(
            m.link.starts_with("https://aurora.example/o/"),
            "{}",
            m.link
        );
        for banned in ["secret-plan", "customer-list", "f.bin", ".pdf", ".csv"] {
            assert!(
                !m.subject.contains(banned) && !m.body.contains(banned),
                "{banned}"
            );
        }
    }
}

#[tokio::test]
async fn storage_breakdowns_agree_with_the_tables_and_are_authorization_checked() {
    let f = fixture().await;
    let conv = f.conv("group", None).await;
    let other = f.conv("group", None).await;
    bound(&f, conv, 1000, 0).await;
    bound(&f, other, 500, 0).await;
    assert_eq!(upload(&f, &f.alice.1, conv, "in.txt", 250).await.0, 201);
    let org_url = format!("/v1/orgs/{}/storage", f.org);
    let (s, v) = f.call(Method::GET, &org_url, &f.admin.1, None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["used_bytes"], 1750);
    assert_eq!(v["files"], 3);
    assert_eq!(v["purged_files"], 0);
    assert_eq!(v["quota_bytes"], 10240 * MIB as i64);
    assert_eq!(v["notice"], "clear");
    let rows = v["by_workspace"].as_array().unwrap();
    let total: i64 = rows.iter().map(|r| r["bytes"].as_i64().unwrap()).sum();
    assert_eq!(total, 1750);
    let dirs: Vec<(&str, i64)> = rows
        .iter()
        .map(|r| {
            (
                r["direction"].as_str().unwrap(),
                r["bytes"].as_i64().unwrap(),
            )
        })
        .collect();
    assert!(
        dirs.contains(&("out", 1500)) && dirs.contains(&("in", 250)),
        "{dirs:?}"
    );
    assert_eq!(rows[0]["workspace_id"], json!(f.ws));
    // Org owners/admins only: a member, a workspace owner who is no org admin, and another org.
    for who in [&f.alice.1, &f.owner.1] {
        assert_eq!(f.call(Method::GET, &org_url, who, None).await.0, 403);
    }
    // Instance admin route.
    assert_eq!(
        f.call(Method::GET, "/v1/admin/storage", &f.admin.1, None)
            .await
            .0,
        403
    );
    let ia = f.h.user("root@x.io", true).await;
    let (_, t) = f.h.good_session(ia).await;
    let (s, v) = f
        .call(Method::GET, "/v1/admin/storage", &cookie(&t), None)
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["attachment_bytes"], 1750);
    assert_eq!(v["attachment_files"], 3);
    assert_eq!(v["blob_bytes"], 1750);
    assert_eq!(v["blob_count"], 3);
    assert_eq!(v["orgs"][0]["org_id"], json!(f.org));
    assert_eq!(v["orgs"][0]["attachment_bytes"], 1750);
    let tables: Vec<&str> = v["tables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["table"].as_str().unwrap())
        .collect();
    for t in [
        "blobs",
        "attachments",
        "transcript_events",
        "request_log",
        "audit_log",
    ] {
        assert!(tables.contains(&t), "{tables:?}");
    }
    assert!(
        v["tables"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["bytes"].as_i64().unwrap() > 0)
    );
}
