//! aurora-channels: neutral content, cards, action tokens and the capability
//! model (docs/13 §2, §4.3-§4.5). Wire types of the connector contract.
#![forbid(unsafe_code)]

pub mod capability;
pub mod card;
pub mod content;
pub mod schema;
pub mod token;

pub use capability::{Capability, CapabilitySet, FallbackContext, Rendered, render_card};
pub use card::{Block, ButtonItem, ButtonStyle, Card, CardError, Field, SelectOption};
pub use content::Content;
pub use token::{ActionKind, ActionToken, Claims, Decision, TokenError};
