use std::net::SocketAddr;

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

pub fn check_origin(headers: &HeaderMap, public_origin: &str) -> Result<(), AppError> {
    let origin = headers
        .get("origin")
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
    use super::resolve_public_origin;

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
