#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P2-DBL-1 (docs/21 §Phase 2 — Database lane), black box: a real `aurora-api`, two gateways and
//! PostgreSQL; the agent connects to the gateway's proxy port with an ordinary PostgreSQL driver
//! (sqlx) as user = connection id, password = its gateway credential. Run with
//! `--test-threads=1`.
mod common;

use aurora_secrets::{Purpose, SealContext, SecretVault, StoreRegistry};
use common::*;
use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgSslMode};
use sqlx::{Connection, PgConnection, Row};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

fn snapshot() -> Value {
    let tool = |id: &str, path: &str| json!({"id": id, "name": id, "host": "*.db.aurora.internal", "path": path, "methods": ["POST"]});
    json!({
        "format": 1, "contract": 1,
        "packages": [{"package_id": "e2e-pg", "version": "1.0.0", "digest": "d", "publisher": "p",
                      "hooks_digest": null, "egress": {}, "context": []}],
        "oauth_clients": [],
        "apps": [{
            "app_id": "e2e-pg", "package_id": "e2e-pg", "about": {"id": "e2e-pg", "name": "PostgreSQL"},
            "credentials": [], "metadata": [], "connect": [], "token": null,
            "tools": {"read": {"items": [tool("pg_read", "/read")]},
                      "write": {"items": [tool("pg_write", "/write"), tool("pg_ddl", "/ddl"),
                                          tool("pg_admin", "/admin")]}},
            "approvals": {}, "agent": null, "database": {"engine": "postgres"}
        }],
        "hosts": [], "reserved_hosts": []
    })
}

async fn client(st: &Stack, gw: &str, conn: Uuid, token: &str) -> sqlx::Result<PgConnection> {
    let (ip, port) = gw.rsplit_once(':').unwrap();
    let opts = PgConnectOptions::new()
        .host(ip)
        .port(port.parse().unwrap())
        .username(&conn.to_string())
        .password(token)
        .database("ignored")
        .ssl_mode(PgSslMode::VerifyFull)
        .ssl_root_cert_from_pem(st.ca_pem.clone().into_bytes());
    PgConnection::connect_with(&opts).await
}

#[tokio::test]
async fn d1_database_lane_login_read_write_approval_ddl_refused() {
    // The PostgreSQL the stored connection points at: not the platform database (the dial guard
    // refuses that by address and port). `scripts/e2e/gateway.sh` starts one.
    let Ok(target) = std::env::var("AURORA_E2E_DB_TARGET") else {
        eprintln!("SKIPPED: AURORA_E2E_DB_TARGET is not set; run scripts/e2e/gateway.sh");
        return;
    };
    let (dbhost, dbport) = target.rsplit_once(':').unwrap();
    let (dbhost, dbport) = (dbhost.to_owned(), dbport.parse::<u16>().unwrap());
    let (dbuser, dbpw, dbname) = (
        "aurora".to_owned(),
        "aurora".to_owned(),
        "postgres".to_owned(),
    );
    let tdb_url = format!("postgres://{dbuser}:{dbpw}@{dbhost}:{dbport}/{dbname}");
    let tdb = sqlx::PgPool::connect(&tdb_url).await.unwrap();
    sqlx::query("DROP TABLE IF EXISTS e2e_t")
        .execute(&tdb)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE e2e_t (id int PRIMARY KEY, v text)")
        .execute(&tdb)
        .await
        .unwrap();

    let st = Stack::start(Opts::default()).await;
    let api = st.signup("owner@e2e.test").await;
    let (agent, token) = api.byo_agent("e2e-db-agent").await;
    let (org, ws): (Uuid, Uuid) = (api.org.parse().unwrap(), api.ws.parse().unwrap());

    // The registry snapshot with a PostgreSQL app (newer than the empty one the stack seeds).
    sqlx::query(
        "INSERT INTO registry_snapshots (id, contract, content_hash, snapshot, compiled_by, activated_at) \
         VALUES ($1, 1, 'e2e-db', $2, 'e2e', now() + interval '1 second')",
    )
    .bind(Uuid::now_v7())
    .bind(snapshot())
    .execute(st.pool())
    .await
    .unwrap();
    sqlx::query("SELECT pg_notify('aurora_registry', '{}')")
        .execute(st.pool())
        .await
        .unwrap();

    // The database connection: credentials sealed like the API does for a connection.
    let conn = Uuid::now_v7();
    let keys = {
        use base64::Engine;
        format!(
            "k1:{}",
            base64::engine::general_purpose::STANDARD.encode([b'a'; 32])
        )
    };
    let keyring = Arc::new(aurora_crypto::Keyring::parse(&keys, "k1").unwrap());
    let vault = SecretVault::new(
        StoreRegistry::from_config(&aurora_config::SecretsSection::default(), keyring).unwrap(),
    );
    let sealed = vault
        .seal(
            &SealContext {
                purpose: Purpose::ConnectionCredentials,
                owner_id: conn,
                org_id: org,
                workspace_id: Some(ws),
            },
            aurora_core::Secret::from(
                serde_json::to_vec(&json!({
                    "host": dbhost, "port": dbport, "dbname": dbname, "user": dbuser,
                    "password": dbpw, "sslmode": "disable"
                }))
                .unwrap(),
            ),
        )
        .await
        .unwrap();
    sqlx::query("INSERT INTO app_connections (id, org_id, workspace_id, app_id, package_id, app_version, label, status, credentials_ref) VALUES ($1,$2,$3,'e2e-pg','e2e-pg','1.0.0','warehouse','active',$4)")
        .bind(conn).bind(org).bind(ws).bind(sealed.to_column())
        .execute(st.pool()).await.unwrap();

    // Grant: read allowed, write held for approval, nothing else (default block).
    api.set_policy("block", json!([])).await;
    for (i, (tool, approval)) in [("pg_read", false), ("pg_write", true)].iter().enumerate() {
        let rule = json!({"name": format!("Grant: {tool}"), "enabled": true,
            "identities": [{"kind": "agent", "id": agent}],
            "targets": [{"type": "connection", "id": conn, "tools": [tool]}],
            "action": "allow", "require_approval": approval, "rate_limit": null, "conditions": []});
        sqlx::query("INSERT INTO derived_rules (id, org_id, workspace_id, source_kind, source_id, ordinal, logical_id, rule) VALUES ($1,$2,$3,'grant',$4,$5,$6,$7)")
            .bind(Uuid::now_v7()).bind(org).bind(ws).bind(conn).bind(i as i32).bind(Uuid::now_v7()).bind(rule)
            .execute(st.pool()).await.unwrap();
    }

    // Wait for the gateways to load the snapshot, then log in with a real driver.
    let mut c = None;
    for _ in 0..80 {
        match client(&st, &st.gw_a, conn, &token).await {
            Ok(x) => {
                c = Some(x);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
    let mut c = c.unwrap_or_else(|| panic!("no login\n{}", st.log_tails()));

    // read: allowed
    let v: i32 = sqlx::query_scalar("select 40 + 2")
        .fetch_one(&mut c)
        .await
        .unwrap();
    assert_eq!(v, 42);

    // write: held for approval, the summary shows the statement with literals redacted
    let insert = tokio::spawn(async move {
        let r = sqlx::query("insert into e2e_t values (1, 'top-secret-literal')")
            .execute(&mut c)
            .await;
        (r.map(|x| x.rows_affected()), c)
    });
    let a = api.wait_pending().await;
    assert_eq!(a["path"], "/write", "{a}");
    let summary = a["summary"].to_string();
    assert!(
        summary.contains("insert into e2e_t values (?, ?)"),
        "{summary}"
    );
    assert!(!summary.contains("top-secret-literal"), "{summary}");
    let (s, v) = api
        .post(
            &format!("/v1/approvals/{}/decision", a["id"].as_str().unwrap()),
            json!({"decision": "approve"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (r, mut c) = insert.await.unwrap();
    assert_eq!(r.unwrap(), 1);
    let n: i64 = sqlx::query("select count(*) from e2e_t")
        .fetch_one(&mut c)
        .await
        .unwrap()
        .get(0);
    assert_eq!(n, 1);

    // ddl and always-refused statements never reach the database
    for sql in ["drop table e2e_t", "alter system set work_mem = '1GB'"] {
        let e = sqlx::query(sql).execute(&mut c).await.unwrap_err();
        let msg = e.to_string();
        assert!(
            e.as_database_error()
                .is_some_and(|d| d.code().as_deref() == Some("42501")),
            "{sql}: {msg}"
        );
    }
    let still: i64 = sqlx::query_scalar("select count(*) from e2e_t")
        .fetch_one(&tdb)
        .await
        .unwrap();
    assert_eq!(still, 1);

    // the other replica serves the lane too; a wrong credential is refused
    let c2 = client(&st, &st.gw_b, conn, &token).await;
    assert!(c2.is_ok(), "{:?}", c2.err());
    assert!(client(&st, &st.gw_a, conn, "aug_wrong").await.is_err());
    st.finish().await;
}
