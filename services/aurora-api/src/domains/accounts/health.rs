//! (P5-UX-14, C-944; P5-POLISH-3, D-383, C-1033) An account's health, read from its status and what was
//! recorded. Real use (an agent's call, written by the gateway, C-985) and Test (C-943) are recorded apart:
//! only real use decides `working` and `failing`; a passed Test is reported next to the state, never as it.

use super::dto::{Health, HealthState, HealthTest};
use aurora_registry::snapshot::AppEntry;
use chrono::{DateTime, Utc};

/// What was recorded for an account.
#[derive(Debug, Clone, Copy, Default)]
pub struct Recorded<'a> {
    pub last_ok_at: Option<DateTime<Utc>>,
    pub last_error_at: Option<DateTime<Utc>>,
    pub last_error: Option<&'a str>,
    pub last_test_at: Option<DateTime<Utc>>,
    pub last_test_result: Option<&'a str>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// `sign_in_expired` is status `reauth_required`; `disabled` is status `disabled`; otherwise `failing` when
/// the last failed call is newer than the last good one, `working` when a good call is the latest, and
/// `not_used` when no call has shown anything.
pub fn state_of(
    status: Option<&str>,
    ok: Option<DateTime<Utc>>,
    error: Option<DateTime<Utc>>,
) -> HealthState {
    match (status, ok, error) {
        (Some("reauth_required"), ..) => HealthState::SignInExpired,
        (Some("disabled"), ..) => HealthState::Disabled,
        // A failure and a success in the same instant: the failure is shown (never hide it).
        (_, Some(ok), Some(err)) if err >= ok => HealthState::Failing,
        (_, _, Some(_)) if ok.is_none() => HealthState::Failing,
        (_, Some(_), _) => HealthState::Working,
        _ => HealthState::NotUsed,
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

/// An account's `Health`. `detail` is the word of what the app refuses for a `rejected` failure; the host
/// of an unreachable account is filled in by the caller (it lives in the sealed credentials, never in a column).
pub fn health(status: Option<&str>, rec: Recorded<'_>, word: &'static str) -> Health {
    let state = state_of(status, rec.last_ok_at, rec.last_error_at);
    let failing = state == HealthState::Failing;
    Health {
        state,
        detail: (failing && rec.last_error == Some("rejected")).then(|| word.to_owned()),
        last_ok_at: rec.last_ok_at,
        last_error_at: rec.last_error_at,
        last_error: rec.last_error.map(str::to_owned),
        test: rec
            .last_test_at
            .zip(rec.last_test_result)
            .map(|(at, result)| HealthTest {
                at,
                result: result.to_owned(),
            }),
        last_used_at: rec.last_used_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn the_status_wins_then_the_newer_of_a_failure_and_a_success_then_not_used() {
        let t = Utc::now();
        let earlier = t - Duration::minutes(5);
        assert_eq!(
            state_of(Some("reauth_required"), Some(t), None),
            HealthState::SignInExpired
        );
        assert_eq!(
            state_of(Some("disabled"), None, Some(t)),
            HealthState::Disabled
        );
        assert_eq!(
            state_of(Some("active"), Some(t), Some(earlier)),
            HealthState::Working
        );
        assert_eq!(
            state_of(Some("active"), Some(earlier), Some(t)),
            HealthState::Failing
        );
        assert_eq!(state_of(None, None, Some(t)), HealthState::Failing);
        assert_eq!(state_of(None, Some(t), None), HealthState::Working);
        assert_eq!(state_of(Some("active"), None, None), HealthState::NotUsed);
    }

    #[test]
    fn a_passed_test_never_makes_an_account_work() {
        let h = health(
            Some("active"),
            Recorded {
                last_test_at: Some(Utc::now()),
                last_test_result: Some("ok"),
                ..Recorded::default()
            },
            "token",
        );
        assert_eq!(h.state, HealthState::NotUsed);
        assert_eq!(h.test.as_ref().map(|t| t.result.as_str()), Some("ok"));
    }

    #[test]
    fn only_a_rejected_failure_carries_the_word_and_never_a_raw_reason() {
        let rec = Recorded {
            last_error_at: Some(Utc::now()),
            last_error: Some("rejected"),
            ..Recorded::default()
        };
        assert_eq!(
            health(Some("active"), rec, "password").detail.as_deref(),
            Some("password")
        );
        let rec = Recorded {
            last_error: Some("unreachable"),
            ..rec
        };
        assert_eq!(health(Some("active"), rec, "password").detail, None);
        let h = health(Some("reauth_required"), Recorded::default(), "token");
        assert_eq!(h.detail, None);
    }
}
