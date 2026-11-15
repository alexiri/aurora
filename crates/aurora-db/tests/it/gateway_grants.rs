#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-016: the gateway grants (03 §Gateway database role grants) applied to a fresh login role,
//! then the gateway's statements run as that role. The privilege check happens at plan time, so
//! `… WHERE false` statements prove a grant without needing rows.

use aurora_db::PgPool;
use aurora_db::grants::{GrantsError, apply_gateway_grants};
use sqlx::{Connection, PgConnection};

/// The SQLSTATE of a failed statement run as the role (`None` when it succeeded).
async fn code(conn: &mut PgConnection, sql: &str) -> Option<String> {
    match sqlx::query(sql).execute(conn).await {
        Ok(_) => None,
        Err(sqlx::Error::Database(e)) => e.code().map(|c| c.to_string()),
        Err(e) => panic!("{sql}: {e}"),
    }
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn missing_role_message_names_both_ways_out(pool: PgPool) {
    let err = apply_gateway_grants(&pool, "fix016_no_such_role")
        .await
        .unwrap_err();
    assert!(matches!(err, GrantsError::RoleMissing(_)));
    assert_eq!(
        err.to_string(),
        "gateway role \"fix016_no_such_role\" does not exist: create it (CREATE ROLE fix016_no_such_role LOGIN PASSWORD '…') or unset database.gateway_role"
    );
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn grants_are_idempotent_and_sufficient_for_the_gateway(pool: PgPool) {
    // The DBA's step: a login role with a password and nothing else (cluster-wide, so dropped below).
    let role = format!("aurora_test_gw_{}", uuid::Uuid::now_v7().simple());
    sqlx::query(&format!(
        "CREATE ROLE \"{role}\" LOGIN PASSWORD 'pw' NOSUPERUSER NOCREATEDB NOCREATEROLE"
    ))
    .execute(&pool)
    .await
    .unwrap();
    // (FIX-106) An earlier release granted `api_keys`: applying the grants takes it back.
    sqlx::query(&format!("GRANT SELECT ON api_keys TO \"{role}\""))
        .execute(&pool)
        .await
        .unwrap();
    // `migrate` applies the grants after every upgrade: twice must be fine.
    apply_gateway_grants(&pool, &role).await.unwrap();
    apply_gateway_grants(&pool, &role).await.unwrap();

    let opts = (*pool.connect_options())
        .clone()
        .username(&role)
        .password("pw");
    let mut c = PgConnection::connect_with(&opts).await.unwrap();

    // Reads the gateway performs (tables and the two column-restricted views).
    for t in [
        "organizations",
        "workspaces",
        "workspace_access",
        "org_members",
        "gateway_users",
        "agents",
        "gateway_credentials",
        "secrets",
        "app_connections",
        "app_oauth_clients",
        "app_instances",
        "grants",
        "derived_rules",
        "policy_generations",
        "policy_rules",
        "registry_snapshots",
        "gateway_plugin_packages",
        "host_claim_decisions",
        "org_app_availability",
        "groups",
        "group_members",
        "group_workspace_access",
    ] {
        assert_eq!(
            code(&mut c, &format!("SELECT * FROM {t} WHERE false")).await,
            None,
            "SELECT on {t}"
        );
    }
    // Writes.
    for sql in [
        "INSERT INTO approvals (id) SELECT gen_random_uuid() WHERE false",
        "UPDATE approvals SET id = id WHERE false",
        "INSERT INTO gateway_instances (id) SELECT gen_random_uuid() WHERE false",
        "UPDATE gateway_instances SET id = id WHERE false",
        "INSERT INTO request_log (id) SELECT gen_random_uuid() WHERE false",
        "UPDATE app_connections SET updated_at = now(), status = status, status_reason = status_reason, \
         credentials_ref = credentials_ref, credentials_version = credentials_version, \
         credentials_expires_at = credentials_expires_at, reauth_required_at = reauth_required_at, \
         last_used_at = last_used_at WHERE false",
        "UPDATE secrets SET last_used_at = last_used_at, origin = origin WHERE false",
        "UPDATE gateway_credentials SET last_used_at = last_used_at WHERE false",
        // The gateway's own statements on these two (repo/gateway.rs): WHERE needs SELECT on the columns read.
        "SELECT status FROM approvals WHERE id = gen_random_uuid()",
        "UPDATE approvals SET status = 'expired', decided_at = now(), resolved_at = now(), \
         updated_at = now() WHERE id = gen_random_uuid() AND status = 'pending'",
        "UPDATE approvals SET status = 'abandoned', resolved_at = now(), updated_at = now() \
         WHERE gateway_instance_id = gen_random_uuid() AND status = 'pending'",
        "UPDATE gateway_instances SET last_seen_at = now() WHERE id = gen_random_uuid()",
        "UPDATE gateway_instances SET stopped_at = now(), updated_at = now() WHERE id = gen_random_uuid()",
        "SELECT pg_try_advisory_lock(1), pg_advisory_unlock(1), pg_advisory_unlock_all()",
        "SELECT pg_try_advisory_xact_lock(1, 2)",
    ] {
        assert_eq!(code(&mut c, sql).await, None, "{sql}");
    }
    // Nothing beyond that: the full user/package tables, other writes, DDL (42501 insufficient_privilege).
    for sql in [
        "SELECT * FROM users WHERE false",
        "SELECT * FROM plugin_packages WHERE false",
        "SELECT * FROM audit_log WHERE false",
        // (FIX-106) No gateway query reads the hashed user API keys.
        "SELECT * FROM api_keys WHERE false",
        "SELECT summary FROM approvals WHERE false",
        "SELECT hostname FROM gateway_instances WHERE false",
        "UPDATE users SET id = id WHERE false",
        "DELETE FROM approvals WHERE false",
        "DELETE FROM request_log WHERE false",
        "UPDATE request_log SET id = id WHERE false",
        "UPDATE secrets SET id = id WHERE false",
        "UPDATE app_connections SET id = id WHERE false",
        "INSERT INTO agents (id) SELECT gen_random_uuid() WHERE false",
        // (P4-GRP-1) The gateway reads the group tables and never writes them.
        "INSERT INTO group_members (group_id) SELECT gen_random_uuid() WHERE false",
        "DELETE FROM groups WHERE false",
        "CREATE TABLE gw_scratch (a int)",
    ] {
        assert_eq!(code(&mut c, sql).await.as_deref(), Some("42501"), "{sql}");
    }
    c.close().await.ok();

    sqlx::query(&format!("DROP OWNED BY \"{role}\""))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(&format!("DROP ROLE \"{role}\""))
        .execute(&pool)
        .await
        .unwrap();
}
