//! Outbound POST to user-supplied URLs (source `unfurl.ts` `parseHttpUrl` +
//! `fetchOutboundPost`).
//!
//! Rules, in order: http/https only, no userinfo, port absent/80/443, blocked
//! host names (localhost, .local, .internal, .arpa, cloud metadata names),
//! literal and resolved addresses outside private/loopback/link-local/
//! multicast/translation ranges (IPv4 and IPv6, including mapped/compatible/
//! 6to4/NAT64 forms). The host is resolved once and the connection goes to
//! that checked address (Host/SNI stay the original name), so a DNS answer
//! that changes between check and connect cannot redirect the request.
//! Redirects are refused, not followed, so a signature never reaches a
//! `Location`. The response body is read up to [`RESPONSE_READ_CAP`] and
//! dropped.
//!
//! `FVOCI_WEBHOOK_ALLOW_TARGETS` (comma list, default empty) exists for local
//! receivers: a listed URL host skips the port and host-name rules, and a
//! listed IP address is accepted as a resolved or literal target. Nothing else
//! relaxes the rules.

use std::collections::HashSet;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use url::{Host, Url};

pub const URL_MAX: usize = 2048;
pub const RESPONSE_READ_CAP: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OutboundRejected {
    #[error("url is not an allowed http(s) target")]
    Url,
    #[error("host resolves to a disallowed address")]
    Address,
    #[error("host did not resolve")]
    Resolve,
    #[error("redirect refused")]
    Redirect,
}

#[derive(Debug, thiserror::Error)]
pub enum OutboundError {
    #[error(transparent)]
    Rejected(#[from] OutboundRejected),
    /// Transport failure. The message never contains the URL.
    #[error("request failed: {0}")]
    Transport(String),
}

/// Operator allowances; see the module docs.
#[derive(Debug, Clone, Default)]
pub struct OutboundPolicy {
    hosts: HashSet<String>,
    addrs: HashSet<IpAddr>,
}

impl OutboundPolicy {
    pub fn parse_allow_list(raw: &str) -> Result<Self, String> {
        let mut policy = Self::default();
        for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            let bare = entry
                .strip_prefix('[')
                .and_then(|e| e.strip_suffix(']'))
                .unwrap_or(entry);
            if let Ok(ip) = bare.parse::<IpAddr>() {
                // 0.0.0.0 / :: are not a receiver; as a connect target they
                // reach the local host.
                if ip.is_unspecified() {
                    return Err(format!(
                        "invalid FVOCI_WEBHOOK_ALLOW_TARGETS entry (unspecified address): {entry}"
                    ));
                }
                policy.addrs.insert(ip);
                policy.hosts.insert(canonical_ip_host(ip));
            } else if bare
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
                && !bare.is_empty()
            {
                policy.hosts.insert(bare.to_ascii_lowercase());
            } else {
                return Err(format!(
                    "invalid FVOCI_WEBHOOK_ALLOW_TARGETS entry (host name or IP): {entry}"
                ));
            }
        }
        Ok(policy)
    }

    pub fn from_env() -> Result<Self, String> {
        match std::env::var("FVOCI_WEBHOOK_ALLOW_TARGETS") {
            Ok(raw) => Self::parse_allow_list(&raw),
            Err(_) => Ok(Self::default()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty() && self.addrs.is_empty()
    }

    fn host_allowed(&self, host: &str) -> bool {
        self.hosts.contains(host)
    }

    fn addr_allowed(&self, ip: IpAddr) -> bool {
        self.addrs.contains(&ip)
    }
}

fn canonical_ip_host(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{v6}]"),
    }
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
        // IETF protocol assignments and documentation ranges.
        || (a == 192 && b == 0 && (c == 0 || c == 2))
        // Deprecated 6to4 relay anycast.
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224
}

/// Source `isPrivateV6`, plus Teredo (embeds an IPv4 endpoint) and the local
/// NAT64 prefix. Mapped/compatible/translated forms are refused even when the
/// embedded IPv4 is public: public IPv4 targets go out over A records.
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

fn host_name_blocked(host: &str) -> bool {
    let host = host.trim_end_matches('.');
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".arpa")
        || host == "metadata.google.internal"
        || host == "metadata.amazonaws.com"
}

/// Source `parseHttpUrl`. Returns the normalized URL.
pub fn parse_target_url(raw: &str, policy: &OutboundPolicy) -> Result<Url, OutboundRejected> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > URL_MAX {
        return Err(OutboundRejected::Url);
    }
    let url = Url::parse(trimmed).map_err(|_| OutboundRejected::Url)?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(OutboundRejected::Url);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(OutboundRejected::Url);
    }
    let host = url.host().ok_or(OutboundRejected::Url)?;
    let host_key = match &host {
        Host::Domain(name) => name.to_ascii_lowercase(),
        Host::Ipv4(v4) => v4.to_string(),
        Host::Ipv6(v6) => format!("[{v6}]"),
    };
    let listed = policy.host_allowed(&host_key);
    if !listed && !matches!(url.port(), None | Some(80) | Some(443)) {
        return Err(OutboundRejected::Url);
    }
    match host {
        Host::Domain(name) => {
            if !listed && host_name_blocked(&name.to_ascii_lowercase()) {
                return Err(OutboundRejected::Url);
            }
        }
        Host::Ipv4(v4) => check_addr(IpAddr::V4(v4), policy).map_err(|_| OutboundRejected::Url)?,
        Host::Ipv6(v6) => check_addr(IpAddr::V6(v6), policy).map_err(|_| OutboundRejected::Url)?,
    }
    Ok(url)
}

fn check_addr(ip: IpAddr, policy: &OutboundPolicy) -> Result<(), OutboundRejected> {
    if is_private_ip(ip) && !policy.addr_allowed(ip) {
        return Err(OutboundRejected::Address);
    }
    Ok(())
}

pub type ResolveFuture<'a> =
    Pin<Box<dyn Future<Output = std::io::Result<Vec<IpAddr>>> + Send + 'a>>;

/// Name resolution used before connecting. The product uses [`SystemResolver`].
pub trait Resolve: Send + Sync {
    fn lookup<'a>(&'a self, host: &'a str) -> ResolveFuture<'a>;
}

pub struct SystemResolver;

impl Resolve for SystemResolver {
    fn lookup<'a>(&'a self, host: &'a str) -> ResolveFuture<'a> {
        Box::pin(async move {
            let addrs = tokio::net::lookup_host((host, 0u16)).await?;
            Ok(addrs.map(|addr| addr.ip()).collect())
        })
    }
}

#[derive(Clone)]
pub struct Outbound {
    policy: Arc<OutboundPolicy>,
    resolver: Arc<dyn Resolve>,
}

impl Outbound {
    pub fn new(policy: OutboundPolicy, resolver: Arc<dyn Resolve>) -> Self {
        Self {
            policy: Arc::new(policy),
            resolver,
        }
    }

    pub fn system(policy: OutboundPolicy) -> Self {
        Self::new(policy, Arc::new(SystemResolver))
    }

    pub fn policy(&self) -> &OutboundPolicy {
        &self.policy
    }

    /// Resolves the URL host once and returns the address to connect to.
    /// Every answer must pass; the first IPv4 answer is preferred (source).
    pub async fn pin(&self, url: &Url) -> Result<SocketAddr, OutboundRejected> {
        let port = url.port_or_known_default().ok_or(OutboundRejected::Url)?;
        let ip = match url.host().ok_or(OutboundRejected::Url)? {
            Host::Ipv4(v4) => IpAddr::V4(v4),
            Host::Ipv6(v6) => IpAddr::V6(v6),
            Host::Domain(name) => {
                let answers = self
                    .resolver
                    .lookup(name)
                    .await
                    .map_err(|_| OutboundRejected::Resolve)?;
                if answers.is_empty() {
                    return Err(OutboundRejected::Resolve);
                }
                for ip in &answers {
                    check_addr(*ip, &self.policy)?;
                }
                answers
                    .iter()
                    .copied()
                    .find(IpAddr::is_ipv4)
                    .unwrap_or(answers[0])
            }
        };
        check_addr(ip, &self.policy)?;
        Ok(SocketAddr::new(ip, port))
    }

    /// POST `body` to `url` after re-validating it, connecting only to the
    /// checked address. Returns the HTTP status. `timeout` bounds name
    /// resolution, connect, request and the capped body read together.
    pub async fn post(
        &self,
        raw_url: &str,
        headers: &[(&str, String)],
        body: Vec<u8>,
        timeout: Duration,
    ) -> Result<u16, OutboundError> {
        let url = parse_target_url(raw_url, &self.policy)?;
        let started = tokio::time::Instant::now();
        // The reqwest timeout does not cover this lookup.
        let pinned = tokio::time::timeout(timeout, self.pin(&url))
            .await
            .map_err(|_| OutboundError::Transport("resolve timeout".into()))??;
        let remaining = timeout
            .saturating_sub(started.elapsed())
            .max(Duration::from_millis(1));
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(remaining)
            .connect_timeout(remaining)
            .pool_max_idle_per_host(0);
        if let Some(Host::Domain(name)) = url.host() {
            builder = builder.resolve(name, pinned);
        }
        let client = builder
            .build()
            .map_err(|err| OutboundError::Transport(err.without_url().to_string()))?;
        let mut request = client.post(url).body(body);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        let response = request
            .send()
            .await
            .map_err(|err| OutboundError::Transport(transport_kind(&err)))?;
        let status = response.status();
        if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
            return Err(OutboundRejected::Redirect.into());
        }
        let mut read = 0usize;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| OutboundError::Transport(transport_kind(&err)))?;
            read += chunk.len();
            if read > RESPONSE_READ_CAP {
                break;
            }
        }
        Ok(status.as_u16())
    }
}

fn transport_kind(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        "timeout".into()
    } else if err.is_connect() {
        "connect".into()
    } else if err.is_body() || err.is_decode() {
        "body".into()
    } else {
        "request".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> OutboundPolicy {
        OutboundPolicy::default()
    }

    #[test]
    fn url_rules_follow_source_parse_http_url() {
        for bad in [
            "ftp://example.com/",
            "http://user:pw@example.com/",
            "http://example.com:8080/",
            "http://localhost/",
            "http://LOCALHOST./",
            "http://a.localhost/",
            "http://printer.local/",
            "http://metadata.google.internal/",
            "http://metadata.amazonaws.com/",
            "http://1.0.0.127.in-addr.arpa/",
            "http://127.0.0.1/",
            "http://2130706433/",
            "http://0x7f.1/",
            "http://10.1.2.3/",
            "http://169.254.169.254/latest/meta-data/",
            "http://100.64.0.1/",
            "http://[::1]/",
            "http://[::]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:8.8.8.8]/",
            "http://[fe80::1]/",
            "http://[fd00::1]/",
            "http://[ff02::1]/",
            "http://[2002:7f00:1::]/",
            "http://[64:ff9b::a9fe:a9fe]/",
            "http://0.0.0.0/",
            "http://224.0.0.1/",
            "http://192.88.99.1/",
            "",
        ] {
            assert!(
                parse_target_url(bad, &none()).is_err(),
                "{bad} must be refused"
            );
        }
        for good in [
            "https://example.com/hook",
            "http://example.com:80/x?y=1",
            "https://example.com:443/",
            "http://8.8.8.8/",
            "http://[2606:4700::1111]/",
        ] {
            assert!(parse_target_url(good, &none()).is_ok(), "{good} must pass");
        }
        let long = format!("https://example.com/{}", "a".repeat(URL_MAX));
        assert!(parse_target_url(&long, &none()).is_err());
    }

    #[test]
    fn allow_list_is_exact() {
        let policy = OutboundPolicy::parse_allow_list("127.0.0.1, hook.test").expect("policy");
        assert!(parse_target_url("http://127.0.0.1:5555/", &policy).is_ok());
        assert!(parse_target_url("http://127.0.0.2:5555/", &policy).is_err());
        assert!(parse_target_url("http://hook.test:5555/", &policy).is_ok());
        assert!(parse_target_url("http://other.test:5555/", &policy).is_err());
        assert!(parse_target_url("http://localhost:5555/", &policy).is_err());
        assert!(OutboundPolicy::parse_allow_list("a b").is_err());
        for unspecified in ["0.0.0.0", "::", "[::]", "127.0.0.1,0.0.0.0"] {
            assert!(
                OutboundPolicy::parse_allow_list(unspecified).is_err(),
                "{unspecified}"
            );
        }
    }

    struct Fixed(Vec<IpAddr>);

    impl Resolve for Fixed {
        fn lookup<'a>(&'a self, _host: &'a str) -> ResolveFuture<'a> {
            let answers = self.0.clone();
            Box::pin(async move { Ok(answers) })
        }
    }

    #[tokio::test]
    async fn pin_refuses_any_private_answer_and_prefers_ipv4() {
        let url = Url::parse("https://example.com/").expect("url");
        let public_v4: IpAddr = "93.184.216.34".parse().expect("ip");
        let public_v6: IpAddr = "2606:2800:220:1::".parse().expect("ip");
        let pinned = Outbound::new(none(), Arc::new(Fixed(vec![public_v6, public_v4])))
            .pin(&url)
            .await
            .expect("pin");
        assert_eq!(pinned, SocketAddr::new(public_v4, 443));
        for private in ["127.0.0.1", "169.254.169.254", "::1", "::ffff:10.0.0.1"] {
            let outbound = Outbound::new(
                none(),
                Arc::new(Fixed(vec![public_v4, private.parse().expect("ip")])),
            );
            assert_eq!(
                outbound.pin(&url).await,
                Err(OutboundRejected::Address),
                "{private}"
            );
        }
        let empty = Outbound::new(none(), Arc::new(Fixed(vec![])));
        assert_eq!(empty.pin(&url).await, Err(OutboundRejected::Resolve));
    }

    struct Hang;

    impl Resolve for Hang {
        fn lookup<'a>(&'a self, _host: &'a str) -> ResolveFuture<'a> {
            Box::pin(std::future::pending())
        }
    }

    #[tokio::test]
    async fn post_bounds_a_lookup_that_never_answers() {
        let outbound = Outbound::new(none(), Arc::new(Hang));
        let started = std::time::Instant::now();
        let result = outbound
            .post(
                "https://example.com/",
                &[],
                Vec::new(),
                Duration::from_millis(200),
            )
            .await;
        assert!(
            matches!(&result, Err(OutboundError::Transport(kind)) if kind == "resolve timeout"),
            "{result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
