//! The [`Connector`] trait implemented by each platform adapter.

use std::sync::Arc;

use async_trait::async_trait;
use aurora_protocol::connector::{ConnectorManifest, Integration, OpResult, Operation, Presence};
use thiserror::Error;
use tokio::sync::watch;
use uuid::Uuid;

use crate::client::CoreClient;
use crate::sink::EventSink;

/// Failure to open or update an integration (retried on the next config poll).
#[derive(Debug, Error)]
#[error("{0}")]
pub struct ConnectorError(pub String);

/// Closed when this replica loses ownership of the integration.
#[derive(Debug, Clone)]
pub struct IntegrationHandle {
    id: Uuid,
    closed: Arc<watch::Sender<bool>>,
}

impl IntegrationHandle {
    pub fn new(id: Uuid) -> Self {
        Self {
            id,
            closed: Arc::new(watch::channel(false).0),
        }
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    /// Signals loss of ownership (called by the runtime).
    pub fn close(&self) {
        self.closed.send_replace(true);
    }

    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// Resolves once the handle is closed.
    pub async fn closed(&self) {
        let mut rx = self.closed.subscribe();
        // The sender lives in `self`, so `wait_for` only errors if dropped.
        let _ = rx.wait_for(|c| *c).await;
    }
}

#[async_trait]
pub trait Connector: Send + Sync + 'static {
    fn manifest(&self) -> ConnectorManifest;

    /// Called by the runtime once after `register` with the authenticated contract
    /// client (token and replica id set), so `execute` can reach the
    /// replica-scoped calls `PUT /files/{op_id}` ([`CoreClient::put_file`]) and
    /// `GET /files/{attachment_id}` ([`CoreClient::get_file`]). Default: ignore.
    fn attach_core_client(&self, _client: Arc<CoreClient>) {}

    /// Called when this replica gains ownership; the returned handle is closed on loss of ownership.
    async fn open_integration(
        &self,
        integration: Integration,
        presences: Vec<Presence>,
        sink: EventSink,
    ) -> Result<IntegrationHandle, ConnectorError>;

    async fn update_integration(
        &self,
        handle: &IntegrationHandle,
        integration: Integration,
        presences: Vec<Presence>,
    ) -> Result<(), ConnectorError>;

    /// Executes one claimed operation: a neutral result or neutral error.
    /// Operations with an unknown `kind` never reach this method (the runtime
    /// answers `unsupported`).
    async fn execute(&self, op: Operation) -> OpResult;

    /// The connector's public platform callbacks (`public_callbacks` in the manifest).
    fn router(&self) -> Option<axum::Router> {
        None
    }
}
