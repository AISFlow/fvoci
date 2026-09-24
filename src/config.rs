use std::env;
use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use crate::auth::password::Keyring;

pub struct Config {
    pub bind: SocketAddr,
    pub migration_url: String,
    pub app_database_url: String,
    pub password_keys: Keyring,
    pub branding_name: String,
    pub public_origin: String,
    pub cookie_secure: bool,
    pub static_dir: Option<PathBuf>,
    /// Wall deadline covering HTTP drain, hub join, and pool close after the stop signal.
    pub shutdown_deadline: Duration,
}

impl Clone for Config {
    fn clone(&self) -> Self {
        Self {
            bind: self.bind,
            migration_url: self.migration_url.clone(),
            app_database_url: self.app_database_url.clone(),
            password_keys: self.password_keys.clone(),
            branding_name: self.branding_name.clone(),
            public_origin: self.public_origin.clone(),
            cookie_secure: self.cookie_secure,
            static_dir: self.static_dir.clone(),
            shutdown_deadline: self.shutdown_deadline,
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("migration_url", &"<redacted>")
            .field("app_database_url", &"<redacted>")
            .field("branding_name", &self.branding_name)
            .field("public_origin", &self.public_origin)
            .field("cookie_secure", &self.cookie_secure)
            .field("static_dir", &self.static_dir)
            .field("shutdown_deadline", &self.shutdown_deadline)
            .finish()
    }
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let bind = env::var("FVOCI_BIND")
            .unwrap_or_else(|_| "127.0.0.1:0".to_string())
            .parse()
            .map_err(|e| format!("invalid FVOCI_BIND: {e}"))?;

        let migration_url = env::var("DATABASE_URL")
            .or_else(|_| env::var("FVOCI_MIGRATION_URL"))
            .map_err(|_| "DATABASE_URL or FVOCI_MIGRATION_URL is required".to_string())?;

        let app_database_url = env::var("DATABASE_APP_URL")
            .or_else(|_| env::var("FVOCI_APP_DATABASE_URL"))
            .map_err(|_| {
                "DATABASE_APP_URL is required and must not fall back to migration URL".to_string()
            })?;

        if app_database_url == migration_url {
            return Err("DATABASE_APP_URL must differ from the migration owner URL".into());
        }

        let migration_role = database_role(&migration_url)?;
        let app_role = database_role(&app_database_url)?;
        if !migration_role.is_empty() && migration_role == app_role {
            return Err(
                "DATABASE_APP_URL must use a different database role than the migration owner"
                    .into(),
            );
        }

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

        let shutdown_deadline =
            parse_shutdown_deadline_ms(env::var("FVOCI_SHUTDOWN_DEADLINE_MS").ok().as_deref())?;

        Ok(Self {
            bind,
            migration_url,
            app_database_url,
            password_keys,
            branding_name,
            public_origin,
            cookie_secure,
            static_dir,
            shutdown_deadline,
        })
    }
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

fn database_role(url: &str) -> Result<String, String> {
    url::Url::parse(url)
        .map(|parsed| parsed.username().to_string())
        .map_err(|e| format!("invalid database url: {e}"))
}
