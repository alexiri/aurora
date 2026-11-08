//! libpq files for the database lane (P2-DBL-2, C-231; `docs/21 §Phase 2 — Database lane`).
//!
//! For each database connection granted to the agent the home projection holds a `[service]`
//! section, so `psql service=<name>` needs nothing on the command line:
//!
//! - `.aurora/pg/pg_service.conf` (`PGSERVICEFILE`, 0644, no secret): host and port of the
//!   **gateway** (the same sandbox-side address the boot document names), user = connection id,
//!   `sslmode=verify-full` against the gateway CA;
//! - `.aurora/pg/pgpass` (`PGPASSFILE`, 0600): the **gateway credential** of this sandbox (the
//!   boot credential), never the database password, which no sandbox ever receives;
//! - `.aurora/pg/root.crt` (`PGSSLROOTCERT`, 0644): the gateway CA.
//!
//! The files are regenerated at every boot and every home sync, so a new boot credential replaces
//! the old one. TLS verification is never relaxed: the gateway presents the proxy listener's
//! certificate (host in its SAN set) or a CA leaf for `<connection_id>.db.aurora.internal`.

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::Path;

use serde::Deserialize;
use uuid::Uuid;

/// Directory of the files, relative to the home.
pub const PG_DIR: &str = ".aurora/pg";
const SERVICE_FILE: &str = "pg_service.conf";
const PASS_FILE: &str = "pgpass";
const ROOT_FILE: &str = "root.crt";
/// Most services written (the API sends far fewer; a bound against a malformed sync).
const SERVICES_MAX: usize = 256;

/// How a sandbox reaches the gateway, from the boot document.
#[derive(Clone)]
pub struct PgGateway {
    /// Host the proxy certificate is verified against (name or IP, no brackets).
    pub host: String,
    pub port: u16,
    /// The sandbox's gateway credential (`aub_...`).
    pub credential: String,
    pub ca_pem: String,
}

impl std::fmt::Debug for PgGateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgGateway")
            .field("host", &self.host)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl PgGateway {
    /// From the boot document's `gateway.proxy` (`host:port`, IPv6 in brackets) and its
    /// verification name.
    pub fn from_boot(
        proxy: &str,
        verify_name: &str,
        credential: &str,
        ca_pem: &str,
    ) -> Option<Self> {
        let port = proxy.rsplit_once(':')?.1.parse().ok()?;
        Some(Self {
            host: verify_name.to_owned(),
            port,
            credential: credential.to_owned(),
            ca_pem: ca_pem.to_owned(),
        })
    }
}

/// One `databases` entry of the `render_inputs` part.
#[derive(Debug, Clone, Deserialize)]
pub struct PgDatabase {
    pub service: String,
    pub connection_id: Uuid,
}

fn service_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 96
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// A value of the service file: refuses anything that could add a key or a section.
fn value_ok(s: &str) -> bool {
    !s.is_empty()
        && !s
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\'' || c == '"')
}

/// A `pgpass` field: `\` and `:` escaped.
fn pass_field(s: &str) -> String {
    s.replace('\\', "\\\\").replace(':', "\\:")
}

fn write_file(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    // Created 0600: the file never exists with wider permissions than its final mode.
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(bytes)?;
    f.set_permissions(fs::Permissions::from_mode(mode))?;
    f.sync_all()?;
    fs::rename(&tmp, path)
}

/// The three file contents. Pure, for tests.
pub fn render_files(
    root: &Path,
    gw: &PgGateway,
    dbs: &[PgDatabase],
) -> Option<(String, String, String)> {
    if !value_ok(&gw.host) || gw.credential.is_empty() {
        return None;
    }
    let dir = root.join(PG_DIR);
    let dir = dir.display().to_string();
    if !value_ok(&dir) {
        return None;
    }
    let mut service = String::from("# Managed by Aurora. Do not edit.\n");
    let mut pass = String::new();
    let mut seen: Vec<&str> = Vec::new();
    for d in dbs.iter().take(SERVICES_MAX) {
        if !service_ok(&d.service) || seen.contains(&d.service.as_str()) {
            tracing::warn!(service = %d.service.escape_debug(), "database service refused");
            continue;
        }
        seen.push(&d.service);
        let _ = write!(
            service,
            "\n[{svc}]\nhost={host}\nport={port}\nuser={user}\ndbname={svc}\nsslmode=verify-full\n\
             sslrootcert={dir}/{ROOT_FILE}\npassfile={dir}/{PASS_FILE}\nconnect_timeout=15\n",
            svc = d.service,
            host = gw.host,
            port = gw.port,
            user = d.connection_id,
        );
        let _ = writeln!(
            pass,
            "{}:{}:*:{}:{}",
            pass_field(&gw.host),
            gw.port,
            d.connection_id,
            pass_field(&gw.credential)
        );
    }
    Some((service, pass, gw.ca_pem.clone()))
}

/// Writes (or, without databases, removes) the files under `root/.aurora/pg`.
pub fn write(root: &Path, gw: &PgGateway, dbs: &[PgDatabase]) -> std::io::Result<()> {
    let dir = root.join(PG_DIR);
    if dbs.is_empty() {
        let _ = fs::remove_dir_all(&dir);
        return Ok(());
    }
    let Some((service, pass, ca)) = render_files(root, gw, dbs) else {
        tracing::warn!("gateway address unusable for libpq files; none written");
        let _ = fs::remove_dir_all(&dir);
        return Ok(());
    };
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755))?;
    write_file(&dir.join(SERVICE_FILE), service.as_bytes(), 0o644)?;
    write_file(&dir.join(ROOT_FILE), ca.as_bytes(), 0o644)?;
    write_file(&dir.join(PASS_FILE), pass.as_bytes(), 0o600)
}

/// Environment of every agent-run process: where libpq finds the files. Only the variables whose
/// files exist matter to `psql service=...`; setting them unconditionally is harmless.
pub fn env(root: &Path) -> Vec<(String, String)> {
    let dir = root.join(PG_DIR);
    let p = |f: &str| dir.join(f).display().to_string();
    vec![
        ("PGSERVICEFILE".into(), p(SERVICE_FILE)),
        ("PGPASSFILE".into(), p(PASS_FILE)),
        ("PGSSLROOTCERT".into(), p(ROOT_FILE)),
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn gw() -> PgGateway {
        PgGateway {
            host: "aurora-gateway".into(),
            port: 4102,
            credential: "aub_fixture".into(),
            ca_pem: "-----BEGIN CERTIFICATE-----\nx\n-----END CERTIFICATE-----\n".into(),
        }
    }

    #[test]
    fn renders_service_pass_and_never_a_database_password() {
        let id = Uuid::now_v7();
        let dbs = [PgDatabase {
            service: "prod".into(),
            connection_id: id,
        }];
        let (svc, pass, ca) = render_files(Path::new("/workspace"), &gw(), &dbs).unwrap();
        assert!(
            svc.contains("[prod]\nhost=aurora-gateway\nport=4102\n"),
            "{svc}"
        );
        assert!(svc.contains(&format!("user={id}\n")));
        assert!(svc.contains("sslmode=verify-full"));
        assert!(svc.contains("sslrootcert=/workspace/.aurora/pg/root.crt"));
        assert!(svc.contains("passfile=/workspace/.aurora/pg/pgpass"));
        assert!(!svc.contains("password"));
        assert_eq!(pass, format!("aurora-gateway:4102:*:{id}:aub_fixture\n"));
        assert!(ca.contains("BEGIN CERTIFICATE"));
    }

    #[test]
    fn refuses_unsafe_service_names_and_hosts() {
        let id = Uuid::now_v7();
        let dbs = [
            PgDatabase {
                service: "x]\n[evil".into(),
                connection_id: id,
            },
            PgDatabase {
                service: "ok".into(),
                connection_id: id,
            },
            PgDatabase {
                service: "ok".into(),
                connection_id: id,
            },
        ];
        let (svc, _, _) = render_files(Path::new("/workspace"), &gw(), &dbs).unwrap();
        assert!(!svc.contains("evil"));
        assert_eq!(svc.matches("[ok]").count(), 1);
        let mut bad = gw();
        bad.host = "h\nsslmode=disable".into();
        assert!(render_files(Path::new("/workspace"), &bad, &dbs).is_none());
    }

    #[test]
    fn pgpass_escapes_colon_and_backslash() {
        assert_eq!(pass_field("a:b\\c"), "a\\:b\\\\c");
    }

    #[test]
    fn from_boot_parses_host_and_port() {
        let g =
            PgGateway::from_boot("aurora-gateway:4102", "aurora-gateway", "aub_x", "ca").unwrap();
        assert_eq!((g.host.as_str(), g.port), ("aurora-gateway", 4102));
        let g = PgGateway::from_boot("[::1]:4102", "::1", "aub_x", "ca").unwrap();
        assert_eq!((g.host.as_str(), g.port), ("::1", 4102));
        assert!(PgGateway::from_boot("nohostport", "n", "t", "ca").is_none());
    }

    #[test]
    fn write_sets_modes_and_removes_when_empty() {
        let d = std::env::temp_dir().join(format!("aurora-pg-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&d).unwrap();
        let dbs = [PgDatabase {
            service: "prod".into(),
            connection_id: Uuid::now_v7(),
        }];
        write(&d, &gw(), &dbs).unwrap();
        let mode = |f: &str| {
            std::fs::metadata(d.join(PG_DIR).join(f))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode(PASS_FILE), 0o600);
        assert_eq!(mode(SERVICE_FILE), 0o644);
        write(&d, &gw(), &[]).unwrap();
        assert!(!d.join(PG_DIR).exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
