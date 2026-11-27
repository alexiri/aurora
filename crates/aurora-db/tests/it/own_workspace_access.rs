#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-234 (D-477, C-1160): `effective_workspace_access` is exactly `own_workspace_access` —
//! an organization role grants no workspace right, so an org owner/admin has no row without a
//! binding of their own.

use aurora_db::PgPool;
use aurora_db::authz;
use aurora_testkit::TestDb;
use uuid::Uuid;

async fn user(pool: &PgPool, org: Uuid, org_role: Option<(&str, &str)>) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2, 'u')")
        .bind(id)
        .bind(format!("{id}@example.test"))
        .execute(pool)
        .await
        .unwrap();
    if let Some((role, status)) = org_role {
        sqlx::query("INSERT INTO org_members (org_id, user_id, role, status) VALUES ($1,$2,$3,$4)")
            .bind(org)
            .bind(id)
            .bind(role)
            .bind(status)
            .execute(pool)
            .await
            .unwrap();
    }
    id
}

async fn workspace(pool: &PgPool, org: Uuid, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1,$2,$3,$4)")
        .bind(id)
        .bind(org)
        .bind(id.to_string())
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn bind(pool: &PgPool, ws: Uuid, u: Uuid, role: &str, status: &str) {
    sqlx::query(
        "INSERT INTO workspace_access (workspace_id, user_id, role, status) VALUES ($1,$2,$3,$4)",
    )
    .bind(ws)
    .bind(u)
    .bind(role)
    .bind(status)
    .execute(pool)
    .await
    .unwrap();
}

async fn group(pool: &PgPool, org: Uuid, org_role: Option<&str>, members: &[Uuid]) -> Uuid {
    let g = Uuid::now_v7();
    sqlx::query("INSERT INTO groups (id, org_id, name, org_role) VALUES ($1,$2,$3,$4)")
        .bind(g)
        .bind(org)
        .bind(g.to_string())
        .bind(org_role)
        .execute(pool)
        .await
        .unwrap();
    for m in members {
        sqlx::query("INSERT INTO group_members (group_id, org_id, user_id) VALUES ($1,$2,$3)")
            .bind(g)
            .bind(org)
            .bind(m)
            .execute(pool)
            .await
            .unwrap();
    }
    g
}

async fn gbind(pool: &PgPool, g: Uuid, ws: Uuid, org: Uuid, role: &str) {
    sqlx::query("INSERT INTO group_workspace_access (group_id, workspace_id, org_id, role) VALUES ($1,$2,$3,$4)")
        .bind(g)
        .bind(ws)
        .bind(org)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
}

async fn rows(pool: &PgPool, source: &str) -> Vec<(Uuid, Uuid, Uuid, String)> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT workspace_id, org_id, user_id, role FROM ({source}) v ORDER BY 1, 3, 4"
    )))
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn the_two_views_are_equal_and_an_admin_has_no_row_without_a_binding() {
    let (_db, pool) = TestDb::with_pool().await;
    let org = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, slug, name, external_id) VALUES ($1,$2,'o',$2)")
        .bind(org)
        .bind(org.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let (ws1, ws2) = (
        workspace(&pool, org, "One").await,
        workspace(&pool, org, "Two").await,
    );
    let owner = user(&pool, org, Some(("owner", "active"))).await;
    let admin = user(&pool, org, Some(("admin", "active"))).await;
    let member = user(&pool, org, Some(("member", "active"))).await;
    let group_admin = user(&pool, org, Some(("member", "active"))).await;
    let direct = user(&pool, org, Some(("member", "active"))).await;
    let group_member = user(&pool, org, Some(("member", "active"))).await;
    let both = user(&pool, org, Some(("member", "active"))).await;
    let suspended_binding = user(&pool, org, Some(("member", "active"))).await;
    let suspended_via_group = user(&pool, org, Some(("member", "active"))).await;
    let inactive_org = user(&pool, org, Some(("member", "suspended"))).await;
    let pending_org = user(&pool, org, Some(("member", "pending"))).await;
    let non_member = user(&pool, org, None).await;
    let _ = (member, non_member);

    // An org admin through a group: no workspace binding, the implicit owner role only.
    group(&pool, org, Some("admin"), &[group_admin]).await;
    // Direct: the manual member, an IdP-claimed binding, a suspended binding, a binding of a
    // person who is no longer an active org member.
    bind(&pool, ws1, direct, "approver", "active").await;
    sqlx::query("INSERT INTO workspace_access (workspace_id, user_id, role, idp_role, idp_source) VALUES ($1,$2,'member','member','idp:test:claim')")
        .bind(ws2)
        .bind(direct)
        .execute(&pool)
        .await
        .unwrap();
    bind(&pool, ws1, suspended_binding, "owner", "suspended").await;
    bind(&pool, ws1, inactive_org, "owner", "active").await;
    bind(&pool, ws1, pending_org, "member", "active").await;
    // Group bindings: the strongest of direct and group wins, an inactive org member derives
    // nothing, a person suspended in the workspace derives nothing from the group there.
    let g = group(
        &pool,
        org,
        None,
        &[
            group_member,
            both,
            suspended_via_group,
            suspended_binding,
            inactive_org,
        ],
    )
    .await;
    gbind(&pool, g, ws1, org, "member").await;
    gbind(&pool, g, ws2, org, "owner").await;
    bind(&pool, ws1, both, "approver", "active").await;
    bind(&pool, ws1, suspended_via_group, "member", "suspended").await;

    // The two views coincide: every workspace right comes from access in one's own right.
    let effective = rows(&pool, "SELECT * FROM effective_workspace_access").await;
    let own = rows(&pool, "SELECT * FROM own_workspace_access").await;
    assert!(own.len() > 5, "the fixture must be non-trivial: {own:?}");
    assert_eq!(effective, own, "the views diverged");

    // Explicit expectations on the interesting people. An org owner, admin or group-mapped
    // admin has no row without a binding of their own.
    let role = |u: Uuid, ws: Uuid| {
        effective
            .iter()
            .find(|r| r.2 == u && r.0 == ws)
            .map(|r| r.3.clone())
    };
    for (u, ws) in [
        (owner, ws1),
        (owner, ws2),
        (admin, ws1),
        (admin, ws2),
        (group_admin, ws1),
        (group_admin, ws2),
    ] {
        assert_eq!(role(u, ws), None, "an org role grants no workspace right");
    }
    assert_eq!(role(direct, ws1).as_deref(), Some("approver"));
    assert_eq!(
        role(direct, ws2).as_deref(),
        Some("member"),
        "the IdP-claimed binding"
    );
    assert_eq!(role(group_member, ws2).as_deref(), Some("owner"));
    assert_eq!(role(both, ws1).as_deref(), Some("approver"));
    assert_eq!(role(both, ws2).as_deref(), Some("owner"));
    assert_eq!(role(suspended_binding, ws1), None);
    assert_eq!(role(suspended_via_group, ws1), None);
    assert_eq!(role(suspended_via_group, ws2).as_deref(), Some("owner"));
    assert_eq!(role(inactive_org, ws1), None);
    assert_eq!(role(pending_org, ws1), None);
    assert_eq!(role(non_member, ws1), None);

    // `own_access` agrees with the view.
    let pool_ref = &pool;
    let own_of = |u: Uuid, ws: Uuid| async move {
        authz::own_access(pool_ref, u, ws)
            .await
            .unwrap()
            .map(|r| r.as_str())
    };
    assert_eq!(own_of(direct, ws1).await, Some("approver"));
    assert_eq!(own_of(direct, ws2).await, Some("member"));
    assert_eq!(own_of(group_member, ws1).await, Some("member"));
    assert_eq!(own_of(group_member, ws2).await, Some("owner"));
    assert_eq!(own_of(both, ws1).await, Some("approver"));
    assert_eq!(own_of(suspended_binding, ws1).await, None);
    assert_eq!(own_of(suspended_via_group, ws1).await, None);
    assert_eq!(own_of(inactive_org, ws1).await, None);
    assert_eq!(own_of(pending_org, ws1).await, None);
    assert_eq!(own_of(non_member, ws1).await, None);
    // An org admin who also holds a direct binding keeps it as own access (a creator's workspace).
    let creator = user(&pool, org, Some(("admin", "active"))).await;
    bind(&pool, ws1, creator, "owner", "active").await;
    assert_eq!(own_of(creator, ws1).await, Some("owner"));
    assert_eq!(own_of(creator, ws2).await, None);
    // No row of either view for the admins' implicit-only workspaces.
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM own_workspace_access WHERE user_id = ANY($1)")
            .bind(vec![owner, admin, group_admin])
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 0);

    // `can_administer` (C-1160): the org role keeps the narrow administer right even without a
    // binding; a bound non-owner and a plain member do not have it.
    assert!(authz::can_administer(&pool, owner, ws1).await.unwrap());
    assert!(authz::can_administer(&pool, admin, ws2).await.unwrap());
    assert!(
        authz::can_administer(&pool, group_admin, ws1)
            .await
            .unwrap()
    );
    assert!(authz::can_administer(&pool, creator, ws1).await.unwrap());
    assert!(!authz::can_administer(&pool, direct, ws1).await.unwrap());
    assert!(
        !authz::can_administer(&pool, group_member, ws1)
            .await
            .unwrap()
    );
}
