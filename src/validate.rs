use regex::Regex;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

use crate::error::{AppError, ProblemCode};

static EMAIL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^@\s]+@[^@\s]+\.[^@\s]+$").expect("email regex"));
static SLUG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9-]{2,32}$").expect("slug regex"));

pub fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

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
    let folded: String = slug.nfkc().collect();
    if !SLUG_RE.is_match(&folded) {
        return Err(AppError::problem(
            axum::http::StatusCode::BAD_REQUEST,
            ProblemCode::InvalidInput,
        ));
    }
    Ok(folded)
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

pub fn validate_password_length(password: &str) -> Result<(), AppError> {
    if utf16_len(password) < 10 {
        return Err(AppError::problem(
            axum::http::StatusCode::BAD_REQUEST,
            ProblemCode::PasswordInvalid,
        ));
    }
    Ok(())
}

pub fn validate_given_name(value: &str) -> Result<(), AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || utf16_len(trimmed) > 100 {
        return Err(AppError::problem(
            axum::http::StatusCode::BAD_REQUEST,
            ProblemCode::InvalidInput,
        ));
    }
    Ok(())
}

pub fn validate_family_name(value: &str) -> Result<(), AppError> {
    if utf16_len(value) > 100 {
        return Err(AppError::problem(
            axum::http::StatusCode::BAD_REQUEST,
            ProblemCode::InvalidInput,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_matches_source_nfkc_without_case_folding_or_trimming() {
        assert_eq!(normalize_slug("ａｃｍｅ").unwrap(), "acme");
        assert_eq!(normalize_slug("ⓐⓑ").unwrap(), "ab");
        assert_eq!(normalize_slug("team-12").unwrap(), "team-12");
        for invalid in ["ACME", "ＡＣＭＥ", " acme", "acme ", "a", "한글"] {
            assert!(normalize_slug(invalid).is_err(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn password_minimum_counts_utf16_code_units() {
        assert!(validate_password_length("가나다라").is_err());
        assert!(validate_password_length("1234567890").is_ok());
        assert!(validate_password_length("🇺🇸🇺🇸").is_err());
    }

    #[test]
    fn name_max_counts_utf16_code_units() {
        let long = "가".repeat(101);
        assert!(validate_given_name(&long).is_err());
        assert!(validate_given_name(&"가".repeat(100)).is_ok());
    }
}
