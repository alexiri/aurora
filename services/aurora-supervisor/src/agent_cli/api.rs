//! (P5-GUIDE-6, `docs/24 §5.4`, C-652) The in-sandbox reader of `api.json`. The supervisor does
//! not depend on `aurora-registry`, so the shape is declared here, tolerant of unknown keys (a
//! newer API may add some) and strict about the `format` string. Nothing here is trusted for
//! anything but help: the gateway decides every request.
use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

pub const FORMAT: &str = "aurora.app-api/1";

#[derive(Debug, Clone, Deserialize)]
pub struct ApiDoc {
    pub format: String,
    pub app: AppInfo,
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub operations: Vec<Operation>,
    /// (C-1135) The app's projected skills; `recipes` of an older file is no longer read.
    #[serde(default)]
    pub skills: Vec<Skill>,
    #[serde(default)]
    pub untooled: BTreeMap<String, String>,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppInfo {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Account {
    pub letter: String,
    pub connection_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub grant: String,
}

/// A skill of the app in the agent's skills folder (C-1135): `path` is home-relative.
#[derive(Debug, Clone, Deserialize)]
pub struct Skill {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Operation {
    pub id: String,
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub params: Vec<Param>,
    #[serde(default)]
    pub body: Option<Body>,
    #[serde(default)]
    pub pagination: Option<Pagination>,
    #[serde(default)]
    pub items: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub access: BTreeMap<String, String>,
    #[serde(default)]
    pub source: String,
    /// (C-911) The account letter this operation is bound to (a webhook), else absent.
    #[serde(default)]
    pub account: Option<String>,
    /// (C-911) The person's note for the agent (a webhook), else absent.
    #[serde(default)]
    pub notes: Option<String>,
}

impl Operation {
    #[must_use]
    pub fn is_catalogue(&self) -> bool {
        self.source == "catalogue"
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Param {
    pub name: String,
    /// `path`, `query` or `header`; absent for a body field.
    #[serde(default, rename = "in")]
    pub location: Option<String>,
    #[serde(default)]
    pub required: Option<bool>,
    #[serde(default, rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "enum")]
    pub values: Option<Vec<Value>>,
    #[serde(default)]
    pub default: Option<Value>,
}

impl Param {
    #[must_use]
    pub fn is_required(&self) -> bool {
        self.required.unwrap_or(false)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Body {
    pub content_type: String,
    #[serde(default)]
    pub fields: Vec<Param>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "style", rename_all = "snake_case")]
pub enum Pagination {
    LinkHeader,
    PageParam {
        param: String,
        #[serde(default)]
        start: Option<u8>,
        #[serde(default)]
        size_param: Option<String>,
    },
    Cursor {
        param: String,
        next: String,
    },
}

impl ApiDoc {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let doc: Self = serde_json::from_slice(bytes).map_err(|e| format!("not valid: {e}"))?;
        if doc.format != FORMAT {
            return Err(format!(
                "unknown format {:?} (this aurora-agent reads {FORMAT})",
                doc.format
            ));
        }
        Ok(doc)
    }

    #[must_use]
    pub fn operation(&self, id: &str) -> Option<&Operation> {
        self.operations.iter().find(|o| o.id == id)
    }

    /// An account by letter (case-insensitive) or connection id.
    #[must_use]
    pub fn account(&self, key: &str) -> Option<&Account> {
        self.accounts
            .iter()
            .find(|a| a.connection_id == key || a.letter.eq_ignore_ascii_case(key))
    }
}

/// One app folder: its id and, when it has one, the parsed `api.json`.
#[derive(Debug)]
pub struct AppFolder {
    pub id: String,
    pub api: Result<ApiDoc, String>,
}

/// Reads every `<apps_dir>/<id>/api.json` (sorted by id). A folder without the file is listed with
/// an error saying so (a database app, an older API): the caller points to `app.md`.
#[must_use]
pub fn load_all(apps_dir: &Path) -> Vec<AppFolder> {
    let Ok(rd) = std::fs::read_dir(apps_dir) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    ids.sort();
    ids.into_iter()
        .map(|id| {
            let p = apps_dir.join(&id).join("api.json");
            let api = match std::fs::read(&p) {
                Ok(b) => ApiDoc::parse(&b),
                Err(_) => Err(format!(
                    "no api.json for this app; read .aurora/apps/{id}/app.md"
                )),
            };
            AppFolder { id, api }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (C-1135) The `ApiDoc` of the release before P5-APPSKILL-2, as it was (it read `recipes` and
    /// knew nothing of `skills`). The new `api.json` must still parse with it: an `aurora-agent` one
    /// release older keeps working against a newer API.
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct OldApiDoc {
        format: String,
        app: AppInfo,
        #[serde(default)]
        accounts: Vec<Account>,
        #[serde(default)]
        operations: Vec<Operation>,
        #[serde(default)]
        recipes: Vec<OldRecipe>,
        #[serde(default)]
        untooled: BTreeMap<String, String>,
        #[serde(default)]
        truncated: bool,
    }

    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct OldRecipe {
        file: String,
        #[serde(default)]
        title: String,
    }

    fn new_json() -> serde_json::Value {
        serde_json::json!({
            "format": "aurora.app-api/1",
            "app": {"id": "indico", "name": "Indico", "version": "1"},
            "accounts": [], "operations": [],
            "skills": [{"name": "indico-book", "description": "Book a room",
                        "path": ".claude/skills/indico-book/SKILL.md"}],
            "recipes": [], "untooled": {}
        })
    }

    #[test]
    fn the_new_api_json_parses_with_the_previous_releases_struct_and_with_this_one() {
        let bytes = new_json().to_string().into_bytes();
        let old: OldApiDoc = serde_json::from_slice(&bytes).unwrap();
        assert!(old.recipes.is_empty(), "{old:?}");
        let new = ApiDoc::parse(&bytes).unwrap();
        assert_eq!(new.skills.len(), 1);
        assert_eq!(new.skills[0].name, "indico-book");
        assert_eq!(new.skills[0].path, ".claude/skills/indico-book/SKILL.md");
        // A file of the previous release (recipes, no skills) still parses here: no skills, no crash.
        let prev = serde_json::json!({
            "format": "aurora.app-api/1",
            "app": {"id": "x", "name": "X", "version": "1"},
            "recipes": [{"file": "a.md", "title": "A"}]
        });
        let doc = ApiDoc::parse(prev.to_string().as_bytes()).unwrap();
        assert!(doc.skills.is_empty());
    }
}
