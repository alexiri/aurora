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

/// Dev mode: write the link to the API log at INFO.
pub struct LogMailer;

#[async_trait::async_trait]
impl Mailer for LogMailer {
    async fn send(&self, mail: Mail) -> Result<(), MailError> {
        tracing::info!(to = %mail.to, subject = %mail.subject, link = %mail.link, "email (log-only mode, SMTP not configured)");
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
    pub fn new(url: &str, from: &str) -> Result<Self, MailError> {
        let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(url)
            .map_err(|e| MailError(e.to_string()))?
            .build();
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
        match SmtpMailer::new(url.expose(), from) {
            Ok(m) => return Arc::new(m),
            Err(e) => {
                tracing::error!(error = %e, "invalid SMTP configuration; using log-only mailer")
            }
        }
    }
    Arc::new(LogMailer)
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
