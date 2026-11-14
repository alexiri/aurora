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
//! Model text is untrusted: it goes out through `Content::untrusted`, so it can't carry
//! mentions or broadcasts (formatting is escaped as a consequence).

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

/// Raw characters per chunk: `markdown_as_data` can double a text, and the platform limit
/// (`message_max_chars`) bounds the escaped result.
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

    // Receipt removal comes with the answer; aborted turns only clear it.
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
        } else {
            post_answer(
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
    if let Some(event_id) = t.source_event_id.as_deref().filter(|_| !proactive) {
        clear_receipt(conn, &ctx, &sender, event_id).await?;
    }
    thread_links::bump_revision(&mut *conn, link.id).await?;
    Ok(())
}

async fn post_answer(
    conn: &mut PgConnection,
    ctx: &Ctx,
    t: &TurnRow,
    sender: &Sender,
    target: &Target,
    attribution: Option<&str>,
    external_url: &str,
) -> Result<(), ApiError> {
    let texts = transcript::answer_texts(&mut *conn, t.conversation_id, t.id).await?;
    let answer = texts.join("\n\n");
    let chunk_max = CHUNK_MAX.min((ctx.message_max_chars() / 2).max(200));
    for (n, chunk) in chunks(&answer, chunk_max).into_iter().enumerate() {
        let mut text = Content::untrusted(&chunk).text.unwrap_or_default();
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
        return Ok(());
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
    Ok(())
}

/// Removes whatever receipt the inbox left: the reaction, the native status, or the
/// "Working…" placeholder (dropped while still queued, deleted once posted).
async fn clear_receipt(
    conn: &mut PgConnection,
    ctx: &Ctx,
    sender: &Sender,
    event_id: &str,
) -> Result<(), ApiError> {
    if let Some(op) = channel_ops::by_dedupe_key(&mut *conn, &format!("receipt:{event_id}")).await?
    {
        let space = op
            .payload
            .get("space_ref")
            .and_then(Value::as_str)
            .unwrap_or("");
        let key = format!("receipt-clear:{event_id}");
        match op.kind.as_str() {
            "reaction.add" => {
                let msg = op
                    .payload
                    .get("message_ref")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                send::reaction(conn, ctx, "reaction.remove", sender, space, msg, key).await?;
            }
            "status.set" => {
                let thread = op
                    .payload
                    .get("thread_ref")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                send::status(conn, ctx, "status.clear", sender, space, thread, key).await?;
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
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Splits at paragraph/line/space boundaries into pieces of at most `max` characters.
fn chunks(s: &str, max: usize) -> Vec<String> {
    let s = s.trim();
    if s.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut rest = s;
    while rest.chars().count() > max {
        let cut_at = rest.char_indices().nth(max).map_or(rest.len(), |(i, _)| i);
        let head = &rest[..cut_at];
        let split = head
            .rfind("\n\n")
            .or_else(|| head.rfind('\n'))
            .or_else(|| head.rfind(' '))
            .filter(|i| *i > cut_at / 2)
            .unwrap_or(cut_at);
        out.push(rest[..split].trim().to_owned());
        rest = rest[split..].trim_start();
    }
    if !rest.is_empty() {
        out.push(rest.to_owned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::chunks;

    #[test]
    fn chunks_split_on_boundaries() {
        assert!(chunks("  ", 10).is_empty());
        assert_eq!(chunks("short", 10), vec!["short"]);
        let c = chunks("aaaa bbbb cccc dddd", 9);
        assert!(c.iter().all(|p| p.chars().count() <= 9), "{c:?}");
        assert_eq!(c.join(" "), "aaaa bbbb cccc dddd");
        let long = "x".repeat(25);
        assert_eq!(chunks(&long, 10).len(), 3);
    }
}
