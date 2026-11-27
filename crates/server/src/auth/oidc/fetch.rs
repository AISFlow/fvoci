//! The requests the server sends to the providers: discovery, tokens, keys.
//!
//! An organization's administrator names the provider, so the address is not the
//! server's own choice: it is held to what may be reached from outside. Every request
//!
//! * goes to an `https` address;
//! * goes to a host whose addresses, as the name is resolved, are all public (no
//!   loopback, private, link-local, shared, documentation or reserved address, and none
//!   of these written as an IPv4-mapped IPv6 address), unless
//!   `oidc_allow_private_addresses` lets it go anywhere;
//! * connects to the addresses that were checked, not to those of a second
//!   resolution: the resolver that checks them is the one the client connects with;
//! * does not follow a redirect, and takes no proxy;
//! * takes at most [`MAX_RESPONSE_BYTES`] of answer, within [`REQUEST_TIMEOUT`].

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use reqwest::Url;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use serde::de::DeserializeOwned;
use thiserror::Error;

/// How long a request to a provider may take, from connecting to the last byte.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How long connecting to a provider may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The most bytes of an answer that are read. A discovery document or a set of keys is
/// a few kilobytes.
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// Why a request to a provider gave nothing to go on. What it says is the server's,
/// never the provider's: the provider's answer is not repeated.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FetchError {
    /// The address is no `https` address.
    #[error("the address is not https")]
    NotHttps,
    /// The host is, or resolves to, an address that is not public.
    #[error("the host is not at a public address")]
    NotPublic,
    /// The host could not be reached, or did not answer in time.
    #[error("the host could not be reached")]
    Unreachable,
    /// The answer has a status other than 2xx; a redirect is one of these.
    #[error("the answer has the status {0}")]
    Status(u16),
    /// The answer is larger than the server reads.
    #[error("the answer is larger than {MAX_RESPONSE_BYTES} bytes")]
    TooLarge,
    /// The answer is not the JSON that was asked for.
    #[error("the answer is not the JSON that was asked for")]
    NotJson,
}

/// The client that talks to providers.
#[derive(Debug, Clone)]
pub struct Fetcher {
    http: reqwest::Client,
    addresses: Addresses,
}

/// Which addresses a provider may be at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Addresses {
    /// Public addresses only.
    PublicOnly,
    /// Any address (`oidc_allow_private_addresses`).
    Any,
}

impl Fetcher {
    /// A client for providers at `addresses`, which trusts the system's certificate
    /// authorities and `extra_roots` (DER).
    ///
    /// # Errors
    ///
    /// Returns an error if the TLS backend cannot be set up or a certificate is none.
    pub fn new(addresses: Addresses, extra_roots: &[Vec<u8>]) -> Result<Self, reqwest::Error> {
        let roots = extra_roots
            .iter()
            .map(|der| reqwest::Certificate::from_der(der))
            .collect::<Result<Vec<_>, _>>()?;
        let http = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .dns_resolver(Arc::new(CheckedResolver { addresses }))
            .tls_certs_merge(roots)
            .build()?;
        Ok(Self { http, addresses })
    }

    /// Reads the JSON at `url` with a `GET`.
    ///
    /// # Errors
    ///
    /// Returns why the answer gives nothing to go on.
    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T, FetchError> {
        let url = self.check(url)?;
        self.json(self.http.get(url)).await
    }

    /// Posts a form to `url` and reads the JSON of the answer. `basic` is the user and
    /// the password of HTTP Basic authentication, if any.
    ///
    /// # Errors
    ///
    /// Returns why the answer gives nothing to go on.
    pub async fn post_form<T: DeserializeOwned>(
        &self,
        url: &str,
        form: &[(&str, &str)],
        basic: Option<(&str, &str)>,
    ) -> Result<T, FetchError> {
        let url = self.check(url)?;
        let mut request = self.http.post(url).form(form);
        if let Some((user, password)) = basic {
            request = request.basic_auth(user, Some(password));
        }
        self.json(request).await
    }

    /// Checks what can be checked of an address before it is resolved: that it is
    /// `https`, and that a host that is an address is a public one.
    fn check(&self, url: &str) -> Result<Url, FetchError> {
        let url = Url::parse(url).map_err(|_| FetchError::NotHttps)?;
        if url.scheme() != "https" {
            return Err(FetchError::NotHttps);
        }
        // An address in the URL is connected to as it is, without the resolver. The
        // parser has written it in its plain form: `0x7f.1` is `127.0.0.1` by now, and
        // an IPv6 address is in brackets.
        let host = url.host_str().ok_or(FetchError::NotHttps)?;
        let literal = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host)
            .parse::<IpAddr>()
            .ok();
        if let Some(address) = literal
            && !self.addresses.allows(address)
        {
            return Err(FetchError::NotPublic);
        }
        Ok(url)
    }

    async fn json<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, FetchError> {
        let mut response = request.send().await.map_err(|error| {
            if is_refused_address(&error) {
                FetchError::NotPublic
            } else {
                // What failed (TLS, a refused connection, the time): for the operator.
                // The request's form is not in it.
                tracing::debug!(error = %error.without_url(), "a provider could not be reached");
                FetchError::Unreachable
            }
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(FetchError::Status(status.as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > u64::try_from(MAX_RESPONSE_BYTES).unwrap_or(u64::MAX))
        {
            return Err(FetchError::TooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            tracing::debug!(error = %error.without_url(), "a provider's answer broke off");
            FetchError::Unreachable
        })? {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(FetchError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| FetchError::NotJson)
    }
}

/// Whether a failed request failed because the resolver refused the host's addresses.
pub(crate) fn is_refused_address(error: &reqwest::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = source {
        if current.downcast_ref::<RefusedAddress>().is_some() {
            return true;
        }
        source = current.source();
    }
    false
}

impl Addresses {
    /// Whether an outbound target may be at `address`.
    pub(crate) fn allows(self, address: IpAddr) -> bool {
        match self {
            Self::Any => true,
            Self::PublicOnly => is_public(address),
        }
    }
}

/// The resolver of outbound clients: the system's, whose answer is refused whole if one
/// of its addresses is not allowed.
#[derive(Debug)]
pub(crate) struct CheckedResolver {
    addresses: Addresses,
}

impl CheckedResolver {
    /// Creates a DNS resolver enforcing `addresses`.
    #[must_use]
    pub(crate) const fn new(addresses: Addresses) -> Self {
        Self { addresses }
    }
}

/// The resolver refused a host for its addresses.
#[derive(Debug, Error)]
#[error("the host resolves to an address that is not allowed")]
pub(crate) struct RefusedAddress;

impl Resolve for CheckedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let addresses = self.addresses;
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let found: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            // A name with one address that may not be reached is refused whole: it is
            // not for the client to pick the harmless ones and be turned away to the
            // others by a second answer.
            if found.is_empty() || !found.iter().all(|found| addresses.allows(found.ip())) {
                let refused: Box<dyn std::error::Error + Send + Sync> = Box::new(RefusedAddress);
                return Err(refused);
            }
            let found: Addrs = Box::new(found.into_iter());
            Ok(found)
        })
    }
}

/// Whether an address is public: one that a host on the internet has. IPv4-mapped IPv6
/// addresses (`::ffff:a.b.c.d`) are unwrapped and judged by the IPv4 rules; other IPv6
/// addresses that embed or translate IPv4 (compatible, NAT64, 6to4, Teredo) are refused
/// in `is_public_v6` because what they reach is not plain from the IPv6 address alone.
pub fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_v4(address),
        IpAddr::V6(address) => match address.to_ipv4_mapped() {
            Some(mapped) => is_public_v4(mapped),
            None => is_public_v6(address),
        },
    }
}

fn is_public_v4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    let special = address.is_unspecified()
        || address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_multicast()
        || address.is_broadcast()
        || address.is_documentation()
        || a == 0
        // Shared address space (RFC 6598).
        || (a == 100 && (64..128).contains(&b))
        // IETF protocol assignments (RFC 6890).
        || (a == 192 && b == 0 && c == 0)
        // Benchmarking (RFC 2544).
        || (a == 198 && (b == 18 || b == 19))
        // The relays of 6to4 (RFC 7526).
        || (a == 192 && b == 88 && c == 99)
        // Reserved, 240.0.0.0/4.
        || a >= 240;
    !special
}

fn is_public_v6(address: Ipv6Addr) -> bool {
    let segments = address.segments();
    // Global unicast is 2000::/3; everything else (loopback, unspecified, unique local,
    // link-local, multicast, IPv4-compatible, NAT64 64:ff9b::/96) is not public.
    if segments[0] & 0xe000 != 0x2000 {
        return false;
    }
    let special =
        // IETF protocol assignments, 2001::/23 (Teredo, benchmarking, ORCHID), and
        // documentation, 2001:db8::/32 and 3fff::/20.
        (segments[0] == 0x2001 && (segments[1] < 0x0200 || segments[1] == 0x0db8))
        || (segments[0] == 0x3fff && segments[1] < 0x1000)
        // 6to4, 2002::/16, which carries an IPv4 address.
        || segments[0] == 0x2002;
    !special
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public(text: &str) -> bool {
        is_public(text.parse().expect("an address"))
    }

    #[test]
    fn only_addresses_of_hosts_on_the_internet_are_public() {
        for yes in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "2606:4700:4700::1111",
            "2a00:1450:4001::200e",
            "::ffff:8.8.8.8",
        ] {
            assert!(public(yes), "{yes}");
        }
        for no in [
            "0.0.0.0",
            "0.1.2.3",
            "127.0.0.1",
            "127.255.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "192.0.0.8",
            "192.0.2.1",
            "192.88.99.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "::127.0.0.1",
            "64:ff9b::a00:1",
            "2001::1",
            "2001:db8::1",
            "2002:a00:1::1",
            "3fff::1",
        ] {
            assert!(!public(no), "{no}");
        }
    }

    fn fetcher(addresses: Addresses) -> Fetcher {
        Fetcher::new(addresses, &[]).expect("a client")
    }

    #[test]
    fn an_address_that_is_not_https_or_not_public_is_refused_before_it_is_asked() {
        let public_only = fetcher(Addresses::PublicOnly);
        for (url, refusal) in [
            ("http://example.com/x", FetchError::NotHttps),
            ("ftp://example.com/x", FetchError::NotHttps),
            ("not a url", FetchError::NotHttps),
            ("https://127.0.0.1/x", FetchError::NotPublic),
            ("https://0x7f.1/x", FetchError::NotPublic),
            ("https://2130706433/x", FetchError::NotPublic),
            ("https://[::1]/x", FetchError::NotPublic),
            ("https://[::ffff:127.0.0.1]/x", FetchError::NotPublic),
            ("https://[::ffff:7f00:1]/x", FetchError::NotPublic),
            ("https://169.254.169.254/latest", FetchError::NotPublic),
        ] {
            assert_eq!(public_only.check(url).err(), Some(refusal), "{url}");
        }
        let any = fetcher(Addresses::Any);
        assert!(any.check("https://127.0.0.1/x").is_ok());
        assert_eq!(
            any.check("http://127.0.0.1/x").err(),
            Some(FetchError::NotHttps),
            "https whatever the address"
        );
    }

    #[tokio::test]
    async fn a_name_that_resolves_to_an_address_that_is_not_public_is_refused() {
        let resolver = CheckedResolver {
            addresses: Addresses::PublicOnly,
        };
        let refused = resolver
            .resolve("localhost".parse().expect("a name"))
            .await
            .err()
            .expect("localhost is refused");
        assert!(
            refused.downcast_ref::<RefusedAddress>().is_some(),
            "{refused}"
        );

        let any = CheckedResolver {
            addresses: Addresses::Any,
        };
        let found: Vec<SocketAddr> = any
            .resolve("localhost".parse().expect("a name"))
            .await
            .map_err(|error| error.to_string())
            .expect("localhost resolves")
            .collect();
        assert!(
            found.iter().all(|address| address.ip().is_loopback()),
            "{found:?}"
        );

        let error = fetcher(Addresses::PublicOnly)
            .get_json::<serde_json::Value>("https://localhost:9/x")
            .await
            .expect_err("refused");
        assert_eq!(error, FetchError::NotPublic);
    }
}
