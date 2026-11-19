#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! P5-POLISH-3 (D-387, C-1031): the text packages carry for people. Every builtin package describes each of
//! its apps (`about.summary`, `about.first_prompt`) and, where its token screen asks the person to tick
//! permissions, says which for each access level; changing only that text never widens permissions.
use aurora_registry::diff::widens;
use aurora_registry::manifest::Connect;
use aurora_registry::{Manifest, validate};

fn builtin_ids() -> Vec<String> {
    let dir = format!("{}/../../plugins/builtin", env!("CARGO_MANIFEST_DIR"));
    let mut ids: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().join("plugin.yaml").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    ids.sort();
    ids
}

fn read(id: &str) -> (String, Manifest) {
    let y = std::fs::read_to_string(format!(
        "{}/../../plugins/builtin/{id}/plugin.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let m = Manifest::parse(&y, "9.9.9").unwrap_or_else(|e| panic!("{id}: {e:?}"));
    (y, m)
}

/// The token methods whose app's own token screen asks for permissions to tick, by app id and method id.
const TICKED: &[(&str, &str)] = &[
    ("github", "token"),
    ("github-ghes", "token"),
    ("gitlab", "token"),
    ("sentry", "token"),
    ("indico", "token"),
];
// (Jira and Confluence Data Center personal access tokens carry the person's own permissions and show no
// permission picker; the passwords and links of CalDAV, ICS and PostgreSQL have none either.)

#[test]
fn every_builtin_app_has_a_summary_and_a_first_prompt() {
    let mut apps = 0;
    for id in builtin_ids() {
        // `webhook` is another lane's package (P5-WEBHOOK-1); it carries its own text.
        if id == "webhook" {
            continue;
        }
        let (_, m) = read(&id);
        for a in &m.apps {
            apps += 1;
            let s = a.about.summary.as_deref().unwrap_or_default();
            let p = a.about.first_prompt.as_deref().unwrap_or_default();
            assert!(
                !s.is_empty() && s.chars().count() <= 100,
                "{id}/{}: summary of 1 to 100 characters, got {s:?}",
                a.about.id
            );
            assert!(
                !p.is_empty() && p.chars().count() <= 120,
                "{id}/{}: first_prompt of 1 to 120 characters, got {p:?}",
                a.about.id
            );
        }
    }
    assert!(apps >= 19, "the builtin catalogue lost apps: {apps}");
}

#[test]
fn token_methods_that_ask_for_permissions_say_which_for_read_and_write() {
    let mut seen = 0;
    for id in builtin_ids() {
        let (_, m) = read(&id);
        for a in &m.apps {
            for c in &a.connect {
                if !TICKED.contains(&(a.about.id.as_str(), c.id())) {
                    continue;
                }
                let Connect::Fields(f) = c else {
                    panic!(
                        "{}/{}: a token method is a `fields` method",
                        a.about.id,
                        c.id()
                    );
                };
                seen += 1;
                for level in ["read", "write"] {
                    let l = f
                        .access_levels
                        .iter()
                        .find(|l| l.id == level)
                        .unwrap_or_else(|| panic!("{}/{}: no `{level}` level", a.about.id, c.id()));
                    assert!(
                        !l.permission_hints.is_empty(),
                        "{}/{} {level}: permission_hints",
                        a.about.id,
                        c.id()
                    );
                }
            }
        }
    }
    assert_eq!(seen, TICKED.len(), "every listed token method exists");
}

#[test]
fn every_builtin_manifest_is_valid_with_its_text() {
    for id in builtin_ids() {
        let (_, m) = read(&id);
        let v = validate::manifest(&m);
        assert!(v.is_empty(), "{id}: {v:#?}");
    }
}

/// Text for people is presentation: a version that only adds or changes it is not a widening, so a release
/// that rewords a summary or a hint needs no re-approval (C-1031).
#[test]
fn changing_only_the_text_never_widens_permissions() {
    let (y, old) = read("github");
    let reworded = y.replacen("summary:", "summary: Reworded. ", 1).replacen(
        "permission_hints: [",
        "permission_hints: [\"Extra · Read\", ",
        1,
    );
    assert_ne!(reworded, y, "the github manifest carries text to change");
    let new = Manifest::parse(&reworded, "9.9.9").unwrap();
    assert!(widens(&old, &new).is_empty());
}
