//! (P5-UX-14, C-944) An account's health, read from its status and its last recorded check. The check comes
//! from real use (the gateway, C-985) or from Test (C-943); nothing else is stored.

use super::dto::{Health, HealthState};
use aurora_registry::snapshot::AppEntry;
use chrono::{DateTime, Utc};

/// `sign_in_expired` is status `reauth_required`; `disabled` is status `disabled`; otherwise the last check
/// result; `unknown` when never used or tested (and for an `unexpected` check, which has no wording of its own).
pub fn state_of(status: Option<&str>, check: Option<&str>) -> HealthState {
    match (status, check) {
        (Some("reauth_required"), _) => HealthState::SignInExpired,
        (Some("disabled"), _) => HealthState::Disabled,
        (_, Some("ok")) => HealthState::Ok,
        (_, Some("rejected")) => HealthState::Rejected,
        (_, Some("unreachable")) => HealthState::Unreachable,
        _ => HealthState::Unknown,
    }
}

/// The word for what the app refuses, from the account's field: `password`, `key` or `token`.
pub fn refused_word(app: Option<&AppEntry>) -> &'static str {
    let Some(app) = app else {
        return "token";
    };
    let secret = |needle: &str| {
        app.credentials
            .iter()
            .any(|c| c.secret && c.name.to_ascii_lowercase().contains(needle))
    };
    if secret("password") {
        "password"
    } else if secret("key") {
        "key"
    } else {
        "token"
    }
}

/// An account's `Health`. `detail` is the word of what the app refuses for `rejected`; the host of an
/// unreachable account is filled in by the caller (it lives in the sealed credentials, never in a column).
pub fn health(
    status: Option<&str>,
    check: Option<&str>,
    checked_at: Option<DateTime<Utc>>,
    last_used_at: Option<DateTime<Utc>>,
    word: &'static str,
) -> Health {
    let state = state_of(status, check);
    Health {
        state,
        detail: (state == HealthState::Rejected).then(|| word.to_owned()),
        checked_at: if check.is_some() { checked_at } else { None },
        last_used_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_wins_then_the_last_check_then_unknown() {
        assert_eq!(
            state_of(Some("reauth_required"), Some("ok")),
            HealthState::SignInExpired
        );
        assert_eq!(
            state_of(Some("disabled"), Some("rejected")),
            HealthState::Disabled
        );
        assert_eq!(state_of(Some("active"), Some("ok")), HealthState::Ok);
        assert_eq!(
            state_of(Some("active"), Some("rejected")),
            HealthState::Rejected
        );
        assert_eq!(
            state_of(None, Some("unreachable")),
            HealthState::Unreachable
        );
        assert_eq!(
            state_of(Some("active"), Some("unexpected")),
            HealthState::Unknown
        );
        assert_eq!(state_of(Some("active"), None), HealthState::Unknown);
    }

    #[test]
    fn only_a_rejected_account_carries_the_word_and_never_a_raw_reason() {
        let h = health(Some("active"), Some("rejected"), None, None, "password");
        assert_eq!(h.detail.as_deref(), Some("password"));
        let h = health(Some("reauth_required"), None, None, None, "token");
        assert_eq!(h.detail, None);
    }
}
