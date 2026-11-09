//! aurora-gateway: forward proxy with TLS interception (docs/06-gateway.md).
#![forbid(unsafe_code)]

pub mod apps;
pub mod auth;
pub mod byo;
pub mod ca;
pub mod check_snapshot;
pub mod coalesce;
pub mod db;
pub mod decide;
pub mod governance;
pub mod guard;
pub mod guidance;
pub mod health;
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
pub mod proxy_protocol;
pub mod proxy_tls;
pub mod refresh;
pub mod reqlog;
pub mod resolve;
pub mod resolver;
pub mod snapshot;
pub mod status;
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

/// `X-Aurora-Key-Id` header for internal calls (RFE-007); empty when no id is configured.
pub(crate) fn key_id_headers(id: Option<&str>) -> reqwest::header::HeaderMap {
    let mut h = reqwest::header::HeaderMap::new();
    if let Some(v) = id.and_then(|i| reqwest::header::HeaderValue::from_str(i).ok()) {
        h.insert(aurora_core::limits::KEY_ID_HEADER, v);
    }
    h
}
