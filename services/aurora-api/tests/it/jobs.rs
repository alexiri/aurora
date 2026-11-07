#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::common::*;
use aurora_api::jobs;
use aurora_db::repo::request_log;
use chrono::{Datelike, Utc};
use uuid::Uuid;

async fn put(h: &Harness, gw: Uuid, deadline_off: i64) -> Uuid {
    let (org, ws) = {
        let o = h.org().await;
        (o, h.workspace(o).await)
    };
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO approvals (id, org_id, workspace_id, agent_id, gateway_instance_id, method, host, path, summary, status, deadline)
         VALUES ($1,$2,$3,$4,$5,'GET','h','/','{}','pending', now() + make_interval(secs => $6))",
    )
    .bind(id).bind(org).bind(ws).bind(Uuid::now_v7()).bind(gw).bind(deadline_off as f64)
    .execute(h.pool()).await.unwrap();
    id
}

async fn gw(h: &Harness, seen_off: i64) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO gateway_instances (id, hostname, version, started_at, last_seen_at) VALUES ($1,'h','0',now(), now() + make_interval(secs => $2))")
        .bind(id).bind(seen_off as f64).execute(h.pool()).await.unwrap();
    id
}

async fn status(h: &Harness, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM approvals WHERE id=$1")
        .bind(id)
        .fetch_one(h.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn expiry_and_abandon_transitions() {
    let h = Harness::new().await;
    let live = gw(&h, 0).await;
    let stale = gw(&h, -120).await;
    let overdue = put(&h, live, -31).await;
    let within_grace = put(&h, live, -10).await;
    let fresh = put(&h, live, 300).await;
    let orphan = put(&h, stale, 300).await;
    let mut l = aurora_db::bus::Listener::connect(h.pool(), &[aurora_db::bus::Channel::Approval])
        .await
        .unwrap();

    assert_eq!(jobs::sweep_approvals(h.pool()).await.unwrap(), (1, 1));
    assert_eq!(status(&h, overdue).await, "expired");
    assert_eq!(status(&h, within_grace).await, "pending");
    assert_eq!(status(&h, fresh).await, "pending");
    assert_eq!(status(&h, orphan).await, "abandoned");
    let decided: Option<bool> =
        sqlx::query_scalar("SELECT decided_at IS NOT NULL FROM approvals WHERE id=$1")
            .bind(overdue)
            .fetch_optional(h.pool())
            .await
            .unwrap();
    assert_eq!(decided, Some(true));
    // notified `expired` for both
    let mut n = 0;
    while n < 2 {
        if let aurora_db::bus::BusEvent::Notification { payload, .. } =
            tokio::time::timeout(std::time::Duration::from_secs(5), l.next())
                .await
                .unwrap()
        {
            assert_eq!(payload["event"], "expired");
            n += 1;
        }
    }
    // idempotent
    assert_eq!(jobs::sweep_approvals(h.pool()).await.unwrap(), (0, 0));
}

#[tokio::test]
async fn partition_job_idempotent_moves_default_rows_and_drops_old() {
    let h = Harness::new().await;
    let now = Utc::now();
    // A row in the default partition for the current month must survive partition creation.
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO request_log (id, ts, org_id, workspace_id, agent_id, credential_kind, lane, method, host, path, decision, status, latency_ms) VALUES ($1, now(), $2,$3,$4,'agent','mitm','GET','h','/','allow',200,1)")
        .bind(id).bind(org).bind(ws).bind(Uuid::now_v7()).execute(h.pool()).await.unwrap();

    assert!(jobs::maintain_request_log(h.pool(), 30).await.unwrap());
    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pg_inherits i JOIN pg_class c ON c.oid=i.inhrelid WHERE i.inhparent='request_log'::regclass AND c.relname ~ '^request_log_p[0-9]{6}$'")
            .fetch_one(h.pool()).await.unwrap()
    };
    assert_eq!(count().await, 3);
    let cur = format!("request_log_p{:04}{:02}", now.year(), now.month());
    let in_part: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {cur} WHERE id = $1"))
        .bind(id)
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(in_part, 1);
    let in_default: i64 = sqlx::query_scalar("SELECT count(*) FROM request_log_default")
        .fetch_one(h.pool())
        .await
        .unwrap();
    assert_eq!(in_default, 0);

    assert!(jobs::maintain_request_log(h.pool(), 30).await.unwrap());
    assert_eq!(count().await, 3);
    assert!(
        request_log::ensure_partitions(h.pool(), now, 2)
            .await
            .unwrap()
            .is_empty()
    );

    // Old partition is created for a past month, then dropped by retention.
    let old = now - chrono::Duration::days(120);
    let created = request_log::ensure_partitions(h.pool(), old, 0)
        .await
        .unwrap();
    assert_eq!(created.len(), 1);
    assert_eq!(count().await, 4);
    let dropped = request_log::drop_expired(h.pool(), now, 30).await.unwrap();
    assert_eq!(dropped, created);
    assert_eq!(count().await, 3);
}
