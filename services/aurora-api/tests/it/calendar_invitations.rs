#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! (P5-CALINV, D-274, C-844, SEC-108) Calendar-invitation tools against a captured mailer.

use crate::common::*;
use aurora_api::domains::auth::mail::{Mail, MailError, Mailer};
use aurora_api::domains::tools::{ToolCallContext, ToolContext, catalogue, registry};
use aurora_protocol::tools::{ToolContent, ToolResult};
use chrono::{Datelike, NaiveDate, TimeDelta, TimeZone, Utc, Weekday};
use serde_json::{Value, json};
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use uuid::Uuid;

const EMAIL: &str =
    "[email]\nsmtp_url = \"smtp://127.0.0.1:2525\"\nfrom = \"Aurora <aurora@example.org>\"\n";
const NAMES: [&str; 4] = [
    "calendar_invite",
    "calendar_invite_update",
    "calendar_invite_cancel",
    "calendar_invite_list",
];

struct W {
    h: Harness,
    org: Uuid,
    ws: Uuid,
    agent: Uuid,
    person: Uuid,
}

async fn world(extra: &str) -> W {
    let h = if extra.is_empty() {
        Harness::new().await
    } else {
        Harness::new().await.with_helm(extra)
    };
    let org = h.org().await;
    let ws = h.workspace(org).await;
    let person = verified_member(&h, org, ws, "grace@example.org").await;
    let agent = new_agent(&h, org, ws, "ada").await;
    W {
        h,
        org,
        ws,
        agent,
        person,
    }
}

/// A verified, active org member with `can_use` on `ws` (P5-PERSON, C-1052: the turn's person
/// needs `can_use` of the agent's workspace).
async fn verified_member(h: &Harness, org: Uuid, ws: Uuid, email: &str) -> Uuid {
    let u = h.user(email, false).await;
    sqlx::query("UPDATE users SET email_verified_at = now() WHERE id = $1")
        .bind(u)
        .execute(h.pool())
        .await
        .unwrap();
    h.member(org, u, "member", "active").await;
    h.bind_ws(ws, u, "member").await;
    u
}

async fn new_agent(h: &Harness, org: Uuid, ws: Uuid, handle: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agents (id, org_id, workspace_id, handle, name, kind, harness)
         VALUES ($1,$2,$3,$4,$5,'hosted','fake')",
    )
    .bind(id)
    .bind(org)
    .bind(ws)
    .bind(handle)
    .bind(format!("Agent {handle}"))
    .execute(h.pool())
    .await
    .unwrap();
    id
}

impl W {
    /// A running turn of `agent` in a fresh conversation of `kind`.
    async fn turn(
        &self,
        agent: Uuid,
        kind: &str,
        speaker: Option<Uuid>,
        door: Value,
    ) -> ToolCallContext {
        let (conv, turn) = (Uuid::now_v7(), Uuid::now_v7());
        sqlx::query(
            "INSERT INTO conversations (id, agent_id, workspace_id, kind, external_ref, last_activity_at)
             VALUES ($1,$2,$3,$4,$1::text, now())",
        )
        .bind(conv)
        .bind(agent)
        .bind(self.ws)
        .bind(kind)
        .execute(self.h.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO turns (id, conversation_id, agent_id, role, status, door, speaker_kind, speaker_user_id, speaker_label, text)
             VALUES ($1,$2,$3,'turn','running',$4,$5,$6,'s','hi')",
        )
        .bind(turn)
        .bind(conv)
        .bind(agent)
        .bind(door)
        .bind(if speaker.is_some() { "person" } else { "system" })
        .bind(speaker)
        .execute(self.h.pool())
        .await
        .unwrap();
        ToolCallContext {
            agent: ToolContext {
                pool: self.h.pool().clone(),
                agent_id: agent,
                workspace_id: self.ws,
                org_id: self.org,
            },
            conversation_id: conv,
            turn_id: Some(turn),
            call_id: "c1".into(),
            state: self.h.state.clone(),
        }
    }

    /// The person's own conversation turn.
    async fn person_turn(&self) -> ToolCallContext {
        self.turn(self.agent, "api", Some(self.person), json!({"kind": "api"}))
            .await
    }
}

async fn run(ctx: &ToolCallContext, tool: &str, args: Value) -> ToolResult {
    registry()
        .call(ctx, tool, args, Duration::from_secs(10))
        .await
}

fn ok(r: &ToolResult) -> Value {
    assert!(r.ok, "{:?}", r.error);
    match &r.content.as_ref().unwrap()[0] {
        ToolContent::Json(j) => j.value.clone(),
        other => panic!("{other:?}"),
    }
}

fn code(r: &ToolResult) -> String {
    assert!(!r.ok);
    r.error.as_ref().unwrap().code.as_str().to_owned()
}

fn event() -> Value {
    json!({"title": "Design review", "start": local(0, 9), "end": local(0, 10),
           "timezone": "Europe/Zurich", "location": "Room 4", "description": "Bring notes",
           "reminder_minutes": 15})
}

/// The day of `event()`: the first Tuesday at least two weeks from now. The tools only accept an
/// event within five years of now. Fixed once per process, so every use in a test agrees.
static EVENT_DAY: LazyLock<NaiveDate> = LazyLock::new(|| {
    let mut day = Utc::now().date_naive() + TimeDelta::days(14);
    while day.weekday() != Weekday::Tue {
        day += TimeDelta::days(1);
    }
    day
});

fn day(offset: i64) -> NaiveDate {
    *EVENT_DAY + TimeDelta::days(offset)
}

/// `hour:00` local time, `offset` days after the event's day, as the tools take it.
fn local(offset: i64, hour: u32) -> String {
    format!("{}T{hour:02}:00:00", day(offset))
}

/// The UTC `DTSTART` line of 09:00 in Zurich, `offset` days after the event's day.
fn dtstart_utc(offset: i64) -> String {
    let at = chrono_tz::Europe::Zurich
        .from_local_datetime(&day(offset).and_hms_opt(9, 0, 0).unwrap())
        .unwrap()
        .with_timezone(&Utc);
    format!("DTSTART:{}", at.format("%Y%m%dT%H%M%SZ"))
}

fn with(mut v: Value, k: &str, x: Value) -> Value {
    v[k] = x;
    v
}

fn ics_of(m: &Mail) -> String {
    m.calendar.as_ref().unwrap().ics.replace("\r\n ", "")
}

/// The first `name` property outside any VTIMEZONE.
fn prop(ics: &str, name: &str) -> String {
    let mut in_tz = false;
    ics.lines()
        .filter(|l| {
            match *l {
                "BEGIN:VTIMEZONE" => in_tz = true,
                "END:VTIMEZONE" => {
                    in_tz = false;
                    return false;
                }
                _ => {}
            }
            !in_tz
        })
        .find(|l| l.starts_with(&format!("{name}:")) || l.starts_with(&format!("{name};")))
        .unwrap_or_else(|| panic!("no {name} in {ics}"))
        .to_owned()
}

async fn count(w: &W, sql: &'static str, id: Uuid) -> i64 {
    sqlx::query_scalar(sql)
        .bind(id)
        .fetch_one(w.h.pool())
        .await
        .unwrap()
}

async fn wait_audit(w: &W, action: &str) -> Value {
    for _ in 0..200 {
        let row: Option<Value> = sqlx::query_scalar(
            "SELECT metadata FROM audit_log WHERE action = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(action)
        .fetch_optional(w.h.pool())
        .await
        .unwrap();
        if let Some(r) = row {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("no audit row for {action}");
}

/// The whole life of an invitation: one message to the person's verified address, the fixed
/// preface, an inline iTIP part and the attachment, an update with the same UID and SEQUENCE + 1,
/// a cancel, and a list that shows only this agent's invitations to this person.
#[tokio::test]
async fn invite_update_cancel_and_list() {
    let w = world(EMAIL).await;
    let ctx = w.person_turn().await;
    let v = ok(&run(&ctx, "calendar_invite", event()).await);
    assert_eq!(v["status"], "sent");
    let id: Uuid = v["id"].as_str().unwrap().parse().unwrap();

    let sent = w.h.mail.sent();
    assert_eq!(sent.len(), 1);
    let m = &sent[0];
    assert_eq!(m.to, "grace@example.org");
    assert_eq!(m.from_name.as_deref(), Some("Agent ada (Aurora)"));
    let subject = format!(
        "Invitation: Design review — {}, 09:00–10:00 Europe/Zurich",
        day(0).format("%a %-d %b %Y")
    );
    assert!(m.subject.starts_with(&subject), "{}", m.subject);
    assert!(m.body.starts_with("Agent ada proposes this event. Accept it to add it to your calendar; Decline or ignore it otherwise."), "{}", m.body);
    let cal = m.calendar.as_ref().unwrap();
    assert_eq!(cal.method, "REQUEST");
    let ics = ics_of(m);
    assert!(
        prop(&ics, "UID").starts_with(&format!("UID:{id}@")),
        "{ics}"
    );
    assert_eq!(prop(&ics, "SEQUENCE"), "SEQUENCE:0");
    assert_eq!(prop(&ics, "METHOD"), "METHOD:REQUEST");
    assert_eq!(prop(&ics, "DTSTART"), dtstart_utc(0));
    assert_eq!(
        prop(&ics, "ORGANIZER"),
        "ORGANIZER;CN=\"Agent ada (Aurora)\":mailto:aurora@example.org"
    );
    assert_eq!(
        prop(&ics, "ATTENDEE"),
        "ATTENDEE;RSVP=FALSE;PARTSTAT=NEEDS-ACTION:mailto:grace@example.org"
    );
    assert!(ics.contains("TRIGGER:-PT15M"), "{ics}");
    assert!(
        prop(&ics, "DESCRIPTION").starts_with("DESCRIPTION:Proposed by Agent ada in Aurora — http"),
        "{ics}"
    );
    let uid = prop(&ics, "UID");

    // The stored row has the times and the UID, but not the description or the location.
    let (title, seq, status, stored_uid): (String, i32, String, String) = sqlx::query_as(
        "SELECT title, sequence, status, uid FROM calendar_invitations WHERE id = $1",
    )
    .bind(id)
    .fetch_one(w.h.pool())
    .await
    .unwrap();
    assert_eq!(
        (title.as_str(), seq, status.as_str()),
        ("Design review", 0, "sent")
    );
    assert_eq!(format!("UID:{stored_uid}"), uid);
    let dump: String =
        sqlx::query_scalar("SELECT row_to_json(c)::text FROM calendar_invitations c WHERE id = $1")
            .bind(id)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert!(
        !dump.contains("Room 4") && !dump.contains("Bring notes"),
        "{dump}"
    );

    // Update: same UID, SEQUENCE + 1, the new time.
    let upd = with(
        with(with(event(), "id", json!(id)), "start", json!(local(1, 9))),
        "end",
        json!(local(1, 10)),
    );
    let v = ok(&run(&ctx, "calendar_invite_update", upd).await);
    assert_eq!(
        (v["id"].as_str().unwrap(), v["status"].as_str().unwrap()),
        (id.to_string().as_str(), "sent")
    );
    let sent = w.h.mail.sent();
    assert_eq!(sent.len(), 2);
    let ics2 = ics_of(&sent[1]);
    assert_eq!(prop(&ics2, "UID"), uid);
    assert_eq!(prop(&ics2, "SEQUENCE"), "SEQUENCE:1");
    assert_eq!(prop(&ics2, "DTSTART"), dtstart_utc(1));
    assert_eq!(sent[1].calendar.as_ref().unwrap().method, "REQUEST");

    // List: this agent's invitations to this person.
    let l = ok(&run(&ctx, "calendar_invite_list", json!({})).await);
    let items = l["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], json!(id));
    assert_eq!(items[0]["start"], local(1, 9));
    assert_eq!(items[0]["timezone"], "Europe/Zurich");
    assert_eq!(items[0]["status"], "sent");

    // Cancel: METHOD:CANCEL, same UID, SEQUENCE 2.
    let v = ok(&run(&ctx, "calendar_invite_cancel", json!({"id": id})).await);
    assert_eq!(v["status"], "cancelled");
    let sent = w.h.mail.sent();
    assert_eq!(sent.len(), 3);
    let ics3 = ics_of(&sent[2]);
    assert_eq!(sent[2].calendar.as_ref().unwrap().method, "CANCEL");
    assert_eq!(prop(&ics3, "METHOD"), "METHOD:CANCEL");
    assert_eq!(prop(&ics3, "UID"), uid);
    assert_eq!(prop(&ics3, "SEQUENCE"), "SEQUENCE:2");
    assert_eq!(prop(&ics3, "STATUS"), "STATUS:CANCELLED");
    // A cancelled invitation cannot be updated or cancelled again.
    assert!(
        !run(&ctx, "calendar_invite_cancel", json!({"id": id}))
            .await
            .ok
    );
    assert!(
        !run(
            &ctx,
            "calendar_invite_update",
            with(event(), "id", json!(id))
        )
        .await
        .ok
    );
    let l = ok(&run(&ctx, "calendar_invite_list", json!({})).await);
    assert_eq!(l["items"][0]["status"], "cancelled");

    // Every message went to the one verified address.
    assert!(w.h.mail.sent().iter().all(|m| m.to == "grace@example.org"));

    // Audit: ids only (no title, no address).
    let meta = wait_audit(&w, "calendar.invitation.cancelled").await;
    assert_eq!(meta["agent_id"], json!(w.agent));
    assert_eq!(meta["person_id"], json!(w.person));
    assert_eq!(meta["sequence"], 2);
    let text = meta.to_string();
    assert!(
        !text.contains("Design review") && !text.contains("grace@") && !text.contains("Room 4"),
        "{text}"
    );
    wait_audit(&w, "calendar.invitation.sent").await;
    wait_audit(&w, "calendar.invitation.updated").await;
}

/// Another agent's invitation, and another person's: `not_found`; `list` shows only this agent's.
#[tokio::test]
async fn other_agents_and_people_cannot_reach_an_invitation() {
    let w = world(EMAIL).await;
    let ctx = w.person_turn().await;
    let id: Uuid = ok(&run(&ctx, "calendar_invite", event()).await)["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let other = new_agent(&w.h, w.org, w.ws, "bob").await;
    let octx = w
        .turn(other, "api", Some(w.person), json!({"kind": "api"}))
        .await;
    for (tool, args) in [
        ("calendar_invite_cancel", json!({"id": id})),
        ("calendar_invite_update", with(event(), "id", json!(id))),
    ] {
        let r = run(&octx, tool, args).await;
        assert_eq!(code(&r), "not_found", "{tool}");
    }
    assert_eq!(
        ok(&run(&octx, "calendar_invite_list", json!({})).await)["items"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    // The same agent talking to another person cannot reach it either.
    let p2 = verified_member(&w.h, w.org, w.ws, "linus@example.org").await;
    let pctx = w
        .turn(w.agent, "api", Some(p2), json!({"kind": "api"}))
        .await;
    assert_eq!(
        code(&run(&pctx, "calendar_invite_cancel", json!({"id": id})).await),
        "not_found"
    );
    assert_eq!(
        ok(&run(&pctx, "calendar_invite_list", json!({})).await)["items"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    // Nothing changed: still one message, still `sent`.
    assert_eq!(w.h.mail.sent().len(), 1);
    assert_eq!(
        count(
            &w,
            "SELECT sequence::bigint FROM calendar_invitations WHERE id = $1",
            id
        )
        .await,
        0
    );
}

/// Refusals: a webhook turn, an unverified or missing person, no address argument.
#[tokio::test]
async fn refusals() {
    let w = world(EMAIL).await;
    // A webhook turn has no person.
    let hook = w
        .turn(
            w.agent,
            "webhook",
            None,
            json!({"kind": "webhook", "ref": Uuid::now_v7().to_string()}),
        )
        .await;
    assert_eq!(
        code(&run(&hook, "calendar_invite", event()).await),
        "no_person"
    );
    assert_eq!(
        code(&run(&hook, "calendar_invite_list", json!({})).await),
        "no_person"
    );
    // A peer-agent turn has none either.
    let peer = w.turn(w.agent, "peer", None, json!({"kind": "peer"})).await;
    assert_eq!(
        code(&run(&peer, "calendar_invite", event()).await),
        "no_person"
    );

    // A person whose e-mail is not verified.
    let u = w.h.user("unverified@example.org", false).await;
    w.h.member(w.org, u, "member", "active").await;
    w.h.bind_ws(w.ws, u, "member").await;
    let ctx = w
        .turn(w.agent, "api", Some(u), json!({"kind": "api"}))
        .await;
    assert_eq!(
        code(&run(&ctx, "calendar_invite", event()).await),
        "no_verified_email"
    );

    // A disabled person and one who left the org.
    let d = verified_member(&w.h, w.org, w.ws, "gone@example.org").await;
    let dctx = w
        .turn(w.agent, "api", Some(d), json!({"kind": "api"}))
        .await;
    w.h.set_member_status(w.org, d, "suspended").await;
    assert_eq!(
        code(&run(&dctx, "calendar_invite", event()).await),
        "no_person"
    );

    // (P5-PERSON, C-1052) An active, verified org member without `can_use` on the agent's
    // workspace is not the turn's person either (stricter than SEC-108 alone).
    let o = w.h.user("outsider@example.org", false).await;
    sqlx::query("UPDATE users SET email_verified_at = now() WHERE id = $1")
        .bind(o)
        .execute(w.h.pool())
        .await
        .unwrap();
    w.h.member(w.org, o, "member", "active").await;
    let octx = w
        .turn(w.agent, "api", Some(o), json!({"kind": "api"}))
        .await;
    assert_eq!(
        code(&run(&octx, "calendar_invite", event()).await),
        "no_person"
    );

    // There is no argument that names a recipient.
    let pctx = w.person_turn().await;
    for k in ["to", "attendees", "attendee", "email", "recipient", "cc"] {
        let r = run(
            &pctx,
            "calendar_invite",
            with(event(), k, json!("evil@example.org")),
        )
        .await;
        assert_eq!(code(&r), "invalid_arguments", "{k}");
    }
    // Invalid arguments name the field and send nothing.
    let r = run(
        &pctx,
        "calendar_invite",
        with(event(), "end", json!(local(0, 8))),
    )
    .await;
    assert_eq!(code(&r), "invalid_arguments");
    assert!(r.error.unwrap().message.starts_with("end:"));
    assert!(w.h.mail.sent().is_empty());
    assert_eq!(
        count(
            &w,
            "SELECT count(*) FROM calendar_invitations WHERE org_id = $1",
            w.org
        )
        .await,
        0
    );
}

/// A schedule's turn has no speaker: the invitation goes to the schedule's creator.
#[tokio::test]
async fn a_schedule_turn_invites_the_schedules_creator() {
    let w = world(EMAIL).await;
    let sched = Uuid::now_v7();
    sqlx::query("INSERT INTO schedules (id, agent_id, name, prompt, cron, timezone, creator_user_id) VALUES ($1,$2,'d','p','0 9 * * *','UTC',$3)")
        .bind(sched)
        .bind(w.agent)
        .bind(w.person)
        .execute(w.h.pool())
        .await
        .unwrap();
    let ctx = w
        .turn(
            w.agent,
            "schedule",
            None,
            json!({"kind": "schedule", "ref": sched.to_string()}),
        )
        .await;
    ok(&run(&ctx, "calendar_invite", event()).await);
    assert_eq!(w.h.mail.sent()[0].to, "grace@example.org");
    // A schedule without a creator has no person.
    let orphan = Uuid::now_v7();
    sqlx::query("INSERT INTO schedules (id, agent_id, name, prompt, cron, timezone) VALUES ($1,$2,'d','p','0 9 * * *','UTC')")
        .bind(orphan)
        .bind(w.agent)
        .execute(w.h.pool())
        .await
        .unwrap();
    let octx = w
        .turn(
            w.agent,
            "schedule",
            None,
            json!({"kind": "schedule", "ref": orphan.to_string()}),
        )
        .await;
    assert_eq!(
        code(&run(&octx, "calendar_invite", event()).await),
        "no_person"
    );
}

/// The tools are absent without a configured mailer and with the org switch off (and a stale
/// session calling them gets `tool_unavailable`).
#[tokio::test]
async fn absent_without_a_mailer_or_with_the_org_switch_off() {
    let names = |c: Vec<aurora_protocol::tools::ToolDescriptor>| -> Vec<String> {
        c.into_iter().map(|t| t.name).collect()
    };
    let has_all = |n: &[String]| NAMES.iter().all(|t| n.iter().any(|x| x == t));
    let has_none = |n: &[String]| NAMES.iter().all(|t| !n.iter().any(|x| x == t));

    // No SMTP configured (log-only): absent, and a call is refused.
    let w = world("").await;
    assert!(has_none(&names(
        catalogue(&w.h.state, w.agent).await.unwrap()
    )));
    let ctx = w.person_turn().await;
    assert_eq!(
        code(&run(&ctx, "calendar_invite", event()).await),
        "tool_unavailable"
    );
    assert!(w.h.mail.sent().is_empty());

    // Configured: present; the org switch off removes them; on again restores them.
    let w = world(EMAIL).await;
    assert!(has_all(&names(
        catalogue(&w.h.state, w.agent).await.unwrap()
    )));
    sqlx::query("UPDATE organizations SET settings = coalesce(settings,'{}'::jsonb) || '{\"calendar_invitations\": {\"enabled\": false}}'::jsonb WHERE id = $1")
        .bind(w.org)
        .execute(w.h.pool())
        .await
        .unwrap();
    assert!(has_none(&names(
        catalogue(&w.h.state, w.agent).await.unwrap()
    )));
    let ctx = w.person_turn().await;
    assert_eq!(
        code(&run(&ctx, "calendar_invite", event()).await),
        "tool_unavailable"
    );
    assert!(w.h.mail.sent().is_empty());
    sqlx::query("UPDATE organizations SET settings = settings || '{\"calendar_invitations\": {\"enabled\": true}}'::jsonb WHERE id = $1")
        .bind(w.org)
        .execute(w.h.pool())
        .await
        .unwrap();
    assert!(has_all(&names(
        catalogue(&w.h.state, w.agent).await.unwrap()
    )));
}

/// The org admin's switch is the org settings PATCH (`settings.calendar_invitations_enabled`).
#[tokio::test]
async fn the_org_settings_patch_turns_the_tools_off_and_on() {
    let w = world(EMAIL).await;
    let owner = w.h.user("owner@example.org", false).await;
    w.h.member(w.org, owner, "owner", "active").await;
    let (_, sess) = w.h.good_session(owner).await;
    let c = cookie(&sess);
    let uri = format!("/v1/orgs/{}", w.org);
    let (s, _, v) =
        w.h.send(http::Method::GET, &uri, &[("cookie", &c)], None)
            .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["settings"]["calendar_invitations_enabled"], true,
        "default on: {v}"
    );
    for (on, present) in [(false, false), (true, true)] {
        let (s, _, v) =
            w.h.send(
                http::Method::PATCH,
                &uri,
                &[("cookie", &c), ("origin", ORIGIN)],
                Some(json!({"settings": {"calendar_invitations_enabled": on}})),
            )
            .await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["settings"]["calendar_invitations_enabled"], on, "{v}");
        let cat = catalogue(&w.h.state, w.agent).await.unwrap();
        assert_eq!(cat.iter().any(|t| t.name == "calendar_invite"), present);
    }
}

/// The 21st send in an hour is `rate_limited` with the retry time; the 61st across agents in a
/// day is too. Updates and cancels count.
#[tokio::test]
async fn the_rate_limits() {
    let w = world(EMAIL).await;
    let ctx = w.person_turn().await;
    let first: Uuid = ok(&run(&ctx, "calendar_invite", event()).await)["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    for _ in 0..19 {
        ok(&run(
            &ctx,
            "calendar_invite_update",
            with(event(), "id", json!(first)),
        )
        .await);
    }
    assert_eq!(w.h.mail.sent().len(), 20);
    let r = run(&ctx, "calendar_invite", event()).await;
    assert_eq!(code(&r), "rate_limited");
    let msg = r.error.unwrap().message;
    let secs: i64 = msg
        .split("retry_after_secs=")
        .nth(1)
        .unwrap()
        .trim_end_matches(").")
        .parse()
        .unwrap();
    assert!((1..=3600).contains(&secs), "{msg}");
    assert_eq!(w.h.mail.sent().len(), 20, "nothing sent over the limit");
    // A cancel is a send too.
    assert_eq!(
        code(&run(&ctx, "calendar_invite_cancel", json!({"id": first})).await),
        "rate_limited"
    );
    // Another agent has its own hourly budget …
    let other = new_agent(&w.h, w.org, w.ws, "bob").await;
    let octx = w
        .turn(other, "api", Some(w.person), json!({"kind": "api"}))
        .await;
    ok(&run(&octx, "calendar_invite", event()).await);
    // … but the day's 60 are shared: age the first 20 out of the hour, then fill the day.
    sqlx::query("UPDATE calendar_invitation_sends SET sent_at = now() - interval '2 hours' WHERE person_id = $1 AND agent_id = $2")
        .bind(w.person)
        .bind(w.agent)
        .execute(w.h.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO calendar_invitation_sends (id, org_id, agent_id, person_id, sent_at) SELECT gen_random_uuid(), $1, $2, $3, now() - interval '3 hours' FROM generate_series(1, 39)")
        .bind(w.org)
        .bind(w.agent)
        .bind(w.person)
        .execute(w.h.pool())
        .await
        .unwrap();
    // 20 (aged) + 39 + 1 (bob) = 60 in the day: the next send by anyone is refused.
    assert_eq!(
        code(&run(&ctx, "calendar_invite", event()).await),
        "rate_limited"
    );
    assert_eq!(
        code(&run(&octx, "calendar_invite", event()).await),
        "rate_limited"
    );
}

struct FailMailer;

#[async_trait::async_trait]
impl Mailer for FailMailer {
    async fn send(&self, _: Mail) -> Result<(), MailError> {
        Err(MailError("smtp down".into()))
    }
}

/// A mailer error is `mail_unavailable` and nothing is recorded as sent (no row, no budget used);
/// for an update the row keeps its old state.
#[tokio::test]
async fn a_mailer_failure_records_nothing() {
    let mut w = world(EMAIL).await;
    let ctx = w.person_turn().await;
    let id: Uuid = ok(&run(&ctx, "calendar_invite", event()).await)["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    w.h.state = w.h.state.clone().with_mailer(Arc::new(FailMailer));
    let ctx = w.person_turn().await;
    assert_eq!(
        code(&run(&ctx, "calendar_invite", event()).await),
        "mail_unavailable"
    );
    assert_eq!(
        count(
            &w,
            "SELECT count(*) FROM calendar_invitations WHERE org_id = $1",
            w.org
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &w,
            "SELECT count(*) FROM calendar_invitation_sends WHERE org_id = $1",
            w.org
        )
        .await,
        1
    );

    let upd = with(with(event(), "id", json!(id)), "title", json!("Moved"));
    assert_eq!(
        code(&run(&ctx, "calendar_invite_update", upd).await),
        "mail_unavailable"
    );
    assert_eq!(
        code(&run(&ctx, "calendar_invite_cancel", json!({"id": id})).await),
        "mail_unavailable"
    );
    let (title, seq, status): (String, i32, String) =
        sqlx::query_as("SELECT title, sequence, status FROM calendar_invitations WHERE id = $1")
            .bind(id)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(
        (title.as_str(), seq, status.as_str()),
        ("Design review", 0, "sent")
    );
    assert_eq!(
        count(
            &w,
            "SELECT count(*) FROM calendar_invitation_sends WHERE org_id = $1",
            w.org
        )
        .await,
        1
    );
}

/// SEC-108: CR/LF in the title or description neither creates a header nor adds a property.
#[tokio::test]
async fn injection_in_the_title_and_description() {
    let w = world(EMAIL).await;
    let ctx = w.person_turn().await;
    let evil = "Hi\r\nBcc: evil@example.org\r\nATTENDEE:mailto:evil@example.org\r\nEND:VEVENT";
    let args = with(
        with(event(), "title", json!(evil)),
        "description",
        json!(evil),
    );
    ok(&run(&ctx, "calendar_invite", args).await);
    let m = &w.h.mail.sent()[0];
    assert!(!m.subject.contains(['\r', '\n']), "{:?}", m.subject);
    assert_eq!(m.to, "grace@example.org");
    let ics = ics_of(m);
    assert_eq!(
        ics.lines().filter(|l| l.starts_with("ATTENDEE")).count(),
        1,
        "{ics}"
    );
    assert_eq!(
        ics.lines().filter(|l| *l == "END:VEVENT").count(),
        1,
        "{ics}"
    );
    // The message as lettre builds it carries no extra header.
    let from: lettre::message::Mailbox = "Aurora <aurora@example.org>".parse().unwrap();
    let raw = String::from_utf8(
        aurora_api::domains::auth::mail::build_message(&from, m)
            .unwrap()
            .formatted(),
    )
    .unwrap();
    let head = raw.split("\r\n\r\n").next().unwrap().to_ascii_lowercase();
    assert!(!head.lines().any(|l| l.starts_with("bcc:")), "{head}");
}

/// An eval turn cannot send an invitation (C-868): the three write tools are refused by the
/// conversation's kind; `calendar_invite_list` is a read, but an eval turn has no person.
#[tokio::test]
async fn eval_turns_cannot_send_invitations() {
    let w = world(EMAIL).await;
    let ctx = w
        .turn(w.agent, "eval", Some(w.person), json!({"kind": "eval"}))
        .await;
    for (tool, args) in [
        ("calendar_invite", event()),
        (
            "calendar_invite_update",
            with(event(), "id", json!(Uuid::now_v7())),
        ),
        ("calendar_invite_cancel", json!({"id": Uuid::now_v7()})),
    ] {
        assert_eq!(
            code(&run(&ctx, tool, args).await),
            "eval_read_only",
            "{tool}"
        );
    }
    assert!(w.h.mail.sent().is_empty());
    // (P5-PERSON, C-1052) An eval turn has no person, whoever started the run: the read answers
    // `no_person` instead of listing the starter's invitations.
    assert_eq!(
        code(&run(&ctx, "calendar_invite_list", json!({})).await),
        "no_person"
    );
}

/// A repeating event carries a VTIMEZONE (TZID) in the mailed calendar.
#[tokio::test]
async fn a_repeating_invitation_carries_its_time_zone() {
    let w = world(EMAIL).await;
    let ctx = w.person_turn().await;
    let args = with(
        event(),
        "repeat",
        json!({"freq": "weekly", "by_day": ["TU"], "count": 30}),
    );
    ok(&run(&ctx, "calendar_invite", args).await);
    let ics = ics_of(&w.h.mail.sent()[0]);
    assert!(
        ics.contains("BEGIN:VTIMEZONE") && ics.contains("TZID:Europe/Zurich"),
        "{ics}"
    );
    assert_eq!(
        prop(&ics, "DTSTART"),
        format!(
            "DTSTART;TZID=Europe/Zurich:{}T090000",
            day(0).format("%Y%m%d")
        )
    );
    assert_eq!(prop(&ics, "RRULE"), "RRULE:FREQ=WEEKLY;BYDAY=TU;COUNT=30");
    let (rep,): (Value,) =
        sqlx::query_as("SELECT repeat FROM calendar_invitations WHERE org_id = $1")
            .bind(w.org)
            .fetch_one(w.h.pool())
            .await
            .unwrap();
    assert_eq!(rep["freq"], "weekly");
}

/// Deletion and retention: an agent, person or org deletion removes the rows (and the ledger);
/// the daily pass prunes ended invitations and old ledger rows, and nothing live.
#[tokio::test]
async fn deletion_and_retention() {
    let w = world(EMAIL).await;
    let ctx = w.person_turn().await;
    ok(&run(&ctx, "calendar_invite", event()).await);
    let n = |sql: &'static str, id: Uuid| {
        let pool = w.h.pool().clone();
        async move {
            sqlx::query_scalar::<_, i64>(sql)
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    let rows = "SELECT (SELECT count(*) FROM calendar_invitations WHERE org_id = $1) + (SELECT count(*) FROM calendar_invitation_sends WHERE org_id = $1)";
    assert_eq!(n(rows, w.org).await, 2);

    // Retention: nothing is old yet.
    let report = aurora_api::jobs::run_retention_prune(w.h.pool())
        .await
        .unwrap();
    assert_eq!(report.calendar_invitations, 0);
    // An event that ended 100 days ago and a ledger row from 3 days ago go; a repeating series
    // and a future event stay.
    sqlx::query("UPDATE calendar_invitations SET starts_at = now() - interval '101 days', ends_at = now() - interval '100 days', updated_at = now() - interval '100 days'")
        .execute(w.h.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE calendar_invitation_sends SET sent_at = now() - interval '3 days'")
        .execute(w.h.pool())
        .await
        .unwrap();
    let repeating = Uuid::now_v7();
    sqlx::query("INSERT INTO calendar_invitations (id, org_id, workspace_id, agent_id, person_id, uid, status, title, starts_at, ends_at, timezone, repeat, updated_at) VALUES ($1,$2,$3,$4,$5,'r@x','sent','r', now() - interval '200 days', now() - interval '200 days', 'UTC', '{\"freq\":\"weekly\"}', now() - interval '200 days')")
        .bind(repeating)
        .bind(w.org)
        .bind(w.ws)
        .bind(w.agent)
        .bind(w.person)
        .execute(w.h.pool())
        .await
        .unwrap();
    let report = aurora_api::jobs::run_retention_prune(w.h.pool())
        .await
        .unwrap();
    assert_eq!(report.calendar_invitations, 2, "{report:?}");
    assert_eq!(
        n(
            "SELECT count(*) FROM calendar_invitations WHERE id = $1",
            repeating
        )
        .await,
        1
    );

    // Deleting the person removes their rows; so does the agent, and so does the org.
    for what in ["person", "agent", "org"] {
        let w2 = world(EMAIL).await;
        let c2 = w2.person_turn().await;
        ok(&run(&c2, "calendar_invite", event()).await);
        assert_eq!(count(&w2, rows, w2.org).await, 2);
        match what {
            "person" => {
                sqlx::query("DELETE FROM users WHERE id = $1")
                    .bind(w2.person)
                    .execute(w2.h.pool())
                    .await
                    .unwrap();
            }
            "agent" => {
                sqlx::query("DELETE FROM agents WHERE id = $1")
                    .bind(w2.agent)
                    .execute(w2.h.pool())
                    .await
                    .unwrap();
            }
            _ => {
                // The real organization delete (owner, typed confirmation).
                let owner = w2.h.user("owner@example.org", false).await;
                w2.h.member(w2.org, owner, "owner", "active").await;
                let (_, sess) = w2.h.good_session(owner).await;
                let c = cookie(&sess);
                let (s, _, v) =
                    w2.h.send(
                        http::Method::DELETE,
                        &format!("/v1/orgs/{}", w2.org),
                        &[("cookie", &c), ("origin", ORIGIN)],
                        Some(json!({"confirm": w2.org.simple().to_string()})),
                    )
                    .await;
                assert_eq!(s, 204, "{v}");
            }
        }
        assert_eq!(
            count(&w2, rows, w2.org).await,
            0,
            "after deleting the {what}"
        );
    }
}
