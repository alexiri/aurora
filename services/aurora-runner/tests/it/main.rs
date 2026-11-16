//! Single integration-test binary for this crate (OPS-016): one link instead of one per file.
mod channel;
mod files_streams;
mod inbound;
mod k8s_mock;
mod leg_limits;
mod podman;
mod preview_streams_flow;
mod runner;
mod ssh_streams;
mod streams;
mod terminal_streams;
