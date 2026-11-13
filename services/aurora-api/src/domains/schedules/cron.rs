//! Cron evaluation (docs/10 §Schedules): 5-field expressions evaluated with `croner` in an IANA
//! timezone; the next real occurrence strictly after an instant (DST-safe).

use aurora_core::{ApiError, FieldError};
use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use croner::Cron;
use std::str::FromStr;

/// Minimum interval between occurrences.
pub const MIN_INTERVAL: Duration = Duration::minutes(5);

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_request("Invalid schedule.", vec![FieldError::new(path, msg)])
}

pub fn parse_tz(tz: &str) -> Result<Tz, ApiError> {
    tz.parse::<Tz>()
        .map_err(|_| bad("/timezone", "must be an IANA timezone name"))
}

fn parse(expr: &str) -> Result<Cron, ApiError> {
    if expr.split_whitespace().count() != 5 {
        return Err(bad(
            "/cron",
            "must have exactly 5 fields: minute hour day-of-month month day-of-week",
        ));
    }
    Cron::from_str(expr).map_err(|e| bad("/cron", &format!("invalid cron expression: {e}")))
}

/// First occurrence strictly after `after`, as an instant.
pub fn next_after(expr: &str, tz: &str, after: DateTime<Utc>) -> Result<DateTime<Utc>, ApiError> {
    let cron = parse(expr)?;
    let tz = parse_tz(tz)?;
    cron.find_next_occurrence(&after.with_timezone(&tz), false)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|_| bad("/cron", "the expression never fires"))
}

/// (FIX-110) How far the minimum interval is checked: a full week of occurrences (an hourly
/// minute list like `0,5,10,15,20,21 * * * *` repeats every hour, a weekday list every week) or
/// this many occurrences, whichever comes first.
const SCAN_SPAN: Duration = Duration::days(8);
const SCAN_MAX: usize = 2_100;

/// Validate the pair and enforce the minimum interval over a full cycle of occurrences (a week);
/// returns the first occurrence after `now`.
pub fn validate(expr: &str, tz: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>, ApiError> {
    let first = next_after(expr, tz, now)?;
    let mut prev = first;
    for _ in 0..SCAN_MAX {
        let Ok(n) = next_after(expr, tz, prev) else {
            break;
        };
        if n - first > SCAN_SPAN {
            break;
        }
        if n - prev < MIN_INTERVAL {
            return Err(bad(
                "/cron",
                "must not fire more often than every 5 minutes",
            ));
        }
        prev = n;
    }
    Ok(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn five_fields_only() {
        assert!(validate("0 9 * * *", "UTC", Utc::now()).is_ok());
        assert!(validate("0 0 9 * * *", "UTC", Utc::now()).is_err());
        assert!(validate("@daily", "UTC", Utc::now()).is_err());
        assert!(validate("* * * *", "UTC", Utc::now()).is_err());
        assert!(validate("0 9 * * *", "Mars/Base", Utc::now()).is_err());
    }

    #[test]
    fn minimum_interval() {
        assert!(validate("* * * * *", "UTC", Utc::now()).is_err());
        assert!(validate("*/4 * * * *", "UTC", Utc::now()).is_err());
        assert!(validate("*/5 * * * *", "UTC", Utc::now()).is_ok());
    }

    /// (FIX-110) The gap is checked over a full cycle, not the next four occurrences.
    #[test]
    fn minimum_interval_holds_over_a_full_cycle() {
        // Created at :20:30, the next occurrences are :21 then 39 and 5-minute gaps; the 21→0
        // pair is not the problem, 20→21 is, one hour later in the cycle.
        assert!(validate("0,5,10,15,20,21 * * * *", "UTC", at("2026-03-02T12:20:30Z")).is_err());
        assert!(validate("0,5,10,15,20,21 * * * *", "UTC", at("2026-03-02T12:22:30Z")).is_err());
        // Day-of-week lists repeat weekly.
        assert!(validate("0,3 9 * * 1", "UTC", at("2026-03-02T12:22:30Z")).is_err());
        assert!(validate("0 9 * * 1-5", "UTC", at("2026-03-02T12:22:30Z")).is_ok());
        assert!(validate("*/5 * * * *", "UTC", at("2026-03-02T12:22:30Z")).is_ok());
    }

    #[test]
    fn next_occurrence_follows_dst() {
        let tz = "America/New_York";
        // 09:00 EST (14:00Z) before the spring-forward, 09:00 EDT (13:00Z) after it.
        let a = next_after("0 9 * * *", tz, at("2026-03-07T15:00:00Z")).unwrap();
        assert_eq!(a, at("2026-03-08T13:00:00Z"));
        let b = next_after("0 9 * * *", tz, at("2026-03-06T15:00:00Z")).unwrap();
        assert_eq!(b, at("2026-03-07T14:00:00Z"));
        // Strictly after.
        let c = next_after("0 9 * * *", tz, at("2026-03-08T13:00:00Z")).unwrap();
        assert_eq!(c, at("2026-03-09T13:00:00Z"));
    }
}
