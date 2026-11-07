//! aurora-connector-mattermost: the Mattermost connector (connector contract
//! v1, `docs/20`). Shared mode: one bot per integration; dedicated bots are MM-002.
#![forbid(unsafe_code)]

pub mod api;
pub mod callbacks;
pub mod connector;
pub mod events;
pub mod integ;
pub mod ops;
pub mod render;
pub mod ws;

pub use connector::{DEFAULT_CAPABILITIES, MattermostConnector};
pub use integ::Settings;
