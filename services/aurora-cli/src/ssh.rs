//! `aurora ssh <agent>` and `aurora ssh cert <agent>` (phase 5, P4-SSH-1, D-239(i), C-798).
//!
//! The CLI mints a short-lived OpenSSH user certificate for the person's registered ed25519 key,
//! pins the host key from `GET /v1/ssh` in its own `known_hosts` file and `exec`s the system
//! `ssh`. **It never reads the private key**: it only builds `-i <path>` from the `.pub` path and
//! lets `ssh` open the file. The argument list is a pure function ([`build_argv`]) so the
//! security-relevant options (`StrictHostKeyChecking=yes`, `IdentitiesOnly=yes`, our own
//! `UserKnownHostsFile`) are tested exactly.
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use aurora_client::config::{self, Config};
use aurora_client::{Client, clean};
use clap::{Args, Subcommand};
use ssh_key::{Algorithm, Certificate, PublicKey};

/// A certificate is reused while it stays valid for more than this many seconds (C-798).
pub const REUSE_MARGIN_SECS: u64 = 60;

#[derive(Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct SshCmd {
    #[command(subcommand)]
    pub sub: Option<SshSub>,
    /// The agent: its id, or its handle in the workspace you logged in to.
    pub agent: Option<String>,
    /// Your public key (default: ~/.ssh/id_ed25519.pub). Its private half is the file without `.pub`.
    #[arg(long)]
    pub key: Option<PathBuf>,
    /// Passed to ssh after the destination (a remote command, or ssh options such as `-v`).
    #[arg(last = true)]
    pub ssh_args: Vec<String>,
}

#[derive(Subcommand)]
pub enum SshSub {
    /// Get a certificate (and the pinned host key) without connecting; prints the file to pass to ssh.
    Cert {
        /// The agent: its id, or its handle in the workspace you logged in to.
        agent: String,
        /// Your public key (default: ~/.ssh/id_ed25519.pub).
        #[arg(long)]
        key: Option<PathBuf>,
    },
}

/// Everything `ssh` needs, after the certificate and the host key are on disk.
#[derive(Debug, PartialEq, Eq)]
pub struct Prepared {
    pub agent_id: String,
    pub private_key: PathBuf,
    pub cert_file: PathBuf,
    pub known_hosts_file: PathBuf,
    pub host: String,
    pub port: u16,
}

/// The `ssh` argument list (without the program name): our options, then the destination
/// `<agent id>@<host>`, then whatever the person passed after `--`.
pub fn build_argv(p: &Prepared, extra: &[String]) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-i".into(),
        p.private_key.display().to_string(),
        "-o".into(),
        format!("CertificateFile={}", p.cert_file.display()),
        "-o".into(),
        format!("UserKnownHostsFile={}", p.known_hosts_file.display()),
        "-o".into(),
        "StrictHostKeyChecking=yes".into(),
        "-o".into(),
        "IdentitiesOnly=yes".into(),
        "-p".into(),
        p.port.to_string(),
        format!("{}@{}", p.agent_id, p.host),
    ];
    a.extend(extra.iter().cloned());
    a
}

/// `~/.ssh/id_ed25519.pub`.
pub fn default_key_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set; pass --key"))?;
    Ok(PathBuf::from(home).join(".ssh").join("id_ed25519.pub"))
}

/// The private key's path: the `.pub` path without its suffix. Never opened by this program.
pub fn private_key_path(public: &Path) -> Result<PathBuf> {
    let s = public.to_string_lossy();
    match s.strip_suffix(".pub") {
        Some(base) if !base.is_empty() => Ok(PathBuf::from(base)),
        _ => bail!(
            "--key must be the path of a public key ending in .pub (got {}); its private half is the same path without .pub",
            clean(&s)
        ),
    }
}

/// `~/.config/aurora/ssh` (or under `$AURORA_CONFIG_DIR`).
pub fn ssh_dir() -> Result<PathBuf> {
    Ok(config::config_dir()?.join("ssh"))
}

pub fn cert_path(dir: &Path, agent_id: &str) -> PathBuf {
    dir.join(format!("{agent_id}-cert.pub"))
}

/// Whether the certificate in `text` can still be used: it parses, certifies `subject`, names
/// `agent_id` as its only principal and stays valid for more than [`REUSE_MARGIN_SECS`].
pub fn cert_reusable(text: &str, now_unix: u64, agent_id: &str, subject: &PublicKey) -> bool {
    let Ok(cert) = Certificate::from_openssh(text.trim()) else {
        return false;
    };
    cert.public_key() == subject.key_data()
        && cert.valid_principals() == [agent_id.to_owned()]
        && cert.valid_before() > now_unix.saturating_add(REUSE_MARGIN_SECS)
        && cert.valid_after() <= now_unix.saturating_add(REUSE_MARGIN_SECS)
}

/// Write `contents` to `path` atomically with mode 0600 (the directory 0700 when created).
pub fn write_private_file(path: &Path, contents: &str) -> Result<()> {
    if let Some(dir) = path.parent()
        && !dir.exists()
    {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(contents.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// The agent's id: an id as given, else the handle looked up in the workspace of the login.
async fn resolve_agent(client: &Client, cfg: &Config, agent: &str) -> Result<String> {
    if uuid::Uuid::parse_str(agent).is_ok() {
        return Ok(agent.to_ascii_lowercase());
    }
    client
        .list_agents(&cfg.workspace.id)
        .await?
        .into_iter()
        .find(|a| a.handle == agent)
        .map(|a| a.id)
        .ok_or_else(|| {
            anyhow!(
                "no agent `{}` in workspace {}/{}; pass its id",
                clean(agent),
                clean(&cfg.workspace.org_slug),
                clean(&cfg.workspace.slug)
            )
        })
}

/// The refusals of C-790..C-792 in the words a person can act on.
fn explain(e: aurora_client::Error, cfg_url: &str) -> anyhow::Error {
    match e.code() {
        Some("ssh_disabled") => anyhow!("SSH is not enabled on this Aurora instance"),
        Some("ssh_key_not_registered") => anyhow!(
            "this key is not registered with Aurora: add it under Account › SSH keys ({}/account/ssh) and try again",
            clean(cfg_url.trim_end_matches('/'))
        ),
        Some("ssh_not_supported") => anyhow!("SSH is available for hosted agents only"),
        Some("frozen") => anyhow!("this agent is frozen; SSH is unavailable while it is frozen"),
        Some("ssh_mint_rate_limited") => anyhow!(
            "too many certificates for this agent in the last hour; try again after the hour changes"
        ),
        _ => anyhow!(e),
    }
}

/// Fetch (or reuse) the certificate, pin the host key and return the facts `ssh` needs. Reads
/// only the *public* key file.
pub async fn prepare(
    client: &Client,
    cfg_url: &str,
    agent_id: &str,
    public_key_file: &Path,
    dir: &Path,
    now_unix: u64,
) -> Result<Prepared> {
    let private_key = private_key_path(public_key_file)?;
    let line = std::fs::read_to_string(public_key_file).with_context(|| {
        format!(
            "cannot read {} (create a key with `ssh-keygen -t ed25519`, or pass --key)",
            public_key_file.display()
        )
    })?;
    let line = line
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let subject = PublicKey::from_openssh(line)
        .map_err(|_| anyhow!("{} is not an OpenSSH public key", public_key_file.display()))?;
    if subject.algorithm() != Algorithm::Ed25519 {
        bail!("Aurora accepts ed25519 keys only; create one with `ssh-keygen -t ed25519`");
    }

    let facts = client.ssh_facts().await.map_err(|e| explain(e, cfg_url))?;
    if facts.known_hosts.is_empty() {
        bail!("SSH is enabled but no SSH server is running; ask your administrator");
    }
    let known_hosts_file = dir.join("known_hosts");
    write_private_file(
        &known_hosts_file,
        &format!("{}\n", facts.known_hosts.join("\n")),
    )?;

    let cert_file = cert_path(dir, agent_id);
    let reuse = std::fs::read_to_string(&cert_file)
        .map(|t| cert_reusable(&t, now_unix, agent_id, &subject))
        .unwrap_or(false);
    if !reuse {
        let minted = client
            .mint_ssh_certificate(agent_id, line)
            .await
            .map_err(|e| explain(e, cfg_url))?;
        write_private_file(&cert_file, &format!("{}\n", minted.certificate.trim()))?;
    }
    Ok(Prepared {
        agent_id: agent_id.to_owned(),
        private_key,
        cert_file,
        known_hosts_file,
        host: facts.host,
        port: facts.port,
    })
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub async fn run(cmd: SshCmd) -> Result<i32> {
    let path = config::default_path()?;
    let cfg = Config::load(&path)?;
    let client = Client::new(&cfg.url, Some(cfg.api_key.clone()))?;
    let (agent, key, extra, connect) = match cmd.sub {
        Some(SshSub::Cert { agent, key }) => (agent, key, Vec::new(), false),
        None => (
            cmd.agent.ok_or_else(|| {
                anyhow!("usage: aurora ssh <agent> [--key <path>] [-- <ssh args>]")
            })?,
            cmd.key,
            cmd.ssh_args,
            true,
        ),
    };
    let key = match key {
        Some(k) => k,
        None => default_key_path()?,
    };
    let agent_id = resolve_agent(&client, &cfg, &agent).await?;
    let p = prepare(&client, &cfg.url, &agent_id, &key, &ssh_dir()?, now_unix()).await?;
    if !connect {
        println!("certificate: {}", p.cert_file.display());
        println!("known_hosts: {}", p.known_hosts_file.display());
        println!(
            "connect:     ssh {}",
            build_argv(&p, &[])
                .iter()
                .map(|a| shell_word(a))
                .collect::<Vec<_>>()
                .join(" ")
        );
        return Ok(0);
    }
    exec_ssh(&build_argv(&p, &extra))
}

/// Quote a word for display only when it needs it.
fn shell_word(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=@:~".contains(c))
    {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Replace this process with `ssh` (so signals, the terminal and the exit status are ssh's own).
fn exec_ssh(argv: &[String]) -> Result<i32> {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new("ssh").args(argv).exec();
    // `exec` only returns on failure.
    if err.kind() == std::io::ErrorKind::NotFound {
        bail!("`ssh` was not found; install an OpenSSH client");
    }
    Err(anyhow!("could not start ssh: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use ssh_key::PrivateKey;
    use ssh_key::certificate::{Builder, CertType};
    use ssh_key::private::Ed25519Keypair;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const AGENT: &str = "11111111-1111-4111-8111-111111111111";

    fn user_key() -> PrivateKey {
        PrivateKey::from(Ed25519Keypair::from_seed(&[3; 32]))
    }

    fn pub_line() -> String {
        format!(
            "{} me@laptop",
            user_key().public_key().to_openssh().unwrap()
        )
    }

    /// A certificate signed by a throwaway CA.
    fn cert_line(agent: &str, valid_after: u64, valid_before: u64) -> String {
        let ca = PrivateKey::from(Ed25519Keypair::from_seed(&[9; 32]));
        let mut b = Builder::new(
            vec![1u8; 32],
            user_key().public_key().key_data().clone(),
            valid_after,
            valid_before,
        )
        .unwrap();
        b.serial(5).unwrap();
        b.cert_type(CertType::User).unwrap();
        b.key_id("aurora:test").unwrap();
        b.valid_principal(agent).unwrap();
        b.extension("permit-pty", "").unwrap();
        b.sign(&ca).unwrap().to_openssh().unwrap()
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aurora-cli-ssh-{tag}-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const NOW: u64 = 2_000_000_000;

    #[test]
    fn argv_is_exact() {
        let p = Prepared {
            agent_id: AGENT.into(),
            private_key: "/home/u/.ssh/id_ed25519".into(),
            cert_file: format!("/home/u/.config/aurora/ssh/{AGENT}-cert.pub").into(),
            known_hosts_file: "/home/u/.config/aurora/ssh/known_hosts".into(),
            host: "ssh.example.org".into(),
            port: 2222,
        };
        assert_eq!(
            build_argv(&p, &["ls".into(), "-la".into()]),
            vec![
                "-i",
                "/home/u/.ssh/id_ed25519",
                "-o",
                &format!("CertificateFile=/home/u/.config/aurora/ssh/{AGENT}-cert.pub"),
                "-o",
                "UserKnownHostsFile=/home/u/.config/aurora/ssh/known_hosts",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "IdentitiesOnly=yes",
                "-p",
                "2222",
                &format!("{AGENT}@ssh.example.org"),
                "ls",
                "-la",
            ]
        );
    }

    #[test]
    fn private_key_is_the_pub_path_without_the_suffix() {
        assert_eq!(
            private_key_path(Path::new("/h/.ssh/id_ed25519.pub")).unwrap(),
            PathBuf::from("/h/.ssh/id_ed25519")
        );
        assert!(private_key_path(Path::new("/h/.ssh/id_ed25519")).is_err());
        assert!(private_key_path(Path::new(".pub")).is_err());
    }

    #[test]
    fn a_certificate_is_reused_only_while_it_stays_valid_long_enough() {
        let subject = user_key().public_key().clone();
        let ok = cert_line(AGENT, NOW - 60, NOW + 900);
        assert!(cert_reusable(&ok, NOW, AGENT, &subject));
        // 61 s left: reused; exactly 60 s left: not.
        assert!(cert_reusable(
            &cert_line(AGENT, NOW - 60, NOW + 61),
            NOW,
            AGENT,
            &subject
        ));
        assert!(!cert_reusable(
            &cert_line(AGENT, NOW - 60, NOW + 60),
            NOW,
            AGENT,
            &subject
        ));
        assert!(!cert_reusable(
            &cert_line(AGENT, NOW - 960, NOW - 60),
            NOW,
            AGENT,
            &subject
        ));
        // Another agent's certificate, another key's, garbage.
        assert!(!cert_reusable(
            &ok,
            NOW,
            "22222222-2222-4222-8222-222222222222",
            &subject
        ));
        let other = PrivateKey::from(Ed25519Keypair::from_seed(&[4; 32]));
        assert!(!cert_reusable(&ok, NOW, AGENT, other.public_key()));
        assert!(!cert_reusable("not a certificate", NOW, AGENT, &subject));
    }

    #[test]
    fn files_are_private() {
        let d = tmp("mode");
        let f = d.join("sub").join("x-cert.pub");
        write_private_file(&f, "data\n").unwrap();
        assert_eq!(
            std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(f.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        // Replacing keeps the mode.
        write_private_file(&f, "more\n").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "more\n");
        assert_eq!(
            std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    async fn server(mint_times: u64, known_hosts: serde_json::Value) -> MockServer {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/ssh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "host": "ssh.example.org", "port": 2222, "cert_ttl_secs": 900,
                "terminators_live": 1, "host_keys": [], "known_hosts": known_hosts
            })))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/v1/agents/{AGENT}/ssh-certificate")))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "certificate": cert_line(AGENT, NOW - 60, NOW + 900), "serial": "5",
                "principal": AGENT, "key_id": "k", "valid_after": "2033-05-18T03:32:00Z",
                "valid_before": "2033-05-18T03:48:00Z", "username": AGENT,
                "host": "ssh.example.org", "port": 2222, "known_hosts": []
            })))
            .expect(mint_times)
            .mount(&s)
            .await;
        s
    }

    fn pub_file(d: &Path) -> PathBuf {
        let f = d.join("id_ed25519.pub");
        std::fs::write(&f, format!("{}\n", pub_line())).unwrap();
        f
    }

    #[tokio::test]
    async fn mints_once_pins_the_host_key_and_reuses_the_certificate() {
        let s = server(1, json!(["[ssh.example.org]:2222 ssh-ed25519 AAAAHOST"])).await;
        let d = tmp("reuse");
        let key = pub_file(&d);
        let dir = d.join("cfg");
        let c = Client::new(&s.uri(), Some("auk_x".into())).unwrap();
        let first = prepare(&c, &s.uri(), AGENT, &key, &dir, NOW).await.unwrap();
        assert_eq!(first.private_key, d.join("id_ed25519"));
        assert_eq!((first.host.as_str(), first.port), ("ssh.example.org", 2222));
        assert_eq!(
            std::fs::read_to_string(&first.known_hosts_file).unwrap(),
            "[ssh.example.org]:2222 ssh-ed25519 AAAAHOST\n"
        );
        for f in [&first.cert_file, &first.known_hosts_file] {
            assert_eq!(
                std::fs::metadata(f).unwrap().permissions().mode() & 0o777,
                0o600,
                "{f:?}"
            );
        }
        // Still valid for 15 minutes: the second run makes no mint request (`expect(1)` checks on drop).
        let second = prepare(&c, &s.uri(), AGENT, &key, &dir, NOW + 30)
            .await
            .unwrap();
        assert_eq!(second, first);
    }

    #[tokio::test]
    async fn an_expiring_certificate_is_replaced() {
        let s = server(1, json!(["h ssh-ed25519 AAAA"])).await;
        let d = tmp("expired");
        let key = pub_file(&d);
        let dir = d.join("cfg");
        write_private_file(
            &cert_path(&dir, AGENT),
            &cert_line(AGENT, NOW - 960, NOW + 30),
        )
        .unwrap();
        let c = Client::new(&s.uri(), Some("auk_x".into())).unwrap();
        let p = prepare(&c, &s.uri(), AGENT, &key, &dir, NOW).await.unwrap();
        let fresh = std::fs::read_to_string(&p.cert_file).unwrap();
        assert!(cert_reusable(&fresh, NOW, AGENT, user_key().public_key()));
    }

    /// The CLI never opens the private key: an unreadable one next to the `.pub` changes nothing.
    #[tokio::test]
    async fn the_private_key_is_never_opened() {
        let s = server(1, json!(["h ssh-ed25519 AAAA"])).await;
        let d = tmp("private");
        let key = pub_file(&d);
        let private = d.join("id_ed25519");
        std::fs::write(&private, "PRIVATE KEY MATERIAL\n").unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o000)).unwrap();
        let c = Client::new(&s.uri(), Some("auk_x".into())).unwrap();
        let p = prepare(&c, &s.uri(), AGENT, &key, &d.join("cfg"), NOW)
            .await
            .unwrap();
        assert_eq!(p.private_key, private);
        // (Running as root would read it anyway; the point is that the code path has no open of it:
        // the file stays untouched and its mode unchanged.)
        assert_eq!(
            std::fs::metadata(&private).unwrap().permissions().mode() & 0o777,
            0o000
        );
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            std::fs::read_to_string(&private).unwrap(),
            "PRIVATE KEY MATERIAL\n"
        );
    }

    #[tokio::test]
    async fn refusals_read_in_plain_language() {
        let d = tmp("errors");
        let key = pub_file(&d);
        let run = |s: &MockServer| {
            let c = Client::new(&s.uri(), Some("auk_x".into())).unwrap();
            let uri = s.uri();
            let key = key.clone();
            let dir = d.join("cfg");
            async move {
                prepare(&c, &uri, AGENT, &key, &dir, NOW)
                    .await
                    .unwrap_err()
                    .to_string()
            }
        };
        // SSH off.
        let off = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/ssh"))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_json(json!({"error": {"code": "ssh_disabled", "message": "m"}})),
            )
            .mount(&off)
            .await;
        assert_eq!(
            run(&off).await,
            "SSH is not enabled on this Aurora instance"
        );
        // No terminator yet.
        let none = server(0, json!([])).await;
        assert!(run(&none).await.contains("no SSH server is running"));
        // Key not registered.
        let unreg = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/ssh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "host": "h", "port": 22, "cert_ttl_secs": 900, "terminators_live": 1,
                "host_keys": [], "known_hosts": ["h ssh-ed25519 AAAA"]})))
            .mount(&unreg)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(422).set_body_json(
                json!({"error": {"code": "ssh_key_not_registered", "message": "m"}}),
            ))
            .mount(&unreg)
            .await;
        let m = run(&unreg).await;
        assert!(m.contains("Account › SSH keys"), "{m}");
        // A missing or non-ed25519 key file.
        let c = Client::new(&unreg.uri(), Some("auk_x".into())).unwrap();
        let e = prepare(
            &c,
            "https://a",
            AGENT,
            &d.join("nope.pub"),
            &d.join("cfg"),
            NOW,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(e.contains("ssh-keygen -t ed25519"), "{e}");
    }

    #[test]
    fn shell_words_are_quoted_only_when_needed() {
        assert_eq!(shell_word("a-b/c.d"), "a-b/c.d");
        assert_eq!(shell_word("a b"), "'a b'");
        assert_eq!(shell_word("it's"), "'it'\\''s'");
    }
}
