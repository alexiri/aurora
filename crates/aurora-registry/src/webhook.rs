//! (P5-WEBHOOK-1, D-320, C-909) The webhook presets of a send-mode secret-link package: the archive
//! files `presets/<file>.json`, each a [`WebhookPreset`], validated with the one implementation in
//! `aurora-webhook` (a failure is `webhook_preset_invalid`), and the loader the API serves them from.
use std::collections::BTreeSet;

use aurora_core::limits::WEBHOOK_CONFIG_MAX;
use aurora_webhook::{WebhookPreset, validate as validate_config};

use crate::Violation;
use crate::manifest::{Manifest, PRESETS_DIR, SecretLinkMode};
use crate::package::Package;
use crate::validate::plain_dns_name;

/// Longest preset id and display name; most hosts a preset suggests itself for.
const ID_MAX: usize = 41;
const NAME_MAX: usize = 60;
const HOSTS_MAX: usize = 8;

fn id_ok(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= ID_MAX
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn bad(path: &str, msg: impl Into<String>) -> Violation {
    Violation::new("webhook_preset_invalid", path, msg)
}

/// Parse and check one preset file.
fn check_preset(path: &str, bytes: &[u8]) -> Result<WebhookPreset, Violation> {
    if bytes.len() > WEBHOOK_CONFIG_MAX * 2 {
        return Err(bad(path, "preset file is too large"));
    }
    let p: WebhookPreset = serde_json::from_slice(bytes)
        .map_err(|e| bad(path, format!("not a webhook preset: {e}")))?;
    if !id_ok(&p.id) {
        return Err(bad(path, "id is lowercase letters, digits and `-` (≤ 41)"));
    }
    if p.name.is_empty() || p.name.chars().count() > NAME_MAX {
        return Err(bad(path, "name is 1..=60 characters"));
    }
    if p.suggest_for_hosts.len() > HOSTS_MAX
        || !p.suggest_for_hosts.iter().all(|h| plain_dns_name(h))
    {
        return Err(bad(
            path,
            "suggest_for_hosts: at most 8 plain lowercase DNS names",
        ));
    }
    if let Err(errs) = validate_config(&p.config, &p.secret_value_names) {
        let list: Vec<String> = errs
            .iter()
            .map(|e| format!("{} {}", e.path, e.code))
            .collect();
        return Err(bad(path, format!("config refused: {}", list.join(", "))));
    }
    Ok(p)
}

/// Archive rules: every listed preset exists and is valid with a unique id, and no file under
/// `presets/` is unlisted.
pub fn archive_rules(m: &Manifest, pkg: &Package, out: &mut Vec<Violation>) {
    let mut listed: BTreeSet<String> = BTreeSet::new();
    for a in &m.apps {
        let Some(s) = &a.secret_link else { continue };
        if s.mode() != SecretLinkMode::Send {
            continue;
        }
        let mut ids = BTreeSet::new();
        for f in s.presets.iter().flatten() {
            let path = format!("{PRESETS_DIR}{f}");
            listed.insert(path.clone());
            let Some(bytes) = pkg.file(&path) else {
                out.push(bad(
                    &path,
                    "listed in secret_link.presets but not in the archive",
                ));
                continue;
            };
            match check_preset(&path, bytes) {
                Ok(p) => {
                    if !ids.insert(p.id.clone()) {
                        out.push(bad(&path, format!("duplicate preset id `{}`", p.id)));
                    }
                }
                Err(v) => out.push(v),
            }
        }
    }
    for path in pkg.files.keys() {
        if path.starts_with(PRESETS_DIR) && !listed.contains(path) {
            out.push(bad(
                path,
                "file is not listed in the secret_link.presets of a send-mode app",
            ));
        }
    }
}

/// The presets of each send-mode app of a (validated) package, in the order listed: what the API
/// serves for `GET /v1/apps/{app}/webhook-presets` from the installed package version.
#[must_use]
pub fn presets(m: &Manifest, pkg: &Package) -> Vec<(String, Vec<WebhookPreset>)> {
    let mut out = Vec::new();
    for a in &m.apps {
        let Some(s) = &a.secret_link else { continue };
        if s.mode() != SecretLinkMode::Send {
            continue;
        }
        let list = s
            .presets
            .iter()
            .flatten()
            .filter_map(|f| pkg.file(&format!("{PRESETS_DIR}{f}")))
            .filter_map(|b| serde_json::from_slice(b).ok())
            .collect();
        out.push((a.about.id.clone(), list));
    }
    out
}
