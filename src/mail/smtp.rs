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
        .map_err(|_| "invalid_from".to_string())?;
    let to: Mailbox = to.parse().map_err(|_| "invalid_recipient".to_string())?;
    let message = Message::builder()
        .from(from)
        .to(to)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(text.to_string())
        .map_err(|_| "invalid_message".to_string())?;

    let tls = TlsParameters::new(smtp.host.clone()).map_err(|_| "tls_config".to_string())?;
    let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(smtp.host.as_str())
        .port(smtp.port)
        .tls(Tls::Opportunistic(tls))
        .timeout(Some(SMTP_TIMEOUT))
        .build();
    transport.send(message).await.map(|_| ()).map_err(|err| {
        if err.is_permanent() {
            "permanent".to_string()
        } else if err.is_transient() {
            "transient".to_string()
        } else if err.is_timeout() {
            "timeout".to_string()
        } else {
            "connection".to_string()
        }
    })
}
