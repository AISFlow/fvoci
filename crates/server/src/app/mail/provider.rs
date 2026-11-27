//! Sending through a mail provider's HTTP API: the client all of them use, and what an
//! answer makes of the attempt by its status. Each provider's module reads its own
//! body on top of that. Nothing of an answer is kept but its status, how long it asked
//! to be left alone, the name of its error, and what its body says of the attempt:
//! never its text.

mod clock;
pub(super) mod cloudflare;
mod http_signature;
pub(super) mod oci;
pub(super) mod ses;
mod sigv4;

use std::time::Duration;

use super::outbound::Outcome;

/// The longest the connection to a provider may take to open, of the attempt's time.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The most of an answer's body that is read: a provider's answers are small, and what
/// is past this is not needed to sort the answer.
const MOST_BODY: usize = 64 * 1024;

/// The client of the providers: no redirect is followed and no proxy of the
/// environment is used (a request carries the provider's keys, and goes where
/// `mail_api_endpoint` or the provider's own address says, nowhere else), and each
/// request ends with the attempt.
///
/// # Errors
/// Returns an error if the TLS of the client cannot be set up.
pub(super) fn client(timeout: Duration) -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(timeout)
        .build()?)
}

/// What a provider answered, as much as is kept of it.
pub(super) struct Answer {
    pub(super) status: u16,
    /// `Retry-After` in seconds, if the answer had one.
    pub(super) retry_after: Option<Duration>,
    /// `X-Amzn-ErrorType` up to its first `:`, by which Amazon's APIs name an error.
    pub(super) error_type: Option<String>,
    /// The body, up to [`MOST_BODY`].
    pub(super) body: Vec<u8>,
}

impl std::fmt::Debug for Answer {
    /// Leaves out the body, which may name the recipient or carry the provider's text.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Answer")
            .field("status", &self.status)
            .field("retry_after", &self.retry_after)
            .finish_non_exhaustive()
    }
}

/// Sends `request` and reads the answer.
///
/// # Errors
/// Returns what the attempt became when no answer came: a request that cannot be made
/// is refused, and a connection that fails or times out is worth another attempt.
pub(super) async fn send(request: reqwest::RequestBuilder) -> Result<Answer, Outcome> {
    let mut response = request.send().await.map_err(|error| {
        if error.is_builder() {
            Outcome::Refused { code: None }
        } else {
            Outcome::Later {
                after: None,
                code: None,
            }
        }
    })?;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    // Only a number of seconds is read; a date is left as if none was given.
    let retry_after = header("retry-after")
        .and_then(|text| text.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let error_type = header("x-amzn-errortype")
        .map(|text| text.split(':').next().unwrap_or_default().trim().to_owned());
    let mut body = Vec::new();
    // A body cut off, or longer than is read, still has its status.
    while let Ok(Some(chunk)) = response.chunk().await {
        let room = MOST_BODY.saturating_sub(body.len());
        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if body.len() >= MOST_BODY {
            break;
        }
    }
    Ok(Answer {
        status,
        retry_after,
        error_type,
        body,
    })
}

/// What an answer's status makes of the attempt:
///
/// * `2xx`: the provider took the message.
/// * `401`, `403`: it refuses the server's token, signature or permissions, so the
///   whole way is closed.
/// * `408`, `429`, `5xx`: not now, after `Retry-After` if it said.
/// * any other `4xx`: this message is refused for good (a request it will not take).
/// * `3xx`: the API does not redirect, so whatever answers so is not the API: the way
///   is closed.
pub(super) fn outcome_of(answer: &Answer) -> Outcome {
    let code = Some(answer.status);
    match answer.status {
        200..=299 => Outcome::Sent,
        401 | 403 | 300..=399 => Outcome::PathDown { code },
        408 | 429 | 500..=599 => Outcome::Later {
            after: answer.retry_after,
            code,
        },
        _ => Outcome::Refused { code },
    }
}

/// The host of `url` as a `Host` header names it: with its port when it is not the
/// scheme's own.
///
/// # Errors
/// Returns an error for a URL without a host.
pub(super) fn host_of(url: &reqwest::Url) -> anyhow::Result<String> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("a provider's address without a host"))?;
    Ok(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(status: u16) -> Answer {
        Answer {
            status,
            retry_after: Some(Duration::from_secs(7)),
            error_type: None,
            body: Vec::new(),
        }
    }

    #[test]
    fn a_status_sorts_the_attempt() {
        assert_eq!(outcome_of(&answer(200)), Outcome::Sent);
        assert_eq!(outcome_of(&answer(202)), Outcome::Sent);
        for status in [401, 403, 302] {
            assert_eq!(
                outcome_of(&answer(status)),
                Outcome::PathDown { code: Some(status) },
                "{status}"
            );
        }
        for status in [408, 429, 500, 503] {
            assert_eq!(
                outcome_of(&answer(status)),
                Outcome::Later {
                    after: Some(Duration::from_secs(7)),
                    code: Some(status)
                },
                "{status}"
            );
        }
        for status in [400, 404, 413, 422] {
            assert_eq!(
                outcome_of(&answer(status)),
                Outcome::Refused { code: Some(status) },
                "{status}"
            );
        }
    }

    #[test]
    fn the_host_header_names_a_port_that_is_not_the_schemes() {
        let host = |url: &str| host_of(&reqwest::Url::parse(url).expect("a URL")).expect("host");
        assert_eq!(
            host("https://email.us-east-1.amazonaws.com"),
            "email.us-east-1.amazonaws.com"
        );
        assert_eq!(host("https://example.test:443/x"), "example.test");
        assert_eq!(host("http://127.0.0.1:8025"), "127.0.0.1:8025");
    }
}
