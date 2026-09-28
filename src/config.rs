use std::env;
use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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
    pub revision: RevisionSettings,
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
            revision: self.revision,
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
            .field("revision", &self.revision)
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
        let revision = revision_settings_from_env();

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
            revision,
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

/// Secret settings that may instead be read from a file named by `<VAR>_FILE`.
pub const SECRET_FILE_VARS: &[&str] = &[
    "DATABASE_APP_URL",
    "PASSWORD_PEPPER_KEYS",
    "PASSWORD_PEPPER_ACTIVE_KEY_ID",
    "ENCRYPTION_KEYS",
    "ENCRYPTION_ACTIVE_KEY_ID",
];

/// Standalone Compose install: the one-shot `init` service
/// (`fvoci-migrate --install`) writes the server's settings into this
/// directory, one file per variable, and the server mounts it read-only.
/// A file here is used only when neither the variable, its `_FILE` form nor
/// an alias is set; both present is an error (as for `<VAR>_FILE`). The image
/// ships the directory empty, so other installs are unaffected.
pub const INSTALL_SETTINGS_DIR: &str = "/run/fvoci/secrets";

/// `(variable, file name in INSTALL_SETTINGS_DIR, other variables that also set it)`.
pub const INSTALL_SETTING_FILES: &[(&str, &str, &[&str])] = &[
    ("DATABASE_APP_URL", "database_app_url", &["FVOCI_APP_DATABASE_URL"]),
    ("PASSWORD_PEPPER_KEYS", "password_pepper_keys", &[]),
    (
        "PASSWORD_PEPPER_ACTIVE_KEY_ID",
        "password_pepper_active_key_id",
        &[],
    ),
    ("ENCRYPTION_KEYS", "encryption_keys", &[]),
    ("ENCRYPTION_ACTIVE_KEY_ID", "encryption_active_key_id", &[]),
    ("FVOCI_MEILI_URL", "meili_url", &[]),
    ("FVOCI_MEILI_KEY", "meili_api_key", &["FVOCI_MEILI_KEY_FILE"]),
];

/// File name of `var` in [`INSTALL_SETTINGS_DIR`].
pub fn install_setting_file(var: &str) -> &'static str {
    INSTALL_SETTING_FILES
        .iter()
        .find(|(name, _, _)| *name == var)
        .map(|(_, file, _)| *file)
        .unwrap_or_else(|| unreachable!("{var} is not an install setting"))
}

const SECRET_FILE_MAX_BYTES: u64 = 64 * 1024;

/// Reads one setting file: UTF-8, at most 64 KiB and nonempty after one
/// trailing newline is dropped. Errors name `label` and the path, never the
/// contents.
pub fn read_setting_file(label: &str, path: &Path) -> Result<String, String> {
    let read = || -> std::io::Result<Vec<u8>> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(SECRET_FILE_MAX_BYTES + 1)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    let bytes = read().map_err(|e| format!("{label}: cannot read {}: {e}", path.display()))?;
    if bytes.len() as u64 > SECRET_FILE_MAX_BYTES {
        return Err(format!("{label}: {} is larger than 64 KiB", path.display()));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("{label}: {} is not UTF-8", path.display()))?;
    let value = text
        .strip_suffix('\n')
        .map(|v| v.strip_suffix('\r').unwrap_or(v))
        .unwrap_or(&text);
    if value.trim().is_empty() {
        return Err(format!("{label}: {} is empty", path.display()));
    }
    Ok(value.to_string())
}

/// Resolves every `<VAR>_FILE` in [`SECRET_FILE_VARS`], then the files present
/// in `install_dir` (see [`INSTALL_SETTINGS_DIR`]). Setting both `<VAR>` and
/// `<VAR>_FILE`, or either of them while the install file exists, is an error;
/// an explicitly empty variable counts as set.
pub fn resolve_secret_files(
    get: impl Fn(&str) -> Option<std::ffi::OsString>,
    install_dir: &Path,
) -> Result<Vec<(&'static str, String)>, String> {
    let mut resolved = Vec::new();
    for &name in SECRET_FILE_VARS {
        let file_var = format!("{name}_FILE");
        let Some(path) = get(&file_var) else { continue };
        if get(name).is_some() {
            return Err(format!("{name} and {file_var} are both set; set only one"));
        }
        let value = read_setting_file(&file_var, &PathBuf::from(path))?;
        resolved.push((name, value));
    }
    for &(name, file, aliases) in INSTALL_SETTING_FILES {
        let path = install_dir.join(file);
        match std::fs::metadata(&path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{name}: cannot read {}: {e}", path.display())),
        }
        let file_var = format!("{name}_FILE");
        let set = std::iter::once(name)
            .chain(std::iter::once(file_var.as_str()))
            .chain(aliases.iter().copied())
            .find(|var| get(var).is_some());
        if let Some(var) = set {
            return Err(format!(
                "{var} is set and the install file {} exists; set only one",
                path.display()
            ));
        }
        resolved.push((name, read_setting_file(name, &path)?));
    }
    Ok(resolved)
}

/// Copies `<VAR>_FILE` contents and install setting files into `<VAR>` so
/// every existing reader sees one source. Call at the start of `main`, before
/// a runtime or any other thread exists (the environment is process-global).
pub fn load_secret_files() -> Result<(), String> {
    for (name, value) in resolve_secret_files(|k| env::var_os(k), Path::new(INSTALL_SETTINGS_DIR))? {
        env::set_var(name, value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_file_vars_resolve_and_refuse_ambiguity() {
        let dir = std::env::temp_dir().join(format!("fvoci-secret-file-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str, body: &[u8]| {
            let path = dir.join(name);
            std::fs::write(&path, body).unwrap();
            path.into_os_string()
        };
        let url = file("url", b"postgres://app:pw@db/fvoci\n");
        let keys = file("keys", br#"{"install":"00"}"#);
        let empty = file("empty", b"\n");
        let big = file("big", &vec![b'a'; 64 * 1024 + 1]);
        let lookup = |pairs: Vec<(&'static str, std::ffi::OsString)>| {
            move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone())
        };

        let none = dir.join("no-install");
        let got = resolve_secret_files(lookup(vec![
            ("DATABASE_APP_URL_FILE", url.clone()),
            ("ENCRYPTION_KEYS_FILE", keys.clone()),
            // Not in the allowlist: ignored, never read.
            ("DATABASE_URL_FILE", "/nonexistent".into()),
        ]), &none)
        .unwrap();
        assert_eq!(
            got,
            vec![
                ("DATABASE_APP_URL", "postgres://app:pw@db/fvoci".to_string()),
                ("ENCRYPTION_KEYS", r#"{"install":"00"}"#.to_string()),
            ]
        );

        let both = resolve_secret_files(lookup(vec![
            ("PASSWORD_PEPPER_KEYS", "x".into()),
            ("PASSWORD_PEPPER_KEYS_FILE", keys.clone()),
        ]), &none)
        .unwrap_err();
        assert!(both.contains("both set"), "{both}");
        let err =
            resolve_secret_files(lookup(vec![("PASSWORD_PEPPER_KEYS_FILE", empty)]), &none).unwrap_err();
        assert!(err.contains("is empty"), "{err}");
        let err =
            resolve_secret_files(lookup(vec![("PASSWORD_PEPPER_KEYS_FILE", big)]), &none).unwrap_err();
        assert!(err.contains("64 KiB"), "{err}");
        let err = resolve_secret_files(lookup(vec![(
            "DATABASE_APP_URL_FILE",
            dir.join("missing").into_os_string(),
        )]), &none)
        .unwrap_err();
        assert!(
            err.starts_with("DATABASE_APP_URL_FILE: cannot read"),
            "{err}"
        );
        assert!(!err.contains("postgres://"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn install_setting_files_fill_unset_variables_only() {
        let dir = std::env::temp_dir().join(format!("fvoci-install-dir-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("database_app_url"), "postgres://a:b@postgres:5432/fvoci\n").unwrap();
        std::fs::write(dir.join("meili_api_key"), "k".repeat(64)).unwrap();
        let lookup = |pairs: Vec<(&'static str, &'static str)>| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| std::ffi::OsString::from(v))
            }
        };

        let got = resolve_secret_files(lookup(vec![("FVOCI_MEILI_URL", "http://m:7700")]), &dir)
            .unwrap();
        assert_eq!(
            got,
            vec![
                ("DATABASE_APP_URL", "postgres://a:b@postgres:5432/fvoci".to_string()),
                ("FVOCI_MEILI_KEY", "k".repeat(64)),
            ]
        );
        // The variable, an alias or its _FILE form next to the install file is
        // ambiguous, including an explicitly empty value.
        for var in ["DATABASE_APP_URL", "FVOCI_APP_DATABASE_URL", "FVOCI_MEILI_KEY_FILE"] {
            let err = resolve_secret_files(lookup(vec![(var, "")]), &dir).unwrap_err();
            assert!(err.starts_with(&format!("{var} is set and the install file")), "{err}");
            assert!(!err.contains("postgres://a:b"), "{err}");
        }
        let url_file = dir.join("database_app_url");
        let err = resolve_secret_files(
            |k: &str| (k == "DATABASE_APP_URL_FILE").then(|| url_file.clone().into_os_string()),
            &dir,
        )
        .unwrap_err();
        assert!(err.starts_with("DATABASE_APP_URL_FILE is set and the install file"), "{err}");
        // No directory (every other install): nothing is read.
        assert!(resolve_secret_files(lookup(vec![]), &dir.join("absent"))
            .unwrap()
            .is_empty());
        std::fs::write(dir.join("encryption_keys"), "\n").unwrap();
        let err = resolve_secret_files(lookup(vec![]), &dir).unwrap_err();
        assert!(err.contains("is empty"), "{err}");
        assert_eq!(install_setting_file("FVOCI_MEILI_URL"), "meili_url");
        std::fs::remove_dir_all(dir).unwrap();
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
