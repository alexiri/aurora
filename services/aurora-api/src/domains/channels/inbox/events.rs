//! The inbox's other event kinds (docs/13 §5.1 "Other events", §5.3, §5.7).

use super::{Retry, ignored, outcome, send};
use crate::app::AppState;
use crate::domains::channels::card_requests::{BindingChoose, card_requests};
use crate::domains::channels::commands;
use crate::domains::connectors::inbox::InboxError;
use aurora_core::sanitize;
use aurora_db::authz;
use aurora_db::repo::channel_inbox::InboxRow;
use aurora_db::repo::channel_presences::{self as presences};
use aurora_db::repo::{
    channel_integrations as integrations, connectors, memory, reach_grants, space_bindings,
    user_links,
};
use aurora_protocol::connector::{BotMembership, PresenceRemoved, Sender};
use serde_json::{Value, json};

pub(super) async fn handle(state: &AppState, row: &InboxRow) -> Result<Value, InboxError> {
    let pool = &state.pool;
    let Some(iid) = row.integration_id else {
        return Ok(ignored("no_integration"));
    };
    // 1. The integration must be active and belong to the posting connector.
    let Some(integ) = integrations::get(pool, iid).await.retry()? else {
        return Ok(ignored("integration_gone"));
    };
    if integ.status != "active" || integ.connector_id != row.connector_id {
        return Ok(ignored("integration_inactive"));
    }
    let manifest = connectors::get(pool, &integ.connector_id)
        .await
        .retry()?
        .and_then(|c| c.manifest);
    let ctx = send::Ctx::new(integ, manifest);
    let bad =
        |e: serde_json::Error| InboxError::Fatal(format!("invalid {} payload: {e}", row.kind));
    match row.kind.as_str() {
        "bot.added" => {
            let p: BotMembership = serde_json::from_value(row.payload.clone()).map_err(bad)?;
            bot_added(state, row, &ctx, p).await
        }
        "bot.removed" => {
            let p: BotMembership = serde_json::from_value(row.payload.clone()).map_err(bad)?;
            bot_removed(state, &ctx, p).await
        }
        "presence.removed" => {
            let p: PresenceRemoved = serde_json::from_value(row.payload.clone()).map_err(bad)?;
            presence_removed(state, &ctx, p).await
        }
        "command.invoked" => {
            let p = serde_json::from_value(row.payload.clone()).map_err(bad)?;
            commands::handle(state, row, &ctx, p).await
        }
        // Recorded in the inbox row; no behaviour in phase 0.
        _ => Ok(outcome("recorded")),
    }
}

/// 5.3: the shared bot was added to a space.
async fn bot_added(
    state: &AppState,
    row: &InboxRow,
    ctx: &send::Ctx,
    p: BotMembership,
) -> Result<Value, InboxError> {
    let pool = &state.pool;
    if ctx.shared_ref().as_deref() != Some(p.bot_ref.as_str()) {
        return Ok(ignored("not_shared_bot"));
    }
    if p.space.kind.as_str() == "dm" {
        return Ok(ignored("dm"));
    }
    let sender = Sender::shared();
    let mut conn = pool.acquire().await.retry()?;
    if space_bindings::active_binding(pool, ctx.id(), &p.space.reference)
        .await
        .retry()?
        .is_some()
    {
        return Ok(ignored("already_bound"));
    }
    // The adder, if linked, who owns agents with a shared presence here.
    let mut allowed = Vec::new();
    let mut actor = None;
    if let Some(r) = p.actor_ref.as_deref()
        && let Some(l) = user_links::user_link_for(pool, ctx.id(), r).await.retry()?
    {
        for pa in presences::active_with_agents(pool, ctx.id())
            .await
            .retry()?
        {
            if pa.mode == "shared"
                && !allowed.contains(&pa.agent_id)
                && authz::can_own(pool, l.user_id, pa.workspace_id)
                    .await
                    .retry()?
            {
                allowed.push(pa.agent_id);
            }
        }
        actor = Some((l.user_id, r.to_owned()));
    }
    let (Some((actor_user_id, actor_ref)), false) = (actor, allowed.is_empty()) else {
        let who = super::speaker::owner_names(state, ctx).await?;
        let text = format!(
            "I'm not set up in this space yet. Ask {who} to bind an agent here (`bind <handle>`)."
        );
        send::say(
            &mut conn,
            ctx,
            &sender,
            &p.space.reference,
            "",
            &text,
            format!("notsetup-added:{}", row.event_id),
        )
        .await
        .retry()?;
        return Ok(outcome("not_set_up"));
    };
    let cards = card_requests(state);
    cards
        .binding_choose(
            &mut conn,
            BindingChoose {
                integration_id: ctx.id(),
                actor_user_id,
                actor_ref: actor_ref.clone(),
                space_ref: p.space.reference.clone(),
                space_label: sanitize::label(&p.space.label),
                allowed_agent_ids: allowed,
            },
        )
        .await
        .retry()?;
    if !cards.renders_cards() {
        send::direct(
            &mut conn,
            ctx,
            &sender,
            &actor_ref,
            &format!(
                "Thanks for adding me to {}. To choose which of your agents answers there, write `bind <handle>` in that space.",
                aurora_channels::content::space_mention(&p.space.reference)
            ),
            format!("bind-hint:{}", row.event_id),
        )
        .await
        .retry()?;
    }
    Ok(outcome("binding_requested"))
}

/// `bot.removed`: the space's binding goes inactive, space reach grants become `left`.
async fn bot_removed(
    state: &AppState,
    ctx: &send::Ctx,
    p: BotMembership,
) -> Result<Value, InboxError> {
    let pool = &state.pool;
    let agent = if ctx.shared_ref().as_deref() == Some(p.bot_ref.as_str()) {
        None
    } else {
        let Some(pa) = presences::active_with_agents(pool, ctx.id())
            .await
            .retry()?
            .into_iter()
            .find(|x| x.platform_user_ref.as_deref() == Some(p.bot_ref.as_str()))
        else {
            return Ok(ignored("unknown_bot"));
        };
        Some(pa.agent_id)
    };
    let mut tx = pool
        .begin()
        .await
        .map_err(aurora_db::DbError::from)
        .retry()?;
    if agent.is_none()
        && let Some(b) = space_bindings::active_binding(&mut *tx, ctx.id(), &p.space.reference)
            .await
            .retry()?
    {
        space_bindings::deactivate(&mut *tx, b.id).await.retry()?;
    }
    let left = reach_grants::leave_space(&mut *tx, ctx.id(), agent, &p.space.reference)
        .await
        .retry()?;
    tx.commit()
        .await
        .map_err(aurora_db::DbError::from)
        .retry()?;
    Ok(json!({"result": "removed", "grants_left": left}))
}

/// `presence.removed`: the presence goes `disabled` (only presences of this integration).
async fn presence_removed(
    state: &AppState,
    ctx: &send::Ctx,
    p: PresenceRemoved,
) -> Result<Value, InboxError> {
    let pool = &state.pool;
    let all = presences::list_for_integration(pool, ctx.id())
        .await
        .retry()?;
    let target = all.iter().find(|x| {
        p.presence_id == Some(x.id)
            || (p.presence_id.is_none()
                && x.platform_user_ref.as_deref() == Some(p.bot_ref.as_str()))
    });
    let Some(t) = target else {
        return Ok(ignored("unknown_presence"));
    };
    if t.status == "disabled" {
        return Ok(outcome("already_disabled"));
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(aurora_db::DbError::from)
        .retry()?;
    presences::set_status(&mut *tx, t.id, "disabled", Some(p.reason.as_str()))
        .await
        .retry()?;
    memory::bump_agent_sync(&mut tx, t.agent_id).await.retry()?;
    tx.commit()
        .await
        .map_err(aurora_db::DbError::from)
        .retry()?;
    Ok(outcome("presence_disabled"))
}
