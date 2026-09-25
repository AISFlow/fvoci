//! Outbound HTTP to identity providers: discovery, JWKS, token and profile.
//!
//! Issuers come from the operator (instance providers) and from workspace
//! admins (workspace SSO), so every request is SSRF-guarded: https only,
//! no credentials in the URL, every resolved address public, the connection
//! pinned to the checked address, no redirects, no proxy, a total deadline
//! and a capped body. `OIDC_ALLOW_INSECURE` admits plain http to loopback
//! only (local development and tests). The address rules follow the
//! integrations branch's outbound policy (source `isPrivateV4/V6`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use url::{Host, Url};

pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Discovery documents, JWKS and token responses are small.
pub const MAX_BODY_BYTES: usize = 256 * 1024;
const URL_MAX: usize = 2048;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("url is not allowed")]
    Url,
    #[error("address is not allowed")]
    Address,
    #[error("name resolution failed")]
    Resolve,
    #[error("transport: {0}")]
    Transport(String),
    #[error("http status {0}")]
    Status(u16),
    #[error("response too large")]
    TooLarge,
    #[error("malformed response")]
    Body,
}

#[derive(Debug, Clone, Copy)]
pub struct FetchPolicy {
    pub allow_insecure_loopback: bool,
}

pub fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    a == 0
        || a == 10
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 100 && (64..=127).contains(&b))
        || (a == 198 && (b == 18 || b == 19))
        || (a == 192 && b == 0 && (c == 0 || c == 2))
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224
}

pub fn is_private_ipv6(ip: Ipv6Addr) -> bool {
    let n = u128::from(ip);
    let prefix = |bits: u32| n >> (128 - bits);
    prefix(96) == 0
        || prefix(10) == 0x3fa
        || prefix(7) == 0x7e
        || prefix(10) == 0x3fb
        || prefix(8) == 0xff
        || prefix(16) == 0x2002
        || prefix(96) == ((0x64u128 << 80) | (0xff9bu128 << 64))
        || prefix(48) == 0x0064_ff9b_0001
        || prefix(96) == 0xffff
        || prefix(96) == 0xffff_0000
        || prefix(32) == 0x2001_0000
        || prefix(32) == 0x2001_0db8
}

pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_ipv4(v4),
        IpAddr::V6(v6) => is_private_ipv6(v6),
    }
}

fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

fn host_name_blocked(host: &str) -> bool {
    let host = host.trim_end_matches('.');
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".arpa")
}

impl FetchPolicy {
    /// URL shape check (no resolution): scheme, credentials, host names.
    pub fn check_url(&self, raw: &str) -> Result<Url, FetchError> {
        if raw.is_empty() || raw.len() > URL_MAX {
            return Err(FetchError::Url);
        }
        let url = Url::parse(raw).map_err(|_| FetchError::Url)?;
        if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
            return Err(FetchError::Url);
        }
        let host = url.host().ok_or(FetchError::Url)?;
        match url.scheme() {
            "https" => {}
            "http" if self.allow_insecure_loopback && self.host_is_loopback(&host) => {}
            _ => return Err(FetchError::Url),
        }
        match host {
            Host::Domain(name) => {
                let name = name.to_ascii_lowercase();
                if host_name_blocked(&name)
                    && !(self.allow_insecure_loopback && name == "localhost")
                {
                    return Err(FetchError::Url);
                }
            }
            Host::Ipv4(v4) => self.check_addr(IpAddr::V4(v4))?,
            Host::Ipv6(v6) => self.check_addr(IpAddr::V6(v6))?,
        }
        Ok(url)
    }

    fn host_is_loopback(&self, host: &Host<&str>) -> bool {
        match host {
            Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
            Host::Ipv4(v4) => v4.is_loopback(),
            Host::Ipv6(v6) => v6.is_loopback(),
        }
    }

    fn check_addr(&self, ip: IpAddr) -> Result<(), FetchError> {
        if self.allow_insecure_loopback && is_loopback(ip) {
            return Ok(());
        }
        if is_private_ip(ip) {
            return Err(FetchError::Address);
        }
        Ok(())
    }

    /// Resolves once; every answer must pass. Returns the address to pin.
    async fn pin(&self, url: &Url) -> Result<SocketAddr, FetchError> {
        let port = url.port_or_known_default().ok_or(FetchError::Url)?;
        let ip = match url.host().ok_or(FetchError::Url)? {
            Host::Ipv4(v4) => IpAddr::V4(v4),
            Host::Ipv6(v6) => IpAddr::V6(v6),
            Host::Domain(name) => {
                let answers: Vec<IpAddr> = tokio::net::lookup_host((name, port))
                    .await
                    .map_err(|_| FetchError::Resolve)?
                    .map(|a| a.ip())
                    .collect();
                if answers.is_empty() {
                    return Err(FetchError::Resolve);
                }
                for ip in &answers {
                    self.check_addr(*ip)?;
                }
                answers
                    .iter()
                    .copied()
                    .find(IpAddr::is_ipv4)
                    .unwrap_or(answers[0])
            }
        };
        self.check_addr(ip)?;
        Ok(SocketAddr::new(ip, port))
    }

    async fn client_for(
        &self,
        url: &Url,
        remaining: Duration,
    ) -> Result<reqwest::Client, FetchError> {
        let pinned = tokio::time::timeout(remaining, self.pin(url))
            .await
            .map_err(|_| FetchError::Transport("resolve timeout".into()))??;
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(remaining)
            .connect_timeout(remaining)
            .pool_max_idle_per_host(0);
        if let Some(Host::Domain(name)) = url.host() {
            builder = builder.resolve(name, pinned);
        }
        builder
            .build()
            .map_err(|err| FetchError::Transport(err.without_url().to_string()))
    }

    pub async fn get_json<T: DeserializeOwned>(
        &self,
        raw_url: &str,
        bearer: Option<&str>,
    ) -> Result<T, FetchError> {
        let url = self.check_url(raw_url)?;
        let started = tokio::time::Instant::now();
        let client = self.client_for(&url, FETCH_TIMEOUT).await?;
        let mut request = client.get(url).header("accept", "application/json");
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        let response = tokio::time::timeout(
            FETCH_TIMEOUT.saturating_sub(started.elapsed()),
            request.send(),
        )
        .await
        .map_err(|_| FetchError::Transport("timeout".into()))?
        .map_err(|err| FetchError::Transport(err.without_url().to_string()))?;
        read_json(response, started).await
    }

    /// POST `application/x-www-form-urlencoded`, optionally with HTTP Basic
    /// client authentication.
    pub async fn post_form<T: DeserializeOwned>(
        &self,
        raw_url: &str,
        form: &[(&str, &str)],
        basic: Option<(&str, &str)>,
    ) -> Result<T, FetchError> {
        let url = self.check_url(raw_url)?;
        let started = tokio::time::Instant::now();
        let client = self.client_for(&url, FETCH_TIMEOUT).await?;
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form.iter().copied())
            .finish();
        let mut request = client
            .post(url)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("accept", "application/json")
            .body(body);
        if let Some((user, pass)) = basic {
            // RFC 6749 2.3.1: form-urlencode both parts before Basic.
            let enc =
                |v: &str| url::form_urlencoded::byte_serialize(v.as_bytes()).collect::<String>();
            request = request.basic_auth(enc(user), Some(enc(pass)));
        }
        let response = tokio::time::timeout(
            FETCH_TIMEOUT.saturating_sub(started.elapsed()),
            request.send(),
        )
        .await
        .map_err(|_| FetchError::Transport("timeout".into()))?
        .map_err(|err| FetchError::Transport(err.without_url().to_string()))?;
        read_json(response, started).await
    }
}

async fn read_json<T: DeserializeOwned>(
    response: reqwest::Response,
    started: tokio::time::Instant,
) -> Result<T, FetchError> {
    let status = response.status();
    if !status.is_success() {
        return Err(FetchError::Status(status.as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|len| len > MAX_BODY_BYTES as u64)
    {
        return Err(FetchError::TooLarge);
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    loop {
        let remaining = FETCH_TIMEOUT.saturating_sub(started.elapsed());
        let next = tokio::time::timeout(remaining, stream.next())
            .await
            .map_err(|_| FetchError::Transport("timeout".into()))?;
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|err| FetchError::Transport(err.without_url().to_string()))?;
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(FetchError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| FetchError::Body)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRICT: FetchPolicy = FetchPolicy {
        allow_insecure_loopback: false,
    };
    const DEV: FetchPolicy = FetchPolicy {
        allow_insecure_loopback: true,
    };

    #[test]
    fn strict_policy_requires_public_https() {
        assert!(STRICT
            .check_url("https://accounts.google.com/.well-known/openid-configuration")
            .is_ok());
        for bad in [
            "http://accounts.google.com",
            "https://user:pw@idp.example.com",
            "https://127.0.0.1/x",
            "https://10.0.0.5/x",
            "https://169.254.169.254/latest",
            "https://[::1]/x",
            "https://[::ffff:127.0.0.1]/x",
            "https://localhost/x",
            "https://idp.internal/x",
            "http://127.0.0.1:8080/x",
            "ftp://idp.example.com",
            "https://idp.example.com/#frag",
        ] {
            assert!(STRICT.check_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn insecure_mode_admits_loopback_http_only() {
        assert!(DEV.check_url("http://127.0.0.1:8080/x").is_ok());
        assert!(DEV.check_url("http://localhost:8080/x").is_ok());
        assert!(DEV.check_url("http://[::1]:8080/x").is_ok());
        assert!(DEV.check_url("http://10.0.0.5/x").is_err());
        assert!(DEV.check_url("http://idp.example.com/x").is_err());
        assert!(DEV.check_url("https://192.168.1.1/x").is_err());
    }

    #[tokio::test]
    async fn resolution_rejects_private_answers() {
        // "localhost" resolves to loopback: refused unless insecure mode.
        let url = Url::parse("https://localhost:9/x").unwrap();
        assert!(matches!(
            STRICT.pin(&url).await,
            Err(FetchError::Address) | Err(FetchError::Resolve)
        ));
    }
}
