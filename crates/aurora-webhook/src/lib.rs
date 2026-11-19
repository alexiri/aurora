//! The outgoing-webhook model (P5-WEBHOOK-1, D-320, C-909, C-910, SEC-127): the configuration
//! types, `validate`, `check_values`, `render` and `sign`. Pure (no IO, no async, no database): the
//! API, the gateway and `aurora-agent` share this one implementation and must not re-implement it.
mod model;
mod render;
mod schema;
mod sign;
mod template;
mod validate;

pub use model::{
    ContentType, FieldError, HeaderTemplate, Method, RATE_PER_DAY_MAX, RATE_PER_MINUTE_MAX, Rate,
    RenderError, Rendered, Rule, SecretValues, Slot, SlotError, SlotType, WebhookConfig,
    WebhookPreset,
};
pub use render::{render, render_masked};
pub use sign::sign;
pub use validate::{
    CheckedValues, CompiledConfig, HEADERS_MAX, SECRET_VALUES_MAX, check_values, validate,
};

#[cfg(test)]
mod tests;
