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
/// The server refused the recipient's mailbox with a permanent (5xx) reply
/// (see `is_mailbox_refusal`).
const CODE_RECIPIENT_REJECTED: &str = "recipient_rejected";
/// Any other permanent (5xx) reply: a policy, quota, system, protocol or
/// content refusal, or a refusal of the sender. It is not known to be about
/// one recipient: relays send these codes both for one recipient (`554 5.7.1
/// Relay access denied` at `RCPT`) and for every recipient (`550 5.4.5` daily
/// limit, a refused sender).
const CODE_PERMANENT: &str = "permanent";
/// The server answered with a transient (4xx) reply.
const CODE_TRANSIENT: &str = "transient";
/// Connection, TLS or protocol failure.
const CODE_CONNECTION: &str = "connection";

/// Whether a send failure is final for this one recipient: the server refused
/// the recipient's mailbox permanently, or the address cannot be sent to.
/// Other failures (4xx, other 5xx, timeouts, connection or local
/// configuration) may pass on a later attempt, may fail for every recipient
/// alike, or (other 5xx, see `is_unclassified_refusal`) may be either.
pub(super) fn is_final_for_recipient(code: &str) -> bool {
    code == CODE_RECIPIENT_REJECTED || code == CODE_INVALID_RECIPIENT
}

/// Whether the recipient address does not parse as a mailbox. The send
/// fails before any contact with the relay, so this says nothing about the
/// relay, and no retry can send it.
pub(super) fn is_unsendable_address(code: &str) -> bool {
    code == CODE_INVALID_RECIPIENT
}

/// Whether a send failure is a permanent (5xx) refusal that does not say
/// whether it is about the recipient or about every recipient (see
/// `CODE_PERMANENT`).
pub(super) fn is_unclassified_refusal(code: &str) -> bool {
    code == CODE_PERMANENT
}

/// Whether a 5xx reply refuses the recipient's mailbox rather than the whole
/// relay. lettre reports every 5xx as permanent whatever command it answers
/// (greeting, EHLO, MAIL FROM, RCPT TO, DATA) and does not say which one, so
/// the RFC 3463 enhanced status code that starts the server text decides.
/// X.3 to X.7 (system, network, protocol, content, policy, including quota
/// such as 5.4.5) are not about the recipient. Of X.1 (addressing) only the
/// registered destination address details count: X.1.1 to X.1.4, X.1.6
/// (mailbox moved) and X.1.10 (null MX). That leaves out X.1.7 and X.1.8
/// (the sender), X.1.0 (other address status, which Postfix and Exchange
/// also send for a refused sender) and vendor details such as Exchange's
/// 5.1.90 (the sender's daily recipient limit). Of X.2 (mailbox status)
/// every detail counts except X.2.0 (other mailbox status, which Exchange
/// also sends for sender quotas and blocks) and X.2.3 (message length over
/// an administrative limit, which some relays apply to every recipient).
///
/// Known limitation: X.2.2 is final for the recipient, since it is how a
/// full mailbox is refused, but Exchange Online also sends 5.2.2 for its
/// sender-wide "submission quota exceeded". Only the text tells them apart.
///
/// Without an enhanced status code only 551 (user not local) counts: a bare
/// 550 or 553 is also sent for policy refusals and sending limits (for
/// example cPanel's hourly limit, qmail's rcpthosts), so those stay
/// unclassified and need a later accepted send to prove them. An enhanced
/// code later in the text (qmail's `(#5.1.1)`) is not read.
fn is_mailbox_refusal(reply: u16, text: &str) -> bool {
    let enhanced = text.split_whitespace().next().and_then(|word| {
        let mut parts = word.split('.');
        let (class, subject, detail) = (parts.next()?, parts.next()?, parts.next()?);
        let number =
            |part: &str| (1..=3).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit());
        (parts.next().is_none() && class == "5" && number(subject) && number(detail))
            .then_some((subject, detail))
    });
    match enhanced {
        Some(("1", detail)) => matches!(detail, "1" | "2" | "3" | "4" | "6" | "10"),
        Some(("2", detail)) => !matches!(detail, "0" | "3"),
        Some(_) => false,
        None => reply == 551,
    }
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

    let transport = build_transport(smtp).map_err(str::to_string)?;
    let sent = tokio::time::timeout(SMTP_SESSION_TIMEOUT, transport.send(message))
        .await
        .map_err(|_| CODE_TIMEOUT.to_string())?;
    sent.map(|_| ()).map_err(|err| {
        if err.is_permanent() {
            // lettre keeps the server text as the error source. It is read
            // here to classify the reply and never logged.
            let reply = err.status().map_or(0, u16::from);
            let text = std::error::Error::source(&err)
                .map(ToString::to_string)
                .unwrap_or_default();
            if is_mailbox_refusal(reply, &text) {
                CODE_RECIPIENT_REJECTED.to_string()
            } else {
                CODE_PERMANENT.to_string()
            }
        } else if err.is_transient() {
            CODE_TRANSIENT.to_string()
        } else if err.is_timeout() {
            CODE_TIMEOUT.to_string()
        } else {
            CODE_CONNECTION.to_string()
        }
    })
}

/// The transport of every send and of the doctor probe: STARTTLS whenever
/// the server offers it, with the certificate verified for `smtp.host`, and
/// `SMTP_TIMEOUT` per command. The error is `CODE_TLS_CONFIG`.
fn build_transport(smtp: &SmtpConfig) -> Result<AsyncSmtpTransport<Tokio1Executor>, &'static str> {
    let tls = TlsParameters::new(smtp.host.clone()).map_err(|_| CODE_TLS_CONFIG)?;
    Ok(
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(smtp.host.as_str())
            .port(smtp.port)
            .tls(Tls::Opportunistic(tls))
            .timeout(Some(SMTP_TIMEOUT))
            .build(),
    )
}

/// Connection check for `fvoci-migrate --doctor`: connect, EHLO and STARTTLS
/// when offered (`build_transport`, as for sending), then QUIT. Sends no
/// mail. The error is a short code without server text.
pub async fn probe_smtp(smtp: &SmtpConfig) -> Result<(), String> {
    let transport = build_transport(smtp).map_err(str::to_string)?;
    match tokio::time::timeout(SMTP_SESSION_TIMEOUT, transport.test_connection()).await {
        Ok(Ok(true)) => Ok(()),
        Ok(Ok(false)) => Err("smtp_not_ready".into()),
        Ok(Err(_)) => Err("smtp_connect_failed".into()),
        Err(_) => Err("smtp_timeout".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_mailbox_refusals_are_final_for_one_recipient() {
        // Enhanced status codes about the recipient's address or mailbox.
        assert!(is_mailbox_refusal(550, "5.1.1 no such user"));
        assert!(is_mailbox_refusal(
            553,
            "5.1.3 bad destination mailbox syntax"
        ));
        assert!(is_mailbox_refusal(
            556,
            "5.1.10 recipient address has null MX"
        ));
        assert!(is_mailbox_refusal(
            550,
            "5.1.2 bad destination system address"
        ));
        assert!(is_mailbox_refusal(
            550,
            "5.1.4 destination mailbox ambiguous"
        ));
        assert!(is_mailbox_refusal(551, "5.1.6 mailbox has moved"));
        assert!(is_mailbox_refusal(550, "5.2.1 mailbox disabled"));
        assert!(is_mailbox_refusal(552, "5.2.2 mailbox full"));
        // The sender, codes also sent for the sender or the whole relay, and
        // every other subject are not certain to be about the recipient.
        assert!(!is_mailbox_refusal(553, "5.1.7 bad sender mailbox syntax"));
        assert!(!is_mailbox_refusal(550, "5.1.8 bad sender system address"));
        assert!(!is_mailbox_refusal(
            550,
            "5.1.0 <from@example.com>: Sender address rejected: User unknown in virtual alias table"
        ));
        assert!(!is_mailbox_refusal(554, "5.1.0 Sender denied"));
        // X.1 details outside the registered destination address codes,
        // such as Exchange's sender limit, are not about the recipient.
        assert!(!is_mailbox_refusal(
            550,
            "5.1.90 Your message can't be sent because you've reached your daily limit for message recipients"
        ));
        assert!(!is_mailbox_refusal(550, "5.1.5 destination address valid"));
        assert!(!is_mailbox_refusal(550, "5.1.9 non-compliant mailer"));
        assert!(!is_mailbox_refusal(
            550,
            "5.1.11 sender address has null MX"
        ));
        assert!(!is_mailbox_refusal(
            552,
            "5.2.3 Your message exceeded the size limit"
        ));
        assert!(!is_mailbox_refusal(
            550,
            "5.4.5 Daily SMTP relay limit exceeded"
        ));
        assert!(!is_mailbox_refusal(550, "5.7.1 relaying denied"));
        assert!(!is_mailbox_refusal(554, "5.6.0 message content rejected"));
        assert!(!is_mailbox_refusal(552, "5.3.4 message too big for system"));
        assert!(!is_mailbox_refusal(530, "5.7.0 authentication required"));
        assert!(!is_mailbox_refusal(500, "5.5.2 syntax error"));
        // Without an enhanced status code only 551 counts.
        assert!(!is_mailbox_refusal(550, "no such user here"));
        assert!(is_mailbox_refusal(551, "user not local"));
        // An enhanced status code decides over the reply code, and only one
        // that starts the text is read.
        assert!(!is_mailbox_refusal(551, "5.7.1 relaying denied"));
        assert!(!is_mailbox_refusal(
            550,
            "sorry, no mailbox here by that name. (#5.1.1)"
        ));
        assert!(!is_mailbox_refusal(553, "mailbox name not allowed"));
        assert!(!is_mailbox_refusal(550, ""));
        assert!(!is_mailbox_refusal(
            550,
            "Domain example.com has exceeded the max emails per hour (100/100 (100%)) allowed."
        ));
        assert!(!is_mailbox_refusal(
            553,
            "sorry, that domain isn't in my list of allowed rcpthosts"
        ));
        assert!(!is_mailbox_refusal(
            554,
            "5.2.0 STOREDRV.Submission.Exception:SubmissionQuotaExceededException"
        ));
        assert!(!is_mailbox_refusal(554, "transaction failed"));
        assert!(!is_mailbox_refusal(552, "storage allocation exceeded"));
        assert!(!is_mailbox_refusal(530, "authentication required"));
        // Not an enhanced status code: the reply code decides.
        assert!(!is_mailbox_refusal(554, "5.1 rejected"));
        assert!(!is_mailbox_refusal(554, "5.1.1.1 rejected"));
        assert!(!is_mailbox_refusal(554, "4.1.1 wrong class"));
        assert!(!is_mailbox_refusal(550, "5.x.1 rejected"));
    }

    #[test]
    fn final_for_recipient_codes() {
        assert!(is_final_for_recipient(CODE_RECIPIENT_REJECTED));
        assert!(is_final_for_recipient(CODE_INVALID_RECIPIENT));
        assert!(!is_final_for_recipient(CODE_PERMANENT));
        assert!(!is_final_for_recipient(CODE_TRANSIENT));
        assert!(!is_final_for_recipient(CODE_TIMEOUT));
        assert!(!is_final_for_recipient(CODE_CONNECTION));
        assert!(is_unclassified_refusal(CODE_PERMANENT));
        for code in [
            CODE_RECIPIENT_REJECTED,
            CODE_INVALID_RECIPIENT,
            CODE_TRANSIENT,
            CODE_TIMEOUT,
            CODE_CONNECTION,
        ] {
            assert!(!is_unclassified_refusal(code), "{code}");
        }
        assert!(is_unsendable_address(CODE_INVALID_RECIPIENT));
        for code in [
            CODE_RECIPIENT_REJECTED,
            CODE_PERMANENT,
            CODE_TRANSIENT,
            CODE_TIMEOUT,
            CODE_CONNECTION,
        ] {
            assert!(!is_unsendable_address(code), "{code}");
        }
    }

    /// `users.email` only has to pass `normalize_email`, whose pattern is
    /// broader than lettre's mailbox parser: these addresses can be stored
    /// but not sent to.
    #[test]
    fn stored_addresses_can_fail_to_parse_as_a_mailbox() {
        for address in ["a..b@example.com", "bob@example.com."] {
            assert!(
                crate::validate::normalize_email(address).is_ok(),
                "{address}"
            );
            assert!(address.parse::<Mailbox>().is_err(), "{address}");
        }
    }

    /// The recipient is parsed before the transport is built or the relay is
    /// contacted, so `invalid_recipient` never follows a session.
    #[tokio::test]
    async fn an_unsendable_address_fails_before_any_connection() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("free port")
            .port();
        let smtp = SmtpConfig {
            host: "127.0.0.1".into(),
            port,
            from: "noreply@example.com".into(),
        };
        let code = send_mail_inner(
            &smtp,
            "noreply@example.com",
            "noreply@example.com",
            "a..b@example.com",
            "subject",
            "text",
        )
        .await
        .expect_err("unsendable");
        assert_eq!(code, CODE_INVALID_RECIPIENT);
    }
}
