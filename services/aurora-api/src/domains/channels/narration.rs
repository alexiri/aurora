//! Live narration cards (docs/10 §Live narration cards, docs/13 §5.12, P4-NARR, D-279, D-280,
//! D-286, C-846..C-850, SEC-111): one editable card per turn on a chat platform whose connector
//! declares `edits`, listing the turn's tool steps and their status, collapsed to one line when the
//! turn ends.
//!
//! The projector is a view of existing durable events and keeps no state of its own. Everything it
//! needs is read back from the transcript (`tool.started` / `tool.finished` of the turn: the only
//! events it reads, C-850) and from the card's own ops (`narr:<turn_id>` the post,
//! `narr:<turn_id>:<seq>` the updates, `narr:<turn_id>:end` the collapse), so a restarted or
//! second replica resumes the same card and never posts a second one (C-848).
//!
//! **Privacy (SEC-111).** A card never carries `activity`, `thinking.delta` or `text.delta`, nor
//! arguments, results, attachment names or answer text: the only inputs are a tool's `name` and
//! its already sanitized `summary`. The `summary` becomes a step's `detail` in a `direct`
//! conversation only; any other conversation shows names. The check lives here, not in the
//! connector (C-850).
//!
//! **Budget (C-849).** At most one update per turn per `NARR_MIN_INTERVAL`, at most
//! `NARR_SPACE_UPDATES_PER_MINUTE` narration writes per space per minute and at most
//! `NARR_CARDS_PER_SPACE` open cards per space. The counters are read from `channel_ops`
//! (serialized per space by an advisory lock), so the cap is shared by every replica; the shared
//! rate-limit counter primitive (`05`) counts requests, not rows a transaction may still roll
//! back, which is why the op rows themselves are the counter. A pending update that a newer state
//! supersedes is never sent: each update is built from the transcript at the time it is
//! written, and a `rate_limited` answer drops it (`connectors::ops::apply_result`). The collapse
//! bypasses the caps and is retried like any op (D-286).
//!
//! **Failure never fails the turn.** Every entry point runs behind a savepoint in its caller and
//! only logs.

use super::inbox::send::{self, Ctx};
use super::mirror::{ctx_of, link_of};
use aurora_channels::{Content, Narration, NarrationStatus, NarrationStep, NarrationStepStatus};
use aurora_core::{ApiError, limits, sanitize};
use aurora_db::DbError;
use aurora_db::repo::turns::TurnRow;
use aurora_db::repo::{agents, channel_ops, transcript, turns};
use aurora_protocol::connector::{MessageUpdateOp, Sender};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::domains::connectors::ops::{self, NewOp};

/// The card post's dedupe key.
fn post_key(turn: Uuid) -> String {
    format!("narr:{turn}")
}

fn end_key(turn: Uuid) -> String {
    format!("narr:{turn}:end")
}

fn update_key(turn: Uuid, seq: i64) -> String {
    format!("narr:{turn}:{seq}")
}

/// The turn's card post, when one was queued.
async fn card_op(
    conn: &mut PgConnection,
    turn: Uuid,
) -> Result<Option<channel_ops::OpRow>, ApiError> {
    Ok(channel_ops::by_dedupe_key(&mut *conn, &post_key(turn)).await?)
}

fn space_of_post(op: &channel_ops::OpRow) -> &str {
    op.payload
        .pointer("/target/space_ref")
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn db(e: sqlx::Error) -> ApiError {
    DbError::from(e).into()
}

/// Serializes the cap checks of one space across replicas, for the rest of the transaction.
async fn lock_space(
    conn: &mut PgConnection,
    integration: Uuid,
    space: &str,
) -> Result<(), ApiError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("narr-space:{integration}:{space}"))
        .execute(&mut *conn)
        .await
        .map_err(db)?;
    Ok(())
}

/// Narration writes (posts, updates, collapses) queued for the space in the last minute.
async fn writes_last_minute(
    conn: &mut PgConnection,
    integration: Uuid,
    space: &str,
) -> Result<i64, ApiError> {
    sqlx::query_scalar(
        "SELECT count(*) FROM channel_ops
          WHERE integration_id = $1 AND dedupe_key LIKE 'narr:%'
            AND created_at > now() - interval '1 minute'
            AND COALESCE(payload->>'space_ref', payload #>> '{target,space_ref}') = $2",
    )
    .bind(integration)
    .bind(space)
    .fetch_one(&mut *conn)
    .await
    .map_err(db)
}

/// Cards of the space posted in the last day and not collapsed yet.
async fn open_cards(
    conn: &mut PgConnection,
    integration: Uuid,
    space: &str,
) -> Result<i64, ApiError> {
    sqlx::query_scalar(
        "SELECT count(*) FROM channel_ops p
          WHERE p.integration_id = $1 AND p.dedupe_key ~ '^narr:[0-9a-f-]{36}$'
            AND p.payload #>> '{target,space_ref}' = $2
            AND p.created_at > now() - interval '1 day'
            AND NOT EXISTS (SELECT 1 FROM channel_ops e WHERE e.dedupe_key = p.dedupe_key || ':end')",
    )
    .bind(integration)
    .bind(space)
    .fetch_one(&mut *conn)
    .await
    .map_err(db)
}

/// What a card shows of a turn, built from the durable tool events only.
struct View {
    steps: Vec<NarrationStep>,
    total: usize,
    failed: usize,
    last_seq: i64,
}

fn clip(s: &str) -> String {
    sanitize::one_line(s, limits::NARR_STEP_CHARS)
}

/// Builds the view. `detail` is only filled for a `direct` conversation (C-850).
async fn view_of(conn: &mut PgConnection, t: &TurnRow, direct: bool) -> Result<View, ApiError> {
    struct Step {
        call_id: String,
        step: NarrationStep,
    }
    let mut steps: Vec<Step> = Vec::new();
    let mut last_seq = 0;
    for ev in transcript::tool_events_of_turn(&mut *conn, t.conversation_id, t.id).await? {
        last_seq = last_seq.max(ev.seq);
        let call_id = ev
            .payload
            .get("call_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        match ev.ty.as_str() {
            "tool.started" => {
                let name = clip(ev.payload.get("name").and_then(Value::as_str).unwrap_or(""));
                let detail = direct
                    .then(|| ev.payload.get("summary").and_then(Value::as_str))
                    .flatten()
                    .map(clip)
                    .filter(|d| !d.is_empty());
                steps.push(Step {
                    call_id: call_id.to_owned(),
                    step: NarrationStep {
                        name: if name.is_empty() {
                            "tool".to_owned()
                        } else {
                            name
                        },
                        detail,
                        status: NarrationStepStatus::Running,
                    },
                });
            }
            "tool.finished" => {
                let ok = ev
                    .payload
                    .get("ok")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if let Some(s) = steps
                    .iter_mut()
                    .rev()
                    .find(|s| s.call_id == call_id && s.step.status == NarrationStepStatus::Running)
                {
                    s.step.status = if ok {
                        NarrationStepStatus::Ok
                    } else {
                        NarrationStepStatus::Failed
                    };
                }
            }
            _ => {}
        }
    }
    let total = steps.len();
    let failed = steps
        .iter()
        .filter(|s| s.step.status == NarrationStepStatus::Failed)
        .count();
    let skip = total.saturating_sub(limits::NARR_MAX_STEPS);
    Ok(View {
        steps: steps.into_iter().skip(skip).map(|s| s.step).collect(),
        total,
        failed,
        last_seq,
    })
}

/// Whether the turn's conversation with the agent is a `direct` one (a DM with the bot, linked
/// as `direct`): the only place a step's summary may be shown.
fn is_direct(link_kind: &str) -> bool {
    link_kind == "direct"
}

struct Target {
    ctx: Ctx,
    sender: Sender,
    direct: bool,
}

/// The card's destination for a turn, or `None` when the turn gets none (no thread link, an
/// inactive integration, no `edits`, the agent's setting off, a turn that is not a plain turn).
async fn target_of(conn: &mut PgConnection, t: &TurnRow) -> Result<Option<Target>, ApiError> {
    if t.role != "turn" {
        return Ok(None);
    }
    // Only a turn that came in through a thread link: doorless turns (schedule, webhook, watch,
    // peer, system) have no message to answer, hence no live card.
    if t.door.get("kind").and_then(Value::as_str) != Some("thread") {
        return Ok(None);
    }
    let Some(agent) = agents::get(&mut *conn, t.agent_id).await? else {
        return Ok(None);
    };
    if !agent.narration_cards {
        return Ok(None);
    }
    let Some(link) = link_of(conn, t, false).await? else {
        return Ok(None);
    };
    let Some(ctx) = ctx_of(conn, link.integration_id).await? else {
        return Ok(None);
    };
    if !ctx.caps.has(&aurora_channels::Capability::Edits) {
        return Ok(None);
    }
    let Ok(sender) = serde_json::from_value::<Sender>(link.sender.clone()) else {
        return Ok(None);
    };
    Ok(Some(Target {
        ctx,
        sender,
        direct: is_direct(&link.kind),
    }))
}

fn running_content(turn: Uuid, v: &View) -> Content {
    Content::narration(Narration {
        turn_id: turn.to_string(),
        status: NarrationStatus::Running,
        steps: v.steps.clone(),
        earlier: u32::try_from(v.total - v.steps.len()).unwrap_or(u32::MAX),
        collapsed: false,
        step_count: None,
        failed_count: None,
        elapsed_ms: None,
    })
}

/// Hook from `mirror::on_turn_started`: posts the card in the turn's thread, when the turn gets
/// one (the setting is on, the connector declares `edits`, and the space has room for another
/// card and one more write).
pub async fn start(conn: &mut PgConnection, t: &TurnRow) -> Result<(), ApiError> {
    let Some(tg) = target_of(conn, t).await? else {
        return Ok(());
    };
    let Some(link) = link_of(conn, t, false).await? else {
        return Ok(());
    };
    if card_op(conn, t.id).await?.is_some() {
        return Ok(());
    }
    let integ = tg.ctx.id();
    lock_space(conn, integ, &link.space_ref).await?;
    if open_cards(conn, integ, &link.space_ref).await? >= limits::NARR_CARDS_PER_SPACE as i64
        || writes_last_minute(conn, integ, &link.space_ref).await?
            >= limits::NARR_SPACE_UPDATES_PER_MINUTE as i64
    {
        tracing::info!(
            target: "aurora_api::channels::narration",
            turn_id = %t.id, integration_id = %integ,
            "narration card skipped: the space is at its card or write cap"
        );
        return Ok(());
    }
    let view = view_of(conn, t, tg.direct).await?;
    send::post(
        conn,
        &tg.ctx,
        send::Post {
            sender: tg.sender.clone(),
            target: send::space_target(&link.space_ref, &link.thread_ref),
            content: running_content(t.id, &view),
            files: Vec::new(),
            ephemeral_to: None,
            dedupe: post_key(t.id),
        },
    )
    .await?;
    Ok(())
}

/// The card's message, once its post was delivered: `(space_ref, message_ref, sender)`.
fn delivered(post: &channel_ops::OpRow) -> Option<(String, String, Sender)> {
    if post.status != "done" {
        return None;
    }
    let message_ref = post
        .result
        .as_ref()?
        .get("message_ref")?
        .as_str()?
        .to_owned();
    let sender = serde_json::from_value(post.payload.get("sender")?.clone()).ok()?;
    Some((space_of_post(post).to_owned(), message_ref, sender))
}

/// When the turn's card was last written (post, update or collapse), and the highest event seq
/// an earlier update already shows.
async fn last_write(
    conn: &mut PgConnection,
    turn: Uuid,
) -> Result<(Option<DateTime<Utc>>, i64), ApiError> {
    let rows: Vec<(String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT dedupe_key, created_at FROM channel_ops
          WHERE dedupe_key = $1 OR dedupe_key LIKE $2",
    )
    .bind(post_key(turn))
    .bind(format!("narr:{turn}:%"))
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    let at = rows.iter().map(|(_, c)| *c).max();
    let seq = rows
        .iter()
        .filter_map(|(k, _)| k.rsplit(':').next()?.parse::<i64>().ok())
        .max()
        .unwrap_or(0);
    Ok((at, seq))
}

/// Hook from the turn-event ingest, after a batch of events: queues one `message.update` when the
/// step list changed, the turn's interval has passed and the space has budget. Never a late
/// update: the content is built from the transcript now, so a coalesced burst is one write.
pub async fn refresh(conn: &mut PgConnection, t: &TurnRow) -> Result<(), ApiError> {
    let Some(post) = card_op(conn, t.id).await? else {
        return Ok(());
    };
    let Some((space, message_ref, sender)) = delivered(&post) else {
        return Ok(());
    };
    let (last_at, shown_seq) = last_write(conn, t.id).await?;
    if channel_ops::by_dedupe_key(&mut *conn, &end_key(t.id))
        .await?
        .is_some()
    {
        return Ok(());
    }
    if last_at.is_some_and(|at| {
        Utc::now() - at < chrono::Duration::from_std(limits::NARR_MIN_INTERVAL).unwrap_or_default()
    }) {
        return Ok(());
    }
    let Some(integ) = post.integration_id else {
        return Ok(());
    };
    let direct = match link_of(conn, t, false).await? {
        Some(l) => is_direct(&l.kind),
        None => false,
    };
    let view = view_of(conn, t, direct).await?;
    if view.last_seq <= shown_seq {
        return Ok(());
    }
    lock_space(conn, integ, &space).await?;
    if writes_last_minute(conn, integ, &space).await?
        >= limits::NARR_SPACE_UPDATES_PER_MINUTE as i64
    {
        return Ok(());
    }
    enqueue_update(
        conn,
        integ,
        &post,
        &space,
        &message_ref,
        &sender,
        running_content(t.id, &view),
        update_key(t.id, view.last_seq),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_update(
    conn: &mut PgConnection,
    integration: Uuid,
    post: &channel_ops::OpRow,
    space: &str,
    message_ref: &str,
    sender: &Sender,
    content: Content,
    dedupe: String,
) -> Result<(), ApiError> {
    let payload = serde_json::to_value(MessageUpdateOp {
        space_ref: space.to_owned(),
        message_ref: message_ref.to_owned(),
        sender: sender.clone(),
        content: serde_json::to_value(&content).map_err(|e| ApiError::internal(e.to_string()))?,
    })
    .map_err(|e| ApiError::internal(e.to_string()))?;
    ops::enqueue(
        conn,
        NewOp {
            kind: "message.update".to_owned(),
            connector_id: None,
            integration_id: Some(integration),
            presence_id: post.presence_id,
            payload,
            dedupe_key: Some(dedupe),
        },
    )
    .await?;
    Ok(())
}

/// Hook from `mirror::on_turn_finished` (and the sweep): the collapse (D-286). A last
/// `message.update` replaces the step list with the outcome, the step count and the elapsed time;
/// it bypasses the caps and is never dropped. When the post is not delivered yet it waits for the
/// sweep.
pub async fn finish(conn: &mut PgConnection, t: &TurnRow) -> Result<(), ApiError> {
    let Some(post) = card_op(conn, t.id).await? else {
        return Ok(());
    };
    if channel_ops::by_dedupe_key(&mut *conn, &end_key(t.id))
        .await?
        .is_some()
    {
        return Ok(());
    }
    let Some((space, message_ref, sender)) = delivered(&post) else {
        return Ok(());
    };
    let Some(integ) = post.integration_id else {
        return Ok(());
    };
    let direct = match link_of(conn, t, false).await? {
        Some(l) => is_direct(&l.kind),
        None => false,
    };
    let view = view_of(conn, t, direct).await?;
    let status = match t.status.as_str() {
        "done" => NarrationStatus::Done,
        "failed" => NarrationStatus::Failed,
        _ => NarrationStatus::Stopped,
    };
    let began = t.started_at.unwrap_or(t.created_at);
    let ended = t.finished_at.unwrap_or_else(Utc::now);
    let elapsed_ms = u64::try_from((ended - began).num_milliseconds().max(0)).unwrap_or(0);
    let content = Content::narration(Narration {
        turn_id: t.id.to_string(),
        status,
        steps: Vec::new(),
        earlier: 0,
        collapsed: true,
        step_count: Some(u32::try_from(view.total).unwrap_or(u32::MAX)),
        failed_count: Some(u32::try_from(view.failed).unwrap_or(u32::MAX)),
        elapsed_ms: Some(elapsed_ms),
    });
    lock_space(conn, integ, &space).await?;
    enqueue_update(
        conn,
        integ,
        &post,
        &space,
        &message_ref,
        &sender,
        content,
        end_key(t.id),
    )
    .await
}

/// The channel-op sweep: collapses the cards of turns that ended while their post was still in
/// flight, and flushes the trailing update of running turns whose last burst fell inside the
/// interval. Returns how many writes it queued.
pub async fn sweep(pool: &PgPool) -> Result<usize, ApiError> {
    let mut conn = pool.acquire().await.map_err(db)?;
    let cards: Vec<(String,)> = sqlx::query_as(
        "SELECT substring(p.dedupe_key FROM 6) FROM channel_ops p
          WHERE p.dedupe_key ~ '^narr:[0-9a-f-]{36}$' AND p.status = 'done'
            AND p.created_at > now() - interval '1 day'
            AND NOT EXISTS (SELECT 1 FROM channel_ops e WHERE e.dedupe_key = p.dedupe_key || ':end')
          ORDER BY p.created_at LIMIT 100",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    let mut queued = 0;
    for (id,) in cards {
        let Ok(id) = id.parse::<Uuid>() else { continue };
        let mut tx = sqlx::Connection::begin(&mut *conn).await.map_err(db)?;
        let Some(t) = turns::get(&mut *tx, id).await? else {
            continue;
        };
        let res = match t.status.as_str() {
            "done" | "failed" | "aborted" => finish(&mut tx, &t).await,
            "running" => refresh(&mut tx, &t).await,
            _ => Ok(()),
        };
        match res {
            Ok(()) => {
                tx.commit().await.map_err(db)?;
                queued += 1;
            }
            Err(e) => {
                tracing::warn!(turn = %id, error = %e.message(), "narration sweep failed");
                tx.rollback().await.map_err(db)?;
            }
        }
    }
    Ok(queued)
}
