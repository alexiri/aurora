//! (P5-REFINE-8, C-1080, D-407) The fixed sentences of the attention feeds (`14 §Needs your attention`):
//! display names and subject names only (sanitized, one line), never an e-mail, never free text in a
//! title. Text that points to an administrator names the **role**, never a person.

use crate::domains::app_setup_requests::dto::RequestLevel;
use crate::domains::apps::setup::SetupNeed;
use aurora_core::sanitize;

const NAME_MAX: usize = 80;

pub fn name(s: &str) -> String {
    sanitize::one_line(s, NAME_MAX)
}

/// The role that resolves a request of `level`, as words.
pub fn role_words(level: RequestLevel) -> &'static str {
    match level {
        RequestLevel::Organization => "an organization admin",
        RequestLevel::Administration => "an instance admin",
    }
}

/// "Bob Builder", "Bob Builder and Carol Member", "Bob Builder, Carol Member and Dan Dev",
/// "Bob Builder, Carol Member and 2 others"; and whether the verb is plural. `names` are the first
/// askers (oldest first), `total` all of them.
pub fn who(names: &[String], total: i64) -> (String, bool) {
    let total = total.max(names.len() as i64);
    let n: Vec<String> = names.iter().map(|s| name(s)).collect();
    match (n.as_slice(), total) {
        ([], _) => ("Someone".to_owned(), false),
        ([a], 1) => (a.clone(), false),
        ([a, b], 2) => (format!("{a} and {b}"), true),
        ([a, b, c], 3) => (format!("{a}, {b} and {c}"), true),
        ([a], t) => (others(a, t - 1), true),
        ([a, b, ..], t) => (others(&format!("{a}, {b}"), t - 2), true),
    }
}

fn others(head: &str, rest: i64) -> String {
    if rest == 1 {
        format!("{head} and 1 other")
    } else {
        format!("{head} and {rest} others")
    }
}

/// "Bob Builder wants Tracker", "Bob Builder and Carol Member want a chat platform".
pub fn request_title(names: &[String], total: i64, thing: &str) -> String {
    let (w, plural) = who(names, total);
    format!("{w} {} {thing}", if plural { "want" } else { "wants" })
}

/// What an app still needs, naming the role that can do it.
pub fn app_detail(
    app: &str,
    needs: &[SetupNeed],
    unavailable: bool,
    level: RequestLevel,
) -> String {
    let role = role_words(level);
    if unavailable {
        return format!(
            "{app} is switched off for the organization; only {role} can switch it on."
        );
    }
    match needs.first() {
        Some(SetupNeed::OauthClient) => {
            format!("Needs a sign-in app for {app} that only {role} can register.")
        }
        Some(SetupNeed::Instance) => format!("Needs a {app} server that only {role} can register."),
        Some(SetupNeed::AdminMaterial) | None => {
            format!("Needs set-up that only {role} can do.")
        }
    }
}

/// A chat platform request's detail: "Your organization has no chat platform connected." (or the type).
pub fn chat_detail(thing: &str) -> String {
    format!(
        "Your organization has no {} connected.",
        lower_article(thing)
    )
}

pub const AI_KEY_DETAIL: &str = "Your organization has no AI key its agents can use.";

/// "a chat platform" stays; "Chatter" stays.
fn lower_article(thing: &str) -> String {
    thing.strip_prefix("a ").unwrap_or(thing).to_owned()
}

/// The first letter in upper case ("a chat platform" → "A chat platform").
pub fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| {
        format!("{}{}", f.to_uppercase(), c.as_str())
    })
}

/// The asker's own open request.
pub fn my_request_text(thing: &str, level: RequestLevel) -> (String, String) {
    (
        format!("You asked for {thing}"),
        format!("Waiting for {}.", role_words(level)),
    )
}

/// The asker's own request that ended: declined (by role) or resolved.
pub fn my_done_text(thing: &str, declined: bool) -> (String, String) {
    if declined {
        crate::domains::app_setup_requests::service::declined_text(thing, None)
    } else {
        (
            format!("{} is ready", capital(thing)),
            "You can use it now.".to_owned(),
        )
    }
}

/// A gateway approval: "<Agent> is waiting for your approval" / what it wants to use.
pub fn approval_text(agent: &str, app: Option<&str>, host: &str) -> (String, String) {
    (
        format!("{} is waiting for your approval", name(agent)),
        match app {
            Some(a) => format!("It wants to use {}.", name(a)),
            None => format!("It wants to reach {}.", name(host)),
        },
    )
}

/// An action approval (the summary is not quoted: it can carry a person's own words, C-856).
pub fn action_approval_text(agent: &str) -> (String, String) {
    (
        format!("{} is waiting for your approval", name(agent)),
        "It wants to take an action that needs your approval.".to_owned(),
    )
}

/// An agent's access request (its own words are not quoted).
pub fn access_text(agent: &str, app: Option<&str>, host: Option<&str>) -> (String, String) {
    let what = match (app, host) {
        (Some(a), _) => name(a),
        (None, Some(h)) => format!("a token for {}", name(h)),
        (None, None) => "access".to_owned(),
    };
    (
        format!("{} asks for {what}", name(agent)),
        "Give it access from the agent's Apps & access tab.".to_owned(),
    )
}

/// An account that stopped working. `fixer` is `None` when the caller can fix it, else the role to ask
/// ("a workspace owner", "an organization admin").
pub fn broken_text(
    account: &str,
    sign_in_expired: bool,
    last_error: Option<&str>,
    fixer: Option<&str>,
) -> (String, String) {
    let n = name(account);
    let (title, why) = if sign_in_expired {
        (
            format!("{n}: the sign-in expired"),
            "Agents can't use it until someone signs in again.",
        )
    } else {
        (
            format!("{n} stopped working"),
            match last_error {
                Some("unreachable") => "It could not be reached.",
                Some("expired") => "The sign-in expired.",
                Some("unexpected") => "It gave an unexpected answer.",
                _ => "It no longer accepts the saved credentials.",
            },
        )
    };
    let detail = match fixer {
        Some(role) => format!("{why} Ask {role}."),
        None => why.to_owned(),
    };
    (title, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn names_and_others() {
        assert_eq!(
            request_title(&n(&["Bob Builder"]), 1, "Tracker"),
            "Bob Builder wants Tracker"
        );
        assert_eq!(
            request_title(&n(&["Bob Builder", "Carol Member"]), 2, "a chat platform"),
            "Bob Builder and Carol Member want a chat platform"
        );
        assert_eq!(
            request_title(
                &n(&["Bob Builder", "Carol Member", "Dan Dev"]),
                3,
                "an AI key"
            ),
            "Bob Builder, Carol Member and Dan Dev want an AI key"
        );
        assert_eq!(
            request_title(
                &n(&["Bob Builder", "Carol Member", "Dan Dev", "Eve"]),
                4,
                "an AI key"
            ),
            "Bob Builder, Carol Member and 2 others want an AI key"
        );
        assert_eq!(
            request_title(&n(&["Bob Builder", "Carol Member"]), 3, "Tracker"),
            "Bob Builder, Carol Member and 1 other want Tracker"
        );
        // A name cannot break the sentence.
        assert_eq!(
            request_title(&n(&["Bob\u{202e}\nBuilder"]), 1, "Tracker"),
            "Bob Builder wants Tracker"
        );
    }

    #[test]
    fn role_words_never_a_person() {
        assert_eq!(
            app_detail(
                "Tracker",
                &[SetupNeed::OauthClient],
                false,
                RequestLevel::Administration
            ),
            "Needs a sign-in app for Tracker that only an instance admin can register."
        );
        assert_eq!(
            app_detail(
                "Tracker",
                &[SetupNeed::Instance],
                false,
                RequestLevel::Organization
            ),
            "Needs a Tracker server that only an organization admin can register."
        );
        assert_eq!(
            app_detail("Tracker", &[], true, RequestLevel::Organization),
            "Tracker is switched off for the organization; only an organization admin can switch it on."
        );
        assert_eq!(
            chat_detail("a chat platform"),
            "Your organization has no chat platform connected."
        );
        assert_eq!(
            chat_detail("Chatter"),
            "Your organization has no Chatter connected."
        );
        assert_eq!(
            my_request_text("Tracker", RequestLevel::Organization),
            (
                "You asked for Tracker".to_owned(),
                "Waiting for an organization admin.".to_owned()
            )
        );
        assert_eq!(
            my_done_text("a chat platform", true).0,
            "An admin isn't setting up a chat platform now"
        );
        assert_eq!(
            my_done_text("a chat platform", false).0,
            "A chat platform is ready"
        );
        assert_eq!(
            broken_text("Code host", true, None, Some("a workspace owner")).1,
            "Agents can't use it until someone signs in again. Ask a workspace owner."
        );
    }
}
