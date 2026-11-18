//! `~/.aurora/servers.json` — the servers of the person-pinned apps (`own_server`, CalDAV, P5-CALDAV,
//! D-275, C-845) an agent may use. The home sync's `render_inputs.servers` lists one entry per
//! active connection of such an app the agent holds a grant on; the supervisor writes
//! `[{ name, app, url }]` where `url` is the server address Aurora discovered (the calendar home).
//! The address is visible to the agent on purpose; the credential never is: nothing here, or in the
//! sync that feeds it, carries one. The file is `0600` and removed when the list is empty.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The file, relative to the home.
pub const SERVERS_FILE: &str = ".aurora/servers.json";
/// Most entries written (a bound against a malformed sync).
const SERVERS_MAX: usize = 256;
/// Characters of a name kept.
const NAME_MAX: usize = 200;
/// Characters of a URL kept (longer ones are dropped, not cut).
const URL_MAX: usize = 2_048;

/// One `servers` entry of the `render_inputs` part.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerInput {
    pub name: String,
    pub app_id: String,
    pub url: String,
}

/// One entry of `servers.json`.
#[derive(Debug, Serialize)]
struct ServerOut {
    name: String,
    app: String,
    url: String,
}

/// The `servers` of an installed `render_inputs.json`.
pub fn installed_servers(raw: &[u8]) -> Vec<ServerInput> {
    serde_json::from_slice::<Value>(raw)
        .ok()
        .and_then(|v| v.get("servers").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn clean(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The file's text. Pure, for tests. Only `https` URLs without userinfo are listed: the file is
/// not a place for anything but an address.
pub fn render(servers: &[ServerInput]) -> String {
    let out: Vec<ServerOut> = servers
        .iter()
        .filter(|s| {
            s.url.len() <= URL_MAX
                && s.url.starts_with("https://")
                && !s.url.chars().any(char::is_control)
                && !s
                    .url
                    .trim_start_matches("https://")
                    .split('/')
                    .next()
                    .unwrap_or("")
                    .contains('@')
        })
        .take(SERVERS_MAX)
        .map(|s| ServerOut {
            name: clean(&s.name, NAME_MAX),
            app: clean(&s.app_id, NAME_MAX),
            url: s.url.clone(),
        })
        .collect();
    let mut text = serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".into());
    text.push('\n');
    text
}

/// Writes `root/.aurora/servers.json` (`0600`) or, with no servers, removes it.
pub fn write(root: &Path, servers: &[ServerInput]) -> std::io::Result<()> {
    let path = root.join(SERVERS_FILE);
    if servers.is_empty() {
        return match fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let _ = fs::remove_file(&tmp);
    let mut f = fs::File::create(&tmp)?;
    f.write_all(render(servers).as_bytes())?;
    f.set_permissions(fs::Permissions::from_mode(0o600))?;
    f.sync_all()?;
    fs::rename(&tmp, &path)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn server(name: &str, url: &str) -> ServerInput {
        ServerInput {
            name: name.into(),
            app_id: "caldav".into(),
            url: url.into(),
        }
    }

    #[test]
    fn the_file_holds_name_app_and_url_only() {
        let v: Value = serde_json::from_str(&render(&[server(
            "alice at dav.example.test",
            "https://dav.example.test/dav/calendars/alice/",
        )]))
        .unwrap();
        assert_eq!(v[0]["name"], "alice at dav.example.test");
        assert_eq!(v[0]["app"], "caldav");
        assert_eq!(v[0]["url"], "https://dav.example.test/dav/calendars/alice/");
        assert_eq!(v[0].as_object().unwrap().len(), 3);
    }

    #[test]
    fn anything_but_a_plain_https_address_is_dropped() {
        let text = render(&[
            server("a", "http://dav.example.test/"),
            server("b", "https://user:pw@dav.example.test/"),
            server("c", "https://dav.example.test/\nx"),
            server(
                "d",
                &format!("https://dav.example.test/{}", "a".repeat(2_100)),
            ),
            server("ok", "https://dav.example.test/dav/"),
        ]);
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1, "{text}");
        assert_eq!(v[0]["name"], "ok");
    }

    #[test]
    fn installed_servers_reads_the_render_inputs_and_tolerates_garbage() {
        let raw = br#"{"servers":[{"name":"n","app_id":"caldav","url":"https://x.test/","connection_id":"ignored"}]}"#;
        assert_eq!(installed_servers(raw).len(), 1);
        assert!(installed_servers(b"not json").is_empty());
        assert!(installed_servers(br#"{"servers":"no"}"#).is_empty());
        assert!(installed_servers(br#"{}"#).is_empty());
    }
}
