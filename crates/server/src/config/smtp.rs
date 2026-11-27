//! Optional SMTP delivery settings; passwords are never exposed in diagnostics.

use anyhow::bail;
use serde::Deserialize;
use serde_json::{Map, Value};

pub(super) const ENV_KEYS: [(&str, super::EnvKind); 14] = [
    ("smtp_host", super::EnvKind::Text),
    ("smtp_port", super::EnvKind::Number),
    ("smtp_tls", super::EnvKind::Text),
    ("smtp_user", super::EnvKind::Text),
    ("smtp_password", super::EnvKind::Text),
    ("mail_from", super::EnvKind::Text),
    ("mail_per_second", super::EnvKind::Number),
    ("mail_reset_queue", super::EnvKind::Number),
    ("mail_reset_per_account_hour", super::EnvKind::Number),
    ("mail_invitation_queue", super::EnvKind::Number),
    ("invitation_mail_recipients", super::EnvKind::Number),
    ("invitation_mail_per_hour", super::EnvKind::Number),
    ("invitation_mail_per_org_hour", super::EnvKind::Number),
    ("invitation_mail_per_address_hour", super::EnvKind::Number),
];

/// How the connection to the SMTP server is protected; a value that is none of these
/// is refused when the configuration is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(rename_all = "lowercase")]
pub enum SmtpTls {
    /// A plain connection that must be upgraded with STARTTLS.
    #[default]
    Starttls,
    /// TLS from the first byte.
    Tls,
    /// No encryption at all, chosen explicitly.
    #[serde(rename = "none")]
    Unencrypted,
}

/// The SMTP settings, flattened into the main configuration file.
#[derive(Clone, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(default)]
pub struct SmtpSettings {
    /// SMTP host name or IP address, without a scheme or port; empty disables mail.
    pub smtp_host: String,
    /// TCP port of the SMTP server.
    pub smtp_port: u16,
    /// Required TLS mode: `starttls`, `tls`, or explicitly unencrypted `none`.
    pub smtp_tls: SmtpTls,
    /// SMTP authentication user; empty for no authentication.
    pub smtp_user: String,
    /// SMTP authentication password; omitted from debug output.
    pub smtp_password: String,
    /// The sender's address of all mail, whatever its way, without a display name.
    pub mail_from: String,
    /// How many attempts to send all mail makes a second, recovery first; at least one.
    pub mail_per_second: u32,
    /// The most recovery messages waiting or being delivered; more are dropped unseen.
    pub mail_reset_queue: u32,
    /// The most recovery messages one account is sent in an hour; more requests are
    /// answered as any other and send nothing.
    pub mail_reset_per_account_hour: u32,
    /// The most invitation messages waiting or being delivered; more are refused (503).
    /// Among several organizations, one holds at most half of what the others leave.
    pub mail_invitation_queue: u32,
    /// The most recipients one invitation mail request names (400 beyond).
    pub invitation_mail_recipients: u32,
    /// The most invitation messages one workspace queues in an hour (429 beyond).
    pub invitation_mail_per_hour: u32,
    /// The most invitation messages one organization queues in an hour (429 beyond),
    /// until organizations have limits of their own (`App::limits`); at least
    /// `invitation_mail_per_hour`.
    pub invitation_mail_per_org_hour: u32,
    /// The most invitation messages sent to a single recipient address in an hour across
    /// all organizations; excess addresses are silently skipped without creating guest links.
    pub invitation_mail_per_address_hour: u32,
}

impl Default for SmtpSettings {
    fn default() -> Self {
        Self {
            smtp_host: String::new(),
            smtp_port: 587,
            smtp_tls: SmtpTls::Starttls,
            smtp_user: String::new(),
            smtp_password: String::new(),
            mail_from: String::new(),
            mail_per_second: 1,
            mail_reset_queue: 200,
            mail_reset_per_account_hour: 3,
            mail_invitation_queue: 200,
            invitation_mail_recipients: 20,
            invitation_mail_per_hour: 100,
            invitation_mail_per_org_hour: 300,
            invitation_mail_per_address_hour: 5,
        }
    }
}

impl std::fmt::Debug for SmtpSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpSettings")
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("smtp_tls", &self.smtp_tls)
            .field("mail_from", &self.mail_from)
            .field("mail_per_second", &self.mail_per_second)
            .field("mail_reset_queue", &self.mail_reset_queue)
            .field(
                "mail_reset_per_account_hour",
                &self.mail_reset_per_account_hour,
            )
            .field("mail_invitation_queue", &self.mail_invitation_queue)
            .field(
                "invitation_mail_recipients",
                &self.invitation_mail_recipients,
            )
            .field("invitation_mail_per_hour", &self.invitation_mail_per_hour)
            .field(
                "invitation_mail_per_org_hour",
                &self.invitation_mail_per_org_hour,
            )
            .field(
                "invitation_mail_per_address_hour",
                &self.invitation_mail_per_address_hour,
            )
            .finish_non_exhaustive()
    }
}

pub(super) fn check_password_is_text(keys: &Map<String, Value>) -> anyhow::Result<()> {
    match keys.get("smtp_password") {
        None | Some(Value::String(_)) => Ok(()),
        Some(_) => bail!("smtp_password must be a string"),
    }
}

impl SmtpSettings {
    /// Whether this server has an outgoing SMTP transport configured.
    pub fn enabled(&self) -> bool {
        !self.smtp_host.is_empty()
    }

    /// Validates the limits and the relay's settings without contacting it; the
    /// sender, and whether the relay is the way mail leaves, are
    /// [`MailSettings::way`](super::mail::MailSettings::way)'s.
    ///
    /// # Errors
    /// Returns an error naming invalid or incomplete settings, without credentials.
    pub fn check(&self) -> anyhow::Result<()> {
        if self.smtp_port == 0 {
            bail!("smtp_port must be 1 to 65535");
        }
        for (key, value) in [
            ("mail_per_second", self.mail_per_second),
            ("mail_reset_queue", self.mail_reset_queue),
            (
                "mail_reset_per_account_hour",
                self.mail_reset_per_account_hour,
            ),
            (
                "invitation_mail_recipients",
                self.invitation_mail_recipients,
            ),
            (
                "invitation_mail_per_address_hour",
                self.invitation_mail_per_address_hour,
            ),
        ] {
            if value == 0 {
                bail!("{key} must be at least 1");
            }
        }
        let recipients = self.invitation_mail_recipients;
        if recipients > self.mail_invitation_queue / 2 {
            bail!(
                "mail_invitation_queue ({}) must be at least twice invitation_mail_recipients \
                 ({recipients}): an organization of several holds half of what the others \
                 leave, and half must hold a request",
                self.mail_invitation_queue
            );
        }
        for (key, limit) in [
            ("invitation_mail_per_hour", self.invitation_mail_per_hour),
            (
                "invitation_mail_per_org_hour",
                self.invitation_mail_per_org_hour,
            ),
        ] {
            if recipients > limit {
                bail!("{key} ({limit}) must be at least invitation_mail_recipients ({recipients})");
            }
        }
        // A workspace is in an organization: its hour above the organization's would
        // be a limit nobody reaches, and the organization's the one that holds.
        if self.invitation_mail_per_org_hour < self.invitation_mail_per_hour {
            bail!(
                "invitation_mail_per_org_hour ({}) must be at least invitation_mail_per_hour ({})",
                self.invitation_mail_per_org_hour,
                self.invitation_mail_per_hour
            );
        }
        if self.smtp_user.is_empty() != self.smtp_password.is_empty() {
            bail!("smtp_user and smtp_password must be supplied together");
        }
        if self
            .smtp_host
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '@' | '#' | '?'))
        {
            bail!("smtp_host must be a host name or IP address");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Configuration;

    #[test]
    fn mail_is_optional_and_incomplete_or_injected_settings_are_refused() {
        assert!(!SmtpSettings::default().enabled());
        SmtpSettings::default().check().expect("disabled mail");
        let json = serde_json::json!({"smtp_host":"smtp.example.test","smtp_tls":"optional"});
        assert!(
            Configuration::parse(&json.to_string(), |_| None).is_err(),
            "an unknown TLS mode is never taken for an unencrypted one"
        );
        for (key, value) in [
            ("mail_from", "sender@example.test\r\nBcc:other@example.test"),
            ("smtp_host", "smtp.example.test\nEHLO evil"),
        ] {
            let mut json = serde_json::json!({"smtp_host":"smtp.example.test","mail_from":"sender@example.test"});
            json[key] = Value::String(value.to_owned());
            assert!(
                Configuration::parse(&json.to_string(), |_| None)
                    .expect("parse")
                    .check()
                    .is_err(),
                "{key}"
            );
        }
    }

    #[test]
    fn mail_limits_hold_a_whole_request_with_or_without_smtp() {
        for (key, value) in [
            ("mail_per_second", 0),
            ("mail_reset_queue", 0),
            ("mail_reset_per_account_hour", 0),
            ("invitation_mail_recipients", 0),
            ("mail_invitation_queue", 39),
            ("invitation_mail_per_hour", 19),
            ("invitation_mail_per_org_hour", 19),
            ("invitation_mail_per_org_hour", 99),
        ] {
            for host in ["", "smtp.example.test"] {
                let mut json = serde_json::json!({"smtp_host": host});
                if !host.is_empty() {
                    json["mail_from"] = Value::from("sender@example.test");
                }
                json[key] = Value::from(value);
                let error = Configuration::parse(&json.to_string(), |_| None)
                    .expect("parse")
                    .check()
                    .expect_err("a limit that cannot hold a request");
                assert!(error.to_string().contains(key), "{key}: {error}");
            }
        }
        let fits = serde_json::json!({
            "invitation_mail_recipients": 1,
            "invitation_mail_per_hour": 1,
            "invitation_mail_per_org_hour": 1,
            "mail_invitation_queue": 2,
            "mail_reset_queue": 1,
            "mail_reset_per_account_hour": 1,
        });
        Configuration::parse(&fits.to_string(), |_| None)
            .expect("parse")
            .check()
            .expect("the smallest limits that hold a request");
    }

    #[test]
    fn smtp_credentials_never_appear_in_debug_or_type_errors() {
        let text = r#"{"smtp_password":1234567890}"#;
        let error = Configuration::parse(text, |_| None)
            .expect_err("wrong type")
            .to_string();
        assert!(!error.contains("1234567890"), "{error}");
        let settings = SmtpSettings {
            smtp_user: "private-user".to_owned(),
            smtp_password: "private-password".to_owned(),
            ..SmtpSettings::default()
        };
        let debug = format!("{settings:?}");
        assert!(!debug.contains("private-"), "{debug}");
    }
}
