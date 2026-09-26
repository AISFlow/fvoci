//! Mail transport and the outbox `mail` External consumer.
//!
//! Source SMTP env is the trio SMTP_HOST/SMTP_PORT/SMTP_FROM (all or none).
//! There is no TLS or AUTH env; the client speaks plaintext SMTP. Invitation
//! mail is sent inline after the invite row commits. Notification mail goes
//! through the outbox: mark processed_events only after SMTP accepts. If SMTP
//! accepted and the process then crashed before the cursor advanced,
//! `--recover-outbox` will send again (at-least-once). processed_events stops
//! a second send after a successful mark.

pub mod consumer;
pub mod digest;
pub mod templates;

mod smtp;
pub use smtp::probe_smtp;

use std::fmt;
use std::sync::Arc;

pub use consumer::{mail_consumer, MAIL_CONSUMER};
pub use digest::send_due_digests;

pub const MAGIC_TTL_SECS: i64 = 15 * 60;
pub const MAGIC_RESPONSE_DELAY_MS: u64 = 100;
pub const MAGIC_PER_IP: u32 = 30;
pub const MAGIC_PER_EMAIL: u32 = 10;

#[derive(Clone, Debug)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub from: String,
}

/// Parse source SMTP env. Empty values (compose `${VAR:-}`) count as unset.
/// Partial config fails closed.
pub fn smtp_from_env() -> Result<Option<SmtpConfig>, String> {
    smtp_from_values(
        std::env::var("SMTP_HOST").ok().as_deref(),
        std::env::var("SMTP_PORT").ok().as_deref(),
        std::env::var("SMTP_FROM").ok().as_deref(),
    )
}

pub fn smtp_from_values(
    host: Option<&str>,
    port: Option<&str>,
    from: Option<&str>,
) -> Result<Option<SmtpConfig>, String> {
    let host = blank_to_none(host);
    let port = blank_to_none(port);
    let from = blank_to_none(from);
    match (host, port, from) {
        (None, None, None) => Ok(None),
        (Some(host), Some(port), Some(from)) => {
            let port: u16 = port
                .parse()
                .map_err(|e| format!("invalid SMTP_PORT: {e}"))?;
            if port == 0 {
                return Err("SMTP_PORT must be a positive integer".into());
            }
            if from.is_empty() {
                return Err("SMTP_FROM must be nonempty".into());
            }
            Ok(Some(SmtpConfig {
                host: host.to_string(),
                port,
                from: from.to_string(),
            }))
        }
        (host, port, from) => {
            let mut missing = Vec::new();
            if host.is_none() {
                missing.push("SMTP_HOST");
            }
            if port.is_none() {
                missing.push("SMTP_PORT");
            }
            if from.is_none() {
                missing.push("SMTP_FROM");
            }
            Err(format!(
                "SMTP_HOST, SMTP_PORT, SMTP_FROM must all be set together, or none (missing {})",
                missing.join(", ")
            ))
        }
    }
}

fn blank_to_none(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[derive(Debug)]
pub struct MailSendError {
    pub op: &'static str,
    pub code: String,
}

impl fmt::Display for MailSendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mailer: {} failed (code={})", self.op, self.code)
    }
}

impl std::error::Error for MailSendError {}

/// RFC 5322 quoted-string From display. Control characters are the branding
/// schema's job; this is defense in depth.
pub fn mail_from(address: &str, display: Option<&str>) -> String {
    match display {
        None => address.to_string(),
        Some(display) => {
            let quoted = display.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{quoted}\" <{address}>")
        }
    }
}

#[derive(Clone)]
pub struct Mailer {
    smtp: Option<SmtpConfig>,
}

impl fmt::Debug for Mailer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mailer")
            .field("smtp", &self.smtp.as_ref().map(|_| "<configured>"))
            .finish()
    }
}

impl Mailer {
    pub fn disabled() -> Self {
        Self { smtp: None }
    }

    pub fn from_smtp(smtp: Option<SmtpConfig>) -> Self {
        Self { smtp }
    }

    pub fn enabled(&self) -> bool {
        self.smtp.is_some()
    }

    pub async fn send_invite(&self, to: &str, url: &str) -> Result<(), MailSendError> {
        self.send_op("sendInvite", to, templates::INVITE_SUBJECT, url)
            .await
    }

    pub async fn send(&self, to: &str, subject: &str, text: &str) -> Result<(), MailSendError> {
        self.send_op("send", to, subject, text).await
    }

    async fn send_op(
        &self,
        op: &'static str,
        to: &str,
        subject: &str,
        text: &str,
    ) -> Result<(), MailSendError> {
        let Some(smtp) = self.smtp.as_ref() else {
            return Ok(());
        };
        let from_header = mail_from(&smtp.from, None);
        smtp::send_mail_op(smtp, op, &from_header, &smtp.from, to, subject, text).await
    }

    /// Fire-and-forget like source sendDetached: SMTP failure is logged without
    /// a recipient address and does not change the HTTP response.
    pub fn send_detached(self: &Arc<Self>, to: String, subject: String, text: String) {
        let mailer = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(err) = mailer.send(&to, &subject, &text).await {
                tracing::warn!(message = %err, "mail.send_failed");
            }
        });
    }
}

pub async fn equalize_magic_response_timing(started: std::time::Instant) {
    let elapsed = started.elapsed();
    let target = std::time::Duration::from_millis(MAGIC_RESPONSE_DELAY_MS);
    if elapsed < target {
        tokio::time::sleep(target - elapsed).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smtp_all_or_none() {
        assert!(smtp_from_values(None, None, None).unwrap().is_none());
        let err = smtp_from_values(Some("h"), None, None).unwrap_err();
        assert!(err.contains("SMTP_PORT"));
        assert!(err.contains("SMTP_FROM"));
        assert!(
            !err.contains("SMTP_HOST, SMTP_PORT, SMTP_FROM must all") || err.contains("missing")
        );
        let err = smtp_from_values(None, Some("587"), None).unwrap_err();
        assert!(err.contains("SMTP_HOST"));
        assert!(err.contains("SMTP_FROM"));
        let parsed = smtp_from_values(
            Some("smtp.example.com"),
            Some("587"),
            Some("noreply@example.com"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(parsed.host, "smtp.example.com");
        assert_eq!(parsed.port, 587);
        assert_eq!(parsed.from, "noreply@example.com");
        assert!(smtp_from_values(Some("  "), None, None).unwrap().is_none());
    }

    #[test]
    fn mail_from_quotes_display() {
        assert_eq!(
            mail_from("no-reply@example.com", None),
            "no-reply@example.com"
        );
        assert_eq!(
            mail_from("no-reply@example.com", Some("연구소")),
            "\"연구소\" <no-reply@example.com>"
        );
        assert_eq!(
            mail_from("a@b.com", Some("x\" <evil@z.com>, \"y")),
            "\"x\\\" <evil@z.com>, \\\"y\" <a@b.com>"
        );
    }

    #[test]
    fn send_error_omits_recipient() {
        let err = MailSendError {
            op: "sendInvite",
            code: "421".into(),
        };
        let message = err.to_string();
        assert!(message.contains("code=421"));
        assert!(!message.contains('@'));
    }

    #[tokio::test]
    async fn disabled_mailer_is_noop() {
        let mailer = Mailer::disabled();
        mailer.send("a@b.com", "제목", "본문").await.expect("noop");
    }
}
