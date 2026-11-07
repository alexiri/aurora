//! Card builders per subject (docs/13 §5.6). All untrusted text goes through the card
//! constructors, which wrap it as data.

use super::Minter;
use crate::domains::channels::card_requests::AgentChoice;
use aurora_channels::card::{Block, ButtonItem, ButtonStyle, Card, MAX_OPTIONS};
use aurora_channels::{ActionKind, Decision};
use aurora_core::ApiError;
use aurora_db::repo::action_approvals::ActionApprovalRow;
use aurora_db::repo::approvals::ApprovalRow;
use aurora_protocol::vocab::BoundedString;
use serde_json::Value;
use uuid::Uuid;

fn card(blocks: Vec<Block>) -> Result<Card, ApiError> {
    Card::new(blocks).map_err(|e| ApiError::internal(format!("card: {e}")))
}

fn blk<T>(r: Result<T, aurora_channels::CardError>) -> Result<T, ApiError> {
    r.map_err(|e| ApiError::internal(format!("card: {e}")))
}

fn decide_buttons(
    m: &Minter<'_>,
    subject: Uuid,
    mk: impl Fn(Decision) -> ActionKind,
    labels: (&str, &str),
) -> Result<Vec<ButtonItem>, ApiError> {
    Ok(vec![
        ButtonItem::new(
            labels.0,
            ButtonStyle::Primary,
            m.mint(subject, mk(Decision::Approve))?,
        ),
        ButtonItem::new(
            labels.1,
            ButtonStyle::Danger,
            m.mint(subject, mk(Decision::Deny))?,
        ),
    ])
}

/// Gateway approval card: header, summary title and fields, context, Approve/Deny and, for
/// batches with more than one approval, Approve all / Deny all.
pub fn approval(m: &Minter<'_>, row: &ApprovalRow) -> Result<Card, ApiError> {
    let title = row
        .summary
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("An agent request needs a decision");
    let mut blocks = vec![Block::header("Approval needed"), Block::data(title)];
    let fields: Vec<(String, String)> = row
        .summary
        .get("fields")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    Some((
                        f.get("label")?.as_str()?.to_owned(),
                        f.get("value")?.as_str()?.to_owned(),
                    ))
                })
                .take(aurora_channels::card::MAX_FIELDS)
                .collect()
        })
        .unwrap_or_default();
    if !fields.is_empty() {
        blocks.push(blk(Block::fields(
            fields.iter().map(|(l, v)| (l.as_str(), v.as_str())),
        ))?);
    }
    let mut ctx = format!("Agent: {}", row.agent_name.as_deref().unwrap_or("unknown"));
    if let Some(rule) = &row.rule_name {
        ctx.push_str(&format!(" · Rule: {rule}"));
    }
    ctx.push_str(&format!(
        " · Expires {}",
        row.deadline.format("%Y-%m-%d %H:%M UTC")
    ));
    // Phase 2, RFE-012: who may decide.
    ctx.push_str(" · Needs an approver");
    blocks.push(Block::context(&ctx));
    let mut items = decide_buttons(
        m,
        row.id,
        |decision| ActionKind::ApprovalDecide { decision },
        ("Approve", "Deny"),
    )?;
    if let (Some(batch), Some(total)) = (&row.batch_id, row.batch_total)
        && total > 1
    {
        items.extend(decide_buttons(
            m,
            row.id,
            |decision| ActionKind::ApprovalBatchDecide {
                batch_id: batch_uuid(batch),
                agent_id: row.agent_id,
                decision,
            },
            ("Approve all", "Deny all"),
        )?);
    }
    blocks.push(blk(Block::buttons(items))?);
    card(blocks)
}

/// `batch_id` is free text on the approval; the token carries a stable UUID derived from it and
/// the router resolves the batch back through [`batch_uuid`] over the agent's pending rows.
pub fn batch_uuid(batch: &str) -> Uuid {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(batch.as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&d[..16]);
    Uuid::from_bytes(b)
}

pub fn action_approval(
    m: &Minter<'_>,
    row: &ActionApprovalRow,
    agent_label: &str,
) -> Result<Card, ApiError> {
    let ctx = format!(
        "Agent: {} · Action: {} · Expires {} · Needs an approver",
        agent_label,
        row.action_key,
        row.expires_at.format("%Y-%m-%d %H:%M UTC")
    );
    let buttons = decide_buttons(
        m,
        row.id,
        |decision| ActionKind::ActionApprovalDecide { decision },
        ("Approve", "Deny"),
    )?;
    card(vec![
        Block::header("Approval needed"),
        Block::data(&row.summary),
        Block::context(&ctx),
        blk(Block::buttons(buttons))?,
    ])
}

pub fn reach(
    m: &Minter<'_>,
    grant: Uuid,
    agent_label: &str,
    subject_label: &str,
) -> Result<Card, ApiError> {
    let buttons = decide_buttons(
        m,
        grant,
        |decision| ActionKind::ReachDecide { decision },
        ("Allow", "Block"),
    )?;
    card(vec![
        Block::header("Reach request"),
        Block::text("Someone wants to talk to an agent you own through this bot. Allow it?"),
        blk(Block::fields([
            ("Agent", agent_label),
            ("Requested by", subject_label),
        ]))?,
        blk(Block::buttons(buttons))?,
    ])
}

fn options(agents: &[AgentChoice]) -> Vec<(String, String)> {
    agents
        .iter()
        .take(MAX_OPTIONS)
        .map(|a| (a.label.clone(), a.agent_id.to_string()))
        .collect()
}

pub fn binding_choose(
    m: &Minter<'_>,
    card_id: Uuid,
    space_ref: &str,
    agents: &[AgentChoice],
) -> Result<Card, ApiError> {
    let allowed: Vec<Uuid> = agents
        .iter()
        .take(MAX_OPTIONS)
        .map(|a| a.agent_id)
        .collect();
    let token = m.mint(
        card_id,
        ActionKind::BindingChoose {
            space_ref: BoundedString::new(space_ref)
                .map_err(|_| ApiError::internal("space_ref too long"))?,
            allowed_agent_ids: allowed,
        },
    )?;
    card(vec![
        Block::header("Choose an agent"),
        Block::text("Which agent should answer in this space?"),
        blk(Block::select("Choose an agent", options(agents), token))?,
    ])
}

pub fn dm_default_choose(
    m: &Minter<'_>,
    card_id: Uuid,
    agents: &[AgentChoice],
) -> Result<Card, ApiError> {
    let allowed: Vec<Uuid> = agents
        .iter()
        .take(MAX_OPTIONS)
        .map(|a| a.agent_id)
        .collect();
    let token = m.mint(
        card_id,
        ActionKind::DmDefaultChoose {
            allowed_agent_ids: allowed,
        },
    )?;
    card(vec![
        Block::header("Choose an agent"),
        Block::text("Which agent should answer your direct messages?"),
        blk(Block::select("Choose an agent", options(agents), token))?,
    ])
}

/// Agent-link request card (docs/07 agent links): `side` is `a` or `b` of the ordered pair.
pub fn agent_link(
    m: &Minter<'_>,
    link: Uuid,
    side: &str,
    requester_label: &str,
    target_label: &str,
) -> Result<Card, ApiError> {
    let side = BoundedString::<32>::new(side).map_err(|_| ApiError::internal("side too long"))?;
    let buttons = decide_buttons(
        m,
        link,
        |decision| ActionKind::AgentLinkDecide {
            side: side.clone(),
            decision,
        },
        ("Allow", "Block"),
    )?;
    card(vec![
        Block::header("Agent link request"),
        blk(Block::fields([
            ("From", requester_label),
            ("To", target_label),
        ]))?,
        blk(Block::buttons(buttons))?,
    ])
}

/// Stop / Send now buttons for a running turn's status card (used by the mirror).
pub fn turn_controls(m: &Minter<'_>, turn: Uuid, subject: Uuid) -> Result<Block, ApiError> {
    blk(Block::buttons(vec![
        ButtonItem::new(
            "Stop",
            ButtonStyle::Danger,
            m.mint(subject, ActionKind::TurnStop { turn_id: turn })?,
        ),
        ButtonItem::new(
            "Send now",
            ButtonStyle::Default,
            m.mint(subject, ActionKind::FollowupSendNow { turn_id: turn })?,
        ),
    ]))
}
