#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P4-GRP-1 (D-130, D-156, SEC-71): the effective-access law with groups, on a real Postgres.

use aurora_db::authz::{
    self, AccessSourceKind, OrgRole, WorkspaceRole, automation_authorized, can_approve, can_own,
    can_use, combine_roles, effective_access, workspace_has_active_owner,
};
use aurora_db::repo::groups;
use aurora_db::{DbError, PgPool};
use uuid::Uuid;

struct Fx {
    org: Uuid,
    ws: Uuid,
}

async fn org(pool: &PgPool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, slug, name, external_id) VALUES ($1, $2, 'o', $2)")
        .bind(id)
        .bind(id.to_string())
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn workspace(pool: &PgPool, org: Uuid) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO workspaces (id, org_id, slug, name) VALUES ($1, $2, $3, 'w')")
        .bind(id)
        .bind(org)
        .bind(id.to_string())
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn user(pool: &PgPool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2, 'u')")
        .bind(id)
        .bind(format!("{id}@example.test"))
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn member(pool: &PgPool, org: Uuid, role: &str) -> Uuid {
    let u = user(pool).await;
    sqlx::query("INSERT INTO org_members (org_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(org)
        .bind(u)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
    u
}

async fn bind(pool: &PgPool, ws: Uuid, u: Uuid, role: &str) {
    sqlx::query("INSERT INTO workspace_access (workspace_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(ws)
        .bind(u)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
}

async fn group(pool: &PgPool, org: Uuid, name: &str, org_role: Option<&str>) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO groups (id, org_id, name, org_role) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(org)
        .bind(name)
        .bind(org_role)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn add(pool: &PgPool, g: Uuid, org: Uuid, u: Uuid) -> Result<(), DbError> {
    let mut c = pool.acquire().await.unwrap();
    groups::add_member(&mut c, g, org, u, "manual", None)
        .await
        .map(|_| ())
}

async fn gbind(pool: &PgPool, g: Uuid, ws: Uuid, org: Uuid, role: &str) -> Result<(), DbError> {
    let mut c = pool.acquire().await.unwrap();
    groups::bind(&mut c, g, ws, org, role, None).await
}

async fn fx(pool: &PgPool) -> Fx {
    let org = org(pool).await;
    let ws = workspace(pool, org).await;
    Fx { org, ws }
}

async fn role(pool: &PgPool, u: Uuid, ws: Uuid) -> Option<WorkspaceRole> {
    authz::workspace_access(pool, u, ws)
        .await
        .unwrap()
        .map(|a| a.effective_ws_role)
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn no_groups_gives_the_phase_0_to_3_answers(pool: PgPool) {
    let f = fx(&pool).await;
    let owner = member(&pool, f.org, "owner").await;
    let plain = member(&pool, f.org, "member").await;
    let bound = member(&pool, f.org, "member").await;
    bind(&pool, f.ws, bound, "approver").await;
    assert_eq!(role(&pool, owner, f.ws).await, Some(WorkspaceRole::Owner));
    assert_eq!(role(&pool, plain, f.ws).await, None);
    assert_eq!(
        role(&pool, bound, f.ws).await,
        Some(WorkspaceRole::Approver)
    );
    assert!(!can_use(&pool, plain, f.ws).await.unwrap());
    assert!(can_approve(&pool, bound, f.ws).await.unwrap());
    assert!(!can_own(&pool, bound, f.ws).await.unwrap());
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn effective_role_matrix_is_the_strongest_never_a_sum(pool: PgPool) {
    let f = fx(&pool).await;
    let u = member(&pool, f.org, "member").await;
    let g1 = group(&pool, f.org, "one", None).await;
    let g2 = group(&pool, f.org, "two", None).await;
    add(&pool, g1, f.org, u).await.unwrap();
    add(&pool, g2, f.org, u).await.unwrap();

    // group only
    gbind(&pool, g1, f.ws, f.org, "member").await.unwrap();
    assert_eq!(role(&pool, u, f.ws).await, Some(WorkspaceRole::Member));
    // several groups: the strongest
    gbind(&pool, g2, f.ws, f.org, "approver").await.unwrap();
    assert_eq!(role(&pool, u, f.ws).await, Some(WorkspaceRole::Approver));
    // both, direct member + group owner: the group raises, never lowers
    bind(&pool, f.ws, u, "member").await;
    gbind(&pool, g1, f.ws, f.org, "owner").await.unwrap();
    assert_eq!(role(&pool, u, f.ws).await, Some(WorkspaceRole::Owner));
    assert!(can_own(&pool, u, f.ws).await.unwrap());
    // removing the direct binding leaves what the groups give
    sqlx::query("DELETE FROM workspace_access WHERE user_id = $1")
        .bind(u)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(role(&pool, u, f.ws).await, Some(WorkspaceRole::Owner));
    // direct owner + group member: the direct role stays
    gbind(&pool, g1, f.ws, f.org, "member").await.unwrap();
    gbind(&pool, g2, f.ws, f.org, "member").await.unwrap();
    bind(&pool, f.ws, u, "owner").await;
    assert_eq!(role(&pool, u, f.ws).await, Some(WorkspaceRole::Owner));
    // removing the groups leaves the direct binding
    sqlx::query("DELETE FROM groups WHERE org_id = $1")
        .bind(f.org)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(role(&pool, u, f.ws).await, Some(WorkspaceRole::Owner));
    // the pure rule agrees
    assert_eq!(
        combine_roles(
            OrgRole::Member,
            &[],
            Some(WorkspaceRole::Member),
            &[WorkspaceRole::Approver, WorkspaceRole::Member]
        ),
        (OrgRole::Member, Some(WorkspaceRole::Approver))
    );
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn org_role_mapping_admin_owns_everywhere_and_survives_direct_demotion(pool: PgPool) {
    let f = fx(&pool).await;
    let ws2 = workspace(&pool, f.org).await;
    let u = member(&pool, f.org, "member").await;
    let g = group(&pool, f.org, "admins", Some("admin")).await;
    add(&pool, g, f.org, u).await.unwrap();
    for ws in [f.ws, ws2] {
        assert!(can_own(&pool, u, ws).await.unwrap());
    }
    assert_eq!(
        authz::effective_org_role(&pool, u, f.org).await.unwrap(),
        Some(OrgRole::Admin)
    );
    // A direct demotion (the row already says member) does not take the mapping away.
    sqlx::query("UPDATE org_members SET role = 'member' WHERE user_id = $1")
        .bind(u)
        .execute(&pool)
        .await
        .unwrap();
    assert!(can_own(&pool, u, f.ws).await.unwrap());
    // `owner` can never be mapped: the CHECK refuses it.
    let err = sqlx::query("UPDATE groups SET org_role = 'owner' WHERE id = $1")
        .bind(g)
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(matches!(DbError::from(err), DbError::CheckViolation { .. }));
    // member mapping gives nothing beyond the direct role
    sqlx::query("UPDATE groups SET org_role = 'member' WHERE id = $1")
        .bind(g)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!can_use(&pool, u, f.ws).await.unwrap());
    // and an effective admin is never an effective owner
    sqlx::query("UPDATE groups SET org_role = 'admin' WHERE id = $1")
        .bind(g)
        .execute(&pool)
        .await
        .unwrap();
    assert_ne!(
        authz::effective_org_role(&pool, u, f.org).await.unwrap(),
        Some(OrgRole::Owner)
    );
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn suspended_member_derives_nothing_from_any_group(pool: PgPool) {
    let f = fx(&pool).await;
    let u = member(&pool, f.org, "member").await;
    let g = group(&pool, f.org, "g", Some("admin")).await;
    gbind(&pool, g, f.ws, f.org, "owner").await.unwrap();
    add(&pool, g, f.org, u).await.unwrap();
    assert!(can_own(&pool, u, f.ws).await.unwrap());
    sqlx::query("UPDATE org_members SET status = 'suspended' WHERE user_id = $1")
        .bind(u)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(role(&pool, u, f.ws).await, None);
    assert_eq!(
        authz::effective_org_role(&pool, u, f.org).await.unwrap(),
        None
    );
    // still listed in the group
    assert!(groups::is_member(&pool, g, u).await.unwrap());
    assert!(!workspace_has_active_owner(&pool, f.ws).await.unwrap());
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn a_group_never_makes_a_non_member_a_member(pool: PgPool) {
    let f = fx(&pool).await;
    let other = org(&pool).await;
    let stranger = member(&pool, other, "member").await;
    let g = group(&pool, f.org, "g", None).await;
    // the composite foreign key to org_members refuses the insert
    let err = add(&pool, g, f.org, stranger).await.unwrap_err();
    assert!(
        matches!(err, DbError::ForeignKeyViolation { .. }),
        "{err:?}"
    );
    // and a user with no membership at all
    let nobody = user(&pool).await;
    assert!(matches!(
        add(&pool, g, f.org, nobody).await.unwrap_err(),
        DbError::ForeignKeyViolation { .. }
    ));
    assert_eq!(groups::member_count(&pool, g).await.unwrap(), 0);
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn leaving_the_org_empties_every_membership(pool: PgPool) {
    let f = fx(&pool).await;
    let u = member(&pool, f.org, "member").await;
    let g1 = group(&pool, f.org, "a", None).await;
    let g2 = group(&pool, f.org, "b", None).await;
    add(&pool, g1, f.org, u).await.unwrap();
    add(&pool, g2, f.org, u).await.unwrap();
    let mut c = pool.acquire().await.unwrap();
    aurora_db::repo::members::remove(&mut c, f.org, u)
        .await
        .unwrap();
    drop(c);
    assert_eq!(groups::member_count(&pool, g1).await.unwrap(), 0);
    assert_eq!(groups::member_count(&pool, g2).await.unwrap(), 0);
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn same_org_composite_foreign_keys(pool: PgPool) {
    let f = fx(&pool).await;
    let other = org(&pool).await;
    let other_ws = workspace(&pool, other).await;
    let g = group(&pool, f.org, "g", None).await;
    // a group cannot bind another org's workspace
    let err = gbind(&pool, g, other_ws, f.org, "member")
        .await
        .unwrap_err();
    assert!(
        matches!(err, DbError::ForeignKeyViolation { .. }),
        "{err:?}"
    );
    // nor claim another org as its own on a binding of its own workspace
    let err = gbind(&pool, g, f.ws, other, "member").await.unwrap_err();
    assert!(
        matches!(err, DbError::ForeignKeyViolation { .. }),
        "{err:?}"
    );
    // a manual group takes only manual members; an idp group only idp members
    let u = member(&pool, f.org, "member").await;
    let mut c = pool.acquire().await.unwrap();
    let err = groups::add_member(&mut c, g, f.org, u, "idp", None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, DbError::ForeignKeyViolation { .. }),
        "{err:?}"
    );
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn approve_see_conversation_and_fire_time_for_group_only_access(pool: PgPool) {
    let f = fx(&pool).await;
    let owner = member(&pool, f.org, "owner").await;
    let approver = member(&pool, f.org, "member").await;
    let g_ap = group(&pool, f.org, "approvers", None).await;
    gbind(&pool, g_ap, f.ws, f.org, "approver").await.unwrap();
    add(&pool, g_ap, f.org, approver).await.unwrap();
    assert!(can_approve(&pool, approver, f.ws).await.unwrap());
    assert!(!can_own(&pool, approver, f.ws).await.unwrap());

    let g_own = group(&pool, f.org, "owners", None).await;
    gbind(&pool, g_own, f.ws, f.org, "owner").await.unwrap();
    let ws_owner = member(&pool, f.org, "member").await;
    add(&pool, g_own, f.org, ws_owner).await.unwrap();
    assert!(workspace_has_active_owner(&pool, f.ws).await.unwrap());

    // a `direct` conversation of someone else: only a (group-derived) owner sees it
    let agent = Uuid::now_v7();
    sqlx::query("INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, is_default) VALUES ($1,$2,$3,'a','a','hosted',true)")
        .bind(agent).bind(f.org).bind(f.ws).execute(&pool).await.unwrap();
    let conv = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, workspace_id, agent_id, kind, external_ref, direct_user_id, last_activity_at) \
         VALUES ($1,$2,$3,'direct','x',$4, now())",
    )
    .bind(conv)
    .bind(f.ws)
    .bind(agent)
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        authz::can_see_conversation_id(&pool, ws_owner, conv)
            .await
            .unwrap()
    );
    assert!(
        !authz::can_see_conversation_id(&pool, approver, conv)
            .await
            .unwrap()
    );

    // fire-time authority of a creator whose only access came through a group
    let mut c = pool.acquire().await.unwrap();
    assert!(
        automation_authorized(&mut c, "creator", Some(approver), f.ws)
            .await
            .unwrap()
    );
    drop(c);
    groups::remove_member(&mut pool.acquire().await.unwrap(), g_ap, approver)
        .await
        .unwrap();
    let mut c = pool.acquire().await.unwrap();
    assert!(
        !automation_authorized(&mut c, "creator", Some(approver), f.ws)
            .await
            .unwrap()
    );
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn sources_explain_the_result(pool: PgPool) {
    let f = fx(&pool).await;
    let u = member(&pool, f.org, "member").await;
    bind(&pool, f.ws, u, "member").await;
    let g = group(&pool, f.org, "g", None).await;
    gbind(&pool, g, f.ws, f.org, "approver").await.unwrap();
    add(&pool, g, f.org, u).await.unwrap();
    let e = effective_access(&pool, u, f.ws).await.unwrap().unwrap();
    assert_eq!(e.auth.effective_ws_role, WorkspaceRole::Approver);
    let kinds: Vec<_> = e.sources.iter().map(|s| s.kind.clone()).collect();
    assert!(kinds.contains(&AccessSourceKind::Direct));
    assert!(kinds.contains(&AccessSourceKind::Group));
    // an IdP-claim row is reported as such
    sqlx::query(
        "UPDATE workspace_access SET idp_role = 'member', idp_source = 'idp:dev:x' WHERE user_id = $1",
    )
    .bind(u)
    .execute(&pool)
    .await
    .unwrap();
    let e = effective_access(&pool, u, f.ws).await.unwrap().unwrap();
    assert!(
        e.sources
            .iter()
            .any(|s| s.kind == AccessSourceKind::IdpClaim)
    );
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn idp_sync_sets_exactly_the_claim_and_never_creates_a_membership(pool: PgPool) {
    let f = fx(&pool).await;
    let other = org(&pool).await;
    let u = member(&pool, f.org, "member").await;
    let non_member_org_user = user(&pool).await; // member of nothing
    let gid = |name: &str, org: Uuid, prov: &str, val: &str| {
        let pool = pool.clone();
        let (name, prov, val) = (name.to_owned(), prov.to_owned(), val.to_owned());
        async move {
            let mut c = pool.acquire().await.unwrap();
            let id = Uuid::now_v7();
            groups::insert(
                &mut c,
                id,
                org,
                &name,
                None,
                "idp",
                Some((&prov, &val)),
                None,
                None,
                None,
            )
            .await
            .unwrap();
            id
        }
    };
    let a = gid("a", f.org, "cern", "alpha").await;
    let b = gid("b", f.org, "cern", "beta").await;
    let other_provider = gid("c", f.org, "other", "alpha").await;
    let other_org_group = gid("d", other, "cern", "alpha").await;
    let mut c = pool.acquire().await.unwrap();

    let ch = groups::sync_idp_memberships(&mut c, u, "cern", &["alpha".into(), "unlinked".into()])
        .await
        .unwrap();
    assert_eq!(ch.len(), 1);
    assert!(groups::is_member(&pool, a, u).await.unwrap());
    assert!(!groups::is_member(&pool, b, u).await.unwrap());
    // another provider's group and another org's group (u is no member there) grant nothing
    assert!(!groups::is_member(&pool, other_provider, u).await.unwrap());
    assert!(!groups::is_member(&pool, other_org_group, u).await.unwrap());
    // a non-member gains nothing, and no org_members row appears
    groups::sync_idp_memberships(&mut c, non_member_org_user, "cern", &["alpha".into()])
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM org_members WHERE user_id = $1")
        .bind(non_member_org_user)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    // switching the claim moves the membership; an absent claim removes every one
    groups::sync_idp_memberships(&mut c, u, "cern", &["beta".into()])
        .await
        .unwrap();
    assert!(!groups::is_member(&pool, a, u).await.unwrap());
    assert!(groups::is_member(&pool, b, u).await.unwrap());
    groups::sync_idp_memberships(&mut c, u, "cern", &[])
        .await
        .unwrap();
    assert_eq!(groups::member_ids(&pool, b).await.unwrap().len(), 0);
    // a suspended member is removed (derives nothing, is not synced in)
    sqlx::query("UPDATE org_members SET status = 'suspended' WHERE user_id = $1")
        .bind(u)
        .execute(&pool)
        .await
        .unwrap();
    groups::sync_idp_memberships(&mut c, u, "cern", &["alpha".into()])
        .await
        .unwrap();
    assert!(!groups::is_member(&pool, a, u).await.unwrap());
}

#[sqlx::test(migrator = "aurora_db::MIGRATOR")]
async fn manual_rows_are_untouched_by_the_idp_sync(pool: PgPool) {
    let f = fx(&pool).await;
    let u = member(&pool, f.org, "member").await;
    let manual = group(&pool, f.org, "manual", None).await;
    add(&pool, manual, f.org, u).await.unwrap();
    let mut c = pool.acquire().await.unwrap();
    groups::sync_idp_memberships(&mut c, u, "cern", &[])
        .await
        .unwrap();
    assert!(groups::is_member(&pool, manual, u).await.unwrap());
}
