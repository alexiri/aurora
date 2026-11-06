//! aurora-connector-sdk: the connector/1 contract client (`docs/13 §4`), the
//! register / config / ops loops with backoff and lease handling, event
//! batching with ack-after-accept, and the [`Connector`] trait (`docs/13 §8`).
#![forbid(unsafe_code)]

mod client;
mod connector;
mod runtime;
mod sink;

pub use client::{ClientError, ConfigPoll, CoreClient, RetryPolicy};
pub use connector::{Connector, ConnectorError, IntegrationHandle};
pub use runtime::{RuntimeConfig, RuntimeError, execute_one, run};
pub use sink::{EventAck, EventSink, SinkClosed, spawn_event_poster};

pub use aurora_protocol::connector as wire;
