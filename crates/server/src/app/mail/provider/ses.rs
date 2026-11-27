//! Sending through Amazon SES's API v2 (`SendEmail`), the message as raw MIME, signed
//! with Signature Version 4.
//! <https://docs.aws.amazon.com/ses/latest/APIReference-V2/API_SendEmail.html>

use std::future::Future;
use std::pin::Pin;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::json;

use super::super::mime;
use super::super::outbound::{Outbound, Outcome, Outgoing};
use super::clock::Moment;
use super::sigv4::{Request, Signer};
use super::{Answer, host_of};
use crate::config::mail::{Secret, SesKeys};

/// The path of `SendEmail`.
const PATH: &str = "/v2/email/outbound-emails";

/// SES's way of sending; constructing it performs no network I/O.
pub(in crate::app::mail) struct Ses {
    client: reqwest::Client,
    url: reqwest::Url,
    /// The `Host` header the requests are signed with.
    host: String,
    region: String,
    access_key_id: String,
    secret_access_key: Secret,
}

impl Ses {
    /// SES at the endpoint of `keys`, whose requests `client` makes.
    ///
    /// # Errors
    /// Returns an error if the endpoint names no host.
    pub(in crate::app::mail) fn new(
        keys: SesKeys,
        client: reqwest::Client,
    ) -> anyhow::Result<Self> {
        let url = reqwest::Url::parse(&format!("{}{PATH}", keys.endpoint))?;
        Ok(Self {
            client,
            host: host_of(&url)?,
            url,
            region: keys.region,
            access_key_id: keys.access_key_id,
            secret_access_key: keys.secret_access_key,
        })
    }

    async fn deliver(&self, message: &Outgoing) -> Outcome {
        let Ok(raw) = mime::message(message) else {
            // Addresses were checked before the message was queued.
            return Outcome::Refused { code: None };
        };
        let body = json!({
            "FromEmailAddress": message.from.as_str(),
            "Destination": {"ToAddresses": [message.to.as_str()]},
            "Content": {"Raw": {"Data": STANDARD.encode(raw)}},
        })
        .to_string();
        let now = Moment::now();
        let amz_date = now.amz_date();
        // What is signed is what is sent: the host is the one reqwest writes.
        let headers = [
            ("content-type", "application/json"),
            ("host", self.host.as_str()),
            ("x-amz-date", amz_date.as_str()),
        ];
        let signer = Signer {
            access_key_id: &self.access_key_id,
            secret_access_key: self.secret_access_key.expose(),
            region: &self.region,
            service: "ses",
        };
        let authorization = signer.authorization(
            &Request {
                method: "POST",
                path: PATH,
                headers: &headers,
                body: body.as_bytes(),
            },
            now,
        );
        let request = headers
            .iter()
            .filter(|(name, _)| *name != "host")
            .fold(
                self.client.post(self.url.clone()),
                |request, (name, value)| request.header(*name, *value),
            )
            .header("authorization", authorization)
            .body(body);
        match super::send(request).await {
            Ok(answer) => outcome_of(&answer),
            Err(outcome) => outcome,
        }
    }
}

impl Outbound for Ses {
    fn name(&self) -> &'static str {
        "ses"
    }

    fn send<'a>(
        &'a self,
        message: &'a Outgoing,
    ) -> Pin<Box<dyn Future<Output = Outcome> + Send + 'a>> {
        Box::pin(self.deliver(message))
    }
}

/// What SES's answer makes of the attempt. SES names its error in `X-Amzn-ErrorType`,
/// and some of its `400`s stop all mail rather than this message: past the sending
/// rate or the day's quota (`ThrottlingException`, also `TooManyRequestsException`'s
/// `429` and `LimitExceededException`) is worth another attempt later, and a paused or
/// suspended account (`SendingPausedException`, `AccountSuspendedException`) closes the
/// way. A sender that is not verified (`MailFromDomainNotVerifiedException`, `400`) is
/// the operator's to set up, and refuses the message as any other `400`.
fn outcome_of(answer: &Answer) -> Outcome {
    let code = Some(answer.status);
    match error_name(answer).as_deref() {
        Some("ThrottlingException" | "TooManyRequestsException" | "LimitExceededException") => {
            Outcome::Later {
                after: answer.retry_after,
                code,
            }
        }
        Some("SendingPausedException" | "AccountSuspendedException") => Outcome::PathDown { code },
        _ => super::outcome_of(answer),
    }
}

/// The name of SES's error: `X-Amzn-ErrorType`, or else the body's `__type` or `code`
/// (`com.amazonaws.sesv2#ThrottlingException`), as AWS's JSON protocols give it. Only
/// the name is kept, never the message.
fn error_name(answer: &Answer) -> Option<String> {
    answer.error_type.clone().or_else(|| {
        let body: serde_json::Value = serde_json::from_slice(&answer.body).ok()?;
        let name = body.get("__type").or_else(|| body.get("code"))?.as_str()?;
        let name = name.rsplit('#').next()?.split(':').next()?.trim();
        (!name.is_empty()).then(|| name.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(status: u16, error_type: &str) -> Answer {
        Answer {
            status,
            retry_after: None,
            error_type: (!error_type.is_empty()).then(|| error_type.to_owned()),
            body: Vec::new(),
        }
    }

    #[test]
    fn ses_errors_sort_by_their_names_first() {
        let later = |status| Outcome::Later {
            after: None,
            code: Some(status),
        };
        assert_eq!(outcome_of(&answer(200, "")), Outcome::Sent);
        assert_eq!(outcome_of(&answer(400, "ThrottlingException")), later(400));
        assert_eq!(
            outcome_of(&answer(400, "LimitExceededException")),
            later(400)
        );
        assert_eq!(
            outcome_of(&answer(429, "TooManyRequestsException")),
            later(429)
        );
        assert_eq!(
            outcome_of(&answer(400, "SendingPausedException")),
            Outcome::PathDown { code: Some(400) }
        );
        assert_eq!(
            outcome_of(&answer(403, "SignatureDoesNotMatch")),
            Outcome::PathDown { code: Some(403) }
        );
        for name in [
            "MessageRejected",
            "MailFromDomainNotVerifiedException",
            "BadRequestException",
        ] {
            assert_eq!(
                outcome_of(&answer(400, name)),
                Outcome::Refused { code: Some(400) },
                "{name}"
            );
        }
        assert_eq!(outcome_of(&answer(500, "InternalFailure")), later(500));
    }

    #[test]
    fn a_throttled_answer_keeps_its_retry_after_and_a_name_in_the_body_counts() {
        let throttled = Answer {
            retry_after: Some(std::time::Duration::from_secs(7)),
            ..answer(400, "ThrottlingException")
        };
        assert_eq!(
            outcome_of(&throttled),
            Outcome::Later {
                after: Some(std::time::Duration::from_secs(7)),
                code: Some(400)
            }
        );
        for body in [
            r#"{"__type":"com.amazonaws.sesv2#ThrottlingException","message":"Maximum sending rate exceeded."}"#,
            r#"{"__type":"ThrottlingException:http://internal.amazon.com/","message":"…"}"#,
            r#"{"code":"ThrottlingException","message":"…"}"#,
        ] {
            let named_in_body = Answer {
                body: body.as_bytes().to_vec(),
                ..answer(400, "")
            };
            assert_eq!(
                outcome_of(&named_in_body),
                Outcome::Later {
                    after: None,
                    code: Some(400)
                },
                "{body}"
            );
        }
        let unnamed = Answer {
            body: br#"{"message":"rejected"}"#.to_vec(),
            ..answer(400, "")
        };
        assert_eq!(outcome_of(&unnamed), Outcome::Refused { code: Some(400) });
    }
}
