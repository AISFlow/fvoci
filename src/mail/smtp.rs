//! SMTP transport matching the source mailer (nodemailer with host/port/from and
//! no AUTH env): STARTTLS is used whenever the server offers it, as nodemailer
//! does by default, and the message is built with RFC 5322/2047 encoding.

use std::time::Duration;

use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use super::{MailSendError, SmtpConfig};

const SMTP_TIMEOUT: Duration = Duration::from_secs(15);
/// Whole-session bound, kept below the 30 s outbox lease.
const SMTP_SESSION_TIMEOUT: Duration = Duration::from_secs(20);

// `MailSendError::code` values from `send_mail_op`. They never carry server
// text or the recipient address.
/// The From header and the envelope sender both fail to parse.
const CODE_INVALID_FROM: &str = "invalid_from";
/// The recipient address does not parse as a mailbox.
const CODE_INVALID_RECIPIENT: &str = "invalid_recipient";
/// The message could not be built.
const CODE_INVALID_MESSAGE: &str = "invalid_message";
/// TLS parameters for the host could not be built.
const CODE_TLS_CONFIG: &str = "tls_config";
/// The session, or one command in it, timed out.
const CODE_TIMEOUT: &str = "timeout";
/// The server answered with a permanent (5xx) reply.
const CODE_PERMANENT: &str = "permanent";
/// The server answered with a transient (4xx) reply.
const CODE_TRANSIENT: &str = "transient";
/// Connection, TLS or protocol failure.
const CODE_CONNECTION: &str = "connection";

/// Whether a send failure is final for this one recipient: the server refused
/// it permanently, or the address cannot be sent to. Other failures (4xx,
/// timeouts, connection or local configuration) may pass on a later attempt,
/// or would fail for every recipient alike.
pub(super) fn is_final_for_recipient(code: &str) -> bool {
    code == CODE_PERMANENT || code == CODE_INVALID_RECIPIENT
}

pub async fn send_mail_op(
    smtp: &SmtpConfig,
    op: &'static str,
    from_header: &str,
    envelope_from: &str,
    to: &str,
    subject: &str,
    text: &str,
) -> Result<(), MailSendError> {
    send_mail_inner(smtp, from_header, envelope_from, to, subject, text)
        .await
        .map_err(|code| MailSendError { op, code })
}

async fn send_mail_inner(
    smtp: &SmtpConfig,
    from_header: &str,
    envelope_from: &str,
    to: &str,
    subject: &str,
    text: &str,
) -> Result<(), String> {
    let from: Mailbox = from_header
        .parse()
        .or_else(|_| envelope_from.parse())
        .map_err(|_| CODE_INVALID_FROM.to_string())?;
    let to: Mailbox = to.parse().map_err(|_| CODE_INVALID_RECIPIENT.to_string())?;
    let message = Message::builder()
        .message_id(None)
        .from(from)
        .to(to)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(text.to_string())
        .map_err(|_| CODE_INVALID_MESSAGE.to_string())?;

    let tls = TlsParameters::new(smtp.host.clone()).map_err(|_| CODE_TLS_CONFIG.to_string())?;
    let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(smtp.host.as_str())
        .port(smtp.port)
        .tls(Tls::Opportunistic(tls))
        .timeout(Some(SMTP_TIMEOUT))
        .build();
    let sent = tokio::time::timeout(SMTP_SESSION_TIMEOUT, transport.send(message))
        .await
        .map_err(|_| CODE_TIMEOUT.to_string())?;
    sent.map(|_| ()).map_err(|err| {
        if err.is_permanent() {
            CODE_PERMANENT.to_string()
        } else if err.is_transient() {
            CODE_TRANSIENT.to_string()
        } else if err.is_timeout() {
            CODE_TIMEOUT.to_string()
        } else {
            CODE_CONNECTION.to_string()
        }
    })
}

/// Connection check for `fvoci-migrate --doctor`: connect, EHLO and STARTTLS
/// when offered (the same transport settings as sending), then QUIT. Sends no
/// mail. The error is a short code without server text.
pub async fn probe_smtp(smtp: &SmtpConfig) -> Result<(), String> {
    let tls = TlsParameters::new(smtp.host.clone()).map_err(|_| "tls_config".to_string())?;
    let transport: AsyncSmtpTransport<Tokio1Executor> =
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(smtp.host.as_str())
            .port(smtp.port)
            .tls(Tls::Opportunistic(tls))
            .timeout(Some(SMTP_TIMEOUT))
            .build();
    match tokio::time::timeout(SMTP_SESSION_TIMEOUT, transport.test_connection()).await {
        Ok(Ok(true)) => Ok(()),
        Ok(Ok(false)) => Err("smtp_not_ready".into()),
        Ok(Err(_)) => Err("smtp_connect_failed".into()),
        Err(_) => Err("smtp_timeout".into()),
    }
}
