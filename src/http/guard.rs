use std::net::SocketAddr;

use axum::http::header::ORIGIN;
use axum::http::HeaderMap;

use crate::error::{AppError, ProblemCode};

pub fn resolve_public_origin(public_origin: &str, bind_addr: SocketAddr) -> Result<String, String> {
    let parsed =
        url::Url::parse(public_origin).map_err(|e| format!("invalid public origin: {e}"))?;
    if parsed.port() == Some(0) {
        let mut resolved = parsed;
        resolved
            .set_port(Some(bind_addr.port()))
            .map_err(|_| "failed to set public origin port".to_string())?;
        return normalize_public_origin(resolved.as_str());
    }
    normalize_public_origin(public_origin)
}

pub fn normalize_public_origin(origin: &str) -> Result<String, String> {
    let parsed = url::Url::parse(origin).map_err(|e| format!("invalid public origin: {e}"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("public origin must use http or https".into());
    }
    if parsed.host_str().is_none() {
        return Err("public origin must include a host".into());
    }
    Ok(parsed.origin().ascii_serialization())
}

/// CSRF check of a state-changing request: a browser's `Origin` must be
/// this server's public origin. Routes call it themselves, before or after
/// reading the body as their status precedence requires.
///
/// An absent `Origin` passes: non-browser clients (API tokens, CLI, MCP) send
/// none, while browsers send one on every request whose method is not GET or
/// HEAD, so a cross-site POST, PUT, PATCH or DELETE from a browser carries
/// the foreign origin or `null`. A value that is not visible ASCII is
/// treated as absent. A present value must equal `public_origin` once both
/// are normalized (scheme, host, non-default port); `null` or an unparsable
/// value is a mismatch. The session cookie's `SameSite=Lax` is the second
/// layer. The OIDC POST starts (invite, link), which bind a provider
/// identity, use [`require_origin`].
///
/// # Errors
///
/// 403 `origin_mismatch`; 500 when `public_origin` itself does not parse.
pub fn check_origin(headers: &HeaderMap, public_origin: &str) -> Result<(), AppError> {
    let origin = headers
        .get(ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::trim);
    if origin.is_none() {
        return Ok(());
    }
    let expected = normalize_public_origin(public_origin)
        .map_err(|_| AppError::from_code(ProblemCode::InternalError))?;
    let actual = normalize_public_origin(origin.unwrap())
        .map_err(|_| AppError::from_code(ProblemCode::OriginMismatch))?;
    if actual == expected {
        Ok(())
    } else {
        Err(AppError::from_code(ProblemCode::OriginMismatch))
    }
}

/// [`check_origin`], except that an absent or unreadable `Origin` is also
/// `origin_mismatch`. For the POST starts of OIDC invite and link, which end
/// by binding a provider identity to an account (invite also replaces the
/// session).
/// Browsers send `Origin` on every POST (the SPA's `fetch` included), so only
/// a client that strips it, such as some privacy extensions, lacks one;
/// passing it would let such a cross-site post start the flow. Invite has no
/// session cookie, so this is its only CSRF gate.
pub fn require_origin(headers: &HeaderMap, public_origin: &str) -> Result<(), AppError> {
    if headers.get(ORIGIN).and_then(|v| v.to_str().ok()).is_none() {
        return Err(AppError::from_code(ProblemCode::OriginMismatch));
    }
    check_origin(headers, public_origin)
}

pub fn reject_bearer(headers: &HeaderMap) -> Result<(), AppError> {
    if let Some(value) = headers.get("authorization") {
        if value
            .to_str()
            .map(|v| v.trim().starts_with("Bearer "))
            .unwrap_or(false)
        {
            return Err(AppError::from_code(ProblemCode::NotFound));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    const PUBLIC: &str = "http://localhost:8080";

    fn with_origin(origin: Option<&[u8]>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(origin) = origin {
            headers.insert(ORIGIN, HeaderValue::from_bytes(origin).unwrap());
        }
        headers
    }

    fn code(result: Result<(), AppError>) -> Option<&'static str> {
        result.err().map(|err| err.code.as_str())
    }

    #[test]
    fn check_origin_passes_the_public_origin_and_an_absent_header() {
        for origin in [
            None,
            Some(&b"http://localhost:8080"[..]),
            Some(b"http://localhost:8080/"),
            Some(b"HTTP://LOCALHOST:8080"),
            // Not visible ASCII: treated as absent.
            Some(b"http://localhost:8080\xff"),
        ] {
            assert_eq!(
                code(check_origin(&with_origin(origin), PUBLIC)),
                None,
                "{origin:?}"
            );
        }
        let default_port = with_origin(Some(b"https://example.com:443"));
        assert_eq!(
            code(check_origin(&default_port, "https://example.com/")),
            None
        );
    }

    #[test]
    fn check_origin_refuses_another_or_opaque_origin() {
        for origin in [
            &b"http://localhost"[..],
            b"https://localhost:8080",
            b"http://evil.example:8080",
            b"null",
            b"not a url",
            b"",
        ] {
            assert_eq!(
                code(check_origin(&with_origin(Some(origin)), PUBLIC)),
                Some("origin_mismatch"),
                "{origin:?}"
            );
        }
        let headers = with_origin(Some(b"http://localhost:8080"));
        assert_eq!(
            code(check_origin(&headers, "no origin")),
            Some("internal_error")
        );
    }

    #[test]
    fn require_origin_also_refuses_an_absent_or_unreadable_header() {
        for origin in [None, Some(&b"http://localhost:8080\xff"[..])] {
            assert_eq!(
                code(require_origin(&with_origin(origin), PUBLIC)),
                Some("origin_mismatch"),
                "{origin:?}"
            );
        }
        let same = with_origin(Some(b"http://localhost:8080/"));
        assert_eq!(code(require_origin(&same, PUBLIC)), None);
        let other = with_origin(Some(b"http://evil.example:8080"));
        assert_eq!(
            code(require_origin(&other, PUBLIC)),
            Some("origin_mismatch")
        );
    }

    #[test]
    fn only_explicit_zero_port_follows_listener() {
        let bind = "127.0.0.1:43210".parse().unwrap();
        for (input, expected) in [
            ("http://localhost:0", "http://localhost:43210"),
            ("http://localhost:8080/", "http://localhost:8080"),
            ("http://localhost", "http://localhost"),
            ("https://localhost:443", "https://localhost"),
        ] {
            assert_eq!(resolve_public_origin(input, bind).unwrap(), expected);
        }
    }
}
