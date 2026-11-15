//! Mailer abstraction: `SmtpMailer` (lettre), `LogMailer` (INFO log with link), `MemoryMailer` (tests).
//!
//! For other domains (invitations, BE-013): build a [`Mail`] with [`Mail::new`] and call
//! [`send_detached`] (fire and forget) or `state.mailer.send(mail).await`.

use aurora_config::ApiConfig;
use lettre::message::Mailbox;
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
}

impl Mail {
    /// `intro` is the sentence(s) before the link in the body.
    pub fn new(to: &str, subject: &str, intro: &str, link: &str) -> Self {
        Self {
            to: to.to_owned(),
            subject: subject.to_owned(),
            body: format!("{intro}\n\n{link}\n"),
            link: link.to_owned(),
        }
    }
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
        let msg = Message::builder()
            .from(self.from.clone())
            .to(mail.to.parse().map_err(|e| MailError(format!("{e}")))?)
            .subject(mail.subject)
            .body(mail.body)
            .map_err(|e| MailError(e.to_string()))?;
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
    if let Some(url) = &cfg.email.smtp_url {
        let from = cfg.email.from.as_deref().unwrap_or_default();
        let credentials = cfg
            .email
            .smtp_username
            .clone()
            .zip(cfg.email.smtp_password.as_ref().map(|p| p.expose().clone()));
        match SmtpMailer::new(url.expose(), from, credentials) {
            Ok(m) => return Arc::new(m),
            Err(e) => {
                tracing::error!(error = %e, "invalid SMTP configuration; using log-only mailer")
            }
        }
    }
    Arc::new(LogMailer {
        log_links: cfg.email.log_links,
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
