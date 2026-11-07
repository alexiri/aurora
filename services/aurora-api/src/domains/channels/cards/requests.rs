//! BE-073's implementation of the [`CardRequests`] seam used by inbox processing (BE-072):
//! `binding.choose` and `dm_default.choose` menus and `reach.decide` DMs to the owners.

use super::build;
use super::{Minter, NewPost, capabilities, dashboard_url_for, post_card};
use crate::domains::channels::card_requests::{
    BindingChoose, CardRequests, DmDefaultChoose, ReachDecide,
};
use aurora_core::ApiError;
use aurora_crypto::DerivedKeys;
use aurora_protocol::connector::{Sender, Target};
use chrono::Utc;
use sqlx::PgConnection;
use std::sync::Arc;
use uuid::Uuid;

/// Requests of one kind to the same place are deduplicated within this window (seconds).
const REQUEST_WINDOW_SECS: i64 = 600;
const REACH_WINDOW_SECS: i64 = 24 * 3600;

pub struct CardsImpl {
    keys: Arc<DerivedKeys>,
    external_url: String,
}

impl CardsImpl {
    pub fn new(keys: Arc<DerivedKeys>, external_url: &str) -> Self {
        Self {
            keys,
            external_url: external_url.to_owned(),
        }
    }

    fn minter(&self, integration_id: Uuid) -> Minter<'_> {
        Minter {
            keys: &self.keys,
            integration_id,
        }
    }
}

fn bucket(secs: i64) -> i64 {
    Utc::now().timestamp() / secs
}

#[async_trait::async_trait]
impl CardRequests for CardsImpl {
    fn renders_cards(&self) -> bool {
        true
    }

    async fn binding_choose(
        &self,
        tx: &mut PgConnection,
        req: BindingChoose,
    ) -> Result<(), ApiError> {
        if req.agents.is_empty() {
            return Ok(());
        }
        let caps = capabilities(tx, req.integration_id).await?;
        let card_id = Uuid::now_v7();
        let thread = req.thread_ref.clone().unwrap_or_default();
        let card = build::binding_choose(
            &self.minter(req.integration_id),
            card_id,
            &req.space_ref,
            &req.agents,
        )?;
        post_card(
            tx,
            &caps,
            NewPost {
                integration_id: req.integration_id,
                subject_kind: "binding",
                subject_id: card_id,
                card_id,
                target: Target::Space {
                    space_ref: req.space_ref.clone(),
                    thread_ref: req.thread_ref,
                },
                sender: Sender::shared(),
                card,
                dashboard_url: Some(dashboard_url_for(&self.external_url, "/")),
                approval_code: None,
                dedupe_key: format!(
                    "binding:{}:{}:{}:{}",
                    req.integration_id,
                    req.space_ref,
                    thread,
                    bucket(REQUEST_WINDOW_SECS)
                ),
            },
        )
        .await?;
        Ok(())
    }

    async fn dm_default_choose(
        &self,
        tx: &mut PgConnection,
        req: DmDefaultChoose,
    ) -> Result<(), ApiError> {
        if req.agents.is_empty() {
            return Ok(());
        }
        let caps = capabilities(tx, req.integration_id).await?;
        let card_id = Uuid::now_v7();
        let card =
            build::dm_default_choose(&self.minter(req.integration_id), card_id, &req.agents)?;
        post_card(
            tx,
            &caps,
            NewPost {
                integration_id: req.integration_id,
                subject_kind: "binding",
                subject_id: card_id,
                card_id,
                target: Target::User {
                    user_ref: req.user_ref.clone(),
                },
                sender: Sender::shared(),
                card,
                dashboard_url: Some(dashboard_url_for(&self.external_url, "/")),
                approval_code: None,
                dedupe_key: format!(
                    "dm_default:{}:{}:{}",
                    req.integration_id,
                    req.user_ref,
                    bucket(REQUEST_WINDOW_SECS)
                ),
            },
        )
        .await?;
        Ok(())
    }

    async fn reach_decide(&self, tx: &mut PgConnection, req: ReachDecide) -> Result<(), ApiError> {
        if req.owner_refs.is_empty() {
            return Ok(());
        }
        let caps = capabilities(tx, req.integration_id).await?;
        for owner in &req.owner_refs {
            let card = build::reach(
                &self.minter(req.integration_id),
                req.grant_id,
                &req.agent_label,
                &req.subject_label,
            )?;
            post_card(
                tx,
                &caps,
                NewPost {
                    integration_id: req.integration_id,
                    subject_kind: "reach",
                    subject_id: req.grant_id,
                    card_id: Uuid::now_v7(),
                    target: Target::User {
                        user_ref: owner.clone(),
                    },
                    sender: Sender::shared(),
                    card,
                    dashboard_url: Some(dashboard_url_for(
                        &self.external_url,
                        &format!("/agents/{}", req.agent_id),
                    )),
                    approval_code: None,
                    dedupe_key: format!(
                        "reach:{}:{}:{}",
                        req.grant_id,
                        owner,
                        bucket(REACH_WINDOW_SECS)
                    ),
                },
            )
            .await?;
        }
        Ok(())
    }
}
