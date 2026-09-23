use axum::http::header::ORIGIN;
use axum::http::HeaderMap;

use crate::http::guard::normalize_public_origin;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollabOriginError {
    Missing,
    NonUtf8,
    Multiple,
    Malformed,
    Mismatch,
}

/// Collab upgrade requires a present, single, valid browser Origin header.
/// Missing Origin is an explicit non-browser source policy (reject).
pub fn validate_collab_origin(
    headers: &HeaderMap,
    public_origin: &str,
) -> Result<(), CollabOriginError> {
    let mut values = headers.get_all(ORIGIN).iter();
    let first = values.next();
    if first.is_none() {
        return Err(CollabOriginError::Missing);
    }
    if values.next().is_some() {
        return Err(CollabOriginError::Multiple);
    }
    let raw = first
        .and_then(|v| v.to_str().ok())
        .map(str::trim);
    let origin = match raw {
        Some(value) if !value.is_empty() => value,
        _ => return Err(CollabOriginError::NonUtf8),
    };
    let expected = normalize_public_origin(public_origin).map_err(|_| CollabOriginError::Malformed)?;
    let actual = normalize_public_origin(origin).map_err(|_| CollabOriginError::Malformed)?;
    if actual == expected {
        Ok(())
    } else {
        Err(CollabOriginError::Mismatch)
    }
}
