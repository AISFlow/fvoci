use axum::http::HeaderMap;

use crate::error::{AppError, ProblemCode};

pub fn check_origin(headers: &HeaderMap, public_origin: &str) -> Result<(), AppError> {
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .map(str::trim);
    if origin.is_none() {
        return Ok(());
    }
    if origin == Some(public_origin) {
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
