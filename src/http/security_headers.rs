//! Global response security headers (source `http-security.ts`, nosecone 1.13
//! defaults with the application CSP).
//!
//! One layer stamps every response — API JSON, the SPA shell and assets, the
//! `/collab` upgrade — but only where the route has not set the header itself:
//! share pages, share fragments, attachment downloads and branding assets keep
//! their own stricter `Content-Security-Policy` (`sandbox` / nonce policies).
//!
//! Differences from the source, all in the stricter direction or invisible to
//! browsers: there is no `SERVER_INSECURE` switch that drops every header,
//! `Strict-Transport-Security` is sent only when the public origin is https
//! (browsers ignore it over http anyway), and a `Permissions-Policy` denies
//! device APIs the product never uses. Inline blocks in the shell are allowed
//! by build-time hashes (the source's shell policy); the Rust server renders no
//! other inline HTML under this policy, so there is no per-request nonce.

use std::path::Path;

use axum::http::header::{self, HeaderName, HeaderValue};
use axum::http::Response;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sha2::{Digest, Sha256};

/// Resolved header set; built once at router construction.
#[derive(Clone, Debug)]
pub struct SecurityHeaders {
    headers: Vec<(HeaderName, HeaderValue)>,
}

/// Device and payment features the product never uses. Embedded players keep
/// `fullscreen`/`autoplay` defaults.
const PERMISSIONS_POLICY: &str =
    "camera=(), microphone=(), geolocation=(), payment=(), usb=(), serial=(), hid=(), bluetooth=(), browsing-topics=()";

impl SecurityHeaders {
    /// `public_origin` decides HSTS and `upgrade-insecure-requests`;
    /// `static_dir` (when the SPA is served) supplies inline block hashes.
    pub fn new(public_origin: &str, static_dir: Option<&Path>) -> Self {
        let https = public_origin.starts_with("https://");
        let (script_hashes, style_hashes) = static_dir
            .and_then(|root| std::fs::read_to_string(root.join("index.html")).ok())
            .map(|html| {
                (
                    inline_hashes(&html, "script"),
                    inline_hashes(&html, "style"),
                )
            })
            .unwrap_or_default();
        let csp = content_security_policy(https, &script_hashes, &style_hashes);

        let mut headers: Vec<(HeaderName, HeaderValue)> = vec![
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_str(&csp).expect("CSP is ASCII"),
            ),
            (
                HeaderName::from_static("cross-origin-opener-policy"),
                HeaderValue::from_static("same-origin"),
            ),
            (
                HeaderName::from_static("cross-origin-resource-policy"),
                HeaderValue::from_static("same-origin"),
            ),
            (
                HeaderName::from_static("origin-agent-cluster"),
                HeaderValue::from_static("?1"),
            ),
            (
                header::REFERRER_POLICY,
                HeaderValue::from_static("no-referrer"),
            ),
        ];
        if https {
            headers.push((
                header::STRICT_TRANSPORT_SECURITY,
                HeaderValue::from_static("max-age=31536000; includeSubDomains"),
            ));
        }
        headers.extend([
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
            (
                header::X_DNS_PREFETCH_CONTROL,
                HeaderValue::from_static("off"),
            ),
            (
                HeaderName::from_static("x-download-options"),
                HeaderValue::from_static("noopen"),
            ),
            (
                header::X_FRAME_OPTIONS,
                HeaderValue::from_static("SAMEORIGIN"),
            ),
            (
                HeaderName::from_static("x-permitted-cross-domain-policies"),
                HeaderValue::from_static("none"),
            ),
            (header::X_XSS_PROTECTION, HeaderValue::from_static("0")),
            (
                HeaderName::from_static("permissions-policy"),
                HeaderValue::from_static(PERMISSIONS_POLICY),
            ),
        ]);
        Self { headers }
    }

    /// Adds each header the response does not already carry.
    pub fn apply<B>(&self, response: &mut Response<B>) {
        let out = response.headers_mut();
        for (name, value) in &self.headers {
            if !out.contains_key(name) {
                out.insert(name.clone(), value.clone());
            }
        }
    }
}

/// Source directive list, in the source order. `connect-src 'self'` covers the
/// same-origin `/collab` WebSocket; S3 is proxied by the API (no browser
/// storage origin). `'wasm-unsafe-eval'` allows compiling wasm, not JS eval.
fn content_security_policy(https: bool, script: &[String], style: &[String]) -> String {
    let join = |base: &[&str], extra: &[String]| {
        base.iter()
            .map(|s| s.to_string())
            .chain(extra.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut directives = vec![
        "default-src 'self'".to_string(),
        "base-uri 'self'".to_string(),
        "font-src 'self' data:".to_string(),
        "form-action 'self'".to_string(),
        "frame-ancestors 'self'".to_string(),
        "img-src 'self' data: blob:".to_string(),
        "object-src 'none'".to_string(),
        format!("script-src {}", join(&["'self'", "'wasm-unsafe-eval'"], script)),
        "script-src-attr 'none'".to_string(),
        format!("style-src {}", join(&["'self'"], style)),
        "connect-src 'self'".to_string(),
        "frame-src 'self' blob: https://www.youtube.com https://player.vimeo.com https://www.figma.com"
            .to_string(),
    ];
    if https {
        directives.push("upgrade-insecure-requests".to_string());
    }
    directives
        .into_iter()
        .map(|d| format!("{d};"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `'sha256-…'` sources for inline `<script>`/`<style>` bodies of the shell
/// (source `inlineCspHashes`); `<script src=…>` is not inline.
pub fn inline_hashes(html: &str, tag: &str) -> Vec<String> {
    let pattern = format!(r"(?is)<{tag}\b([^>]*)>(.*?)</{tag}>");
    let re = regex::Regex::new(&pattern).expect("static pattern");
    let src = regex::Regex::new(r"(?i)\bsrc\s*=").expect("static pattern");
    re.captures_iter(html)
        .filter(|c| !src.is_match(&c[1]))
        .map(|c| {
            format!(
                "'sha256-{}'",
                STANDARD.encode(Sha256::digest(c[2].as_bytes()))
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_only_inline_blocks() {
        let html = r#"<head><script type="module" src="/assets/a.js"></script><script>x=1</script><style>a{}</style></head>"#;
        let scripts = inline_hashes(html, "script");
        assert_eq!(scripts.len(), 1);
        assert_eq!(
            scripts[0],
            format!("'sha256-{}'", STANDARD.encode(Sha256::digest(b"x=1")))
        );
        assert_eq!(inline_hashes(html, "style").len(), 1);
    }

    #[test]
    fn https_adds_hsts_and_upgrade() {
        let plain = SecurityHeaders::new("http://localhost:8080", None);
        assert!(!plain
            .headers
            .iter()
            .any(|(n, _)| n == header::STRICT_TRANSPORT_SECURITY));
        let tls = SecurityHeaders::new("https://fvoci.example", None);
        let csp = &tls
            .headers
            .iter()
            .find(|(n, _)| n == header::CONTENT_SECURITY_POLICY)
            .unwrap()
            .1;
        assert!(csp
            .to_str()
            .unwrap()
            .ends_with("upgrade-insecure-requests;"));
        assert!(tls
            .headers
            .iter()
            .any(|(n, _)| n == header::STRICT_TRANSPORT_SECURITY));
    }
}
