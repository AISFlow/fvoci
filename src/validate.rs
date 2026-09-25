use regex::Regex;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

use crate::error::{AppError, ProblemCode};

static EMAIL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^@\s]+@[^@\s]+\.[^@\s]+$").expect("email regex"));
static SLUG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9-]{2,32}$").expect("slug regex"));

static ZOD_DATETIME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d{4}-\d{2}-\d{2})T((?:[01]\d|2[0-3]):[0-5]\d)(?::([0-5]\d)(?:\.(\d+))?)?Z$")
        .expect("iso datetime regex")
});

/// zod v4 `z.iso.datetime()`: UTC `Z` only, seconds and any fraction
/// optional, calendar-valid date. Parsed like JS `new Date` (milliseconds).
pub fn parse_iso_datetime(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let caps = ZOD_DATETIME_RE.captures(value)?;
    let date = crate::tasks::parse_iso_date(caps.get(1)?.as_str())?;
    let hm = caps.get(2)?.as_str();
    let hour: u32 = hm[..2].parse().ok()?;
    let minute: u32 = hm[3..5].parse().ok()?;
    let second: u32 = caps.get(3).map_or(Some(0), |m| m.as_str().parse().ok())?;
    let millis: u32 = caps.get(4).map_or(Some(0), |m| {
        let digits: String = m.as_str().chars().chain("000".chars()).take(3).collect();
        digits.parse().ok()
    })?;
    let time = chrono::NaiveTime::from_hms_milli_opt(hour, minute, second, millis)?;
    Some(chrono::DateTime::from_naive_utc_and_offset(
        date.and_time(time),
        chrono::Utc,
    ))
}

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

/// The instance `auth.passwordMinLength` setting raises the contract's floor
/// of 10 (source setup/invitation/reset routes).
pub async fn validate_password_setting(
    pool: &sqlx::PgPool,
    password: &str,
) -> Result<(), AppError> {
    validate_password_length(password)?;
    let min = crate::settings::current_values(pool, "FVOCI")
        .await
        .map_err(|err| {
            tracing::error!("settings read failed: {}", err);
            AppError::internal()
        })?
        .auth
        .password_min_length;
    if (utf16_len(password) as i64) < min {
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

#[cfg(test)]
mod iso_datetime_tests {
    use super::parse_iso_datetime;

    #[test]
    fn accepts_zod_iso_datetime_forms() {
        for ok in [
            "2026-09-26T00:00Z",
            "2026-09-26T00:00:00Z",
            "2026-09-26T00:00:00.123456Z",
            "2024-02-29T23:59:59Z",
        ] {
            assert!(parse_iso_datetime(ok).is_some(), "{ok}");
        }
        for bad in [
            "2026-09-26",
            "2026-09-26T00:00:00+09:00",
            "2026-09-26T24:00:00Z",
            "2025-02-29T00:00:00Z",
            "2026-09-26 00:00:00Z",
        ] {
            assert!(parse_iso_datetime(bad).is_none(), "{bad}");
        }
        assert_eq!(
            parse_iso_datetime("2026-09-26T01:02:03.4567Z")
                .unwrap()
                .timestamp_subsec_millis(),
            456
        );
    }
}
