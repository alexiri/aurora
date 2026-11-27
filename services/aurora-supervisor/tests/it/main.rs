//! Single integration-test binary for this crate (OPS-016): one link instead of one per file.
mod agent_cli;
mod background;
mod boot_proxy;
mod common;
mod exec_sftp;
mod home;
mod inbound;
mod pgrelay;
mod previews;
mod pty_env_probe;
mod pty_leak;
mod pty_shell;
mod spawn_env;
mod streams;
mod tools;
mod turns;
mod wrappers;
