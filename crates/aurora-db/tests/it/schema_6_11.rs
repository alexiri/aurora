#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use aurora_db::grants::{GrantsError, apply_gateway_grants};
use aurora_db::{DbError, PgPool};
use uuid::Uuid;

async fn exec(pool: &PgPool, sql: &str, binds: &[Uuid]) -> Result<(), DbError> {
    let mut q = sqlx::query(sql);
    for b in binds {
        q = q.bind(*b);
    }
    q.execute(pool).await?;
    Ok(())
}

/// Returns (org, workspace, agent).
async fn fixture(pool: &PgPool) -> (Uuid, Uuid, Uuid) {
    let (o, w, a) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    exec(pool, "INSERT INTO organizations (id, slug, name, external_id) VALUES ($1, $1::text, 'o', $1::text)", &[o]).await.unwrap();
    exec(
        pool,
        "INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1, $2, $1::text, 'w')",
        &[w, o],
    )
    .await
    .unwrap();
    exec(pool, "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind) VALUES ($1,$2,$3,'a','a','hosted')", &[a, o, w]).await.unwrap();
    (o, w, a)
}

async fn conversation(pool: &PgPool, agent: Uuid, ws: Uuid) -> Uuid {
    let c = Uuid::now_v7();
    exec(pool, "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at) VALUES ($1,$2,$3,'api',$1::text, now())", &[c, agent, ws]).await.unwrap();
    c
}

async fn turn(
    pool: &PgPool,
    conv: Uuid,
    agent: Uuid,
    role: &str,
    status: &str,
) -> Result<(), DbError> {
    let sql = format!(
        "INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_label, text) \
         VALUES ($1,$2,$3,'{role}','{status}','{{\"kind\":\"api\"}}','system','s','t')"
    );
    exec(pool, &sql, &[Uuid::now_v7(), conv, agent]).await
}

fn unique(r: Result<(), DbError>, name: &str) {
    let e = r.unwrap_err();
    assert!(matches!(e, DbError::UniqueViolation { .. }), "{e:?}");
    assert_eq!(e.constraint(), Some(name));
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn one_active_turn_per_conversation(pool: PgPool) {
    let (_, w, a) = fixture(&pool).await;
    let c = conversation(&pool, a, w).await;
    turn(&pool, c, a, "turn", "done").await.unwrap();
    turn(&pool, c, a, "turn", "queued").await.unwrap();
    unique(
        turn(&pool, c, a, "turn", "running").await,
        "turns_one_active_per_conversation_uq",
    );
    // Follow-ups are not active turns; role/status mismatch is rejected.
    turn(&pool, c, a, "followup", "joining").await.unwrap();
    let e = turn(&pool, c, a, "followup", "queued").await.unwrap_err();
    assert!(matches!(e, DbError::CheckViolation { .. }), "{e:?}");
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn one_open_runtime_interval_per_sandbox_and_no_fks(pool: PgPool) {
    let (o, w, a) = fixture(&pool).await;
    let sb = Uuid::now_v7();
    let ins = |boot: Uuid, ended: bool| {
        let pool = pool.clone();
        async move {
            let sql = format!(
                "INSERT INTO runtime_intervals (id, org_id, workspace_id, agent_id, sandbox_id, boot_id, started_at, ended_at) \
                 VALUES ($1,$2,$3,$4,$5,$6, now(), {})",
                if ended { "now()" } else { "NULL" }
            );
            exec(&pool, &sql, &[Uuid::now_v7(), o, w, a, sb, boot]).await
        }
    };
    // Dangling ids are fine (ledger: no FKs).
    ins(Uuid::now_v7(), false).await.unwrap();
    unique(
        ins(Uuid::now_v7(), false).await,
        "runtime_intervals_one_open_uq",
    );
    ins(Uuid::now_v7(), true).await.unwrap();

    for t in [
        "runtime_intervals",
        "runtime_daily",
        "request_log",
        "request_log_default",
    ] {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_constraint WHERE contype='f' AND conrelid = to_regclass($1)",
        )
        .bind(t)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n, 0, "{t} must have no FKs");
    }
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn active_space_binding_unique(pool: PgPool) {
    let (o, _, a) = fixture(&pool).await;
    let (conn, integ) = (
        format!("c{}", &Uuid::now_v7().simple().to_string()[20..]),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO connectors (id, token_hash, display_name, status) VALUES ($1, decode(repeat('ab',32),'hex'), 'C', 'active')")
        .bind(&conn).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO channel_integrations (id, org_id, connector_id, tenant_ref, tenant_label, config, credentials_ref, status) VALUES ($1,$2,$3,'t','t','{}','ref','active')")
        .bind(integ).bind(o).bind(&conn).execute(&pool).await.unwrap();
    let bind = |active: bool| {
        let pool = pool.clone();
        async move {
            let sql = format!(
                "INSERT INTO space_bindings (id, integration_id, space_ref, space_label, agent_id, active) VALUES ($1,$2,'sp','sp',$3,{active})"
            );
            exec(&pool, &sql, &[Uuid::now_v7(), integ, a]).await
        }
    };
    bind(true).await.unwrap();
    unique(bind(true).await, "space_bindings_active_space_uq");
    bind(false).await.unwrap();
    bind(false).await.unwrap();
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn deferred_conversation_fks(pool: PgPool) {
    let (_, w, a) = fixture(&pool).await;
    let c = conversation(&pool, a, w).await;
    // gateway_credentials.conversation_id now enforces the FK and cascades.
    let ins = |conv: Uuid| {
        let pool = pool.clone();
        async move {
            let sql = "INSERT INTO gateway_credentials (id, kind, token_hash, display_prefix, agent_id, conversation_id) \
                       VALUES ($1,'context', decode(md5($1::text)||md5($1::text),'hex'), 'p', $2, $3)";
            exec(&pool, sql, &[Uuid::now_v7(), a, conv]).await
        }
    };
    let e = ins(Uuid::now_v7()).await.unwrap_err();
    assert!(matches!(e, DbError::ForeignKeyViolation { .. }), "{e:?}");
    ins(c).await.unwrap();
    exec(&pool, "DELETE FROM conversations WHERE id = $1", &[c])
        .await
        .unwrap();
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM gateway_credentials WHERE kind='context'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 0);
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn gateway_grants(pool: PgPool) {
    let role = format!(
        "aurora_gw_test_{}",
        &Uuid::now_v7().simple().to_string()[20..]
    );
    assert!(matches!(
        apply_gateway_grants(&pool, "Bad-Role").await,
        Err(GrantsError::InvalidRole(_))
    ));
    assert!(matches!(
        apply_gateway_grants(&pool, &role).await,
        Err(GrantsError::RoleMissing(_))
    ));
    sqlx::query(&format!("CREATE ROLE {role} NOLOGIN"))
        .execute(&pool)
        .await
        .unwrap();

    apply_gateway_grants(&pool, &role).await.unwrap();
    apply_gateway_grants(&pool, &role).await.unwrap();

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query(&format!("SET ROLE {role}"))
        .execute(&mut *conn)
        .await
        .unwrap();
    async {
        assert!(
            sqlx::query_scalar::<_, bool>("SELECT has_table_privilege('request_log', 'INSERT')")
                .fetch_one(&mut *conn)
                .await
                .unwrap()
        );
        sqlx::query("SELECT * FROM agents")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT id, email, disabled_at FROM gateway_users")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT status FROM gateway_plugin_packages")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT pg_advisory_lock(1)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT pg_advisory_unlock(1)")
            .execute(&mut *conn)
            .await
            .unwrap();
        // No direct access to users or write on agents.
        assert!(
            sqlx::query("SELECT * FROM users")
                .fetch_all(&mut *conn)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("UPDATE agents SET name = 'x'")
                .execute(&mut *conn)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("UPDATE app_connections SET label = 'x'")
                .execute(&mut *conn)
                .await
                .is_err()
        );
        sqlx::query("UPDATE app_connections SET credentials_ref = credentials_ref")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("UPDATE secrets SET last_used_at = now()")
            .execute(&mut *conn)
            .await
            .unwrap();
        assert!(
            sqlx::query("UPDATE secrets SET name = name")
                .execute(&mut *conn)
                .await
                .is_err()
        );
        // (RFE-013, C-316) The gateway writes `last_used_at` of credentials and nothing else:
        // not the expiry, not the revocation, not the token hash.
        sqlx::query("UPDATE gateway_credentials SET last_used_at = now()")
            .execute(&mut *conn)
            .await
            .unwrap();
        for col in ["expires_at = now()", "revoked_at = now()", "kind = kind"] {
            assert!(
                sqlx::query(&format!("UPDATE gateway_credentials SET {col}"))
                    .execute(&mut *conn)
                    .await
                    .is_err(),
                "{col}"
            );
        }
        assert!(
            sqlx::query("SELECT * FROM request_log")
                .fetch_all(&mut *conn)
                .await
                .is_err()
        );
    }
    .await;
    sqlx::query("RESET ROLE").execute(&mut *conn).await.unwrap();
    drop(conn);
    sqlx::query(&format!("DROP OWNED BY {role}"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(&format!("DROP ROLE {role}"))
        .execute(&pool)
        .await
        .unwrap();
}
