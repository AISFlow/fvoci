use std::env;
use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use crate::attachments::{PresignTtls, TransferMode, UploadLimits};
use crate::auth::password::Keyring;
use crate::search::meili::{meili_config_from_env, MeiliConfig};

pub const DEFAULT_UPLOAD_PART_SIZE_BYTES: i64 = 32 * 1024 * 1024;
pub const DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES: i64 = 5120_i64 * 1024 * 1024;
pub const DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN: u32 = 120;
pub const DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS: u32 = 64;
/// Twice the web client's part parallelism (3).
pub const DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS_PER_USER: u32 = 6;
pub const DEFAULT_UPLOAD_INCOMPLETE_TTL_HOURS: u64 = 24;
pub const DEFAULT_REVISION_KEEP: u32 = 200;
pub const DEFAULT_REVISION_SNAPSHOT_INTERVAL_HOURS: u32 = 24;

/// Automatic revision policy (source `packages/config`).
#[derive(Clone, Copy, Debug)]
pub struct RevisionSettings {
    /// `REVISION_SESSION_SNAPSHOT` — snapshot when the last collab client leaves (default on).
    pub session_snapshot_enabled: bool,
    /// `REVISION_KEEP` — retention cap for automatic rows (`session`/`scheduled` only).
    pub keep: u32,
    /// `REVISION_SNAPSHOT_INTERVAL_HOURS` — scheduled snapshot interval (`0` disables snapshots).
    pub snapshot_interval_hours: u32,
}

impl Default for RevisionSettings {
    fn default() -> Self {
        Self {
            session_snapshot_enabled: true,
            keep: DEFAULT_REVISION_KEEP,
            snapshot_interval_hours: DEFAULT_REVISION_SNAPSHOT_INTERVAL_HOURS,
        }
    }
}

pub fn revision_settings_from_env() -> RevisionSettings {
    let session_snapshot_enabled = match env::var("REVISION_SESSION_SNAPSHOT").ok() {
        None => true,
        Some(raw) => {
            let v = raw.trim();
            v == "1" || v.eq_ignore_ascii_case("true")
        }
    };
    let keep = env::var("REVISION_KEEP")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_REVISION_KEEP);
    let snapshot_interval_hours = env::var("REVISION_SNAPSHOT_INTERVAL_HOURS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_REVISION_SNAPSHOT_INTERVAL_HOURS);
    RevisionSettings {
        session_snapshot_enabled,
        keep,
        snapshot_interval_hours,
    }
}

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

/// Explicit connection configuration. PostgreSQL remains the default when
/// the selector is absent; a selected family never reads a PostgreSQL URL.
#[derive(Clone)]
pub enum DatabaseSettings {
    Postgres {
        app_url: String,
    },
    Sqlite {
        path: PathBuf,
    },
    LibsqlRemote {
        primary_url: String,
        auth_token: String,
    },
}

impl fmt::Debug for DatabaseSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // URLs may contain credentials, routing identifiers, or query tokens.
        f.write_str(match self {
            Self::Postgres { .. } => "Postgres(<configured>)",
            Self::Sqlite { .. } => "Sqlite(<configured>)",
            Self::LibsqlRemote { .. } => "LibsqlRemote(<configured>)",
        })
    }
}

impl DatabaseSettings {
    pub fn from_env() -> Result<Self, String> {
        let values = [
            "FVOCI_DATABASE_BACKEND",
            "DATABASE_APP_URL",
            "FVOCI_APP_DATABASE_URL",
            "FVOCI_SQLITE_PATH",
            "FVOCI_LIBSQL_URL",
            "FVOCI_LIBSQL_AUTH_TOKEN",
        ]
        .into_iter()
        .map(|name| Ok((name, env_unicode(name)?)))
        .collect::<Result<Vec<_>, String>>()?;
        Self::from_lookup(|name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .and_then(|(_, value)| value.clone())
        })
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let selected = get("FVOCI_DATABASE_BACKEND").unwrap_or_else(|| "postgres".into());
        let required = |name| {
            get(name)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| format!("{name} is required"))
        };
        let refuse = |names: &[&str]| -> Result<(), String> {
            for name in names {
                if get(name).is_some() {
                    return Err(format!("{name} conflicts with FVOCI_DATABASE_BACKEND"));
                }
            }
            Ok(())
        };
        match selected.as_str() {
            "postgres" => {
                refuse(&[
                    "FVOCI_SQLITE_PATH",
                    "FVOCI_LIBSQL_URL",
                    "FVOCI_LIBSQL_AUTH_TOKEN",
                ])?;
                let app_url = get("DATABASE_APP_URL")
                    .or_else(|| get("FVOCI_APP_DATABASE_URL"))
                    .filter(|value| !value.trim().is_empty())
                    .ok_or("DATABASE_APP_URL is required")?;
                Ok(Self::Postgres { app_url })
            }
            "sqlite" => {
                refuse(&[
                    "DATABASE_APP_URL",
                    "FVOCI_APP_DATABASE_URL",
                    "FVOCI_LIBSQL_URL",
                    "FVOCI_LIBSQL_AUTH_TOKEN",
                ])?;
                let path = PathBuf::from(required("FVOCI_SQLITE_PATH")?);
                if !path.is_absolute() || path.file_name().is_none() {
                    return Err(
                        "FVOCI_SQLITE_PATH must be an absolute persistent database file".into(),
                    );
                }
                Ok(Self::Sqlite { path })
            }
            "libsql-remote" => {
                refuse(&[
                    "DATABASE_APP_URL",
                    "FVOCI_APP_DATABASE_URL",
                    "FVOCI_SQLITE_PATH",
                ])?;
                let primary_url = required("FVOCI_LIBSQL_URL")?;
                let parsed = url::Url::parse(&primary_url)
                    .map_err(|_| "FVOCI_LIBSQL_URL must be a TLS primary endpoint")?;
                if !matches!(parsed.scheme(), "https" | "libsql")
                    || parsed.host_str().is_none()
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.fragment().is_some()
                {
                    return Err("FVOCI_LIBSQL_URL must be a TLS primary endpoint without user credentials or fragment".into());
                }
                Ok(Self::LibsqlRemote {
                    primary_url,
                    auth_token: required("FVOCI_LIBSQL_AUTH_TOKEN")?,
                })
            }
            _ => Err("FVOCI_DATABASE_BACKEND must be postgres, sqlite, or libsql-remote".into()),
        }
    }
}

pub struct Config {
    pub bind: SocketAddr,
    pub database: DatabaseSettings,
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
    pub revision: RevisionSettings,
    pub attachment_transfer: AttachmentTransferConfig,
}

impl Clone for Config {
    fn clone(&self) -> Self {
        Self {
            bind: self.bind,
            database: self.database.clone(),
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
            revision: self.revision,
            attachment_transfer: self.attachment_transfer,
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("database", &self.database)
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
            .field("revision", &self.revision)
            .field("attachment_transfer", &self.attachment_transfer)
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

        let database = DatabaseSettings::from_env()?;

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
        let revision = revision_settings_from_env();
        let attachment_transfer = attachment_transfer_from_values(
            env_unicode("FVOCI_ATTACHMENT_TRANSFER_MODE")?.as_deref(),
            env::var("FVOCI_ATTACHMENT_PRESIGN_PART_TTL_SECS")
                .ok()
                .as_deref(),
            env::var("FVOCI_ATTACHMENT_PRESIGN_DOWNLOAD_TTL_SECS")
                .ok()
                .as_deref(),
            &storage,
            &public_origin,
        )?;

        Ok(Self {
            bind,
            database,
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
            revision,
            attachment_transfer,
        })
    }
}

/// A variable that is unset or valid Unicode; anything else is refused
/// rather than read as unset.
fn env_unicode(name: &str) -> Result<Option<String>, String> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(format!("{name} must be valid UTF-8")),
    }
}

/// Startup view of the attachment transfer settings (#149). The mode itself
/// is resolved per request by the settings store
/// ([`crate::settings::attachment_transfer`]); startup proves that every mode
/// the store could apply is usable, so a bad environment value refuses to
/// start instead of being ignored with a warning.
#[derive(Clone, Copy, Debug, Default)]
pub struct AttachmentTransferConfig {
    /// `FVOCI_ATTACHMENT_TRANSFER_MODE`, when set to a non-empty value.
    pub env_mode: Option<TransferMode>,
    pub ttls: PresignTtls,
}

/// Validates the transfer variables against the storage and app origin.
///
/// # Errors
///
/// A message naming the offending variable when the mode is not exactly
/// `proxy` or `presigned`; when `presigned` is forced without
/// `STORAGE_DRIVER=s3` or `S3_PUBLIC_ENDPOINT`; when a TTL is outside its
/// range; or when `S3_PUBLIC_ENDPOINT` (checked whenever it is set, since an
/// admin can switch to `presigned` at run time) is plain http under an https
/// app origin (mixed content) or shares the app's host. Cookies are scoped to
/// the host and not the port (RFC 6265 §8.5), so a same-host storage origin
/// would receive the session cookie on browser navigations and image loads,
/// and it cannot send the `nosniff` and sandbox CSP headers the app origin
/// relies on for attachment bytes.
pub fn attachment_transfer_from_values(
    mode: Option<&str>,
    part_ttl: Option<&str>,
    download_ttl: Option<&str>,
    storage: &StorageSettings,
    public_origin: &str,
) -> Result<AttachmentTransferConfig, String> {
    const MODE_VAR: &str = "FVOCI_ATTACHMENT_TRANSFER_MODE";
    // Empty counts as unset, as in the settings store.
    let env_mode = match mode.filter(|raw| !raw.is_empty()) {
        None => None,
        Some(raw) => Some(
            TransferMode::parse(raw)
                .ok_or_else(|| format!("{MODE_VAR} must be \"proxy\" or \"presigned\""))?,
        ),
    };
    let ttls = PresignTtls {
        part: parse_ttl_secs(
            "FVOCI_ATTACHMENT_PRESIGN_PART_TTL_SECS",
            part_ttl,
            PresignTtls::default().part,
            5..=3600,
        )?,
        download: parse_ttl_secs(
            "FVOCI_ATTACHMENT_PRESIGN_DOWNLOAD_TTL_SECS",
            download_ttl,
            PresignTtls::default().download,
            5..=300,
        )?,
    };
    let storage_origin = match storage {
        StorageSettings::S3(s3) => crate::attachments::presign_origin_for(s3)?,
        StorageSettings::Local { .. } => None,
    };
    if let Some(storage_origin) = &storage_origin {
        let app = url::Url::parse(public_origin)
            .map_err(|e| format!("invalid FVOCI_PUBLIC_ORIGIN: {e}"))?;
        if app.scheme() == "https" && storage_origin.scheme() != "https" {
            return Err(
                "S3_PUBLIC_ENDPOINT must be https when FVOCI_PUBLIC_ORIGIN is https (browsers block mixed content)"
                    .into(),
            );
        }
        if app.host_str() == storage_origin.host_str() {
            return Err(
                "S3_PUBLIC_ENDPOINT must use a host other than FVOCI_PUBLIC_ORIGIN's: cookies ignore ports, and the storage origin cannot send the app's nosniff/sandbox headers"
                    .into(),
            );
        }
    }
    if env_mode == Some(TransferMode::Presigned) {
        match storage {
            StorageSettings::Local { .. } => {
                return Err(format!("{MODE_VAR}=presigned requires STORAGE_DRIVER=s3"));
            }
            StorageSettings::S3(_) if storage_origin.is_none() => {
                return Err(format!("{MODE_VAR}=presigned requires S3_PUBLIC_ENDPOINT"));
            }
            StorageSettings::S3(_) => {}
        }
    }
    Ok(AttachmentTransferConfig { env_mode, ttls })
}

fn parse_ttl_secs(
    name: &str,
    raw: Option<&str>,
    default: Duration,
    range: std::ops::RangeInclusive<u64>,
) -> Result<Duration, String> {
    let Some(raw) = raw else {
        return Ok(default);
    };
    let out_of_range = || {
        format!(
            "{name} must be an integer from {} to {} (seconds)",
            range.start(),
            range.end()
        )
    };
    let secs: u64 = raw.trim().parse().map_err(|_| out_of_range())?;
    if !range.contains(&secs) {
        return Err(out_of_range());
    }
    Ok(Duration::from_secs(secs))
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
/// Creates the local storage root. Only the server does this at start;
/// parsing the settings (doctor, migrate) never touches the filesystem.
pub fn ensure_storage_root(settings: &StorageSettings) -> Result<(), String> {
    if let StorageSettings::Local { root } = settings {
        std::fs::create_dir_all(root)
            .map_err(|e| format!("failed to create storage root {}: {e}", root.display()))?;
    }
    Ok(())
}

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

    fn database_settings(pairs: &[(&str, &str)]) -> Result<DatabaseSettings, String> {
        DatabaseSettings::from_lookup(|name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
        })
    }

    #[test]
    fn selected_database_never_uses_another_backends_credentials() {
        assert!(
            matches!(database_settings(&[("DATABASE_APP_URL", "postgres://fixture")]).unwrap(),
            DatabaseSettings::Postgres { app_url } if app_url == "postgres://fixture")
        );
        let sqlite = [
            ("FVOCI_DATABASE_BACKEND", "sqlite"),
            ("FVOCI_SQLITE_PATH", "/owned/wiki.sqlite"),
        ];
        assert!(
            matches!(database_settings(&sqlite).unwrap(), DatabaseSettings::Sqlite { path }
            if path.as_path() == std::path::Path::new("/owned/wiki.sqlite"))
        );
        for foreign in [
            "DATABASE_APP_URL",
            "FVOCI_APP_DATABASE_URL",
            "FVOCI_LIBSQL_URL",
            "FVOCI_LIBSQL_AUTH_TOKEN",
        ] {
            let mut mixed = sqlite.to_vec();
            mixed.push((foreign, "synthetic-secret"));
            let error = database_settings(&mixed).unwrap_err();
            assert!(error.contains(foreign));
            assert!(!error.contains("synthetic-secret"));
        }
        for path in ["", "relative.sqlite", ":memory:", "/"] {
            assert!(database_settings(&[
                ("FVOCI_DATABASE_BACKEND", "sqlite"),
                ("FVOCI_SQLITE_PATH", path)
            ])
            .is_err());
        }
        for selector in ["", "sqlite ", "replica", "turso", "unknown-secret"] {
            let error = database_settings(&[("FVOCI_DATABASE_BACKEND", selector)]).unwrap_err();
            assert!(!error.contains("unknown-secret"));
        }
    }

    #[test]
    fn selected_remote_requires_tls_primary_and_redacts_all_connection_inputs() {
        for endpoint in [
            "https://primary.example.test",
            "libsql://primary.example.test",
        ] {
            let remote = database_settings(&[
                ("FVOCI_DATABASE_BACKEND", "libsql-remote"),
                ("FVOCI_LIBSQL_URL", endpoint),
                ("FVOCI_LIBSQL_AUTH_TOKEN", "synthetic-secret"),
            ])
            .unwrap();
            let debug = format!("{remote:?}");
            assert!(!debug.contains(endpoint));
            assert!(!debug.contains("synthetic-secret"));
        }
        for endpoint in [
            "http://localhost",
            "file:/owned/db",
            "https://user:synthetic-secret@primary.example.test",
            "https://primary.example.test/#synthetic-secret",
            "invalid-synthetic-secret",
        ] {
            let error = database_settings(&[
                ("FVOCI_DATABASE_BACKEND", "libsql-remote"),
                ("FVOCI_LIBSQL_URL", endpoint),
                ("FVOCI_LIBSQL_AUTH_TOKEN", "synthetic-secret"),
            ])
            .unwrap_err();
            assert!(!error.contains("synthetic-secret"));
        }
        assert!(database_settings(&[
            ("FVOCI_DATABASE_BACKEND", "libsql-remote"),
            ("FVOCI_LIBSQL_URL", "https://primary.example.test")
        ])
        .is_err());
    }

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
    fn revision_session_snapshot_defaults_and_parses() {
        let defaults = revision_settings_from_env();
        assert!(defaults.session_snapshot_enabled);
        assert_eq!(defaults.keep, DEFAULT_REVISION_KEEP);
        assert_eq!(
            defaults.snapshot_interval_hours,
            DEFAULT_REVISION_SNAPSHOT_INTERVAL_HOURS
        );
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

    fn s3_with_public(public_endpoint: Option<&str>, path_style: bool) -> StorageSettings {
        StorageSettings::S3(S3Settings {
            endpoint: "http://minio.internal:9000".into(),
            public_endpoint: public_endpoint.map(str::to_string),
            region: "us-east-1".into(),
            bucket: "fvoci".into(),
            access_key_id: "id".into(),
            secret_access_key: "secret".into(),
            force_path_style: path_style,
        })
    }

    #[test]
    fn transfer_mode_env_is_exact_and_needs_a_presign_capable_storage() {
        let local = StorageSettings::Local {
            root: PathBuf::from("/tmp/x"),
        };
        let capable = s3_with_public(Some("http://files.example.test"), true);
        let origin = "http://app.example.test";
        let run = |mode: Option<&str>, storage: &StorageSettings| {
            attachment_transfer_from_values(mode, None, None, storage, origin)
        };
        assert_eq!(run(None, &local).unwrap().env_mode, None);
        assert_eq!(run(Some(""), &local).unwrap().env_mode, None);
        assert_eq!(
            run(Some("proxy"), &local).unwrap().env_mode,
            Some(TransferMode::Proxy)
        );
        assert_eq!(
            run(Some("presigned"), &capable).unwrap().env_mode,
            Some(TransferMode::Presigned)
        );
        for bad in ["direct", " presigned", "PROXY"] {
            let err = run(Some(bad), &capable).unwrap_err();
            assert!(err.contains("FVOCI_ATTACHMENT_TRANSFER_MODE"), "{err}");
        }
        let err = run(Some("presigned"), &local).unwrap_err();
        assert!(err.contains("STORAGE_DRIVER=s3"), "{err}");
        let err = run(Some("presigned"), &s3_with_public(None, true)).unwrap_err();
        assert!(err.contains("S3_PUBLIC_ENDPOINT"), "{err}");
        // Proxy needs no capability.
        assert!(run(Some("proxy"), &s3_with_public(None, true)).is_ok());
    }

    #[test]
    fn public_endpoint_must_be_https_under_https_and_on_another_host() {
        let check = |endpoint: &str, path_style: bool, origin: &str| {
            attachment_transfer_from_values(
                None,
                None,
                None,
                &s3_with_public(Some(endpoint), path_style),
                origin,
            )
        };
        let err = check(
            "http://files.example.test",
            true,
            "https://app.example.test",
        )
        .unwrap_err();
        assert!(err.contains("https"), "{err}");
        assert!(check(
            "https://files.example.test",
            true,
            "https://app.example.test"
        )
        .is_ok());
        // Same origin, and same host on another port (cookies ignore ports).
        for endpoint in ["http://app.example.test", "http://app.example.test:9000"] {
            let err = check(endpoint, true, "http://app.example.test").unwrap_err();
            assert!(err.contains("S3_PUBLIC_ENDPOINT"), "{err}");
        }
        // The app's port-0 placeholder is compared by host as well.
        assert!(check("http://127.0.0.1:9000", true, "http://127.0.0.1:0").is_err());
        assert!(check("http://localhost:9000", true, "http://127.0.0.1:0").is_ok());
        // Virtual-host style: browsers use `<bucket>.<host>`, another host.
        assert!(check("http://app.example.test", false, "http://app.example.test").is_ok());
    }

    #[test]
    fn presign_ttls_default_and_stay_in_range() {
        let local = StorageSettings::Local {
            root: PathBuf::from("/tmp/x"),
        };
        let ttls = |part: Option<&str>, download: Option<&str>| {
            attachment_transfer_from_values(None, part, download, &local, "http://a.test")
                .map(|c| c.ttls)
        };
        assert_eq!(ttls(None, None).unwrap(), PresignTtls::default());
        assert_eq!(ttls(None, None).unwrap().part, Duration::from_secs(900));
        assert_eq!(ttls(None, None).unwrap().download, Duration::from_secs(60));
        assert_eq!(
            ttls(Some("5"), Some("300")).unwrap(),
            PresignTtls {
                part: Duration::from_secs(5),
                download: Duration::from_secs(300)
            }
        );
        for (part, download, var) in [
            (Some("4"), None, "FVOCI_ATTACHMENT_PRESIGN_PART_TTL_SECS"),
            (Some("3601"), None, "FVOCI_ATTACHMENT_PRESIGN_PART_TTL_SECS"),
            (Some("x"), None, "FVOCI_ATTACHMENT_PRESIGN_PART_TTL_SECS"),
            (
                None,
                Some("301"),
                "FVOCI_ATTACHMENT_PRESIGN_DOWNLOAD_TTL_SECS",
            ),
            (None, Some(""), "FVOCI_ATTACHMENT_PRESIGN_DOWNLOAD_TTL_SECS"),
        ] {
            let err = ttls(part, download).unwrap_err();
            assert!(err.contains(var), "{err}");
        }
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
