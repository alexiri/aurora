//! (P5-GUIDE-6, `docs/24 §10a`) `list`, `search` and `describe` for app targets. Pure functions of
//! the parsed `api.json` files: no network, no model. Every line is one line, bounded.
use aurora_core::limits::AGENT_CLI_PREVIEW_MAX;

use super::api::{ApiDoc, AppFolder, Operation, Pagination, Param};
use super::text::{bound, oneline};

const SUMMARY_MAX: usize = 100;

fn access_cell(doc: &ApiDoc, op: &Operation) -> String {
    if op.access.is_empty() {
        return String::new();
    }
    let first = op.access.values().next();
    if op.access.values().all(|v| Some(v) == first) && doc.accounts.len() <= 1 {
        return format!(" [{}]", oneline(first.map_or("", String::as_str), 20));
    }
    format!(
        " [{}]",
        op.access
            .iter()
            .map(|(k, v)| format!("{}: {}", oneline(k, 8), oneline(v, 20)))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn accounts_cell(doc: &ApiDoc) -> String {
    doc.accounts
        .iter()
        .map(|a| format!("{}={}", oneline(&a.letter, 8), oneline(&a.label, 40)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `list` without a target: one line per connected app.
#[must_use]
pub fn list_apps(apps: &[AppFolder]) -> String {
    if apps.is_empty() {
        return "no connected apps in .aurora/apps/\n".to_owned();
    }
    let mut o = String::new();
    for a in apps {
        match &a.api {
            Ok(d) => {
                let ops = d.operations.len();
                o.push_str(&format!(
                    "{} — {} · accounts: {} · {} operation{}, {} skill{}\n",
                    oneline(&d.app.id, 48),
                    oneline(&d.app.name, 60),
                    accounts_cell(d),
                    ops,
                    if ops == 1 { "" } else { "s" },
                    d.skills.len(),
                    if d.skills.len() == 1 { "" } else { "s" },
                ));
            }
            Err(e) => o.push_str(&format!("{} — {}\n", oneline(&a.id, 48), oneline(e, 120))),
        }
    }
    bound(&o)
}

/// `list <app>`: its operations and skills.
#[must_use]
pub fn list_app(doc: &ApiDoc) -> String {
    let mut o = format!(
        "{} ({}) · accounts: {}\n",
        oneline(&doc.app.name, 60),
        oneline(&doc.app.id, 48),
        accounts_cell(doc)
    );
    if doc.truncated {
        o.push_str("note: api.json was cut to fit its size limit; parameters of some operations are missing\n");
    }
    for op in &doc.operations {
        o.push_str(&format!(
            "{} {} {} — {}{}\n",
            oneline(&op.id, 64),
            oneline(&op.method, 8),
            oneline(&op.path, 80),
            oneline(&op.summary, SUMMARY_MAX),
            access_cell(doc, op)
        ));
    }
    for s in &doc.skills {
        o.push_str(&format!(
            "skill {} — {}\n",
            oneline(&s.name, 64),
            oneline(&s.description, SUMMARY_MAX)
        ));
    }
    // A long list is cut by lines, never mid-line; the agent narrows with `search`.
    bound_lines(&o, "use `aurora-agent search <words> --in <app>` to narrow")
}

fn bound_lines(s: &str, hint: &str) -> String {
    if s.chars().count() <= AGENT_CLI_PREVIEW_MAX {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut shown = 0usize;
    let total = s.lines().count();
    for l in s.lines() {
        if out.chars().count() + l.chars().count() + 1 + hint.len() + 40 > AGENT_CLI_PREVIEW_MAX {
            break;
        }
        out.push_str(l);
        out.push('\n');
        shown += 1;
    }
    out.push_str(&format!("… {} of {total} lines shown; {hint}\n", shown));
    out
}

pub(super) fn score(terms: &[String], weighted: &[(&str, u32)]) -> u32 {
    let mut total = 0;
    for t in terms {
        // A term counts once, at the best field it appears in.
        let best = weighted
            .iter()
            .filter(|(h, _)| h.to_lowercase().contains(t.as_str()))
            .map(|(_, w)| *w)
            .max()
            .unwrap_or(0);
        total += best;
    }
    total
}

/// `search`: ranks operations, catalogue entries and app skills across apps (or one).
#[must_use]
pub fn search(apps: &[AppFolder], words: &[String], only: Option<&str>, limit: usize) -> String {
    search_with(apps, &[], words, only, limit)
}

/// [`search`] with the platform tools ranked beside the apps' operations (P5-GUIDE-7): they are
/// in the ranking when `only` is none or `aurora`.
#[must_use]
pub fn search_with(
    apps: &[AppFolder],
    platform: &[aurora_protocol::tools::ToolDescriptor],
    words: &[String],
    only: Option<&str>,
    limit: usize,
) -> String {
    let terms: Vec<String> = words
        .iter()
        .flat_map(|w| w.split_whitespace())
        .map(str::to_lowercase)
        .collect();
    if terms.is_empty() {
        return "search: give one or more words\n".to_owned();
    }
    let mut hits: Vec<(u32, String, String, String)> = Vec::new();
    if only.is_none_or(|o| o == super::PLATFORM_TARGET) {
        hits.extend(super::platform::search_hits(platform, &terms));
    }
    for a in apps {
        let Ok(d) = &a.api else { continue };
        if only.is_some_and(|o| o != d.app.id) {
            continue;
        }
        for op in &d.operations {
            let tags = op.tags.join(" ");
            // (C-911) A webhook is found by its note and its slot names too.
            let notes = op.notes.as_deref().unwrap_or_default();
            let slots = if op.account.is_some() {
                op.body
                    .iter()
                    .flat_map(|b| b.fields.iter().map(|f| f.name.as_str()))
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                String::new()
            };
            let s = score(
                &terms,
                &[
                    (&op.id, 6),
                    (&tags, 4),
                    (&op.summary, 3),
                    (notes, 2),
                    (&slots, 2),
                    (&op.path, 2),
                    (&d.app.name, 1),
                ],
            );
            if s > 0 {
                hits.push((
                    s,
                    d.app.id.clone(),
                    op.id.clone(),
                    format!(
                        "{} {} {} {} — {}{}",
                        oneline(&d.app.id, 48),
                        oneline(&op.id, 64),
                        oneline(&op.method, 8),
                        oneline(&op.path, 80),
                        oneline(&op.summary, SUMMARY_MAX),
                        access_cell(d, op)
                    ),
                ));
            }
        }
        // (C-1135) Skill names weigh 5, descriptions 3.
        for sk in &d.skills {
            let s = score(&terms, &[(&sk.name, 5), (&sk.description, 3)]);
            if s > 0 {
                hits.push((
                    s,
                    d.app.id.clone(),
                    format!("~{}", sk.name),
                    format!(
                        "{} skill {} — {}",
                        oneline(&d.app.id, 48),
                        oneline(&sk.name, 64),
                        oneline(&sk.description, SUMMARY_MAX)
                    ),
                ));
            }
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| (&a.1, &a.2).cmp(&(&b.1, &b.2))));
    if hits.is_empty() {
        return "no match; try other words or `aurora-agent list`\n".to_owned();
    }
    let total = hits.len();
    let mut o: String = hits
        .iter()
        .take(limit)
        .map(|h| format!("{}\n", h.3))
        .collect();
    if total > limit {
        o.push_str(&format!(
            "… {} more; raise --limit or narrow the words\n",
            total - limit
        ));
    }
    bound_lines(&o, "narrow the words")
}

fn param_line(p: &Param, desc_max: usize) -> String {
    let mut o = format!("  {} (", oneline(&p.name, 64));
    if let Some(l) = &p.location {
        o.push_str(&format!("{}, ", oneline(l, 8)));
    }
    o.push_str(&oneline(&p.ty, 12));
    if p.is_required() {
        o.push_str(", required");
    }
    o.push(')');
    if let Some(v) = &p.values {
        let vs: Vec<String> = v.iter().take(12).map(|x| oneline(&scalar(x), 30)).collect();
        o.push_str(&format!(" one of {}", vs.join("|")));
    }
    if let Some(d) = &p.default {
        o.push_str(&format!(" default {}", oneline(&scalar(d), 30)));
    }
    if desc_max > 0
        && let Some(d) = &p.description
    {
        o.push_str(&format!(" — {}", oneline(d, desc_max)));
    }
    o.push('\n');
    o
}

fn scalar(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The method writes (a body is expected).
fn writes(method: &str) -> bool {
    matches!(
        method.to_ascii_uppercase().as_str(),
        "POST" | "PUT" | "PATCH"
    )
}

/// (FIX-239) The command that calls a catalogue entry: it has no name `call` accepts, so the
/// example gives its method and path (`*` shown as `<id>`), and `--body @<file>` for a write.
pub(crate) fn catalogue_example(app: &str, op: &Operation) -> String {
    let mut o = format!(
        "aurora-agent call {app} --method {} --path {}",
        oneline(&op.method, 8).to_ascii_uppercase(),
        oneline(&op.path, 160).replace('*', "<id>")
    );
    if writes(&op.method) {
        o.push_str(" --body @<file.json>");
    }
    o
}

fn example(app: &str, op: &Operation) -> String {
    if op.is_catalogue() {
        return catalogue_example(app, op);
    }
    let mut o = format!("aurora-agent call {app} {}", oneline(&op.id, 64));
    // (FIX-239) A body no field describes (nested JSON) is passed whole.
    if writes(&op.method) && op.body.as_ref().is_some_and(|b| b.fields.is_empty()) {
        o.push_str(" --body @<file.json>");
    }
    for p in op
        .params
        .iter()
        .chain(op.body.iter().flat_map(|b| b.fields.iter()))
        .filter(|p| p.is_required())
    {
        if p.ty == "json" {
            // (C-911) A JSON field is passed with `:=`.
            o.push_str(&format!(" {}:=<json>", oneline(&p.name, 64)));
            continue;
        }
        let v = p
            .default
            .as_ref()
            .or_else(|| p.values.as_ref().and_then(|v| v.first()))
            .map_or_else(|| format!("<{}>", oneline(&p.ty, 12)), scalar);
        let v = if v.contains(char::is_whitespace) {
            format!("'{}'", oneline(&v, 40).replace('\'', ""))
        } else {
            oneline(&v, 40)
        };
        o.push_str(&format!(" {}={}", oneline(&p.name, 64), v));
    }
    o
}

fn describe_with(doc: &ApiDoc, op: &Operation, desc_max: usize) -> String {
    let host = op
        .host
        .clone()
        .or_else(|| {
            doc.accounts
                .iter()
                .find_map(|a| a.base_url.clone())
                .map(|u| u.trim_start_matches("https://").to_owned())
        })
        .unwrap_or_else(|| "(the account's host)".to_owned());
    let mut o = format!(
        "{} {} ({})\n{} https://{}{}\n",
        oneline(&doc.app.id, 48),
        oneline(&op.id, 64),
        if op.is_catalogue() {
            "catalogue entry: call it with an explicit --path"
        } else {
            "package operation"
        },
        oneline(&op.method, 8),
        oneline(&host, 120),
        oneline(&op.path, 160),
    );
    if !op.summary.is_empty() {
        o.push_str(&format!("summary: {}\n", oneline(&op.summary, 200)));
    }
    if let Some(n) = op.notes.as_deref().filter(|n| !n.is_empty()) {
        o.push_str(&format!("notes: {}\n", oneline(n, 1000)));
    }
    if let Some(a) = &op.account {
        o.push_str(&format!(
            "account: {} (this operation is bound to it; --account is not needed)\n",
            oneline(a, 8)
        ));
    }
    match &op.tool {
        Some(t) => o.push_str(&format!("tool: {}\n", oneline(t, 64))),
        None => o.push_str("tool: none (a request that matches no catalogue operation)\n"),
    }
    if !op.access.is_empty() {
        o.push_str(&format!(
            "access (advisory): {}\n",
            op.access
                .iter()
                .map(|(k, v)| format!("{}: {}", oneline(k, 8), oneline(v, 20)))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if doc.truncated {
        o.push_str("note: api.json was cut to fit its size limit; parameters may be missing\n");
    }
    if !op.params.is_empty() {
        o.push_str("parameters:\n");
        for p in &op.params {
            o.push_str(&param_line(p, desc_max));
        }
    }
    if let Some(b) = &op.body {
        o.push_str(&format!("body ({}):\n", oneline(&b.content_type, 60)));
        for f in &b.fields {
            o.push_str(&param_line(f, desc_max));
        }
    }
    match &op.pagination {
        Some(Pagination::LinkHeader) => {
            o.push_str("pagination: Link header (--all follows rel=next)\n")
        }
        Some(Pagination::PageParam { param, start, .. }) => o.push_str(&format!(
            "pagination: page number in `{}` from {} (--all)\n",
            oneline(param, 40),
            start.unwrap_or(1)
        )),
        Some(Pagination::Cursor { param, next }) => o.push_str(&format!(
            "pagination: cursor in `{}` from {} (--all)\n",
            oneline(param, 40),
            oneline(next, 60)
        )),
        None => {}
    }
    if let Some(i) = &op.items {
        o.push_str(&format!("items at: {}\n", oneline(i, 60)));
    }
    o.push_str(&format!("example: {}\n", example(&doc.app.id, op)));
    // (C-911) A webhook can be checked without sending: the dry run shows the rendered body.
    if op.account.is_some() && op.params.iter().any(|p| p.name == "X-Aurora-Dry-Run") {
        o.push_str(&format!(
            "dry run (sends nothing): {} X-Aurora-Dry-Run=1\n",
            example(&doc.app.id, op)
        ));
    }
    o
}

/// `describe <app> <name>`: an operation (descriptions are dropped first when it would exceed the
/// bound), or a skill.
pub fn describe(doc: &ApiDoc, name: &str) -> Result<String, String> {
    if let Some(op) = doc.operation(name) {
        for desc_max in [160, 60, 0] {
            let s = describe_with(doc, op, desc_max);
            if s.chars().count() <= AGENT_CLI_PREVIEW_MAX {
                return Ok(s);
            }
        }
        return Ok(bound(&describe_with(doc, op, 0)));
    }
    if let Some(sk) = doc.skills.iter().find(|s| s.name == name) {
        return Ok(format!(
            "{} skill {} — {}\nRead it: /workspace/{}\n",
            oneline(&doc.app.id, 48),
            oneline(&sk.name, 64),
            oneline(&sk.description, 120),
            oneline(&sk.path, 120),
        ));
    }
    let near: Vec<&str> = doc
        .operations
        .iter()
        .map(|o| o.id.as_str())
        .filter(|id| id.contains(name) || name.contains(id))
        .take(5)
        .collect();
    Err(format!(
        "{}: no operation or skill named {:?}{}",
        doc.app.id,
        oneline(name, 64),
        if near.is_empty() {
            "; `aurora-agent list <app>` shows them".to_owned()
        } else {
            format!("; did you mean {}?", near.join(", "))
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn fixture() -> Vec<AppFolder> {
        let json = serde_json::json!({
            "format": "aurora.app-api/1",
            "app": {"id": "indico", "name": "Indico", "version": "1"},
            "accounts": [{"letter": "A", "connection_id": "c_1", "label": "CERN Indico",
                          "base_url": "https://indico.example.test", "grant": "custom"}],
            "operations": [
              {"id": "search_rooms", "method": "GET", "path": "/rooms/api/rooms/search",
               "host": null, "summary": "Rooms free in a building for a time slot", "tags": ["rooms"],
               "params": [
                 {"name": "building", "in": "query", "required": true, "type": "string", "description": "Building number"},
                 {"name": "repeat_frequency", "in": "query", "required": true, "type": "string",
                  "enum": ["NEVER", "DAY"], "default": "NEVER"}],
               "body": null, "pagination": null, "items": "$", "tool": "search_rooms",
               "access": {"A": "allowed"}, "source": "package"},
              {"id": "book_room", "method": "POST", "path": "/rooms/api/rooms/{id}/book", "host": null,
               "summary": "Book a room", "tags": ["rooms", "booking"],
               "params": [{"name": "id", "in": "path", "required": true, "type": "integer"}],
               "body": {"content_type": "application/json", "fields": [
                 {"name": "reason", "type": "string", "required": true}]},
               "pagination": null, "items": null, "tool": null, "access": {"A": "blocked"}, "source": "package"},
              {"id": "list_events", "method": "GET", "path": "/events/*", "host": null,
               "summary": "Events", "tags": [], "params": [], "body": null, "pagination": null,
               "items": null, "tool": "list_events", "access": {"A": "approval"}, "source": "catalogue"}
            ],
            "skills": [{"name": "indico-book-free-room", "description": "Find a free room and book it", "path": ".claude/skills/indico-book-free-room/SKILL.md"}],
            "recipes": [],
            "untooled": {"A": "blocked"}
        });
        vec![
            AppFolder {
                id: "indico".into(),
                api: ApiDoc::parse(json.to_string().as_bytes()),
            },
            AppFolder {
                id: "dbapp".into(),
                api: Err("no api.json for this app; read .aurora/apps/dbapp/app.md".into()),
            },
        ]
    }

    #[test]
    fn list_shows_apps_and_operations() {
        let f = fixture();
        insta::assert_snapshot!("list_apps", list_apps(&f));
        insta::assert_snapshot!("list_app", list_app(f[0].api.as_ref().unwrap()));
    }

    #[test]
    fn search_ranks_names_above_summaries_and_includes_skills() {
        let f = fixture();
        let s = search(&f, &["book".into(), "room".into()], None, 15);
        insta::assert_snapshot!("search_book_room", &s);
        let first = s.lines().next().unwrap();
        assert!(first.contains("book_room"), "{first}");
        let limited = search(&f, &["room".into()], Some("indico"), 1);
        assert!(
            limited.contains("1 more") || limited.lines().count() == 2,
            "{limited}"
        );
        assert!(search(&f, &["zzz".into()], None, 15).starts_with("no match"));
        assert!(search(&f, &["rooms".into()], Some("other"), 15).starts_with("no match"));
    }

    #[test]
    fn describe_lists_parameters_and_an_example() {
        let f = fixture();
        let d = f[0].api.as_ref().unwrap();
        insta::assert_snapshot!(
            "describe_search_rooms",
            describe(d, "search_rooms").unwrap()
        );
        insta::assert_snapshot!("describe_book_room", describe(d, "book_room").unwrap());
        let ev = describe(d, "list_events").unwrap();
        assert!(ev.contains("explicit --path"), "{ev}");
        // (FIX-239) A catalogue entry's example is a command `call` accepts, not its bare name.
        assert!(
            ev.contains("example: aurora-agent call ") && ev.contains(" --path "),
            "{ev}"
        );
        assert!(
            !ev.contains("example: aurora-agent call indico list_events"),
            "{ev}"
        );
        assert!(
            describe(d, "indico-book-free-room")
                .unwrap()
                .contains("Read it: /workspace/.claude/skills/indico-book-free-room/SKILL.md")
        );
        assert!(
            describe(d, "book")
                .unwrap_err()
                .contains("did you mean book_room")
        );
    }

    #[test]
    fn describe_stays_under_the_bound_dropping_descriptions_first() {
        let long = "d".repeat(190);
        let params: Vec<_> = (0..40)
            .map(|i| {
                serde_json::json!({"name": format!("p{i}"), "in": "query", "type": "string", "description": long})
            })
            .collect();
        let json = serde_json::json!({
            "format": "aurora.app-api/1", "app": {"id": "big", "name": "Big", "version": "1"},
            "operations": [{"id": "op", "method": "GET", "path": "/x", "summary": "s", "params": params, "source": "package"}]
        });
        let d = ApiDoc::parse(json.to_string().as_bytes()).unwrap();
        let s = describe(&d, "op").unwrap();
        assert!(
            s.chars().count() <= AGENT_CLI_PREVIEW_MAX,
            "{}",
            s.chars().count()
        );
        assert!(s.contains("p39"));
    }
}
