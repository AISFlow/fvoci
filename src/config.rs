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
pub const DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS: u32 = 64;
/// Twice the web client's part parallelism (3).
pub const DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS_PER_USER: u32 = 6;
pub const DEFAULT_UPLOAD_INCOMPLETE_TTL_HOURS: u64 = 24;

#[derive(Clone)]
pub struct S3Settings {
    pub endpoint: String,
    pub public_endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub force_path_style: bool,
}

impl fmt::Debug for S3Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Settings")
            .field("endpoint", &self.endpoint)
            .field("public_endpoint", &self.public_endpoint)
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .field("force_path_style", &self.force_path_style)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub enum StorageSettings {
    Local { root: PathBuf },
    S3(S3Settings),
}

pub struct Config {
    pub bind: SocketAddr,
    pub app_database_url: String,
    pub password_keys: Keyring,
    pub branding_name: String,
    pub public_origin: String,
    pub cookie_secure: bool,
    pub static_dir: Option<PathBuf>,
    pub storage: StorageSettings,
    pub upload: UploadLimits,
    pub upload_incomplete_ttl: Duration,
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
            storage: self.storage.clone(),
            upload: self.upload.clone(),
            upload_incomplete_ttl: self.upload_incomplete_ttl,
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
            .field("storage", &self.storage)
            .field("upload", &self.upload)
            .field("upload_incomplete_ttl", &self.upload_incomplete_ttl)
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

        let storage = storage_settings_from_env()?;
        let upload = required_upload_limits_from_env()?;
        check_part_size_for_storage(&storage, &upload)?;
        let upload_incomplete_ttl =
            parse_incomplete_ttl_hours(env::var("UPLOAD_INCOMPLETE_TTL_HOURS").ok().as_deref())?;

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
            storage,
            upload,
            upload_incomplete_ttl,
            shutdown_deadline,
            meili,
            smtp,
        })
    }
}

fn nonempty_env(name: &str) -> Result<String, String> {
    let value = env::var(name).map_err(|_| format!("{name} is required when STORAGE_DRIVER=s3"))?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{name} is required when STORAGE_DRIVER=s3"));
    }
    Ok(trimmed.to_string())
}

fn parse_s3_endpoint(name: &str, raw: &str) -> Result<String, String> {
    let url = url::Url::parse(raw).map_err(|e| format!("invalid {name}: {e}"))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(format!(
            "{name} must be an HTTP(S) URL without credentials, query or fragment"
        ));
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(format!(
            "{name} must be an HTTP(S) URL without credentials, query or fragment"
        ));
    }
    Ok(raw.trim().trim_end_matches('/').to_string())
}

/// Attachment storage settings from `STORAGE_DRIVER` and its variables, as
/// the server reads them. Also used by `fvoci-migrate --verify-storage`.
pub fn storage_settings_from_env() -> Result<StorageSettings, String> {
    let driver = env::var("STORAGE_DRIVER")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "local".to_string());
    match driver.as_str() {
        "local" => {
            let path = storage_root_path_from_values(
                env::var("FVOCI_STORAGE_DIR").ok().as_deref(),
                env::var("STORAGE_LOCAL_PATH").ok().as_deref(),
            )?;
            std::fs::create_dir_all(&path)
                .map_err(|e| format!("failed to create storage root {}: {e}", path.display()))?;
            Ok(StorageSettings::Local { root: path })
        }
        "s3" => {
            let endpoint = parse_s3_endpoint("S3_ENDPOINT", &nonempty_env("S3_ENDPOINT")?)?;
            let public_endpoint = match env::var("S3_PUBLIC_ENDPOINT") {
                Ok(value) if !value.trim().is_empty() => {
                    Some(parse_s3_endpoint("S3_PUBLIC_ENDPOINT", value.trim())?)
                }
                _ => None,
            };
            Ok(StorageSettings::S3(S3Settings {
                endpoint,
                public_endpoint,
                region: nonempty_env("S3_REGION")?,
                bucket: nonempty_env("S3_BUCKET")?,
                access_key_id: nonempty_env("S3_ACCESS_KEY_ID")?,
                secret_access_key: nonempty_env("S3_SECRET_ACCESS_KEY")?,
                force_path_style: env::var("S3_FORCE_PATH_STYLE")
                    .map(|v| v.trim() != "0")
                    .unwrap_or(true),
            }))
        }
        other => Err(format!(
            "STORAGE_DRIVER must be \"local\" or \"s3\", got \"{other}\""
        )),
    }
}

/// S3 rejects non-final multipart parts under 5 MiB (source:
/// `UPLOAD_PART_SIZE_MB must be >= 5`). The local driver keeps its range.
pub const S3_MIN_PART_SIZE_BYTES: i64 = 5 * 1024 * 1024;
/// Largest part this server proxies to S3. S3 itself allows 5 GiB; parts are
/// streamed rather than buffered, but one PUT still holds an S3 connection
/// for its whole transfer, so the cap stays well below the protocol limit
/// (the source's default `UPLOAD_MAX_PART_SIZE_MB` is 100).
pub const S3_MAX_PART_SIZE_BYTES: i64 = 1024 * 1024 * 1024;

fn check_part_size_for_storage(
    storage: &StorageSettings,
    upload: &UploadLimits,
) -> Result<(), String> {
    if !matches!(storage, StorageSettings::S3(_)) {
        return Ok(());
    }
    if upload.part_size_bytes < S3_MIN_PART_SIZE_BYTES {
        return Err(format!(
            "FVOCI_UPLOAD_PART_SIZE_BYTES must be >= {S3_MIN_PART_SIZE_BYTES} (S3 minimum part size) when STORAGE_DRIVER=s3"
        ));
    }
    if upload.part_size_bytes > S3_MAX_PART_SIZE_BYTES {
        return Err(format!(
            "FVOCI_UPLOAD_PART_SIZE_BYTES must be <= {S3_MAX_PART_SIZE_BYTES} when STORAGE_DRIVER=s3"
        ));
    }
    Ok(())
}

fn parse_incomplete_ttl_hours(raw: Option<&str>) -> Result<Duration, String> {
    let hours = parse_positive_u64(
        "UPLOAD_INCOMPLETE_TTL_HOURS",
        raw,
        DEFAULT_UPLOAD_INCOMPLETE_TTL_HOURS,
    )?;
    Ok(Duration::from_secs(hours.saturating_mul(60 * 60)))
}

fn parse_positive_u64(name: &str, raw: Option<&str>, default: u64) -> Result<u64, String> {
    let Some(raw) = raw else {
        return Ok(default);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("{name} must be a positive integer"));
    }
    let value: u64 = trimmed
        .parse()
        .map_err(|e| format!("invalid {name}: {e}"))?;
    if value == 0 {
        return Err(format!("{name} must be a positive integer"));
    }
    Ok(value)
}

pub(crate) fn storage_root_path_from_values(
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
    let mut limits = upload_limits_from_values(
        env::var("FVOCI_UPLOAD_PART_SIZE_BYTES").ok().as_deref(),
        env::var("FVOCI_UPLOAD_MAX_FILE_SIZE_BYTES").ok().as_deref(),
        env::var("FVOCI_UPLOAD_CREATE_RATE_PER_5MIN")
            .ok()
            .as_deref(),
    )?;
    limits.part_put_slots = crate::attachments::PartPutSlots::with_per_user(
        parse_positive_u32(
            "FVOCI_UPLOAD_MAX_CONCURRENT_PARTS",
            env::var("FVOCI_UPLOAD_MAX_CONCURRENT_PARTS")
                .ok()
                .as_deref(),
            DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS,
        )?,
        parse_positive_u32(
            "FVOCI_UPLOAD_MAX_CONCURRENT_PARTS_PER_USER",
            env::var("FVOCI_UPLOAD_MAX_CONCURRENT_PARTS_PER_USER")
                .ok()
                .as_deref(),
            DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS_PER_USER,
        )?,
    );
    Ok(limits)
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
        part_put_slots: crate::attachments::PartPutSlots::new(DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS),
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

    #[test]
    fn s3_requires_minimum_part_size_but_local_does_not() {
        let small = upload_limits_from_values(Some("1024"), None, None).unwrap();
        let local = StorageSettings::Local {
            root: PathBuf::from("/tmp/x"),
        };
        assert!(check_part_size_for_storage(&local, &small).is_ok());
        let s3 = StorageSettings::S3(S3Settings {
            endpoint: "http://127.0.0.1:9000".into(),
            public_endpoint: None,
            region: "us-east-1".into(),
            bucket: "fvoci".into(),
            access_key_id: "id".into(),
            secret_access_key: "secret".into(),
            force_path_style: true,
        });
        assert!(check_part_size_for_storage(&s3, &small).is_err());
        let default = upload_limits_from_values(None, None, None).unwrap();
        assert!(check_part_size_for_storage(&s3, &default).is_ok());
        let at_cap = upload_limits_from_values(
            Some(&S3_MAX_PART_SIZE_BYTES.to_string()),
            Some(&(6 * S3_MAX_PART_SIZE_BYTES).to_string()),
            None,
        )
        .unwrap();
        assert!(check_part_size_for_storage(&s3, &at_cap).is_ok());
        // 5 GiB + 1 is above the S3 protocol limit; 2 GiB is above the cap.
        for part in [5 * 1024 * 1024 * 1024 + 1_i64, 2 * S3_MAX_PART_SIZE_BYTES] {
            let big = upload_limits_from_values(
                Some(&part.to_string()),
                Some(&(6 * 1024 * 1024 * 1024_i64).to_string()),
                None,
            )
            .unwrap();
            assert!(check_part_size_for_storage(&s3, &big).is_err());
            assert!(check_part_size_for_storage(&local, &big).is_ok());
        }
    }

    #[test]
    fn incomplete_ttl_defaults_and_rejects_zero() {
        assert_eq!(
            parse_incomplete_ttl_hours(None).unwrap(),
            Duration::from_secs(24 * 60 * 60)
        );
        assert!(parse_incomplete_ttl_hours(Some("0")).is_err());
        assert_eq!(
            parse_incomplete_ttl_hours(Some("2")).unwrap(),
            Duration::from_secs(2 * 60 * 60)
        );
    }

    #[test]
    fn s3_endpoint_rejects_credentials_query_and_fragment() {
        assert!(parse_s3_endpoint("S3_ENDPOINT", "http://127.0.0.1:9000").is_ok());
        assert!(parse_s3_endpoint("S3_ENDPOINT", "http://user:pass@127.0.0.1:9000").is_err());
        assert!(parse_s3_endpoint("S3_ENDPOINT", "http://127.0.0.1:9000/?x=1").is_err());
        assert!(parse_s3_endpoint("S3_ENDPOINT", "http://127.0.0.1:9000/#frag").is_err());
        assert_eq!(
            parse_s3_endpoint("S3_ENDPOINT", "http://127.0.0.1:9000/").unwrap(),
            "http://127.0.0.1:9000"
        );
    }

    #[test]
    fn s3_settings_debug_redacts_keys() {
        let settings = S3Settings {
            endpoint: "http://127.0.0.1:9000".into(),
            public_endpoint: None,
            region: "us-east-1".into(),
            bucket: "fvoci".into(),
            access_key_id: "access-secret".into(),
            secret_access_key: "secret-secret".into(),
            force_path_style: true,
        };
        let rendered = format!("{settings:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("access-secret"));
        assert!(!rendered.contains("secret-secret"));
    }
}
