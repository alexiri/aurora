//! (P5-POLISH-4, D-386, C-1030, SEC-135) A person asks the organization's owners and admins to make an
//! app ready. A request gives nothing: it is a record and a notice, resolved by a real change
//! ([`service::reevaluate`], called after the commit of whatever can change an app's readiness), withdrawn
//! by the asker or declined by an owner or admin.
pub mod dto;
mod routes;
pub mod service;

pub use routes::router;
