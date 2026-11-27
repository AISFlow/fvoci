//! Sending through Cloudflare Email Service's REST API: one JSON request per message,
//! under the account's API token.
//! <https://developers.cloudflare.com/email-service/api/send-emails/rest-api/>

use std::future::Future;
use std::pin::Pin;

use serde::Serialize;
use serde_json::Value;

use super::super::outbound::{Outbound, Outcome, Outgoing};
use super::Answer;
use crate::config::mail::{CloudflareKeys, Secret};

/// Cloudflare's way of sending; constructing it performs no network I/O.
pub(in crate::app::mail) struct Cloudflare {
    client: reqwest::Client,
    /// `…/client/v4/accounts/{account_id}/email/sending/send`.
    url: String,
    token: Secret,
}

impl Cloudflare {
    pub(in crate::app::mail) fn new(keys: CloudflareKeys, client: reqwest::Client) -> Self {
        Self {
            client,
            url: format!(
                "{}/client/v4/accounts/{}/email/sending/send",
                keys.endpoint, keys.account_id
            ),
            token: keys.api_token,
        }
    }

    async fn deliver(&self, message: &Outgoing) -> Outcome {
        let body = Sending {
            from: message.from.as_str(),
            to: message.to.as_str(),
            subject: message.subject,
            text: &message.plain,
            html: &message.html,
        };
        let request = self
            .client
            .post(&self.url)
            .bearer_auth(self.token.expose())
            .json(&body);
        match super::send(request).await {
            Ok(answer) => outcome_of(&answer, message.to.as_str()),
            Err(outcome) => outcome,
        }
    }
}

/// The body of a sending request: one recipient, the message in plain text and HTML.
#[derive(Serialize)]
struct Sending<'a> {
    from: &'a str,
    to: &'a str,
    subject: &'a str,
    text: &'a str,
    html: &'a str,
}

impl Outbound for Cloudflare {
    fn name(&self) -> &'static str {
        "cloudflare"
    }

    fn send<'a>(
        &'a self,
        message: &'a Outgoing,
    ) -> Pin<Box<dyn Future<Output = Outcome> + Send + 'a>> {
        Box::pin(self.deliver(message))
    }
}

/// What Cloudflare's answer makes of the attempt: by its status (its `429` carries the
/// code `10004`; its `401` and `403` the token's and the account's refusals), and a
/// message it took but lists among `result.permanent_bounces` refused for good. Its
/// `404` answers an account ID it cannot route to: the server's configuration, the same
/// for every message, so the whole way is closed rather than each message refused.
fn outcome_of(answer: &Answer, to: &str) -> Outcome {
    if answer.status == 404 {
        return Outcome::PathDown {
            code: Some(answer.status),
        };
    }
    let outcome = super::outcome_of(answer);
    if outcome != Outcome::Sent {
        return outcome;
    }
    let bounced = serde_json::from_slice::<Value>(&answer.body)
        .ok()
        .and_then(|body| {
            body.pointer("/result/permanent_bounces")
                .and_then(Value::as_array)
                .map(|bounces| {
                    bounces
                        .iter()
                        .filter_map(Value::as_str)
                        .any(|address| address.eq_ignore_ascii_case(to))
                })
        })
        .unwrap_or(false);
    if bounced {
        Outcome::Refused {
            code: Some(answer.status),
        }
    } else {
        Outcome::Sent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(status: u16, body: &str) -> Answer {
        Answer {
            status,
            retry_after: None,
            error_type: None,
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn a_recipient_among_the_permanent_bounces_is_refused_for_good() {
        let taken = r#"{"success":true,"errors":[],"messages":[],"result":{"delivered":["a@example.test"],"permanent_bounces":[],"queued":[]}}"#;
        assert_eq!(
            outcome_of(&answer(200, taken), "a@example.test"),
            Outcome::Sent
        );
        let bounced = r#"{"success":true,"errors":[],"messages":[],"result":{"delivered":[],"permanent_bounces":["A@example.test"],"queued":[]}}"#;
        assert_eq!(
            outcome_of(&answer(200, bounced), "a@example.test"),
            Outcome::Refused { code: Some(200) }
        );
        let throttled = r#"{"success":false,"errors":[{"code":10004,"message":"email.sending.error.throttled"}],"messages":[],"result":null}"#;
        assert_eq!(
            outcome_of(&answer(429, throttled), "a@example.test"),
            Outcome::Later {
                after: None,
                code: Some(429)
            }
        );
        let unauthorized = r#"{"success":false,"errors":[{"code":10101,"message":"email.sending.error.authentication.unauthorized"}],"messages":[],"result":null}"#;
        assert_eq!(
            outcome_of(&answer(401, unauthorized), "a@example.test"),
            Outcome::PathDown { code: Some(401) }
        );
        let unrouted = r#"{"success":false,"errors":[{"code":7003,"message":"Could not route to /client/v4/accounts/0/email/sending/send, perhaps your object identifier is invalid?"}],"messages":[],"result":null}"#;
        assert_eq!(
            outcome_of(&answer(404, unrouted), "a@example.test"),
            Outcome::PathDown { code: Some(404) }
        );
        let invalid = r#"{"success":false,"errors":[{"code":10001,"message":"email.sending.error.invalid_request_schema"}],"messages":[],"result":null}"#;
        for status in [400, 413, 422] {
            assert_eq!(
                outcome_of(&answer(status, invalid), "a@example.test"),
                Outcome::Refused { code: Some(status) },
                "{status}"
            );
        }
    }
}
