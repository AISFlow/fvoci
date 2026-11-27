//! Sending over SMTP: the way of [`Outbound`] that `smtp_host` configures.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use lettre::address::Envelope;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Address, AsyncSmtpTransport, AsyncTransport as _, Tokio1Executor};

use super::mime;
use super::outbound::{Outbound, Outcome, Outgoing};
use crate::config::smtp::{SmtpSettings, SmtpTls};

/// An SMTP relay; constructing it performs no network I/O.
pub(super) struct Smtp {
    transport: AsyncSmtpTransport<Tokio1Executor>,
}

impl Smtp {
    /// The relay of `settings`, whose commands each get `timeout`.
    ///
    /// # Errors
    /// Returns an error if the host cannot be used for TLS.
    pub(super) fn new(settings: &SmtpSettings, timeout: Duration) -> anyhow::Result<Self> {
        let mut builder = match settings.smtp_tls {
            SmtpTls::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&settings.smtp_host)?,
            SmtpTls::Starttls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&settings.smtp_host)?
            }
            SmtpTls::Unencrypted => {
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&settings.smtp_host)
            }
        }
        .port(settings.smtp_port)
        .timeout(Some(timeout));
        if !settings.smtp_user.is_empty() {
            builder = builder.credentials(Credentials::new(
                settings.smtp_user.clone(),
                settings.smtp_password.clone(),
            ));
        }
        Ok(Self {
            transport: builder.build(),
        })
    }

    async fn deliver(&self, message: &Outgoing) -> Outcome {
        let Ok((envelope, bytes)) = build(message) else {
            // An address the model takes and SMTP does not (a dot at an end of its
            // local part, or two dots together) would be refused by every relay.
            return Outcome::Refused { code: None };
        };
        match self.transport.send_raw(&envelope, &bytes).await {
            Ok(_) => Outcome::Sent,
            Err(error) => outcome_of_error(&error),
        }
    }
}

impl Outbound for Smtp {
    fn name(&self) -> &'static str {
        "smtp"
    }

    fn send<'a>(
        &'a self,
        message: &'a Outgoing,
    ) -> Pin<Box<dyn Future<Output = Outcome> + Send + 'a>> {
        Box::pin(self.deliver(message))
    }
}

/// What an SMTP error makes of the attempt. A connection that cannot be made (a host
/// name that names nothing, a refused connection, a TLS handshake that fails), a relay
/// without the STARTTLS that `smtp_tls` asks for, and one with no authentication the
/// credentials can use close the whole way. lettre tells these apart from the other
/// failures only by their text, so the text is read here and nowhere else, and never
/// kept. A relay that does not answer in time is worth another attempt, as a timeout
/// is anywhere else; one that refuses the server at its greeting (`554`) cannot be told
/// apart from one that refuses a recipient, and refuses the message.
fn outcome_of_error(error: &lettre::transport::smtp::Error) -> Outcome {
    if error.status().is_none() {
        let text = error.to_string();
        if CLOSED_WAY_ERRORS
            .iter()
            .any(|closed| text.starts_with(closed))
        {
            return Outcome::PathDown { code: None };
        }
    }
    outcome_of(error.status().map(u16::from))
}

/// How lettre's texts of the failures that close the whole way start: no connection,
/// no STARTTLS, no authentication mechanism in common.
const CLOSED_WAY_ERRORS: [&str; 3] = [
    "Connection error",
    "internal client error: STARTTLS is not supported",
    "internal client error: No compatible authentication mechanism",
];

/// What an SMTP reply makes of the attempt: the relay refusing the server's credentials
/// (`530` authentication required, `534` a mechanism too weak, `535` the credentials
/// themselves, `538` authentication only over encryption) closes the whole way; any other 5xx refuses this message for good; a 4xx
/// reply (`421`, `451`, `454`) and anything without a reply (a timeout, a connection
/// lost in the middle) is worth another attempt. Only the reply code is kept, never its
/// text.
fn outcome_of(reply: Option<u16>) -> Outcome {
    match reply {
        Some(code @ (530 | 534 | 535 | 538)) => Outcome::PathDown { code: Some(code) },
        Some(code @ 500..=599) => Outcome::Refused { code: Some(code) },
        code => Outcome::Later { after: None, code },
    }
}

/// The envelope and the MIME text of `message`.
fn build(message: &Outgoing) -> anyhow::Result<(Envelope, Vec<u8>)> {
    let from: Address = message.from.as_str().parse()?;
    let envelope = Envelope::new(Some(from), vec![message.to.as_str().parse()?])?;
    Ok((envelope, mime::message(message)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_of_the_credentials_closes_the_way() {
        for code in [530, 534, 535, 538] {
            assert_eq!(
                outcome_of(Some(code)),
                Outcome::PathDown { code: Some(code) },
                "the relay refuses the credentials: the whole way is closed"
            );
        }
    }

    #[test]
    fn another_5xx_reply_refuses_for_good_and_anything_else_asks_for_later() {
        assert_eq!(outcome_of(Some(550)), Outcome::Refused { code: Some(550) });
        assert_eq!(outcome_of(Some(554)), Outcome::Refused { code: Some(554) });
        for code in [Some(421), Some(451), Some(454), None] {
            assert_eq!(outcome_of(code), Outcome::Later { after: None, code });
        }
    }
}
