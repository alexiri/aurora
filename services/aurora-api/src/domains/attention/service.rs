//! (P5-REFINE-8, D-417, C-1080, C-1082) One attention feed per level, built from the same sources the
//! counts read.
//!
//! Each level has one *collect* step (bounded queries, no names beyond the rows, no sentences) and one
//! *render* step (sentences, links). The feed renders what it collected; the counts (C-1082) count what
//! the same collect step returned, so a rail's number is the feed's `count` by construction.
//!
//! - Workspace: approvals the caller may decide, access requests (`can_own`), broken accounts, and the
//!   caller's own requests (`own: true`, never counted).
//! - Organization: setup requests whose level ([`level::of`]) is `organization`.
//! - Administration: the six C-1022 kinds (`admin::attention`) plus requests of level `administration`.

use super::text;
use crate::app::AppState;
use crate::domains::accounts::dto::HealthState;
use crate::domains::accounts::health;
use crate::domains::admin::dto::{
    AttentionActor, AttentionItem, AttentionKind, AttentionLevel, AttentionList, AttentionState,
};
use crate::domains::app_setup_requests::dto::{ClosedBy, ClosedByRole, RequestLevel};
use crate::domains::app_setup_requests::level;
use crate::domains::app_setup_requests::service::{
    self as requests, REQUEST_HISTORY_DAYS, needs_of, thing_in,
};
use crate::domains::hosted::answer::{self, AnswerReason};
use crate::http::auth::Caller;
use aurora_core::{ApiError, dash_paths};
use aurora_db::authz::WorkspaceAuth;
use aurora_db::repo::action_approvals::{self, ActionApprovalRow};
use aurora_db::repo::app_setup_requests::{self as repo, AskerRow, MineDone, RequestRow};
use aurora_db::repo::approvals::{self, ApprovalRow};
use aurora_db::repo::attention::{self as arepo, AccessRow, AccountHealthRow, HostedAgentRow};
use aurora_db::repo::contacts;
use aurora_registry::snapshot::Snapshot;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use utoipa::ToSchema;
use uuid::Uuid;

/// The most items one source returns (C-1080: "at most 200 items; the rest are summarized by `count`").
pub const FEED_LIMIT: i64 = 200;
/// The most askers an item names.
const ACTORS_MAX: usize = 5;

pub const TOPIC_APPS: &str = "apps";
pub const TOPIC_CHAT: &str = "chat-platforms";
pub const TOPIC_AI: &str = "ai-models";
pub const TOPIC_PEOPLE: &str = "people";
pub const TOPIC_APPROVALS: &str = "approvals";
pub const TOPIC_ACCOUNTS: &str = "accounts";
pub const TOPIC_AGENTS: &str = "agents";

/// `open` (default) or `done`; 400 otherwise.
pub fn parse_state(state: Option<&str>) -> Result<bool, ApiError> {
    match state.unwrap_or("open") {
        "open" => Ok(false),
        "done" => Ok(true),
        _ => Err(ApiError::invalid_field(
            "Unknown state.",
            "/state",
            "open or done",
        )),
    }
}

/// The list's `count`: the open items with `own: false`.
pub fn count_of(items: &[AttentionItem]) -> i64 {
    items
        .iter()
        .filter(|i| i.state == AttentionState::Open && !i.own)
        .count() as i64
}

/// Decisions first, then oldest first (C-1022); `done` newest first.
fn sort(items: &mut [AttentionItem], done: bool) {
    if done {
        items.sort_by(|a, b| b.closed_at.cmp(&a.closed_at).then(b.since.cmp(&a.since)));
    } else {
        items.sort_by_key(|i| (!i.decision, i.since));
    }
}

pub fn list_of(mut items: Vec<AttentionItem>, done: bool) -> AttentionList {
    sort(&mut items, done);
    let count = count_of(&items);
    AttentionList { items, count }
}

fn subject_topic(subject: &str) -> &'static str {
    match subject {
        "chat_platform" => TOPIC_CHAT,
        "ai_key" => TOPIC_AI,
        _ => TOPIC_APPS,
    }
}

fn request_kind(subject: &str) -> AttentionKind {
    match subject {
        "chat_platform" => AttentionKind::ChatPlatformRequest,
        "ai_key" => AttentionKind::AiKeyRequest,
        _ => AttentionKind::AppRequest,
    }
}

// ---------------------------------------------------------------------------------------------
// Requests (organization and Administration levels)
// ---------------------------------------------------------------------------------------------

/// The open requests of `org` (all organizations with `None`) whose level is `want`. One query.
pub async fn open_requests_at(
    s: &AppState,
    org: Option<Uuid>,
    want: RequestLevel,
) -> Result<Vec<RequestRow>, ApiError> {
    Ok(repo::open_requests(&s.pool, org)
        .await?
        .into_iter()
        .filter(|r| level::of(r) == want)
        .take(FEED_LIMIT as usize)
        .collect())
}

async fn closed_requests_at(
    s: &AppState,
    org: Option<Uuid>,
    want: RequestLevel,
) -> Result<Vec<RequestRow>, ApiError> {
    Ok(
        repo::closed_within(&s.pool, org, REQUEST_HISTORY_DAYS, FEED_LIMIT * 4)
            .await?
            .into_iter()
            .filter(|r| level::of(r) == want)
            .take(FEED_LIMIT as usize)
            .collect(),
    )
}

/// Slugs of the organizations of `rows` (one query each distinct organization; the organization
/// feed has one).
async fn org_slugs(s: &AppState, rows: &[RequestRow]) -> Result<HashMap<Uuid, String>, ApiError> {
    let mut out = HashMap::new();
    for r in rows {
        if let std::collections::hash_map::Entry::Vacant(e) = out.entry(r.org_id)
            && let Some((slug, _)) = contacts::slugs(&s.pool, r.org_id, None).await?
        {
            e.insert(slug);
        }
    }
    Ok(out)
}

/// Renders requests at `at` for a viewer who may act there (organization owners and admins, or
/// instance admins). Askers are loaded in one query, the people a request was resolved for in another.
async fn render_requests(
    s: &AppState,
    rows: Vec<RequestRow>,
    at: RequestLevel,
    done: bool,
) -> Result<Vec<AttentionItem>, ApiError> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let askers = repo::askers(&s.pool, &ids).await?;
    let resolved = if done {
        repo::resolved_for(&s.pool, &ids).await?
    } else {
        Vec::new()
    };
    let snap = requests::snapshot_for(s, &rows).await;
    let slugs = if at == RequestLevel::Organization {
        org_slugs(s, &rows).await?
    } else {
        HashMap::new()
    };
    let mut by_req: HashMap<Uuid, Vec<&AskerRow>> = HashMap::new();
    for a in &askers {
        by_req.entry(a.request_id).or_default().push(a);
    }
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let mine = by_req.remove(&r.id).unwrap_or_default();
        let mut people: Vec<(Uuid, String)> = mine
            .iter()
            .map(|a| (a.user_id, a.display_name.clone()))
            .collect();
        for (_, u, n, _) in resolved.iter().filter(|x| x.0 == r.id) {
            if !people.iter().any(|(p, _)| p == u) {
                people.push((*u, n.clone()));
            }
        }
        let reason = mine.first().and_then(|a| a.reason.clone());
        out.push(request_item(
            &r,
            at,
            &people,
            reason,
            snap.as_ref(),
            slugs.get(&r.org_id).map(String::as_str),
        ));
    }
    Ok(out)
}

fn request_item(
    r: &RequestRow,
    at: RequestLevel,
    people: &[(Uuid, String)],
    reason: Option<String>,
    snap: Option<&Snapshot>,
    org_slug: Option<&str>,
) -> AttentionItem {
    let thing = thing_in(snap, &r.subject, &r.app_id);
    let names: Vec<String> = people
        .iter()
        .take(ACTORS_MAX)
        .map(|(_, n)| n.clone())
        .collect();
    let total = people.len() as i64;
    let detail = match r.subject.as_str() {
        "chat_platform" => text::chat_detail(&thing),
        "ai_key" => text::AI_KEY_DETAIL.to_owned(),
        _ => text::app_detail(&thing, &needs_of(&r.needs), r.unavailable, at),
    };
    let (label, href) = match (r.subject.as_str(), at, org_slug) {
        ("chat_platform", RequestLevel::Administration, _) => (
            "Connect a chat platform",
            "/admin/chat-platforms".to_owned(),
        ),
        ("chat_platform", _, Some(o)) => {
            ("Connect a chat platform", format!("/o/{o}/chat-platforms"))
        }
        ("ai_key", _, Some(o)) => ("Add an AI key", dash_paths::org_ai_models(o)),
        ("ai_key", _, None) => ("Add an AI key", dash_paths::admin_ai_models()),
        (_, RequestLevel::Administration, _) => ("Set up", dash_paths::admin_oauth_clients()),
        (_, _, Some(o)) => ("Set up", dash_paths::org_app_setup(o, &r.app_id)),
        (_, _, None) => ("Set up", dash_paths::admin_oauth_clients()),
    };
    let lvl = match at {
        RequestLevel::Organization => AttentionLevel::Organization,
        RequestLevel::Administration => AttentionLevel::Administration,
    };
    let mut item = AttentionItem::new(
        request_kind(&r.subject),
        lvl,
        subject_topic(&r.subject),
        r.id.to_string(),
        (text::request_title(&names, total, &thing), detail),
        r.created_at,
        href.clone(),
    );
    item.id = r.id.to_string();
    item.actors = people
        .iter()
        .take(ACTORS_MAX)
        .map(|(u, n)| AttentionActor {
            user_id: *u,
            display_name: text::name(n),
        })
        .collect();
    item.actors_total = total;
    item.reason = reason;
    if r.status == "open" {
        item = item.action(label, href);
        item.can_decline = true;
    } else {
        let role = r.closed_by_role.as_deref().and_then(ClosedByRole::parse);
        item.state = if r.status == "declined" {
            AttentionState::Declined
        } else {
            AttentionState::Done
        };
        item.closed_at = r.closed_at;
        item.message.clone_from(&r.closed_message);
        item.closed_by_role = role;
        // The decliner's name for the viewers of the level that declined it only (C-981).
        let shown = matches!(
            (at, role),
            (
                RequestLevel::Administration,
                Some(ClosedByRole::InstanceAdmin | ClosedByRole::OrgAdmin)
            ) | (RequestLevel::Organization, Some(ClosedByRole::OrgAdmin))
        );
        item.closed_by = r
            .closed_by_name
            .as_deref()
            .filter(|_| shown)
            .map(|n| ClosedBy {
                display_name: text::name(n),
            });
    }
    item
}

/// `GET /orgs/{org}/attention` (organization owners and admins).
pub async fn org_feed(
    s: &AppState,
    c: &Caller,
    org: Uuid,
    done: bool,
) -> Result<AttentionList, ApiError> {
    c.org(s, org).await?.require_admin()?;
    let rows = if done {
        closed_requests_at(s, Some(org), RequestLevel::Organization).await?
    } else {
        open_requests_at(s, Some(org), RequestLevel::Organization).await?
    };
    let items = render_requests(s, rows, RequestLevel::Organization, done).await?;
    Ok(list_of(items, done))
}

/// The Administration level's request items (the rest of `GET /admin/attention` is C-1022's).
pub async fn admin_request_items(s: &AppState, done: bool) -> Result<Vec<AttentionItem>, ApiError> {
    let rows = if done {
        closed_requests_at(s, None, RequestLevel::Administration).await?
    } else {
        open_requests_at(s, None, RequestLevel::Administration).await?
    };
    render_requests(s, rows, RequestLevel::Administration, done).await
}

// ---------------------------------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------------------------------

/// What the workspace feed is made of, before any sentence (the counts count this).
struct WsOpen {
    approvals: Vec<ApprovalRow>,
    actions: Vec<ActionApprovalRow>,
    access: Vec<AccessRow>,
    accounts: Vec<(AccountHealthRow, HealthState)>,
    /// (C-1080 `agent_cannot_answer`) Hosted agents with no usable AI key or model; workspace owners only.
    cannot_answer: Vec<(HostedAgentRow, AnswerReason)>,
    mine: Vec<RequestRow>,
}

/// The agents of the workspace that cannot answer, for the people who can give them a key (D-384).
async fn cannot_answer_of(
    s: &AppState,
    auth: &WorkspaceAuth,
) -> Result<Vec<(HostedAgentRow, AnswerReason)>, ApiError> {
    let agents = arepo::hosted_agents_running(&s.pool, auth.workspace_id, FEED_LIMIT).await?;
    if agents.is_empty() {
        return Ok(Vec::new());
    }
    let mut conn = s.pool.acquire().await.map_err(aurora_db::DbError::from)?;
    let mut out = Vec::new();
    for a in agents {
        if let Some(r) = answer::reason_of(
            s,
            &mut conn,
            a.id,
            auth.org_id,
            auth.workspace_id,
            a.harness.as_deref(),
        )
        .await?
        {
            out.push((a, r));
        }
    }
    Ok(out)
}

fn reason_key(r: AnswerReason) -> &'static str {
    match r {
        AnswerReason::NoAiKey => "no_ai_key",
        AnswerReason::KeyNotUsable => "key_not_usable",
        AnswerReason::NoModel => "no_model",
    }
}

fn cannot_answer_item(a: &HostedAgentRow, r: AnswerReason, sl: &Slugs) -> AttentionItem {
    let id = a.id.to_string();
    ws_item(
        AttentionKind::AgentCannotAnswer,
        TOPIC_AGENTS,
        id.clone(),
        text::cannot_answer_text(&a.name, reason_key(r)),
        a.created_at,
        String::new(),
    )
    .action("Fix", dash_paths::ws_agent_access(&sl.org, &sl.ws, &id))
}

async fn ws_open(s: &AppState, c: &Caller, auth: &WorkspaceAuth) -> Result<WsOpen, ApiError> {
    let ws = auth.workspace_id;
    let (approvals, actions) = if auth.can_approve() {
        (
            approvals::list_pending_for(&s.pool, c.user_id, None, Some(ws), FEED_LIMIT).await?,
            action_approvals::list_workspace(
                &s.pool,
                c.user_id,
                ws,
                Some("pending"),
                None,
                None,
                FEED_LIMIT,
            )
            .await?,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let access = if auth.can_own() {
        arepo::access_requests(&s.pool, ws, false, REQUEST_HISTORY_DAYS, FEED_LIMIT).await?
    } else {
        Vec::new()
    };
    let accounts = arepo::accounts_needing_attention(&s.pool, auth.org_id, ws, FEED_LIMIT)
        .await?
        .into_iter()
        .filter_map(|a| {
            let st = health::state_of(a.status.as_deref(), a.last_ok_at, a.last_error_at);
            matches!(st, HealthState::Failing | HealthState::SignInExpired).then_some((a, st))
        })
        .collect();
    let cannot_answer = if auth.can_own() {
        cannot_answer_of(s, auth).await?
    } else {
        Vec::new()
    };
    let mine = repo::open_of_asker(&s.pool, auth.org_id, c.user_id).await?;
    Ok(WsOpen {
        approvals,
        actions,
        access,
        accounts,
        cannot_answer,
        mine,
    })
}

struct Slugs {
    org: String,
    ws: String,
}

async fn ws_slugs(s: &AppState, auth: &WorkspaceAuth) -> Result<Slugs, ApiError> {
    let (org, ws) = contacts::slugs(&s.pool, auth.org_id, Some(auth.workspace_id))
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace not found."))?;
    Ok(Slugs {
        org,
        ws: ws.ok_or_else(|| ApiError::not_found("Workspace not found."))?,
    })
}

fn ws_item(
    kind: AttentionKind,
    topic: &str,
    subject_id: String,
    text: (String, String),
    since: chrono::DateTime<chrono::Utc>,
    href: String,
) -> AttentionItem {
    AttentionItem::new(
        kind,
        AttentionLevel::Workspace,
        topic,
        subject_id,
        text,
        since,
        href,
    )
}

async fn agent_names(
    s: &AppState,
    rows: &[ActionApprovalRow],
) -> Result<HashMap<Uuid, String>, ApiError> {
    if rows.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: Vec<Uuid> = rows.iter().map(|r| r.agent_id).collect();
    Ok(arepo::agent_names(&s.pool, &ids)
        .await?
        .into_iter()
        .collect())
}

fn approval_item(a: &ApprovalRow, snap: Option<&Snapshot>, sl: &Slugs) -> AttentionItem {
    let app = a
        .app_id
        .as_deref()
        .map(|id| requests::app_name_in(snap, id));
    let id = a.id.to_string();
    ws_item(
        AttentionKind::Approval,
        TOPIC_APPROVALS,
        id.clone(),
        text::approval_text(
            a.agent_name.as_deref().unwrap_or("An agent"),
            app.as_deref(),
            &a.host,
        ),
        a.created_at,
        String::new(),
    )
    .action(
        "Review",
        dash_paths::ws_approvals(&sl.org, &sl.ws, Some(&id)),
    )
}

fn action_item(a: &ActionApprovalRow, agent: &str, sl: &Slugs) -> AttentionItem {
    let id = a.id.to_string();
    ws_item(
        AttentionKind::Approval,
        TOPIC_APPROVALS,
        id.clone(),
        text::action_approval_text(agent),
        a.created_at,
        String::new(),
    )
    .action(
        "Review",
        dash_paths::ws_approvals(&sl.org, &sl.ws, Some(&id)),
    )
}

fn access_item(a: &AccessRow, snap: Option<&Snapshot>, sl: &Slugs) -> AttentionItem {
    let app = a
        .app_id
        .as_deref()
        .map(|id| requests::app_name_in(snap, id));
    ws_item(
        AttentionKind::AccessRequest,
        TOPIC_ACCOUNTS,
        a.id.to_string(),
        text::access_text(&a.agent_name, app.as_deref(), a.host.as_deref()),
        a.created_at,
        String::new(),
    )
    .action(
        "Review",
        dash_paths::ws_agent_access(&sl.org, &sl.ws, &a.agent_id.to_string()),
    )
}

fn account_item(
    a: &AccountHealthRow,
    st: HealthState,
    auth: &WorkspaceAuth,
    sl: &Slugs,
) -> AttentionItem {
    let expired = st == HealthState::SignInExpired;
    // Workspace accounts are their owners'; organization accounts the organization's (C-917 `can.manage`).
    let can_fix = if a.org_account {
        auth.org_role.is_admin()
    } else {
        auth.can_own()
    };
    let fixer = (!can_fix).then_some(if a.org_account {
        "an organization admin"
    } else {
        "a workspace owner"
    });
    let accounts = dash_paths::ws_accounts(&sl.org, &sl.ws);
    let item = ws_item(
        AttentionKind::BrokenAccount,
        TOPIC_ACCOUNTS,
        a.id.to_string(),
        text::broken_text(&a.name, expired, a.last_error.as_deref(), fixer),
        a.since,
        accounts.clone(),
    );
    match (can_fix, expired && a.is_connection) {
        (false, _) => item,
        (true, true) => item.action(
            "Sign in again",
            dash_paths::ws_reconnect(&sl.org, &sl.ws, &a.id.to_string()),
        ),
        (true, false) => item.action("Fix", accounts),
    }
}

/// The caller's own request, open or ended (`own: true`, never counted).
fn mine_item(
    r: &RequestRow,
    my_ask: Option<&AskerRow>,
    me: (Uuid, &str),
    ended: Option<&MineDone>,
    snap: Option<&Snapshot>,
    sl: &Slugs,
) -> AttentionItem {
    let thing = thing_in(snap, &r.subject, &r.app_id);
    let lvl = level::of(r);
    let href = match r.subject.as_str() {
        "app" => dash_paths::ws_app_setup(&sl.org, &sl.ws, &r.app_id),
        _ => dash_paths::ws_overview(&sl.org, &sl.ws),
    };
    let words = match ended {
        None => text::my_request_text(&thing, lvl),
        Some(d) => text::my_done_text(&thing, !d.resolved_for_me && r.status == "declined"),
    };
    let mut item = ws_item(
        AttentionKind::MyRequest,
        subject_topic(&r.subject),
        r.id.to_string(),
        words,
        my_ask.map_or(r.created_at, |a| a.asked_at),
        href,
    );
    item.id = r.id.to_string();
    item.own = true;
    item.actors = vec![AttentionActor {
        user_id: me.0,
        display_name: text::name(me.1),
    }];
    item.actors_total = 1;
    item.reason = my_ask.and_then(|a| a.reason.clone());
    if let Some(d) = ended {
        let declined = !d.resolved_for_me && r.status == "declined";
        item.state = if declined {
            AttentionState::Declined
        } else {
            AttentionState::Done
        };
        item.closed_at = Some(d.done_at);
        if declined {
            item.message.clone_from(&r.closed_message);
            item.closed_by_role = r.closed_by_role.as_deref().and_then(ClosedByRole::parse);
        } else {
            item.closed_by_role = Some(ClosedByRole::System);
        }
    }
    item
}

/// `GET /workspaces/{ws}/attention` (any active member).
pub async fn workspace_feed(
    s: &AppState,
    c: &Caller,
    ws: Uuid,
    done: bool,
) -> Result<AttentionList, ApiError> {
    let auth = c.workspace(s, ws).await?;
    let sl = ws_slugs(s, &auth).await?;
    let me = (c.user_id, repo::asker_name(&s.pool, c.user_id).await?);
    let items = if done {
        workspace_done(s, c, &auth, &sl, (me.0, &me.1)).await?
    } else {
        let o = ws_open(s, c, &auth).await?;
        let names = agent_names(s, &o.actions).await?;
        let need_snap = o.approvals.iter().any(|a| a.app_id.is_some())
            || o.access.iter().any(|a| a.app_id.is_some())
            || o.mine.iter().any(|r| r.subject == "app");
        let snap = if need_snap {
            crate::domains::apps::service::snapshot(s)
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        let my_asks = if o.mine.is_empty() {
            Vec::new()
        } else {
            let ids: Vec<Uuid> = o.mine.iter().map(|r| r.id).collect();
            repo::askers(&s.pool, &ids).await?
        };
        let mut items = Vec::new();
        items.extend(
            o.approvals
                .iter()
                .map(|a| approval_item(a, snap.as_ref(), &sl)),
        );
        items.extend(o.actions.iter().map(|a| {
            action_item(
                a,
                names.get(&a.agent_id).map_or("An agent", String::as_str),
                &sl,
            )
        }));
        items.extend(o.access.iter().map(|a| access_item(a, snap.as_ref(), &sl)));
        items.extend(
            o.accounts
                .iter()
                .map(|(a, st)| account_item(a, *st, &auth, &sl)),
        );
        items.extend(
            o.cannot_answer
                .iter()
                .map(|(a, r)| cannot_answer_item(a, *r, &sl)),
        );
        items.extend(o.mine.iter().map(|r| {
            let ask = my_asks
                .iter()
                .find(|a| a.request_id == r.id && a.user_id == c.user_id);
            mine_item(r, ask, (me.0, &me.1), None, snap.as_ref(), &sl)
        }));
        items
    };
    Ok(list_of(items, done))
}

async fn workspace_done(
    s: &AppState,
    c: &Caller,
    auth: &WorkspaceAuth,
    sl: &Slugs,
    me: (Uuid, &str),
) -> Result<Vec<AttentionItem>, ApiError> {
    let ws = auth.workspace_id;
    let since = chrono::Utc::now() - chrono::Duration::days(i64::from(REQUEST_HISTORY_DAYS));
    let mut items = Vec::new();
    let snap = crate::domains::apps::service::snapshot(s)
        .await
        .ok()
        .flatten();
    if auth.can_approve() {
        for a in
            approvals::list_decided_for(&s.pool, c.user_id, ws, REQUEST_HISTORY_DAYS, FEED_LIMIT)
                .await?
        {
            let mut i = approval_item(&a, snap.as_ref(), sl);
            i.state = AttentionState::Done;
            i.closed_at = a.decided_at;
            items.push(i);
        }
        let decided: Vec<ActionApprovalRow> =
            action_approvals::list_workspace(&s.pool, c.user_id, ws, None, None, None, FEED_LIMIT)
                .await?
                .into_iter()
                .filter(|a| a.status != "pending" && a.decided_at.is_some_and(|d| d >= since))
                .collect();
        let names = agent_names(s, &decided).await?;
        for a in &decided {
            let mut i = action_item(
                a,
                names.get(&a.agent_id).map_or("An agent", String::as_str),
                sl,
            );
            i.state = AttentionState::Done;
            i.closed_at = a.decided_at;
            items.push(i);
        }
    }
    if auth.can_own() {
        for a in arepo::access_requests(&s.pool, ws, true, REQUEST_HISTORY_DAYS, FEED_LIMIT).await?
        {
            let mut i = access_item(&a, snap.as_ref(), sl);
            i.state = AttentionState::Done;
            i.closed_at = a.resolved_at;
            items.push(i);
        }
    }
    for d in repo::history_of_asker(
        &s.pool,
        auth.org_id,
        c.user_id,
        REQUEST_HISTORY_DAYS,
        FEED_LIMIT,
    )
    .await?
    {
        items.push(mine_item(&d.row, None, me, Some(&d), snap.as_ref(), sl));
    }
    Ok(items)
}

// ---------------------------------------------------------------------------------------------
// Counts (C-1082)
// ---------------------------------------------------------------------------------------------

/// One level's rail number and its topics.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LevelCount {
    /// C-1080's `count` for the level.
    pub count: i64,
    /// The open items per `topic` (only topics with items).
    pub topics: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct OrgCount {
    pub org_id: Uuid,
    pub count: i64,
    pub topics: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WorkspaceCount {
    pub workspace_id: Uuid,
    pub count: i64,
    pub topics: BTreeMap<String, i64>,
}

/// `GET /attention/counts` (C-1082): `null` for every scope the caller cannot act on or did not ask for.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AttentionCounts {
    pub administration: Option<LevelCount>,
    pub organization: Option<OrgCount>,
    pub workspace: Option<WorkspaceCount>,
}

fn topics_of<'a>(it: impl Iterator<Item = &'a str>) -> (i64, BTreeMap<String, i64>) {
    let mut m: BTreeMap<String, i64> = BTreeMap::new();
    let mut n = 0;
    for t in it {
        *m.entry(t.to_owned()).or_default() += 1;
        n += 1;
    }
    (n, m)
}

/// The Administration level's open topics (C-1022's six kinds and the requests), no sentences.
pub async fn admin_topics(s: &AppState) -> Result<(i64, BTreeMap<String, i64>), ApiError> {
    let six = crate::domains::admin::attention::open_items(s).await?;
    let reqs = open_requests_at(s, None, RequestLevel::Administration).await?;
    Ok(topics_of(
        six.iter()
            .map(|i| i.topic.as_str())
            .chain(reqs.iter().map(|r| subject_topic(&r.subject))),
    ))
}

pub async fn counts(
    s: &AppState,
    c: &Caller,
    org: Option<Uuid>,
    ws: Option<Uuid>,
) -> Result<AttentionCounts, ApiError> {
    let administration = if c.is_session() && c.is_instance_admin {
        let (count, topics) = admin_topics(s).await?;
        Some(LevelCount { count, topics })
    } else {
        None
    };
    let organization = match org {
        Some(o) if c.org(s, o).await.is_ok_and(|a| a.role.is_admin()) => {
            let reqs = open_requests_at(s, Some(o), RequestLevel::Organization).await?;
            let (count, topics) = topics_of(reqs.iter().map(|r| subject_topic(&r.subject)));
            Some(OrgCount {
                org_id: o,
                count,
                topics,
            })
        }
        _ => None,
    };
    let workspace = match ws {
        Some(w) => match c.workspace(s, w).await {
            Ok(auth) => {
                let o = ws_open(s, c, &auth).await?;
                let (count, topics) = topics_of(
                    o.approvals
                        .iter()
                        .map(|_| TOPIC_APPROVALS)
                        .chain(o.actions.iter().map(|_| TOPIC_APPROVALS))
                        .chain(o.access.iter().map(|_| TOPIC_ACCOUNTS))
                        .chain(o.accounts.iter().map(|_| TOPIC_ACCOUNTS))
                        .chain(o.cannot_answer.iter().map(|_| TOPIC_AGENTS)),
                );
                Some(WorkspaceCount {
                    workspace_id: w,
                    count,
                    topics,
                })
            }
            Err(_) => None,
        },
        None => None,
    };
    Ok(AttentionCounts {
        administration,
        organization,
        workspace,
    })
}
