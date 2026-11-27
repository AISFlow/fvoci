//! Webhooks: every inserted or changed block is posted, as JSON, to the addresses in the
//! `block_change_webhooks` setting.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use fvoci_model::Block;
use reqwest::Url;

use crate::auth::oidc::fetch::{Addresses, CONNECT_TIMEOUT, CheckedResolver, is_refused_address};

/// How long one webhook request may take, so an unresponsive receiver cannot stall shutdown.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Posts block changes to the configured addresses.
#[derive(Debug, Clone)]
pub struct Webhook {
    urls: Vec<String>,
    addresses: Addresses,
    /// `None` if there are no addresses: no client is needed then, and building one
    /// can fail (for example on a system without certificate authorities).
    http: Option<reqwest::Client>,
}

impl Webhook {
    /// Creates a client that notifies the given public `https` addresses (`H-1`).
    ///
    /// # Errors
    ///
    /// Returns an error if there are addresses but the HTTP client for them cannot be
    /// built, which means the TLS backend cannot be initialized.
    pub fn new(urls: Vec<String>) -> Result<Self, reqwest::Error> {
        Self::with_addresses(urls, Addresses::PublicOnly)
    }

    /// Creates a webhook client with an explicit address policy (`Addresses::Any` is
    /// reserved for local test receivers).
    pub(crate) fn with_addresses(
        urls: Vec<String>,
        addresses: Addresses,
    ) -> Result<Self, reqwest::Error> {
        let http = if urls.is_empty() {
            None
        } else {
            Some(
                reqwest::Client::builder()
                    .https_only(addresses == Addresses::PublicOnly)
                    .redirect(reqwest::redirect::Policy::none())
                    .no_proxy()
                    .timeout(REQUEST_TIMEOUT)
                    .connect_timeout(CONNECT_TIMEOUT)
                    .dns_resolver(Arc::new(CheckedResolver::new(addresses)))
                    .build()?,
            )
        };
        Ok(Self {
            urls,
            addresses,
            http,
        })
    }

    /// Creates a webhook client that permits loopback HTTP receivers in integration tests.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be initialized.
    #[doc(hidden)]
    pub fn for_testing(urls: Vec<String>) -> Result<Self, reqwest::Error> {
        Self::with_addresses(urls, Addresses::Any)
    }

    fn check_url(&self, raw: &str) -> Result<(Url, String), &'static str> {
        let url = Url::parse(raw).map_err(|_| "the webhook URL is invalid")?;
        match (self.addresses, url.scheme()) {
            (_, "https") | (Addresses::Any, "http") => {}
            (Addresses::PublicOnly, _) => return Err("the webhook URL is not https"),
            (Addresses::Any, _) => return Err("the webhook URL is not http or https"),
        }
        let host = url.host_str().ok_or("the webhook URL has no host")?;
        let literal = host
            .strip_prefix('[')
            .and_then(|inner| inner.strip_suffix(']'))
            .unwrap_or(host)
            .parse::<IpAddr>()
            .ok();
        if let Some(ip) = literal
            && !self.addresses.allows(ip)
        {
            return Err("the webhook host is not at a public address");
        }
        let host_label = host.to_owned();
        Ok((url, host_label))
    }

    /// Posts the block to every address, one after the other. A request that fails is
    /// logged and does not stop delivery to the remaining addresses.
    pub async fn notify_update(&self, block: &Block) {
        let Some(http) = &self.http else {
            return;
        };
        for raw_url in &self.urls {
            // Webhook addresses often carry a secret in their path or query; the log
            // names the host only.
            let (url, host) = match self.check_url(raw_url) {
                Ok(checked) => checked,
                Err(reason) => {
                    let host = Url::parse(raw_url)
                        .ok()
                        .and_then(|parsed| parsed.host_str().map(str::to_owned))
                        .unwrap_or_default();
                    tracing::warn!(host, reason, "webhook.NotifyUpdate refused");
                    continue;
                }
            };
            match http.post(url).json(block).send().await {
                Ok(response) => {
                    let status = response.status();
                    // Drain at most 64 KiB so short acknowledgements can reuse the
                    // connection without buffering unbounded response bodies; redirects
                    // are never followed.
                    if status.is_redirection() {
                        tracing::warn!(
                            host,
                            status = status.as_u16(),
                            "webhook.NotifyUpdate refused redirect"
                        );
                        continue;
                    }
                    drain_bounded(response).await;
                    tracing::debug!(host, "webhook.NotifyUpdate");
                }
                Err(error) => {
                    if is_refused_address(&error) {
                        tracing::warn!(
                            host,
                            "webhook.NotifyUpdate refused non-public resolved address"
                        );
                    } else {
                        let error = error.without_url();
                        tracing::warn!(host, %error, "webhook.NotifyUpdate failed");
                    }
                }
            }
        }
    }
}

async fn drain_bounded(mut response: reqwest::Response) {
    const MAX_DRAIN_BYTES: usize = 64 * 1024;
    let mut drained = 0usize;
    while let Ok(Some(chunk)) = response.chunk().await {
        drained = drained.saturating_add(chunk.len());
        if drained >= MAX_DRAIN_BYTES {
            break;
        }
    }
}
