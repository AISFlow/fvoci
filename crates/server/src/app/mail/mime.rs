//! The MIME text of a message, as SMTP carries it and SES's raw sending takes it.

use mail_builder::MessageBuilder;
use mail_builder::headers::raw::Raw;

use super::outbound::Outgoing;

/// The MIME text of `message`: its plain text and HTML, its language, and a Message-ID
/// of the sender's domain.
///
/// # Errors
/// Returns what the builder refuses.
pub(super) fn message(message: &Outgoing) -> anyhow::Result<Vec<u8>> {
    let message_id = format!(
        "{}@{}",
        crate::auth::secret::new_secret(),
        message.from.domain()
    );
    Ok(MessageBuilder::new()
        .from(message.from.as_str())
        .to(message.to.as_str())
        .message_id(message_id)
        .header("Content-Language", Raw::new(message.language.tag()))
        .subject(message.subject)
        .text_body(message.plain.as_str())
        .html_body(message.html.as_str())
        .write_to_vec()?)
}
