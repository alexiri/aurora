//! Mailer abstraction: `SmtpMailer` (lettre), `LogMailer` (INFO log with link), `MemoryMailer` (tests).
//!
//! For other domains (invitations, BE-013): build a [`Mail`] with [`Mail::new`] and call
//! [`send_detached`] (fire and forget) or `state.mailer.send(mail).await`.

use aurora_config::ApiConfig;
use lettre::message::header::{ContentDisposition, ContentType};
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::AsyncSmtpTransport;
use lettre::{AsyncTransport, Message, Tokio1Executor};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    /// Plain-text body (contains `link`).
    pub body: String,
    /// The action link, kept separately for the log-only/in-memory mailers.
    pub link: String,
    /// (P5-CALINV) Display name of the sender (`From: "<name>" <email.from>`); the address is
    /// always the configured one.
    pub from_name: Option<String>,
    /// (P5-CALINV) An iTIP calendar part: the message becomes multipart/mixed with the plain body,
    /// an inline `text/calendar; method=…` part and the same calendar as `invite.ics`.
    pub calendar: Option<CalendarPart>,
}

/// The calendar of a calendar-invitation mail (iMIP, RFC 6047).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarPart {
    /// `REQUEST` or `CANCEL`.
    pub method: &'static str,
    /// The complete iCalendar object (CRLF line ends).
    pub ics: String,
}

impl Mail {
    /// `intro` is the sentence(s) before the link in the body.
    pub fn new(to: &str, subject: &str, intro: &str, link: &str) -> Self {
        Self {
            to: to.to_owned(),
            subject: subject.to_owned(),
            body: format!("{intro}\n\n{link}\n"),
            link: link.to_owned(),
            from_name: None,
            calendar: None,
        }
    }
}

/// The SMTP message of a [`Mail`] from `from`: plain text, or multipart/mixed (plain, inline
/// calendar, `invite.ics`) with a calendar part. Header values lettre receives never carry CR/LF/NUL
/// (they are removed here as well as by the callers).
pub fn build_message(from: &Mailbox, mail: &Mail) -> Result<Message, MailError> {
    let err = |e: &dyn std::fmt::Display| MailError(e.to_string());
    let from = match &mail.from_name {
        Some(n) => Mailbox::new(Some(header_safe(n)), from.email.clone()),
        None => from.clone(),
    };
    let builder = Message::builder()
        .from(from)
        .to(mail.to.parse().map_err(|e| err(&e))?)
        .subject(header_safe(&mail.subject));
    match &mail.calendar {
        None => builder.body(mail.body.clone()).map_err(|e| err(&e)),
        Some(c) => {
            let ct = ContentType::parse(&format!(
                "text/calendar; method={}; charset=utf-8",
                c.method
            ))
            .map_err(|e| err(&e))?;
            let inline = SinglePart::builder()
                .header(ct.clone())
                .header(ContentDisposition::inline())
                .body(c.ics.clone());
            let attachment = Attachment::new("invite.ics".to_owned()).body(c.ics.clone(), ct);
            builder
                .multipart(
                    MultiPart::mixed()
                        .singlepart(SinglePart::plain(mail.body.clone()))
                        .singlepart(inline)
                        .singlepart(attachment),
                )
                .map_err(|e| err(&e))
        }
    }
}

/// One line with no control characters (header injection: CR, LF, NUL and the rest are dropped).
fn header_safe(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

#[derive(Debug, thiserror::Error)]
#[error("mail delivery failed: {0}")]
pub struct MailError(pub String);

#[async_trait::async_trait]
pub trait Mailer: Send + Sync {
    async fn send(&self, mail: Mail) -> Result<(), MailError>;
}

/// Log-only mode (SMTP not configured). The recipient and subject are logged; the action link
/// (a sign-in or invitation token, SEC-10) only when `email.log_links` is set (development).
pub struct LogMailer {
    pub log_links: bool,
}

#[async_trait::async_trait]
impl Mailer for LogMailer {
    async fn send(&self, mail: Mail) -> Result<(), MailError> {
        if self.log_links {
            tracing::info!(to = %mail.to, subject = %mail.subject, link = %mail.link, "email (log-only mode, SMTP not configured)");
        } else {
            tracing::info!(to = %mail.to, subject = %mail.subject, "email (log-only mode, SMTP not configured; set email.log_links to log the link in development)");
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct MemoryMailer {
    sent: Mutex<Vec<Mail>>,
}

impl MemoryMailer {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }
    pub fn sent(&self) -> Vec<Mail> {
        self.sent.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    pub fn clear(&self) {
        self.sent.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
}

#[async_trait::async_trait]
impl Mailer for MemoryMailer {
    async fn send(&self, mail: Mail) -> Result<(), MailError> {
        self.sent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(mail);
        Ok(())
    }
}

pub struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl SmtpMailer {
    /// `credentials` is `(username, password)` from `email.smtp_username` and
    /// `AURORA_SMTP_PASSWORD`; the URL never carries them.
    pub fn new(
        url: &str,
        from: &str,
        credentials: Option<(String, String)>,
    ) -> Result<Self, MailError> {
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::from_url(url)
            .map_err(|e| MailError(e.to_string()))?;
        if let Some((user, pass)) = credentials {
            builder = builder.credentials(
                lettre::transport::smtp::authentication::Credentials::new(user, pass),
            );
        }
        let transport = builder.build();
        let from = from.parse().map_err(|e| MailError(format!("{e}")))?;
        Ok(Self { transport, from })
    }
}

#[async_trait::async_trait]
impl Mailer for SmtpMailer {
    async fn send(&self, mail: Mail) -> Result<(), MailError> {
        let msg = build_message(&self.from, &mail)?;
        self.transport
            .send(msg)
            .await
            .map(|_| ())
            .map_err(|e| MailError(e.to_string()))
    }
}

/// SMTP when `email.smtp_url` is set (falling back to log-only with an error log if it is
/// unusable), else log-only.
pub fn from_config(cfg: &ApiConfig) -> Arc<dyn Mailer> {
    from_parts(
        &cfg.email,
        cfg.email.smtp_password.as_ref().map(|p| p.expose().clone()),
    )
}

/// The mailer for e-mail settings and an already resolved SMTP password (SET-4: the password is
/// the environment's or one the dashboard sealed; the URL never carries it).
pub fn from_parts(
    email: &aurora_config::EmailSection,
    password: Option<String>,
) -> Arc<dyn Mailer> {
    if let Some(url) = &email.smtp_url {
        let from = email.from.as_deref().unwrap_or_default();
        let credentials = email.smtp_username.clone().zip(password);
        match SmtpMailer::new(url.expose(), from, credentials) {
            Ok(m) => return Arc::new(m),
            Err(e) => {
                tracing::error!(error = %e, "invalid SMTP configuration; using log-only mailer")
            }
        }
    }
    Arc::new(LogMailer {
        log_links: email.log_links,
    })
}

/// Send in the background; failures are logged (never exposing existence of accounts).
pub fn send_detached(mailer: &Arc<dyn Mailer>, mail: Mail) {
    let mailer = Arc::clone(mailer);
    tokio::spawn(async move {
        if let Err(e) = mailer.send(mail).await {
            tracing::error!(error = %e, "email send failed");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buf {
        type Writer = Buf;
        fn make_writer(&'a self) -> Buf {
            self.clone()
        }
    }

    async fn logged(log_links: bool) -> String {
        let buf = Buf::default();
        let sub = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_ansi(false)
            .finish();
        let guard = tracing::subscriber::set_default(sub);
        let mail = Mail::new(
            "invitee@example.org",
            "You are invited",
            "Join:",
            "https://aurora.example.org/join?token=SECRET-TOKEN-123",
        );
        LogMailer { log_links }.send(mail).await.unwrap();
        drop(guard);
        let out = buf.0.lock().unwrap().clone();
        String::from_utf8(out).unwrap()
    }

    fn invite_mail(subject: &str, from_name: &str) -> Mail {
        let mut m = Mail::new("ada@example.org", subject, "Intro", "https://x.example/a");
        m.from_name = Some(from_name.to_owned());
        m.calendar = Some(CalendarPart {
            method: "REQUEST",
            ics: "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n".into(),
        });
        m
    }

    /// (P5-CALINV) A calendar mail is multipart/mixed: plain text, an inline `text/calendar`
    /// part with the method, and the same calendar as `invite.ics`; the sender address is the
    /// configured one whatever name is given.
    #[test]
    fn a_calendar_mail_is_plain_inline_calendar_and_attachment() {
        let from: Mailbox = "Aurora <aurora@example.org>".parse().unwrap();
        let msg = build_message(&from, &invite_mail("Invitation: Review", "Ada (Aurora)")).unwrap();
        let raw = String::from_utf8(msg.formatted()).unwrap();
        assert!(raw.contains("Content-Type: multipart/mixed"), "{raw}");
        assert!(raw.contains("Content-Type: text/plain"), "{raw}");
        assert!(
            raw.contains("Content-Type: text/calendar; method=REQUEST; charset=utf-8"),
            "{raw}"
        );
        assert!(raw.contains("Content-Disposition: inline"), "{raw}");
        assert!(raw.contains("filename=\"invite.ics\""), "{raw}");
        assert!(raw.contains("<aurora@example.org>"), "{raw}");
        // The display name is an RFC 2047 encoded word (lettre encodes the parentheses).
        assert!(
            raw.contains("From: =?utf-8?b?QWRhIChBdXJvcmEp?= <aurora@example.org>"),
            "{raw}"
        );
        assert_eq!(
            raw.matches("BEGIN:VCALENDAR").count(),
            2,
            "inline and attachment: {raw}"
        );
    }

    /// (SEC-108) CR/LF/NUL in an agent-supplied subject or name never starts a header.
    #[test]
    fn crlf_in_the_subject_or_the_name_never_creates_a_header() {
        let from: Mailbox = "Aurora <aurora@example.org>".parse().unwrap();
        let m = invite_mail(
            "Invitation: x\r\nBcc: evil@example.org\r\nX-Injected: 1\0",
            "Eve\r\nBcc: evil@example.org",
        );
        let raw = String::from_utf8(build_message(&from, &m).unwrap().formatted()).unwrap();
        let head = raw.split("\r\n\r\n").next().unwrap();
        for line in head.lines() {
            assert!(
                !line.to_ascii_lowercase().starts_with("bcc:")
                    && !line.to_ascii_lowercase().starts_with("x-injected"),
                "injected header line {line:?} in {head}"
            );
        }
        assert!(!head.contains('\0'));
    }

    /// FIX-116: with SMTP unset the log carries recipient and subject, never the token link
    /// (SEC-10) — unless development sets `email.log_links`.
    #[tokio::test]
    async fn log_mailer_hides_the_link_unless_asked() {
        let quiet = logged(false).await;
        assert!(quiet.contains("invitee@example.org"), "{quiet}");
        assert!(!quiet.contains("SECRET-TOKEN-123"), "{quiet}");
        let dev = logged(true).await;
        assert!(dev.contains("SECRET-TOKEN-123"), "{dev}");
    }
}
