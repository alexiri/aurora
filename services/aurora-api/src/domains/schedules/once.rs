//! (P5-SCHED-ONCE, D-421, C-1092) One-time schedules: `run_at` is a local date-time without an
//! offset, read in the schedule's IANA time zone. The rules follow `tools/calendar.rs`: an
//! ambiguous local time (clocks go back) is the earlier instant, a time inside a daylight-saving
//! gap is refused. Bounds: not before the start of the current minute, at most
//! `SCHEDULE_ONCE_MAX_AHEAD` ahead; no minimum interval.

use super::cron::parse_tz;
use aurora_core::{ApiError, limits};
use chrono::{DateTime, NaiveDateTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

const PATH: &str = "/run_at";

fn bad(msg: &str) -> ApiError {
    ApiError::invalid_field("Invalid schedule.", PATH, msg)
}

/// The accepted request formats (no offset, no `Z`).
const FORMATS: [&str; 2] = ["%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S"];

/// A local date-time `YYYY-MM-DDTHH:MM[:SS]`; anything else (an offset, `Z`, another shape) is
/// refused with the one message that says what to send.
pub fn parse_local(s: &str) -> Result<NaiveDateTime, ApiError> {
    let s = s.trim();
    FORMATS
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(s, f).ok())
        .ok_or_else(|| {
            bad("must be a local date and time such as 2026-11-21T09:00, without an offset; set timezone")
        })
}

/// The instant of a local time in `tz`: the earlier one when clocks go back, refused in a gap.
pub fn to_instant(local: NaiveDateTime, tz: Tz) -> Result<DateTime<Utc>, ApiError> {
    tz.from_local_datetime(&local)
        .earliest()
        .map(|t| t.with_timezone(&Utc))
        .ok_or_else(|| bad("does not exist in that time zone (a daylight-saving gap)"))
}

/// The bounds of a pending one-time schedule: from the start of the current minute (a time in
/// the current minute fires at once) to `SCHEDULE_ONCE_MAX_AHEAD` ahead.
pub fn check_bounds(at: DateTime<Utc>, now: DateTime<Utc>) -> Result<(), ApiError> {
    let minute = now
        .with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(now);
    if at < minute {
        return Err(bad("is in the past"));
    }
    let ahead = chrono::Duration::from_std(limits::SCHEDULE_ONCE_MAX_AHEAD)
        .unwrap_or(chrono::Duration::MAX);
    if at > now + ahead {
        return Err(bad("must be at most 5 years ahead"));
    }
    Ok(())
}

/// Parse and validate a request's `run_at` in `tz`; returns the instant.
pub fn validate(run_at: &str, tz: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>, ApiError> {
    let tz = parse_tz(tz)?;
    let at = to_instant(parse_local(run_at)?, tz)?;
    check_bounds(at, now)?;
    Ok(at)
}

/// The local wall time of `at` in `tz`, in the request's format (seconds only when not zero), so
/// it round-trips. An unknown zone (never stored) falls back to UTC.
pub fn local_of(at: DateTime<Utc>, tz: &str) -> String {
    let local = tz
        .parse::<Tz>()
        .map_or_else(|_| at.naive_utc(), |z| at.with_timezone(&z).naive_local());
    if local.second() == 0 {
        local.format("%Y-%m-%dT%H:%M").to_string()
    } else {
        local.format("%Y-%m-%dT%H:%M:%S").to_string()
    }
}

/// The same wall time in another zone (a `timezone`-only change keeps 9:00 at 9:00).
pub fn rebase(at: DateTime<Utc>, from: &str, to: &str) -> Result<DateTime<Utc>, ApiError> {
    let to = parse_tz(to)?;
    to_instant(parse_local(&local_of(at, from))?, to)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn field_msg(e: &ApiError) -> String {
        e.details()
            .and_then(|d| d["fields"][0]["message"].as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    #[test]
    fn local_formats_only() {
        assert!(parse_local("2026-11-21T09:00").is_ok());
        assert!(parse_local(" 2026-11-21T09:00:30 ").is_ok());
        for bad in [
            "2026-11-21T09:00Z",
            "2026-11-21T09:00:00+01:00",
            "2026-11-21T09:00:00-05:00",
            "2026-11-21",
            "tomorrow at 9",
            "2026-11-21 09:00",
        ] {
            let e = parse_local(bad).unwrap_err();
            assert!(field_msg(&e).contains("without an offset"), "{bad}");
        }
    }

    #[test]
    fn read_in_the_zone_with_the_dst_rules() {
        let now = at("2026-01-01T00:00:00Z");
        // 09:00 in Zurich in winter is 08:00Z.
        assert_eq!(
            validate("2026-11-21T09:00", "Europe/Zurich", now).unwrap(),
            at("2026-11-21T08:00:00Z")
        );
        // Spring forward in Zurich, 2026-03-29: 02:00..03:00 does not exist.
        let e = validate("2026-03-29T02:30", "Europe/Zurich", now).unwrap_err();
        assert!(field_msg(&e).contains("daylight-saving gap"));
        // Fall back, 2026-10-25: 02:30 happens twice; the earlier one is 00:30Z (CEST).
        assert_eq!(
            validate("2026-10-25T02:30", "Europe/Zurich", now).unwrap(),
            at("2026-10-25T00:30:00Z")
        );
        assert!(validate("2026-11-21T09:00", "Mars/Base", now).is_err());
    }

    #[test]
    fn bounds_start_of_the_minute_to_five_years() {
        let now = at("2026-11-20T10:15:40Z");
        // The current minute fires at once; the minute before is in the past.
        assert!(validate("2026-11-20T10:15", "UTC", now).is_ok());
        let e = validate("2026-11-20T10:14", "UTC", now).unwrap_err();
        assert_eq!(field_msg(&e), "is in the past");
        assert!(validate("2031-11-20T10:00", "UTC", now).is_ok());
        let e = validate("2032-11-20T10:00", "UTC", now).unwrap_err();
        assert_eq!(field_msg(&e), "must be at most 5 years ahead");
    }

    #[test]
    fn local_round_trips_and_rebase_keeps_the_wall_time() {
        let i = at("2026-11-21T08:00:00Z");
        assert_eq!(local_of(i, "Europe/Zurich"), "2026-11-21T09:00");
        assert_eq!(
            local_of(at("2026-11-21T08:00:30Z"), "Europe/Zurich"),
            "2026-11-21T09:00:30"
        );
        let ny = rebase(i, "Europe/Zurich", "America/New_York").unwrap();
        assert_eq!(ny, at("2026-11-21T14:00:00Z"));
        assert_eq!(local_of(ny, "America/New_York"), "2026-11-21T09:00");
    }
}
