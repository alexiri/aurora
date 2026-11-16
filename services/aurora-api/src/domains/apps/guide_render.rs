//! The setup guide, filled in for the viewer (P5-UX-13, D-334, C-940).
//!
//! A package's `docs/setup.md` carries `{{aurora.<name>}}` placeholders (the set is
//! `aurora_registry::guide::PLACEHOLDERS`, checked when the package is ingested). The guide route
//! resolves what each one means for the caller, then [`render`] substitutes them in one literal
//! pass: a value is never scanned again, and every value is Markdown-escaped (a code span for the
//! callback URL and the external id, backslash escapes in link labels and names).
//!
//! Page placeholders become a link to that tab at the highest level the caller can open, or,
//! when they cannot open one, the sentence saying who can (`WhoCan`'s role and names).

use crate::app::AppState;
use crate::http::auth::Caller;
use crate::http::who_can::{self, WhoCanRole};
use aurora_core::{ApiError, FieldError, dash_paths};
use aurora_db::repo::{connections, contacts};
use aurora_registry::guide;
use uuid::Uuid;

/// What a placeholder stands for, for one caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A link the caller can open.
    Link { label: String, path: String },
    /// A page the caller cannot open: who can (role, at most three names).
    Ask {
        label: String,
        role: WhoCanRole,
        people: Vec<String>,
    },
}

/// Everything [`render`] substitutes.
#[derive(Debug, Clone)]
pub struct GuideValues {
    /// Absolute `<external_url>/v1/oauth/callback`.
    pub callback_url: String,
    /// `None` for someone who is not a member of the organization (or names none).
    pub external_id: Option<String>,
    /// `external_url`, joined to each path.
    pub base_url: String,
    pub app_setup: Target,
    pub oauth_clients: Target,
    pub self_hosted: Target,
    pub packages: Target,
}

const MEMBERS_ONLY: &str = "(shown to members of your organization)";

fn role_words(role: WhoCanRole) -> &'static str {
    match role {
        WhoCanRole::WorkspaceOwner => "a workspace owner",
        WhoCanRole::WorkspaceApprover => "a workspace approver",
        WhoCanRole::OrgAdmin => "an organization admin",
        WhoCanRole::OrgOwner => "an organization owner",
        WhoCanRole::InstanceAdmin => "an administrator of this Aurora instance",
    }
}

/// Backslash-escape the characters that mean something inside a Markdown line (CommonMark: a
/// backslash before ASCII punctuation is always legal); line breaks become spaces.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "\\`*_[]()<>#|!&~{}".contains(c) {
            out.push('\\');
        }
        if c == '\n' || c == '\r' {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

/// An inline code span holding `s` exactly.
pub fn code_span(s: &str) -> String {
    let s = s.replace(['\n', '\r'], " ");
    let mut longest = 0;
    let mut run = 0;
    for c in s.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat(longest + 1);
    let pad = if s.starts_with('`') || s.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{s}{pad}{fence}")
}

/// A URL that cannot end the Markdown link early.
fn link_url(u: &str) -> String {
    u.replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29")
        .replace('<', "%3C")
        .replace('>', "%3E")
}

fn target_text(t: &Target, base: &str) -> String {
    match t {
        Target::Link { label, path } => {
            let url = format!("{}{path}", base.trim_end_matches('/'));
            format!("[{}]({})", escape(label), link_url(&url))
        }
        Target::Ask {
            label,
            role,
            people,
        } => {
            let who = if people.is_empty() {
                role_words(*role).to_owned()
            } else {
                let names: Vec<_> = people.iter().map(|p| escape(p)).collect();
                format!("{}: {}", role_words(*role), names.join(", "))
            };
            format!("{} (ask {who})", escape(label))
        }
    }
}

/// Substitute the placeholders of `raw`. Unknown names (a package that predates the check) stay
/// as written.
pub fn render(raw: &str, v: &GuideValues) -> String {
    guide::fill(raw, |name| {
        Some(match name {
            "callback_url" => code_span(&v.callback_url),
            "external_id" => match &v.external_id {
                Some(id) => code_span(id),
                None => MEMBERS_ONLY.to_owned(),
            },
            "app_setup" => target_text(&v.app_setup, &v.base_url),
            "page:oauth_clients" => target_text(&v.oauth_clients, &v.base_url),
            "page:self_hosted" => target_text(&v.self_hosted, &v.base_url),
            "page:packages" => target_text(&v.packages, &v.base_url),
            _ => return None,
        })
    })
}

/// The level the guide is read at.
enum Level {
    /// No level named: the callback URL is the only value that needs none.
    None,
    Workspace {
        org: Uuid,
        workspace: Uuid,
        org_slug: String,
        ws_slug: String,
        owner: bool,
        org_admin: bool,
    },
    Org {
        org: Uuid,
        org_slug: String,
        admin: bool,
    },
}

async fn ask(
    state: &AppState,
    label: &str,
    role: WhoCanRole,
    org: Option<Uuid>,
    workspace: Option<Uuid>,
) -> Target {
    let people = match org {
        Some(org) => who_can::people(state, role, org, workspace)
            .await
            .into_iter()
            .map(|p| p.name)
            .collect(),
        None => Vec::new(),
    };
    Target::Ask {
        label: label.to_owned(),
        role,
        people,
    }
}

/// Resolve the values for `caller` at the level `workspace_id` or `org_id` names.
pub async fn resolve(
    state: &AppState,
    caller: &Caller,
    app_id: &str,
    app_name: &str,
    workspace_id: Option<Uuid>,
    org_id: Option<Uuid>,
) -> Result<GuideValues, ApiError> {
    let base = state.config.external_url.trim_end_matches('/').to_owned();
    let level = match (workspace_id, org_id) {
        (Some(ws), org) => {
            let a = caller.workspace(state, ws).await?;
            if org.is_some_and(|o| o != a.org_id) {
                return Err(ApiError::invalid_request(
                    "org_id does not match the workspace.",
                    vec![FieldError::new(
                        "/org_id",
                        "not the workspace's organization",
                    )],
                ));
            }
            let (org_slug, ws_slug) = contacts::slugs(&state.pool, a.org_id, Some(ws))
                .await?
                .ok_or_else(|| ApiError::not_found("Workspace not found."))?;
            Level::Workspace {
                org: a.org_id,
                workspace: ws,
                org_slug,
                ws_slug: ws_slug.unwrap_or_default(),
                owner: a.can_own(),
                org_admin: a.org_role.is_admin(),
            }
        }
        (None, Some(org)) => {
            let a = caller.org(state, org).await?;
            let (org_slug, _) = contacts::slugs(&state.pool, org, None)
                .await?
                .ok_or_else(|| ApiError::not_found("Organization not found."))?;
            Level::Org {
                org,
                org_slug,
                admin: a.role.is_admin(),
            }
        }
        (None, None) => Level::None,
    };
    let instance_admin = caller.is_instance_admin;
    let (org, workspace) = match &level {
        Level::None => (None, None),
        Level::Workspace { org, workspace, .. } => (Some(*org), Some(*workspace)),
        Level::Org { org, .. } => (Some(*org), None),
    };
    let external_id = match org {
        Some(org) => connections::org_external_id(&state.pool, org).await?,
        None => None,
    };
    let org_admin = matches!(
        level,
        Level::Workspace {
            org_admin: true,
            ..
        } | Level::Org { admin: true, .. }
    );
    let org_slug = match &level {
        Level::Workspace { org_slug, .. } | Level::Org { org_slug, .. } => Some(org_slug.clone()),
        Level::None => None,
    };

    let app_setup = match &level {
        Level::Workspace {
            org_slug,
            ws_slug,
            owner: true,
            ..
        } => Target::Link {
            label: format!("{app_name} setup"),
            path: dash_paths::ws_app_setup(org_slug, ws_slug, app_id),
        },
        Level::Org {
            org_slug,
            admin: true,
            ..
        } => Target::Link {
            label: format!("{app_name} setup"),
            path: dash_paths::org_app_setup(org_slug, app_id),
        },
        Level::Workspace { .. } => {
            ask(
                state,
                "the app's setup page",
                WhoCanRole::WorkspaceOwner,
                org,
                workspace,
            )
            .await
        }
        _ => {
            ask(
                state,
                "the app's setup page",
                WhoCanRole::OrgAdmin,
                org,
                None,
            )
            .await
        }
    };

    let oauth_clients = if instance_admin {
        Target::Link {
            label: "OAuth clients".into(),
            path: dash_paths::admin_oauth_clients(),
        }
    } else if let (true, Some(slug)) = (org_admin, &org_slug) {
        Target::Link {
            label: "OAuth clients".into(),
            path: dash_paths::org_oauth_clients(slug),
        }
    } else if let Level::Workspace {
        org_slug,
        ws_slug,
        owner: true,
        ..
    } = &level
    {
        // A workspace has no such tab: its owner registers a client on the app's setup page.
        Target::Link {
            label: format!("{app_name} setup"),
            path: dash_paths::ws_app_setup(org_slug, ws_slug, app_id),
        }
    } else {
        ask(state, "OAuth clients", WhoCanRole::OrgAdmin, org, None).await
    };

    let self_hosted = if instance_admin {
        Target::Link {
            label: "Self-hosted apps".into(),
            path: dash_paths::admin_self_hosted(),
        }
    } else if let (true, Some(slug)) = (org_admin, &org_slug) {
        Target::Link {
            label: "Self-hosted apps".into(),
            path: dash_paths::org_self_hosted(slug),
        }
    } else {
        ask(state, "Self-hosted apps", WhoCanRole::OrgAdmin, org, None).await
    };

    let packages = if instance_admin {
        Target::Link {
            label: "Packages".into(),
            path: dash_paths::admin_apps("packages"),
        }
    } else {
        ask(state, "Packages", WhoCanRole::InstanceAdmin, org, None).await
    };

    Ok(GuideValues {
        callback_url: format!("{base}/v1/oauth/callback"),
        external_id,
        base_url: base,
        app_setup,
        oauth_clients,
        self_hosted,
        packages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(external_id: Option<&str>) -> GuideValues {
        GuideValues {
            callback_url: "https://aurora.example/v1/oauth/callback".into(),
            external_id: external_id.map(str::to_owned),
            base_url: "https://aurora.example".into(),
            app_setup: Target::Link {
                label: "Demo setup".into(),
                path: "/o/acme/w/web/apps/demo".into(),
            },
            oauth_clients: Target::Ask {
                label: "OAuth clients".into(),
                role: WhoCanRole::OrgAdmin,
                people: vec!["Ana *B*".into(), "Bo".into()],
            },
            self_hosted: Target::Ask {
                label: "Self-hosted apps".into(),
                role: WhoCanRole::OrgAdmin,
                people: vec![],
            },
            packages: Target::Ask {
                label: "Packages".into(),
                role: WhoCanRole::InstanceAdmin,
                people: vec![],
            },
        }
    }

    #[test]
    fn fills_each_placeholder_once_and_escapes_values() {
        let raw = "{{aurora.callback_url}} | {{aurora.external_id}} | {{aurora.app_setup}} | {{aurora.page:oauth_clients}} | {{aurora.page:self_hosted}} | {{aurora.page:packages}}";
        let out = render(raw, &values(Some("ext`*[x](y)")));
        assert_eq!(
            out,
            "`https://aurora.example/v1/oauth/callback` | ``ext`*[x](y)`` | [Demo setup](https://aurora.example/o/acme/w/web/apps/demo) | OAuth clients (ask an organization admin: Ana \\*B\\*, Bo) | Self-hosted apps (ask an organization admin) | Packages (ask an administrator of this Aurora instance)"
        );
        // A non-member gets the sentence, not the id.
        assert_eq!(
            render("{{aurora.external_id}}", &values(None)),
            "(shown to members of your organization)"
        );
        // Substitution is not recursive.
        let mut v = values(Some("{{aurora.callback_url}}"));
        v.external_id = Some("{{aurora.callback_url}}".into());
        assert_eq!(
            render("{{aurora.external_id}}", &v),
            "`{{aurora.callback_url}}`"
        );
    }
}
