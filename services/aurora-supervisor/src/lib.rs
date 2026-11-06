//! aurora-supervisor: Aurora's only program inside a hosted sandbox (`docs/12-supervisor-harness.md`).
//!
//! Modules: [`boot`] (boot document, non-dumpable, files), [`channel`] (W6 client with
//! sequenced/acked frames and reconnect), [`proxy`] (loopback credential-adding forwarders),
//! [`harness`] (vendor-neutral adapter traits), [`fake`] (scripted adapter), [`supervisor`]
//! (turn execution), [`logging`].
#![forbid(unsafe_code)]

pub mod boot;
pub mod channel;
pub mod fake;
pub mod harness;
pub mod harvest;
pub mod home;
pub mod instructions;
pub mod logging;
pub mod outbound;
pub mod proxy;
pub mod supervisor;
