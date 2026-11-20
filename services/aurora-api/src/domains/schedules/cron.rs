//! Cron evaluation (docs/10 §Schedules): 5-field expressions evaluated with `croner` in an IANA
//! timezone; the next real occurrence strictly after an instant (DST-safe).

use aurora_core::ApiError;
use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use croner::Cron;
use croner::parser::CronParser;

/// Minimum interval between occurrences.
pub const MIN_INTERVAL: Duration = Duration::minutes(5);

fn bad(path: &str, msg: &str) -> ApiError {
    ApiError::invalid_field("Invalid schedule.", path, msg)
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
    // croner 4 rejects the step shorthand `5/5` (= `5-59/5`) that 3.x accepted; schedules saved
    // with it must keep evaluating, so the 3.x acceptance is kept.
    CronParser::builder()
        .sloppy_ranges(true)
        .build()
        .parse(expr)
        .map_err(|e| bad("/cron", &format!("invalid cron expression: {e}")))
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
    validate_min(expr, tz, now, MIN_INTERVAL)
}

/// As [`validate`] with a caller-chosen minimum interval (eval schedules, `EVAL_SCHEDULE_MIN_INTERVAL`).
pub fn validate_min(
    expr: &str,
    tz: &str,
    now: DateTime<Utc>,
    min: Duration,
) -> Result<DateTime<Utc>, ApiError> {
    let first = next_after(expr, tz, now)?;
    let mut prev = first;
    for _ in 0..SCAN_MAX {
        let Ok(n) = next_after(expr, tz, prev) else {
            break;
        };
        if n - first > SCAN_SPAN {
            break;
        }
        if n - prev < min {
            return Err(bad(
                "/cron",
                &format!(
                    "must not fire more often than every {} minutes",
                    min.num_minutes()
                ),
            ));
        }
        prev = n;
    }
    Ok(first)
}

/// The next `n` instants after `after` (fewer when the expression stops firing).
pub fn next_n(
    expr: &str,
    tz: &str,
    after: DateTime<Utc>,
    n: usize,
) -> Result<Vec<DateTime<Utc>>, ApiError> {
    let mut out = Vec::with_capacity(n);
    let mut prev = next_after(expr, tz, after)?;
    out.push(prev);
    while out.len() < n {
        let Ok(next) = next_after(expr, tz, prev) else {
            break;
        };
        out.push(next);
        prev = next;
    }
    Ok(out)
}

/// How many instants the preview reads to find the smallest gap (C-1077).
const PREVIEW_SCAN: usize = 12;

/// (P5-REFINE-7, C-1077) What a cadence would do, as you type: the next three instants, the
/// smallest gap among the next 12 (seconds) and whether it is below [`MIN_INTERVAL`]. Create and
/// update still refuse a too-frequent cadence over a full cycle ([`validate`]).
pub struct Preview {
    pub next: Vec<DateTime<Utc>>,
    pub interval_secs: Option<i64>,
    pub too_frequent: bool,
}

pub fn preview(expr: &str, tz: &str, now: DateTime<Utc>) -> Result<Preview, ApiError> {
    let all = next_n(expr.trim(), tz, now, PREVIEW_SCAN)?;
    let interval_secs = all.windows(2).map(|w| (w[1] - w[0]).num_seconds()).min();
    Ok(Preview {
        next: all.iter().take(3).copied().collect(),
        interval_secs,
        too_frequent: interval_secs.is_some_and(|s| s < MIN_INTERVAL.num_seconds()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_lists_three_and_flags_the_five_minute_minimum() {
        let now = at("2026-03-02T12:00:30Z");
        let every_minute = preview("* * * * *", "UTC", now).unwrap();
        assert_eq!(every_minute.next.len(), 3);
        assert_eq!(every_minute.next[0], at("2026-03-02T12:01:00Z"));
        assert_eq!(every_minute.interval_secs, Some(60));
        assert!(every_minute.too_frequent);
        let four = preview("*/4 * * * *", "UTC", now).unwrap();
        assert!(four.too_frequent);
        let five = preview("*/5 * * * *", "UTC", now).unwrap();
        assert_eq!(five.interval_secs, Some(300));
        assert!(!five.too_frequent);
        assert_eq!(
            five.next,
            vec![
                at("2026-03-02T12:05:00Z"),
                at("2026-03-02T12:10:00Z"),
                at("2026-03-02T12:15:00Z")
            ]
        );
        assert!(preview("0 9 * *", "UTC", now).is_err());
        assert!(preview("0 9 * * *", "Mars/Base", now).is_err());
    }

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

    #[test]
    fn a_job_before_the_dst_gap_does_not_also_fire_after_it() {
        // croner#67 (fixed in 4.0.3): 00:00 GMT on the spring-forward day, then 00:00 BST the next
        // day, not 02:00 BST right after the 01:00 gap.
        let next = next_after("0 0 * * *", "Europe/London", at("2026-03-29T00:00:00Z")).unwrap();
        assert_eq!(next, at("2026-03-29T23:00:00Z"));
    }

    #[test]
    fn the_step_shorthand_saved_under_croner_3_still_parses() {
        // `5/5` = `5-59/5`; croner 4 rejects it unless sloppy ranges are on.
        let next = next_after("5/5 * * * *", "UTC", at("2026-03-02T12:00:30Z")).unwrap();
        assert_eq!(next, at("2026-03-02T12:05:00Z"));
        assert!(validate("5/5 * * * *", "UTC", at("2026-03-02T12:00:30Z")).is_ok());
    }
}
