//! (P5-UX-14, C-944; P5-POLISH-3, D-383, C-1033) An account's health, read from its status and what was
//! recorded. Real use (an agent's call, written by the gateway, C-985) and Test (C-943) are recorded apart:
//! only real use decides `working` and `failing`; a passed Test is reported next to the state, never as it.

use super::dto::{Health, HealthState, HealthTest, Verification, VerificationState};
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

/// (C-1065) Whether an AI key works. `origin` is the stored origin (status and when it was
/// checked), `queued` that a background re-check is running on this replica. A pending origin with
/// a queued re-check is `checking` whatever older evidence says (the key was just saved or
/// replaced); otherwise the **newest** evidence decides: a verified origin, a real call or a
/// passed Test is `works`; a rejection from the provider is `rejected`; a Test that found nothing
/// answering is `unreachable`. With none, `not_checked`. Advisory: Test is authoritative.
pub fn verification(
    origin: Option<(aurora_llm::governance::OriginStatus, Option<DateTime<Utc>>)>,
    rec: &Recorded<'_>,
    queued: bool,
) -> Verification {
    use aurora_llm::governance::OriginStatus;
    let pending = matches!(origin, Some((OriginStatus::Pending, _)));
    if pending && queued {
        return Verification {
            state: VerificationState::Checking,
            checked_at: None,
        };
    }
    let mut evidence: Vec<(DateTime<Utc>, VerificationState)> = Vec::new();
    if let Some((OriginStatus::Verified, Some(at))) = origin {
        evidence.push((at, VerificationState::Works));
    }
    if let Some(at) = rec.last_ok_at {
        evidence.push((at, VerificationState::Works));
    }
    if let Some(at) = rec.last_error_at {
        match rec.last_error {
            Some("rejected") => evidence.push((at, VerificationState::Rejected)),
            Some("unreachable") => evidence.push((at, VerificationState::Unreachable)),
            _ => {}
        }
    }
    if let (Some(at), Some(r)) = (rec.last_test_at, rec.last_test_result) {
        match r {
            "ok" => evidence.push((at, VerificationState::Works)),
            "rejected" => evidence.push((at, VerificationState::Rejected)),
            "unreachable" => evidence.push((at, VerificationState::Unreachable)),
            _ => {}
        }
    }
    match evidence.into_iter().max_by_key(|(at, _)| *at) {
        Some((at, state)) => Verification {
            state,
            checked_at: Some(at),
        },
        // (P5-REFINE-20) The re-checks are over and nothing answered: final, with when it was
        // last tried (a pending origin's `checked_at` is its last attempt). A real call that
        // succeeds later is evidence above and wins.
        None => Verification {
            state: VerificationState::NotChecked,
            checked_at: match origin {
                Some((OriginStatus::Pending, at)) => at,
                _ => None,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurora_llm::governance::OriginStatus;
    use chrono::Duration;

    #[test]
    fn verification_pending_is_checking_only_while_a_recheck_is_queued() {
        let none = Recorded::default();
        let p = Some((OriginStatus::Pending, None));
        assert_eq!(
            verification(p, &none, true).state,
            VerificationState::Checking
        );
        assert_eq!(
            verification(p, &none, false).state,
            VerificationState::NotChecked
        );
        // (P5-REFINE-20) Once the re-checks are over a pending key is final: `not_checked` with
        // the time of the last attempt, and a later real call makes it work.
        let t = Utc::now();
        let tried = Some((OriginStatus::Pending, Some(t)));
        let over = verification(tried, &none, false);
        assert_eq!(over.state, VerificationState::NotChecked);
        assert_eq!(over.checked_at, Some(t));
        let called = Recorded {
            last_ok_at: Some(t + Duration::minutes(1)),
            ..Recorded::default()
        };
        assert_eq!(
            verification(tried, &called, false).state,
            VerificationState::Works
        );
        // A passed Test resolves a pending key; a verified origin works.
        let tested = Recorded {
            last_test_at: Some(t),
            last_test_result: Some("ok"),
            ..Recorded::default()
        };
        assert_eq!(
            verification(p, &tested, false).state,
            VerificationState::Works
        );
        let v = Some((OriginStatus::Verified, Some(t)));
        assert_eq!(
            verification(v, &none, false).state,
            VerificationState::Works
        );
    }

    #[test]
    fn verification_the_newest_evidence_wins() {
        let t = Utc::now();
        let earlier = t - Duration::minutes(5);
        let v = Some((OriginStatus::Verified, Some(earlier)));
        let rejected_now = Recorded {
            last_test_at: Some(t),
            last_test_result: Some("rejected"),
            ..Recorded::default()
        };
        assert_eq!(
            verification(v, &rejected_now, false).state,
            VerificationState::Rejected
        );
        let ok_after_error = Recorded {
            last_ok_at: Some(t),
            last_error_at: Some(earlier),
            last_error: Some("rejected"),
            ..Recorded::default()
        };
        assert_eq!(
            verification(None, &ok_after_error, false).state,
            VerificationState::Works
        );
        let unreachable = Recorded {
            last_test_at: Some(t),
            last_test_result: Some("unreachable"),
            ..Recorded::default()
        };
        assert_eq!(
            verification(None, &unreachable, false).state,
            VerificationState::Unreachable
        );
    }

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
