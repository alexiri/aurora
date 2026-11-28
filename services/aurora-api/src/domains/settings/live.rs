//! The sign-in and e-mail settings as live state (SET-4, D-236(f)): what the API reads on every
//! request is replaced atomically when the `sign_in` or `email` section changes, in every replica.
//!
//! * [`LiveAuth`] — sign-up mode, organization creation, session lifetimes, CLI key lifetime and
//!   the OIDC providers (with the client secrets that come from the environment; a secret the
//!   dashboard sealed is opened when the provider is used, so a swap never awaits the store).
//! * [`LiveMailer`] — the mailer; a swap leaves mail in flight to finish with the old transport.

use crate::domains::auth::mail::{self, Mail, MailError, Mailer};
use arc_swap::ArcSwap;
use aurora_config::{
    ApiConfig, AuthSection, EmailSection, OrgCreation, Redacted, SealedSecret, SignupMode,
};
use aurora_core::Secret;
use aurora_secrets::{Purpose, SealContext, SecretRef, SecretVault};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The sign-in settings in force.
#[derive(Debug, Clone)]
pub struct LiveAuth {
    pub auth: AuthSection,
    pub signup_mode: SignupMode,
    pub org_creation: OrgCreation,
    /// Client secrets read from the environment (Helm-defined or mirrored providers).
    pub oidc_client_secrets: BTreeMap<String, Redacted<String>>,
    /// SMTP is configured (`email.smtp_url`): invitations and approval e-mails are delivered.
    pub smtp: bool,
}

impl LiveAuth {
    pub fn from_config(c: &ApiConfig) -> Self {
        Self {
            auth: c.auth.clone(),
            signup_mode: c.signup_mode,
            org_creation: c.org_creation,
            oidc_client_secrets: c.oidc_client_secrets.clone(),
            smtp: c.email.smtp_url.is_some(),
        }
    }
}

/// A mailer whose transport is replaced when the `email` settings change.
pub struct LiveMailer {
    cell: ArcSwap<Arc<dyn Mailer>>,
    /// The transports still being built after a change of the settings.
    pub swaps: crate::in_flight::InFlight,
}

impl LiveMailer {
    pub fn new(m: Arc<dyn Mailer>) -> Self {
        Self {
            cell: ArcSwap::from_pointee(m),
            swaps: crate::in_flight::InFlight::default(),
        }
    }
    pub fn set(&self, m: Arc<dyn Mailer>) {
        self.cell.store(Arc::new(m));
    }
}

#[async_trait::async_trait]
impl Mailer for LiveMailer {
    async fn send(&self, mail: Mail) -> Result<(), MailError> {
        // One load per send: a swap mid-send never mixes two transports.
        let m: Arc<dyn Mailer> = Arc::clone(&self.cell.load());
        m.send(mail).await
    }
}

/// The context a dashboard-sealed value is opened under (D-236(g)).
fn seal_context(owner: uuid::Uuid) -> SealContext {
    SealContext {
        purpose: Purpose::InstanceSetting,
        owner_id: owner,
        org_id: uuid::Uuid::nil(),
        workspace_id: None,
    }
}

/// Open a value the dashboard sealed in the inline store.
pub async fn open_sealed(vault: &SecretVault, s: &SealedSecret) -> Result<String, String> {
    let r: SecretRef = s
        .inner
        .sealed_ref
        .parse()
        .map_err(|_| "sealed reference is malformed".to_string())?;
    let v: Secret<Vec<u8>> = vault
        .open(&seal_context(s.inner.owner), &r)
        .await
        .map_err(|e| format!("sealed value not opened: {e}"))?;
    String::from_utf8(v.expose().clone()).map_err(|_| "sealed value is not text".to_string())
}

/// The SMTP password in force: the environment's, else the one the dashboard sealed.
pub async fn smtp_password(
    vault: &SecretVault,
    email: &EmailSection,
) -> Result<Option<String>, String> {
    if let Some(p) = &email.smtp_password {
        return Ok(Some(p.expose().clone()));
    }
    match &email.smtp_password_sealed {
        Some(s) => open_sealed(vault, s).await.map(Some),
        None => Ok(None),
    }
}

/// A mailer from the e-mail settings; log-only without an SMTP URL (and, with an error log, when
/// the configuration cannot be used or the sealed password cannot be opened).
pub async fn build_mailer(vault: &SecretVault, email: &EmailSection) -> Arc<dyn Mailer> {
    let password = match smtp_password(vault, email).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "SMTP password not available; using the log-only mailer");
            return Arc::new(mail::LogMailer {
                log_links: email.log_links,
            });
        }
    };
    mail::from_parts(email, password)
}
