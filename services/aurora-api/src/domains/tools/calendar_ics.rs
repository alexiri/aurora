//! (P5-CALINV, D-274, SEC-108) A small typed iCalendar (RFC 5545) writer for calendar invitations.
//!
//! No iCalendar crate is in the workspace and a new dependency would need its own supply-chain
//! review, so the writer is in-house — but it is a *writer*, not string concatenation: a property
//! is a name, parameters and a value, and every agent-supplied string goes through exactly one of
//! [`escape_text`] (TEXT values), [`param_value`] (quoted parameter values) or a closed formatter
//! (dates, offsets, `mailto:` URIs). Lines are folded at 75 octets in [`Writer::finish`]. Nothing
//! an agent writes can add a property, a component or an attendee: control characters never reach
//! the output, and `,` `;` `\` and line breaks are escaped.

use chrono::{
    DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone, Utc,
    Weekday,
};
use chrono_tz::{OffsetComponents, OffsetName, Tz};

/// iTIP method of the calendar object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Request,
    Cancel,
}

impl Method {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Request => "REQUEST",
            Self::Cancel => "CANCEL",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Freq {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "DAILY",
            Self::Weekly => "WEEKLY",
            Self::Monthly => "MONTHLY",
            Self::Yearly => "YEARLY",
        }
    }
}

/// A bounded recurrence: exactly one of `count` and `until` (validated by the caller).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repeat {
    pub freq: Freq,
    pub interval: u32,
    pub by_day: Vec<Weekday>,
    pub count: Option<u32>,
    /// Last day of the series (inclusive), a date in the event's own zone.
    pub until: Option<NaiveDate>,
}

/// When the event happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum When {
    /// A single timed event: UTC times.
    Utc {
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
    /// A repeating timed event: local times with a `TZID` and a generated `VTIMEZONE`.
    Zoned {
        tz: Tz,
        start: NaiveDateTime,
        end: NaiveDateTime,
    },
    /// An all-day event; `end` is the last day, inclusive (`DTEND` is written exclusive).
    Day { start: NaiveDate, end: NaiveDate },
}

#[derive(Debug, Clone)]
pub struct Calendar<'a> {
    pub method: Method,
    pub uid: &'a str,
    pub sequence: u32,
    pub dtstamp: DateTime<Utc>,
    pub summary: &'a str,
    pub description: Option<&'a str>,
    pub location: Option<&'a str>,
    pub when: When,
    pub repeat: Option<&'a Repeat>,
    pub reminder_minutes: Option<u32>,
    pub organizer_name: &'a str,
    pub organizer_email: &'a str,
    pub attendee_email: &'a str,
}

/// Escapes a TEXT value (RFC 5545 §3.3.11): `\` `;` `,` and line breaks; other control characters
/// (including NUL) are dropped, tab becomes a space.
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str("\\n");
            }
            '\n' | '\u{2028}' | '\u{2029}' | '\u{85}' => out.push_str("\\n"),
            '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// A parameter value as a quoted string: `"` and every control character removed (a quoted
/// parameter value cannot contain either), so it cannot end the parameter early.
pub fn param_value(s: &str) -> String {
    let inner: String = s.chars().filter(|c| !c.is_control() && *c != '"').collect();
    format!("\"{inner}\"")
}

/// A `mailto:` URI value: only characters that can appear in an address survive.
fn mailto(addr: &str) -> String {
    let a: String = addr
        .chars()
        .filter(|c| c.is_ascii_graphic() && !matches!(c, '"' | '<' | '>' | ',' | ';' | '\\'))
        .collect();
    format!("mailto:{a}")
}

/// Accumulates unfolded content lines; folding happens once, in [`Writer::finish`].
#[derive(Default)]
struct Writer {
    lines: Vec<String>,
}

impl Writer {
    fn begin(&mut self, name: &str) {
        self.lines.push(format!("BEGIN:{name}"));
    }
    fn end(&mut self, name: &str) {
        self.lines.push(format!("END:{name}"));
    }
    /// A property whose value is already in its final form (a closed formatter's output or an
    /// escaped TEXT). `params` are `NAME=value` pairs with values already quoted where needed.
    fn raw(&mut self, name: &str, params: &[(&str, String)], value: &str) {
        let mut line = name.to_owned();
        for (k, v) in params {
            line.push(';');
            line.push_str(k);
            line.push('=');
            line.push_str(v);
        }
        line.push(':');
        line.push_str(value);
        self.lines.push(line);
    }
    fn text(&mut self, name: &str, value: &str) {
        let v = escape_text(value);
        self.raw(name, &[], &v);
    }
    /// Folds every line at 75 octets (continuation lines start with one space) and joins with
    /// CRLF, never splitting a UTF-8 sequence.
    fn finish(self) -> String {
        let mut out = String::new();
        for line in self.lines {
            let mut budget = 75;
            let mut used = 0;
            for c in line.chars() {
                let n = c.len_utf8();
                if used + n > budget {
                    out.push_str("\r\n ");
                    budget = 74;
                    used = 0;
                }
                out.push(c);
                used += n;
            }
            out.push_str("\r\n");
        }
        out
    }
}

fn fmt_utc(t: DateTime<Utc>) -> String {
    t.format("%Y%m%dT%H%M%SZ").to_string()
}
fn fmt_local(t: NaiveDateTime) -> String {
    t.format("%Y%m%dT%H%M%S").to_string()
}
fn fmt_date(d: NaiveDate) -> String {
    d.format("%Y%m%d").to_string()
}

fn fmt_offset(secs: i32) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.unsigned_abs();
    let (h, m) = (a / 3600, a % 3600 / 60);
    format!("{sign}{h:02}{m:02}")
}

const fn day_code(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => "MO",
        Weekday::Tue => "TU",
        Weekday::Wed => "WE",
        Weekday::Thu => "TH",
        Weekday::Fri => "FR",
        Weekday::Sat => "SA",
        Weekday::Sun => "SU",
    }
}

/// `RRULE` value. A timed series' `UNTIL` is UTC (RFC 5545 §3.3.10: it must be when `DTSTART` has
/// a `TZID`): the end of the last local day; an all-day series' is a date.
fn rrule(r: &Repeat, when: &When) -> String {
    let mut s = format!("FREQ={}", r.freq.as_str());
    if r.interval > 1 {
        s.push_str(&format!(";INTERVAL={}", r.interval));
    }
    if !r.by_day.is_empty() {
        let days: Vec<&str> = r.by_day.iter().map(|d| day_code(*d)).collect();
        s.push_str(&format!(";BYDAY={}", days.join(",")));
    }
    if let Some(n) = r.count {
        s.push_str(&format!(";COUNT={n}"));
    } else if let Some(u) = r.until {
        match when {
            When::Day { .. } => s.push_str(&format!(";UNTIL={}", fmt_date(u))),
            When::Zoned { tz, .. } => {
                let last = u.and_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap_or_default());
                let utc = tz
                    .from_local_datetime(&last)
                    .latest()
                    .map_or_else(|| last.and_utc(), |t| t.with_timezone(&Utc));
                s.push_str(&format!(";UNTIL={}", fmt_utc(utc)));
            }
            When::Utc { .. } => s.push_str(&format!(";UNTIL={}T235959Z", fmt_date(u))),
        }
    }
    s
}

/// One offset change of a zone.
struct Transition {
    /// First UTC instant with the new offset.
    at: DateTime<Utc>,
    from: i32,
    to: i32,
    name: String,
    dst: bool,
}

fn offset_secs(tz: Tz, t: DateTime<Utc>) -> i32 {
    tz.offset_from_utc_datetime(&t.naive_utc())
        .fix()
        .local_minus_utc()
}

/// The zone's offset changes from `first_year` to `last_year` (inclusive): a daily scan, then a
/// bisection to the second.
fn transitions(tz: Tz, first_year: i32, last_year: i32) -> Vec<Transition> {
    let mut out = Vec::new();
    let Some(mut day) = NaiveDate::from_ymd_opt(first_year, 1, 1) else {
        return out;
    };
    let end = NaiveDate::from_ymd_opt(last_year + 1, 1, 1).unwrap_or(day);
    while day < end {
        let a = day.and_time(NaiveTime::MIN).and_utc();
        let b = a + Duration::days(1);
        let (oa, ob) = (offset_secs(tz, a), offset_secs(tz, b));
        if oa != ob {
            let (mut lo, mut hi) = (a, b);
            while hi - lo > Duration::seconds(1) {
                let mid = lo + (hi - lo) / 2;
                if offset_secs(tz, mid) == oa {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let off = tz.offset_from_utc_datetime(&hi.naive_utc());
            out.push(Transition {
                at: hi,
                from: oa,
                to: ob,
                name: off.abbreviation().unwrap_or("").to_owned(),
                dst: !off.dst_offset().is_zero(),
            });
        }
        day += Duration::days(1);
    }
    out
}

/// A `VTIMEZONE` that covers the series: a base observance, then one component per distinct
/// change with `RDATE`s, so daylight-saving changes keep the local hour.
fn vtimezone(w: &mut Writer, tz: Tz, first_year: i32, last_year: i32) {
    w.begin("VTIMEZONE");
    w.raw("TZID", &[], tz.name());
    // The base observance, in force before the first change.
    let start = NaiveDate::from_ymd_opt(first_year, 1, 1)
        .unwrap_or_default()
        .and_time(NaiveTime::MIN)
        .and_utc();
    let base = tz.offset_from_utc_datetime(&start.naive_utc());
    let kind = |dst: bool| if dst { "DAYLIGHT" } else { "STANDARD" };
    let base_off = base.fix().local_minus_utc();
    w.begin(kind(!base.dst_offset().is_zero()));
    w.raw("DTSTART", &[], "19700101T000000");
    w.raw("TZOFFSETFROM", &[], &fmt_offset(base_off));
    w.raw("TZOFFSETTO", &[], &fmt_offset(base_off));
    let n = base.abbreviation().unwrap_or("");
    if !n.is_empty() {
        w.text("TZNAME", n);
    }
    w.end(kind(!base.dst_offset().is_zero()));
    // One component per distinct (kind, from, to, name).
    let ts = transitions(tz, first_year, last_year);
    let mut seen: Vec<(bool, i32, i32, String)> = Vec::new();
    for t in &ts {
        let key = (t.dst, t.from, t.to, t.name.clone());
        if seen.contains(&key) {
            continue;
        }
        let same: Vec<&Transition> = ts
            .iter()
            .filter(|u| (u.dst, u.from, u.to, &u.name) == (key.0, key.1, key.2, &key.3))
            .collect();
        let local =
            |u: &Transition| fmt_local(u.at.naive_utc() + Duration::seconds(i64::from(u.from)));
        w.begin(kind(t.dst));
        w.raw("DTSTART", &[], &local(same[0]));
        w.raw("TZOFFSETFROM", &[], &fmt_offset(t.from));
        w.raw("TZOFFSETTO", &[], &fmt_offset(t.to));
        if !t.name.is_empty() {
            w.text("TZNAME", &t.name);
        }
        for u in &same[1..] {
            w.raw("RDATE", &[], &local(u));
        }
        w.end(kind(t.dst));
        seen.push(key);
    }
    w.end("VTIMEZONE");
}

/// The iCalendar object (`BEGIN:VCALENDAR` … `END:VCALENDAR`, CRLF line ends, folded).
pub fn write(c: &Calendar<'_>) -> String {
    let mut w = Writer::default();
    w.begin("VCALENDAR");
    w.raw("VERSION", &[], "2.0");
    w.raw("PRODID", &[], "-//Aurora//Calendar invitations//EN");
    w.raw("CALSCALE", &[], "GREGORIAN");
    w.raw("METHOD", &[], c.method.as_str());
    if let (When::Zoned { tz, start, .. }, Some(_)) = (&c.when, c.repeat) {
        vtimezone(&mut w, *tz, start.year() - 1, start.year() + 30);
    }
    w.begin("VEVENT");
    w.text("UID", c.uid);
    w.raw("DTSTAMP", &[], &fmt_utc(c.dtstamp));
    w.raw("SEQUENCE", &[], &c.sequence.to_string());
    match &c.when {
        When::Utc { start, end } => {
            w.raw("DTSTART", &[], &fmt_utc(*start));
            w.raw("DTEND", &[], &fmt_utc(*end));
        }
        When::Zoned { tz, start, end } => {
            let p = [("TZID", tz.name().to_owned())];
            w.raw("DTSTART", &p, &fmt_local(*start));
            w.raw("DTEND", &p, &fmt_local(*end));
        }
        When::Day { start, end } => {
            let p = [("VALUE", "DATE".to_owned())];
            w.raw("DTSTART", &p, &fmt_date(*start));
            w.raw("DTEND", &p, &fmt_date(*end + Duration::days(1)));
        }
    }
    if let Some(r) = c.repeat {
        w.raw("RRULE", &[], &rrule(r, &c.when));
    }
    w.text("SUMMARY", c.summary);
    if let Some(d) = c.description.filter(|d| !d.is_empty()) {
        w.text("DESCRIPTION", d);
    }
    if let Some(l) = c.location.filter(|l| !l.is_empty()) {
        w.text("LOCATION", l);
    }
    w.raw(
        "ORGANIZER",
        &[("CN", param_value(c.organizer_name))],
        &mailto(c.organizer_email),
    );
    w.raw(
        "ATTENDEE",
        &[
            ("RSVP", "FALSE".to_owned()),
            ("PARTSTAT", "NEEDS-ACTION".to_owned()),
        ],
        &mailto(c.attendee_email),
    );
    w.raw(
        "STATUS",
        &[],
        if c.method == Method::Cancel {
            "CANCELLED"
        } else {
            "CONFIRMED"
        },
    );
    w.raw("TRANSP", &[], "OPAQUE");
    if let (Some(m), Method::Request) = (c.reminder_minutes, c.method) {
        w.begin("VALARM");
        w.raw("ACTION", &[], "DISPLAY");
        w.text("DESCRIPTION", "Reminder");
        w.raw(
            "TRIGGER",
            &[],
            &if m == 0 {
                "PT0S".to_owned()
            } else {
                format!("-PT{m}M")
            },
        );
        w.end("VALARM");
    }
    w.end("VEVENT");
    w.end("VCALENDAR");
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base<'a>(when: When, repeat: Option<&'a Repeat>) -> Calendar<'a> {
        Calendar {
            method: Method::Request,
            uid: "0194-abc@aurora.example.org",
            sequence: 0,
            dtstamp: Utc.with_ymd_and_hms(2026, 11, 18, 10, 0, 0).unwrap(),
            summary: "Review",
            description: None,
            location: None,
            when,
            repeat,
            reminder_minutes: None,
            organizer_name: "Ada (Aurora)",
            organizer_email: "aurora@example.org",
            attendee_email: "ada@example.org",
        }
    }

    fn single() -> When {
        When::Utc {
            start: Utc.with_ymd_and_hms(2026, 12, 1, 9, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 12, 1, 10, 0, 0).unwrap(),
        }
    }

    /// Unfolded content lines (what a client parses).
    fn lines(ics: &str) -> Vec<String> {
        assert!(ics.ends_with("\r\n"));
        let mut out: Vec<String> = Vec::new();
        for l in ics.trim_end_matches("\r\n").split("\r\n") {
            assert!(l.len() <= 75, "line over 75 octets: {l:?}");
            if let Some(rest) = l.strip_prefix(' ') {
                out.last_mut().unwrap().push_str(rest);
            } else {
                out.push(l.to_owned());
            }
        }
        out
    }

    #[test]
    fn a_single_event_uses_utc_times_and_one_attendee() {
        let ics = write(&base(single(), None));
        let l = lines(&ics);
        assert_eq!(l.first().unwrap(), "BEGIN:VCALENDAR");
        assert_eq!(l.last().unwrap(), "END:VCALENDAR");
        for want in [
            "METHOD:REQUEST",
            "UID:0194-abc@aurora.example.org",
            "SEQUENCE:0",
            "DTSTART:20261201T090000Z",
            "DTEND:20261201T100000Z",
            "SUMMARY:Review",
            "ORGANIZER;CN=\"Ada (Aurora)\":mailto:aurora@example.org",
            "ATTENDEE;RSVP=FALSE;PARTSTAT=NEEDS-ACTION:mailto:ada@example.org",
            "STATUS:CONFIRMED",
        ] {
            assert!(l.iter().any(|x| x == want), "missing {want}: {l:#?}");
        }
        assert_eq!(l.iter().filter(|x| x.starts_with("ATTENDEE")).count(), 1);
        assert!(!l.iter().any(|x| x.contains("VTIMEZONE")));
    }

    #[test]
    fn an_all_day_event_writes_dates_with_an_exclusive_end() {
        let d = |m, day| NaiveDate::from_ymd_opt(2026, m, day).unwrap();
        let ics = write(&base(
            When::Day {
                start: d(12, 24),
                end: d(12, 26),
            },
            None,
        ));
        let l = lines(&ics);
        assert!(l.contains(&"DTSTART;VALUE=DATE:20261224".to_owned()));
        assert!(l.contains(&"DTEND;VALUE=DATE:20261227".to_owned()));
    }

    #[test]
    fn a_reminder_is_a_display_alarm() {
        let mut c = base(single(), None);
        c.reminder_minutes = Some(15);
        let l = lines(&write(&c));
        assert!(l.contains(&"BEGIN:VALARM".to_owned()));
        assert!(l.contains(&"TRIGGER:-PT15M".to_owned()));
        c.reminder_minutes = Some(0);
        assert!(lines(&write(&c)).contains(&"TRIGGER:PT0S".to_owned()));
    }

    /// SEC-108: an agent's text round-trips as one escaped value and cannot add properties,
    /// components or attendees.
    #[test]
    fn injection_corpus_stays_one_escaped_value() {
        let evil = [
            "Standup\r\nATTENDEE:mailto:x@y",
            "a\nEND:VEVENT\nBEGIN:VEVENT",
            "semi;colon, comma \\ back",
            "nul\0byte\u{7}bell\u{2028}ls",
            &"x".repeat(300),
        ];
        for title in evil {
            let mut c = base(single(), None);
            c.summary = title;
            c.description = Some(title);
            c.location = Some(title);
            let ics = write(&c);
            let l = lines(&ics);
            assert_eq!(
                l.iter().filter(|x| x.starts_with("ATTENDEE")).count(),
                1,
                "{title:?}"
            );
            assert_eq!(
                l.iter().filter(|x| *x == "END:VEVENT").count(),
                1,
                "{title:?}"
            );
            assert_eq!(
                l.iter().filter(|x| *x == "BEGIN:VEVENT").count(),
                1,
                "{title:?}"
            );
            let summary = l.iter().find(|x| x.starts_with("SUMMARY:")).unwrap();
            assert!(!summary.contains('\0') && !summary.contains('\r'));
            // Every physical line is a property of the closed set.
            for x in &l {
                let name = x.split([':', ';']).next().unwrap();
                assert!(
                    [
                        "BEGIN",
                        "END",
                        "VERSION",
                        "PRODID",
                        "CALSCALE",
                        "METHOD",
                        "UID",
                        "DTSTAMP",
                        "SEQUENCE",
                        "DTSTART",
                        "DTEND",
                        "SUMMARY",
                        "DESCRIPTION",
                        "LOCATION",
                        "ORGANIZER",
                        "ATTENDEE",
                        "STATUS",
                        "TRANSP",
                    ]
                    .contains(&name),
                    "unexpected property {name} in {x:?} for {title:?}"
                );
            }
        }
        // The escapes themselves.
        assert_eq!(
            escape_text("a;b,c\\d\r\ne\nf\rg"),
            "a\\;b\\,c\\\\d\\ne\\nf\\ng"
        );
        assert_eq!(escape_text("a\0b\tc"), "ab c");
    }

    #[test]
    fn long_lines_fold_at_75_octets_without_splitting_a_character() {
        let mut c = base(single(), None);
        let long = "é".repeat(200);
        c.summary = &long;
        let ics = write(&c);
        let l = lines(&ics); // asserts every physical line <= 75 octets
        let s = l.iter().find(|x| x.starts_with("SUMMARY:")).unwrap();
        assert_eq!(s.trim_start_matches("SUMMARY:"), long);
    }

    #[test]
    fn organizer_name_cannot_end_its_parameter() {
        let mut c = base(single(), None);
        c.organizer_name = "Eve\";ROLE=CHAIR\r\nATTENDEE:mailto:x@y";
        let l = lines(&write(&c));
        let o = l.iter().find(|x| x.starts_with("ORGANIZER")).unwrap();
        assert!(
            o.starts_with("ORGANIZER;CN=\"Eve;ROLE=CHAIRATTENDEE:mailto:x@y\":mailto:"),
            "{o}"
        );
        assert_eq!(l.iter().filter(|x| x.starts_with("ATTENDEE")).count(), 1);
    }

    #[test]
    fn a_sequence_increment_and_cancel() {
        let mut c = base(single(), None);
        c.sequence = 3;
        c.method = Method::Cancel;
        c.reminder_minutes = Some(10);
        let l = lines(&write(&c));
        assert!(l.contains(&"METHOD:CANCEL".to_owned()));
        assert!(l.contains(&"SEQUENCE:3".to_owned()));
        assert!(l.contains(&"STATUS:CANCELLED".to_owned()));
        assert!(
            !l.contains(&"BEGIN:VALARM".to_owned()),
            "a cancel carries no alarm"
        );
    }

    fn weekly_zurich(count: bool) -> (Repeat, When) {
        let tz: Tz = "Europe/Zurich".parse().unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let t = |h| day.and_time(NaiveTime::from_hms_opt(h, 0, 0).unwrap());
        (
            Repeat {
                freq: Freq::Weekly,
                interval: 1,
                by_day: vec![Weekday::Mon],
                count: count.then_some(20),
                until: (!count).then(|| NaiveDate::from_ymd_opt(2027, 4, 5).unwrap()),
            },
            When::Zoned {
                tz,
                start: t(9),
                end: t(10),
            },
        )
    }

    /// A repeating event keeps its local hour across the October and March changes: the
    /// `VTIMEZONE` carries both observances for Europe/Zurich.
    #[test]
    fn a_repeating_event_carries_a_vtimezone_with_the_dst_changes() {
        let (r, when) = weekly_zurich(true);
        let l = lines(&write(&base(when, Some(&r))));
        assert!(l.contains(&"TZID:Europe/Zurich".to_owned()));
        assert!(l.contains(&"DTSTART;TZID=Europe/Zurich:20261019T090000".to_owned()));
        assert!(l.contains(&"DTEND;TZID=Europe/Zurich:20261019T100000".to_owned()));
        assert!(l.contains(&"RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=20".to_owned()));
        // DAYLIGHT: +0100 -> +0200 on the last Sunday of March at 02:00 local (the first change
        // is the component's DTSTART, from the year before the event; the rest are RDATEs).
        assert!(l.contains(&"DTSTART:20250330T020000".to_owned()), "{l:#?}");
        assert!(l.contains(&"RDATE:20270328T020000".to_owned()), "{l:#?}");
        // STANDARD: +0200 -> +0100 on the last Sunday of October at 03:00 local.
        assert!(l.contains(&"DTSTART:20251026T030000".to_owned()), "{l:#?}");
        assert!(l.contains(&"RDATE:20261025T030000".to_owned()), "{l:#?}");
        assert!(l.contains(&"RDATE:20271031T030000".to_owned()), "{l:#?}");
        assert!(l.contains(&"TZOFFSETFROM:+0200".to_owned()));
        assert!(l.contains(&"TZOFFSETTO:+0100".to_owned()));
        assert!(l.contains(&"TZOFFSETFROM:+0100".to_owned()));
        assert!(l.contains(&"TZOFFSETTO:+0200".to_owned()));
        assert!(l.contains(&"TZNAME:CEST".to_owned()) && l.contains(&"TZNAME:CET".to_owned()));
        // The VTIMEZONE comes before the event.
        let vt = l.iter().position(|x| x == "BEGIN:VTIMEZONE").unwrap();
        let ev = l.iter().position(|x| x == "BEGIN:VEVENT").unwrap();
        assert!(vt < ev);
    }

    #[test]
    fn an_until_in_a_zone_is_written_in_utc() {
        let (r, when) = weekly_zurich(false);
        let l = lines(&write(&base(when, Some(&r))));
        // 2027-04-05 23:59:59 in Zurich (+0200) = 21:59:59 UTC.
        assert!(
            l.contains(&"RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20270405T215959Z".to_owned()),
            "{l:#?}"
        );
    }

    #[test]
    fn a_zone_without_changes_still_gets_a_valid_vtimezone() {
        let tz: Tz = "Asia/Tokyo".parse().unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 12, 1).unwrap();
        let t = |h| day.and_time(NaiveTime::from_hms_opt(h, 0, 0).unwrap());
        let r = Repeat {
            freq: Freq::Daily,
            interval: 2,
            by_day: vec![],
            count: Some(5),
            until: None,
        };
        let l = lines(&write(&base(
            When::Zoned {
                tz,
                start: t(9),
                end: t(10),
            },
            Some(&r),
        )));
        assert!(l.contains(&"TZID:Asia/Tokyo".to_owned()));
        assert!(l.contains(&"TZOFFSETTO:+0900".to_owned()));
        assert!(l.contains(&"RRULE:FREQ=DAILY;INTERVAL=2;COUNT=5".to_owned()));
    }
}
