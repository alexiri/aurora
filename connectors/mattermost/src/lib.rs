//! aurora-connector-mattermost: the Mattermost connector (connector contract
//! v1, `docs/20`). Shared mode (one bot per integration) and dedicated mode (a bot per agent).
#![forbid(unsafe_code)]

pub mod api;
pub mod callbacks;
pub mod connector;
pub mod events;
pub mod integ;
pub mod lanes;
pub mod managed;
pub mod ops;
pub mod presence;
pub mod render;
pub mod tls;
pub mod ws;

pub use connector::{DEFAULT_CAPABILITIES, MattermostConnector};
pub use integ::Settings;
