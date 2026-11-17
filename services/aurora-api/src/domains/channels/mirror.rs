//! The mirror (docs/13 §5.2): what a finished turn posts back to the platform. Runs on the
//! turn-finished lifecycle hook, inside the finishing transaction, behind a savepoint so a
//! mirror problem can never fail or roll back the turn's own completion.
//!
//! * a turn with door `thread` posts its durable answer text once, complete (`dedupe_key`
//!   `answer:<turn_id>`, further chunks `answer:<turn_id>:<n>`) to that link, then `send_file`
//!   files, then the receipt is removed;
//! * doorless turns (schedule, webhook, watch, peer, system) go to the conversation's most
//!   recently used thread link; web and API turns are never mirrored;
//! * a failed turn posts a short failure line with a dashboard link;
//! * a shared bot starts its first post in a thread, and every proactive post, with
//!   "Agent · via Bot".
//!
//! The answer is the agent's own message body: it goes out through `Content::agent_markdown`
//! (D-226/D-227): its Markdown is kept byte for byte except the four security exceptions (no
//! inline images, link-scheme allowlist, raw HTML allowlist, typed neutral tokens inert), and a
//! standalone `@here` is the only broadcast. Long answers are split at Markdown block
//! boundaries (`sanitize::markdown_chunks`). Everything else the mirror posts (failure lines,
//! attribution, file names) is platform voice.

use super::inbox::send::{self, Ctx};
use aurora_channels::Content;
use aurora_core::{ApiError, sanitize};
use aurora_db::repo::turns::TurnRow;
use aurora_db::repo::{
    agents, attachments, channel_integrations as integrations, channel_ops, connectors,
    thread_links, transcript,
};
use aurora_protocol::connector::{OutFile, Sender, Target};
use serde_json::Value;
use sqlx::{Connection, PgConnection};
use std::sync::OnceLock;

/// Raw characters per chunk. `markdown_safe` and the connector's conversions (footnote notes,
/// alert prefixes, HTML → Markdown, a reopened fence) can lengthen a piece a little, so a chunk
/// is also capped at half the integration's `message_max_chars`: that margin, not a measured
/// length, keeps the converted post under the platform limit.
const CHUNK_MAX: usize = 3_500;

static EXTERNAL_URL: OnceLock<String> = OnceLock::new();

/// The dashboard origin used in links (the lifecycle hook has no `AppState`); set once at
/// startup and by the first inbox event. Unset, links are origin-relative.
pub fn init(external_url: &str) {
    let _ = EXTERNAL_URL.set(external_url.to_owned());
}

fn external_url() -> &'static str {
    EXTERNAL_URL.get().map_or("", String::as_str)
}

/// Hook from `hosted::lifecycle::finish`.
pub async fn on_turn_finished(conn: &mut PgConnection, t: &TurnRow) -> Result<(), ApiError> {
    let external_url = external_url();
    if t.role != "turn" || !matches!(t.status.as_str(), "done" | "failed" | "aborted") {
        return Ok(());
    }
    let door = t.door.get("kind").and_then(Value::as_str).unwrap_or("");
    if matches!(door, "web" | "api") {
        return Ok(());
    }
    let mut sp = conn.begin().await.map_err(aurora_db::DbError::from)?;
    match mirror(&mut sp, external_url, t, door).await {
        Ok(()) => sp.commit().await.map_err(aurora_db::DbError::from)?,
        Err(e) => {
            tracing::warn!(turn = %t.id, error = %e.message(), "mirror failed; turn completion kept");
            sp.rollback().await.map_err(aurora_db::DbError::from)?;
        }
    }
    Ok(())
}

async fn mirror(
    conn: &mut PgConnection,
    external_url: &str,
    t: &TurnRow,
    door: &str,
) -> Result<(), ApiError> {
    let proactive = door != "thread";
    let link = if proactive {
        thread_links::latest_for_conversation(&mut *conn, t.conversation_id).await?
    } else {
        match t
            .door
            .get("thread_link_id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok())
        {
            Some(id) => thread_links::get(&mut *conn, id).await?,
            None => None,
        }
    };
    let Some(link) = link else {
        return Ok(());
    };
    let Some(integ) = integrations::get(&mut *conn, link.integration_id).await? else {
        return Ok(());
    };
    if integ.status != "active" {
        return Ok(());
    }
    let manifest = connectors::get(&mut *conn, &integ.connector_id)
        .await?
        .and_then(|c| c.manifest);
    let ctx = Ctx::new(integ, manifest);
    let Ok(sender) = serde_json::from_value::<Sender>(link.sender.clone()) else {
        return Ok(());
    };
    let Some(agent) = agents::get(&mut *conn, t.agent_id).await? else {
        return Ok(());
    };
    let target = send::space_target(&link.space_ref, &link.thread_ref);

    // The working indicator goes first, so the platform never shows "typing" under the answer.
    clear_working(conn, t.id).await?;

    // (FIX-158, D-319) The receipt stays until this thread link gets the answer or the failure
    // line; aborted turns only clear their own.
    let mut posted = false;
    if t.status != "aborted" {
        let shared = matches!(sender, Sender::Shared(_));
        let first =
            proactive || thread_links::finished_turns_via(&mut *conn, link.id, t.id).await? == 0;
        // FIX-128 (docs/13 §5.2): no "Agent · via Bot" line in a 1:1 DM with the bot, where one
        // agent answers and the line only repeats what the conversation already says.
        let attribution = (shared && first && link.kind != "direct").then(|| {
            format!(
                "{} · via {}",
                sanitize::label(&agent.name),
                sanitize::label(&ctx.shared_display_name())
            )
        });
        if t.status == "failed" {
            let code = sanitize::one_line(t.error_code.as_deref().unwrap_or("error"), 64);
            let url = super::cards::workspace_page_url_for(
                &mut *conn,
                external_url,
                agent.workspace_id,
                &format!("agents/{}", agent.handle),
            )
            .await?;
            let mut text = format!(
                "Something went wrong while working on that (`{code}`). See the conversation in the dashboard: {url}"
            );
            if let Some(a) = &attribution {
                text = format!("{a}\n\n{text}");
            }
            send::post(
                conn,
                &ctx,
                send::Post {
                    sender: sender.clone(),
                    target: target.clone(),
                    content: Content::platform(&text),
                    files: Vec::new(),
                    ephemeral_to: None,
                    dedupe: format!("answer:{}", t.id),
                },
            )
            .await?;
            posted = true;
        } else {
            posted = post_answer(
                conn,
                &ctx,
                t,
                &sender,
                &target,
                attribution.as_deref(),
                external_url,
            )
            .await?;
        }
    }
    let own = t.source_event_id.as_deref().filter(|_| !proactive);
    if posted {
        let reason = if t.status == "failed" {
            "failed"
        } else {
            "answer_posted"
        };
        if let Some(event_id) = own {
            clear_receipt(conn, &ctx, &sender, event_id, Some(t.id), reason).await?;
        }
        // Receipts of earlier messages of this thread whose own turn ended silent while work was
        // pending (a watch, an approval) are answered by this post too.
        for r in channel_ops::open_receipts(&mut *conn, Some(link.id), None, 50).await? {
            clear_receipt(conn, &ctx, &sender, &r.event_id, Some(t.id), reason).await?;
        }
    } else if let Some(event_id) = own {
        if t.status == "aborted" {
            clear_receipt(conn, &ctx, &sender, event_id, Some(t.id), "aborted").await?;
        } else if channel_ops::follow_on_pending(&mut *conn, t.conversation_id, Some(t.id)).await? {
            tracing::info!(
                target: "aurora_api::channels::receipt",
                event_id,
                turn_id = %t.id,
                integration_id = %ctx.id(),
                reason = "follow_on_pending",
                "receipt kept open"
            );
        } else {
            clear_receipt(conn, &ctx, &sender, event_id, Some(t.id), "no_followon").await?;
        }
    }
    thread_links::bump_revision(&mut *conn, link.id).await?;
    Ok(())
}

/// Hook from the supervisor's `turn.started` report (FIX-158 Part C, D-319): a running turn of a
/// chat thread shows the platform's working status ("is typing…" where the platform has it) until the turn
/// finishes. Only on integrations that declare `native_status`; web and API turns have none.
pub async fn on_turn_started(conn: &mut PgConnection, t: &TurnRow) -> Result<(), ApiError> {
    if t.role != "turn" {
        return Ok(());
    }
    let door = t.door.get("kind").and_then(Value::as_str).unwrap_or("");
    if matches!(door, "web" | "api") {
        return Ok(());
    }
    let mut sp = conn.begin().await.map_err(aurora_db::DbError::from)?;
    match set_working(&mut sp, t, door != "thread").await {
        Ok(()) => sp.commit().await.map_err(aurora_db::DbError::from)?,
        Err(e) => {
            tracing::warn!(turn = %t.id, error = %e.message(), "working status failed; turn start kept");
            sp.rollback().await.map_err(aurora_db::DbError::from)?;
        }
    }
    Ok(())
}

/// The thread link a turn's mirror goes to: its own (door `thread`), else the conversation's
/// latest.
async fn link_of(
    conn: &mut PgConnection,
    t: &TurnRow,
    proactive: bool,
) -> Result<Option<thread_links::ThreadLinkRow>, ApiError> {
    if proactive {
        return Ok(thread_links::latest_for_conversation(&mut *conn, t.conversation_id).await?);
    }
    match t
        .door
        .get("thread_link_id")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
    {
        Some(id) => Ok(thread_links::get(&mut *conn, id).await?),
        None => Ok(None),
    }
}

/// `integration` with its manifest, when active.
async fn ctx_of(conn: &mut PgConnection, integration: uuid::Uuid) -> Result<Option<Ctx>, ApiError> {
    let Some(integ) = integrations::get(&mut *conn, integration).await? else {
        return Ok(None);
    };
    if integ.status != "active" {
        return Ok(None);
    }
    let manifest = connectors::get(&mut *conn, &integ.connector_id)
        .await?
        .and_then(|c| c.manifest);
    Ok(Some(Ctx::new(integ, manifest)))
}

async fn set_working(
    conn: &mut PgConnection,
    t: &TurnRow,
    proactive: bool,
) -> Result<(), ApiError> {
    let Some(link) = link_of(conn, t, proactive).await? else {
        return Ok(());
    };
    let Some(ctx) = ctx_of(conn, link.integration_id).await? else {
        return Ok(());
    };
    if !ctx.caps.has(&aurora_channels::Capability::NativeStatus) {
        return Ok(());
    }
    let Ok(sender) = serde_json::from_value::<Sender>(link.sender.clone()) else {
        return Ok(());
    };
    send::status(
        conn,
        &ctx,
        "status.set",
        &sender,
        &link.space_ref,
        &link.thread_ref,
        format!("working:{}", t.id),
    )
    .await?;
    Ok(())
}

/// `status.clear` for the turn's working indicator, to where it was set.
async fn clear_working(conn: &mut PgConnection, turn: uuid::Uuid) -> Result<(), ApiError> {
    let Some(op) = channel_ops::by_dedupe_key(&mut *conn, &format!("working:{turn}")).await? else {
        return Ok(());
    };
    let Some(integration) = op.integration_id else {
        return Ok(());
    };
    let Some(ctx) = ctx_of(conn, integration).await? else {
        return Ok(());
    };
    let Some(sender) = op
        .payload
        .get("sender")
        .and_then(|s| serde_json::from_value::<Sender>(s.clone()).ok())
    else {
        return Ok(());
    };
    let space = op
        .payload
        .get("space_ref")
        .and_then(Value::as_str)
        .unwrap_or("");
    let thread = op
        .payload
        .get("thread_ref")
        .and_then(Value::as_str)
        .unwrap_or("");
    send::status(
        conn,
        &ctx,
        "status.clear",
        &sender,
        space,
        thread,
        format!("working-clear:{turn}"),
    )
    .await?;
    Ok(())
}

/// The channel-op sweep (FIX-158 Part B): clears open receipts whose conversation has had no
/// active turn and no pending follow-on work for `RECEIPT_ORPHAN_SECS`, so a receipt never
/// sticks. Returns how many were cleared.
pub async fn sweep_orphan_receipts(pool: &sqlx::PgPool) -> Result<usize, ApiError> {
    let secs = aurora_core::limits::RECEIPT_ORPHAN_SECS as f64;
    let mut conn = pool.acquire().await.map_err(aurora_db::DbError::from)?;
    let open = channel_ops::open_receipts(&mut *conn, None, Some(secs), 100).await?;
    let mut cleared = 0;
    for r in open {
        if !channel_ops::conversation_quiet(&mut *conn, r.conversation_id, secs).await?
            || channel_ops::follow_on_pending(&mut *conn, r.conversation_id, None).await?
        {
            continue;
        }
        let mut sp = conn.begin().await.map_err(aurora_db::DbError::from)?;
        let done: Result<(), ApiError> = async {
            let Some(link) = thread_links::get(&mut *sp, r.thread_link_id).await? else {
                return Ok(());
            };
            let Some(ctx) = ctx_of(&mut sp, link.integration_id).await? else {
                return Ok(());
            };
            let Ok(sender) = serde_json::from_value::<Sender>(link.sender.clone()) else {
                return Ok(());
            };
            clear_receipt(&mut sp, &ctx, &sender, &r.event_id, None, "orphan").await
        }
        .await;
        match done {
            Ok(()) => {
                sp.commit().await.map_err(aurora_db::DbError::from)?;
                cleared += 1;
            }
            Err(e) => {
                tracing::warn!(event_id = %r.event_id, error = %e.message(), "orphan receipt not cleared");
                sp.rollback().await.map_err(aurora_db::DbError::from)?;
            }
        }
    }
    Ok(cleared)
}

/// Posts the turn's answer chunks and files; `true` when anything was posted.
async fn post_answer(
    conn: &mut PgConnection,
    ctx: &Ctx,
    t: &TurnRow,
    sender: &Sender,
    target: &Target,
    attribution: Option<&str>,
    external_url: &str,
) -> Result<bool, ApiError> {
    let texts = transcript::answer_texts(&mut *conn, t.conversation_id, t.id).await?;
    let answer = texts.join("\n\n");
    let chunk_max = CHUNK_MAX.min((ctx.message_max_chars() / 2).max(200));
    let mut posted = false;
    for (n, chunk) in sanitize::markdown_chunks(&answer, chunk_max)
        .into_iter()
        .enumerate()
    {
        posted = true;
        let mut text = Content::agent_markdown(&chunk).text.unwrap_or_default();
        if n == 0
            && let Some(a) = attribution
        {
            text = format!(
                "{}\n\n{text}",
                Content::platform(a).text.unwrap_or_default()
            );
        }
        send::post(
            conn,
            ctx,
            send::Post {
                sender: sender.clone(),
                target: target.clone(),
                content: Content {
                    text: Some(text),
                    card: None,
                },
                files: Vec::new(),
                ephemeral_to: None,
                dedupe: if n == 0 {
                    format!("answer:{}", t.id)
                } else {
                    format!("answer:{}:{n}", t.id)
                },
            },
        )
        .await?;
    }
    // Files from `send_file` follow the answer.
    let files = attachments::out_for_turn(&mut *conn, t.id).await?;
    if files.is_empty() {
        return Ok(posted);
    }
    let (text, out) = if ctx.caps.has(&aurora_channels::Capability::Files) {
        let base = external_url.trim_end_matches('/');
        let list: Vec<OutFile> = files
            .iter()
            .map(|f| OutFile {
                attachment_id: f.id,
                name: sanitize::label(&f.filename),
                download_url: format!("{base}/v1/connector/files/{}", f.id),
            })
            .collect();
        let names: Vec<String> = list.iter().map(|f| f.name.clone()).collect();
        (names.join(", "), list)
    } else {
        (
            format!(
                "{} file(s) were not sent: this platform doesn't support files here.",
                files.len()
            ),
            Vec::new(),
        )
    };
    send::post(
        conn,
        ctx,
        send::Post {
            sender: sender.clone(),
            target: target.clone(),
            content: Content::platform(&text),
            files: out,
            ephemeral_to: None,
            dedupe: format!("answer-files:{}", t.id),
        },
    )
    .await?;
    Ok(true)
}

/// Removes whatever receipt the inbox left: the reaction, the native status, or the
/// "Working…" placeholder (dropped while still queued, deleted once posted). A reaction whose add
/// is still queued is dropped with no remove; one whose add is in flight is removed only after
/// the add finishes (the claim waits). One `info` line per receipt cleared, with its reason.
async fn clear_receipt(
    conn: &mut PgConnection,
    ctx: &Ctx,
    sender: &Sender,
    event_id: &str,
    turn_id: Option<uuid::Uuid>,
    reason: &str,
) -> Result<(), ApiError> {
    let log = |what: &str| {
        tracing::info!(
            target: "aurora_api::channels::receipt",
            event_id,
            turn_id = ?turn_id,
            integration_id = %ctx.id(),
            reason,
            "receipt {what}"
        );
    };
    if let Some(op) = channel_ops::by_dedupe_key(&mut *conn, &format!("receipt:{event_id}")).await?
    {
        let space = op
            .payload
            .get("space_ref")
            .and_then(Value::as_str)
            .unwrap_or("");
        let key = format!("receipt-clear:{event_id}");
        match op.kind.as_str() {
            "reaction.add" if op.status == "pending" => {
                channel_ops::drop_pending(&mut *conn, op.id).await?;
                log("dropped before it was added");
            }
            "reaction.add" if matches!(op.status.as_str(), "claimed" | "done") => {
                let msg = op
                    .payload
                    .get("message_ref")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                send::reaction(conn, ctx, "reaction.remove", sender, space, msg, key).await?;
                log("cleared");
            }
            "status.set" => {
                let thread = op
                    .payload
                    .get("thread_ref")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                send::status(conn, ctx, "status.clear", sender, space, thread, key).await?;
                log("cleared");
            }
            _ => {}
        }
    }
    if let Some(op) =
        channel_ops::by_dedupe_key(&mut *conn, &format!("placeholder:{event_id}")).await?
    {
        match op.status.as_str() {
            "pending" => {
                channel_ops::drop_pending(&mut *conn, op.id).await?;
                log("placeholder dropped before it was posted");
            }
            "done" => {
                let space = op
                    .payload
                    .pointer("/target/space_ref")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let msg = op
                    .result
                    .as_ref()
                    .and_then(|r| r.get("message_ref"))
                    .and_then(Value::as_str);
                if let Some(msg) = msg {
                    send::delete(
                        conn,
                        ctx,
                        sender,
                        space,
                        msg,
                        format!("placeholder-clear:{event_id}"),
                    )
                    .await?;
                    log("placeholder deleted");
                }
            }
            _ => {}
        }
    }
    Ok(())
}
