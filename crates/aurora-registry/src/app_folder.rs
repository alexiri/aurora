//! (P5-GUIDE-1, P5-APPSKILL-2, `docs/24 §5`, C-646, C-647, C-1135) The app folder renderer: the files
//! Aurora writes into an app's folder in the agent's home, `app.md`, `tools.md` and `api.json`.
//!
//! A **pure function** (no I/O, no database type): the API calls it for the home sync and
//! `aurora plugin preview` calls it for authors, so an author previews exactly what an agent will
//! read. Every string that comes from a package, a connection label or a host is untrusted: it
//! goes through the one-line sanitizer ([`aurora_core::sanitize::inline`]) or, for the essentials,
//! a code fence longer than any backtick run it holds ([`aurora_core::sanitize::fenced`]). Text
//! Aurora writes is never mixed into a third-party block and the reverse.
//!
//! Access (`allowed` / `approval` / `blocked` / `unavailable`) is derived **from the grant row**
//! alone (D-171(d)) and is advisory (SEC-91): the gateway decides every request, so the table can
//! be more permissive than the gateway, never less.
use std::collections::BTreeSet;
use std::fmt::Write as _;

use aurora_core::limits::AGENT_SNIPPET_MAX;
use aurora_core::sanitize::{fenced, inline};
use aurora_policy::HostPattern;

mod api_json;

use crate::manifest::{GitKind, GraphqlKind, INSTANCE_HOST, ParamCondition, ParamSource, ToolItem};
use crate::snapshot::{AppEntry, HostEntry};

const NAME_MAX: usize = 120;
const LABEL_MAX: usize = 120;
const LINK_MAX: usize = 300;
const DESCRIPTION_MAX: usize = 200;
const FIELD_MAX: usize = 200;

/// One account of the agent on the app: a granted, active connection.
#[derive(Debug, Clone, PartialEq)]
pub struct AppAccount {
    pub id: String,
    pub label: String,
    /// Where requests go: the instance host, the tenant host or the app's default host.
    pub host: Option<String>,
    /// libpq service name of a database connection (`psql service=<name>`, C-231).
    pub service: Option<String>,
    pub access: GrantAccess,
    /// (P5-WEBHOOK-3, C-911) The webhook of a send-mode connection: what the agent may know of it.
    pub webhook: Option<AppWebhook>,
}

/// (P5-WEBHOOK-3, C-911) The render input of one send-mode account: the name, the person's note,
/// the method and the typed slots. **Never** the template, the headers, the URL or any secret
/// value: this type has no field for them (a test serializes it and checks the keys).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct AppWebhook {
    pub name: String,
    pub description: String,
    pub method: String,
    pub slots: Vec<aurora_webhook::Slot>,
}

/// The grant row's mode (`grants.mode`, `allow_tools`, `ask_tools`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantAccess {
    Full,
    Custom {
        allow: BTreeSet<String>,
        ask: BTreeSet<String>,
    },
}

/// What the agent may do with one tool (`24 §5.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Allowed,
    Approval,
    Blocked,
    Unavailable,
}

impl Access {
    /// The word printed in `tools.md`.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Approval => "approval",
            Self::Blocked => "blocked",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Which tier's skill of the same name the agent uses instead of an app skill (C-1133).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowTier {
    Agent,
    Workspace,
    Organization,
}

impl ShadowTier {
    fn word(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Workspace => "workspace",
            Self::Organization => "organization",
        }
    }
}

/// One skill of the app as the agent's home sees it (`description` and `name` are third-party and
/// pass the one-line sanitizer on the way out).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillLine {
    pub name: String,
    pub description: String,
    /// `Some` when another tier's skill of the name is used instead (this one is not projected).
    pub shadowed_by: Option<ShadowTier>,
}

/// The app's skills for one agent (C-1133, C-1134): what `app.md` lists and `api.json` carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppSkills {
    /// In name order.
    pub lines: Vec<SkillLine>,
    /// The home-sync budget left this app's skills out (`APP_SKILLS_SYNC_MAX`): `app.md` says so.
    pub left_out: bool,
    /// The harness skills folder relative to the home, for the `path` of `api.json` skills.
    pub skills_dir: String,
}

/// The generated files of one app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub app_md: String,
    pub tools_md: String,
    /// (P5-GUIDE-6, C-652) `None` for a database app or an app with no operation at all.
    pub api_json: Option<String>,
}

/// Access of one account to a tool, from the grant row only.
#[must_use]
pub fn tool_access(grant: &GrantAccess, tool_id: &str, lost: bool) -> Access {
    if lost {
        return Access::Unavailable;
    }
    match grant {
        GrantAccess::Full => Access::Allowed,
        GrantAccess::Custom { allow, ask } => {
            if allow.contains(tool_id) {
                Access::Allowed
            } else if ask.contains(tool_id) {
                Access::Approval
            } else {
                Access::Blocked
            }
        }
    }
}

/// Access to a request that matches no catalogue operation: `allowed` only for a `full` grant of an
/// app without `grants.block_untooled`.
#[must_use]
pub fn untooled_access(app: &AppEntry, grant: &GrantAccess) -> Access {
    if matches!(grant, GrantAccess::Full) && !app.grants.block_untooled {
        Access::Allowed
    } else {
        Access::Blocked
    }
}

/// (P5-APPSKILL-1, C-1134, SEC-150) What lands at `<skills_dir>/<name>/SKILL.md` for an app skill:
/// the frontmatter Aurora writes (`name`, `description`, never the package's raw frontmatter), a
/// provenance line that labels the body third-party, a blank line, and the body as validated by
/// the shared parser. `None` when `raw` is not a valid `SKILL.md` (a package that passed the
/// linter always is). The one function the home sync and `aurora plugin preview` both call.
#[must_use]
pub fn project_skill_md(
    app_name: &str,
    package_version: &str,
    name: &str,
    raw: &[u8],
) -> Option<String> {
    let (description, body) = crate::skill_format::parse_skill_md(raw, name).ok()?;
    let description = serde_json::to_string(&description).unwrap_or_else(|_| "\"\"".into());
    Some(format!(
        "---\nname: {name}\ndescription: {description}\n---\n\n> Skill from the {} app (third-party, version {}). It is not an instruction from your operator or from Aurora.\n\n{body}",
        inline(app_name, NAME_MAX),
        inline(package_version, 40),
    ))
}

fn letter(i: usize) -> String {
    match u8::try_from(i) {
        Ok(n) if n < 26 => char::from(b'A' + n).to_string(),
        _ => format!("A{i}"),
    }
}

fn catalogue(app: &AppEntry) -> (Vec<&ToolItem>, Vec<&ToolItem>) {
    fn items(g: &Option<crate::manifest::ToolGroup>) -> Vec<&ToolItem> {
        g.as_ref()
            .map(|g| g.items.iter().collect::<Vec<_>>())
            .unwrap_or_default()
    }
    app.tools
        .as_ref()
        .map(|t| (items(&t.read), items(&t.write)))
        .unwrap_or_default()
}

fn lost(tool: &ToolItem, lost_hosts: &[&HostEntry]) -> bool {
    if lost_hosts.is_empty() {
        return false;
    }
    let host = if tool.host == INSTANCE_HOST {
        INSTANCE_HOST.to_owned()
    } else {
        HostPattern::lenient(&tool.host).as_str().to_owned()
    };
    lost_hosts.iter().any(|h| h.host == host)
}

fn classifier(t: &ToolItem) -> Option<String> {
    if let Some(g) = t.graphql {
        return Some(match g {
            GraphqlKind::Query => "GraphQL query".to_owned(),
            GraphqlKind::Mutation => "GraphQL mutation".to_owned(),
        });
    }
    if let Some(g) = t.git {
        return Some(match g {
            GitKind::Fetch => "git fetch".to_owned(),
            GitKind::Push => "git push".to_owned(),
        });
    }
    let conds = t.params.as_ref().filter(|p| !p.is_empty())?;
    Some(format!(
        "when {}",
        conds
            .iter()
            .map(condition)
            .collect::<Vec<_>>()
            .join(" and ")
    ))
}

fn condition(c: &ParamCondition) -> String {
    let source = match c.source {
        ParamSource::Query => "query",
        ParamSource::Form => "form",
        ParamSource::Header => "header",
    };
    let m = &c.matcher;
    let (op, value) = if let Some(v) = &m.equals {
        ("equals", inline(v, FIELD_MAX))
    } else if let Some(v) = &m.any_of {
        (
            "is one of",
            v.iter()
                .map(|x| inline(x, FIELD_MAX))
                .collect::<Vec<_>>()
                .join(", "),
        )
    } else if let Some(v) = &m.prefix {
        ("starts with", inline(v, FIELD_MAX))
    } else {
        ("matches", String::new())
    };
    format!("{source} {} {op} {value}", inline(&c.name, 64))
        .trim_end()
        .to_owned()
}

fn sentence(name: &str) -> String {
    let n = inline(name, NAME_MAX);
    if n.ends_with(['.', '!', '?']) {
        n
    } else {
        format!("{n}.")
    }
}

/// The access column of a tool: one word when every account agrees, else `A: …; B: …`.
fn access_cell(states: &[Access]) -> Option<String> {
    let first = *states.first()?;
    if states.iter().all(|s| *s == first) {
        return Some(first.word().to_owned());
    }
    Some(
        states
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{}: {}", letter(i), s.word()))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

fn grant_word(g: &GrantAccess) -> &'static str {
    match g {
        GrantAccess::Full => "full access",
        GrantAccess::Custom { .. } => "custom access",
    }
}

/// (P5-WEBHOOK-3, C-911) One section per webhook account: its note, its slots and the `curl`
/// example to its `base_url` (the fallback without `aurora-agent`). Only what the render input
/// holds: never the template, headers, URL or secret values.
fn webhooks_md(o: &mut String, accounts: &[AppAccount]) {
    let ids = api_json::webhook_ids(accounts);
    for (i, (a, id)) in accounts.iter().zip(ids).enumerate() {
        let (Some(w), Some(id)) = (&a.webhook, id) else {
            continue;
        };
        let _ = write!(
            o,
            "\n## Webhook: {} (account {}, `{id}`)\n",
            inline(&w.name, 80),
            letter(i)
        );
        let notes = inline(&w.description, 1000);
        if !notes.is_empty() {
            let _ = writeln!(o, "\n{notes}");
        }
        if w.slots.is_empty() {
            o.push_str("\nNo slots: the message is fixed.\n");
        } else {
            o.push_str("\n| slot | type | required | notes |\n|---|---|---|---|\n");
            for s in &w.slots {
                let ty = match s.ty {
                    aurora_webhook::SlotType::StringList => "string list",
                    aurora_webhook::SlotType::Enum => "enum",
                    aurora_webhook::SlotType::Json => "json",
                    aurora_webhook::SlotType::Integer => "integer",
                    aurora_webhook::SlotType::Number => "number",
                    aurora_webhook::SlotType::Boolean => "boolean",
                    aurora_webhook::SlotType::String => "string",
                };
                let mut notes = api_json::slot_description(s);
                if let Some(v) = &s.r#enum {
                    let vs: Vec<String> = v.iter().map(|x| inline(x, 40)).collect();
                    if !notes.is_empty() {
                        notes.push_str("; ");
                    }
                    let _ = write!(notes, "one of {}", vs.join(", "));
                }
                let _ = writeln!(
                    o,
                    "| `{}` | {ty} | {} | {} |",
                    inline(&s.name, 64).replace('|', "/"),
                    if s.required { "yes" } else { "no" },
                    notes.replace('|', "/")
                );
            }
        }
        let body: String = w
            .slots
            .iter()
            .filter(|s| s.required)
            .map(|s| {
                let v = match s.ty {
                    aurora_webhook::SlotType::Integer | aurora_webhook::SlotType::Number => "1",
                    aurora_webhook::SlotType::Boolean => "true",
                    aurora_webhook::SlotType::StringList => "[\"…\"]",
                    aurora_webhook::SlotType::Json => "{}",
                    _ => "\"…\"",
                };
                format!(
                    "\"{}\":{v}",
                    inline(&s.name, 64).replace(['"', '\\', '\''], "")
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let _ = write!(
            o,
            "\n```\ncurl -sS -X {} https://{}/ -H 'Content-Type: application/json' -d '{{{body}}}'\n```\nAdd `-H 'X-Aurora-Dry-Run: 1'` to check the body without sending.\n",
            inline(&w.method, 8),
            api_json::webhook_host(&a.id)
        );
    }
}

fn tools_md(app: &AppEntry, lost_hosts: &[&HostEntry], accounts: &[AppAccount]) -> String {
    let name = inline(&app.about.name, NAME_MAX);
    let mut o = format!(
        "# {name}: operations and your access\n\nWritten by Aurora from the app's catalogue and your grants. The gateway decides every request;\nworkspace and organization policy can be stricter than this list, never looser.\n"
    );
    if !accounts.is_empty() {
        o.push_str("\n## Accounts\n");
        for (i, a) in accounts.iter().enumerate() {
            let _ = writeln!(
                o,
                "- {}: `{}` {} ({})",
                letter(i),
                inline(&a.id, 64),
                inline(&a.label, LABEL_MAX),
                grant_word(&a.access)
            );
        }
    }
    webhooks_md(&mut o, accounts);
    let (read, write) = catalogue(app);
    let hosts: BTreeSet<&str> = read.iter().chain(&write).map(|t| t.host.as_str()).collect();
    let show_host = hosts.len() > 1;
    if read.is_empty() && write.is_empty() {
        o.push_str("\nThis app has no catalogue operations.\n");
    }
    for (title, group) in [("Read", &read), ("Write", &write)] {
        if group.is_empty() {
            continue;
        }
        let _ = write!(o, "\n## {title}\n");
        for t in group {
            let l = lost(t, lost_hosts);
            let states: Vec<Access> = accounts
                .iter()
                .map(|a| tool_access(&a.access, &t.id, l))
                .collect();
            let methods = if t.methods.is_empty() {
                "ANY".to_owned()
            } else {
                t.methods
                    .iter()
                    .map(|m| inline(m, 16))
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let host = if show_host {
                if t.host == INSTANCE_HOST {
                    "<instance host> ".to_owned()
                } else {
                    format!("{} ", inline(&t.host, 120))
                }
            } else {
                String::new()
            };
            let class = classifier(t).map(|c| format!(" ({c})")).unwrap_or_default();
            let desc = t
                .description
                .as_deref()
                .map(|d| inline(d, DESCRIPTION_MAX))
                .filter(|d| !d.is_empty())
                .map(|d| {
                    if d.ends_with(['.', '!', '?']) {
                        format!(" {d}")
                    } else {
                        format!(" {d}.")
                    }
                })
                .unwrap_or_default();
            let access = access_cell(&states)
                .map(|c| format!(" [{c}]"))
                .unwrap_or_default();
            let _ = writeln!(
                o,
                "- `{}` {methods} {host}{}{class}: {}{desc}{access}",
                inline(&t.id, 64),
                inline(&t.path, 200),
                sentence(&t.name)
            );
        }
    }
    o.push_str("\n## Requests that match no operation above\n");
    let states: Vec<Access> = accounts
        .iter()
        .map(|a| untooled_access(app, &a.access))
        .collect();
    match access_cell(&states) {
        Some(c) => {
            let _ = writeln!(o, "[{c}]");
        }
        None => o.push_str("No account is granted.\n"),
    }
    o
}

fn counts(app: &AppEntry, lost_hosts: &[&HostEntry], a: &AppAccount) -> [usize; 4] {
    let (read, write) = catalogue(app);
    let mut n = [0usize; 4];
    for t in read.iter().chain(&write) {
        match tool_access(&a.access, &t.id, lost(t, lost_hosts)) {
            Access::Allowed => n[0] += 1,
            Access::Approval => n[1] += 1,
            Access::Blocked => n[2] += 1,
            Access::Unavailable => n[3] += 1,
        }
    }
    n
}

fn app_md(
    app: &AppEntry,
    lost_hosts: &[&HostEntry],
    accounts: &[AppAccount],
    skills: &AppSkills,
    has_api_json: bool,
) -> String {
    let name = inline(&app.about.name, NAME_MAX);
    let mut o = format!(
        "# {name} ({})\n\nWritten by Aurora for this agent, except the last section.\n",
        inline(&app.app_id, 64)
    );
    if !accounts.is_empty() {
        o.push_str(
            "\n## Accounts\n\nChoose an account with the request header `x-aurora-connection-id: <connection id>`. Without it the gateway uses your only account, or refuses when there are several.\n\n",
        );
        for a in accounts {
            let place = match (&a.service, &a.host) {
                (Some(s), _) => format!(" (database: `psql service={}`)", inline(s, 64)),
                (None, Some(h)) => format!(" (host {})", inline(h, 253)),
                (None, None) => String::new(),
            };
            let _ = writeln!(
                o,
                "- `{}`: {}{place}",
                inline(&a.id, 64),
                inline(&a.label, LABEL_MAX)
            );
        }
        o.push_str("\n## Your access\n\n");
        for a in accounts {
            let _ = write!(o, "- `{}`: ", inline(&a.id, 64));
            match &a.access {
                GrantAccess::Full => o.push_str("full access\n"),
                GrantAccess::Custom { .. } => {
                    let n = counts(app, lost_hosts, a);
                    let _ = write!(
                        o,
                        "custom access: {} allowed, {} need approval, {} blocked",
                        n[0], n[1], n[2]
                    );
                    if n[3] > 0 {
                        let _ = write!(o, ", {} unavailable", n[3]);
                    }
                    o.push('\n');
                }
            }
        }
        o.push_str("\n`tools.md` lists every operation and your access to it.\n");
    }
    let name_tp = inline(&app.about.name, NAME_MAX);
    if skills.left_out {
        let _ = write!(
            o,
            "\n## Skills\n\nThe skills of the {name_tp} app (third-party) were left out of your skills folder: they did not fit in what one sync can carry.\n"
        );
    } else if !skills.lines.is_empty() {
        let _ = write!(
            o,
            "\n## Skills\n\nSkills from the {name_tp} app (third-party), in your skills folder. Open the one that matches your task before you start.\n\n"
        );
        for l in &skills.lines {
            let name = inline(&l.name, 64);
            let by = l.shadowed_by.map_or_else(String::new, |t| {
                format!(
                    " — a skill of the same name from your {} is used instead",
                    t.word()
                )
            });
            let _ = writeln!(
                o,
                "- `{name}`: {} (`$SKILLS_DIR/{name}/SKILL.md`){by}",
                inline(&l.description, DESCRIPTION_MAX)
            );
        }
    }
    o.push_str(
        "\n## Files in this folder\n\n- `tools.md`: every operation and your access to it\n",
    );
    if has_api_json {
        o.push_str(
            "- `api.json`: the operations of this app, for `aurora-agent`. Use `aurora-agent search|describe|call <app_id> …` rather than reading it.\n",
        );
    }
    let docs: Vec<(&str, &str)> = [
        ("API documentation", app.about.api_docs.as_deref()),
        ("API specification", app.about.api_spec.as_deref()),
    ]
    .into_iter()
    .filter_map(|(k, v)| v.map(|v| (k, v)))
    .collect();
    if !docs.is_empty() {
        o.push_str("\n## Documentation\n\n");
        for (k, v) in docs {
            let _ = writeln!(o, "- {k}: {}", inline(v, LINK_MAX));
        }
    }
    if let Some(ag) = &app.agent
        && !ag.instructions.trim().is_empty()
    {
        let _ = write!(
            o,
            "\n## Notes from the {name_tp} plugin (third-party)\n\n{}",
            fenced(&ag.instructions, AGENT_SNIPPET_MAX)
        );
    }
    o
}

/// Render `app.md`, `tools.md` and `api.json` of one app for one agent (C-646, C-647, C-1135).
///
/// * `lost_hosts`: the snapshot's host entries of this app whose claim is `lost` (`09 §7`); a tool
///   on such a host is `unavailable`.
/// * `accounts`: the agent's granted, active connections, in the order the letters `A`, `B`, … are
///   given.
/// * `skills`: the app's skills for this agent (C-1133); `app.md` lists them, `api.json` carries the
///   ones that are projected.
///
/// `package_version` is printed in `api.json` only.
#[must_use]
pub fn render(
    app: &AppEntry,
    lost_hosts: &[&HostEntry],
    accounts: &[AppAccount],
    package_version: &str,
    skills: &AppSkills,
) -> Rendered {
    render_with_operations(app, lost_hosts, accounts, package_version, skills, None)
}

/// [`render`] with the app's parsed operations file (C-652): `api.json` then carries the package
/// operations next to the catalogue entries no operation maps to.
#[must_use]
pub fn render_with_operations(
    app: &AppEntry,
    lost_hosts: &[&HostEntry],
    accounts: &[AppAccount],
    package_version: &str,
    skills: &AppSkills,
    operations: Option<&crate::operations::OperationsFile>,
) -> Rendered {
    let api_json = api_json::render(
        app,
        lost_hosts,
        accounts,
        package_version,
        skills,
        operations,
    );
    Rendered {
        app_md: app_md(app, lost_hosts, accounts, skills, api_json.is_some()),
        tools_md: tools_md(app, lost_hosts, accounts),
        api_json,
    }
}
