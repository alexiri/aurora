//! The only place a child process is created (SEC-34, decision I-23).
//!
//! The supervisor cannot remove `AURORA_BOOT` from its own environment (no `unsafe`), so every
//! child starts from a **rebuilt** environment: `env_clear()`, then the supervisor's container
//! environment (`docs/11-runner-sandboxes.md` §The boot document: TZ, HOME, proxy and CA
//! variables, and the snapshot's app `env` entries) and per-spawn additions, each passed through
//! the same secret filter. `AURORA_BOOT*`, platform credentials (`aub_`/`aup_`) and
//! secret-shaped names with a non-placeholder value never reach a child.
//!
//! Enforcement against bypass: `clippy.toml` (this crate) lists `Command::new` as a disallowed
//! method, and the `no_direct_spawn` test below greps the sources.

use std::ffi::{OsStr, OsString};

const DEFAULT_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
/// Credential stand-in that is safe to pass (`adapters::claude_code::config::PLACEHOLDER`).
const PLACEHOLDER: &str = "aurora-placeholder";

/// Whether a variable may reach a child: never the boot document, never a platform
/// credential (`aub_`/`aup_`), and no secret-shaped name unless the value is the placeholder.
fn may_pass(key: &str, value: &str) -> bool {
    let k = key.to_ascii_uppercase();
    if k.starts_with("AURORA_BOOT") || value.starts_with("aub_") || value.starts_with("aup_") {
        return false;
    }
    let secret_name = ["_TOKEN", "_SECRET", "_PASSWORD", "_KEY", "_CREDENTIALS"]
        .iter()
        .any(|s| k.ends_with(s))
        || k == "TOKEN"
        || k == "SECRET"
        || k == "PASSWORD";
    !secret_name || value == PLACEHOLDER
}

/// The complete environment of a child: the filtered supervisor environment, then `extra`
/// (later entries win; rejected entries are dropped).
pub fn child_env<K, V>(extra: impl IntoIterator<Item = (K, V)>) -> Vec<(OsString, OsString)>
where
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut env: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(k, v)| match (k.to_str(), v.to_str()) {
            (Some(ks), Some(vs)) => may_pass(ks, vs),
            _ => false,
        })
        .collect();
    if !env.iter().any(|(k, _)| k == "PATH") {
        env.push(("PATH".into(), DEFAULT_PATH.into()));
    }
    for (k, v) in extra {
        let (Some(ks), Some(vs)) = (k.as_ref().to_str(), v.as_ref().to_str()) else {
            continue;
        };
        if !may_pass(ks, vs) {
            continue;
        }
        env.retain(|(ek, _)| ek != k.as_ref());
        env.push((k.as_ref().to_owned(), v.as_ref().to_owned()));
    }
    env
}

/// `std::process::Command` with a scrubbed environment.
#[allow(clippy::disallowed_methods)]
pub fn command<K, V>(
    program: impl AsRef<OsStr>,
    extra: impl IntoIterator<Item = (K, V)>,
) -> std::process::Command
where
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut c = std::process::Command::new(program);
    c.env_clear().envs(child_env(extra));
    c
}

/// `tokio::process::Command` with a scrubbed environment.
#[allow(clippy::disallowed_methods)]
pub fn tokio_command<K, V>(
    program: impl AsRef<OsStr>,
    extra: impl IntoIterator<Item = (K, V)>,
) -> tokio::process::Command
where
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut c = tokio::process::Command::new(program);
    c.env_clear().envs(child_env(extra));
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_filtered() {
        assert!(!may_pass("AURORA_BOOT", "x"));
        assert!(!may_pass("FOO", "aub_123"));
        assert!(!may_pass("GITHUB_TOKEN", "ghp"));
        assert!(may_pass("ANTHROPIC_AUTH_TOKEN", PLACEHOLDER));
        assert!(may_pass("HTTPS_PROXY", "http://127.0.0.1:3128"));
        let env = child_env([("AURORA_BOOT", "s"), ("MY_APP", "1")]);
        assert!(env.iter().all(|(k, _)| k != "AURORA_BOOT"));
        assert!(env.iter().any(|(k, v)| k == "MY_APP" && v == "1"));
        assert!(env.iter().any(|(k, _)| k == "PATH"));
    }

    /// No source file other than this one may create a process directly.
    #[test]
    fn no_direct_spawn() {
        fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, hits);
                } else if p.extension().is_some_and(|x| x == "rs")
                    && p.file_name().is_some_and(|n| n != "spawn.rs")
                {
                    let s = std::fs::read_to_string(&p).unwrap();
                    let needle = ["Command", "::new("].concat();
                    if s.contains(&needle) {
                        hits.push(p.display().to_string());
                    }
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut hits = Vec::new();
        walk(&root.join("src"), &mut hits);
        walk(&root.join("tests"), &mut hits);
        assert!(hits.is_empty(), "use crate::spawn instead: {hits:?}");
    }
}
