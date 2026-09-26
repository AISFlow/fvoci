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
use openidconnect::http::{
    self, header::ACCEPT, header::AUTHORIZATION, header::CONTENT_TYPE, Method,
};
use openidconnect::{HttpRequest, HttpResponse};
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

    /// The only way an identity-provider request leaves the process: the
    /// `openidconnect` code exchange calls this as its HTTP client, and the
    /// helpers below build on it. A redirect is refused, not followed.
    pub async fn send(&self, request: HttpRequest) -> Result<HttpResponse, FetchError> {
        self.send_within(request, FETCH_TIMEOUT).await
    }

    async fn send_within(
        &self,
        request: HttpRequest,
        budget: Duration,
    ) -> Result<HttpResponse, FetchError> {
        let started = tokio::time::Instant::now();
        let url = self.check_url(&request.uri().to_string())?;
        let (parts, body) = request.into_parts();
        if parts.method != Method::GET && parts.method != Method::POST {
            return Err(FetchError::Url);
        }
        let client = self.client_for(&url, budget).await?;
        let send = client
            .request(parts.method, url)
            .headers(parts.headers)
            .body(body)
            .send();
        let response = tokio::time::timeout(budget.saturating_sub(started.elapsed()), send)
            .await
            .map_err(|_| FetchError::Transport("timeout".into()))?
            .map_err(|err| FetchError::Transport(err.without_url().to_string()))?;
        let status = response.status();
        if status.is_redirection() {
            return Err(FetchError::Status(status.as_u16()));
        }
        let content_type = response.headers().get(CONTENT_TYPE).cloned();
        let body = read_capped(response, started, budget).await?;
        let mut out = http::Response::builder().status(status);
        if let Some(content_type) = content_type {
            out = out.header(CONTENT_TYPE, content_type);
        }
        out.body(body).map_err(|_| FetchError::Body)
    }

    async fn send_json<T: DeserializeOwned>(&self, request: HttpRequest) -> Result<T, FetchError> {
        let response = self.send(request).await?;
        if !response.status().is_success() {
            return Err(FetchError::Status(response.status().as_u16()));
        }
        serde_json::from_slice(response.body()).map_err(|_| FetchError::Body)
    }

    pub async fn get_json<T: DeserializeOwned>(
        &self,
        raw_url: &str,
        bearer: Option<&str>,
    ) -> Result<T, FetchError> {
        let mut request = http::Request::get(raw_url).header(ACCEPT, "application/json");
        if let Some(token) = bearer {
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        self.send_json(request.body(Vec::new()).map_err(|_| FetchError::Url)?)
            .await
    }

    /// POST `application/x-www-form-urlencoded` (Naver's OAuth2 token call;
    /// OIDC token requests are shaped by `openidconnect`).
    pub async fn post_form<T: DeserializeOwned>(
        &self,
        raw_url: &str,
        form: &[(&str, &str)],
    ) -> Result<T, FetchError> {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form.iter().copied())
            .finish();
        let request = http::Request::post(raw_url)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(ACCEPT, "application/json")
            .body(body.into_bytes())
            .map_err(|_| FetchError::Url)?;
        self.send_json(request).await
    }
}

async fn read_capped(
    response: reqwest::Response,
    started: tokio::time::Instant,
    budget: Duration,
) -> Result<Vec<u8>, FetchError> {
    if response
        .content_length()
        .is_some_and(|len| len > MAX_BODY_BYTES as u64)
    {
        return Err(FetchError::TooLarge);
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    loop {
        let remaining = budget.saturating_sub(started.elapsed());
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
    Ok(body)
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

    /// A loopback server that answers every connection with `reply` (after
    /// reading the request head) and then holds the socket open.
    async fn serve(
        reply: &'static [u8],
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = hits.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = socket.read(&mut buf).await;
                    let _ = socket.write_all(reply).await;
                    tokio::time::sleep(Duration::from_secs(30)).await;
                });
            }
        });
        (base, hits)
    }

    fn get(url: &str) -> HttpRequest {
        http::Request::get(url).body(Vec::new()).unwrap()
    }

    #[tokio::test]
    async fn adapter_enforces_the_total_deadline() {
        // No response at all, then headers with a body that never finishes.
        for reply in [
            &b""[..],
            b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\n{\"partial\":",
        ] {
            let (base, _) = serve(reply).await;
            let started = std::time::Instant::now();
            let err = DEV
                .send_within(get(&format!("{base}/x")), Duration::from_millis(300))
                .await
                .unwrap_err();
            assert!(matches!(err, FetchError::Transport(_)), "{err:?}");
            assert!(started.elapsed() < Duration::from_secs(3));
        }
    }

    #[tokio::test]
    async fn adapter_caps_the_body() {
        const DECLARED: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 262145\r\n\r\n";
        let (base, _) = serve(DECLARED).await;
        assert!(matches!(
            DEV.send(get(&format!("{base}/x"))).await,
            Err(FetchError::TooLarge)
        ));
        // Chunked, no length: counted while streaming.
        static CHUNKED: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        let chunked = CHUNKED.get_or_init(|| {
            let mut out = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n".to_vec();
            for _ in 0..5 {
                out.extend_from_slice(b"10000\r\n");
                out.extend(std::iter::repeat_n(b'x', 0x10000));
                out.extend_from_slice(b"\r\n");
            }
            out.extend_from_slice(b"0\r\n\r\n");
            out
        });
        let (base, _) = serve(chunked).await;
        assert!(matches!(
            DEV.send(get(&format!("{base}/x"))).await,
            Err(FetchError::TooLarge)
        ));
        // Within the cap the body, status and content type come back.
        let (base, _) = serve(
            b"HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}",
        )
        .await;
        let response = DEV.send(get(&format!("{base}/x"))).await.unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
        assert_eq!(response.body(), b"{}");
    }

    #[tokio::test]
    async fn adapter_refuses_redirects_methods_and_private_targets() {
        let (target, target_hits) = serve(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}").await;
        let location: &'static str = Box::leak(
            format!("HTTP/1.1 302 Found\r\nlocation: {target}/x\r\ncontent-length: 0\r\n\r\n")
                .into_boxed_str(),
        );
        let (base, _) = serve(location.as_bytes()).await;
        assert!(matches!(
            DEV.send(get(&format!("{base}/x"))).await,
            Err(FetchError::Status(302))
        ));
        assert_eq!(target_hits.load(std::sync::atomic::Ordering::SeqCst), 0);

        let put = http::Request::put(format!("{target}/x"))
            .body(Vec::new())
            .unwrap();
        assert!(matches!(DEV.send(put).await, Err(FetchError::Url)));
        assert_eq!(target_hits.load(std::sync::atomic::Ordering::SeqCst), 0);

        for url in [
            "https://10.0.0.5/x",
            "https://169.254.169.254/x",
            "https://[::1]/x",
        ] {
            assert!(STRICT.send(get(url)).await.is_err(), "{url}");
        }
        assert!(STRICT.send(get(&format!("{target}/x"))).await.is_err());
        assert_eq!(target_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
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
