//! `~/.aurora/links.json` — the calendar links (secret-link apps, P5-ICS, D-273, C-843) an agent may
//! read. The home sync's `render_inputs.links` lists one entry per active connection of a
//! secret-link app the agent holds a grant on; the supervisor writes `[{ name, app, url }]` where
//! `url` is the connection's **synthetic address** `https://<connection_id>.link.aurora.internal/`.
//! The real link is a secret that never leaves the gateway and never enters the sandbox: nothing
//! here (or in the sync that feeds it) carries it. The file is `0600` and removed when the list is
//! empty.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::nofollow;

/// The file, relative to the home.
pub const LINKS_FILE: &str = ".aurora/links.json";
/// The suffix of a secret-link connection's synthetic host (`aurora_registry`'s
/// `SECRET_LINK_HOST_SUFFIX`; the supervisor does not depend on the registry crate).
const HOST_SUFFIX: &str = ".link.aurora.internal";
/// Most entries written (a bound against a malformed sync).
const LINKS_MAX: usize = 256;
/// Characters of a name kept.
const NAME_MAX: usize = 200;

/// One `links` entry of the `render_inputs` part.
#[derive(Debug, Clone, Deserialize)]
pub struct LinkInput {
    pub name: String,
    pub app_id: String,
    pub connection_id: Uuid,
}

/// One entry of `links.json`.
#[derive(Debug, Serialize)]
struct LinkOut {
    name: String,
    app: String,
    url: String,
}

/// The `links` of an installed `render_inputs.json`.
pub fn installed_links(raw: &[u8]) -> Vec<LinkInput> {
    serde_json::from_slice::<Value>(raw)
        .ok()
        .and_then(|v| v.get("links").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn clean(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .take(NAME_MAX)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The file's text. Pure, for tests. Two links with one name keep both entries: the address
/// tells them apart.
pub fn render(links: &[LinkInput]) -> String {
    let out: Vec<LinkOut> = links
        .iter()
        .take(LINKS_MAX)
        .map(|l| LinkOut {
            name: clean(&l.name),
            app: clean(&l.app_id),
            url: format!("https://{}{HOST_SUFFIX}/", l.connection_id),
        })
        .collect();
    let mut text = serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".into());
    text.push('\n');
    text
}

/// Writes `root/.aurora/links.json` (`0600`) or, with no links, removes it; no link in the
/// home is followed.
pub fn write(root: &Path, links: &[LinkInput]) -> std::io::Result<()> {
    let path = Path::new(LINKS_FILE);
    if links.is_empty() {
        return nofollow::remove_file_beneath(root, path);
    }
    nofollow::write_beneath(root, path, render(links).as_bytes(), 0o600)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    fn link(name: &str, id: Uuid) -> LinkInput {
        LinkInput {
            name: name.into(),
            app_id: "ics-calendar".into(),
            connection_id: id,
        }
    }

    #[test]
    fn the_file_holds_the_synthetic_address_and_nothing_secret() {
        let id = Uuid::now_v7();
        let v: Value = serde_json::from_str(&render(&[link("Private calendar", id)])).unwrap();
        assert_eq!(v[0]["name"], "Private calendar");
        assert_eq!(v[0]["app"], "ics-calendar");
        assert_eq!(v[0]["url"], format!("https://{id}.link.aurora.internal/"));
        assert_eq!(v[0].as_object().unwrap().len(), 3);
    }

    #[test]
    fn a_duplicated_name_keeps_both_entries_told_apart_by_address() {
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        let v: Value = serde_json::from_str(&render(&[link("Work", a), link("Work", b)])).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert_ne!(v[0]["url"], v[1]["url"]);
    }

    #[test]
    fn control_characters_in_a_name_are_dropped() {
        let v: Value =
            serde_json::from_str(&render(&[link("a\nb\u{0}c", Uuid::now_v7())])).unwrap();
        assert_eq!(v[0]["name"], "abc");
    }

    #[test]
    fn written_0600_replaced_and_removed_when_the_list_empties() {
        let d = std::env::temp_dir().join(format!("aurora-links-{}", Uuid::now_v7()));
        let file = d.join(LINKS_FILE);
        write(&d, &[link("One", Uuid::now_v7())]).unwrap();
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let first = fs::read_to_string(&file).unwrap();
        write(&d, &[link("Two", Uuid::now_v7())]).unwrap();
        assert_ne!(first, fs::read_to_string(&file).unwrap());
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        write(&d, &[]).unwrap();
        assert!(!file.exists());
        // Removing what is not there is fine.
        write(&d, &[]).unwrap();
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn links_are_read_from_the_installed_render_inputs() {
        let id = Uuid::now_v7();
        let raw = serde_json::json!({
            "links": [{"name": "N", "app_id": "ics-calendar", "connection_id": id}],
            "databases": []
        })
        .to_string();
        let l = installed_links(raw.as_bytes());
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].connection_id, id);
        assert!(installed_links(b"{}").is_empty());
        assert!(installed_links(b"not json").is_empty());
    }
}
