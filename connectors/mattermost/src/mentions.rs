//! Which `@name`s in an agent's message are one user and which are a group (D-271(c)).
//!
//! A group mention (Mattermost custom groups and LDAP-synced groups, `@<group>`) notifies every
//! member, like `@channel`. The connector asks the platform: a name that is a user passes, a name
//! that is a group is defused with the broadcast words, a name that is neither passes (nobody is
//! notified), and when the platform cannot answer — or the message has more names than are worth
//! asking about — the names left unresolved are treated as groups (fail closed, [`Lookups::closed`]).
//!
//! Answers are cached for a minute. Groups need a Mattermost licence: an unlicensed server answers
//! `501` on the groups API, which is a definite "no groups here" (cached for ten minutes).

use crate::api::{Mm, R};
use crate::render::Lookups;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long an answer about a name is trusted.
const TTL: Duration = Duration::from_secs(60);
/// How long "this server has no groups feature" is trusted.
const NO_GROUPS_TTL: Duration = Duration::from_secs(600);
/// More distinct names than this in one message: not asked about, the message fails closed.
pub const MAX_NAMES: usize = 25;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    User,
    Group,
    Neither,
}

/// Cached classification of mention names, per integration.
#[derive(Default)]
pub struct NameCache {
    names: Mutex<HashMap<String, (Instant, Class)>>,
    no_groups_until: Mutex<Option<Instant>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A `501` from Mattermost (not implemented / needs a licence) as the connector maps it.
fn is_not_implemented(e: &aurora_protocol::connector::OpError) -> bool {
    e.message.contains("answered 501")
}

/// Reads with the bot's token, then the admin's when the bot may not (a group list needs it).
async fn read(mm: &Mm, bot: &str, admin: Option<&str>, path: &str, q: &[(&str, &str)]) -> R<Value> {
    match mm.get_q(bot, path, q).await {
        Err(e)
            if matches!(
                e.code,
                aurora_protocol::connector::ErrorCode::Forbidden
                    | aurora_protocol::connector::ErrorCode::NotFound
            ) =>
        {
            match admin {
                Some(a) => mm.get_q(a, path, q).await,
                None => Err(e),
            }
        }
        other => other,
    }
}

/// Resolves `names` (lowercase mention names, see [`crate::render::MentionRefs::names`]) into
/// `lk`: usernames into `lk.known`, group names into `lk.groups`, and `lk.closed` when a lookup
/// could not be answered.
pub async fn resolve(
    mm: &Mm,
    bot: &str,
    admin: Option<&str>,
    cache: &NameCache,
    names: &[String],
    lk: &mut Lookups,
) {
    if names.len() > MAX_NAMES {
        lk.closed = true;
    }
    let names = &names[..names.len().min(MAX_NAMES)];
    let mut todo: Vec<String> = Vec::new();
    {
        let c = lock(&cache.names);
        for n in names {
            match c.get(n) {
                Some((at, class)) if at.elapsed() < TTL => apply(*class, n, lk),
                _ => todo.push(n.clone()),
            }
        }
    }
    if todo.is_empty() {
        return;
    }
    // Users, in one request.
    let users: Vec<String> = match mm.post(bot, "/users/usernames", &json!(todo)).await {
        Ok(Value::Array(a)) => a
            .iter()
            .filter_map(|u| u.get("username").and_then(Value::as_str))
            .map(str::to_ascii_lowercase)
            .collect(),
        _ => {
            tracing::warn!(
                "username lookup failed; unresolved mention names are treated as groups"
            );
            lk.closed = true;
            return;
        }
    };
    for n in todo {
        let class = if users.contains(&n) {
            Class::User
        } else {
            match group_named(mm, bot, admin, cache, &n).await {
                Some(true) => Class::Group,
                Some(false) => Class::Neither,
                None => {
                    tracing::warn!(
                        "group lookup failed; unresolved mention names are treated as groups"
                    );
                    lk.closed = true;
                    continue;
                }
            }
        };
        lock(&cache.names).insert(n.clone(), (Instant::now(), class));
        apply(class, &n, lk);
    }
}

fn apply(class: Class, name: &str, lk: &mut Lookups) {
    match class {
        Class::User => {
            lk.known.insert(name.to_owned());
        }
        Class::Group => {
            lk.groups.insert(name.to_owned());
        }
        Class::Neither => {}
    }
}

/// Whether a group has the mention name `name`; `None` when the platform could not say.
async fn group_named(
    mm: &Mm,
    bot: &str,
    admin: Option<&str>,
    cache: &NameCache,
    name: &str,
) -> Option<bool> {
    if lock(&cache.no_groups_until).is_some_and(|t| Instant::now() < t) {
        return Some(false);
    }
    match read(
        mm,
        bot,
        admin,
        "/groups",
        &[("q", name), ("per_page", "60")],
    )
    .await
    {
        Ok(Value::Array(groups)) => Some(groups.iter().any(|g| {
            g.get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })),
        Ok(_) => None,
        Err(e) if is_not_implemented(&e) => {
            *lock(&cache.no_groups_until) = Some(Instant::now() + NO_GROUPS_TTL);
            Some(false)
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::NetPolicy;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    async fn run(mm: &MockServer, cache: &NameCache, n: &[&str]) -> Lookups {
        let c = Mm::with_policy(&mm.uri(), &NetPolicy::allow_all()).unwrap();
        let mut lk = Lookups::default();
        resolve(&c, "tok", None, cache, &names(n), &mut lk).await;
        lk
    }

    async fn users(mm: &MockServer, found: &[&str]) {
        Mock::given(method("POST"))
            .and(path("/api/v4/users/usernames"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    found
                        .iter()
                        .map(|u| json!({"id": "u", "username": u}))
                        .collect::<Vec<_>>(),
                ),
            )
            .mount(mm)
            .await;
    }

    #[tokio::test]
    async fn a_user_passes_a_group_is_a_broadcast_and_a_stranger_is_neither() {
        let mm = MockServer::start().await;
        users(&mm, &["alice"]).await;
        Mock::given(method("GET"))
            .and(path("/api/v4/groups"))
            .and(query_param("q", "devs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id": "g1", "name": "devs"},
                {"id": "g2", "name": "devs-eu"}
            ])))
            .mount(&mm)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v4/groups"))
            .and(query_param("q", "stranger"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"id": "g3", "name": "strangers"}
            ])))
            .mount(&mm)
            .await;
        let lk = run(&mm, &NameCache::default(), &["alice", "devs", "stranger"]).await;
        assert!(lk.known.contains("alice"));
        assert!(lk.groups.contains("devs"));
        assert!(
            !lk.groups.contains("stranger"),
            "a prefix match is no match"
        );
        assert!(!lk.known.contains("stranger") && !lk.closed);
        // The render path: the group is defused, the user and the stranger pass.
        assert_eq!(
            crate::render::defuse("@alice @devs @stranger", &lk),
            "@alice `@devs` @stranger"
        );
    }

    /// An unlicensed server answers 501 on the groups API: no groups, no fail-closed — and the
    /// answer is remembered (one request, however many names).
    #[tokio::test]
    async fn an_unlicensed_server_has_no_groups_and_is_asked_once() {
        let mm = MockServer::start().await;
        users(&mm, &[]).await;
        Mock::given(method("GET"))
            .and(path("/api/v4/groups"))
            .respond_with(ResponseTemplate::new(501).set_body_json(
                json!({"id": "api.license_error", "message": "api endpoint requires a license"}),
            ))
            .expect(1)
            .mount(&mm)
            .await;
        let lk = run(&mm, &NameCache::default(), &["a", "b", "c"]).await;
        assert!(!lk.closed && lk.groups.is_empty());
    }

    #[tokio::test]
    async fn a_lookup_that_fails_closes_the_message() {
        // The username lookup fails.
        let mm = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v4/users/usernames"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mm)
            .await;
        assert!(run(&mm, &NameCache::default(), &["bob"]).await.closed);
        // The group lookup fails (a 500, or a 403 the admin token cannot cure).
        for status in [500, 403] {
            let mm = MockServer::start().await;
            users(&mm, &["alice"]).await;
            Mock::given(method("GET"))
                .and(path("/api/v4/groups"))
                .respond_with(ResponseTemplate::new(status))
                .mount(&mm)
                .await;
            let lk = run(&mm, &NameCache::default(), &["alice", "bob"]).await;
            assert!(lk.closed, "{status}");
            assert!(lk.known.contains("alice"));
            assert_eq!(
                crate::render::defuse("@alice @bob", &lk),
                "@alice `@bob`",
                "{status}"
            );
        }
    }

    #[tokio::test]
    async fn too_many_names_close_the_message_and_answers_are_cached() {
        let mm = MockServer::start().await;
        users(&mm, &["alice"]).await;
        Mock::given(method("GET"))
            .and(path("/api/v4/groups"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&mm)
            .await;
        let many: Vec<String> = (0..=MAX_NAMES).map(|i| format!("n{i}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(run(&mm, &NameCache::default(), &many).await.closed);
        // Cached: a second resolution of the same names makes no request.
        let mm = MockServer::start().await;
        users(&mm, &["alice"]).await;
        let cache = NameCache::default();
        let first = run(&mm, &cache, &["alice"]).await;
        let second = run(&mm, &cache, &["alice"]).await;
        assert!(first.known.contains("alice") && second.known.contains("alice"));
        let reqs = mm.received_requests().await.unwrap();
        assert_eq!(reqs.len(), 1, "{reqs:?}");
    }
}
