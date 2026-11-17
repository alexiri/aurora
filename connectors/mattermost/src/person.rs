//! The client for a person's own token (phase 5, P4-UTOK, D-281, D-282, SEC-112, SEC-113; docs/20
//! §Operations).
//!
//! A person's personal access token reaches Mattermost through **this type only**, and this type
//! has exactly two methods: [`PersonClient::me`] (`GET /api/v4/users/me`, the verify op) and
//! [`PersonClient::post`] (`POST /api/v4/posts`, the send). There is no generic request method, so
//! no other endpoint can be called with a person's token, and the bot's client ([`crate::api::Mm`])
//! has no way to take one. The token is passed per call, never stored in the client, and appears
//! in no log line, error text or metric label: a `401` answers `credential_dead` with no detail
//! (`api::map_status` never echoes a body).

use aurora_protocol::connector::{ErrorCode, OpError};
use serde_json::Value;

use crate::api::{R, check, map_transport};

/// A Mattermost server as seen with a person's token (see the module docs).
#[derive(Clone)]
pub struct PersonClient {
    http: reqwest::Client,
    base: String,
}

impl PersonClient {
    pub(crate) fn new(http: reqwest::Client, base: String) -> Self {
        Self { http, base }
    }

    /// `GET /api/v4/users/me` with the person's token: the account the token belongs to.
    pub async fn me(&self, token: &str) -> R<Value> {
        let resp = self
            .http
            .get(format!("{}/api/v4/users/me", self.base))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| map_transport(&e))?;
        check(resp).await?.json::<Value>().await.map_err(|_| {
            OpError::new(
                ErrorCode::Unavailable,
                "unexpected mattermost response to users/me",
            )
        })
    }

    /// `POST /api/v4/posts` with the person's token: the post as that person.
    pub async fn post(&self, token: &str, body: &Value) -> R<Value> {
        let resp = self
            .http
            .post(format!("{}/api/v4/posts", self.base))
            .bearer_auth(token)
            .json(body)
            .send()
            .await
            .map_err(|e| map_transport(&e))?;
        check(resp).await?.json::<Value>().await.map_err(|_| {
            OpError::new(
                ErrorCode::Unavailable,
                "unexpected mattermost response to posts",
            )
        })
    }
}
