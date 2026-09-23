use regex::Regex;
use std::sync::LazyLock;

use crate::error::{AppError, ProblemCode};

static EMAIL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^@\s]+@[^@\s]+\.[^@\s]+$").expect("email regex"));
static SLUG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9-]{2,32}$").expect("slug regex"));

pub fn normalize_email(email: &str) -> Result<String, AppError> {
    let trimmed = email.trim();
    if trimmed.is_empty() || !EMAIL_RE.is_match(trimmed) {
        return Err(AppError::problem(
            axum::http::StatusCode::BAD_REQUEST,
            ProblemCode::InvalidInput,
        ));
    }
    Ok(trimmed.to_ascii_lowercase())
}

pub fn normalize_slug(slug: &str) -> Result<String, AppError> {
    let folded = slug.trim();
    if !SLUG_RE.is_match(folded) {
        return Err(AppError::problem(
            axum::http::StatusCode::BAD_REQUEST,
            ProblemCode::InvalidInput,
        ));
    }
    Ok(folded.to_string())
}

pub fn validate_locale(locale: &str) -> Result<(), AppError> {
    if locale == "ko" {
        Ok(())
    } else {
        Err(AppError::problem(
            axum::http::StatusCode::BAD_REQUEST,
            ProblemCode::InvalidInput,
        ))
    }
}

pub fn validate_text_scale(value: i16) -> Result<(), AppError> {
    if matches!(value, 16 | 18 | 20) {
        Ok(())
    } else {
        Err(AppError::problem(
            axum::http::StatusCode::BAD_REQUEST,
            ProblemCode::InvalidInput,
        ))
    }
}

pub fn validate_week_starts_on(value: i32) -> Result<(), AppError> {
    if matches!(value, 0 | 1) {
        Ok(())
    } else {
        Err(AppError::problem(
            axum::http::StatusCode::BAD_REQUEST,
            ProblemCode::InvalidInput,
        ))
    }
}

pub fn validate_given_name(value: &str) -> Result<(), AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 100 {
        return Err(AppError::problem(
            axum::http::StatusCode::BAD_REQUEST,
            ProblemCode::InvalidInput,
        ));
    }
    Ok(())
}

pub fn validate_family_name(value: &str) -> Result<(), AppError> {
    if value.len() > 100 {
        return Err(AppError::problem(
            axum::http::StatusCode::BAD_REQUEST,
            ProblemCode::InvalidInput,
        ));
    }
    Ok(())
}
