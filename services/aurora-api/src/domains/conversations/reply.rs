//! Quote-reply (P5-CHAT-1, D-228, C-725) and files-only messages (C-726): validating a reply
//! target, composing the `turn.queued` `reply_to` payload, and the one function that builds the
//! text delivered to the agent (turn, steered follow-up and promoted follow-up all call it).

use aurora_core::{ApiError, sanitize};
use aurora_db::DbError;
use aurora_db::repo::agents;
use aurora_db::repo::attachments;
use aurora_db::repo::transcript::{self, ReplyTargetRow};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

/// Excerpt length (characters) of a quote (C-725).
pub const EXCERPT_MAX: usize = 280;

/// The quoted message as the agent and the transcript see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Quote {
    Quoted {
        seq: i64,
        author_label: String,
        /// `agent` | `person` | `other`
        author_kind: &'static str,
        at: String,
        excerpt: String,
    },
    /// The target was redacted (or no longer exists).
    Redacted { seq: i64 },
}

impl Quote {
    /// The `turn.queued` payload field.
    pub fn payload(&self) -> Value {
        match self {
            Quote::Quoted {
                seq,
                author_label,
                author_kind,
                at,
                excerpt,
            } => json!({
                "seq": seq,
                "author_label": author_label,
                "author_kind": author_kind,
                "at": at,
                "excerpt": excerpt,
            }),
            Quote::Redacted { seq } => json!({"seq": seq, "redacted": true}),
        }
    }
}

fn invalid(reason: &str) -> ApiError {
    ApiError::unprocessable(
        "reply_target_invalid",
        "The message you are replying to cannot be quoted.",
    )
    .with_details(json!({"reason": reason}))
}

/// `sanitize::block`, clamped to [`EXCERPT_MAX`] characters on a word boundary with "…".
pub fn excerpt_of(text: &str) -> String {
    let block = sanitize::block(text, 8_000);
    if block.chars().count() <= EXCERPT_MAX {
        return block;
    }
    let cut = block
        .char_indices()
        .nth(EXCERPT_MAX - 1)
        .map_or(block.len(), |(i, _)| i);
    let head = &block[..cut];
    // Back up to the last whitespace unless that would throw most of the excerpt away.
    let at = head
        .rfind(char::is_whitespace)
        .filter(|i| head[..*i].chars().count() >= EXCERPT_MAX / 2)
        .unwrap_or(head.len());
    format!("{}…", head[..at].trim_end())
}

async fn describe(
    conn: &mut PgConnection,
    agent_id: Uuid,
    t: &ReplyTargetRow,
) -> Result<Quote, DbError> {
    let (label, kind) = match t.ty.as_str() {
        "turn.queued" => (
            sanitize::label(t.payload["speaker_label"].as_str().unwrap_or("")),
            if t.speaker_kind.as_deref() == Some("person") {
                "person"
            } else {
                "other"
            },
        ),
        _ => {
            let name = agents::get(&mut *conn, agent_id)
                .await?
                .map(|a| a.name)
                .unwrap_or_default();
            (sanitize::label(&name), "agent")
        }
    };
    let text = t.payload["text"].as_str().unwrap_or("");
    let raw = match t.ty.as_str() {
        "attachment" => {
            let name = t.payload["filename"].as_str().unwrap_or("");
            match t.payload["caption"].as_str().filter(|c| !c.is_empty()) {
                Some(c) => format!("{name}\n{c}"),
                None => name.to_owned(),
            }
        }
        "turn.queued" if text.trim().is_empty() => {
            let ids: Vec<Uuid> = t.payload["attachments"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().and_then(|s| s.parse().ok()))
                        .collect()
                })
                .unwrap_or_default();
            attachments::filenames(&mut *conn, &ids).await?.join(", ")
        }
        _ => text.to_owned(),
    };
    Ok(Quote::Quoted {
        seq: t.seq,
        author_label: label,
        author_kind: kind,
        at: t.created_at.to_rfc3339(),
        excerpt: excerpt_of(&raw),
    })
}

/// Validate `seq` as a reply target of `conversation` and compose its quote (C-725). Another
/// conversation's or an unknown seq is `not_found`; a type other than `text`, `turn.queued` or
/// `attachment` is `not_replyable`; a redacted event is `redacted`.
pub async fn resolve(
    conn: &mut PgConnection,
    conversation: Uuid,
    agent_id: Uuid,
    seq: i64,
) -> Result<Quote, ApiError> {
    let Some(t) = transcript::reply_target(&mut *conn, conversation, seq).await? else {
        return Err(invalid("not_found"));
    };
    if !matches!(t.ty.as_str(), "text" | "turn.queued" | "attachment") {
        return Err(invalid("not_replyable"));
    }
    if t.redacted_at.is_some() {
        return Err(invalid("redacted"));
    }
    Ok(describe(conn, agent_id, &t).await?)
}

/// The quote as it stands at delivery time: a target redacted or gone since the reply was
/// written is a redacted quote, never its text.
pub async fn for_delivery(
    conn: &mut PgConnection,
    conversation: Uuid,
    agent_id: Uuid,
    seq: i64,
) -> Result<Quote, DbError> {
    match transcript::reply_target(&mut *conn, conversation, seq).await? {
        Some(t) if t.redacted_at.is_none() => describe(conn, agent_id, &t).await,
        _ => Ok(Quote::Redacted { seq }),
    }
}

/// The text delivered to the agent (never stored): the quote block (C-725), then the person's text,
/// or `(sent N file(s))` when the message is files only (C-726).
pub fn compose_delivered_text(text: &str, reply: Option<&Quote>, n_files: usize) -> String {
    let body = if text.trim().is_empty() && n_files > 0 {
        format!(
            "(sent {n_files} file{})",
            if n_files == 1 { "" } else { "s" }
        )
    } else {
        text.to_owned()
    };
    let Some(q) = reply else {
        return body;
    };
    let mut out = match q {
        Quote::Redacted { .. } => "> In reply to a message that was redacted.".to_owned(),
        Quote::Quoted {
            author_label,
            at,
            excerpt,
            ..
        } => {
            let mut s = format!("> In reply to {author_label}'s message ({at}):");
            for line in excerpt.split('\n') {
                s.push_str("\n> ");
                s.push_str(line);
            }
            s
        }
    };
    out.push_str("\n\n");
    out.push_str(&body);
    out
}

/// Compose the delivered text of a stored turn or follow-up row: looks up its quote and its
/// files. The one call every delivery path makes.
pub async fn delivered_text_of(
    conn: &mut PgConnection,
    conversation: Uuid,
    agent_id: Uuid,
    text: &str,
    reply_to_seq: Option<i64>,
    n_files: usize,
) -> Result<String, DbError> {
    let quote = match reply_to_seq {
        Some(seq) => Some(for_delivery(conn, conversation, agent_id, seq).await?),
        None => None,
    };
    Ok(compose_delivered_text(text, quote.as_ref(), n_files))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quoted(excerpt: &str) -> Quote {
        Quote::Quoted {
            seq: 7,
            author_label: "Ada".into(),
            author_kind: "person",
            at: "2026-11-14T10:00:00+00:00".into(),
            excerpt: excerpt.into(),
        }
    }

    #[test]
    fn plain_text_is_untouched() {
        assert_eq!(compose_delivered_text("hello", None, 0), "hello");
    }

    #[test]
    fn reply_prepends_the_quote_block() {
        let out = compose_delivered_text("yes", Some(&quoted("shall we?")), 0);
        assert_eq!(
            out,
            "> In reply to Ada's message (2026-11-14T10:00:00+00:00):\n> shall we?\n\nyes"
        );
    }

    #[test]
    fn multi_line_excerpt_is_quoted_line_by_line() {
        let out = compose_delivered_text("ok", Some(&quoted("one\ntwo\n\nthree")), 0);
        assert!(out.contains("\n> one\n> two\n> \n> three\n\nok"), "{out}");
    }

    #[test]
    fn redacted_target_gives_the_redacted_line() {
        let out = compose_delivered_text("ok", Some(&Quote::Redacted { seq: 3 }), 0);
        assert_eq!(out, "> In reply to a message that was redacted.\n\nok");
    }

    #[test]
    fn files_only_says_how_many() {
        assert_eq!(compose_delivered_text("", None, 2), "(sent 2 files)");
        assert_eq!(compose_delivered_text("  ", None, 1), "(sent 1 file)");
        // Text with files stays the text.
        assert_eq!(compose_delivered_text("look", None, 2), "look");
    }

    #[test]
    fn reply_and_files_only() {
        let out = compose_delivered_text("", Some(&quoted("x")), 3);
        assert!(out.ends_with("\n> x\n\n(sent 3 files)"), "{out}");
    }

    #[test]
    fn excerpt_is_clamped_on_a_word_boundary() {
        let text = "word ".repeat(100);
        let e = excerpt_of(&text);
        assert!(e.chars().count() <= EXCERPT_MAX, "{}", e.chars().count());
        assert!(e.ends_with("word…"), "{e}");
        assert_eq!(excerpt_of("short"), "short");
    }
}
