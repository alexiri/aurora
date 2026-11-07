//! Manifest mechanisms (docs/09 §5) applied to app-connection requests:
//! guard, inject, route, transform, finalize, intercept.

pub use aurora_registry::manifest::{Finalize, Transform};

pub mod finalize;
pub mod inject;
pub mod intercept;
pub mod route;
pub mod template;
pub mod transform;

/// A template or mechanism could not be applied; the request fails with 502
/// `credential_unavailable` and nothing is sent upstream.
#[derive(Debug, PartialEq, Eq)]
pub struct Unavailable;
