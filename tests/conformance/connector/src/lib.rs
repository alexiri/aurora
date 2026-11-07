//! Connector contract v1 conformance suite (docs/13 §7, TEST-015).
//!
//! * [`fake_core`]: an in-memory implementation of the connector plane
//!   (`/v1/connector/*`) that scripts operations, records events and results,
//!   and expires claim leases on demand. Every request is validated.
//! * [`target`]: runs the connector under test, in-process through the SDK
//!   runtime or as a spawned binary (`scripts/conformance.sh <binary>`).
//! * [`driver`]: the platform side. The fake connector's driver is
//!   [`fake_driver::FakeDriver`]; a real connector's suite needs its own.
//!
//! The twelve scenarios live in `tests/scenarios.rs`.
#![forbid(unsafe_code)]

pub mod driver;
pub mod fake_core;
pub mod fake_driver;
pub mod mattermost_driver;
pub mod target;
