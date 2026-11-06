//! aurora-gateway: forward proxy with TLS interception (docs/06-gateway.md).
#![forbid(unsafe_code)]

pub mod apps;
pub mod auth;
pub mod ca;
pub mod check_snapshot;
pub mod decide;
pub mod governance;
pub mod guard;
pub mod guidance;
pub mod hold;
pub mod hygiene;
pub mod inject;
pub mod instance;
pub mod invalidation;
pub mod mechanisms;
pub mod mint;
pub mod pipeline;
pub mod plaintext;
pub mod proxy;
pub mod refresh;
pub mod reqlog;
pub mod resolve;
pub mod resolver;
pub mod snapshot;
pub mod summarize_client;
pub mod upstream;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full, combinators::UnsyncBoxBody};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
/// Body type used on both sides of the proxy.
pub type Body = UnsyncBoxBody<Bytes, BoxError>;

pub fn empty_body() -> Body {
    Empty::<Bytes>::new().map_err(|e| match e {}).boxed_unsync()
}

pub fn full_body(b: impl Into<Bytes>) -> Body {
    Full::new(b.into()).map_err(|e| match e {}).boxed_unsync()
}
