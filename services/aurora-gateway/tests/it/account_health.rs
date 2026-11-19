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

/// What the gateway recorded on an account row (C-1033): the last failure and the last success apart.
#[derive(Debug, Clone, PartialEq)]
struct Rec {
    last_error: Option<String>,
    last_error_at: Option<chrono::DateTime<chrono::Utc>>,
    last_ok_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The Test pair and the retired `last_check_*` pair: the gateway never writes either.
    untouched: bool,
    status: String,
    used: bool,
}

impl Fx {
    async fn row(&self) -> Rec {
        type Cols = (
            Option<String>,
            Option<chrono::DateTime<chrono::Utc>>,
            Option<chrono::DateTime<chrono::Utc>>,
            bool,
            String,
            bool,
        );
        let (last_error, last_error_at, last_ok_at, untouched, status, used): Cols =
            sqlx::query_as(
                "SELECT last_error, last_error_at, last_ok_at, \
             (last_test_at IS NULL AND last_test_result IS NULL AND last_check_at IS NULL \
              AND last_check_result IS NULL), status, last_used_at IS NOT NULL \
             FROM app_connections WHERE id = $1",
            )
            .bind(self.conn)
            .fetch_one(self.db.pool())
            .await
            .unwrap();
        Rec {
            last_error,
            last_error_at,
            last_ok_at,
            untouched,
            status,
            used,
        }
    }

    /// Polls until `done` holds for the row (a bounded wait on the background write, no sleep to synchronise).
    async fn wait_for(&self, what: &str, done: impl Fn(&Rec) -> bool) -> Rec {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let r = self.row().await;
            if done(&r) {
                return r;
            }
            assert!(Instant::now() < deadline, "{what}: never happened: {r:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Polls until the failure is recorded as `want`.
    async fn wait_error(&self, want: &str) -> Rec {
        self.wait_for(want, |r| r.last_error.as_deref() == Some(want))
            .await
    }

    /// Polls until a success is recorded.
    async fn wait_ok(&self) -> Rec {
        self.wait_for("last_ok_at", |r| r.last_ok_at.is_some())
            .await
    }

    /// Polls until the use of the account is recorded (the churn write that precedes the upstream call).
    async fn wait_used(&self) {
        self.wait_for("last_used_at", |r| r.used).await;
    }
}

#[tokio::test]
async fn a_401_records_a_failure_a_later_success_records_use_and_the_status_never_changes() {
    let fx = start_health(Duration::ZERO).await;
    // Nothing recorded yet, never used.
    let r = fx.row().await;
    assert_eq!(
        (
            r.last_error,
            r.last_error_at.is_some(),
            r.last_ok_at.is_some(),
            r.used
        ),
        (None, false, false, false)
    );
    assert_eq!(r.status, "active");

    // The app refuses the injected token: a failure. The answer is passed through untouched.
    let r = get(&fx.env, "/x/401", "").await;
    assert_eq!(r.status, 401, "{}", r.body);
    fx.wait_error("rejected").await;
    fx.wait_used().await; // a connection's use is recorded too (it read "Never used" before)
    let rec = fx.row().await;
    assert!(rec.last_error_at.is_some());
    assert!(
        rec.last_ok_at.is_none(),
        "a failure never records a success"
    );
    assert_eq!(
        rec.status, "active",
        "recording never changes the account's status"
    );

    // It works again: a success is recorded, next to the failure (state is derived from which is newer).
    let r = get(&fx.env, "/x/fine", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let rec = fx.wait_ok().await;
    assert!(rec.last_ok_at > rec.last_error_at);
    assert_eq!(rec.status, "active");
    assert!(
        rec.untouched,
        "the gateway never writes last_test_* or last_check_*"
    );
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
    type Row = (
        Option<String>,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
        String,
    );
    let row = || async {
        sqlx::query_as::<_, Row>(
            "SELECT last_error, last_error_at, last_ok_at, status FROM secrets WHERE id = $1",
        )
        .bind(id)
        .fetch_one(db.pool())
        .await
        .unwrap()
    };
    let wait = |ok: bool| {
        let row = &row;
        async move {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let (err, err_at, ok_at, _) = row().await;
                if (ok && ok_at.is_some())
                    || (!ok && err.as_deref() == Some("rejected") && err_at.is_some())
                {
                    return;
                }
                assert!(Instant::now() < deadline, "write never happened (ok: {ok})");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    };

    let r = get(&env, "/x/401", "").await;
    assert_eq!(r.status, 401, "{}", r.body);
    wait(false).await;
    let (_, _, ok_at, status) = row().await;
    assert!(ok_at.is_none());
    assert_eq!(status, "active", "recording never changes the status");

    let r = get(&env, "/x/fine", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    wait(true).await;
}

/// A 403 is an answer the app gave to the credential: a failure too (C-1033), with the reason `rejected`.
#[tokio::test]
async fn a_403_records_a_failure() {
    let fx = start_health(Duration::ZERO).await;
    let r = get(&fx.env, "/x/403", "").await;
    assert_eq!(r.status, 403, "{}", r.body);
    let rec = fx.wait_error("rejected").await;
    assert!(rec.last_ok_at.is_none());
}

/// A status that says nothing about the credential (a 404) records nothing.
#[tokio::test]
async fn a_404_records_nothing() {
    let fx = start_health(Duration::ZERO).await;
    let r = get(&fx.env, "/x/404", "").await;
    assert_eq!(r.status, 404, "{}", r.body);
    // The use is written before the upstream call, the health (if any) after the answer: once the use is
    // visible and the reply is in, a recording would already be on its way.
    fx.wait_used().await;
    let rec = fx.row().await;
    assert_eq!(
        (rec.last_error, rec.last_error_at, rec.last_ok_at),
        (None, None, None)
    );
}

/// At most one write a minute per outcome, and a failure is never hidden: it is written inside the
/// minute after a success.
#[tokio::test]
async fn a_failure_is_written_inside_the_minute_after_a_success() {
    let fx = start_health(Duration::from_secs(60)).await;
    let r = get(&fx.env, "/x/fine", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let first = fx.wait_ok().await;

    // The failure comes at once, though the minute for successes has not passed.
    let r = get(&fx.env, "/x/401", "").await;
    assert_eq!(r.status, 401);
    let rec = fx.wait_error("rejected").await;
    assert!(rec.last_error_at >= rec.last_ok_at);

    // A second success inside the minute is not written: the stamp stays as it was for the whole bounded wait.
    let r = get(&fx.env, "/x/fine", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        let rec = fx.row().await;
        assert_eq!(
            rec.last_ok_at, first.last_ok_at,
            "a second write inside the minute"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
