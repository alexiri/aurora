//! libpq service file for the database lane (P2-DBL-2, C-232; `docs/21 §Phase 2 — Database lane`).
//!
//! For each database connection granted to the agent the home projection holds a `[service]`
//! section in `.aurora/pg/pg_service.conf` (`PGSERVICEFILE`, 0644), so `psql service=<name>` needs
//! nothing on the command line. Every section points at the **loopback relay** of the supervisor
//! (`crate::pgrelay`, 127.0.0.1 only, `sslmode=disable` on that leg): user = connection id, no
//! password. There is no password file and no CA file: the relay holds the gateway credential and
//! verifies the gateway's TLS itself, so nothing in the sandbox's files or environment is a
//! credential. A `pgpass` or `root.crt` left by an earlier version is removed on the next sync.

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use serde::Deserialize;
use uuid::Uuid;

/// Directory of the files, relative to the home.
pub const PG_DIR: &str = ".aurora/pg";
const SERVICE_FILE: &str = "pg_service.conf";
/// Files an earlier version wrote; deleted on every sync.
const STALE_FILES: [&str; 3] = ["pgpass", "root.crt", "pg_service.conf.tmp"];
/// Most services written (the API sends far fewer; a bound against a malformed sync).
const SERVICES_MAX: usize = 256;

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

/// The service file. Pure, for tests.
pub fn render_service(relay_port: u16, dbs: &[PgDatabase]) -> String {
    let mut service = String::from("# Managed by Aurora. Do not edit.\n");
    let mut seen: Vec<&str> = Vec::new();
    for d in dbs.iter().take(SERVICES_MAX) {
        if !service_ok(&d.service) || seen.contains(&d.service.as_str()) {
            tracing::warn!(service = %d.service.escape_debug(), "database service refused");
            continue;
        }
        seen.push(&d.service);
        let _ = write!(
            service,
            "\n[{svc}]\nhost=127.0.0.1\nport={relay_port}\nuser={user}\ndbname={svc}\nsslmode=disable\n\
             connect_timeout=15\n",
            svc = d.service,
            user = d.connection_id,
        );
    }
    service
}

fn write_file(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    let mut f = fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.set_permissions(fs::Permissions::from_mode(mode))?;
    f.sync_all()?;
    fs::rename(&tmp, path)
}

/// Writes (or, without databases, removes) the service file under `root/.aurora/pg`.
pub fn write(root: &Path, relay_port: u16, dbs: &[PgDatabase]) -> std::io::Result<()> {
    let dir = root.join(PG_DIR);
    if dbs.is_empty() {
        let _ = fs::remove_dir_all(&dir);
        return Ok(());
    }
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755))?;
    for f in STALE_FILES {
        let _ = fs::remove_file(dir.join(f));
    }
    write_file(
        &dir.join(SERVICE_FILE),
        render_service(relay_port, dbs).as_bytes(),
        0o644,
    )
}

/// Environment of every agent-run process: where libpq finds the service file.
pub fn env(root: &Path) -> Vec<(String, String)> {
    vec![(
        "PGSERVICEFILE".into(),
        root.join(PG_DIR).join(SERVICE_FILE).display().to_string(),
    )]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_loopback_service_and_no_secret() {
        let id = Uuid::now_v7();
        let dbs = [PgDatabase {
            service: "prod".into(),
            connection_id: id,
        }];
        let svc = render_service(3130, &dbs);
        assert!(svc.contains("[prod]\nhost=127.0.0.1\nport=3130\n"), "{svc}");
        assert!(svc.contains(&format!("user={id}\n")));
        assert!(svc.contains("sslmode=disable"));
        for banned in ["password", "passfile", "sslrootcert", "aub_", "auc_"] {
            assert!(!svc.contains(banned), "{banned} in {svc}");
        }
    }

    #[test]
    fn refuses_unsafe_and_duplicate_service_names() {
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
        let svc = render_service(3130, &dbs);
        assert!(!svc.contains("evil"));
        assert_eq!(svc.matches("[ok]").count(), 1);
    }

    #[test]
    fn write_removes_stale_password_files_and_the_dir_when_empty() {
        let d = std::env::temp_dir().join(format!("aurora-pg-{}", Uuid::now_v7()));
        let dir = d.join(PG_DIR);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("pgpass"), "h:1:*:u:aub_old\n").unwrap();
        fs::write(dir.join("root.crt"), "ca").unwrap();
        let dbs = [PgDatabase {
            service: "prod".into(),
            connection_id: Uuid::now_v7(),
        }];
        write(&d, 3130, &dbs).unwrap();
        assert!(!dir.join("pgpass").exists() && !dir.join("root.crt").exists());
        let mode = fs::metadata(dir.join(SERVICE_FILE))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o644);
        write(&d, 3130, &[]).unwrap();
        assert!(!dir.exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn only_the_service_file_variable_is_set() {
        let e = env(Path::new("/workspace"));
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].0, "PGSERVICEFILE");
    }
}
