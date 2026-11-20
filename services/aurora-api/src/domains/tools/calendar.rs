//! Calendar tools (docs/12 Phase 0 catalogue; P5-CALINV, D-274, C-844, SEC-108):
//! `calendar_invite`, `calendar_invite_update`, `calendar_invite_cancel`, `calendar_invite_list`.
//!
//! An agent puts an event in a person's calendar by e-mailing them a standard invitation (iTIP
//! `REQUEST`/`CANCEL` over e-mail, RFC 6047) from the instance mailer; the person's Accept is the
//! approval. The only recipient is the verified e-mail of the person the turn runs for — no tool
//! argument names an address — and update, cancel and list reach only this agent's invitations to
//! that person. The tools exist only while the mailer is configured and the org switch is on.

use super::calendar_ics::{self as ics, Calendar, Freq, Method, Repeat, When};
use super::registry::{PlatformTool, ToolCallContext, ToolContext, ToolError};
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use crate::domains::auth::mail::{CalendarPart, Mail};
use crate::domains::people::turn_person::{self, Scope};
use crate::domains::tenancy::dto::OrgSettings;
use aurora_core::limits::{CALENDAR_SENDS_PER_AGENT_HOUR, CALENDAR_SENDS_PER_PERSON_DAY};
use aurora_core::sanitize;
use aurora_db::repo::agents;
use aurora_db::repo::calendar_invitations::{self as repo, Event, InvitationRow, Owner};
use aurora_protocol::tools::{JsonContent, ToolContent};
use chrono::{
    DateTime, Duration, Months, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc, Weekday,
};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

const TITLE_MAX: usize = 200;
const LOCATION_MAX: usize = 200;
const DESCRIPTION_MAX: usize = 4_000;
const MAX_EVENT_DAYS: i64 = 14;
const MAX_REMINDER_MINUTES: u32 = 10_080;
const LIST_MAX: i64 = 50;
/// A series ends within this many months of its start (`until`); `count` is bounded by 200.
const MAX_UNTIL_MONTHS: u32 = 120;

pub fn tools() -> Vec<Box<dyn PlatformTool>> {
    vec![
        Box::new(InviteT),
        Box::new(UpdateT),
        Box::new(CancelT),
        Box::new(ListT),
    ]
}

// ---------------------------------------------------------------------------------------------
// Arguments

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EventArgs {
    title: String,
    start: String,
    end: String,
    timezone: Option<String>,
    all_day: Option<bool>,
    location: Option<String>,
    description: Option<String>,
    repeat: Option<RepeatArgs>,
    reminder_minutes: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepeatArgs {
    freq: String,
    interval: Option<u32>,
    by_day: Option<Vec<String>>,
    count: Option<u32>,
    until: Option<String>,
}

/// The repeat as stored (`calendar_invitations.repeat`) and rebuilt for a cancel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct RepeatSpec {
    freq: String,
    interval: u32,
    by_day: Vec<String>,
    count: Option<u32>,
    until: Option<NaiveDate>,
}

impl RepeatSpec {
    fn to_repeat(&self) -> Repeat {
        Repeat {
            freq: match self.freq.as_str() {
                "daily" => Freq::Daily,
                "weekly" => Freq::Weekly,
                "monthly" => Freq::Monthly,
                _ => Freq::Yearly,
            },
            interval: self.interval,
            by_day: self.by_day.iter().filter_map(|d| weekday(d)).collect(),
            count: self.count,
            until: self.until,
        }
    }

    fn describe(&self) -> String {
        let every = if self.interval > 1 {
            format!("every {} ", self.interval)
        } else {
            String::new()
        };
        let unit = match (self.freq.as_str(), self.interval > 1) {
            ("daily", false) => "daily".to_owned(),
            ("weekly", false) => "weekly".to_owned(),
            ("monthly", false) => "monthly".to_owned(),
            ("yearly", false) => "yearly".to_owned(),
            (f, _) => format!("{every}{}", f.trim_end_matches("ly").replace("dai", "day")) + "s",
        };
        let days = if self.by_day.is_empty() {
            String::new()
        } else {
            format!(" on {}", self.by_day.join(", "))
        };
        let end = match (self.count, self.until) {
            (Some(n), _) => format!(", {n} times"),
            (_, Some(u)) => format!(", until {u}"),
            _ => String::new(),
        };
        format!("{unit}{days}{end}")
    }
}

fn weekday(s: &str) -> Option<Weekday> {
    Some(match s {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

/// A validated event: what is stored (times, zone, repeat) and what the e-mail shows
/// (description, location, reminder).
#[derive(Debug, Clone)]
struct Parsed {
    title: String,
    all_day: bool,
    timezone: String,
    /// Stored `starts_at`/`ends_at` (an all-day event: the first and last day at 00:00 UTC).
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    when: When,
    repeat: Option<RepeatSpec>,
    location: Option<String>,
    description: Option<String>,
    reminder_minutes: Option<u32>,
}

fn invalid(field: &str, why: &str) -> ToolError {
    ToolError::InvalidArguments(format!("{field}: {why}"))
}

fn text_field(
    field: &str,
    v: Option<String>,
    max: usize,
    required: bool,
) -> Result<Option<String>, ToolError> {
    let Some(v) = v else {
        return if required {
            Err(invalid(field, "is required"))
        } else {
            Ok(None)
        };
    };
    if required && v.trim().is_empty() {
        return Err(invalid(field, "must not be empty"));
    }
    if v.chars().count() > max {
        return Err(invalid(field, &format!("is at most {max} characters")));
    }
    Ok(Some(v))
}

fn parse_date(field: &str, s: &str) -> Result<NaiveDate, ToolError> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .map_err(|_| invalid(field, "must be a date, YYYY-MM-DD (all_day events)"))
}

/// A local date-time in `tz`; an RFC 3339 value with an offset is converted to the zone's local
/// time.
fn parse_local(field: &str, s: &str, tz: Tz) -> Result<NaiveDateTime, ToolError> {
    let s = s.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Ok(t.with_timezone(&tz).naive_local());
    }
    for f in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(t) = NaiveDateTime::parse_from_str(s, f) {
            return Ok(t);
        }
    }
    Err(invalid(
        field,
        "must be a local date-time such as 2026-12-01T09:00:00 (with `timezone`)",
    ))
}

fn to_utc(field: &str, tz: Tz, local: NaiveDateTime) -> Result<DateTime<Utc>, ToolError> {
    tz.from_local_datetime(&local)
        .earliest()
        .map(|t| t.with_timezone(&Utc))
        .ok_or_else(|| {
            invalid(
                field,
                "does not exist in that time zone (a daylight-saving gap)",
            )
        })
}

fn validate_repeat(r: RepeatArgs, start_date: NaiveDate) -> Result<RepeatSpec, ToolError> {
    let freq = r.freq.trim().to_ascii_lowercase();
    if !["daily", "weekly", "monthly", "yearly"].contains(&freq.as_str()) {
        return Err(invalid(
            "repeat.freq",
            "must be daily, weekly, monthly or yearly",
        ));
    }
    let interval = r.interval.unwrap_or(1);
    if !(1..=12).contains(&interval) {
        return Err(invalid("repeat.interval", "must be between 1 and 12"));
    }
    let mut by_day: Vec<String> = Vec::new();
    for d in r.by_day.unwrap_or_default() {
        let d = d.trim().to_ascii_uppercase();
        if weekday(&d).is_none() {
            return Err(invalid(
                "repeat.by_day",
                "entries are MO, TU, WE, TH, FR, SA or SU",
            ));
        }
        if !by_day.contains(&d) {
            by_day.push(d);
        }
    }
    if !by_day.is_empty() && freq != "weekly" {
        return Err(invalid("repeat.by_day", "applies to weekly repeats only"));
    }
    let until = r
        .until
        .as_deref()
        .map(|u| parse_date("repeat.until", u))
        .transpose()?;
    match (r.count, until) {
        (Some(_), Some(_)) => {
            return Err(invalid("repeat", "set either count or until, not both"));
        }
        (None, None) => {
            return Err(invalid(
                "repeat",
                "count or until is required (a series has an end)",
            ));
        }
        _ => {}
    }
    if let Some(n) = r.count
        && !(1..=200).contains(&n)
    {
        return Err(invalid("repeat.count", "must be between 1 and 200"));
    }
    if let Some(u) = until {
        let latest = start_date
            .checked_add_months(Months::new(MAX_UNTIL_MONTHS))
            .unwrap_or(start_date);
        if u < start_date || u > latest {
            return Err(invalid(
                "repeat.until",
                "must be on or after the start and within 10 years of it",
            ));
        }
    }
    Ok(RepeatSpec {
        freq,
        interval,
        by_day,
        count: r.count,
        until,
    })
}

/// Every bound of C-844. `default_tz` is the workspace's zone (the person has no profile zone).
fn validate(a: EventArgs, default_tz: &str, now: DateTime<Utc>) -> Result<Parsed, ToolError> {
    let title = text_field("title", Some(a.title), TITLE_MAX, true)?.unwrap_or_default();
    let location = text_field("location", a.location, LOCATION_MAX, false)?;
    let description = text_field("description", a.description, DESCRIPTION_MAX, false)?;
    if let Some(m) = a.reminder_minutes
        && m > MAX_REMINDER_MINUTES
    {
        return Err(invalid("reminder_minutes", "must be between 0 and 10080"));
    }
    let tz_name = a.timezone.as_deref().map_or(default_tz, str::trim);
    let tz: Tz = tz_name.parse().map_err(|_| {
        invalid(
            "timezone",
            "must be an IANA zone name such as Europe/Zurich",
        )
    })?;
    let all_day = a.all_day.unwrap_or(false);
    let horizon = Duration::days(5 * 366);
    let (starts_at, ends_at, when, start_date) = if all_day {
        let (s, e) = (parse_date("start", &a.start)?, parse_date("end", &a.end)?);
        if e < s {
            return Err(invalid(
                "end",
                "must not be before start (the last day, inclusive)",
            ));
        }
        if (e - s).num_days() > MAX_EVENT_DAYS {
            return Err(invalid("end", "the event is at most 14 days long"));
        }
        let at = |d: NaiveDate| d.and_time(NaiveTime::MIN).and_utc();
        (at(s), at(e), When::Day { start: s, end: e }, s)
    } else {
        let (ls, le) = (
            parse_local("start", &a.start, tz)?,
            parse_local("end", &a.end, tz)?,
        );
        let (us, ue) = (to_utc("start", tz, ls)?, to_utc("end", tz, le)?);
        if ue <= us {
            return Err(invalid("end", "must be after start"));
        }
        if ue - us > Duration::days(MAX_EVENT_DAYS) {
            return Err(invalid("end", "the event is at most 14 days long"));
        }
        let when = if a.repeat.is_some() {
            When::Zoned {
                tz,
                start: ls,
                end: le,
            }
        } else {
            When::Utc { start: us, end: ue }
        };
        (us, ue, when, ls.date())
    };
    if (starts_at - now).abs() > horizon {
        return Err(invalid("start", "must be within 5 years of now"));
    }
    let repeat = a
        .repeat
        .map(|r| validate_repeat(r, start_date))
        .transpose()?;
    Ok(Parsed {
        title,
        all_day,
        timezone: tz.name().to_owned(),
        starts_at,
        ends_at,
        when,
        repeat,
        location,
        description,
        reminder_minutes: a.reminder_minutes,
    })
}

/// The event of a stored row (a cancel has no description, location or reminder).
fn from_row(r: &InvitationRow) -> Parsed {
    let tz: Tz = r.timezone.parse().unwrap_or(Tz::UTC);
    let repeat: Option<RepeatSpec> = r
        .repeat
        .clone()
        .and_then(|v| serde_json::from_value(v).ok());
    let when = if r.all_day {
        When::Day {
            start: r.starts_at.date_naive(),
            end: r.ends_at.date_naive(),
        }
    } else if repeat.is_some() {
        When::Zoned {
            tz,
            start: r.starts_at.with_timezone(&tz).naive_local(),
            end: r.ends_at.with_timezone(&tz).naive_local(),
        }
    } else {
        When::Utc {
            start: r.starts_at,
            end: r.ends_at,
        }
    };
    Parsed {
        title: r.title.clone(),
        all_day: r.all_day,
        timezone: r.timezone.clone(),
        starts_at: r.starts_at,
        ends_at: r.ends_at,
        when,
        repeat,
        location: None,
        description: None,
        reminder_minutes: None,
    }
}

// ---------------------------------------------------------------------------------------------
// The message

struct Compose<'a> {
    method: Method,
    uid: &'a str,
    sequence: u32,
    dtstamp: DateTime<Utc>,
    event: &'a Parsed,
    agent_name: &'a str,
    agent_link: &'a str,
    instance_url: &'a str,
    organizer_email: &'a str,
    person_email: &'a str,
}

/// The local wording of when the event happens, `Tue 1 Dec 2026, 09:00–10:00 Europe/Zurich`.
fn when_text(e: &Parsed) -> String {
    let tz: Tz = e.timezone.parse().unwrap_or(Tz::UTC);
    if e.all_day {
        let (s, l) = (e.starts_at.date_naive(), e.ends_at.date_naive());
        let day = |d: NaiveDate| d.format("%a %-d %b %Y").to_string();
        return if s == l {
            format!("{} (all day)", day(s))
        } else {
            format!("{} – {} (all day)", day(s), day(l))
        };
    }
    let (s, f) = (e.starts_at.with_timezone(&tz), e.ends_at.with_timezone(&tz));
    if s.date_naive() == f.date_naive() {
        format!(
            "{}, {}–{} {}",
            s.format("%a %-d %b %Y"),
            s.format("%H:%M"),
            f.format("%H:%M"),
            tz.name()
        )
    } else {
        format!(
            "{} – {} {}",
            s.format("%a %-d %b %Y %H:%M"),
            f.format("%a %-d %b %Y %H:%M"),
            tz.name()
        )
    }
}

/// The e-mail: fixed plain-text template (the agent is named as the proposer; no HTML), the iTIP
/// part, and the same calendar as an attachment (the mailer builds the multipart).
fn compose(c: &Compose<'_>) -> Mail {
    let e = c.event;
    let agent = sanitize::one_line(c.agent_name, 80);
    let organizer = format!("{agent} (Aurora)");
    let proposed = format!("Proposed by {agent} in Aurora — {}", c.agent_link);
    let description = match e.description.as_deref().filter(|d| !d.trim().is_empty()) {
        Some(d) => format!("{proposed}\n\n{d}"),
        None => proposed,
    };
    let cal = ics::write(&Calendar {
        method: c.method,
        uid: c.uid,
        sequence: c.sequence,
        dtstamp: c.dtstamp,
        summary: &e.title,
        description: (c.method == Method::Request).then_some(description.as_str()),
        location: e.location.as_deref(),
        when: e.when.clone(),
        repeat: e.repeat.as_ref().map(RepeatSpec::to_repeat).as_ref(),
        reminder_minutes: e.reminder_minutes,
        organizer_name: &organizer,
        organizer_email: c.organizer_email,
        attendee_email: c.person_email,
    });
    let title = sanitize::one_line(&e.title, 100);
    let (subject, lead) = match c.method {
        Method::Request => (
            format!("Invitation: {title} — {}", when_text(e)),
            format!(
                "{agent} proposes this event. Accept it to add it to your calendar; Decline or ignore it otherwise."
            ),
        ),
        Method::Cancel => (
            format!("Cancelled: {title} — {}", when_text(e)),
            format!(
                "{agent} cancelled this event. Your calendar removes it when you accept the cancellation."
            ),
        ),
    };
    let mut body = format!(
        "{lead}\n\nWhat: {}\nWhen: {}\n",
        e.title.replace(['\r', '\n'], " "),
        when_text(e)
    );
    if let Some(r) = &e.repeat {
        body.push_str(&format!("Repeats: {}\n", r.describe()));
    }
    if let Some(l) = e.location.as_deref().filter(|l| !l.trim().is_empty()) {
        body.push_str(&format!("Where: {}\n", l.replace(['\r', '\n'], " ")));
    }
    if c.method == Method::Request
        && let Some(d) = e.description.as_deref().filter(|d| !d.trim().is_empty())
    {
        body.push_str(&format!("\n{d}\n"));
    }
    body.push_str(&format!(
        "\n--\nSent by Aurora ({}) on behalf of {agent}, who works for you. Agent: {}\n",
        c.instance_url, c.agent_link
    ));
    Mail {
        to: c.person_email.to_owned(),
        subject,
        link: c.agent_link.to_owned(),
        body,
        from_name: Some(organizer),
        calendar: Some(CalendarPart {
            method: c.method.as_str(),
            ics: cal,
        }),
    }
}

// ---------------------------------------------------------------------------------------------
// The person, the instance, the limits

struct Person {
    id: Uuid,
    email: String,
}

/// The person the turn runs for (SEC-108, now the shared C-1052 rule): the speaker of the
/// triggering message, or a schedule's creator, while active, a member and holding `can_use`. A
/// webhook, peer-agent or creator-less schedule turn has none → `no_person`. The address is the
/// verified account e-mail, never anything the agent wrote.
async fn person_of(ctx: &ToolCallContext) -> Result<Person, ToolError> {
    let no_person = || {
        ToolError::Coded(
            "no_person",
            "There is no person behind this turn to invite: calendar invitations go only to the person you are working for (a conversation with them, or their schedule).".into(),
        )
    };
    let mut conn = ctx
        .agent
        .pool
        .acquire()
        .await
        .map_err(aurora_db::DbError::from)?;
    let p = turn_person::of_turn_id(
        &mut conn,
        ctx.turn_id,
        ctx.conversation_id,
        Scope {
            agent_id: ctx.agent.agent_id,
            org_id: ctx.agent.org_id,
            workspace_id: ctx.agent.workspace_id,
        },
    )
    .await?
    .ok_or_else(no_person)?;
    if !p.email_verified {
        return Err(ToolError::Coded(
            "no_verified_email",
            "The person's account has no verified e-mail address, so no invitation can be sent."
                .into(),
        ));
    }
    Ok(Person {
        id: p.user_id,
        email: p.email,
    })
}

/// `email.from` as it is now (SET-4's live settings).
fn organizer_email(ctx: &ToolCallContext) -> Option<String> {
    ctx.state
        .settings
        .snapshot()
        .effective
        .raw
        .email
        .from
        .clone()
        .filter(|f| !f.trim().is_empty())
        .map(|f| {
            // `Name <addr>` or `addr`: the address only.
            f.rsplit_once('<')
                .map_or(f.as_str(), |(_, a)| a.trim_end_matches('>'))
                .trim()
                .to_owned()
        })
}

fn mail_unavailable() -> ToolError {
    ToolError::Coded(
        "mail_unavailable",
        "The invitation could not be sent (the instance mailer failed). Nothing was recorded; try again later.".into(),
    )
}

fn instance_host(external_url: &str) -> String {
    external_url
        .split("://")
        .last()
        .unwrap_or(external_url)
        .split(['/', ':'])
        .next()
        .filter(|h| !h.is_empty())
        .unwrap_or("aurora.invalid")
        .to_owned()
}

/// Refuses a send over the limits (20 per person per agent per hour, 60 per person per day).
async fn check_limits(
    conn: &mut sqlx::PgConnection,
    person: Uuid,
    agent: Uuid,
) -> Result<(), ToolError> {
    let w = repo::send_window(conn, person, agent).await?;
    let now = Utc::now();
    let mut retry: Option<i64> = None;
    let mut over = |count: i64, limit: i64, oldest: Option<DateTime<Utc>>, span: Duration| {
        if count >= limit {
            let secs = oldest.map_or(span, |o| o + span - now).num_seconds().max(1);
            retry = Some(retry.map_or(secs, |r| r.max(secs)));
        }
    };
    over(
        w.hour_count,
        CALENDAR_SENDS_PER_AGENT_HOUR,
        w.hour_oldest,
        Duration::hours(1),
    );
    over(
        w.day_count,
        CALENDAR_SENDS_PER_PERSON_DAY,
        w.day_oldest,
        Duration::days(1),
    );
    match retry {
        None => Ok(()),
        Some(secs) => Err(ToolError::Coded(
            "rate_limited",
            format!(
                "Too many calendar invitations to this person (at most {CALENDAR_SENDS_PER_AGENT_HOUR} per hour from you and {CALENDAR_SENDS_PER_PERSON_DAY} per day from all agents). Retry in {secs} seconds (retry_after_secs={secs})."
            ),
        )),
    }
}

/// Everything a send needs about the agent and the instance.
struct Sender {
    agent_id: Uuid,
    org_id: Uuid,
    workspace_id: Uuid,
    agent_name: String,
    agent_handle: String,
    agent_link: String,
    instance_url: String,
    host: String,
    organizer_email: String,
    default_tz: String,
}

async fn sender(ctx: &ToolCallContext) -> Result<Sender, ToolError> {
    let pool = &ctx.agent.pool;
    let agent = agents::get(pool, ctx.agent.agent_id)
        .await?
        .ok_or_else(|| ToolError::Failed("Unknown agent.".into()))?;
    let organizer = organizer_email(ctx).ok_or_else(mail_unavailable)?;
    let (org_slug, ws_slug, tz): (String, String, String) = sqlx::query_as(
        "SELECT o.slug, w.slug, w.timezone FROM workspaces w JOIN organizations o ON o.id = w.org_id WHERE w.id = $1",
    )
    .bind(agent.workspace_id)
    .fetch_one(pool)
    .await?;
    let instance_url = ctx
        .state
        .config
        .external_url
        .trim_end_matches('/')
        .to_owned();
    Ok(Sender {
        agent_link: format!(
            "{instance_url}/o/{org_slug}/w/{ws_slug}/agents/{}",
            agent.handle
        ),
        host: instance_host(&instance_url),
        instance_url,
        agent_id: agent.id,
        org_id: agent.org_id,
        workspace_id: agent.workspace_id,
        agent_name: agent.name,
        agent_handle: agent.handle,
        organizer_email: organizer,
        default_tz: if tz.is_empty() { "UTC".into() } else { tz },
    })
}

fn audit_event(ctx: &ToolCallContext, s: &Sender, action: &str, row: &InvitationRow) {
    audit::record(
        &ctx.state,
        AuditEvent::new(
            AuditActor {
                kind: "agent",
                id: Some(s.agent_id),
                label: s.agent_handle.clone(),
                source: "system",
                ip: None,
            },
            action,
            Outcome::Success,
        )
        .org(s.org_id)
        .workspace(s.workspace_id)
        .target("calendar_invitation", row.id)
        .metadata(json!({
            "agent_id": s.agent_id, "person_id": row.person_id,
            "conversation_id": row.conversation_id, "sequence": row.sequence,
        })),
    );
}

fn out(value: Value) -> Result<Vec<ToolContent>, ToolError> {
    Ok(vec![ToolContent::Json(JsonContent { value })])
}

fn parse_args<T: for<'de> Deserialize<'de>>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))
}

fn repeat_json(p: &Parsed) -> Option<Value> {
    p.repeat.as_ref().and_then(|r| serde_json::to_value(r).ok())
}

fn event_of<'a>(p: &'a Parsed, repeat: Option<&'a Value>) -> Event<'a> {
    Event {
        title: &p.title,
        starts_at: p.starts_at,
        ends_at: p.ends_at,
        timezone: &p.timezone,
        all_day: p.all_day,
        repeat,
    }
}

// ---------------------------------------------------------------------------------------------
// Tools

/// (P5-GUIDE-7, C-654) The family of every tool of this file (P5-CALINV).
const FAMILY: &str = "calendar";

macro_rules! common {
    ($name:expr, $desc:expr, $schema:expr) => {
        fn name(&self) -> &'static str {
            $name
        }
        fn family(&self) -> &'static str {
            FAMILY
        }
        fn description(&self, _: &ToolContext) -> String {
            $desc.into()
        }
        fn input_schema(&self) -> Value {
            $schema
        }
        fn available(&self, _: &ToolContext) -> bool {
            true
        }
        fn needs_mailer(&self) -> bool {
            true
        }
        fn unavailable_reason(&self) -> &'static str {
            "Calendar invitations are not available: the instance has no mailer configured, or the organization turned them off."
        }
    };
}

/// The org switch `calendar_invitations.enabled` (default on). A lookup failure turns the tools
/// off (fail closed).
async fn org_switch_on(ctx: &ToolContext) -> bool {
    sqlx::query_scalar::<_, Value>("SELECT settings FROM organizations WHERE id = $1")
        .bind(ctx.org_id)
        .fetch_optional(&ctx.pool)
        .await
        .ok()
        .flatten()
        .is_some_and(|s| OrgSettings::calendar_invitations_enabled(&s))
}

fn event_schema_props() -> Value {
    json!({
        "title": {"type": "string", "maxLength": TITLE_MAX},
        "start": {"type": "string", "description": "Local date-time, e.g. 2026-12-01T09:00:00 (a date, 2026-12-01, when all_day)."},
        "end": {"type": "string", "description": "Local date-time after start, at most 14 days later (all_day: the last day, inclusive)."},
        "timezone": {"type": "string", "description": "IANA zone, e.g. Europe/Zurich (default: the workspace's)."},
        "all_day": {"type": "boolean"},
        "location": {"type": "string", "maxLength": LOCATION_MAX},
        "description": {"type": "string", "maxLength": DESCRIPTION_MAX, "description": "Plain text."},
        "repeat": {"type": "object", "additionalProperties": false, "required": ["freq"],
            "description": "Needs exactly one of count and until.",
            "properties": {
                "freq": {"type": "string", "enum": ["daily", "weekly", "monthly", "yearly"]},
                "interval": {"type": "integer", "minimum": 1, "maximum": 12},
                "by_day": {"type": "array", "items": {"type": "string", "enum": ["MO", "TU", "WE", "TH", "FR", "SA", "SU"]}},
                "count": {"type": "integer", "minimum": 1, "maximum": 200},
                "until": {"type": "string", "description": "Last day, YYYY-MM-DD."}}},
        "reminder_minutes": {"type": "integer", "minimum": 0, "maximum": MAX_REMINDER_MINUTES}
    })
}

fn invite_schema(with_id: bool) -> Value {
    let mut props = event_schema_props();
    let mut required = vec![json!("title"), json!("start"), json!("end")];
    if with_id && let Some(o) = props.as_object_mut() {
        o.insert("id".into(), json!({"type": "string", "description": "The invitation id from calendar_invite or calendar_invite_list."}));
        required.insert(0, json!("id"));
    }
    json!({"type": "object", "additionalProperties": false, "required": required, "properties": props})
}

struct InviteT;

#[async_trait::async_trait]
impl PlatformTool for InviteT {
    common!(
        "calendar_invite",
        "Put an event in the person's calendar by e-mailing them a calendar invitation (they accept it \
         there). It goes only to the person you are working for, at their verified address; you cannot \
         invite anyone else. Needs start and end (local times) and optionally timezone, all_day, \
         location, description, repeat (count or until required) and reminder_minutes.",
        invite_schema(false)
    );
    async fn enabled(&self, ctx: &ToolContext) -> bool {
        org_switch_on(ctx).await
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let a: EventArgs = parse_args(args)?;
        let person = person_of(ctx).await?;
        let s = sender(ctx).await?;
        let ev = validate(a, &s.default_tz, Utc::now())?;
        let id = Uuid::now_v7();
        let uid = format!("{id}@{}", s.host);
        let repeat = repeat_json(&ev);
        let mut tx = ctx.agent.pool.begin().await?;
        repo::lock_person(&mut tx, person.id).await?;
        check_limits(&mut tx, person.id, s.agent_id).await?;
        repo::record_send(&mut tx, s.org_id, s.agent_id, person.id).await?;
        let row = repo::insert(
            &mut tx,
            id,
            &uid,
            &Owner {
                org_id: s.org_id,
                workspace_id: s.workspace_id,
                agent_id: s.agent_id,
                person_id: person.id,
                conversation_id: Some(ctx.conversation_id),
            },
            &event_of(&ev, repeat.as_ref()),
        )
        .await?;
        deliver(ctx, &s, &person, &row, &ev, Method::Request).await?;
        tx.commit().await?;
        audit_event(ctx, &s, "calendar.invitation.sent", &row);
        out(json!({"id": id, "status": "sent"}))
    }
}

/// Sends the mail while the transaction is open: a mailer failure rolls the row and the ledger
/// entry back (nothing is recorded as sent).
async fn deliver(
    ctx: &ToolCallContext,
    s: &Sender,
    person: &Person,
    row: &InvitationRow,
    ev: &Parsed,
    method: Method,
) -> Result<(), ToolError> {
    let mail = compose(&Compose {
        method,
        uid: &row.uid,
        sequence: u32::try_from(row.sequence).unwrap_or(0),
        dtstamp: Utc::now(),
        event: ev,
        agent_name: &s.agent_name,
        agent_link: &s.agent_link,
        instance_url: &s.instance_url,
        organizer_email: &s.organizer_email,
        person_email: &person.email,
    });
    ctx.state.mailer.send(mail).await.map_err(|e| {
        tracing::warn!(error = %e, "calendar invitation mail failed");
        mail_unavailable()
    })
}

fn not_found() -> ToolError {
    ToolError::Coded("not_found", "Invitation not found.".into())
}

fn invitation_id(args: &mut Value) -> Result<Uuid, ToolError> {
    let id = args
        .as_object_mut()
        .and_then(|o| o.remove("id"))
        .ok_or_else(|| invalid("id", "is required"))?;
    id.as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| invalid("id", "must be an invitation id"))
}

struct UpdateT;

#[async_trait::async_trait]
impl PlatformTool for UpdateT {
    common!(
        "calendar_invite_update",
        "Change an invitation you sent: send the full event again (same fields as calendar_invite) \
         with the invitation id. The person's calendar updates the event and may ask them to accept again.",
        invite_schema(true)
    );
    async fn enabled(&self, ctx: &ToolContext) -> bool {
        org_switch_on(ctx).await
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        mut args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let id = invitation_id(&mut args)?;
        let a: EventArgs = parse_args(args)?;
        let person = person_of(ctx).await?;
        let s = sender(ctx).await?;
        let ev = validate(a, &s.default_tz, Utc::now())?;
        let repeat = repeat_json(&ev);
        let mut tx = ctx.agent.pool.begin().await?;
        repo::lock_person(&mut tx, person.id).await?;
        let cur = repo::get_own_for_update(&mut tx, id, s.agent_id, person.id)
            .await?
            .ok_or_else(not_found)?;
        if cur.status == "cancelled" {
            return Err(ToolError::Failed(
                "This invitation was cancelled; send a new one with calendar_invite.".into(),
            ));
        }
        check_limits(&mut tx, person.id, s.agent_id).await?;
        repo::record_send(&mut tx, s.org_id, s.agent_id, person.id).await?;
        let row = repo::update_event(&mut tx, id, &event_of(&ev, repeat.as_ref())).await?;
        deliver(ctx, &s, &person, &row, &ev, Method::Request).await?;
        tx.commit().await?;
        audit_event(ctx, &s, "calendar.invitation.updated", &row);
        out(json!({"id": id, "status": "sent"}))
    }
}

struct CancelT;

#[async_trait::async_trait]
impl PlatformTool for CancelT {
    common!(
        "calendar_invite_cancel",
        "Cancel an invitation you sent (the id from calendar_invite or calendar_invite_list): the \
         person's calendar removes the event.",
        json!({"type": "object", "additionalProperties": false, "required": ["id"],
            "properties": {"id": {"type": "string"}}})
    );
    async fn enabled(&self, ctx: &ToolContext) -> bool {
        org_switch_on(ctx).await
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        mut args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        let id = invitation_id(&mut args)?;
        if args.as_object().is_some_and(|o| !o.is_empty()) {
            return Err(ToolError::InvalidArguments("Only `id` is accepted.".into()));
        }
        let person = person_of(ctx).await?;
        let s = sender(ctx).await?;
        let mut tx = ctx.agent.pool.begin().await?;
        repo::lock_person(&mut tx, person.id).await?;
        let cur = repo::get_own_for_update(&mut tx, id, s.agent_id, person.id)
            .await?
            .ok_or_else(not_found)?;
        if cur.status == "cancelled" {
            return Err(ToolError::Failed(
                "This invitation is already cancelled.".into(),
            ));
        }
        check_limits(&mut tx, person.id, s.agent_id).await?;
        repo::record_send(&mut tx, s.org_id, s.agent_id, person.id).await?;
        let row = repo::cancel(&mut tx, id).await?;
        deliver(ctx, &s, &person, &row, &from_row(&row), Method::Cancel).await?;
        tx.commit().await?;
        audit_event(ctx, &s, "calendar.invitation.cancelled", &row);
        out(json!({"id": id, "status": "cancelled"}))
    }
}

struct ListT;

#[async_trait::async_trait]
impl PlatformTool for ListT {
    common!(
        "calendar_invite_list",
        "List the calendar invitations you sent to the person you are working for (newest first, at most 50).",
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    );
    async fn enabled(&self, ctx: &ToolContext) -> bool {
        org_switch_on(ctx).await
    }
    async fn call(
        &self,
        ctx: &ToolCallContext,
        args: Value,
    ) -> Result<Vec<ToolContent>, ToolError> {
        match &args {
            Value::Null => {}
            Value::Object(m) if m.is_empty() => {}
            _ => {
                return Err(ToolError::InvalidArguments(
                    "This tool takes no arguments.".into(),
                ));
            }
        }
        let person = person_of(ctx).await?;
        let mut conn = ctx.agent.pool.acquire().await?;
        let rows = repo::list(&mut conn, ctx.agent.agent_id, person.id, LIST_MAX).await?;
        let items: Vec<Value> = rows
            .iter()
            .map(|r| {
                let tz: Tz = r.timezone.parse().unwrap_or(Tz::UTC);
                let local = |t: DateTime<Utc>| {
                    if r.all_day {
                        t.date_naive().to_string()
                    } else {
                        t.with_timezone(&tz)
                            .naive_local()
                            .format("%Y-%m-%dT%H:%M:%S")
                            .to_string()
                    }
                };
                json!({"id": r.id, "title": r.title, "start": local(r.starts_at),
                    "end": local(r.ends_at), "timezone": r.timezone, "all_day": r.all_day,
                    "status": r.status})
            })
            .collect();
        out(json!({"items": items}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 18, 10, 0, 0).unwrap()
    }

    fn args(v: Value) -> EventArgs {
        serde_json::from_value(v).unwrap()
    }

    fn ok(v: Value) -> Parsed {
        validate(args(v), "Europe/Zurich", now()).unwrap()
    }

    fn err(v: Value) -> String {
        match validate(args(v), "Europe/Zurich", now()) {
            Err(ToolError::InvalidArguments(m)) => m,
            other => panic!("expected invalid arguments, got {other:?}"),
        }
    }

    fn base() -> Value {
        json!({"title": "Review", "start": "2026-12-01T09:00:00", "end": "2026-12-01T10:00:00"})
    }

    fn with(k: &str, v: Value) -> Value {
        let mut b = base();
        b[k] = v;
        b
    }

    #[test]
    fn a_timed_event_uses_the_default_zone_and_stores_utc() {
        let e = ok(base());
        assert_eq!(e.timezone, "Europe/Zurich");
        assert_eq!(
            e.starts_at,
            Utc.with_ymd_and_hms(2026, 12, 1, 8, 0, 0).unwrap()
        );
        assert!(matches!(e.when, When::Utc { .. }));
        let e = ok(with("timezone", json!("America/New_York")));
        assert_eq!(
            e.starts_at,
            Utc.with_ymd_and_hms(2026, 12, 1, 14, 0, 0).unwrap()
        );
        // An RFC 3339 value with an offset keeps its instant.
        let e = ok(
            json!({"title": "x", "start": "2026-12-01T08:00:00Z", "end": "2026-12-01T09:00:00Z"}),
        );
        assert_eq!(
            e.starts_at,
            Utc.with_ymd_and_hms(2026, 12, 1, 8, 0, 0).unwrap()
        );
    }

    #[test]
    fn a_repeating_event_is_zoned_and_an_all_day_event_is_dates() {
        let r = json!({"freq": "weekly", "by_day": ["mo"], "count": 10});
        let e = ok(with("repeat", r));
        assert!(matches!(e.when, When::Zoned { .. }));
        assert_eq!(e.repeat.unwrap().by_day, vec!["MO"]);
        let e = ok(
            json!({"title": "Offsite", "start": "2026-12-01", "end": "2026-12-03", "all_day": true}),
        );
        assert!(matches!(e.when, When::Day { .. }));
        assert_eq!(
            e.ends_at,
            Utc.with_ymd_and_hms(2026, 12, 3, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn every_bound_is_enforced_and_names_its_field() {
        assert!(err(with("title", json!(""))).starts_with("title:"));
        assert!(err(with("title", json!("x".repeat(201)))).starts_with("title:"));
        ok(with("title", json!("x".repeat(200))));
        assert!(err(with("location", json!("x".repeat(201)))).starts_with("location:"));
        assert!(err(with("description", json!("x".repeat(4001)))).starts_with("description:"));
        ok(with("description", json!("x".repeat(4000))));
        assert!(err(with("timezone", json!("Mars/Base"))).starts_with("timezone:"));
        assert!(err(with("start", json!("tomorrow"))).starts_with("start:"));
        assert!(
            err(with("end", json!("2026-12-01T09:00:00"))).starts_with("end:"),
            "end = start"
        );
        assert!(err(with("end", json!("2026-12-01T08:00:00"))).starts_with("end:"));
        ok(with("end", json!("2026-12-15T09:00:00")));
        assert!(
            err(with("end", json!("2026-12-15T09:00:01"))).starts_with("end:"),
            "over 14 days"
        );
        assert!(err(with("reminder_minutes", json!(10_081))).starts_with("reminder_minutes:"));
        ok(with("reminder_minutes", json!(10_080)));
        ok(with("reminder_minutes", json!(0)));
        // Within 5 years of now.
        let far =
            json!({"title": "x", "start": "2032-01-01T09:00:00", "end": "2032-01-01T10:00:00"});
        assert!(err(far).starts_with("start:"));
        // A daylight-saving gap: 02:30 does not exist in Zurich on 2027-03-28.
        let gap =
            json!({"title": "x", "start": "2027-03-28T02:30:00", "end": "2027-03-28T04:00:00"});
        assert!(err(gap).starts_with("start:"));
        // All day: dates only, end not before start, at most 14 days.
        let d = |s: &str, e: &str| json!({"title": "x", "start": s, "end": e, "all_day": true});
        assert!(err(d("2026-12-01T09:00:00", "2026-12-02")).starts_with("start:"));
        assert!(err(d("2026-12-02", "2026-12-01")).starts_with("end:"));
        ok(d("2026-12-01", "2026-12-01"));
        ok(d("2026-12-01", "2026-12-15"));
        assert!(err(d("2026-12-01", "2026-12-16")).starts_with("end:"));
    }

    #[test]
    fn repeat_bounds() {
        let rep = |r: Value| err(with("repeat", r));
        assert!(rep(json!({"freq": "hourly", "count": 2})).starts_with("repeat.freq"));
        assert!(
            rep(json!({"freq": "daily", "count": 2, "interval": 0})).starts_with("repeat.interval")
        );
        assert!(
            rep(json!({"freq": "daily", "count": 2, "interval": 13}))
                .starts_with("repeat.interval")
        );
        assert!(
            rep(json!({"freq": "daily"})).starts_with("repeat:"),
            "one of count/until is required"
        );
        assert!(
            rep(json!({"freq": "daily", "count": 2, "until": "2027-01-01"})).starts_with("repeat:"),
            "not both"
        );
        assert!(rep(json!({"freq": "daily", "count": 0})).starts_with("repeat.count"));
        assert!(rep(json!({"freq": "daily", "count": 201})).starts_with("repeat.count"));
        assert!(
            rep(json!({"freq": "weekly", "count": 2, "by_day": ["XX"]}))
                .starts_with("repeat.by_day")
        );
        assert!(
            rep(json!({"freq": "daily", "count": 2, "by_day": ["MO"]}))
                .starts_with("repeat.by_day")
        );
        assert!(
            rep(json!({"freq": "daily", "until": "2026-11-30"})).starts_with("repeat.until"),
            "before start"
        );
        assert!(rep(json!({"freq": "daily", "until": "2037-01-01"})).starts_with("repeat.until"));
        ok(with(
            "repeat",
            json!({"freq": "yearly", "interval": 12, "count": 200}),
        ));
        ok(with(
            "repeat",
            json!({"freq": "weekly", "until": "2026-12-01"}),
        ));
        // Unknown fields are refused (no attendee-like extras).
        assert!(serde_json::from_value::<EventArgs>(with("attendees", json!(["a@b"]))).is_err());
        assert!(serde_json::from_value::<EventArgs>(with("to", json!("a@b"))).is_err());
    }

    fn compose_for(ev: &Parsed, method: Method, sequence: u32) -> Mail {
        compose(&Compose {
            method,
            uid: "id@aurora.example.org",
            sequence,
            dtstamp: now(),
            event: ev,
            agent_name: "Ada",
            agent_link: "https://aurora.example.org/o/acme/w/main/agents/ada",
            instance_url: "https://aurora.example.org",
            organizer_email: "aurora@example.org",
            person_email: "grace@example.org",
        })
    }

    #[test]
    fn the_message_is_a_fixed_template_naming_the_agent() {
        let mut ev = ok(with("location", json!("Room 4\nBcc: x@y")));
        ev.description = Some("Agenda\r\nATTENDEE:mailto:x@y".into());
        let m = compose_for(&ev, Method::Request, 0);
        assert_eq!(m.to, "grace@example.org");
        assert_eq!(m.from_name.as_deref(), Some("Ada (Aurora)"));
        assert!(
            m.subject
                .starts_with("Invitation: Review — Tue 1 Dec 2026, 09:00–10:00 Europe/Zurich"),
            "{}",
            m.subject
        );
        assert!(m.body.starts_with(
            "Ada proposes this event. Accept it to add it to your calendar; Decline or ignore it otherwise."
        ));
        assert!(m.body.contains("Where: Room 4 Bcc: x@y"), "{}", m.body);
        assert!(
            m.body
                .contains("https://aurora.example.org/o/acme/w/main/agents/ada")
        );
        assert!(!m.body.contains('<'), "no HTML: {}", m.body);
        let c = m.calendar.unwrap();
        assert_eq!(c.method, "REQUEST");
        let flat = c.ics.replace("\r\n ", "");
        assert!(flat.contains("DESCRIPTION:Proposed by Ada in Aurora — https://aurora.example.org/o/acme/w/main/agents/ada\\n\\nAgenda\\nATTENDEE:mailto:x@y\r\n"), "{flat}");
        assert_eq!(
            flat.lines().filter(|l| l.starts_with("ATTENDEE")).count(),
            1,
            "{flat}"
        );
    }

    #[test]
    fn a_cancel_keeps_the_uid_and_raises_the_sequence() {
        let ev = ok(with("repeat", json!({"freq": "weekly", "count": 3})));
        let row_event = Parsed {
            description: None,
            location: None,
            reminder_minutes: None,
            ..ev
        };
        let m = compose_for(&row_event, Method::Cancel, 2);
        assert!(m.subject.starts_with("Cancelled: Review"));
        let c = m.calendar.unwrap();
        assert_eq!(c.method, "CANCEL");
        assert!(c.ics.contains("METHOD:CANCEL\r\n") && c.ics.contains("SEQUENCE:2\r\n"));
        assert!(c.ics.contains("UID:id@aurora.example.org\r\n"));
    }

    #[test]
    fn instance_host_is_the_external_url_host() {
        assert_eq!(
            instance_host("https://aurora.example.org"),
            "aurora.example.org"
        );
        assert_eq!(instance_host("http://localhost:4100/x"), "localhost");
        assert_eq!(instance_host(""), "aurora.invalid");
    }

    #[test]
    fn repeats_are_described_for_people() {
        let r = |f: &str, i: u32, d: &[&str], c: Option<u32>| RepeatSpec {
            freq: f.into(),
            interval: i,
            by_day: d.iter().map(|s| (*s).to_owned()).collect(),
            count: c,
            until: None,
        };
        assert_eq!(
            r("weekly", 1, &["MO", "WE"], Some(10)).describe(),
            "weekly on MO, WE, 10 times"
        );
        assert_eq!(
            r("daily", 2, &[], Some(3)).describe(),
            "every 2 days, 3 times"
        );
        assert_eq!(
            r("monthly", 3, &[], Some(4)).describe(),
            "every 3 months, 4 times"
        );
    }

    #[test]
    fn a_stored_row_rebuilds_its_event() {
        let ev = ok(with(
            "repeat",
            json!({"freq": "weekly", "by_day": ["MO"], "count": 3}),
        ));
        let now = Utc::now();
        let row = InvitationRow {
            id: Uuid::nil(),
            org_id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            agent_id: Uuid::nil(),
            person_id: Uuid::nil(),
            conversation_id: None,
            uid: "u".into(),
            sequence: 1,
            status: "sent".into(),
            title: ev.title.clone(),
            starts_at: ev.starts_at,
            ends_at: ev.ends_at,
            timezone: ev.timezone.clone(),
            all_day: false,
            repeat: repeat_json(&ev),
            created_at: now,
            updated_at: now,
        };
        let back = from_row(&row);
        assert_eq!(back.when, ev.when);
        assert_eq!(back.repeat, ev.repeat);
    }
}
