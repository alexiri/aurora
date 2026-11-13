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

/// (FIX-111 3) A request-log row one day past the cutoff in a month that is kept (its
/// partition straddles the cutoff) is removed, rows inside the period stay, and the same goes
/// for the default partition. Whole-month dropping alone kept rows 30 to 61 days.
#[tokio::test]
async fn request_log_rows_past_the_cutoff_are_removed_inside_a_kept_month() {
    let h = Harness::new().await;
    // A fixed "now" mid-month: the cutoff (30 days back) falls inside the previous month.
    let now = chrono::NaiveDate::from_ymd_opt(2027, 3, 20)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap()
        .and_utc();
    let (org, ws) = {
        let o = h.org().await;
        (o, h.workspace(o).await)
    };
    // Partitions for February and March 2027 (now - 30 d = 18 Feb: February straddles it).
    request_log::ensure_partitions(h.pool(), now - chrono::Duration::days(30), 1)
        .await
        .unwrap();
    let row = |id: Uuid, ts: chrono::DateTime<Utc>| {
        let pool = h.pool().clone();
        async move {
            sqlx::query("INSERT INTO request_log (id, ts, org_id, workspace_id, agent_id, credential_kind, lane, method, host, path, decision, status, latency_ms) VALUES ($1,$2,$3,$4,$5,'agent','mitm','GET','h','/','allow',200,1)")
                .bind(id).bind(ts).bind(org).bind(ws).bind(Uuid::now_v7())
                .execute(&pool).await.unwrap();
        }
    };
    let (stale, fresh, stale_default) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    // 31 days old: one day past the cutoff, in the February partition.
    row(stale, now - chrono::Duration::days(31)).await;
    // 29 days old: inside the period, same partition.
    row(fresh, now - chrono::Duration::days(29)).await;
    // Far in the past, no partition: lands in the default partition.
    row(stale_default, now - chrono::Duration::days(400)).await;
    let in_default: i64 =
        sqlx::query_scalar("SELECT count(*) FROM request_log_default WHERE id = $1")
            .bind(stale_default)
            .fetch_one(h.pool())
            .await
            .unwrap();
    assert_eq!(in_default, 1);
    request_log::drop_expired(h.pool(), now, 30).await.unwrap();
    let left = |id: Uuid| {
        let pool = h.pool().clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM request_log WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        left(stale).await,
        0,
        "one day past the cutoff, in a kept month"
    );
    assert_eq!(left(fresh).await, 1, "inside the period");
    assert_eq!(
        left(stale_default).await,
        0,
        "default partition trimmed too"
    );
}

/// (FIX-111 1, 2) The daily retention pass: rows older than each window go, newer ones stay,
/// a pending `presence.deprovision` op survives however old, a deprovision op whose sealed
/// copy is still set survives, a revoked session past the grace is gone, and webhook
/// deliveries past the retention or over the per-webhook cap are trimmed.
#[tokio::test]
async fn retention_pass_removes_only_what_each_window_allows() {
    let h = Harness::new().await;
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, effort, brief) VALUES ($1,$2,$3,'bot','Bot','byo','medium','')")
        .bind(agent).bind(org).bind(ws).execute(h.pool()).await.unwrap();
    let user = h.user("u@x.io", false).await;
    let old = "now() - interval '8 days'"; // past the 7-day windows
    let new = "now() - interval '1 day'";
    let ids: Vec<Uuid> = (0..24).map(|_| Uuid::now_v7()).collect();
    let q = |sql: String, binds: Vec<Uuid>| {
        let pool = h.pool().clone();
        async move {
            let mut qq = sqlx::query(&sql);
            for b in binds {
                qq = qq.bind(b);
            }
            qq.execute(&pool).await.unwrap();
        }
    };
    // channel_inbox: done+old (gone), done+new, pending+old (kept: not terminal).
    for (i, (status, when)) in [("done", old), ("done", new), ("pending", old)]
        .into_iter()
        .enumerate()
    {
        q(format!("INSERT INTO channel_inbox (id, connector_id, event_id, kind, payload, status, next_attempt_at, updated_at) VALUES ($1,'c',$1::text,'message','{{}}','{status}', now(), {when})"), vec![ids[i]]).await;
    }
    // channel_ops: done+old (gone), done+new, pending deprovision+old (kept),
    // done deprovision+old with the sealed copy still set (kept), done deprovision+old copy cleared (gone).
    for (i, (status, kind, payload, when)) in [
        ("done", "message.send", "{}", old),
        ("done", "message.send", "{}", new),
        (
            "pending",
            "presence.deprovision",
            "{\"credentials_ref\": \"ref\"}",
            old,
        ),
        (
            "done",
            "presence.deprovision",
            "{\"credentials_ref\": \"ref\"}",
            old,
        ),
        (
            "done",
            "presence.deprovision",
            "{\"credentials_ref\": null}",
            old,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        q(format!("INSERT INTO channel_ops (id, connector_id, kind, payload, status, updated_at) VALUES ($1,'c','{kind}','{payload}','{status}', {when})"), vec![ids[3 + i]]).await;
    }
    // channel_cards: settled+old (gone), settled+new, posted+old (kept).
    sqlx::query("INSERT INTO connectors (id, token_hash, display_name, manifest, status) VALUES ('chat', $1, 'chat', '{}', 'active')")
        .bind(vec![0u8; 32]).execute(h.pool()).await.unwrap();
    let integ = Uuid::now_v7();
    sqlx::query("INSERT INTO channel_integrations (id, org_id, connector_id, tenant_ref, tenant_label, config, credentials_ref, status) VALUES ($1,$2,'chat','T','T','{}','x','active')")
        .bind(integ).bind(org).execute(h.pool()).await.unwrap();
    for (i, (state, when)) in [("settled", old), ("settled", new), ("posted", old)]
        .into_iter()
        .enumerate()
    {
        q(format!("INSERT INTO channel_cards (id, subject_kind, subject_id, integration_id, post_op_id, space_ref, thread_ref, state, updated_at) VALUES ($1,'approval',$1,$2,$1,'s','t','{state}', {when})"), vec![ids[8 + i], integ]).await;
    }
    // link_requests: expired long ago (gone), expired recently, still valid.
    for (i, exp) in [old, new, "now() + interval '10 minutes'"]
        .into_iter()
        .enumerate()
    {
        q(format!("INSERT INTO link_requests (id, integration_id, platform_user_ref, token_hash, expires_at) VALUES ($1,$2,'U',decode(replace($1::text,'-','') || replace($1::text,'-',''),'hex'),{exp})"), vec![ids[11 + i], integ]).await;
    }
    // sessions: revoked 8 days ago (gone), revoked 1 day ago, expired 8 days ago (gone), live.
    let sess = |revoked: Option<&str>, expires: &str| {
        let pool = h.pool().clone();
        let (revoked, expires) = (revoked.map(str::to_owned), expires.to_owned());
        async move {
            let id = Uuid::now_v7();
            sqlx::query(&format!("INSERT INTO sessions (id, user_id, token_hash, last_seen_at, expires_at, revoked_at) VALUES ($1,$2,decode(replace($1::text,'-','') || replace($1::text,'-',''),'hex'), now(), {expires}, {})", revoked.as_deref().unwrap_or("NULL")))
                .bind(id).bind(user).execute(&pool).await.unwrap();
            id
        }
    };
    let s_revoked_old = sess(Some(old), "now() + interval '1 day'").await;
    let s_revoked_new = sess(Some(new), "now() + interval '1 day'").await;
    let s_expired_old = sess(None, old).await;
    let s_live = sess(None, "now() + interval '1 day'").await;
    // context credentials: expired long ago (gone), expired recently, agent credential untouched.
    for (i, (kind, exp)) in [("context", old), ("context", new), ("agent", old)]
        .into_iter()
        .enumerate()
    {
        q(format!("INSERT INTO gateway_credentials (id, kind, token_hash, display_prefix, agent_id, expires_at) VALUES ($1,'{kind}',decode(replace($1::text,'-','') || replace($1::text,'-',''),'hex'),'auc_x',$2,{exp})"), vec![ids[14 + i], agent]).await;
    }
    // webhook deliveries: one old (gone), one new, plus a flood over the cap on another webhook.
    let hook = |n: usize| {
        let pool = h.pool().clone();
        async move {
            let id = Uuid::now_v7();
            sqlx::query("INSERT INTO webhooks (id, agent_id, name, instructions, token_hash, display_prefix) VALUES ($1,$2,$3,'x',decode(replace($1::text,'-','') || replace($1::text,'-',''),'hex'),'auw_x')")
                .bind(id).bind(agent).bind(format!("h{n}")).execute(&pool).await.unwrap();
            id
        }
    };
    let (h1, h2) = (hook(1).await, hook(2).await);
    sqlx::query("INSERT INTO webhook_deliveries (id, webhook_id, status, size_bytes, created_at) VALUES (gen_random_uuid(), $1, 'accepted', 1, now() - interval '31 days'), (gen_random_uuid(), $1, 'accepted', 1, now() - interval '1 day')")
        .bind(h1).execute(h.pool()).await.unwrap();
    let over = aurora_core::limits::WEBHOOK_DELIVERY_MAX_PER_WEBHOOK + 5;
    sqlx::query("INSERT INTO webhook_deliveries (id, webhook_id, status, size_bytes, created_at) SELECT gen_random_uuid(), $1, 'rejected', 1, now() - make_interval(secs => g) FROM generate_series(1, $2) g")
        .bind(h2).bind(over).execute(h.pool()).await.unwrap();

    let report = jobs::run_retention_prune(h.pool()).await.unwrap();
    let left = |sql: &'static str, id: Uuid| {
        let pool = h.pool().clone();
        async move {
            sqlx::query_scalar::<_, i64>(sql)
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    let inbox = |i: usize| left("SELECT count(*) FROM channel_inbox WHERE id = $1", ids[i]);
    assert_eq!((inbox(0).await, inbox(1).await, inbox(2).await), (0, 1, 1));
    let ops = |i: usize| left("SELECT count(*) FROM channel_ops WHERE id = $1", ids[3 + i]);
    assert_eq!(
        (
            ops(0).await,
            ops(1).await,
            ops(2).await,
            ops(3).await,
            ops(4).await
        ),
        (0, 1, 1, 1, 0),
        "a pending deprovision op and one with a sealed copy survive"
    );
    let cards = |i: usize| {
        left(
            "SELECT count(*) FROM channel_cards WHERE id = $1",
            ids[8 + i],
        )
    };
    assert_eq!((cards(0).await, cards(1).await, cards(2).await), (0, 1, 1));
    let links = |i: usize| {
        left(
            "SELECT count(*) FROM link_requests WHERE id = $1",
            ids[11 + i],
        )
    };
    assert_eq!((links(0).await, links(1).await, links(2).await), (0, 1, 1));
    let sessions = |id: Uuid| left("SELECT count(*) FROM sessions WHERE id = $1", id);
    assert_eq!(
        (
            sessions(s_revoked_old).await,
            sessions(s_revoked_new).await,
            sessions(s_expired_old).await,
            sessions(s_live).await
        ),
        (0, 1, 0, 1),
        "a revoked session past the grace is gone"
    );
    let creds = |i: usize| {
        left(
            "SELECT count(*) FROM gateway_credentials WHERE id = $1",
            ids[14 + i],
        )
    };
    assert_eq!((creds(0).await, creds(1).await, creds(2).await), (0, 1, 1));
    let n_h1 = left(
        "SELECT count(*) FROM webhook_deliveries WHERE webhook_id = $1",
        h1,
    )
    .await;
    assert_eq!(n_h1, 1, "only the delivery inside 30 days");
    let n_h2 = left(
        "SELECT count(*) FROM webhook_deliveries WHERE webhook_id = $1",
        h2,
    )
    .await;
    assert_eq!(
        n_h2,
        aurora_core::limits::WEBHOOK_DELIVERY_MAX_PER_WEBHOOK,
        "trimmed to the cap"
    );
    assert_eq!(report.channel_inbox, 1);
    assert_eq!(report.sessions, 2);
    // A second pass has nothing to do.
    assert_eq!(
        jobs::run_retention_prune(h.pool()).await.unwrap(),
        jobs::PruneReport::default()
    );
}
