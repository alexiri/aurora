//! Conversation data lifecycle (phase 2, RFE-010, D-53): archive, delete, transcript redaction,
//! per-org retention and export. Authorization is decided here, server-side, on top of the access
//! law (`can_see_conversation` always applies: an invisible conversation is a 404).

use super::dto::{
    Conversation, PutRetention, RedactRequest, RedactResult, Retention, TranscriptEvent,
};
use super::service::{self, load_conversation_auth, speaker_label};
use super::zip::{self, ZipWriter};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::attachments::{blob, quota};
use crate::http::auth::Caller;
use aurora_core::limits::{
    EXPORT_MAX_BYTES, EXPORT_MAX_EVENTS, RETENTION_MAX_DAYS, RETENTION_MIN_DAYS,
    RETENTION_SWEEP_BATCH, RETENTION_SWEEP_PER_ORG,
};
use aurora_core::{ApiError, FieldError};
use aurora_db::authz::WorkspaceAuth;
use aurora_db::repo::conversations::{self as convs, ConversationRow};
use aurora_db::repo::lifecycle::{self, Deleted};
use aurora_db::repo::{transcript, turns};
use aurora_db::{DbError, PgPool};
use serde_json::{Map, Value, json};
use uuid::Uuid;

const REASON_MAX: usize = 500;
/// Structural ids a redaction keeps (docs/10 §Transcript redaction); everything else is content.
const STRUCTURAL: [&str; 5] = [
    "turn_id",
    "call_id",
    "message_id",
    "approval_id",
    "attachment_id",
];

fn forbidden() -> ApiError {
    ApiError::forbidden("You are not allowed to do this to this conversation.")
}

fn db(e: sqlx::Error) -> ApiError {
    ApiError::from(DbError::from(e))
}

// ------------------------------------------------------------------ authority

/// Delete / export: a `direct` conversation by its user or `can_own`; every other kind by
/// `can_own` (export additionally: the turn author alone is not enough, and neither is the role).
fn owner_or_direct_user(c: &Caller, row: &ConversationRow, auth: &WorkspaceAuth) -> bool {
    auth.can_own() || (row.kind == "direct" && row.direct_user_id == Some(c.user_id))
}

// ------------------------------------------------------------------ archive

pub async fn set_archived(
    state: &AppState,
    c: &Caller,
    id: Uuid,
    archived: bool,
) -> Result<Conversation, ApiError> {
    // `can_see_conversation` (404 otherwise); `can_use` holds for every workspace member, so the
    // non-direct rule needs no further check.
    let (row, auth) = load_conversation_auth(state, c, id).await?;
    if !auth.can_use() {
        return Err(forbidden());
    }
    let mut conn = state.pool.acquire().await.map_err(db)?;
    lifecycle::set_archived(&mut conn, id, archived, archived.then_some(c.user_id)).await?;
    let row = convs::get(&mut *conn, id).await?.unwrap_or(row);
    let active = turns::active_turn(&mut *conn, id).await?;
    Ok(Conversation::from_row(row, active))
}

// ------------------------------------------------------------------ delete

/// Remove blob bytes after the delete committed; a failure leaves an orphan for the daily sweep.
pub(crate) async fn remove_blobs(pool: &PgPool, blobs: &[(String, String)]) {
    for (backend, key) in blobs {
        match blob::for_backend(backend, pool) {
            Some(store) => {
                if let Err(e) = store.delete(key).await {
                    tracing::warn!(error = %e.message(), "blob delete failed; left to the orphan sweep");
                }
            }
            None => tracing::warn!(backend, "unknown blob backend; bytes left in place"),
        }
    }
}

pub async fn delete(state: &AppState, c: &Caller, id: Uuid) -> Result<(), ApiError> {
    let (row, auth) = load_conversation_auth(state, c, id).await?;
    if !owner_or_direct_user(c, &row, &auth) {
        return Err(forbidden());
    }
    let mut tx = state.pool.begin().await.map_err(db)?;
    if !convs::lock(&mut tx, id).await? {
        return Err(ApiError::not_found("Conversation not found."));
    }
    if lifecycle::has_active_turn(&mut tx, id).await? {
        return Err(ApiError::conflict(
            "conversation_active",
            "A turn is still running in this conversation; abort it first.",
        ));
    }
    let gone = lifecycle::delete_conversation(&mut tx, id).await?;
    let ev = AuditEvent::new(
        AuditActor::from_caller(c),
        "conversation.delete",
        Outcome::Success,
    )
    .org(auth.org_id)
    .workspace(row.workspace_id)
    .target("conversation", id)
    .metadata(delete_meta(&[(id, &row.kind)], row.agent_id, &gone, None));
    audit::insert_tx(&mut tx, &ev).await?;
    service::notify_work(&mut tx).await?;
    tx.commit().await.map_err(db)?;
    remove_blobs(&state.pool, &gone.blobs).await;
    Ok(())
}

/// Ids, kinds and counts only: never content (docs/04 §Audit).
pub(crate) fn delete_meta(
    convs: &[(Uuid, &str)],
    agent: Uuid,
    gone: &Deleted,
    reason: Option<&str>,
) -> Value {
    let mut m = json!({
        "conversation_ids": convs.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        "kinds": convs.iter().map(|(_, k)| *k).collect::<Vec<_>>(),
        "agent_id": agent,
        "turns": gone.turns, "events": gone.events, "attachments": gone.attachments,
        "session_purge_queued": gone.session_purged,
    });
    if let Some(r) = reason {
        m["reason"] = json!(r);
    }
    m
}

// ------------------------------------------------------------------ redaction

fn marker(orig: &Value, label: &str, reason: Option<&str>) -> Value {
    let mut m = Map::new();
    for k in STRUCTURAL {
        if let Some(v) = orig.get(k) {
            m.insert(k.to_owned(), v.clone());
        }
    }
    m.insert("redacted".into(), json!(true));
    m.insert("redacted_at".into(), json!(chrono::Utc::now().to_rfc3339()));
    m.insert("redacted_by".into(), json!(label));
    if let Some(r) = reason {
        m.insert("reason".into(), json!(r));
    }
    Value::Object(m)
}

fn clean_reason(r: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(r) = r.map(str::trim).filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    if r.chars().count() > REASON_MAX || r.chars().any(char::is_control) {
        return Err(ApiError::invalid_request(
            "Invalid request.",
            vec![FieldError::new(
                "/reason",
                "must be at most 500 characters without control characters",
            )],
        ));
    }
    Ok(Some(r.to_owned()))
}

pub enum Target {
    Event(i64),
    Turn(Uuid),
}

/// Redact one event or every event of a turn. Authority: the author of the event's turn, the
/// direct user of a direct conversation, or `can_own`, always limited to conversations the caller
/// can see. Idempotent and irreversible.
pub async fn redact(
    state: &AppState,
    c: &Caller,
    conversation: Uuid,
    target: Target,
    req: &RedactRequest,
) -> Result<RedactResult, ApiError> {
    let reason = clean_reason(req.reason.as_deref())?;
    let (row, auth) = load_conversation_auth(state, c, conversation).await?;
    let label = speaker_label(state, c).await?;
    let mut tx = state.pool.begin().await.map_err(db)?;
    if !convs::lock(&mut tx, conversation).await? {
        return Err(ApiError::not_found("Conversation not found."));
    }
    let events = match &target {
        Target::Event(seq) => vec![
            lifecycle::event_ref(&mut tx, conversation, *seq)
                .await?
                .ok_or_else(|| ApiError::not_found("Event not found."))?,
        ],
        Target::Turn(t) => {
            let evs = lifecycle::turn_events(&mut tx, conversation, *t).await?;
            if evs.is_empty() && turns::get(&mut *tx, *t).await?.is_none() {
                return Err(ApiError::not_found("Turn not found."));
            }
            evs
        }
    };
    // Who authored the turn(s) the events belong to.
    let mut authors: Vec<Option<Uuid>> = Vec::new();
    for tid in events.iter().filter_map(|e| e.turn_id) {
        let a = turns::get(&mut *tx, tid)
            .await?
            .and_then(|t| t.speaker_user_id);
        authors.push(a);
    }
    let direct_user = row.kind == "direct" && row.direct_user_id == Some(c.user_id);
    let may = auth.can_own()
        || direct_user
        || (!authors.is_empty() && authors.iter().all(|a| *a == Some(c.user_id)));
    if !may {
        return Err(forbidden());
    }
    let mut done = Vec::new();
    let mut types = Vec::new();
    for e in events.iter().filter(|e| e.redacted_at.is_none()) {
        let payload = marker(&e.payload, &label, reason.as_deref());
        lifecycle::write_redaction(&mut tx, conversation, e.seq, &payload, Some(c.user_id)).await?;
        if e.ty == "turn.queued"
            && let Some(t) = e.turn_id
        {
            lifecycle::scrub_turn_text(&mut tx, t).await?;
        }
        done.push(e.seq);
        types.push(e.ty.clone());
    }
    // A turn redaction also scrubs the turn's stored text, even when its queued event is gone.
    if let Target::Turn(t) = &target {
        lifecycle::scrub_turn_text(&mut tx, *t).await?;
    }
    let mut session_reset = false;
    if req.reset_session {
        let (agent, old) = lifecycle::reset_session(&mut tx, conversation).await?;
        if let Some(r) = old.as_deref().filter(|r| !r.is_empty()) {
            lifecycle::enqueue_purge(&mut tx, agent, Some(conversation), r).await?;
        }
        session_reset = true;
        service::notify_work(&mut tx).await?;
    }
    if !done.is_empty() || session_reset {
        let ev = AuditEvent::new(
            AuditActor::from_caller(c),
            "transcript_event.redact",
            Outcome::Success,
        )
        .org(auth.org_id)
        .workspace(row.workspace_id)
        .target("conversation", conversation)
        .metadata(json!({
            "seqs": done, "types": types, "reason": reason,
            "turn_id": match &target { Target::Turn(t) => Some(*t), Target::Event(_) => None },
            "reset_session": session_reset,
        }));
        audit::insert_tx(&mut tx, &ev).await?;
    }
    tx.commit().await.map_err(db)?;
    for seq in &done {
        state
            .conversation_bus
            .publish_redacted(&state.pool, conversation, *seq)
            .await;
    }
    Ok(RedactResult {
        redacted_seqs: done,
        session_reset,
    })
}

// ------------------------------------------------------------------ retention

async fn retention_view(
    state: &AppState,
    conn: &mut sqlx::PgConnection,
    org: Uuid,
) -> Result<Retention, ApiError> {
    let (quota, days) = aurora_db::repo::attachments::org_limits(&mut *conn, org).await?;
    let a = &state.config.attachments;
    Ok(Retention {
        conversations_days: lifecycle::org_conversations_days(&mut *conn, org).await?,
        conversations_days_max: state.config.retention.conversation_days_max.map(i64::from),
        attachments_days: days,
        attachments_days_max: a.days_max.map(i64::from),
        attachment_quota_mib: quota,
        attachment_quota_mib_effective: quota::effective_quota_bytes(
            &state.config.attachments,
            quota,
        )
        .map(|b| b / quota::MIB),
        attachment_quota_mib_max: (a.org_quota_mib_max > 0)
            .then(|| i64::try_from(a.org_quota_mib_max).unwrap_or(i64::MAX)),
    })
}

pub async fn get_retention(state: &AppState, c: &Caller, org: Uuid) -> Result<Retention, ApiError> {
    c.org(state, org).await?.require_admin()?;
    let mut conn = state.pool.acquire().await.map_err(db)?;
    retention_view(state, &mut conn, org).await
}

/// Validate a window in days against the bounds and the instance ceiling.
fn check_days(d: i64, ceiling: Option<i64>) -> Result<i64, ApiError> {
    if !(RETENTION_MIN_DAYS..=RETENTION_MAX_DAYS).contains(&d) {
        return Err(ApiError::unprocessable(
            "retention_invalid",
            "The retention window must be between 7 and 3650 days.",
        ));
    }
    if ceiling.is_some_and(|m| d > m) {
        return Err(ApiError::unprocessable(
            "retention_above_instance_maximum",
            "The retention window exceeds the maximum set by the instance.",
        ));
    }
    Ok(d)
}

/// Validate an org attachment quota (MiB; `0` = unlimited) against the instance ceiling.
fn check_quota(v: i64, ceiling_mib: i64) -> Result<i64, ApiError> {
    if !(0..=(1 << 30)).contains(&v) {
        return Err(ApiError::unprocessable(
            "attachment_quota_invalid",
            "The attachment quota must be a number of MiB, 0 for unlimited.",
        ));
    }
    if ceiling_mib > 0 && (v == 0 || v > ceiling_mib) {
        return Err(ApiError::unprocessable(
            "attachment_quota_above_instance_maximum",
            "The attachment quota exceeds the maximum set by the instance.",
        ));
    }
    Ok(v)
}

pub async fn put_retention(
    state: &AppState,
    c: &Caller,
    org: Uuid,
    req: PutRetention,
) -> Result<Retention, ApiError> {
    c.org(state, org).await?.require_admin()?;
    let ceiling = state.config.retention.conversation_days_max.map(i64::from);
    let att_ceiling = state.config.attachments.days_max.map(i64::from);
    let quota_ceiling = i64::try_from(state.config.attachments.org_quota_mib_max).unwrap_or(0);
    let mut tx = state.pool.begin().await.map_err(db)?;
    let old = lifecycle::org_conversations_days(&mut tx, org).await?;
    let (old_quota, old_att_days) = aurora_db::repo::attachments::org_limits(&mut *tx, org).await?;
    let new = match req.conversations_days {
        None => old,
        Some(None) => None,
        Some(Some(d)) => Some(check_days(d, ceiling)?),
    };
    let new_att_days = match req.attachments_days {
        None => old_att_days,
        Some(None) => None,
        Some(Some(d)) => Some(check_days(d, att_ceiling)?),
    };
    let new_quota = match req.attachment_quota_mib {
        None => old_quota,
        Some(None) => None,
        Some(Some(v)) => Some(check_quota(v, quota_ceiling)?),
    };
    let mut changed = serde_json::Map::new();
    if new != old {
        lifecycle::set_org_conversations_days(&mut tx, org, new).await?;
        changed.insert("conversations_days".into(), json!({"from": old, "to": new}));
    }
    if new_att_days != old_att_days {
        lifecycle::set_org_retention_key(&mut tx, org, "attachments_days", new_att_days).await?;
        changed.insert(
            "attachments_days".into(),
            json!({"from": old_att_days, "to": new_att_days}),
        );
    }
    if new_quota != old_quota {
        lifecycle::set_org_retention_key(&mut tx, org, "attachment_quota_mib", new_quota).await?;
        changed.insert(
            "attachment_quota_mib".into(),
            json!({"from": old_quota, "to": new_quota}),
        );
    }
    if !changed.is_empty() {
        // Security-critical: the row commits or rolls back with the change.
        let ev = AuditEvent::new(
            AuditActor::from_caller(c),
            "retention.update",
            Outcome::Success,
        )
        .org(org)
        .target("organization", org)
        .metadata(Value::Object(changed));
        audit::insert_tx(&mut tx, &ev).await?;
    }
    let out = retention_view(state, &mut tx, org).await?;
    tx.commit().await.map_err(db)?;
    Ok(out)
}

/// Daily conversation retention sweep (singleton, docs/05 §Background jobs): per org, delete the
/// conversations idle past the window (the smaller of the org's window and the instance maximum)
/// that have no active turn and no pending approval, `RETENTION_SWEEP_BATCH` per transaction
/// through the same delete path, one `conversation.delete` audit row (actor `system`,
/// `reason = "retention"`) per batch. Returns the number of conversations deleted.
pub async fn sweep_retention(pool: &PgPool, ceiling: Option<u32>) -> Result<usize, ApiError> {
    let orgs = {
        let mut conn = pool.acquire().await.map_err(db)?;
        lifecycle::orgs_with_retention(&mut conn).await?
    };
    let mut total = 0;
    for (org, days) in orgs {
        let days = ceiling.map_or(days, |m| days.min(i64::from(m)));
        if !(RETENTION_MIN_DAYS..=RETENTION_MAX_DAYS).contains(&days) {
            continue;
        }
        let mut left = RETENTION_SWEEP_PER_ORG;
        while left > 0 {
            let n = sweep_batch(pool, org, days, left.min(RETENTION_SWEEP_BATCH)).await?;
            if n == 0 {
                break;
            }
            total += n;
            left -= i64::try_from(n).unwrap_or(left);
        }
    }
    Ok(total)
}

async fn sweep_batch(pool: &PgPool, org: Uuid, days: i64, limit: i64) -> Result<usize, ApiError> {
    let mut tx = pool.begin().await.map_err(db)?;
    let ids = lifecycle::retention_candidates(&mut tx, org, days, limit).await?;
    let mut done: Vec<(Uuid, String)> = Vec::new();
    let mut agg = Deleted::default();
    let mut agents: Vec<Uuid> = Vec::new();
    for id in ids {
        // Re-check under the lock: a turn or approval may have appeared since the scan.
        if !convs::lock(&mut tx, id).await?
            || lifecycle::has_active_turn(&mut tx, id).await?
            || lifecycle::has_pending_approval(&mut tx, id).await?
        {
            continue;
        }
        let Some(row) = convs::get(&mut *tx, id).await? else {
            continue;
        };
        let g = lifecycle::delete_conversation(&mut tx, id).await?;
        agg.turns += g.turns;
        agg.events += g.events;
        agg.attachments += g.attachments;
        agg.session_purged |= g.session_purged;
        agg.blobs.extend(g.blobs);
        if !agents.contains(&row.agent_id) {
            agents.push(row.agent_id);
        }
        done.push((id, row.kind));
    }
    if done.is_empty() {
        tx.commit().await.map_err(db)?;
        return Ok(0);
    }
    let kinds: Vec<(Uuid, &str)> = done.iter().map(|(i, k)| (*i, k.as_str())).collect();
    let mut meta = delete_meta(&kinds, Uuid::nil(), &agg, Some("retention"));
    meta["agent_ids"] = json!(agents);
    meta["retention_days"] = json!(days);
    meta.as_object_mut().map(|o| o.remove("agent_id"));
    let ev = AuditEvent::new(
        AuditActor::system("retention"),
        "conversation.delete",
        Outcome::Success,
    )
    .org(org)
    .target("organization", org)
    .metadata(meta);
    audit::insert_tx(&mut tx, &ev).await?;
    service::notify_work(&mut tx).await?;
    tx.commit().await.map_err(db)?;
    remove_blobs(pool, &agg.blobs).await;
    Ok(done.len())
}

// ------------------------------------------------------------------ export

fn too_large() -> ApiError {
    ApiError::unprocessable(
        "export_too_large",
        "The export exceeds 256 MiB or 200 000 events; page it with after_seq.",
    )
}

pub struct Exported {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    pub filename: String,
}

/// `GET /conversations/{id}/export`: format `aurora.conversation.v1`. Authority: `can_own`, or
/// the direct user of that direct conversation; `can_see_conversation` always applies.
pub async fn export(
    state: &AppState,
    c: &Caller,
    id: Uuid,
    after_seq: Option<i64>,
    zip_files: bool,
) -> Result<Exported, ApiError> {
    let after = after_seq.unwrap_or(0);
    if after < 0 {
        return Err(ApiError::invalid_request(
            "Invalid request.",
            vec![FieldError::new("/after_seq", "must be >= 0")],
        ));
    }
    let (row, auth) = load_conversation_auth(state, c, id).await?;
    if !owner_or_direct_user(c, &row, &auth) {
        return Err(forbidden());
    }
    let mut conn = state.pool.acquire().await.map_err(db)?;
    let (n_events, approx) = lifecycle::export_size(&mut conn, id, after).await?;
    let atts = lifecycle::export_attachments(&mut conn, id).await?;
    let att_bytes: i64 = if zip_files {
        atts.iter()
            .filter(|a| !a.purged)
            .map(|a| a.size_bytes)
            .sum()
    } else {
        0
    };
    let limit = i64::try_from(EXPORT_MAX_BYTES).unwrap_or(i64::MAX);
    if n_events > EXPORT_MAX_EVENTS
        || approx.saturating_add(att_bytes) > limit
        || (zip_files && atts.len() >= zip::MAX_ENTRIES)
    {
        return Err(too_large());
    }
    let mut events: Vec<TranscriptEvent> = Vec::new();
    let mut cursor = after;
    loop {
        let page = transcript::list_after(&mut *conn, id, cursor, 1000).await?;
        let Some(last) = page.last().map(|e| e.seq) else {
            break;
        };
        cursor = last;
        let short = page.len() < 1000;
        events.extend(page.into_iter().map(TranscriptEvent::from));
        if short {
            break;
        }
    }
    let turn_list = lifecycle::export_turns(&mut conn, id).await?;
    let conv = Conversation::from_row(row.clone(), None);
    let att_json: Vec<Value> = atts
        .iter()
        .map(|a| {
            json!({"id": a.id, "filename": a.filename, "size": a.size_bytes, "sha256": a.sha256,
                   "content_type": a.content_type, "direction": a.direction, "purged": a.purged})
        })
        .collect();
    let doc = json!({
        "format": "aurora.conversation.v1",
        "exported_at": chrono::Utc::now(),
        "conversation": conv,
        "turns": turn_list,
        "events": events,
        "attachments": att_json,
    });
    let json_bytes = serde_json::to_vec(&doc).map_err(|_| ApiError::internal("Export failed."))?;
    if json_bytes.len() as u64 > EXPORT_MAX_BYTES {
        return Err(too_large());
    }
    audit::record(
        state,
        AuditEvent::new(
            AuditActor::from_caller(c),
            "conversation.export",
            Outcome::Success,
        )
        .org(auth.org_id)
        .workspace(row.workspace_id)
        .target("conversation", id)
        .metadata(json!({"events": events.len(), "attachments": zip_files,
                             "kind": row.kind, "after_seq": after})),
    );
    let stem = format!("conversation-{id}");
    if !zip_files {
        return Ok(Exported {
            bytes: json_bytes,
            content_type: "application/json",
            filename: format!("{stem}.json"),
        });
    }
    let mut z = ZipWriter::new();
    let mut total = json_bytes.len() as u64;
    let _ = z.add("conversation.json", &json_bytes);
    drop(json_bytes);
    for a in atts.iter().filter(|a| !a.purged) {
        let Some(store) = blob::for_backend(&a.blob_backend, &state.pool) else {
            continue;
        };
        let Some(bytes) = store.get(&a.blob_key).await? else {
            continue;
        };
        total += bytes.len() as u64;
        if total > EXPORT_MAX_BYTES {
            return Err(too_large());
        }
        let name = format!("attachments/{}-{}", a.id, zip::safe_name(&a.filename));
        if !z.add(&name, &bytes) {
            return Err(too_large());
        }
    }
    Ok(Exported {
        bytes: z.finish(),
        content_type: "application/zip",
        filename: format!("{stem}.zip"),
    })
}
