//! Text embeddings for semantic search (source `packages/jobs/src/embed.ts` and
//! the `FVOCI_AI_*` config contract at SHA
//! `393795261322b916e588043cf94feca999175843`).
//!
//! One OpenAI-compatible `POST {base}/embeddings` call (BYOK or a local model
//! server); no in-process model. Enabled only when `FVOCI_AI_ENABLED=1` and
//! `FVOCI_AI_EMBEDDINGS_BASE_URL` is set. `FVOCI_AI_SECRET` (optional) goes out
//! only as the bearer header; it never appears in logs, errors or `Debug`.
//!
//! Outbound rules (the operator owns the URL, but it is still guarded like the
//! OIDC fetcher): http(s) without credentials, query or fragment; the host is
//! resolved on every call, every answer must pass the address rule and the
//! connection is pinned to the checked address; no redirects, no proxy, one
//! 30 s deadline per call (source `EMBED_TIMEOUT_MS`) and a capped response.
//! Addresses: public targets must use https. Private and loopback targets
//! (a local model server) need `FVOCI_AI_EMBEDDINGS_ALLOW_PRIVATE=1`, and only
//! then may use plain http. Link-local (cloud metadata), unspecified and
//! multicast addresses and metadata host names are always refused. The
//! opt-in is an FVOCI hardening; the source calls any configured URL.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{json, Value};
use url::{Host, Url};

use crate::integrations::outbound::is_private_ip;
use crate::search::meili::EMBEDDING_DIMENSIONS;

pub const EMBED_TIMEOUT: Duration = Duration::from_secs(30);
/// Source `EMBED_BATCH`: chunks per provider call on the index-time pass.
pub const EMBED_BATCH: usize = 32;
pub const DEFAULT_EMBEDDINGS_MODEL: &str = "text-embedding-3-small";
/// 32 vectors x 1536 floats in JSON stay well below this.
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const URL_MAX: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EmbedError {
    #[error("embeddings url is not allowed")]
    Url,
    #[error("embeddings host resolves to a disallowed address")]
    Address,
    #[error("embeddings host did not resolve")]
    Resolve,
    #[error("embeddings request failed")]
    Transport,
    #[error("embeddings request timed out")]
    Timeout,
    #[error("embeddings HTTP {0}")]
    Status(u16),
    #[error("embeddings response exceeds byte limit")]
    TooLarge,
    #[error("embeddings response is malformed")]
    Body,
}

struct EmbedderInner {
    base_url: String,
    secret: Option<String>,
    model: String,
    allow_private: bool,
}

/// Configured embeddings provider. Cheap to clone.
#[derive(Clone)]
pub struct Embedder {
    inner: Arc<EmbedderInner>,
}

impl std::fmt::Debug for Embedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Embedder")
            .field("model", &self.inner.model)
            .field("secret", &self.inner.secret.as_ref().map(|_| "<redacted>"))
            .field("allow_private", &self.inner.allow_private)
            .finish()
    }
}

/// Raw `FVOCI_AI_*` values; `None` = unset.
#[derive(Debug, Default, Clone, Copy)]
pub struct EmbedderEnv<'a> {
    pub enabled: Option<&'a str>,
    pub secret: Option<&'a str>,
    pub base_url: Option<&'a str>,
    pub model: Option<&'a str>,
    pub dim: Option<&'a str>,
    pub allow_private: Option<&'a str>,
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

impl Embedder {
    /// Source `loadConfig` + `createEmbedder`: the dimension and URL are
    /// validated whether or not AI is enabled (a bad value refuses startup);
    /// the embedder exists only when enabled with a base URL.
    pub fn from_values(env: EmbedderEnv<'_>) -> Result<Option<Self>, String> {
        if let Some(raw) = non_empty(env.dim) {
            let dim: u32 =
                raw.parse().ok().filter(|d| *d > 0).ok_or_else(|| {
                    "FVOCI_AI_EMBEDDINGS_DIM must be a positive integer".to_string()
                })?;
            if dim != EMBEDDING_DIMENSIONS {
                return Err(format!(
                    "FVOCI_AI_EMBEDDINGS_DIM must be {EMBEDDING_DIMENSIONS} (search embedding dimensions)"
                ));
            }
        }
        let allow_private = match non_empty(env.allow_private) {
            None | Some("0") => false,
            Some("1") => true,
            Some(_) => return Err("FVOCI_AI_EMBEDDINGS_ALLOW_PRIVATE must be 0 or 1".into()),
        };
        let base_url = match non_empty(env.base_url) {
            None => None,
            Some(raw) => Some(parse_base_url(raw, allow_private)?),
        };
        let model = non_empty(env.model)
            .unwrap_or(DEFAULT_EMBEDDINGS_MODEL)
            .to_string();
        let enabled = env.enabled == Some("1");
        let (true, Some(base_url)) = (enabled, base_url) else {
            return Ok(None);
        };
        Ok(Some(Self {
            inner: Arc::new(EmbedderInner {
                base_url,
                secret: non_empty(env.secret).map(str::to_string),
                model,
                allow_private,
            }),
        }))
    }

    pub fn from_env() -> Result<Option<Self>, String> {
        let var = |name: &str| std::env::var(name).ok();
        let (enabled, secret, base_url, model, dim, allow_private) = (
            var("FVOCI_AI_ENABLED"),
            var("FVOCI_AI_SECRET"),
            var("FVOCI_AI_EMBEDDINGS_BASE_URL"),
            var("FVOCI_AI_EMBEDDINGS_MODEL"),
            var("FVOCI_AI_EMBEDDINGS_DIM"),
            var("FVOCI_AI_EMBEDDINGS_ALLOW_PRIVATE"),
        );
        Self::from_values(EmbedderEnv {
            enabled: enabled.as_deref(),
            secret: secret.as_deref(),
            base_url: base_url.as_deref(),
            model: model.as_deref(),
            dim: dim.as_deref(),
            allow_private: allow_private.as_deref(),
        })
    }

    pub fn model(&self) -> &str {
        &self.inner.model
    }

    /// Source `embedTexts`: one vector per input, in input order.
    pub async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        tokio::time::timeout(EMBED_TIMEOUT, self.embed_inner(inputs))
            .await
            .unwrap_or(Err(EmbedError::Timeout))
    }

    async fn embed_inner(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let url = Url::parse(&format!("{}/embeddings", self.inner.base_url))
            .map_err(|_| EmbedError::Url)?;
        let pinned = pin(&url, self.inner.allow_private).await?;
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(EMBED_TIMEOUT)
            .connect_timeout(EMBED_TIMEOUT)
            .pool_max_idle_per_host(0);
        if let Some(Host::Domain(name)) = url.host() {
            builder = builder.resolve(name, pinned);
        }
        let client = builder.build().map_err(|_| EmbedError::Transport)?;
        let mut request = client
            .post(url)
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .json(&json!({ "model": self.inner.model, "input": inputs }));
        if let Some(secret) = &self.inner.secret {
            request = request.bearer_auth(secret);
        }
        let response = request.send().await.map_err(transport)?;
        let status = response.status();
        if !status.is_success() {
            return Err(EmbedError::Status(status.as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|len| len > MAX_RESPONSE_BYTES as u64)
        {
            return Err(EmbedError::TooLarge);
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(transport)?;
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(EmbedError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        let payload: Value = serde_json::from_slice(&body).map_err(|_| EmbedError::Body)?;
        parse_embedding_response(&payload, inputs.len(), EMBEDDING_DIMENSIONS as usize)
    }
}

fn transport(err: reqwest::Error) -> EmbedError {
    if err.is_timeout() {
        EmbedError::Timeout
    } else {
        EmbedError::Transport
    }
}

fn host_name_blocked(host: &str) -> bool {
    let host = host.trim_end_matches('.');
    host == "metadata.google.internal"
        || host == "metadata.amazonaws.com"
        || host.ends_with(".arpa")
}

/// Always refused, even with the private opt-in: link-local (cloud metadata
/// at 169.254.169.254 / fe80::/10), unspecified, multicast and broadcast.
/// IPv4 addresses carried inside IPv6 forms that can route to them: mapped
/// (::ffff:0:0/96), compatible (::/96), NAT64 (64:ff9b::/96) and 6to4 (2002::/16).
fn embedded_ipv4(v6: std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    if let Some(v4) = v6.to_ipv4_mapped() {
        return Some(v4);
    }
    let bits = u128::from(v6);
    let low32 = std::net::Ipv4Addr::from((bits & 0xffff_ffff) as u32);
    if bits >> 32 == 0 && bits > 1 {
        return Some(low32);
    }
    if bits >> 32 == 0x0064_ff9b_0000_0000_0000_0000 {
        return Some(low32);
    }
    if (bits >> 112) == 0x2002 {
        return Some(std::net::Ipv4Addr::from(
            ((bits >> 80) & 0xffff_ffff) as u32,
        ));
    }
    None
}

fn never_allowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_link_local() || v4.is_unspecified() || v4.is_multicast() || v4.is_broadcast()
        }
        IpAddr::V6(v6) => {
            (u128::from(v6) >> 118) == 0x3fa
                || v6.is_unspecified()
                || v6.is_multicast()
                || embedded_ipv4(v6).is_some_and(|v4| never_allowed(IpAddr::V4(v4)))
        }
    }
}

/// `Ok(true)` = the address is private (needs the opt-in and may use http).
fn check_addr(ip: IpAddr, allow_private: bool) -> Result<bool, EmbedError> {
    if never_allowed(ip) {
        return Err(EmbedError::Address);
    }
    let v4_mapped_private = match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .is_some_and(|v4| is_private_ip(IpAddr::V4(v4))),
        IpAddr::V4(_) => false,
    };
    if is_private_ip(ip) || v4_mapped_private {
        if allow_private {
            return Ok(true);
        }
        return Err(EmbedError::Address);
    }
    Ok(false)
}

/// Startup shape check; literal addresses are checked here too.
fn parse_base_url(raw: &str, allow_private: bool) -> Result<String, String> {
    const MSG: &str = "FVOCI_AI_EMBEDDINGS_BASE_URL must be an http(s) URL without credentials, query or fragment";
    if raw.len() > URL_MAX {
        return Err(MSG.into());
    }
    let url = Url::parse(raw).map_err(|_| MSG.to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(MSG.into());
    }
    let literal = match url.host().ok_or_else(|| MSG.to_string())? {
        Host::Domain(name) => {
            if host_name_blocked(&name.to_ascii_lowercase()) {
                return Err(MSG.into());
            }
            None
        }
        Host::Ipv4(v4) => Some(IpAddr::V4(v4)),
        Host::Ipv6(v6) => Some(IpAddr::V6(v6)),
    };
    if let Some(ip) = literal {
        let private = check_addr(ip, allow_private).map_err(|_| {
            "FVOCI_AI_EMBEDDINGS_BASE_URL points at a private address; set FVOCI_AI_EMBEDDINGS_ALLOW_PRIVATE=1 for a local model server".to_string()
        })?;
        if url.scheme() == "http" && !private {
            return Err("FVOCI_AI_EMBEDDINGS_BASE_URL must use https for a public host".into());
        }
    }
    // Source strips trailing slashes before appending `/embeddings`.
    Ok(raw.trim_end_matches('/').to_string())
}

/// Resolves once; every answer must pass. Returns the address to connect to.
async fn pin(url: &Url, allow_private: bool) -> Result<SocketAddr, EmbedError> {
    let port = url.port_or_known_default().ok_or(EmbedError::Url)?;
    let answers: Vec<IpAddr> = match url.host().ok_or(EmbedError::Url)? {
        Host::Ipv4(v4) => vec![IpAddr::V4(v4)],
        Host::Ipv6(v6) => vec![IpAddr::V6(v6)],
        Host::Domain(name) => {
            if host_name_blocked(&name.to_ascii_lowercase()) {
                return Err(EmbedError::Url);
            }
            tokio::net::lookup_host((name, port))
                .await
                .map_err(|_| EmbedError::Resolve)?
                .map(|a| a.ip())
                .collect()
        }
    };
    let Some(first) = answers.first().copied() else {
        return Err(EmbedError::Resolve);
    };
    let mut all_private = true;
    for ip in &answers {
        all_private &= check_addr(*ip, allow_private)?;
    }
    if url.scheme() == "http" && !all_private {
        return Err(EmbedError::Url);
    }
    let ip = answers
        .iter()
        .copied()
        .find(IpAddr::is_ipv4)
        .unwrap_or(first);
    Ok(SocketAddr::new(ip, port))
}

/// Source `validateEmbedding`: exact length, finite as float32, non-zero norm.
pub fn validate_embedding(value: &Value, dimensions: usize) -> Option<Vec<f32>> {
    let items = value.as_array()?;
    if items.len() != dimensions {
        return None;
    }
    let mut out = Vec::with_capacity(dimensions);
    for item in items {
        let n = item.as_f64()? as f32;
        if !n.is_finite() {
            return None;
        }
        out.push(n);
    }
    out.iter().any(|n| *n != 0.0).then_some(out)
}

/// Source `parseEmbeddingResponse`: `data[]` of `{ index?, embedding }`, one
/// per input, ordered by `index` (position when absent). FVOCI additionally
/// requires the indices to be a permutation of the inputs.
pub fn parse_embedding_response(
    payload: &Value,
    expected: usize,
    dimensions: usize,
) -> Result<Vec<Vec<f32>>, EmbedError> {
    let data = payload
        .get("data")
        .and_then(Value::as_array)
        .ok_or(EmbedError::Body)?;
    if data.len() != expected {
        return Err(EmbedError::Body);
    }
    let mut slots: Vec<Option<Vec<f32>>> = vec![None; expected];
    for (position, entry) in data.iter().enumerate() {
        let index = match entry.get("index") {
            None => position,
            Some(raw) => raw
                .as_u64()
                .and_then(|i| usize::try_from(i).ok())
                .ok_or(EmbedError::Body)?,
        };
        let vector = entry
            .get("embedding")
            .and_then(|v| validate_embedding(v, dimensions))
            .ok_or(EmbedError::Body)?;
        let slot = slots.get_mut(index).ok_or(EmbedError::Body)?;
        if slot.replace(vector).is_some() {
            return Err(EmbedError::Body);
        }
    }
    slots
        .into_iter()
        .map(|slot| slot.ok_or(EmbedError::Body))
        .collect()
}

/// Stored `attachment_text.embedding` (jsonb) back to a vector; anything that
/// fails validation is treated as missing rather than sent to Meili.
pub fn embedding_from_json(value: Option<Value>) -> Option<Vec<f32>> {
    value.and_then(|v| validate_embedding(&v, EMBEDDING_DIMENSIONS as usize))
}

/// Compact jsonb text for a validated vector (shortest float32 digits).
pub fn embedding_to_json_text(vector: &[f32]) -> String {
    serde_json::to_string(vector).expect("f32 vector json")
}

#[cfg(test)]
mod tests {
    #[test]
    fn embedded_link_local_ipv4_is_never_allowed() {
        for addr in [
            "64:ff9b::a9fe:a9fe",
            "::169.254.169.254",
            "2002:a9fe:a9fe::1",
            "::ffff:169.254.169.254",
        ] {
            let ip: std::net::IpAddr = addr.parse().unwrap();
            assert!(super::never_allowed(ip), "{addr}");
        }
        let ok: std::net::IpAddr = "64:ff9b::0a00:0001".parse().unwrap();
        assert!(!super::never_allowed(ok));
    }

    use super::*;

    fn env<'a>(base: Option<&'a str>, private: Option<&'a str>) -> EmbedderEnv<'a> {
        EmbedderEnv {
            enabled: Some("1"),
            secret: Some("sk-test-secret"),
            base_url: base,
            model: None,
            dim: None,
            allow_private: private,
        }
    }

    #[test]
    fn config_requires_enabled_and_base_url() {
        assert!(Embedder::from_values(env(None, None)).unwrap().is_none());
        let mut off = env(Some("https://api.example.com/v1"), None);
        off.enabled = Some("true");
        assert!(Embedder::from_values(off).unwrap().is_none());
        let on = Embedder::from_values(env(Some("https://api.example.com/v1//"), None))
            .unwrap()
            .expect("embedder");
        assert_eq!(on.inner.base_url, "https://api.example.com/v1");
        assert_eq!(on.model(), DEFAULT_EMBEDDINGS_MODEL);
        assert_eq!(on.inner.secret.as_deref(), Some("sk-test-secret"));
    }

    #[test]
    fn config_rejects_wrong_dimension_even_when_disabled() {
        let mut bad = EmbedderEnv {
            dim: Some("768"),
            ..EmbedderEnv::default()
        };
        let err = Embedder::from_values(bad).unwrap_err();
        assert!(err.contains("must be 1536"), "{err}");
        bad.dim = Some("1536");
        assert!(Embedder::from_values(bad).unwrap().is_none());
        bad.dim = Some("x");
        assert!(Embedder::from_values(bad).is_err());
    }

    #[test]
    fn config_url_rules() {
        for bad in [
            "ftp://api.example.com",
            "https://user:pw@api.example.com/v1",
            "https://api.example.com/v1?x=1",
            "https://api.example.com/v1#f",
            "http://api.example.com/v1",
            "http://8.8.8.8/v1",
            "https://metadata.google.internal/v1",
            "not a url",
        ] {
            if bad == "http://api.example.com/v1" {
                // A domain over http is decided per call from the resolved address.
                assert!(Embedder::from_values(env(Some(bad), None)).is_ok(), "{bad}");
                continue;
            }
            assert!(
                Embedder::from_values(env(Some(bad), None)).is_err(),
                "{bad} should be refused"
            );
        }
        let err = Embedder::from_values(env(Some("http://127.0.0.1:9/v1"), None)).unwrap_err();
        assert!(err.contains("FVOCI_AI_EMBEDDINGS_ALLOW_PRIVATE"), "{err}");
        assert!(
            Embedder::from_values(env(Some("http://127.0.0.1:9/v1"), Some("1")))
                .unwrap()
                .is_some()
        );
        for never in [
            "http://169.254.169.254/v1",
            "http://[fe80::1]/v1",
            "http://0.0.0.0/v1",
        ] {
            assert!(
                Embedder::from_values(env(Some(never), Some("1"))).is_err(),
                "{never}"
            );
        }
        assert!(Embedder::from_values(env(Some("https://api.example.com"), Some("yes"))).is_err());
    }

    #[test]
    fn debug_redacts_secret() {
        let e = Embedder::from_values(env(Some("https://api.example.com/v1"), None))
            .unwrap()
            .unwrap();
        let shown = format!("{e:?}");
        assert!(!shown.contains("sk-test-secret"), "{shown}");
        assert!(!shown.contains("api.example.com"), "{shown}");
    }

    fn vector(seed: f32) -> Value {
        json!((0..1536).map(|i| seed + i as f32).collect::<Vec<f32>>())
    }

    #[test]
    fn response_is_ordered_by_index_and_validated() {
        let payload = json!({ "data": [
            { "index": 1, "embedding": vector(2.0) },
            { "index": 0, "embedding": vector(1.0) },
        ]});
        let out = parse_embedding_response(&payload, 2, 1536).unwrap();
        assert_eq!(out[0][0], 1.0);
        assert_eq!(out[1][0], 2.0);
        let positional = json!({ "data": [{ "embedding": vector(3.0) }] });
        assert_eq!(
            parse_embedding_response(&positional, 1, 1536).unwrap()[0][0],
            3.0
        );

        for bad in [
            json!({}),
            json!({ "data": [{ "embedding": vector(1.0) }] }),
            json!({ "data": [{ "index": 0, "embedding": vector(1.0) }, { "index": 0, "embedding": vector(1.0) }] }),
            json!({ "data": [{ "index": 0, "embedding": [1.0, 2.0] }, { "index": 1, "embedding": vector(1.0) }] }),
            json!({ "data": [{ "index": 5, "embedding": vector(1.0) }, { "index": 1, "embedding": vector(1.0) }] }),
            json!({ "data": [{ "index": 0, "embedding": vec![0.0; 1536] }, { "index": 1, "embedding": vector(1.0) }] }),
        ] {
            assert_eq!(
                parse_embedding_response(&bad, 2, 1536),
                Err(EmbedError::Body),
                "{bad}"
            );
        }
        let huge = json!({ "data": [{ "embedding": json!(vec![1e300_f64; 1536]) }] });
        assert_eq!(
            parse_embedding_response(&huge, 1, 1536),
            Err(EmbedError::Body)
        );
    }

    #[test]
    fn stored_vector_round_trips() {
        let v: Vec<f32> = (0..1536).map(|i| i as f32 * 0.1).collect();
        let text = embedding_to_json_text(&v);
        let back = embedding_from_json(Some(serde_json::from_str(&text).unwrap())).unwrap();
        assert_eq!(back, v);
        assert!(embedding_from_json(Some(json!([1.0]))).is_none());
        assert!(embedding_from_json(None).is_none());
    }

    #[test]
    fn address_rule() {
        let public: IpAddr = "8.8.8.8".parse().unwrap();
        let private: IpAddr = "10.0.0.5".parse().unwrap();
        let mapped: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert_eq!(check_addr(public, false), Ok(false));
        assert_eq!(check_addr(private, false), Err(EmbedError::Address));
        assert_eq!(check_addr(private, true), Ok(true));
        assert_eq!(check_addr(mapped, false), Err(EmbedError::Address));
        assert_eq!(
            check_addr("169.254.169.254".parse().unwrap(), true),
            Err(EmbedError::Address)
        );
    }
}
