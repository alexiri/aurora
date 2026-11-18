//! (phase 5, P5-UX-14, D-350, C-985, SEC-129) Account health from real use: when an upstream answers 401 to a
//! request in which an account's credential was injected the gateway records `rejected`; a later 2xx or 3xx
//! records `ok`; a 403 records nothing. Writes happen only on a change of state and at most once a minute, and
//! never change the account's status. The writes go through the gateway's own least-privilege database role,
//! so the column grants are proven too.
//!
//! A child module of `apps` (it reuses that file's pipeline harness).
use super::*;
use aurora_gateway::account_health::AccountHealth;
use std::time::Instant;

struct Fx {
    env: Env,
    db: aurora_testkit::TestDb,
    conn: Uuid,
}

/// A real organization, workspace and connection, the gateway writing through its own role.
async fn start_health(min_interval: Duration) -> Fx {
    let mut db = aurora_testkit::TestDb::new().await;
    let role_url = db.gateway_role_url().await.unwrap();
    let pool = aurora_db::PgPool::connect(&role_url).await.unwrap();
    let w = World {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent: Uuid::now_v7(),
    };
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,'o','O','ext')",
    )
    .bind(w.org)
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1,$2,'w','W')")
        .bind(w.ws)
        .bind(w.org)
        .execute(db.pool())
        .await
        .unwrap();
    let c = conn(&w, "demo", "c1", None);
    sqlx::query(
        "INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) \
         VALUES ($1,$2,$3,'demo','pkg','1','L','active',$4)",
    )
    .bind(c.id)
    .bind(w.org)
    .bind(w.ws)
    .bind(&c.credentials_ref)
    .execute(db.pool())
    .await
    .unwrap();
    let conn_id = c.id;
    let (env, ..) = start_in(
        w,
        one_app(json!([{"bearer": "{credentials.tok}"}])),
        &[("c1", r#"{"tok":"T"}"#)],
        granted(c),
        None,
        Some(pool),
        Some(Arc::new(AccountHealth::with_min_interval(min_interval))),
        false,
    )
    .await;
    Fx {
        env,
        db,
        conn: conn_id,
    }
}

impl Fx {
    async fn row(
        &self,
    ) -> (
        Option<String>,
        Option<chrono::DateTime<chrono::Utc>>,
        String,
        bool,
    ) {
        sqlx::query_as(
            "SELECT last_check_result, last_check_at, status, last_used_at IS NOT NULL \
             FROM app_connections WHERE id = $1",
        )
        .bind(self.conn)
        .fetch_one(self.db.pool())
        .await
        .unwrap()
    }

    /// Polls until the stored result is `want` (a bounded wait on the background write, no sleep to synchronise).
    async fn wait_result(&self, want: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (r, ..) = self.row().await;
            if r.as_deref() == Some(want) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "result never became {want}: {r:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Polls until the use of the account is recorded (the churn write that precedes the upstream call).
    async fn wait_used(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.row().await.3 {
            assert!(Instant::now() < deadline, "last_used_at never written");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

#[tokio::test]
async fn a_401_records_rejected_a_later_success_records_ok_and_the_status_never_changes() {
    let fx = start_health(Duration::ZERO).await;
    // Nothing recorded yet, never used.
    let (r, at, status, used) = fx.row().await;
    assert_eq!((r, at.is_some(), used), (None, false, false));
    assert_eq!(status, "active");

    // The app refuses the injected token: rejected. The answer is passed through untouched.
    let r = get(&fx.env, "/x/401", "").await;
    assert_eq!(r.status, 401, "{}", r.body);
    fx.wait_result("rejected").await;
    fx.wait_used().await; // a connection's use is recorded too (it read "Never used" before)
    let (_, at, status, _) = fx.row().await;
    assert!(at.is_some());
    assert_eq!(
        status, "active",
        "recording never changes the account's status"
    );

    // It works again: ok.
    let r = get(&fx.env, "/x/fine", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    fx.wait_result("ok").await;
    assert_eq!(fx.row().await.2, "active");
}

/// A generic secret (a website's token) injected by host: its own row records the health, through the same role.
#[tokio::test]
async fn a_secret_account_records_its_health_in_its_own_row() {
    let mut db = aurora_testkit::TestDb::new().await;
    let role_url = db.gateway_role_url().await.unwrap();
    let pool = aurora_db::PgPool::connect(&role_url).await.unwrap();
    let w = World {
        org: Uuid::now_v7(),
        ws: Uuid::now_v7(),
        agent: Uuid::now_v7(),
    };
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,'o','O','ext')",
    )
    .bind(w.org)
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1,$2,'w','W')")
        .bind(w.ws)
        .bind(w.org)
        .execute(db.pool())
        .await
        .unwrap();
    let s = SecretMetaRow {
        id: Uuid::now_v7(),
        workspace_id: Some(w.ws),
        name: "weather".into(),
        kind: "generic".into(),
        llm_provider: None,
        llm_auth_mode: None,
        llm_endpoint_id: None,
        host_pattern: "127.0.0.1".into(),
        path_pattern: None,
        injection: json!({"kind": "bearer"}),
        value_ref: SecretRef::ext("mem", "s1", None, "1").unwrap().to_column(),
        value_version: 1,
        origin: None,
    };
    sqlx::query(
        "INSERT INTO secrets (id, org_id, workspace_id, name, kind, host_pattern, injection, value_source, value_ref) \
         VALUES ($1,$2,$3,'weather','generic','127.0.0.1','{\"kind\":\"bearer\"}','managed','x:')",
    )
    .bind(s.id)
    .bind(w.org)
    .bind(w.ws)
    .execute(db.pool())
    .await
    .unwrap();
    let id = s.id;
    let (env, ..) = start_in(
        w,
        one_app(json!([])),
        &[("s1", "S")],
        move |w| {
            let mut r = Rows::default();
            r.ws.rules.push(rule(
                "g",
                "allow",
                json!([{"type":"secret","id":id}]),
                false,
            ));
            r.secrets.push(s);
            let _ = w;
            r
        },
        None,
        Some(pool),
        Some(Arc::new(AccountHealth::with_min_interval(Duration::ZERO))),
        false,
    )
    .await;
    let row = || async {
        sqlx::query_as::<
            _,
            (
                Option<String>,
                Option<chrono::DateTime<chrono::Utc>>,
                String,
            ),
        >("SELECT last_check_result, last_check_at, status FROM secrets WHERE id = $1")
        .bind(id)
        .fetch_one(db.pool())
        .await
        .unwrap()
    };
    let wait = |want: &'static str| {
        let row = &row;
        async move {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let (r, ..) = row().await;
                if r.as_deref() == Some(want) {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "result never became {want}: {r:?}"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    };

    let r = get(&env, "/x/401", "").await;
    assert_eq!(r.status, 401, "{}", r.body);
    wait("rejected").await;
    let (_, at, status) = row().await;
    assert!(at.is_some());
    assert_eq!(status, "active", "recording never changes the status");

    let r = get(&env, "/x/fine", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    wait("ok").await;
}

#[tokio::test]
async fn a_403_records_nothing() {
    let fx = start_health(Duration::ZERO).await;
    let r = get(&fx.env, "/x/403", "").await;
    assert_eq!(r.status, 403, "{}", r.body);
    // The use is written before the upstream call, the health (if any) after the answer: once the use is
    // visible and the reply is in, a recording would already be on its way.
    fx.wait_used().await;
    let (result, at, ..) = fx.row().await;
    assert_eq!((result, at.is_some()), (None, false));
}

#[tokio::test]
async fn at_most_one_write_a_minute_and_only_on_a_change() {
    let fx = start_health(Duration::from_secs(60)).await;
    let r = get(&fx.env, "/x/401", "").await;
    assert_eq!(r.status, 401);
    fx.wait_result("rejected").await;
    let (_, first, ..) = fx.row().await;

    // A success inside the minute is not written: the result stays as it was for the whole bounded wait.
    let r = get(&fx.env, "/x/fine", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        let (r, at, ..) = fx.row().await;
        assert_eq!(
            r.as_deref(),
            Some("rejected"),
            "a second write inside the minute"
        );
        assert_eq!(at, first);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
