use std::env;
use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use crate::attachments::UploadLimits;
use crate::auth::password::Keyring;
use crate::search::meili::{meili_config_from_env, MeiliConfig};

pub const DEFAULT_UPLOAD_PART_SIZE_BYTES: i64 = 32 * 1024 * 1024;
pub const DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES: i64 = 5120_i64 * 1024 * 1024;
pub const DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN: u32 = 120;

pub struct Config {
    pub bind: SocketAddr,
    pub app_database_url: String,
    pub password_keys: Keyring,
    pub branding_name: String,
    pub public_origin: String,
    pub cookie_secure: bool,
    pub static_dir: Option<PathBuf>,
    pub storage_root: PathBuf,
    pub upload: UploadLimits,
    /// Wall deadline covering HTTP drain, hub join, and pool close after the stop signal.
    pub shutdown_deadline: Duration,
    /// Present when `FVOCI_MEILI_URL` is set. The API key is never logged.
    pub meili: Option<MeiliConfig>,
    /// SMTP_HOST/PORT/FROM all set, or None when mail is disabled.
    pub smtp: Option<crate::mail::SmtpConfig>,
}

impl Clone for Config {
    fn clone(&self) -> Self {
        Self {
            bind: self.bind,
            app_database_url: self.app_database_url.clone(),
            password_keys: self.password_keys.clone(),
            branding_name: self.branding_name.clone(),
            public_origin: self.public_origin.clone(),
            cookie_secure: self.cookie_secure,
            static_dir: self.static_dir.clone(),
            storage_root: self.storage_root.clone(),
            upload: self.upload.clone(),
            shutdown_deadline: self.shutdown_deadline,
            meili: self.meili.clone(),
            smtp: self.smtp.clone(),
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("app_database_url", &"<redacted>")
            .field("branding_name", &self.branding_name)
            .field("public_origin", &self.public_origin)
            .field("cookie_secure", &self.cookie_secure)
            .field("static_dir", &self.static_dir)
            .field("storage_root", &self.storage_root)
            .field("upload", &self.upload)
            .field("shutdown_deadline", &self.shutdown_deadline)
            .field("meili", &self.meili)
            .field("smtp", &self.smtp.as_ref().map(|_| "<configured>"))
            .finish()
    }
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let bind = env::var("FVOCI_BIND")
            .unwrap_or_else(|_| "127.0.0.1:0".to_string())
            .parse()
            .map_err(|e| format!("invalid FVOCI_BIND: {e}"))?;

        if env::var("DATABASE_URL").is_ok() || env::var("FVOCI_MIGRATION_URL").is_ok() {
            tracing::warn!(
                "DATABASE_URL/FVOCI_MIGRATION_URL is set but ignored by fvoci-server; use fvoci-migrate for schema changes"
            );
        }

        let app_database_url = env::var("DATABASE_APP_URL")
            .or_else(|_| env::var("FVOCI_APP_DATABASE_URL"))
            .map_err(|_| "DATABASE_APP_URL is required".to_string())?;

        let pepper_keys = env::var("PASSWORD_PEPPER_KEYS").map_err(|_| {
            "PASSWORD_PEPPER_KEYS is required (JSON map of key id to 64-char hex)".to_string()
        })?;
        let pepper_active = env::var("PASSWORD_PEPPER_ACTIVE_KEY_ID")
            .map_err(|_| "PASSWORD_PEPPER_ACTIVE_KEY_ID is required".to_string())?;
        let password_keys = Keyring::parse(&pepper_keys, &pepper_active)?;

        let branding_name = env::var("FVOCI_BRANDING_NAME").unwrap_or_else(|_| "FVOCI".to_string());
        let public_origin_raw =
            env::var("FVOCI_PUBLIC_ORIGIN").unwrap_or_else(|_| "http://localhost:5173".to_string());
        let public_origin = crate::http::guard::normalize_public_origin(&public_origin_raw)?;
        let cookie_secure = env::var("FVOCI_COOKIE_SECURE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(public_origin.starts_with("https://"));

        let static_dir = match env::var("FVOCI_STATIC_DIR") {
            Ok(value) if !value.trim().is_empty() => {
                Some(crate::http::static_assets::validate_static_root(
                    PathBuf::from(value.trim()).as_path(),
                )?)
            }
            _ => None,
        };

        let storage_root = required_storage_root_from_env()?;
        let upload = required_upload_limits_from_env()?;

        let shutdown_deadline =
            parse_shutdown_deadline_ms(env::var("FVOCI_SHUTDOWN_DEADLINE_MS").ok().as_deref())?;
        let meili = meili_config_from_env()?;
        let smtp = crate::mail::smtp_from_env()?;

        Ok(Self {
            bind,
            app_database_url,
            password_keys,
            branding_name,
            public_origin,
            cookie_secure,
            static_dir,
            storage_root,
            upload,
            shutdown_deadline,
            meili,
            smtp,
        })
    }
}

fn required_storage_root_from_env() -> Result<PathBuf, String> {
    let path = storage_root_path_from_values(
        env::var("FVOCI_STORAGE_DIR").ok().as_deref(),
        env::var("STORAGE_LOCAL_PATH").ok().as_deref(),
    )?;
    std::fs::create_dir_all(&path)
        .map_err(|e| format!("failed to create storage root {}: {e}", path.display()))?;
    Ok(path)
}

fn storage_root_path_from_values(
    fvoci_storage_dir: Option<&str>,
    storage_local_path: Option<&str>,
) -> Result<PathBuf, String> {
    let raw = fvoci_storage_dir.or(storage_local_path).ok_or_else(|| {
        "FVOCI_STORAGE_DIR or STORAGE_LOCAL_PATH is required and must be a nonempty path"
            .to_string()
    })?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(
            "FVOCI_STORAGE_DIR or STORAGE_LOCAL_PATH is required and must be a nonempty path"
                .into(),
        );
    }
    Ok(PathBuf::from(trimmed))
}

fn required_upload_limits_from_env() -> Result<UploadLimits, String> {
    upload_limits_from_values(
        env::var("FVOCI_UPLOAD_PART_SIZE_BYTES").ok().as_deref(),
        env::var("FVOCI_UPLOAD_MAX_FILE_SIZE_BYTES").ok().as_deref(),
        env::var("FVOCI_UPLOAD_CREATE_RATE_PER_5MIN")
            .ok()
            .as_deref(),
    )
}

fn upload_limits_from_values(
    part_size: Option<&str>,
    max_file_size: Option<&str>,
    create_rate: Option<&str>,
) -> Result<UploadLimits, String> {
    let part_size_bytes = parse_positive_i64(
        "FVOCI_UPLOAD_PART_SIZE_BYTES",
        part_size,
        DEFAULT_UPLOAD_PART_SIZE_BYTES,
    )?;
    let max_file_size_bytes = parse_positive_i64(
        "FVOCI_UPLOAD_MAX_FILE_SIZE_BYTES",
        max_file_size,
        DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
    )?;
    let create_rate_per_5min = parse_positive_u32(
        "FVOCI_UPLOAD_CREATE_RATE_PER_5MIN",
        create_rate,
        DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
    )?;
    if part_size_bytes > max_file_size_bytes {
        return Err(
            "FVOCI_UPLOAD_PART_SIZE_BYTES must be <= FVOCI_UPLOAD_MAX_FILE_SIZE_BYTES".into(),
        );
    }
    Ok(UploadLimits {
        part_size_bytes,
        max_file_size_bytes,
        create_rate_per_5min,
    })
}

fn parse_positive_i64(name: &str, raw: Option<&str>, default: i64) -> Result<i64, String> {
    let Some(raw) = raw else {
        return Ok(default);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("{name} must be a positive integer"));
    }
    let value: i64 = trimmed
        .parse()
        .map_err(|e| format!("invalid {name}: {e}"))?;
    if value <= 0 {
        return Err(format!("{name} must be a positive integer"));
    }
    Ok(value)
}

fn parse_positive_u32(name: &str, raw: Option<&str>, default: u32) -> Result<u32, String> {
    let Some(raw) = raw else {
        return Ok(default);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("{name} must be a positive integer"));
    }
    let value: u32 = trimmed
        .parse()
        .map_err(|e| format!("invalid {name}: {e}"))?;
    if value == 0 {
        return Err(format!("{name} must be a positive integer"));
    }
    Ok(value)
}

/// Default 30s; values below 1ms are rejected so expiry cannot be confused with success.
fn parse_shutdown_deadline_ms(raw: Option<&str>) -> Result<Duration, String> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(Duration::from_millis(30_000));
    };
    let millis: u64 = raw
        .parse()
        .map_err(|e| format!("invalid FVOCI_SHUTDOWN_DEADLINE_MS: {e}"))?;
    if millis == 0 {
        return Err("FVOCI_SHUTDOWN_DEADLINE_MS must be at least 1".into());
    }
    Ok(Duration::from_millis(millis))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_root_requires_explicit_nonempty_path() {
        let err = storage_root_path_from_values(None, None).unwrap_err();
        assert!(err.contains("FVOCI_STORAGE_DIR"));
        assert!(storage_root_path_from_values(Some("   "), None)
            .unwrap_err()
            .contains("nonempty"));
        assert!(storage_root_path_from_values(None, Some(""))
            .unwrap_err()
            .contains("nonempty"));
    }

    #[test]
    fn storage_root_accepts_source_alias_and_prefers_primary() {
        let alias = storage_root_path_from_values(None, Some("/tmp/fvoci-alias")).unwrap();
        assert_eq!(alias, PathBuf::from("/tmp/fvoci-alias"));
        let primary =
            storage_root_path_from_values(Some("/tmp/fvoci-primary"), Some("/tmp/fvoci-alias"))
                .unwrap();
        assert_eq!(primary, PathBuf::from("/tmp/fvoci-primary"));
        let again = storage_root_path_from_values(None, Some("/tmp/fvoci-alias")).unwrap();
        assert_eq!(again, alias);
    }

    #[test]
    fn invalid_upload_limits_fail_closed() {
        assert!(upload_limits_from_values(Some("0"), None, None)
            .unwrap_err()
            .contains("FVOCI_UPLOAD_PART_SIZE_BYTES"));
        assert!(upload_limits_from_values(Some("-1"), None, None).is_err());
        assert!(upload_limits_from_values(Some("nope"), None, None).is_err());
        assert!(upload_limits_from_values(Some("64"), Some("32"), None)
            .unwrap_err()
            .contains("must be <="));
        assert!(parse_positive_u32("FVOCI_UPLOAD_CREATE_RATE_PER_5MIN", Some("0"), 120).is_err());
    }

    #[test]
    fn omitted_upload_limits_use_defaults() {
        let limits = upload_limits_from_values(None, None, None).unwrap();
        assert_eq!(limits.part_size_bytes, DEFAULT_UPLOAD_PART_SIZE_BYTES);
        assert_eq!(
            limits.max_file_size_bytes,
            DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES
        );
        assert_eq!(
            limits.create_rate_per_5min,
            DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN
        );
    }
}
