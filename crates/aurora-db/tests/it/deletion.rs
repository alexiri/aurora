#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! (FIX-097) Deletion completeness. `docs/03`: restrict "so a forgotten table fails loudly" —
//! which only helps when a test notices the table that was forgotten. This one reads
//! `pg_constraint` for every `RESTRICT`/`NO ACTION` foreign key into `organizations` and
//! `workspaces` and demands that each referencing table is accounted for by the cascade that
//! deletes the parent (`aurora_db::repo::orgs::{ORG_CASCADE_TABLES, ORG_VIA_WORKSPACES}`).

use std::collections::BTreeSet;

use aurora_db::PgPool;
use aurora_db::repo::{orgs, workspaces};
use uuid::Uuid;

/// Tables with a non-cascading foreign key into `parent`.
async fn restrict_children(pool: &PgPool, parent: &str) -> BTreeSet<String> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT c.conrelid::regclass::text FROM pg_constraint c
         WHERE c.contype = 'f' AND c.confdeltype IN ('r', 'a') AND c.confrelid = $1::regclass",
    )
    .bind(parent)
    .fetch_all(pool)
    .await
    .unwrap();
    rows.into_iter().collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| (*s).to_owned()).collect()
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn every_restrict_child_of_organizations_is_handled_by_the_cascade(pool: PgPool) {
    let found = restrict_children(&pool, "organizations").await;
    let handled: BTreeSet<String> = orgs::ORG_CASCADE_TABLES
        .iter()
        .chain(orgs::ORG_VIA_WORKSPACES.iter())
        .map(|s| (*s).to_owned())
        .collect();
    let missing: Vec<_> = found.difference(&handled).collect();
    assert!(
        missing.is_empty(),
        "tables referencing organizations with RESTRICT that `orgs::delete_cascade` does not \
         handle (add them to ORG_CASCADE_TABLES, or to ORG_VIA_WORKSPACES when the workspace \
         cascade removes their rows): {missing:?}"
    );
    // The via-workspaces list names only real restricting tables (the explicit list may also
    // name tables whose FK is not RESTRICT, e.g. `app_instances`, deleted early on purpose).
    let stale: Vec<_> = orgs::ORG_VIA_WORKSPACES
        .iter()
        .filter(|t| !found.contains(**t))
        .collect();
    assert!(
        stale.is_empty(),
        "ORG_VIA_WORKSPACES lists tables that no longer restrict organizations: {stale:?}"
    );
    // `organizations` itself is deleted last and is never in the lists.
    assert!(!handled.contains("organizations"));
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn every_restrict_child_of_workspaces_is_handled_by_workspace_deletion(pool: PgPool) {
    let found = restrict_children(&pool, "workspaces").await;
    let handled = set(workspaces::WORKSPACE_RESTRICT_CHILDREN);
    assert_eq!(
        found, handled,
        "tables referencing workspaces with RESTRICT must be listed (and removed by the \
         workspace delete path) — update `workspaces::WORKSPACE_RESTRICT_CHILDREN`"
    );
}

/// A manual group (bound to a workspace), a SCIM token and a SCIM link used to make the
/// final `DELETE FROM organizations` fail (RESTRICT) and roll the whole deletion back.
#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn org_with_groups_and_scim_rows_is_deleted(pool: PgPool) {
    let org = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, slug, name, external_id) VALUES ($1, $2, 'o', $2)")
        .bind(org)
        .bind(org.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let ws = Uuid::now_v7();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1, $2, $3, 'w')")
        .bind(ws)
        .bind(org)
        .bind(ws.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let user = Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2, 'U')")
        .bind(user)
        .bind(format!("{user}@x.io"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO org_members (org_id, user_id, role, status) VALUES ($1, $2, 'member', 'active')",
    )
    .bind(org)
    .bind(user)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO scim_tokens (id, org_id, name, token_hash, display_prefix, expires_at)
         VALUES ($1, $2, 't', $3, 'aud_x', now() + interval '1 day')",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(vec![9u8; 32])
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO scim_users (id, org_id, user_id, user_name) VALUES ($1, $2, $3, 'u@x.io')",
    )
    .bind(Uuid::now_v7())
    .bind(org)
    .bind(user)
    .execute(&pool)
    .await
    .unwrap();
    let group = Uuid::now_v7();
    sqlx::query("INSERT INTO groups (id, org_id, name, source) VALUES ($1, $2, 'g', 'manual')")
        .bind(group)
        .bind(org)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO group_workspace_access (group_id, workspace_id, org_id, role)
         VALUES ($1, $2, $3, 'member')",
    )
    .bind(group)
    .bind(ws)
    .bind(org)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO group_members (group_id, org_id, user_id) VALUES ($1, $2, $3)")
        .bind(group)
        .bind(org)
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    orgs::delete_cascade(&mut tx, org).await.unwrap();
    tx.commit().await.unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM organizations WHERE id = $1")
        .bind(org)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}
